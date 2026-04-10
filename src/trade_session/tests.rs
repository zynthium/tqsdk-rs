use super::*;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::types::{DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, PRICE_TYPE_LIMIT};
use crate::websocket::WebSocketConfig;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::time::{Duration, sleep, timeout};

fn build_dm() -> Arc<DataManager> {
    Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()))
}

fn build_session(dm: Arc<DataManager>) -> TradeSession {
    TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    )
}

fn order_json(order_id: &str, status: &str) -> serde_json::Value {
    json!({
        "order_id": order_id,
        "status": status,
        "direction": "BUY",
        "offset": "OPEN",
        "volume_orign": 1,
        "volume_left": if status == "FINISHED" { 0 } else { 1 },
        "price_type": "LIMIT",
        "limit_price": 520.0
    })
}

fn trade_json(order_id: &str, trade_id: &str) -> serde_json::Value {
    json!({
        "trade_id": trade_id,
        "order_id": order_id,
        "direction": "BUY",
        "offset": "OPEN",
        "volume": 1,
        "price": 520.0
    })
}

#[tokio::test]
async fn trade_session_emits_order_then_trade_events() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));
    let mut stream = session.subscribe_events();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    let first = stream.recv().await.unwrap();
    let second = stream.recv().await.unwrap();

    assert!(matches!(
        first.kind,
        TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
    ));
    assert!(matches!(
        second.kind,
        TradeSessionEventKind::TradeCreated { ref trade_id, .. } if trade_id == "t1"
    ));
}

#[tokio::test]
async fn trade_session_deduplicates_identical_order_snapshots() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));
    let mut stream = session.subscribe_events();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    let _ = stream.recv().await.unwrap();
    assert!(timeout(Duration::from_millis(50), stream.recv()).await.is_err());
}

#[tokio::test]
async fn trade_session_deduplicates_replayed_trade_ids() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));
    let mut stream = session.subscribe_events();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    let _ = stream.recv().await.unwrap();
    assert!(timeout(Duration::from_millis(50), stream.recv()).await.is_err());
}

#[tokio::test]
async fn trade_session_wait_order_update_reliable_wakes_on_order_event() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_order_update_reliable("o1").await })
    };
    tokio::task::yield_now().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    assert!(timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap().is_ok());
}

#[tokio::test]
async fn trade_session_wait_order_update_reliable_wakes_on_trade_event() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_order_update_reliable("o1").await })
    };
    tokio::task::yield_now().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    assert!(timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap().is_ok());
}

#[tokio::test]
async fn trade_session_close_resets_logged_in_state() {
    let dm = build_dm();
    let session = build_session(dm);

    session.running.store(true, Ordering::SeqCst);
    session.logged_in.store(true, Ordering::SeqCst);

    session.close().await.unwrap();

    assert!(!session.logged_in.load(Ordering::SeqCst));
}

#[tokio::test]
async fn trade_session_failed_connect_clears_stale_logged_in_state() {
    let dm = build_dm();
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "not-a-valid-url".to_string(),
        WebSocketConfig::default(),
        8,
    );

    session.logged_in.store(true, Ordering::SeqCst);

    assert!(session.connect().await.is_err());
    assert!(!session.logged_in.load(Ordering::SeqCst));
}

#[tokio::test]
async fn trade_session_failed_connect_does_not_keep_reconnecting_in_background() {
    let dm = build_dm();
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
        8,
    );

    let errors = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let errors = Arc::clone(&errors);
        session
            .on_error(move |_msg| {
                errors.fetch_add(1, Ordering::SeqCst);
            })
            .await;
    }

    session.ws.force_send_failure_for_test();
    assert!(session.connect().await.is_err());

    sleep(Duration::from_millis(120)).await;

    assert_eq!(errors.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn trade_commands_do_not_queue_when_transport_actor_is_missing() {
    let dm = build_dm();
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
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
