use super::*;
use crate::cache::DataSeriesCache;
use crate::datamanager::DataManagerConfig;
use crate::errors::{Result, TqError};
use crate::marketdata::MarketDataState;
use crate::types::{Kline, SeriesData, SeriesSnapshot, Tick, UpdateInfo};
use crate::websocket::WebSocketConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use std::collections::{HashMap, HashSet};
use std::env;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{Duration, timeout};

#[test]
fn series_api_should_expose_get_serial_methods() {
    fn _get_kline_serial<'a>(
        api: &'a SeriesAPI,
        symbols: &'a str,
        duration: StdDuration,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.get_kline_serial(symbols, duration, data_length))
    }

    fn _get_tick_serial<'a>(
        api: &'a SeriesAPI,
        symbol: &'a str,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.get_tick_serial(symbol, data_length))
    }

    let _ = (_get_kline_serial, _get_tick_serial);
}

#[derive(Default)]
struct TestAuth {
    features: HashSet<String>,
}

impl TestAuth {
    fn with_features(features: &[&str]) -> Self {
        Self {
            features: features.iter().map(|feature| (*feature).to_string()).collect(),
        }
    }
}

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

    fn has_feature(&self, feature: &str) -> bool {
        self.features.contains(feature)
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

fn unique_test_cache_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("tqsdk_rs_series_{name}_{nanos}"))
}

fn sample_kline(id: i64, close: f64) -> Kline {
    Kline {
        id,
        datetime: id * 1_000_000_000,
        open: close - 1.0,
        close,
        high: close + 1.0,
        low: close - 2.0,
        open_oi: id * 10,
        close_oi: id * 10 + 1,
        volume: id * 100,
        epoch: None,
    }
}

fn sample_tick(id: i64, price: f64) -> Tick {
    Tick {
        id,
        datetime: id * 100,
        last_price: price,
        highest: price + 10.0,
        lowest: price - 10.0,
        average: price + 1.0,
        bid_price1: price - 1.0,
        bid_volume1: 11,
        bid_price2: price - 2.0,
        bid_volume2: 12,
        bid_price3: price - 3.0,
        bid_volume3: 13,
        bid_price4: price - 4.0,
        bid_volume4: 14,
        bid_price5: price - 5.0,
        bid_volume5: 15,
        ask_price1: price + 1.0,
        ask_volume1: 21,
        ask_price2: price + 2.0,
        ask_volume2: 22,
        ask_price3: price + 3.0,
        ask_volume3: 23,
        ask_price4: price + 4.0,
        ask_volume4: 24,
        ask_price5: price + 5.0,
        ask_volume5: 25,
        volume: id * 10,
        amount: price * 1000.0,
        open_interest: id * 100,
        epoch: None,
    }
}

async fn build_series_subscription(message_queue_capacity: usize) -> SeriesSubscription {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
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
    .await
    .unwrap()
}

fn sample_series_snapshot(symbol: &str, epoch: i64) -> SeriesSnapshot {
    SeriesSnapshot {
        data: Arc::new(SeriesData {
            is_multi: false,
            is_tick: false,
            symbols: vec![symbol.to_string()],
            single: None,
            multi: None,
            tick_data: None,
        }),
        update: Arc::new(UpdateInfo {
            chart_ready: true,
            has_new_bar: true,
            ..UpdateInfo::default()
        }),
        epoch,
    }
}

#[tokio::test]
async fn get_kline_serial_returns_started_subscription() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(dm, ws, auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 32)
        .await
        .unwrap();

    assert!(*sub.running.read().await);
    assert!(sub.watch_task_active_for_test());
    assert!(sub.snapshot().await.is_none());

    sub.close().await.unwrap();
}

#[tokio::test]
async fn get_kline_serial_creation_failure_rolls_back_series_watch_task() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(Arc::clone(&dm), Arc::clone(&ws), auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 32)
        .await;

    assert!(sub.is_err());
}

#[tokio::test]
async fn get_kline_serial_caps_data_length_at_10000() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(dm, ws, auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 20_000)
        .await
        .unwrap();

    assert_eq!(sub.options.view_width, 10_000);

    sub.close().await.unwrap();
}

#[tokio::test]
async fn kline_data_series_by_datetime_returns_cached_data_without_network() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::with_features(&["tq_dl"])));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: true,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let duration = 1_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_datetime_cache_hit");
    let data_series_cache = Arc::new(DataSeriesCache::new(Some(test_cache_dir.clone())));
    data_series_cache
        .write_kline_segment(
            symbol,
            duration,
            &[
                sample_kline(1000, 10.0),
                sample_kline(1001, 11.0),
                sample_kline(1002, 12.0),
                sample_kline(1003, 13.0),
            ],
        )
        .expect("prepare cache should succeed");
    api.data_series_cache = Arc::clone(&data_series_cache);

    let start_dt = DateTime::<Utc>::from_timestamp_nanos(1001 * 1_000_000_000);
    let end_dt = DateTime::<Utc>::from_timestamp_nanos(1003 * 1_000_000_000);
    let out = api
        .kline_data_series(symbol, StdDuration::from_secs(1), start_dt, end_dt)
        .await
        .expect("datetime cache hit should not require websocket fetch");

    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, 1001);
    assert_eq!(out[1].id, 1002);

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn kline_data_series_by_datetime_cache_miss_should_trigger_fetch_path() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::with_features(&["tq_dl"])));
    let api = SeriesAPI::new(dm, ws, auth);

    let start_dt = DateTime::<Utc>::from_timestamp_nanos(2_000_000_000);
    let end_dt = DateTime::<Utc>::from_timestamp_nanos(4_000_000_000);
    let err = api
        .kline_data_series("SHFE.au2602", StdDuration::from_secs(1), start_dt, end_dt)
        .await
        .expect_err("datetime cache miss should attempt fetch and fail when websocket send fails");

    assert!(
        matches!(err, TqError::WebSocketError(_)),
        "expected websocket error for fetch path, got: {err:?}"
    );
}

#[tokio::test]
async fn kline_data_series_cache_hit_is_not_blocked_by_concurrent_cache_miss_on_same_symbol() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::with_features(&["tq_dl"])));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: true,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let duration = 1_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_cache_lock_split");
    let data_series_cache = Arc::new(DataSeriesCache::new(Some(test_cache_dir.clone())));
    data_series_cache
        .write_kline_segment(
            symbol,
            duration,
            &[
                sample_kline(1000, 10.0),
                sample_kline(1001, 11.0),
                sample_kline(1002, 12.0),
                sample_kline(1003, 13.0),
            ],
        )
        .expect("prepare cache should succeed");
    api.data_series_cache = Arc::clone(&data_series_cache);
    let api = Arc::new(api);

    let slow_api = Arc::clone(&api);
    let slow = async move {
        slow_api
            .kline_data_series(
                symbol,
                StdDuration::from_secs(1),
                DateTime::<Utc>::from_timestamp_nanos(1000 * 1_000_000_000),
                DateTime::<Utc>::from_timestamp_nanos(1006 * 1_000_000_000),
            )
            .await
    };

    let fast_api = Arc::clone(&api);
    let fast = async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        timeout(
            Duration::from_millis(100),
            fast_api.kline_data_series(
                symbol,
                StdDuration::from_secs(1),
                DateTime::<Utc>::from_timestamp_nanos(1000 * 1_000_000_000),
                DateTime::<Utc>::from_timestamp_nanos(1003 * 1_000_000_000),
            ),
        )
        .await
    };

    let (slow_res, fast_res) = tokio::join!(slow, fast);

    let rows = fast_res
        .expect("cache hit should not wait behind unrelated download")
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![1000, 1001, 1002]
    );

    let slow_err = slow_res.expect_err("cache miss should still reach fetch path and time out in test");
    assert!(matches!(slow_err, TqError::Timeout));

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn tick_data_series_returns_cached_data_without_network() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::with_features(&["tq_dl"])));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: true,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let test_cache_dir = unique_test_cache_dir("tick_data_series_cache_hit");
    let data_series_cache = Arc::new(DataSeriesCache::new(Some(test_cache_dir.clone())));
    data_series_cache
        .write_tick_segment(
            symbol,
            &[
                sample_tick(1000, 10.0),
                sample_tick(1001, 11.0),
                sample_tick(1002, 12.0),
            ],
        )
        .expect("prepare tick cache should succeed");
    api.data_series_cache = Arc::clone(&data_series_cache);

    let out = api
        .tick_data_series(
            symbol,
            DateTime::<Utc>::from_timestamp_nanos(1000 * 100),
            DateTime::<Utc>::from_timestamp_nanos(1002 * 100),
        )
        .await
        .expect("tick cache hit should not require websocket fetch");

    assert_eq!(out.iter().map(|tick| tick.id).collect::<Vec<_>>(), vec![1000, 1001]);

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn tick_data_series_cache_miss_should_trigger_fetch_path() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::with_features(&["tq_dl"])));
    let api = SeriesAPI::new(dm, ws, auth);

    let err = api
        .tick_data_series(
            "SHFE.au2602",
            DateTime::<Utc>::from_timestamp_nanos(100),
            DateTime::<Utc>::from_timestamp_nanos(400),
        )
        .await
        .expect_err("tick cache miss should attempt fetch and fail when websocket send fails");

    assert!(
        matches!(err, TqError::WebSocketError(_)),
        "expected websocket error for tick fetch path, got: {err:?}"
    );
}

#[tokio::test]
async fn load_returns_error_before_first_snapshot() {
    let sub = build_series_subscription(WebSocketConfig::default().message_queue_capacity).await;

    assert!(sub.snapshot().await.is_none());
    assert!(matches!(sub.load().await, Err(TqError::DataNotFound(_))));
    assert!(timeout(Duration::from_millis(50), sub.wait_update()).await.is_err());
}

#[tokio::test]
async fn kline_data_series_requires_tq_dl_even_when_cache_hits() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: true,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let duration = 1_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_requires_tq_dl");
    let data_series_cache = Arc::new(DataSeriesCache::new(Some(test_cache_dir.clone())));
    data_series_cache
        .write_kline_segment(symbol, duration, &[sample_kline(1000, 10.0), sample_kline(1001, 11.0)])
        .expect("prepare cache should succeed");
    api.data_series_cache = Arc::clone(&data_series_cache);

    let err = api
        .kline_data_series(
            symbol,
            StdDuration::from_secs(1),
            DateTime::<Utc>::from_timestamp_nanos(1000 * 1_000_000_000),
            DateTime::<Utc>::from_timestamp_nanos(1002 * 1_000_000_000),
        )
        .await
        .expect_err("history api should reject when tq_dl is missing");

    assert!(matches!(err, TqError::PermissionDenied(_)));
    assert!(err.to_string().contains("专业版"));

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn replay_kline_data_series_bypasses_tq_dl_guard() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(dm, ws, auth);

    let err = api
        .replay_kline_data_series(
            "SHFE.au2602",
            StdDuration::from_secs(1),
            DateTime::<Utc>::from_timestamp_nanos(2_000_000_000),
            DateTime::<Utc>::from_timestamp_nanos(4_000_000_000),
        )
        .await
        .expect_err("replay history path should bypass tq_dl guard and reach fetch path");

    assert!(
        matches!(err, TqError::WebSocketError(_)),
        "expected websocket error for replay fetch path, got: {err:?}"
    );
}

#[tokio::test]
async fn tick_data_series_requires_tq_dl_even_when_cache_hits() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: true,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let test_cache_dir = unique_test_cache_dir("tick_data_series_requires_tq_dl");
    let data_series_cache = Arc::new(DataSeriesCache::new(Some(test_cache_dir.clone())));
    data_series_cache
        .write_tick_segment(symbol, &[sample_tick(1000, 10.0), sample_tick(1001, 11.0)])
        .expect("prepare tick cache should succeed");
    api.data_series_cache = Arc::clone(&data_series_cache);

    let err = api
        .tick_data_series(
            symbol,
            DateTime::<Utc>::from_timestamp_nanos(1000 * 100),
            DateTime::<Utc>::from_timestamp_nanos(1002 * 100),
        )
        .await
        .expect_err("history api should reject when tq_dl is missing");

    assert!(matches!(err, TqError::PermissionDenied(_)));
    assert!(err.to_string().contains("专业版"));

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn wait_update_and_load_track_latest_snapshot_state() {
    let sub = build_series_subscription(WebSocketConfig::default().message_queue_capacity).await;
    let wait_sub = sub.clone();
    let waiter = tokio::spawn(async move { wait_sub.wait_update().await.expect("wait_update should succeed") });

    tokio::task::yield_now().await;

    let snapshot = sample_series_snapshot("SHFE.au2602", 7);
    sub.emit_snapshot(snapshot.clone()).await;

    let received = timeout(Duration::from_millis(100), waiter)
        .await
        .expect("waiter should finish")
        .expect("waiter task should not panic");
    assert_eq!(received.epoch, 7);
    assert_eq!(received.data.symbols, snapshot.data.symbols);
    assert!(received.update.chart_ready);

    let loaded = sub.load().await.expect("load should return latest snapshot data");
    assert_eq!(loaded.symbols, snapshot.data.symbols);
}

#[tokio::test]
async fn series_subscription_start_and_close_manage_watch_task_lifecycle() {
    let sub = build_series_subscription(WebSocketConfig::default().message_queue_capacity).await;

    assert!(sub.watch_task_active_for_test());

    sub.close().await.expect("close should succeed");
    assert!(!sub.watch_task_active_for_test());
}

#[tokio::test]
async fn series_wait_update_after_close_returns_subscription_closed() {
    let sub = build_series_subscription(WebSocketConfig::default().message_queue_capacity).await;
    sub.close().await.expect("close should succeed");

    let err = sub
        .wait_update()
        .await
        .expect_err("wait_update should fail after close");
    assert!(matches!(err, TqError::SubscriptionClosed { .. }));
}

#[tokio::test]
async fn series_wait_update_close_wakes_already_blocked_waiter() {
    let sub = Arc::new(build_series_subscription(WebSocketConfig::default().message_queue_capacity).await);
    let waiter_sub = Arc::clone(&sub);
    let waiter = tokio::spawn(async move { waiter_sub.wait_update().await });

    tokio::task::yield_now().await;
    assert!(!waiter.is_finished(), "wait_update should be blocked before close");

    sub.close().await.expect("close should succeed");

    let result = timeout(Duration::from_millis(100), waiter)
        .await
        .expect("blocked waiter should be woken by close")
        .expect("waiter task should not panic");
    assert!(matches!(result, Err(TqError::SubscriptionClosed { .. })));
}
