use super::*;
use crate::cache::kline::DiskKlineCache;
use crate::datamanager::DataManagerConfig;
use crate::errors::{Result, TqError};
use crate::types::{Kline, SeriesData, UpdateInfo};
use crate::websocket::WebSocketConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, pin_mut};
use reqwest::header::HeaderMap;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration as StdDuration;
use std::time::{SystemTime, UNIX_EPOCH};
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

    // 新增 kline_cache 和 disk_cache
    let kline_cache = Arc::new(RwLock::new(HashMap::new()));
    let test_cache_dir = env::temp_dir().join("tqsdk_rs_test_kline_cache");
    // 确保目录存在，且清空旧数据
    let _ = std::fs::remove_dir_all(&test_cache_dir); // 尝试清空，忽略错误
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir)));

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
        kline_cache,
        disk_cache,
        SeriesCachePolicy::default(),
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
async fn kline_history_should_not_merge_disk_cache_into_global_datamanager() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let mut api = SeriesAPI::new(Arc::clone(&dm), ws, auth);

    let symbol = "SHFE.au2602";
    let duration = 60_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("history_preheat_no_merge");
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir.clone())));
    disk_cache
        .append_klines(symbol, duration, &[sample_kline(100, 510.0), sample_kline(101, 511.0)])
        .expect("prepare disk cache should succeed");
    api.disk_cache = Arc::clone(&disk_cache);

    let epoch_before = dm.get_epoch();
    let _sub = api
        .kline_history(symbol, StdDuration::from_secs(60), 8, 100)
        .await
        .expect("kline_history subscribe should succeed");

    assert_eq!(
        dm.get_epoch(),
        epoch_before,
        "subscribe 阶段不应因缓存预热修改全局 DataManager epoch"
    );
    assert!(
        dm.get_by_path(&["klines", symbol, &duration.to_string()]).is_none(),
        "subscribe 阶段不应把磁盘缓存注入全局 DataManager"
    );

    let _ = std::fs::remove_dir_all(test_cache_dir);
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

    // 新增 kline_cache 和 disk_cache
    let kline_cache = Arc::new(RwLock::new(HashMap::new()));
    let test_cache_dir = env::temp_dir().join("tqsdk_rs_test_kline_cache_failure_test"); // 使用不同的临时目录
    let _ = std::fs::remove_dir_all(&test_cache_dir);
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir)));

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
        kline_cache,
        disk_cache,
        SeriesCachePolicy::default(),
    )
    .unwrap();

    assert!(sub.start().await.is_err());
    assert!(!*sub.running.read().await);
    assert!(sub.data_cb_id.lock().unwrap().is_none());
}

#[tokio::test]
async fn realtime_kline_should_persist_without_on_new_bar_callback() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));

    let symbol = "SHFE.au2602";
    let duration = 60_000_000_000i64;
    let chart_id = "chart_no_callback_persist";
    let test_cache_dir = unique_test_cache_dir("realtime_no_on_new_bar");
    let kline_cache = Arc::new(RwLock::new(HashMap::new()));
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir.clone())));
    let sub = SeriesSubscription::new(
        Arc::clone(&dm),
        ws,
        SeriesOptions {
            symbols: vec![symbol.to_string()],
            duration,
            view_width: 16,
            chart_id: Some(chart_id.to_string()),
            left_kline_id: None,
            focus_datetime: None,
            focus_position: None,
        },
        kline_cache,
        Arc::clone(&disk_cache),
        SeriesCachePolicy::default(),
    )
    .expect("create subscription should succeed");

    sub.start().await.expect("start should succeed");

    dm.merge_data(
        json!({
            "charts": {
                (chart_id): {
                    "left_id": 100,
                    "right_id": 100,
                    "more_data": false,
                    "ready": true
                }
            },
            "klines": {
                (symbol): {
                    (duration.to_string()): {
                        "last_id": 101,
                        "data": {
                            "100": {
                                "datetime": 100_000_000_000i64,
                                "open": 510.0,
                                "high": 512.0,
                                "low": 509.0,
                                "close": 511.0,
                                "volume": 1000,
                                "open_oi": 2000,
                                "close_oi": 2001
                            }
                        }
                    }
                }
            }
        }),
        true,
        false,
    );

    let mut persisted = false;
    for _ in 0..50 {
        if let Ok(Some(metadata)) = disk_cache.read_metadata(symbol, duration)
            && metadata.ranges.iter().any(|r| r.start == 100 && r.end == 101)
        {
            persisted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let _ = sub.close().await;
    let _ = std::fs::remove_dir_all(test_cache_dir);

    assert!(persisted, "即使没有 on_new_bar 回调，也应该写入闭合 K 线到磁盘缓存");
}

#[tokio::test]
async fn kline_data_series_by_id_returns_cached_data_without_network() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let mut api = SeriesAPI::new(dm, ws, auth);

    let symbol = "SHFE.au2602";
    let duration = 60_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_cache_hit");
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir.clone())));
    disk_cache
        .append_klines(
            symbol,
            duration,
            &[
                sample_kline(200, 700.0),
                sample_kline(201, 701.0),
                sample_kline(202, 702.0),
                sample_kline(203, 703.0),
            ],
        )
        .expect("prepare disk cache should succeed");
    api.disk_cache = Arc::clone(&disk_cache);

    let out = api
        .kline_data_series_by_id(symbol, StdDuration::from_secs(60), 4, 200)
        .await
        .expect("cache hit should not require websocket fetch");

    assert_eq!(out.len(), 4);
    assert_eq!(out[0].id, 200);
    assert_eq!(out[3].id, 203);

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn kline_data_series_by_id_ignores_disk_cache_when_cache_disabled() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let mut api = SeriesAPI::new_with_cache_policy(
        dm,
        ws,
        auth,
        SeriesCachePolicy {
            enabled: false,
            ..SeriesCachePolicy::default()
        },
    );

    let symbol = "SHFE.au2602";
    let duration = 60_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_cache_disabled");
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir.clone())));
    disk_cache
        .append_klines(
            symbol,
            duration,
            &[
                sample_kline(200, 700.0),
                sample_kline(201, 701.0),
                sample_kline(202, 702.0),
                sample_kline(203, 703.0),
            ],
        )
        .expect("prepare disk cache should succeed");
    api.disk_cache = Arc::clone(&disk_cache);

    let err = api
        .kline_data_series_by_id(symbol, StdDuration::from_secs(60), 4, 200)
        .await
        .expect_err("cache disabled should bypass disk cache and hit websocket fetch path");

    assert!(
        matches!(err, TqError::WebSocketError(_)),
        "expected websocket error for fetch path when cache disabled, got: {err:?}"
    );

    let _ = std::fs::remove_dir_all(test_cache_dir);
}

#[tokio::test]
async fn kline_data_series_by_id_cache_miss_should_trigger_fetch_path() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let api = SeriesAPI::new(dm, ws, auth);

    let err = api
        .kline_data_series_by_id("SHFE.au2602", StdDuration::from_secs(60), 3, 300)
        .await
        .expect_err("cache miss should attempt fetch and fail when websocket send fails");

    assert!(
        matches!(err, TqError::WebSocketError(_)),
        "expected websocket error for fetch path, got: {err:?}"
    );
}

#[tokio::test]
async fn kline_data_series_by_datetime_returns_cached_data_without_network() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let mut api = SeriesAPI::new(dm, ws, auth);

    let symbol = "SHFE.au2602";
    let duration = 1_000_000_000i64;
    let test_cache_dir = unique_test_cache_dir("data_series_datetime_cache_hit");
    let disk_cache = Arc::new(DiskKlineCache::new(Some(test_cache_dir.clone())));
    disk_cache
        .append_klines(
            symbol,
            duration,
            &[
                sample_kline(1000, 10.0),
                sample_kline(1001, 11.0),
                sample_kline(1002, 12.0),
                sample_kline(1003, 13.0),
            ],
        )
        .expect("prepare disk cache should succeed");
    api.disk_cache = Arc::clone(&disk_cache);

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
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
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
