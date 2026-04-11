use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    AccountHandle, ExecutionAdapter, ExecutionEngine, MarketAdapter, RuntimeError, RuntimeMode, RuntimeResult,
    TaskRegistry,
};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

pub struct TqRuntime {
    id: String,
    mode: RuntimeMode,
    registry: Arc<TaskRegistry>,
    market: Arc<dyn MarketAdapter>,
    execution: Arc<dyn ExecutionAdapter>,
    engine: Arc<ExecutionEngine>,
}

impl TqRuntime {
    pub(crate) fn new(mode: RuntimeMode, market: Arc<dyn MarketAdapter>, execution: Arc<dyn ExecutionAdapter>) -> Self {
        let id = format!("runtime-{}", NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed));
        Self::with_id(id, mode, market, execution)
    }

    pub(crate) fn with_id(
        id: impl Into<String>,
        mode: RuntimeMode,
        market: Arc<dyn MarketAdapter>,
        execution: Arc<dyn ExecutionAdapter>,
    ) -> Self {
        Self {
            id: id.into(),
            mode,
            registry: Arc::new(TaskRegistry::default()),
            market,
            execution,
            engine: Arc::new(ExecutionEngine),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn mode(&self) -> RuntimeMode {
        self.mode
    }

    pub(crate) fn registry(&self) -> Arc<TaskRegistry> {
        Arc::clone(&self.registry)
    }

    pub(crate) fn market(&self) -> Arc<dyn MarketAdapter> {
        Arc::clone(&self.market)
    }

    pub(crate) fn execution(&self) -> Arc<dyn ExecutionAdapter> {
        Arc::clone(&self.execution)
    }

    pub(crate) fn engine(&self) -> Arc<ExecutionEngine> {
        Arc::clone(&self.engine)
    }

    pub fn account(self: &Arc<Self>, account_key: &str) -> RuntimeResult<AccountHandle> {
        if !self.execution.has_account(account_key) {
            return Err(RuntimeError::AccountNotFound {
                account_key: account_key.to_string(),
            });
        }

        Ok(AccountHandle::new(Arc::clone(self), account_key))
    }
}
