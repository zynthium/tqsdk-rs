use super::*;
use crate::datamanager::DataManagerConfig;
use crate::errors::{Result, TqError};
use crate::types::{SeriesData, UpdateInfo};
use crate::websocket::WebSocketConfig;
use async_trait::async_trait;
use futures::{StreamExt, pin_mut};
use reqwest::header::HeaderMap;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration as StdDuration;
use tokio::time::{Duration, timeout};

#[test]
fn series_api_should_expose_kline_and_tick_methods() {
    fn _kline<'a>(
        api: &'a SeriesAPI,
        symbols: &'a str,
        duration: StdDuration,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.kline(symbols, duration, data_length))
    }

    fn _tick<'a>(
        api: &'a SeriesAPI,
        symbol: &'a str,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.tick(symbol, data_length))
    }

    let _ = (_kline, _tick);
}

#[derive(Default)]
struct TestAuth;

#[async_trait]
impl Authenticator for TestAuth {
    fn base_header(&self) -> HeaderMap {
        HeaderMap::new()
    }

    async fn login(&mut self) -> Result<()> {
        Ok(())
    }

    async fn get_td_url(&self, _broker_id: &str, _account_id: &str) -> Result<crate::auth::BrokerInfo> {
        Err(TqError::NotLoggedIn)
    }

    async fn get_md_url(&self, _stock: bool, _backtest: bool) -> Result<String> {
        Ok("wss://example.com".to_string())
    }

    fn has_feature(&self, _feature: &str) -> bool {
        false
    }

    fn has_md_grants(&self, _symbols: &[&str]) -> Result<()> {
        Ok(())
    }

    fn has_td_grants(&self, _symbol: &str) -> Result<()> {
        Ok(())
    }

    fn get_auth_id(&self) -> &str {
        ""
    }

    fn get_access_token(&self) -> &str {
        ""
    }
}

fn build_series_subscription(message_queue_capacity: usize) -> SeriesSubscription {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig {
            message_queue_capacity,
            ..WebSocketConfig::default()
        },
    ));

    SeriesSubscription::new(
        dm,
        ws,
        SeriesOptions {
            symbols: vec!["SHFE.au2602".to_string()],
            duration: 60_000_000_000,
            view_width: 32,
            chart_id: Some("chart_test".to_string()),
            left_kline_id: None,
            focus_datetime: None,
            focus_position: None,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn kline_returns_unstarted_subscription() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let api = SeriesAPI::new(dm, ws, auth);

    let sub = api.kline("SHFE.au2602", StdDuration::from_secs(60), 32).await.unwrap();

    assert!(!*sub.running.read().await);
    assert!(sub.data_cb_id.lock().unwrap().is_none());
}

#[tokio::test]
async fn start_failure_rolls_back_series_callback_registration() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();

    let sub = SeriesSubscription::new(
        dm,
        ws,
        SeriesOptions {
            symbols: vec!["SHFE.au2602".to_string()],
            duration: 60_000_000_000,
            view_width: 32,
            chart_id: Some("chart_test".to_string()),
            left_kline_id: None,
            focus_datetime: None,
            focus_position: None,
        },
    )
    .unwrap();

    assert!(sub.start().await.is_err());
    assert!(!*sub.running.read().await);
    assert!(sub.data_cb_id.lock().unwrap().is_none());
}

#[tokio::test]
async fn data_stream_does_not_override_on_update() {
    let sub = build_series_subscription(WebSocketConfig::default().message_queue_capacity);

    let fired = Arc::new(AtomicUsize::new(0));
    {
        let fired = Arc::clone(&fired);
        sub.on_update(move |_data, _info| {
            fired.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    }

    let stream = sub.data_stream().await;
    pin_mut!(stream);

    let data = Arc::new(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec!["SHFE.au2602".to_string()],
        single: None,
        multi: None,
        tick_data: None,
    });
    let info = Arc::new(UpdateInfo {
        chart_ready: true,
        ..UpdateInfo::default()
    });

    sub.emit_update(Arc::clone(&data), Arc::clone(&info)).await;

    assert_eq!(fired.load(Ordering::SeqCst), 1);

    let streamed = timeout(Duration::from_millis(100), stream.next())
        .await
        .expect("stream should receive update")
        .expect("stream item should exist");
    assert_eq!(streamed.symbols, data.symbols);
}

#[tokio::test]
async fn data_stream_is_bounded_and_drops_overflow_updates() {
    let sub = build_series_subscription(1);
    let stream = sub.data_stream().await;
    pin_mut!(stream);

    let first = Arc::new(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec!["FIRST".to_string()],
        single: None,
        multi: None,
        tick_data: None,
    });
    let second = Arc::new(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec!["SECOND".to_string()],
        single: None,
        multi: None,
        tick_data: None,
    });
    let info = Arc::new(UpdateInfo::default());

    sub.emit_update(Arc::clone(&first), Arc::clone(&info)).await;
    sub.emit_update(Arc::clone(&second), Arc::clone(&info)).await;

    let streamed = timeout(Duration::from_millis(100), stream.next())
        .await
        .expect("stream should receive first update")
        .expect("stream item should exist");
    assert_eq!(streamed.symbols, first.symbols);

    assert!(
        timeout(Duration::from_millis(50), stream.next()).await.is_err(),
        "second update should be dropped when stream queue is full"
    );
}

#[tokio::test]
async fn closed_data_stream_subscriber_is_pruned_on_dispatch() {
    let sub = build_series_subscription(1);

    let stream = sub.data_stream().await;
    assert_eq!(sub.stream_subscribers.read().await.len(), 1);
    drop(stream);

    let data = Arc::new(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec!["SHFE.au2602".to_string()],
        single: None,
        multi: None,
        tick_data: None,
    });
    let info = Arc::new(UpdateInfo::default());

    sub.emit_update(data, info).await;

    assert!(
        sub.stream_subscribers.read().await.is_empty(),
        "closed stream subscribers should be pruned after dispatch"
    );
}
