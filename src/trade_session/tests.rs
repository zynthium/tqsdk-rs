use super::*;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::types::{DIRECTION_BUY, InsertOrderRequest, Notification, OFFSET_OPEN, PRICE_TYPE_LIMIT};
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

fn account_json(balance: f64, available: f64) -> serde_json::Value {
    json!({
        "user_id": "u",
        "currency": "CNY",
        "balance": balance,
        "available": available
    })
}

fn position_json() -> serde_json::Value {
    json!({
        "user_id": "u",
        "exchange_id": "SHFE",
        "instrument_id": "au2602",
        "volume_long_today": 1,
        "volume_long": 1
    })
}

#[tokio::test]
async fn trade_session_wait_update_still_delivers_snapshot_and_reliable_event_updates() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));
    let mut events = session.subscribe_events();

    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    let snapshot_waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            session.wait_update().await.unwrap();
            let account = session.get_account().await.unwrap();
            let positions = session.get_positions().await.unwrap();
            (account.balance, positions["SHFE.au2602"].volume_long)
        })
    };
    tokio::task::yield_now().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "accounts": {
                        "CNY": account_json(1000.0, 900.0)
                    },
                    "positions": {
                        "SHFE.au2602": position_json()
                    },
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    },
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );

    let (balance, volume_long) = timeout(Duration::from_secs(1), snapshot_waiter).await.unwrap().unwrap();
    let first = timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap();
    let second = timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap();
    let account = session.get_account().await.unwrap();
    let positions = session.get_positions().await.unwrap();

    assert_eq!(balance, 1000.0);
    assert_eq!(volume_long, 1);
    assert_eq!(account.balance, 1000.0);
    assert_eq!(positions["SHFE.au2602"].volume_long, 1);
    assert!(session.logged_in.load(Ordering::SeqCst));
    assert!(matches!(
        first.kind,
        TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
    ));
    assert!(matches!(
        second.kind,
        TradeSessionEventKind::TradeCreated { ref trade_id, .. } if trade_id == "t1"
    ));

    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_wait_update_consumes_snapshot_epochs_that_arrived_between_calls() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": account_json(1000.0, 900.0)
                    }
                }
            }
        }),
        true,
        true,
    );
    timeout(Duration::from_secs(1), session.wait_update())
        .await
        .unwrap()
        .unwrap();
    let first_seen_epoch = (*session.snapshot_epoch_tx.borrow()).unwrap();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": account_json(1001.0, 901.0)
                    }
                }
            }
        }),
        true,
        true,
    );

    timeout(Duration::from_secs(1), async {
        loop {
            let current = *session.snapshot_epoch_tx.borrow();
            if current.is_some_and(|epoch| epoch > first_seen_epoch) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    timeout(Duration::from_millis(200), session.wait_update())
        .await
        .unwrap()
        .unwrap();
    let account = session.get_account().await.unwrap();
    assert_eq!(account.balance, 1001.0);

    session.close().await.unwrap();
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
async fn trade_session_failed_connect_resets_event_hub_for_existing_and_future_subscribers() {
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
    let old_hub = session.trade_events.read().unwrap().clone();
    let mut stale_stream = session.subscribe_events();

    assert!(session.connect().await.is_err());

    let err = timeout(Duration::from_secs(1), stale_stream.recv())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, TradeEventRecvError::Closed));

    let new_hub = session.trade_events.read().unwrap().clone();
    assert!(!Arc::ptr_eq(&old_hub, &new_hub));

    let mut fresh_stream = session.subscribe_events();
    let _ = new_hub.publish_transport_error("after-failed-connect".to_string());
    let event = timeout(Duration::from_secs(1), fresh_stream.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event.kind,
        TradeSessionEventKind::TransportError { ref message } if message == "after-failed-connect"
    ));
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
    let mut events = session.subscribe_events();

    session.ws.force_send_failure_for_test();
    assert!(session.connect().await.is_err());

    sleep(Duration::from_millis(120)).await;

    for _ in 0..2 {
        let next = timeout(Duration::from_millis(20), events.recv()).await;
        let Ok(Ok(event)) = next else {
            break;
        };
        assert!(matches!(
            event.kind,
            TradeSessionEventKind::TransportError { .. } | TradeSessionEventKind::NotificationReceived { .. }
        ));
    }

    let stale_end = timeout(Duration::from_millis(50), events.recv())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(stale_end, TradeEventRecvError::Closed));

    let mut fresh_events = session.subscribe_events();
    assert!(timeout(Duration::from_millis(50), fresh_events.recv()).await.is_err());
}

#[tokio::test]
async fn trade_session_close_resets_event_hub_for_existing_and_future_subscribers() {
    let dm = build_dm();
    let session = build_session(dm);
    let old_hub = session.trade_events.read().unwrap().clone();
    let mut stale_stream = session.subscribe_events();

    session.running.store(true, Ordering::SeqCst);
    session.close().await.unwrap();

    let err = timeout(Duration::from_secs(1), stale_stream.recv())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, TradeEventRecvError::Closed));

    let new_hub = session.trade_events.read().unwrap().clone();
    assert!(!Arc::ptr_eq(&old_hub, &new_hub));

    let mut fresh_stream = session.subscribe_events();
    let _ = new_hub.publish_transport_error("after-close".to_string());
    let event = timeout(Duration::from_secs(1), fresh_stream.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event.kind,
        TradeSessionEventKind::TransportError { ref message } if message == "after-close"
    ));
}

#[tokio::test]
async fn trade_session_emits_notification_events_on_unified_stream() {
    let dm = build_dm();
    let session = build_session(dm);
    let mut stream = session.subscribe_events();
    let notification = Notification {
        code: "2019112901".to_string(),
        level: "INFO".to_string(),
        r#type: "MESSAGE".to_string(),
        content: "connected".to_string(),
        bid: "simnow".to_string(),
        user_id: "u".to_string(),
    };

    session.ws.emit_notify_for_test(notification.clone());

    let event = timeout(Duration::from_secs(1), stream.recv()).await.unwrap().unwrap();
    assert!(matches!(
        event.kind,
        TradeSessionEventKind::NotificationReceived { notification: ref got }
            if got.content == notification.content
    ));
}

#[tokio::test]
async fn trade_session_emits_transport_errors_on_unified_stream() {
    let dm = build_dm();
    let session = build_session(dm);
    let mut stream = session.subscribe_events();

    session.ws.emit_error_for_test("transport failed".to_string());

    let event = timeout(Duration::from_secs(1), stream.recv()).await.unwrap().unwrap();
    assert!(matches!(
        event.kind,
        TradeSessionEventKind::TransportError { ref message } if message == "transport failed"
    ));
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
