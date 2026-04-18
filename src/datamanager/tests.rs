use super::*;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
fn test_merge_data() {
    let initial_data = HashMap::new();
    let config = DataManagerConfig::default();
    let dm = DataManager::new(initial_data, config);

    let source = json!({
        "quotes": {
            "SHFE.au2602": {
                "last_price": 500.0,
                "volume": 1000
            }
        }
    });

    dm.merge_data(source, true, false);

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]);
    assert!(quote.is_some());

    if let Some(Value::Object(quote_map)) = quote {
        assert_eq!(quote_map.get("last_price"), Some(&json!(500.0)));
        assert_eq!(quote_map.get("volume"), Some(&json!(1000)));
    }
}

#[test]
fn test_set_default_creates_nested_path() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());

    let inserted = dm.set_default(
        &["trade", "u", "orders"],
        json!({
            "count": 1
        }),
    );

    assert_eq!(
        inserted,
        Some(json!({
            "count": 1
        }))
    );
    assert_eq!(
        dm.get_by_path(&["trade", "u", "orders"]),
        Some(json!({
            "count": 1
        }))
    );
}

#[test]
fn test_watch_handle_channel_is_bounded() {
    let config = DataManagerConfig {
        watch_channel_capacity: 1,
        ..DataManagerConfig::default()
    };
    let dm = DataManager::new(HashMap::new(), config);
    let handle = dm.watch_handle(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);
    let rx = handle.receiver().clone();

    dm.merge_data(
        json!({
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
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );

    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[test]
fn datamanager_drops_persistently_full_watcher() {
    let config = DataManagerConfig {
        watch_channel_capacity: 1,
        ..DataManagerConfig::default()
    };
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, config);

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let _slow = dm.watch_handle(path.clone());
    let fast = dm.watch_handle(path.clone());

    for last_price in 0..64 {
        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "instrument_id": "SHFE.au2602",
                        "datetime": "2024-01-01 09:00:00.000000",
                        "last_price": 500.0 + (last_price as f64)
                    }
                }
            }),
            true,
            true,
        );

        fast.receiver()
            .try_recv()
            .expect("healthy watcher should keep draining updates");
    }

    assert_eq!(
        dm.watchers.read().unwrap().get(&path_key).map(Vec::len),
        Some(1),
        "persistently full watcher should be removed while healthy watcher remains"
    );
}

#[tokio::test]
async fn subscribe_epoch_observes_completed_merge_state() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
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

    rx.changed().await.unwrap();

    assert_eq!(*rx.borrow(), 1);
    assert_eq!(
        dm.get_by_path(&["quotes", "SHFE.au2602", "last_price"]),
        Some(json!(500.0))
    );
    assert_eq!(dm.get_path_epoch(&["quotes", "SHFE.au2602"]), 1);
}

#[tokio::test]
async fn subscribe_epoch_coalesces_multiple_merges_but_keeps_latest_epoch() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        json!({
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

    rx.changed().await.unwrap();

    assert_eq!(*rx.borrow(), 2);
    assert_eq!(dm.get_quote_data("SHFE.au2602").unwrap().last_price, 501.0);
}

#[tokio::test]
async fn subscribe_epoch_slow_consumer_can_reconcile_with_path_epoch() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": {
                            "balance": 1000.0,
                            "available": 900.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": {
                            "balance": 1002.0,
                            "available": 901.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    rx.changed().await.unwrap();

    let seen_epoch = *rx.borrow();
    let path_epoch = dm.get_path_epoch(&["trade", "u", "accounts", "CNY"]);
    let account = dm.get_account_data("u", "CNY").unwrap();

    assert_eq!(seen_epoch, 2);
    assert_eq!(path_epoch, 2);
    assert_eq!(account.balance, 1002.0);
    assert_eq!(account.available, 901.0);
}

#[test]
fn datamanager_merge_quote_only_update_does_not_touch_trade_epochs() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {
                            "order_id": "o1",
                            "status": "ALIVE",
                            "exchange_order_id": "ex1"
                        }
                    },
                    "trades": {
                        "t1": {
                            "trade_id": "t1",
                            "order_id": "o1",
                            "volume": 2,
                            "price": 521.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    let user_epoch_before = dm.get_path_epoch(&["trade", "u"]);
    let order_epoch_before = dm.get_path_epoch(&["trade", "u", "orders", "o1"]);
    assert_eq!(
        dm.get_by_path(&["trade", "u", "orders", "o1", "trade_price"]),
        Some(json!(521.0))
    );

    dm.merge_data(
        json!({
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

    assert_eq!(
        dm.get_path_epoch(&["trade", "u"]),
        user_epoch_before,
        "quote-only merge should not bump trade user epoch"
    );
    assert_eq!(
        dm.get_path_epoch(&["trade", "u", "orders", "o1"]),
        order_epoch_before,
        "quote-only merge should not bump order epoch"
    );
}

#[test]
fn datamanager_merge_trade_only_update_still_derives_trade_price() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {
                            "order_id": "o1",
                            "status": "ALIVE",
                            "exchange_order_id": "ex1"
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    assert_eq!(dm.get_by_path(&["trade", "u", "orders", "o1", "trade_price"]), None);

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": {
                            "trade_id": "t1",
                            "order_id": "o1",
                            "volume": 2,
                            "price": 521.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    assert_eq!(
        dm.get_by_path(&["trade", "u", "orders", "o1", "trade_price"]),
        Some(json!(521.0)),
        "trade-only merge should still refresh derived order trade_price"
    );
}

#[test]
fn test_is_changing() {
    let initial_data = HashMap::new();
    let config = DataManagerConfig::default();
    let dm = DataManager::new(initial_data, config);

    let source = json!({
        "quotes": {
            "SHFE.au2602": {
                "last_price": 500.0
            }
        }
    });

    dm.merge_data(source, true, false);

    assert!(dm.is_changing(&["quotes", "SHFE.au2602"]));
    assert!(!dm.is_changing(&["quotes", "SHFE.ag2512"]));
}

fn test_kline_value(datetime: i64) -> Value {
    json!({
        "datetime": datetime,
        "open": 1.0,
        "close": 2.0,
        "high": 3.0,
        "low": 0.5,
        "open_oi": 0,
        "close_oi": 0,
        "volume": 1
    })
}

fn build_large_multi_kline_fixture(rows: usize) -> (HashMap<String, Value>, Vec<String>, i64) {
    let dur_id: i64 = 60_000_000_000;
    let duration_key = dur_id.to_string();
    let mut main_data = Map::new();
    let mut other_data = Map::new();
    let mut binding_map = Map::new();

    for id in 1..=rows as i64 {
        let mapped_id = id * 10;
        main_data.insert(id.to_string(), test_kline_value(id * 1_000_000_000));
        other_data.insert(mapped_id.to_string(), test_kline_value(id * 1_000_000_000));
        binding_map.insert(id.to_string(), json!(mapped_id));
    }

    let mut a_duration = Map::new();
    a_duration.insert("last_id".to_string(), json!(rows as i64));
    a_duration.insert("trading_day_start_id".to_string(), json!(0));
    a_duration.insert("trading_day_end_id".to_string(), json!(0));
    let mut binding_root = Map::new();
    binding_root.insert("B".to_string(), Value::Object(binding_map));
    a_duration.insert("binding".to_string(), Value::Object(binding_root));
    a_duration.insert("data".to_string(), Value::Object(main_data));

    let mut b_duration = Map::new();
    b_duration.insert("last_id".to_string(), json!((rows as i64) * 10));
    b_duration.insert("trading_day_start_id".to_string(), json!(0));
    b_duration.insert("trading_day_end_id".to_string(), json!(0));
    b_duration.insert("data".to_string(), Value::Object(other_data));

    let mut a_map = Map::new();
    a_map.insert(duration_key.clone(), Value::Object(a_duration));
    let mut b_map = Map::new();
    b_map.insert(duration_key, Value::Object(b_duration));

    let mut klines = Map::new();
    klines.insert("A".to_string(), Value::Object(a_map));
    klines.insert("B".to_string(), Value::Object(b_map));

    let mut initial_data = HashMap::new();
    initial_data.insert(
        "charts".to_string(),
        json!({
            "C1": {
                "left_id": -1,
                "right_id": -1
            }
        }),
    );
    initial_data.insert("klines".to_string(), Value::Object(klines));

    (initial_data, vec!["A".to_string(), "B".to_string()], dur_id)
}

#[test]
fn test_get_multi_klines_data_strict_alignment_skips_incomplete_rows() {
    let dur_id: i64 = 60_000_000_000;
    let mut initial_data = HashMap::new();
    initial_data.insert(
        "charts".to_string(),
        json!({
            "C1": {
                "left_id": -1,
                "right_id": -1
            }
        }),
    );
    initial_data.insert(
        "klines".to_string(),
        json!({
            "A": {
                "60000000000": {
                    "last_id": 2,
                    "trading_day_start_id": 0,
                    "trading_day_end_id": 0,
                    "binding": {
                        "B": {
                            "1": 10
                        }
                    },
                    "data": {
                        "1": test_kline_value(1_000_000_000),
                        "2": test_kline_value(2_000_000_000)
                    }
                }
            },
            "B": {
                "60000000000": {
                    "last_id": 20,
                    "trading_day_start_id": 0,
                    "trading_day_end_id": 0,
                    "data": {
                        "10": test_kline_value(1_000_000_000),
                        "20": test_kline_value(2_000_000_000)
                    }
                }
            }
        }),
    );

    let dm = DataManager::new(initial_data, DataManagerConfig::default());
    let symbols = vec!["A".to_string(), "B".to_string()];
    let res = dm.get_multi_klines_data(&symbols, dur_id, "C1", 100).unwrap();

    assert_eq!(res.data.len(), 1);
    assert_eq!(res.data[0].klines.len(), 2);
    assert!(res.data[0].klines.contains_key("A"));
    assert!(res.data[0].klines.contains_key("B"));
}

#[test]
fn test_get_multi_klines_data_strict_alignment_keeps_complete_rows() {
    let dur_id: i64 = 60_000_000_000;
    let mut initial_data = HashMap::new();
    initial_data.insert(
        "charts".to_string(),
        json!({
            "C1": {
                "left_id": -1,
                "right_id": -1
            }
        }),
    );
    initial_data.insert(
        "klines".to_string(),
        json!({
            "A": {
                "60000000000": {
                    "last_id": 2,
                    "trading_day_start_id": 0,
                    "trading_day_end_id": 0,
                    "binding": {
                        "B": {
                            "1": 10,
                            "2": 20
                        }
                    },
                    "data": {
                        "1": test_kline_value(1_000_000_000),
                        "2": test_kline_value(2_000_000_000)
                    }
                }
            },
            "B": {
                "60000000000": {
                    "last_id": 20,
                    "trading_day_start_id": 0,
                    "trading_day_end_id": 0,
                    "data": {
                        "10": test_kline_value(1_000_000_000),
                        "20": test_kline_value(2_000_000_000)
                    }
                }
            }
        }),
    );

    let dm = DataManager::new(initial_data, DataManagerConfig::default());
    let symbols = vec!["A".to_string(), "B".to_string()];
    let res = dm.get_multi_klines_data(&symbols, dur_id, "C1", 100).unwrap();

    assert_eq!(res.data.len(), 2);
    for set in res.data {
        assert_eq!(set.klines.len(), 2);
        assert!(set.klines.contains_key("A"));
        assert!(set.klines.contains_key("B"));
    }
}

#[test]
fn multi_kline_query_releases_read_lock_before_post_processing() {
    let row_count = 50_000;
    let (initial_data, symbols, dur_id) = build_large_multi_kline_fixture(row_count);
    let dm = Arc::new(DataManager::new(initial_data, DataManagerConfig::default()));
    let started = Arc::new(Barrier::new(2));
    let finished = Arc::new(AtomicBool::new(false));

    let dm_for_query = Arc::clone(&dm);
    let started_for_query = Arc::clone(&started);
    let finished_for_query = Arc::clone(&finished);
    let handle = std::thread::spawn(move || {
        started_for_query.wait();
        let result = dm_for_query
            .get_multi_klines_data(&symbols, dur_id, "C1", row_count)
            .expect("multi-kline query should succeed");
        finished_for_query.store(true, Ordering::SeqCst);
        result.data.len()
    });

    started.wait();
    let mut released_before_finish = false;
    for _ in 0..200 {
        if let Ok(write_guard) = dm.data.try_write() {
            if !finished.load(Ordering::SeqCst) {
                released_before_finish = true;
                drop(write_guard);
                break;
            }
            drop(write_guard);
        }

        if finished.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        released_before_finish,
        "query should release the global read lock before finishing post-processing"
    );

    assert_eq!(handle.join().unwrap(), row_count);
}

#[test]
fn multi_kline_query_reuses_numeric_index_cache_until_epoch_changes() {
    let row_count = 128;
    let (initial_data, symbols, dur_id) = build_large_multi_kline_fixture(row_count);
    let dm = DataManager::new(initial_data, DataManagerConfig::default());
    let cache_key = format!("klines:A:{dur_id}");

    dm.get_multi_klines_data(&symbols, dur_id, "C1", row_count).unwrap();
    let first_index = dm
        .numeric_value_index_cache
        .read()
        .unwrap()
        .get(&cache_key)
        .expect("first query should populate numeric index cache")
        .index
        .clone();

    dm.get_multi_klines_data(&symbols, dur_id, "C1", row_count).unwrap();
    let second_index = dm
        .numeric_value_index_cache
        .read()
        .unwrap()
        .get(&cache_key)
        .expect("same-epoch query should reuse numeric index cache")
        .index
        .clone();
    assert!(
        Arc::ptr_eq(&first_index, &second_index),
        "same-epoch query should reuse cached numeric index"
    );

    dm.merge_data(
        json!({
            "klines": {
                "A": {
                    dur_id.to_string(): {
                        "last_id": row_count as i64 + 1,
                        "binding": {
                            "B": {
                                (row_count as i64 + 1).to_string(): (row_count as i64 + 1) * 10
                            }
                        },
                        "data": {
                            (row_count as i64 + 1).to_string(): test_kline_value((row_count as i64 + 1) * 1_000_000_000)
                        }
                    }
                },
                "B": {
                    dur_id.to_string(): {
                        "last_id": (row_count as i64 + 1) * 10,
                        "data": {
                            ((row_count as i64 + 1) * 10).to_string(): test_kline_value((row_count as i64 + 1) * 1_000_000_000)
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    dm.get_multi_klines_data(&symbols, dur_id, "C1", row_count + 1).unwrap();
    let third_index = dm
        .numeric_value_index_cache
        .read()
        .unwrap()
        .get(&cache_key)
        .expect("epoch-changing query should rebuild numeric index cache")
        .index
        .clone();
    assert!(
        !Arc::ptr_eq(&first_index, &third_index),
        "epoch change should invalidate and rebuild cached numeric index"
    );
}

#[test]
fn test_merge_persist_mode() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.persist = true;
    config.merge_semantics.reduce_diff = true;
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": null
                }
            }
        }),
        true,
        true,
    );

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
    assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
}

#[test]
fn test_merge_prototype_mode() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.prototype = Some(json!({
        "quotes": {
            "*": {
                "price_tick": 0.2
            }
        }
    }));
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "price_tick": "invalid"
                }
            }
        }),
        true,
        true,
    );

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
    assert_eq!(quote.get("price_tick"), Some(&json!(0.2)));
}

#[test]
fn test_merge_reduce_diff_mode() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.reduce_diff = true;
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    assert!(!dm.is_changing(&["quotes", "SHFE.au2602"]));
}

#[test]
fn test_merge_prototype_at_branch_default_path() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.prototype = Some(json!({
        "quotes": {
            "@": {
                "ins_class": "FUTURE",
                "price_tick": 0.2
            }
        }
    }));
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
    assert_eq!(quote.get("ins_class"), Some(&json!("FUTURE")));
    assert_eq!(quote.get("price_tick"), Some(&json!(0.2)));
    assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
}

#[test]
fn test_merge_prototype_hash_branch_persist_with_reduce_diff() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.reduce_diff = true;
    config.merge_semantics.prototype = Some(json!({
        "quotes": {
            "#": {
                "price_tick": 0.2
            }
        }
    }));
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": null
                }
            }
        }),
        true,
        true,
    );

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
    assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
}

#[test]
fn test_merge_prototype_hash_branch_delete_without_reduce_diff() {
    let initial_data = HashMap::new();
    let mut config = DataManagerConfig::default();
    config.merge_semantics.reduce_diff = false;
    config.merge_semantics.prototype = Some(json!({
        "quotes": {
            "#": {
                "price_tick": 0.2
            }
        }
    }));
    let dm = DataManager::new(initial_data, config);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": null
                }
            }
        }),
        true,
        true,
    );

    let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
    assert!(quote.get("last_price").is_none());
}

#[test]
fn test_watch_handle_allows_multiple_receivers_for_same_path() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let first = dm.watch_handle(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);
    let second = dm.watch_handle(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);
    let rx1 = first.receiver().clone();
    let rx2 = second.receiver().clone();

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
}

#[test]
fn datamanager_watchers_same_path_cancel_independently() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let mut first = dm.watch_register(path.clone());
    let second = dm.watch_register(path.clone());
    let rx1 = first.receiver().clone();

    assert!(first.cancel());
    assert_eq!(
        dm.watchers.read().unwrap().get(&path_key).map(Vec::len),
        Some(1),
        "cancel should only remove the targeted watcher"
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    assert!(rx1.try_recv().is_err());
    assert!(second.receiver().try_recv().is_ok());

    drop(second);
    assert!(
        !dm.watchers.read().unwrap().contains_key(&path_key),
        "dropping the remaining guard should immediately unregister that watcher"
    );
}

#[test]
fn data_watch_handle_cancel_is_per_registration() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let mut first = dm.watch_handle(path.clone());
    let second = dm.watch_handle(path.clone());
    let rx1 = first.receiver().clone();
    let rx2 = second.receiver().clone();

    assert!(first.cancel());
    assert!(!first.cancel(), "cancel should be idempotent for one handle");
    assert_eq!(
        dm.watchers.read().unwrap().get(&path_key).map(Vec::len),
        Some(1),
        "cancel should only remove the targeted registration"
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    assert!(rx1.try_recv().is_err());
    assert!(rx2.try_recv().is_ok());
}

#[test]
fn data_watch_handle_drop_unregisters_its_own_watcher_only() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let first = dm.watch_handle(path.clone());
    let second = dm.watch_handle(path.clone());

    drop(first);
    assert_eq!(
        dm.watchers.read().unwrap().get(&path_key).map(Vec::len),
        Some(1),
        "dropping one handle should only unregister its own watcher"
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    assert!(second.receiver().try_recv().is_ok());

    drop(second);
    assert!(
        !dm.watchers.read().unwrap().contains_key(&path_key),
        "dropping the final handle should unregister the remaining watcher"
    );
}

#[test]
fn data_watch_handle_temporary_receiver_clone_stays_active() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let rx = dm.watch_handle(path.clone()).receiver().clone();

    assert_eq!(
        dm.watchers.read().unwrap().get(&path_key).map(Vec::len),
        Some(1),
        "temporary handle drop should keep watcher while receiver clone is alive"
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    assert!(rx.try_recv().is_ok());

    drop(rx);
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );

    assert!(
        !dm.watchers.read().unwrap().contains_key(&path_key),
        "closed receiver clones should be pruned on subsequent notify_watchers"
    );
}

#[test]
fn test_notify_watchers_prunes_closed_receivers() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let rx = dm.watch_handle(path).receiver().clone();
    drop(rx);

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    assert!(
        !dm.watchers.read().unwrap().contains_key(&path_key),
        "closed receivers should be pruned after notify_watchers"
    );
}
