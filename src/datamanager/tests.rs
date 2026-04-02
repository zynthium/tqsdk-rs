use super::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Instant;

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
fn test_watch_channel_is_bounded() {
    let config = DataManagerConfig {
        watch_channel_capacity: 1,
        ..DataManagerConfig::default()
    };
    let dm = DataManager::new(HashMap::new(), config);
    let rx = dm.watch(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);

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
fn test_watch_allows_multiple_receivers_for_same_path() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let rx1 = dm.watch(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);
    let rx2 = dm.watch(vec!["quotes".to_string(), "SHFE.au2602".to_string()]);

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
fn test_unwatch_by_id_removes_only_target_watcher() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let (watcher_id, rx1) = dm.watch_register(path.clone());
    let (_other_id, rx2) = dm.watch_register(path.clone());

    assert!(dm.unwatch_by_id(&path, watcher_id));

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
fn test_notify_watchers_prunes_closed_receivers() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let (_watcher_id, rx) = dm.watch_register(path);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_on_data_spawn_layer_overhead_profile() {
    let rounds = 10000usize;
    let callback_count = 6usize;
    let payload = json!({
        "quotes": {
            "SHFE.au2602": {
                "last_price": 500.0,
                "volume": 1000
            }
        }
    });

    let direct_counter = Arc::new(AtomicUsize::new(0));
    let direct_dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    for _ in 0..callback_count {
        let counter = Arc::clone(&direct_counter);
        direct_dm.on_data(move || {
            counter.fetch_add(1, AtomicOrdering::Relaxed);
        });
    }

    let direct_start = Instant::now();
    for _ in 0..rounds {
        direct_dm.merge_data(payload.clone(), true, true);
    }
    let direct_elapsed = direct_start.elapsed();
    let direct_total = direct_counter.load(AtomicOrdering::Relaxed);

    let spawned_counter = Arc::new(AtomicUsize::new(0));
    let spawned_dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    for _ in 0..callback_count {
        let counter = Arc::clone(&spawned_counter);
        spawned_dm.on_data(move || {
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                counter.fetch_add(1, AtomicOrdering::Relaxed);
            });
        });
    }

    let spawned_start = Instant::now();
    for _ in 0..rounds {
        spawned_dm.merge_data(payload.clone(), true, true);
    }
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let spawned_elapsed = spawned_start.elapsed();
    let spawned_total = spawned_counter.load(AtomicOrdering::Relaxed);

    assert_eq!(direct_total, rounds * callback_count);
    assert_eq!(spawned_total, rounds * callback_count);

    println!(
        "on_data_profile rounds={} callbacks={} direct_us={} spawned_us={}",
        rounds,
        callback_count,
        direct_elapsed.as_micros(),
        spawned_elapsed.as_micros()
    );
}
