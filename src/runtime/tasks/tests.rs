use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::TimeZone;
use serde_json::json;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::{Duration, sleep, timeout};

use crate::runtime::{
    ChildOrderRunner, ExecutionAdapter, MarketAdapter, OffsetAction, OpenLimitBudget, OrderDirection, PlannedOffset,
    PriceMode, RuntimeError, RuntimeMode, RuntimeResult, TargetPosScheduleStep, TqRuntime, compute_plan,
    parse_offset_priority, validate_quote_constraints,
};
use crate::types::{
    DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, Position, Quote,
    Trade,
};

#[test]
fn offset_priority_parser_accepts_python_compatible_forms() {
    assert_eq!(
        parse_offset_priority("今昨,开").expect("default form should parse"),
        vec![
            vec![OffsetAction::Today, OffsetAction::Yesterday],
            vec![OffsetAction::Open],
        ]
    );
    assert_eq!(
        parse_offset_priority("今昨开").expect("simultaneous form should parse"),
        vec![vec![OffsetAction::Today, OffsetAction::Yesterday, OffsetAction::Open,]]
    );
    assert_eq!(
        parse_offset_priority("昨开").expect("yesterday-then-open should parse"),
        vec![vec![OffsetAction::Yesterday, OffsetAction::Open]]
    );
    assert_eq!(
        parse_offset_priority("开").expect("open only should parse"),
        vec![vec![OffsetAction::Open]]
    );
}

#[test]
fn planning_rejects_contracts_with_open_min_volume_gt_one() {
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        open_min_market_order_volume: 2,
        ..Quote::default()
    };

    let err = validate_quote_constraints(&quote).expect_err("open min volume > 1 should be rejected");
    assert!(format!("{err}").contains("最小市价开仓手数"));
}

#[test]
fn planning_respects_shfe_close_today_vs_close_yesterday() {
    let priorities = parse_offset_priority("今昨开").expect("priority should parse");
    let position: Position = serde_json::from_value(json!({
        "exchange_id": "SHFE",
        "instrument_id": "rb2601",
        "pos_long_today": 2,
        "pos_long_his": 3,
        "volume_long_today": 2,
        "volume_long_his": 3,
        "volume_long": 5
    }))
    .expect("position should deserialize");

    let shfe_quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ..Quote::default()
    };
    let shfe_plan = compute_plan(&shfe_quote, &position, -1, &priorities, None).expect("shfe plan should succeed");
    assert_eq!(shfe_plan.len(), 1);
    assert_eq!(shfe_plan[0].orders.len(), 3);
    assert_eq!(shfe_plan[0].orders[0].offset, PlannedOffset::CloseToday);
    assert_eq!(shfe_plan[0].orders[0].volume, 2);
    assert_eq!(shfe_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(shfe_plan[0].orders[1].volume, 3);
    assert_eq!(shfe_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(shfe_plan[0].orders[2].volume, 1);

    let cffex_quote = Quote {
        instrument_id: "CFFEX.IF2601".to_string(),
        exchange_id: "CFFEX".to_string(),
        ..Quote::default()
    };
    let cffex_plan = compute_plan(&cffex_quote, &position, -1, &priorities, None).expect("cffex plan should succeed");
    assert_eq!(cffex_plan.len(), 1);
    assert_eq!(cffex_plan[0].orders.len(), 3);
    assert_eq!(cffex_plan[0].orders[0].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[0].volume, 2);
    assert_eq!(cffex_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[1].volume, 3);
    assert_eq!(cffex_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(cffex_plan[0].orders[2].volume, 1);
}

#[test]
fn planning_ignores_open_limit_when_quote_does_not_provide_it() {
    let priorities = parse_offset_priority("开").expect("priority should parse");
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        open_limit: 0,
        ..Quote::default()
    };
    let position = make_position("SHFE", "rb2601");

    let plan =
        compute_plan(&quote, &position, 2, &priorities, None).expect("missing open_limit should not block planning");

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].orders[0].offset, PlannedOffset::Open);
    assert_eq!(plan[0].orders[0].volume, 2);
}

#[test]
fn planning_rejects_when_requested_volume_exceeds_remaining_open_limit() {
    let priorities = parse_offset_priority("开").expect("priority should parse");
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        open_limit: 10,
        ..Quote::default()
    };
    let position = make_position("SHFE", "rb2601");

    let err = compute_plan(
        &quote,
        &position,
        4,
        &priorities,
        Some(OpenLimitBudget {
            open_limit: 10,
            used_volume: 8,
            remaining_limit: 2,
        }),
    )
    .expect_err("plan should be rejected once it exceeds remaining daily limit");

    assert!(matches!(
        err,
        RuntimeError::OpenLimitExceeded {
            open_limit: 10,
            used_volume: 8,
            remaining_limit: 2,
            requested_plan_volume: 4,
            ..
        }
    ));
}

#[tokio::test]
async fn child_order_runner_reprices_when_market_turns_worse() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market.clone(),
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Buy,
        PlannedOffset::Open,
        2,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run_until_all_traded().await });

    execution.wait_for_insert_count(1).await;
    let first_order = execution.latest_order_id().await.expect("first order should exist");

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            ask_price1: 102.0,
            bid_price1: 101.0,
            last_price: 101.0,
            ..Quote::default()
        })
        .await;

    execution.wait_for_cancel_count(1).await;
    execution.wait_for_insert_count(2).await;
    let second_order = execution.latest_order_id().await.expect("second order should exist");
    assert_ne!(first_order, second_order);

    execution
        .finish_order(&second_order, 0, vec![make_trade(&second_order, "trade-1", 2, 102.0)])
        .await;

    task.await.expect("join should succeed").expect("runner should succeed");

    assert_eq!(execution.inserted_prices().await, vec![101.0, 102.0]);
    assert_eq!(execution.cancelled_order_ids().await, vec![first_order]);
}

#[tokio::test]
async fn child_order_runner_waits_for_trade_aggregation_after_finished_order() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market,
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Buy,
        PlannedOffset::Open,
        1,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run_until_all_traded().await });

    execution.wait_for_insert_count(1).await;
    let order_id = execution.latest_order_id().await.expect("order should exist");
    execution.finish_order(&order_id, 0, vec![]).await;

    sleep(Duration::from_millis(50)).await;
    assert!(
        !task.is_finished(),
        "runner should wait for trade aggregation after FINISHED"
    );

    execution
        .append_trade(&order_id, make_trade(&order_id, "trade-1", 1, 101.0))
        .await;

    timeout(Duration::from_secs(1), task)
        .await
        .expect("runner should finish once trades are visible")
        .expect("join should succeed")
        .expect("runner should succeed");
}

#[tokio::test]
async fn child_order_runner_retries_after_external_unfilled_finish() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market.clone(),
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Buy,
        PlannedOffset::Open,
        1,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run_until_all_traded().await });

    execution.wait_for_insert_count(1).await;
    let first_order = execution.latest_order_id().await.expect("first order should exist");
    execution
        .finish_order_externally(&first_order, 1, "交易日结束，自动撤销当日有效的委托单（GFD）")
        .await;

    sleep(Duration::from_millis(50)).await;
    assert!(
        !task.is_finished(),
        "runner should keep waiting for a fresh quote after an external unfilled finish"
    );

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            ask_price1: 102.0,
            bid_price1: 101.0,
            last_price: 101.0,
            ..Quote::default()
        })
        .await;

    execution.wait_for_insert_count(2).await;
    let second_order = execution.latest_order_id().await.expect("second order should exist");
    assert_ne!(first_order, second_order);

    execution
        .finish_order(&second_order, 0, vec![make_trade(&second_order, "trade-1", 1, 102.0)])
        .await;

    timeout(Duration::from_secs(1), task)
        .await
        .expect("runner should finish after retrying on the fresh quote")
        .expect("join should succeed")
        .expect("runner should treat external unfilled finishes as retryable");

    assert_eq!(execution.inserted_prices().await, vec![101.0, 102.0]);
}

#[tokio::test]
async fn child_order_runner_replans_after_close_today_error_finish() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::default());
    let runner = ChildOrderRunner::new(
        market.clone(),
        execution.clone(),
        "SIM",
        "SHFE.rb2601",
        OrderDirection::Sell,
        PlannedOffset::CloseToday,
        3,
        None,
        PriceMode::Active,
    );

    let task = tokio::spawn(async move { runner.run().await });

    execution.wait_for_insert_count(1).await;
    let order_id = execution
        .latest_order_id()
        .await
        .expect("close today order should exist");
    execution.finish_order_with_error(&order_id, 3, "平今仓手数不足").await;

    sleep(Duration::from_millis(50)).await;
    assert!(
        !task.is_finished(),
        "close-today rejection should wait for the parent task to replan on a fresh quote"
    );

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            ask_price1: 102.0,
            bid_price1: 101.0,
            last_price: 101.0,
            ..Quote::default()
        })
        .await;

    let outcome = timeout(Duration::from_secs(1), task)
        .await
        .expect("runner should finish once it can replan")
        .expect("join should succeed")
        .expect("close-today rejection should not surface as a hard runtime error");
    assert_eq!(outcome, super::ChildOrderStatus::NeedsReplan);
}

#[tokio::test]
async fn target_pos_handle_latest_target_overrides_previous_target() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .build()
        .expect("target task should build");

    task.set_target_volume(-1).expect("first target should be accepted");
    task.set_target_volume(1)
        .expect("latest target should replace the previous one");
    task.wait_target_reached()
        .await
        .expect("latest target should eventually be reached");

    let position = execution
        .position("SIM", "SHFE.rb2601")
        .await
        .expect("position should be readable");
    assert_eq!(net_position(&position), 1);
    assert_eq!(execution.inserted_prices().await, vec![101.0]);
}

#[tokio::test]
async fn target_pos_uses_only_current_trading_day_trades_for_open_limit() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-15 09:00:00.000000".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        open_limit: 5,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    execution
        .set_symbol_trades(vec![make_trade_at(
            "old-order",
            "old-trade",
            4,
            100.0,
            shanghai_nanos(2026, 4, 14, 9, 1, 0),
        )])
        .await;
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account.target_pos("SHFE.rb2601").build().expect("task should build");

    task.set_target_volume(1).expect("target should be accepted");
    task.wait_target_reached()
        .await
        .expect("prior-day trades must not consume today's budget");

    assert_eq!(execution.inserted_prices().await, vec![101.0]);
}

#[tokio::test]
async fn target_pos_fails_when_same_day_trades_exhaust_open_limit() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-15 09:00:00.000000".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        open_limit: 5,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        false,
    ));
    execution
        .set_symbol_trades(vec![make_trade_at(
            "same-day-order",
            "same-day-trade",
            5,
            100.0,
            shanghai_nanos(2026, 4, 15, 9, 1, 0),
        )])
        .await;
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account.target_pos("SHFE.rb2601").build().expect("task should build");

    task.set_target_volume(1).expect("target should be accepted");

    let err = task
        .wait_target_reached()
        .await
        .expect_err("same-day trades should exhaust the daily open_limit budget");

    assert!(matches!(err, RuntimeError::TargetTaskFailed { .. }));
    assert!(err.to_string().contains("open_limit"));
}

#[tokio::test]
async fn target_pos_cancel_transitions_to_finished_after_owned_orders_are_closed() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        false,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .build()
        .expect("target task should build");

    task.set_target_volume(1).expect("target should be accepted");
    timeout(Duration::from_secs(1), execution.wait_for_insert_count(1))
        .await
        .expect("first child order should be submitted promptly");
    assert!(!task.is_finished(), "task should remain active before cancel");

    task.cancel().await.expect("cancel should be accepted");
    timeout(Duration::from_secs(1), execution.wait_for_cancel_count(1))
        .await
        .expect("owned order should be canceled promptly");
    timeout(Duration::from_secs(1), task.wait_finished())
        .await
        .expect("task should transition to finished promptly")
        .expect("task should finish after owned orders are canceled");
    assert!(task.is_finished(), "task should report finished after cleanup");
}

#[tokio::test]
async fn target_pos_handle_runs_same_batch_orders_concurrently() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position_with_long("SHFE", "rb2601", 1, 1),
        false,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .config(crate::runtime::TargetPosConfig {
            offset_priority: crate::runtime::OffsetPriority::TodayYesterdayThenOpen,
            ..crate::runtime::TargetPosConfig::default()
        })
        .build()
        .expect("target task should build");

    task.set_target_volume(-1).expect("target should be accepted");

    timeout(Duration::from_secs(1), execution.wait_for_insert_count(3))
        .await
        .expect("all same-batch child orders should be submitted without waiting for fills");

    task.cancel().await.expect("cancel should be accepted");
    timeout(Duration::from_secs(1), task.wait_finished())
        .await
        .expect("task should finish after cancel")
        .expect("task should finish cleanly");
}

#[tokio::test]
async fn target_pos_handle_waits_for_prior_batch_before_submitting_next_batch() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position_with_long("SHFE", "rb2601", 1, 1),
        false,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .config(crate::runtime::TargetPosConfig {
            offset_priority: crate::runtime::OffsetPriority::TodayYesterdayThenOpenWait,
            ..crate::runtime::TargetPosConfig::default()
        })
        .build()
        .expect("target task should build");

    task.set_target_volume(-1).expect("target should be accepted");

    timeout(Duration::from_secs(1), execution.wait_for_insert_count(2))
        .await
        .expect("first batch should submit both close orders");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        execution.inserted_prices().await.len(),
        2,
        "open batch should not start before close batch finishes"
    );

    let alive_orders = execution.alive_order_ids().await;
    assert_eq!(alive_orders.len(), 2, "close batch should have two live orders");
    for (idx, order_id) in alive_orders.iter().enumerate() {
        execution
            .finish_order(
                order_id,
                0,
                vec![make_trade(order_id, &format!("trade-{idx}"), 1, 100.0)],
            )
            .await;
    }

    timeout(Duration::from_secs(1), execution.wait_for_insert_count(3))
        .await
        .expect("open batch should start after close batch finishes");

    task.cancel().await.expect("cancel should be accepted");
    timeout(Duration::from_secs(1), task.wait_finished())
        .await
        .expect("task should finish after cancel")
        .expect("task should finish cleanly");
}

#[tokio::test]
async fn scheduler_runs_deadline_bounded_steps_then_finishes_last_step_on_target() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        false,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: StdDuration::from_millis(50),
                target_volume: 1,
                price_mode: Some(PriceMode::Passive),
            },
            TargetPosScheduleStep {
                interval: StdDuration::from_millis(50),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()
        .expect("scheduler should build");

    timeout(Duration::from_secs(1), execution.wait_for_insert_count(1))
        .await
        .expect("first step should submit an order");
    timeout(Duration::from_secs(1), execution.wait_for_cancel_count(1))
        .await
        .expect("first step should stop at deadline and cancel outstanding work");
    timeout(Duration::from_secs(1), execution.wait_for_insert_count(2))
        .await
        .expect("last step should continue with a new target task");

    let last_order_id = execution.latest_order_id().await.expect("last order should exist");
    execution
        .finish_order(
            &last_order_id,
            0,
            vec![make_trade(&last_order_id, "trade-final", 1, 101.0)],
        )
        .await;

    timeout(Duration::from_secs(1), scheduler.wait_finished())
        .await
        .expect("scheduler should finish once last step reaches target")
        .expect("scheduler should finish cleanly");
    assert!(scheduler.is_finished(), "scheduler should report finished");

    let position = execution
        .position("SIM", "SHFE.rb2601")
        .await
        .expect("position should be readable");
    assert_eq!(net_position(&position), 1);
}

#[tokio::test]
async fn scheduler_deadline_uses_trading_time_ranges_instead_of_counting_breaks() {
    let market = Arc::new(FakeMarketAdapter::with_trading_time(
        Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            datetime: "2026-04-09 11:29:30.000000".to_string(),
            ask_price1: 101.0,
            bid_price1: 100.0,
            last_price: 100.0,
            ..Quote::default()
        },
        json!({
            "day": [["09:00:00", "11:30:00"], ["13:30:00", "15:00:00"]],
            "night": []
        }),
    ));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        false,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Live,
        market.clone(),
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: StdDuration::from_secs(60),
                target_volume: 1,
                price_mode: Some(PriceMode::Passive),
            },
            TargetPosScheduleStep {
                interval: StdDuration::from_secs(1),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()
        .expect("scheduler should build");

    timeout(Duration::from_secs(1), execution.wait_for_insert_count(1))
        .await
        .expect("first step should submit an order");

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            datetime: "2026-04-09 13:30:01.000000".to_string(),
            ask_price1: 101.0,
            bid_price1: 100.0,
            last_price: 100.0,
            ..Quote::default()
        })
        .await;

    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        execution.cancelled_order_ids().await.len(),
        0,
        "crossing a non-trading break must not consume the full scheduler interval"
    );
    assert_eq!(
        execution.inserted_prices().await.len(),
        1,
        "next step should not start until enough trading time has elapsed"
    );

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            datetime: "2026-04-09 13:30:30.000000".to_string(),
            ask_price1: 101.0,
            bid_price1: 100.0,
            last_price: 100.0,
            ..Quote::default()
        })
        .await;

    timeout(Duration::from_secs(1), execution.wait_for_cancel_count(1))
        .await
        .expect("deadline should elapse once sixty trading seconds accumulate");
    timeout(Duration::from_secs(1), execution.wait_for_insert_count(2))
        .await
        .expect("second step should begin after the deadline");

    let last_order_id = execution.latest_order_id().await.expect("last order should exist");
    execution
        .finish_order(
            &last_order_id,
            0,
            vec![make_trade(&last_order_id, "trade-after-break", 1, 101.0)],
        )
        .await;

    timeout(Duration::from_secs(1), scheduler.wait_finished())
        .await
        .expect("scheduler should finish once final step reaches target")
        .expect("scheduler should finish cleanly");
}

#[tokio::test]
async fn scheduler_collects_trade_records_into_execution_report() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));
    let account = runtime.account("SIM").expect("account should exist");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: StdDuration::from_millis(30),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
            TargetPosScheduleStep {
                interval: StdDuration::from_millis(30),
                target_volume: 2,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()
        .expect("scheduler should build");

    timeout(Duration::from_secs(1), scheduler.wait_finished())
        .await
        .expect("scheduler should finish")
        .expect("scheduler should finish cleanly");

    let report = scheduler.execution_report();
    assert_eq!(report.trades.len(), 2, "two steps should contribute two trades");
    assert_eq!(
        report.trades.iter().map(|trade| trade.volume).sum::<i64>(),
        2,
        "execution report should aggregate traded volume across steps"
    );
}

#[tokio::test]
async fn target_pos_task_runs_under_backtest_mode() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-09 09:00:00.000000".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Backtest,
        market,
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .build()
        .expect("target task should build");

    task.set_target_volume(1).expect("target should be accepted");
    task.wait_target_reached()
        .await
        .expect("backtest mode should reach target");

    let position = execution
        .position("SIM", "SHFE.rb2601")
        .await
        .expect("position should be readable");
    assert_eq!(net_position(&position), 1);
}

#[tokio::test]
async fn wait_target_reached_returns_failure_when_task_exits_with_error() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-09 09:00:00.000000".to_string(),
        open_min_market_order_volume: 2,
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Backtest,
        market,
        execution,
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account
        .target_pos("SHFE.rb2601")
        .build()
        .expect("target task should build");

    task.set_target_volume(1).expect("target should be accepted");
    let err = timeout(Duration::from_secs(1), task.wait_target_reached())
        .await
        .expect("wait_target_reached should not hang on task failure")
        .expect_err("task should fail because quote constraint is unsupported");
    assert!(matches!(err, RuntimeError::TargetTaskFailed { .. }));
}

#[tokio::test]
async fn scheduler_runs_under_backtest_mode() {
    let market = Arc::new(FakeMarketAdapter::with_trading_time(
        Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            datetime: "2026-04-09 11:29:30.000000".to_string(),
            ask_price1: 101.0,
            bid_price1: 100.0,
            last_price: 100.0,
            ..Quote::default()
        },
        json!({
            "day": [["09:00:00", "11:30:00"], ["13:30:00", "15:00:00"]],
            "night": []
        }),
    ));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Backtest,
        market.clone(),
        execution.clone(),
    ));
    let account = runtime.account("SIM").expect("account should exist");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: StdDuration::from_secs(60),
                target_volume: 0,
                price_mode: None,
            },
            TargetPosScheduleStep {
                interval: StdDuration::from_secs(1),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()
        .expect("scheduler should build");

    sleep(Duration::from_millis(50)).await;
    assert!(
        !scheduler.is_finished(),
        "scheduler should still be waiting during the midday break"
    );

    market
        .update_quote(Quote {
            instrument_id: "SHFE.rb2601".to_string(),
            exchange_id: "SHFE".to_string(),
            datetime: "2026-04-09 13:30:30.000000".to_string(),
            ask_price1: 101.0,
            bid_price1: 100.0,
            last_price: 100.0,
            ..Quote::default()
        })
        .await;

    timeout(Duration::from_secs(1), scheduler.wait_finished())
        .await
        .expect("scheduler should finish under backtest mode")
        .expect("scheduler should finish cleanly");

    let position = execution
        .position("SIM", "SHFE.rb2601")
        .await
        .expect("position should be readable");
    assert_eq!(net_position(&position), 1);
}

struct FakeMarketAdapter {
    quote: RwLock<Quote>,
    trading_time: RwLock<Option<serde_json::Value>>,
    updates_tx: broadcast::Sender<()>,
}

impl FakeMarketAdapter {
    fn new(initial_quote: Quote) -> Self {
        Self::with_optional_trading_time(initial_quote, None)
    }

    fn with_trading_time(initial_quote: Quote, trading_time: serde_json::Value) -> Self {
        Self::with_optional_trading_time(initial_quote, Some(trading_time))
    }

    fn with_optional_trading_time(initial_quote: Quote, trading_time: Option<serde_json::Value>) -> Self {
        let (updates_tx, _) = broadcast::channel(32);
        Self {
            quote: RwLock::new(initial_quote),
            trading_time: RwLock::new(trading_time),
            updates_tx,
        }
    }

    async fn update_quote(&self, quote: Quote) {
        *self.quote.write().await = quote;
        let _ = self.updates_tx.send(());
    }
}

#[async_trait]
impl MarketAdapter for FakeMarketAdapter {
    async fn latest_quote(&self, _symbol: &str) -> RuntimeResult<Quote> {
        Ok(self.quote.read().await.clone())
    }

    async fn trading_time(&self, _symbol: &str) -> RuntimeResult<Option<serde_json::Value>> {
        Ok(self.trading_time.read().await.clone())
    }

    async fn wait_quote_update(&self, _symbol: &str) -> RuntimeResult<()> {
        let mut rx = self.updates_tx.subscribe();
        rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
            resource: "fake market updates",
        })
    }
}

struct FakeExecutionAdapter {
    state: Mutex<FakeExecutionState>,
    updates_tx: broadcast::Sender<String>,
}

struct FakeExecutionState {
    next_order_seq: usize,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    symbol_trades: Vec<Trade>,
    inserted_prices: Vec<f64>,
    cancelled_order_ids: Vec<String>,
    position: Position,
    autofill_on_insert: bool,
}

impl Default for FakeExecutionAdapter {
    fn default() -> Self {
        Self::with_position(make_position("", ""), false)
    }
}

impl FakeExecutionAdapter {
    fn with_position(position: Position, autofill_on_insert: bool) -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            state: Mutex::new(FakeExecutionState {
                next_order_seq: 0,
                orders: HashMap::new(),
                trades_by_order: HashMap::new(),
                symbol_trades: Vec::new(),
                inserted_prices: Vec::new(),
                cancelled_order_ids: Vec::new(),
                position,
                autofill_on_insert,
            }),
            updates_tx,
        }
    }

    async fn wait_for_insert_count(&self, expected: usize) {
        loop {
            if self.state.lock().await.inserted_prices.len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_cancel_count(&self, expected: usize) {
        loop {
            if self.state.lock().await.cancelled_order_ids.len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn latest_order_id(&self) -> Option<String> {
        let state = self.state.lock().await;
        state.orders.keys().cloned().max()
    }

    async fn alive_order_ids(&self) -> Vec<String> {
        let state = self.state.lock().await;
        state
            .orders
            .iter()
            .filter_map(|(order_id, order)| {
                if order.status == ORDER_STATUS_ALIVE {
                    Some(order_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn set_symbol_trades(&self, trades: Vec<Trade>) {
        self.state.lock().await.symbol_trades = trades;
    }

    async fn finish_order(&self, order_id: &str, volume_left: i64, trades: Vec<Trade>) {
        let mut state = self.state.lock().await;
        if let Some(order) = state.orders.get_mut(order_id) {
            let filled_delta = (order.volume_orign - volume_left) - (order.volume_orign - order.volume_left);
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.volume_left = volume_left;
            if filled_delta > 0 {
                let order_snapshot = order.clone();
                apply_position_fill(&mut state.position, &order_snapshot, filled_delta);
            }
        }
        state.symbol_trades.extend(trades.iter().cloned());
        state.trades_by_order.insert(order_id.to_string(), trades);
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string());
    }

    async fn finish_order_externally(&self, order_id: &str, volume_left: i64, last_msg: &str) {
        let mut state = self.state.lock().await;
        if let Some(order) = state.orders.get_mut(order_id) {
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.volume_left = volume_left;
            order.exchange_order_id = order_id.to_string();
            order.last_msg = last_msg.to_string();
            order.is_dead = true;
            order.is_online = false;
            order.is_error = false;
        }
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string());
    }

    async fn finish_order_with_error(&self, order_id: &str, volume_left: i64, last_msg: &str) {
        let mut state = self.state.lock().await;
        if let Some(order) = state.orders.get_mut(order_id) {
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.volume_left = volume_left;
            order.exchange_order_id = order_id.to_string();
            order.last_msg = last_msg.to_string();
            order.is_dead = true;
            order.is_online = false;
            order.is_error = true;
        }
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string());
    }

    async fn append_trade(&self, order_id: &str, trade: Trade) {
        let mut state = self.state.lock().await;
        state.symbol_trades.push(trade.clone());
        state
            .trades_by_order
            .entry(order_id.to_string())
            .or_default()
            .push(trade);
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string());
    }

    async fn inserted_prices(&self) -> Vec<f64> {
        self.state.lock().await.inserted_prices.clone()
    }

    async fn cancelled_order_ids(&self) -> Vec<String> {
        self.state.lock().await.cancelled_order_ids.clone()
    }
}

#[async_trait]
impl ExecutionAdapter for FakeExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        vec!["SIM".to_string()]
    }

    async fn insert_order(&self, _account_key: &str, req: &InsertOrderRequest) -> RuntimeResult<String> {
        let mut state = self.state.lock().await;
        state.next_order_seq += 1;
        let order_id = format!("order-{}", state.next_order_seq);
        let trade_id = format!("trade-{}", state.next_order_seq);
        state.inserted_prices.push(req.limit_price);
        let mut order = make_order(&order_id, req.volume, req.limit_price, ORDER_STATUS_ALIVE, req.volume);
        order.direction = req.direction.clone();
        order.offset = req.offset.clone();
        state.orders.insert(order_id.clone(), order);
        if state.autofill_on_insert {
            let order = state
                .orders
                .get_mut(&order_id)
                .expect("inserted order should still exist");
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.volume_left = 0;
            let order_snapshot = order.clone();
            let trade = make_trade(&order_id, &trade_id, req.volume, req.limit_price);
            apply_position_fill(&mut state.position, &order_snapshot, req.volume);
            state.symbol_trades.push(trade.clone());
            state.trades_by_order.insert(order_id.clone(), vec![trade]);
        }
        drop(state);
        let _ = self.updates_tx.send(order_id.clone());
        Ok(order_id)
    }

    async fn cancel_order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let mut state = self.state.lock().await;
        state.cancelled_order_ids.push(order_id.to_string());
        if let Some(order) = state.orders.get_mut(order_id) {
            order.status = ORDER_STATUS_FINISHED.to_string();
        }
        drop(state);
        let _ = self.updates_tx.send(order_id.to_string());
        Ok(())
    }

    async fn order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        self.state
            .lock()
            .await
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| RuntimeError::OrderNotFound {
                account_key: "SIM".to_string(),
                order_id: order_id.to_string(),
            })
    }

    async fn trades_by_order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Ok(self
            .state
            .lock()
            .await
            .trades_by_order
            .get(order_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn trades_for_symbol(&self, _account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>> {
        let Some((exchange_id, instrument_id)) = symbol.split_once('.') else {
            return Ok(Vec::new());
        };

        Ok(self
            .state
            .lock()
            .await
            .symbol_trades
            .iter()
            .filter(|trade| trade.exchange_id == exchange_id && trade.instrument_id == instrument_id)
            .cloned()
            .collect())
    }

    async fn position(&self, _account_key: &str, _symbol: &str) -> RuntimeResult<Position> {
        Ok(self.state.lock().await.position.clone())
    }

    async fn wait_order_update(&self, _account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let mut rx = self.updates_tx.subscribe();
        loop {
            let updated = rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "fake execution updates",
            })?;
            if updated == order_id {
                return Ok(());
            }
        }
    }
}

fn apply_position_fill(position: &mut Position, order: &Order, filled: i64) {
    match (order.direction.as_str(), order.offset.as_str()) {
        (DIRECTION_BUY, OFFSET_OPEN) => {
            position.pos_long_today += filled;
            position.volume_long_today += filled;
            position.volume_long = position.volume_long_today + position.volume_long_his;
        }
        ("SELL", OFFSET_OPEN) => {
            position.pos_short_today += filled;
            position.volume_short_today += filled;
            position.volume_short = position.volume_short_today + position.volume_short_his;
        }
        (DIRECTION_BUY, _) => {
            let close_today = filled.min(position.pos_short_today);
            position.pos_short_today -= close_today;
            position.volume_short_today -= close_today;
            let close_his = filled - close_today;
            position.pos_short_his -= close_his;
            position.volume_short_his -= close_his;
            position.volume_short = position.volume_short_today + position.volume_short_his;
        }
        ("SELL", _) => {
            let close_today = filled.min(position.pos_long_today);
            position.pos_long_today -= close_today;
            position.volume_long_today -= close_today;
            let close_his = filled - close_today;
            position.pos_long_his -= close_his;
            position.volume_long_his -= close_his;
            position.volume_long = position.volume_long_today + position.volume_long_his;
        }
        _ => {}
    }
}

fn make_position(exchange_id: &str, instrument_id: &str) -> Position {
    Position {
        exchange_id: exchange_id.to_string(),
        instrument_id: instrument_id.to_string(),
        user_id: "SIM".to_string(),
        volume_long_today: 0,
        volume_long_his: 0,
        volume_long: 0,
        volume_long_frozen_today: 0,
        volume_long_frozen_his: 0,
        volume_long_frozen: 0,
        volume_short_today: 0,
        volume_short_his: 0,
        volume_short: 0,
        volume_short_frozen_today: 0,
        volume_short_frozen_his: 0,
        volume_short_frozen: 0,
        volume_long_yd: 0,
        volume_short_yd: 0,
        pos_long_his: 0,
        pos_long_today: 0,
        pos_short_his: 0,
        pos_short_today: 0,
        open_price_long: 0.0,
        open_price_short: 0.0,
        open_cost_long: 0.0,
        open_cost_short: 0.0,
        position_price_long: 0.0,
        position_price_short: 0.0,
        position_cost_long: 0.0,
        position_cost_short: 0.0,
        last_price: 0.0,
        float_profit_long: 0.0,
        float_profit_short: 0.0,
        float_profit: 0.0,
        position_profit_long: 0.0,
        position_profit_short: 0.0,
        position_profit: 0.0,
        margin_long: 0.0,
        margin_short: 0.0,
        margin: 0.0,
        market_value_long: 0.0,
        market_value_short: 0.0,
        market_value: 0.0,
        pos: 0,
        pos_long: 0,
        pos_short: 0,
        epoch: None,
    }
}

fn make_position_with_long(exchange_id: &str, instrument_id: &str, today: i64, his: i64) -> Position {
    let mut position = make_position(exchange_id, instrument_id);
    position.pos_long_today = today;
    position.pos_long_his = his;
    position.volume_long_today = today;
    position.volume_long_his = his;
    position.volume_long = today + his;
    position.pos_long = today + his;
    position.pos = position.pos_long;
    position
}

fn net_position(position: &Position) -> i64 {
    (position.pos_long_today + position.pos_long_his) - (position.pos_short_today + position.pos_short_his)
}

fn make_order(order_id: &str, volume: i64, limit_price: f64, status: &str, volume_left: i64) -> Order {
    Order {
        seqno: 0,
        user_id: "SIM".to_string(),
        order_id: order_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume_orign: volume,
        price_type: "LIMIT".to_string(),
        limit_price,
        time_condition: "GFD".to_string(),
        volume_condition: "ANY".to_string(),
        insert_date_time: 0,
        exchange_order_id: String::new(),
        status: status.to_string(),
        volume_left,
        frozen_margin: 0.0,
        frozen_premium: 0.0,
        last_msg: String::new(),
        is_dead: status == ORDER_STATUS_FINISHED,
        is_online: false,
        is_error: false,
        trade_price: 0.0,
        epoch: None,
    }
}

fn make_trade(order_id: &str, trade_id: &str, volume: i64, price: f64) -> Trade {
    Trade {
        seqno: 0,
        user_id: "SIM".to_string(),
        trade_id: trade_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        order_id: order_id.to_string(),
        exchange_trade_id: String::new(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume,
        price,
        trade_date_time: 0,
        commission: 0.0,
        epoch: None,
    }
}

fn shanghai_nanos(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap()
}

fn make_trade_at(order_id: &str, trade_id: &str, volume: i64, price: f64, trade_date_time: i64) -> Trade {
    Trade {
        seqno: 0,
        user_id: "SIM".to_string(),
        trade_id: trade_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        order_id: order_id.to_string(),
        exchange_trade_id: String::new(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume,
        price,
        trade_date_time,
        commission: 0.0,
        epoch: None,
    }
}
