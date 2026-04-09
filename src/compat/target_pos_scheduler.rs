use std::sync::Arc;

use crate::runtime::{
    RuntimeResult, TargetPosExecutionReport, TargetPosScheduleStep, TargetPosSchedulerConfig, TargetPosSchedulerHandle,
    TqRuntime,
};

pub type TargetPosSchedulerOptions = TargetPosSchedulerConfig;

#[derive(Clone)]
pub struct TargetPosScheduler {
    inner: TargetPosSchedulerHandle,
}

impl TargetPosScheduler {
    pub async fn new(
        runtime: Arc<TqRuntime>,
        account_key: impl AsRef<str>,
        symbol: impl Into<String>,
        steps: Vec<TargetPosScheduleStep>,
        options: TargetPosSchedulerOptions,
    ) -> RuntimeResult<Self> {
        let account = runtime.account(account_key.as_ref())?;
        let inner = account
            .target_pos_scheduler(symbol)
            .steps(steps)
            .config(options)
            .build()?;
        Ok(Self { inner })
    }

    pub fn account_key(&self) -> &str {
        self.inner.account().account_key()
    }

    pub fn symbol(&self) -> &str {
        self.inner.symbol()
    }

    pub fn config(&self) -> &TargetPosSchedulerOptions {
        self.inner.config()
    }

    pub fn steps(&self) -> &[TargetPosScheduleStep] {
        self.inner.steps()
    }

    pub async fn cancel(&self) -> RuntimeResult<()> {
        self.inner.cancel().await
    }

    pub fn is_finished(&self) -> bool {
        self.inner.is_finished()
    }

    pub async fn wait_finished(&self) -> RuntimeResult<()> {
        self.inner.wait_finished().await
    }

    pub fn execution_report(&self) -> TargetPosExecutionReport {
        self.inner.execution_report()
    }

    pub fn trades(&self) -> Vec<crate::types::Trade> {
        self.inner.trades()
    }

    pub fn into_inner(self) -> TargetPosSchedulerHandle {
        self.inner
    }

    pub fn inner(&self) -> &TargetPosSchedulerHandle {
        &self.inner
    }
}
