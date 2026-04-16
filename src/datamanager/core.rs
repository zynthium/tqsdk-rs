use super::{DataManager, DataManagerConfig};
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
            next_watcher_id: std::sync::atomic::AtomicI64::new(1),
            numeric_value_index_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 获取当前版本号
    pub fn get_epoch(&self) -> i64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// 订阅 merge 完成后的全局 epoch
    pub fn subscribe_epoch(&self) -> tokio::sync::watch::Receiver<i64> {
        self.epoch_tx.subscribe()
    }
}
