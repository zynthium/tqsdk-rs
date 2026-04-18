use super::*;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::TqError;
use crate::types::{
    DIRECTION_BUY, InsertOrderOptions, InsertOrderRequest, Notification, OFFSET_OPEN, PRICE_TYPE_ANY, PRICE_TYPE_BEST,
    PRICE_TYPE_LIMIT, TIME_CONDITION_GFD, TIME_CONDITION_IOC, VOLUME_CONDITION_ALL, VOLUME_CONDITION_ANY,
};
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
    build_session_with_login_options(dm, TradeLoginOptions::default())
}

fn build_session_with_login_options(dm: Arc<DataManager>, login_options: TradeLoginOptions) -> TradeSession {
    TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        dm,
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
        login_options,
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
async fn trade_session_wait_update_before_connect_returns_not_connected() {
    let dm = build_dm();
    let session = build_session(dm);

    let err = session
        .wait_update()
        .await
        .expect_err("wait_update should fail before connect");
    assert!(matches!(err, TqError::TradeSessionNotConnected));
}

#[tokio::test]
async fn trade_session_wait_ready_before_connect_returns_not_connected() {
    let dm = build_dm();
    let session = build_session(dm);

    let err = session
        .wait_ready()
        .await
        .expect_err("wait_ready should fail before connect");
    assert!(matches!(err, TqError::TradeSessionNotConnected));
}

#[tokio::test]
async fn trade_session_wait_ready_blocks_until_trade_snapshot_ready() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.ws.force_missing_io_actor_for_test();
    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    let mut waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_ready().await })
    };
    tokio::task::yield_now().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "trade_more_data": true
                }
            }
        }),
        true,
        true,
    );

    assert!(timeout(Duration::from_millis(100), &mut waiter).await.is_err());

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trade_more_data": false
                }
            }
        }),
        true,
        true,
    );

    timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap().unwrap();
    assert!(session.is_ready());

    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_wait_ready_unblocks_with_not_connected_after_close() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_ready().await })
    };
    tokio::task::yield_now().await;

    session.close().await.unwrap();

    let err = timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .expect_err("wait_ready should fail once session is closed");
    assert!(matches!(err, TqError::TradeSessionNotConnected));
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
async fn trade_session_requires_trade_snapshot_completion_before_ready() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.ws.force_missing_io_actor_for_test();
    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "trade_more_data": true
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

    assert!(session.logged_in.load(Ordering::SeqCst));
    assert!(!session.snapshot_ready_for_test());
    assert!(!session.is_ready());

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trade_more_data": false
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

    assert!(session.snapshot_ready_for_test());
    assert!(session.is_ready());

    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_reconnect_notifications_reset_readiness_until_next_snapshot_epoch() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.ws.force_missing_io_actor_for_test();
    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "trade_more_data": false,
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
    assert!(session.is_ready());

    session.ws.emit_notify_for_test(Notification {
        code: "2019112902".to_string(),
        level: "WARNING".to_string(),
        r#type: "MESSAGE".to_string(),
        content: "reconnected".to_string(),
        bid: "simnow".to_string(),
        user_id: "u".to_string(),
    });

    assert!(!session.snapshot_ready_for_test());
    assert!(!session.is_ready());

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

    timeout(Duration::from_secs(1), session.wait_update())
        .await
        .unwrap()
        .unwrap();

    assert!(session.snapshot_ready_for_test());
    assert!(session.is_ready());

    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_get_position_exposes_derived_net_position_fields() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "positions": {
                        "SHFE.au2602": {
                            "user_id": "u",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "pos_long_today": 2,
                            "pos_long_his": 3,
                            "pos_short_today": 1,
                            "pos_short_his": 1
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    let position = session.get_position("SHFE.au2602").await.unwrap();
    assert_eq!(position.pos_long, 5);
    assert_eq!(position.pos_short, 2);
    assert_eq!(position.pos, 3);
}

#[tokio::test]
async fn trade_session_get_order_exposes_derived_flags_and_trade_price() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {
                            "order_id": "o1",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "status": "ALIVE",
                            "volume_orign": 2,
                            "volume_left": 1,
                            "exchange_order_id": "EX123"
                        },
                        "o2": {
                            "order_id": "o2",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "status": "FINISHED",
                            "volume_orign": 1,
                            "volume_left": 1
                        }
                    },
                    "trades": {
                        "t1": {
                            "trade_id": "t1",
                            "order_id": "o1",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume": 1,
                            "price": 520.0
                        },
                        "t2": {
                            "trade_id": "t2",
                            "order_id": "o1",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume": 1,
                            "price": 522.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    let online_order = session.get_order("o1").await.unwrap();
    assert!(online_order.is_online);
    assert!(!online_order.is_dead);
    assert!(!online_order.is_error);
    assert_eq!(online_order.trade_price, 521.0);

    let rejected_order = session.get_order("o2").await.unwrap();
    assert!(!rejected_order.is_online);
    assert!(rejected_order.is_dead);
    assert!(rejected_order.is_error);
}

#[tokio::test]
async fn trade_session_get_trade_returns_single_trade_snapshot() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": {
                            "trade_id": "t1",
                            "order_id": "o1",
                            "exchange_id": "SHFE",
                            "instrument_id": "au2602",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume": 2,
                            "price": 520.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    let trade = session.get_trade("t1").await.unwrap();
    assert_eq!(trade.trade_id, "t1");
    assert_eq!(trade.order_id, "o1");
    assert_eq!(trade.volume, 2);
    assert_eq!(trade.price, 520.0);
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
        TradeLoginOptions::default(),
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
        TradeLoginOptions::default(),
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
        TradeLoginOptions::default(),
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
        TradeLoginOptions::default(),
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

#[test]
fn build_insert_order_packet_defaults_best_to_ioc_without_limit_price() {
    let req = InsertOrderRequest {
        symbol: "CFFEX.IF2606".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_BEST.to_string(),
        limit_price: 0.0,
        volume: 2,
    };

    let (order_id, pack) = super::ops::build_insert_order_packet("u", &req, &InsertOrderOptions::default()).unwrap();

    assert!(order_id.starts_with("TQRS_"));
    assert_eq!(pack["price_type"], PRICE_TYPE_BEST);
    assert_eq!(pack["time_condition"], TIME_CONDITION_IOC);
    assert_eq!(pack["volume_condition"], VOLUME_CONDITION_ANY);
    assert!(pack.get("limit_price").is_none());
}

#[test]
fn build_insert_order_packet_supports_limit_fak_semantics() {
    let req = InsertOrderRequest {
        symbol: "DCE.m2609".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_LIMIT.to_string(),
        limit_price: 3025.0,
        volume: 3,
    };

    let (order_id, pack) = super::ops::build_insert_order_packet(
        "u",
        &req,
        &InsertOrderOptions {
            time_condition: Some(TIME_CONDITION_IOC.to_string()),
            ..InsertOrderOptions::default()
        },
    )
    .unwrap();

    assert!(order_id.starts_with("TQRS_"));
    assert_eq!(pack["price_type"], PRICE_TYPE_LIMIT);
    assert_eq!(pack["limit_price"], 3025.0);
    assert_eq!(pack["time_condition"], TIME_CONDITION_IOC);
    assert_eq!(pack["volume_condition"], VOLUME_CONDITION_ANY);
}

#[test]
fn build_insert_order_packet_supports_market_fok_with_custom_order_id() {
    let req = InsertOrderRequest {
        symbol: "DCE.m2609".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_ANY.to_string(),
        limit_price: 0.0,
        volume: 1,
    };

    let (order_id, pack) = super::ops::build_insert_order_packet(
        "u",
        &req,
        &InsertOrderOptions {
            order_id: Some("CUSTOM_ID".to_string()),
            time_condition: Some(TIME_CONDITION_IOC.to_string()),
            volume_condition: Some(VOLUME_CONDITION_ALL.to_string()),
        },
    )
    .unwrap();

    assert_eq!(order_id, "CUSTOM_ID");
    assert_eq!(pack["order_id"], "CUSTOM_ID");
    assert_eq!(pack["price_type"], PRICE_TYPE_ANY);
    assert_eq!(pack["time_condition"], TIME_CONDITION_IOC);
    assert_eq!(pack["volume_condition"], VOLUME_CONDITION_ALL);
    assert!(pack.get("limit_price").is_none());
}

#[test]
fn build_insert_order_packet_defaults_limit_to_gfd_any() {
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

    let (_order_id, pack) = super::ops::build_insert_order_packet("u", &req, &InsertOrderOptions::default()).unwrap();

    assert_eq!(pack["time_condition"], TIME_CONDITION_GFD);
    assert_eq!(pack["volume_condition"], VOLUME_CONDITION_ANY);
    assert_eq!(pack["limit_price"], 520.0);
}

#[tokio::test]
async fn trade_session_send_login_includes_client_metadata_fields() {
    let dm = build_dm();
    let session = build_session_with_login_options(
        dm,
        TradeLoginOptions {
            front: Some(TradeFrontConfig {
                broker_id: "9999".to_string(),
                url: "tcp://127.0.0.1:12345".to_string(),
            }),
            client_system_info: Some("BASE64_SYSTEM_INFO".to_string()),
            client_mac_address: Some("AA-BB-CC-DD-EE-FF".to_string()),
            ..TradeLoginOptions::default()
        },
    );

    session.send_login().await.unwrap();

    let req = session
        .ws
        .req_login_for_test()
        .expect("login packet should be recorded");
    assert_eq!(req["bid"], "simnow");
    assert_eq!(req["user_name"], "u");
    assert_eq!(req["password"], "p");
    assert_eq!(req["broker_id"], "9999");
    assert_eq!(req["front"], "tcp://127.0.0.1:12345");
    assert_eq!(req["client_system_info"], "BASE64_SYSTEM_INFO");
    assert_eq!(req["client_app_id"], "SHINNY_TQ_1.0");
    assert_eq!(req["client_mac_address"], "AA-BB-CC-DD-EE-FF");
}

#[tokio::test]
async fn trade_session_send_login_auto_fills_mac_address_when_not_configured() {
    let dm = build_dm();
    let session = build_session(dm);

    session.send_login().await.unwrap();

    let req = session
        .ws
        .req_login_for_test()
        .expect("login packet should be recorded");
    let mac = req["client_mac_address"]
        .as_str()
        .expect("auto-detected mac address should be present");
    assert_eq!(mac.len(), 17);
    assert!(
        mac.chars()
            .enumerate()
            .all(|(idx, ch)| if [2, 5, 8, 11, 14].contains(&idx) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase()
            })
    );
}

#[tokio::test]
async fn trade_session_can_skip_confirm_settlement_for_non_futures_logins() {
    let dm = build_dm();
    let session = build_session_with_login_options(
        dm,
        TradeLoginOptions {
            confirm_settlement: false,
            ..TradeLoginOptions::default()
        },
    );

    session.send_confirm_settlement().await.unwrap();

    assert!(
        session.ws.confirm_settlement_for_test().is_none(),
        "non-futures login should be able to skip confirm_settlement"
    );
}
