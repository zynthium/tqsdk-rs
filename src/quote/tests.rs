use super::QuoteSubscription;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::marketdata::MarketDataState;
use crate::websocket::{TqQuoteWebsocket, WebSocketConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, sleep, timeout};

#[tokio::test]
async fn start_failure_rolls_back_quote_callback_registration() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();

    let sub = QuoteSubscription::new(dm, ws, vec!["SHFE.au2602".to_string()]);

    assert!(sub.start().await.is_err());
    assert!(!*sub.running.read().await);
    assert!(sub.data_cb_id.lock().unwrap().is_none());
}

#[tokio::test]
async fn quote_channel_is_bounded_and_drops_overflow_updates() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig {
            message_queue_capacity: 1,
            ..WebSocketConfig::default()
        },
    ));
    let sub = QuoteSubscription::new(dm.clone(), ws, vec!["SHFE.au2602".to_string()]);

    *sub.running.write().await = true;
    sub.start_watching().await;

    dm.merge_data(
        serde_json::json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:00.000000",
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        serde_json::json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:01.000000",
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );

    sleep(Duration::from_millis(30)).await;

    let rx = sub.quote_channel();
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn quote_channel_overflow_still_invokes_callback_for_each_update() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig {
            message_queue_capacity: 1,
            ..WebSocketConfig::default()
        },
    ));
    let sub = QuoteSubscription::new(dm.clone(), ws, vec!["SHFE.au2602".to_string()]);

    let fired = Arc::new(AtomicUsize::new(0));
    {
        let fired = Arc::clone(&fired);
        sub.on_quote(move |_quote| {
            fired.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    }

    *sub.running.write().await = true;
    sub.start_watching().await;

    dm.merge_data(
        serde_json::json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:00.000000",
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        serde_json::json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:01.000000",
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );
    timeout(Duration::from_millis(100), async {
        while fired.load(Ordering::SeqCst) < 1 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("first callback should arrive");

    dm.merge_data(
        serde_json::json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:01.000000",
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );

    timeout(Duration::from_millis(100), async {
        while fired.load(Ordering::SeqCst) < 2 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("second callback should arrive");

    assert_eq!(fired.load(Ordering::SeqCst), 2);

    let rx = sub.quote_channel();
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}
