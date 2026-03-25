use super::*;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::websocket::WebSocketConfig;
use async_channel::bounded;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, Ordering as AtomicOrdering};
use tokio::sync::RwLock;

#[tokio::test]
async fn trade_session_callback_executes_outside_lock() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {}
                    }
                }
            }
        }),
        true,
        true,
    );

    let (order_tx, _order_rx) = bounded(8);
    let on_order: OrderCallback = Arc::new(RwLock::new(None));

    {
        let on_order_for_cb = Arc::clone(&on_order);
        *on_order.write().await = Some(Arc::new(move |_order: Order| {
            assert!(on_order_for_cb.try_write().is_ok());
        }));
    }

    TradeSession::process_order_update(&dm, "u", &order_tx, &on_order, -1).await;
}

#[tokio::test]
async fn trade_session_channel_overflow_drops_but_callbacks_still_fire() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {},
                        "o2": {}
                    }
                }
            }
        }),
        true,
        true,
    );

    let (order_tx, order_rx) = bounded(1);
    let fired = Arc::new(AtomicUsize::new(0));
    let on_order: OrderCallback = Arc::new(RwLock::new(None));
    {
        let fired = Arc::clone(&fired);
        *on_order.write().await = Some(Arc::new(move |_order: Order| {
            fired.fetch_add(1, AtomicOrdering::SeqCst);
        }));
    }

    TradeSession::process_order_update(&dm, "u", &order_tx, &on_order, -1).await;

    assert_eq!(fired.load(AtomicOrdering::SeqCst), 2);
    assert!(order_rx.try_recv().is_ok());
    assert!(order_rx.try_recv().is_err());
}

#[tokio::test]
async fn trade_session_close_resets_logged_in_state() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
    );

    session.running.store(true, Ordering::SeqCst);
    session.logged_in.store(true, Ordering::SeqCst);

    session.close().await.unwrap();

    assert!(!session.logged_in.load(Ordering::SeqCst));
}

#[tokio::test]
async fn trade_session_failed_connect_clears_stale_logged_in_state() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "not-a-valid-url".to_string(),
        WebSocketConfig::default(),
    );

    session.logged_in.store(true, Ordering::SeqCst);

    assert!(session.connect().await.is_err());
    assert!(!session.logged_in.load(Ordering::SeqCst));
}
