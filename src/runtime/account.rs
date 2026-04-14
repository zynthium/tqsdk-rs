use std::sync::Arc;

use crate::trade_session::{OrderEventStream, TradeEventStream, TradeOnlyEventStream};
use crate::types::InsertOrderRequest;

use super::{TargetPosBuilder, TargetPosSchedulerBuilder, TqRuntime};

#[derive(Clone)]
pub struct AccountHandle {
    runtime: Arc<TqRuntime>,
    account_key: String,
}

impl AccountHandle {
    pub(crate) fn new(runtime: Arc<TqRuntime>, account_key: impl Into<String>) -> Self {
        Self {
            runtime,
            account_key: account_key.into(),
        }
    }

    pub fn account_key(&self) -> &str {
        &self.account_key
    }

    pub fn runtime_id(&self) -> &str {
        self.runtime.id()
    }

    pub fn runtime(&self) -> Arc<TqRuntime> {
        Arc::clone(&self.runtime)
    }

    pub fn target_pos(&self, symbol: impl Into<String>) -> TargetPosBuilder {
        TargetPosBuilder::new(self.clone(), symbol)
    }

    pub fn target_pos_scheduler(&self, symbol: impl Into<String>) -> TargetPosSchedulerBuilder {
        TargetPosSchedulerBuilder::new(self.clone(), symbol)
    }

    pub async fn insert_order(&self, req: &InsertOrderRequest) -> super::RuntimeResult<String> {
        self.runtime
            .engine()
            .insert_manual_order(&self.runtime, &self.account_key, req)
            .await
    }

    pub fn subscribe_events(&self) -> super::RuntimeResult<TradeEventStream> {
        self.runtime.execution().subscribe_events(&self.account_key)
    }

    pub fn subscribe_order_events(&self) -> super::RuntimeResult<OrderEventStream> {
        self.runtime.execution().subscribe_order_events(&self.account_key)
    }

    pub fn subscribe_trade_events(&self) -> super::RuntimeResult<TradeOnlyEventStream> {
        self.runtime.execution().subscribe_trade_events(&self.account_key)
    }
}
