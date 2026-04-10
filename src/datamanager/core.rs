use super::{DataCallbackEntry, DataManager, DataManagerConfig};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

impl DataManager {
    /// 创建新的数据管理器
    pub fn new(initial_data: HashMap<String, serde_json::Value>, config: DataManagerConfig) -> Self {
        let (epoch_tx, _) = tokio::sync::watch::channel(0i64);
        Self {
            data: Arc::new(RwLock::new(initial_data)),
            epoch: std::sync::atomic::AtomicI64::new(0),
            epoch_tx,
            config,
            watchers: Arc::new(RwLock::new(HashMap::new())),
            on_data_callbacks: Arc::new(RwLock::new(Vec::new())),
            next_callback_id: std::sync::atomic::AtomicI64::new(1),
            next_watcher_id: std::sync::atomic::AtomicI64::new(1),
        }
    }

    /// 注册数据更新回调（无句柄版本，兼容旧调用）
    pub fn on_data<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let _ = self.on_data_register(callback);
    }

    /// 注册数据更新回调，返回句柄 ID，用于后续注销
    pub fn on_data_register<F>(&self, callback: F) -> i64
    where
        F: Fn() + Send + Sync + 'static,
    {
        let id = self.next_callback_id.fetch_add(1, Ordering::SeqCst);
        let mut callbacks = self.on_data_callbacks.write().unwrap();
        callbacks.push(DataCallbackEntry {
            id,
            f: Arc::new(callback),
        });
        id
    }

    /// 注销数据更新回调
    pub fn off_data(&self, id: i64) -> bool {
        let mut callbacks = self.on_data_callbacks.write().unwrap();
        let before = callbacks.len();
        callbacks.retain(|entry| entry.id != id);
        callbacks.len() < before
    }

    /// 获取当前版本号
    pub fn get_epoch(&self) -> i64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// 订阅 merge 完成后的全局 epoch
    pub fn subscribe_epoch(&self) -> tokio::sync::watch::Receiver<i64> {
        self.epoch_tx.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn callback_count_for_test(&self) -> usize {
        self.on_data_callbacks.read().unwrap().len()
    }
}
