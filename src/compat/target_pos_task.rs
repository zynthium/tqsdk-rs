use std::sync::Arc;

use crate::runtime::{RuntimeResult, TargetPosConfig, TargetPosHandle, TaskId, TqRuntime};

pub type TargetPosTaskOptions = TargetPosConfig;

#[derive(Clone)]
pub struct TargetPosTask {
    inner: TargetPosHandle,
}

impl TargetPosTask {
    pub async fn new(
        runtime: Arc<TqRuntime>,
        account_key: impl AsRef<str>,
        symbol: impl Into<String>,
        options: TargetPosTaskOptions,
    ) -> RuntimeResult<Self> {
        let account = runtime.account(account_key.as_ref())?;
        let inner = account.target_pos(symbol).config(options).build()?;
        Ok(Self { inner })
    }

    pub fn account_key(&self) -> &str {
        self.inner.account().account_key()
    }

    pub fn symbol(&self) -> &str {
        self.inner.symbol()
    }

    pub fn config(&self) -> &TargetPosTaskOptions {
        self.inner.config()
    }

    pub fn task_id(&self) -> TaskId {
        self.inner.task_id()
    }

    pub fn set_target_volume(&self, volume: i64) -> RuntimeResult<()> {
        self.inner.set_target_volume(volume)
    }

    pub async fn wait_target_reached(&self) -> RuntimeResult<()> {
        self.inner.wait_target_reached().await
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

    pub fn into_inner(self) -> TargetPosHandle {
        self.inner
    }

    pub fn inner(&self) -> &TargetPosHandle {
        &self.inner
    }
}
