use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::datamanager::{DataManager, DataManagerConfig};
use crate::runtime::{
    BacktestExecutionAdapter, ExecutionAdapter, MarketAdapter, OffsetPriority, PriceMode, RuntimeError, RuntimeMode,
    TargetPosConfig, TargetPosScheduleStep, TaskRegistry, TqRuntime,
};
use crate::types::{DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, PRICE_TYPE_LIMIT};
use async_trait::async_trait;

#[test]
fn target_pos_config_default_uses_active_price_mode() {
    let cfg = TargetPosConfig::default();
    assert!(matches!(cfg.price_mode, PriceMode::Active));
    assert!(matches!(
        cfg.offset_priority,
        OffsetPriority::TodayYesterdayThenOpenWait
    ));
    assert!(cfg.split_policy.is_none());
}

#[test]
fn task_registry_reuses_same_key_same_config() {
    let registry = TaskRegistry::default();
    let cfg = TargetPosConfig::default();

    let first = registry
        .register_target_task("runtime-1", "SIM", "SHFE.rb2601", &cfg)
        .expect("first task should register");
    let second = registry
        .register_target_task("runtime-1", "SIM", "SHFE.rb2601", &cfg)
        .expect("same config should reuse task");

    assert_eq!(first.task_id, second.task_id);
    assert!(first.created);
    assert!(!second.created);
}

#[test]
fn task_registry_rejects_same_key_different_config() {
    let registry = TaskRegistry::default();
    let cfg = TargetPosConfig::default();

    registry
        .register_target_task("runtime-1", "SIM", "SHFE.rb2601", &cfg)
        .expect("first task should register");

    let err = registry
        .register_target_task(
            "runtime-1",
            "SIM",
            "SHFE.rb2601",
            &TargetPosConfig {
                offset_priority: OffsetPriority::OpenOnly,
                ..TargetPosConfig::default()
            },
        )
        .expect_err("different config should conflict");

    assert!(matches!(
        err,
        RuntimeError::TaskConflict {
            runtime_id,
            account_key,
            symbol,
        } if runtime_id == "runtime-1" && account_key == "SIM" && symbol == "SHFE.rb2601"
    ));
}

#[test]
fn task_registry_tracks_order_owner() {
    let registry = TaskRegistry::default();
    let cfg = TargetPosConfig::default();
    let task = registry
        .register_target_task("runtime-1", "SIM", "SHFE.rb2601", &cfg)
        .expect("task should register");

    registry.bind_order_owner("order-1", task.task_id);

    assert_eq!(registry.order_owner("order-1"), Some(task.task_id));
}

#[tokio::test]
async fn runtime_exposes_account_handle_for_registered_account() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(FakeExecutionAdapter {
        accounts: vec!["SIM".to_string()],
    });
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));

    let account = runtime.account("SIM").expect("registered account should be exposed");

    assert_eq!(account.account_key(), "SIM");
    assert_eq!(account.runtime_id(), "runtime-1");
}

#[tokio::test]
async fn runtime_can_be_constructed_with_backtest_execution_mode() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(BacktestExecutionAdapter::new(vec!["SIM".to_string()]));
    let runtime = Arc::new(TqRuntime::with_id(
        "runtime-1",
        RuntimeMode::Backtest,
        market,
        execution,
    ));

    let account = runtime
        .account("SIM")
        .expect("backtest runtime should expose configured account");

    assert_eq!(runtime.mode(), RuntimeMode::Backtest);
    assert_eq!(account.account_key(), "SIM");
}

#[tokio::test]
async fn manual_insert_order_is_blocked_while_target_task_owns_symbol() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(FakeExecutionAdapter {
        accounts: vec!["SIM".to_string()],
    });
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));
    let account = runtime.account("SIM").expect("registered account should be exposed");
    let _task = account
        .target_pos("SHFE.rb2601")
        .build()
        .expect("target task should build");

    let err = account
        .insert_order(&InsertOrderRequest {
            symbol: "SHFE.rb2601".to_string(),
            exchange_id: None,
            instrument_id: None,
            direction: DIRECTION_BUY.to_string(),
            offset: OFFSET_OPEN.to_string(),
            price_type: PRICE_TYPE_LIMIT.to_string(),
            limit_price: 101.0,
            volume: 1,
        })
        .await
        .expect_err("manual insert should be blocked while target task owns the symbol");

    assert!(matches!(
        err,
        RuntimeError::ManualOrderConflict { account_key, symbol }
        if account_key == "SIM" && symbol == "SHFE.rb2601"
    ));
}

#[tokio::test]
async fn compat_target_pos_task_wraps_runtime_handle() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(FakeExecutionAdapter {
        accounts: vec!["SIM".to_string()],
    });
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));

    let task = crate::compat::TargetPosTask::new(
        runtime,
        "SIM",
        "SHFE.rb2601",
        crate::compat::TargetPosTaskOptions::default(),
    )
    .await
    .expect("compat target task should build");

    assert_eq!(task.account_key(), "SIM");
    assert_eq!(task.symbol(), "SHFE.rb2601");
}

#[tokio::test]
async fn compat_target_pos_scheduler_wraps_runtime_handle() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(FakeExecutionAdapter {
        accounts: vec!["SIM".to_string()],
    });
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));

    let scheduler = crate::compat::TargetPosScheduler::new(
        runtime,
        "SIM",
        "SHFE.rb2601",
        vec![TargetPosScheduleStep {
            interval: std::time::Duration::from_millis(1),
            target_volume: 0,
            price_mode: None,
        }],
        crate::compat::TargetPosSchedulerOptions::default(),
    )
    .await
    .expect("compat target scheduler should build");

    scheduler
        .wait_finished()
        .await
        .expect("empty compat target scheduler should finish");
    assert_eq!(scheduler.account_key(), "SIM");
    assert_eq!(scheduler.symbol(), "SHFE.rb2601");
}

#[tokio::test]
async fn manual_insert_order_is_blocked_while_scheduler_wait_step_owns_symbol() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let market = Arc::new(FakeMarketAdapter { dm: Arc::clone(&dm) });
    let execution = Arc::new(FakeExecutionAdapter {
        accounts: vec!["SIM".to_string()],
    });
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));
    let account = runtime.account("SIM").expect("registered account should be exposed");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: Duration::from_secs(5),
                target_volume: 0,
                price_mode: None,
            },
            TargetPosScheduleStep {
                interval: Duration::from_secs(1),
                target_volume: 0,
                price_mode: None,
            },
        ])
        .build()
        .expect("scheduler should build");

    let err = account
        .insert_order(&InsertOrderRequest {
            symbol: "SHFE.rb2601".to_string(),
            exchange_id: None,
            instrument_id: None,
            direction: DIRECTION_BUY.to_string(),
            offset: OFFSET_OPEN.to_string(),
            price_type: PRICE_TYPE_LIMIT.to_string(),
            limit_price: 101.0,
            volume: 1,
        })
        .await
        .expect_err("manual insert should be blocked while scheduler owns the symbol");

    assert!(matches!(
        err,
        RuntimeError::ManualOrderConflict { account_key, symbol }
        if account_key == "SIM" && symbol == "SHFE.rb2601"
    ));

    scheduler.cancel().await.expect("cancel should be accepted");
    scheduler
        .wait_finished()
        .await
        .expect("scheduler should finish after cancel");
}

struct FakeMarketAdapter {
    dm: Arc<DataManager>,
}

#[async_trait]
impl MarketAdapter for FakeMarketAdapter {
    fn dm(&self) -> Arc<DataManager> {
        Arc::clone(&self.dm)
    }
}

#[derive(Debug)]
struct FakeExecutionAdapter {
    accounts: Vec<String>,
}

#[async_trait]
impl ExecutionAdapter for FakeExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }
}
