use super::*;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::types::{
    DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, PRICE_TYPE_LIMIT,
};
use crate::websocket::WebSocketConfig;
use async_channel::bounded;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, Ordering as AtomicOrdering};
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};

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

#[tokio::test]
async fn trade_session_failed_connect_does_not_keep_reconnecting_in_background() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "ws://127.0.0.1:1".to_string(),
        WebSocketConfig {
            reconnect_interval: Duration::from_millis(10),
            ..WebSocketConfig::default()
        },
    );

    let errors = Arc::new(AtomicUsize::new(0));
    {
        let errors = Arc::clone(&errors);
        session
            .on_error(move |_msg| {
                errors.fetch_add(1, AtomicOrdering::SeqCst);
            })
            .await;
    }

    session.ws.force_send_failure_for_test();
    assert!(session.connect().await.is_err());

    sleep(Duration::from_millis(120)).await;

    assert_eq!(errors.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn trade_commands_do_not_queue_when_transport_actor_is_missing() {
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

    session.logged_in.store(true, Ordering::SeqCst);
    session.ws.force_missing_io_actor_for_test();

    let req = InsertOrderRequest {
        symbol: "SHFE.au2602".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_LIMIT.to_string(),
        limit_price: 520.0,
        volume: 1,
    };

    let insert_res = session.insert_order(&req).await;
    assert!(insert_res.is_err(), "下单指令不应在断线竞态下排队补发");
    assert_eq!(session.ws.pending_queue_len_for_test().await, 0);

    let cancel_res = session.cancel_order("TQRS_TEST").await;
    assert!(cancel_res.is_err(), "撤单指令不应在断线竞态下排队补发");
    assert_eq!(session.ws.pending_queue_len_for_test().await, 0);
}
