use std::collections::HashMap;
use std::sync::Arc;

use crate::datamanager::{DataManager, DataManagerConfig};
use crate::runtime::{
    ExecutionAdapter, MarketAdapter, OffsetPriority, PriceMode, RuntimeError, RuntimeMode, TargetPosConfig,
    TaskRegistry, TqRuntime,
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
