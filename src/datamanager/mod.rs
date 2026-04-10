//! 数据管理器
//!
//! 实现 DIFF 协议的数据合并和管理，包括：
//! - DIFF 数据递归合并
//! - 路径访问和版本追踪
//! - Watch/UnWatch 路径监听
//! - 数据类型转换

mod core;
mod merge;
mod query;
mod watch;

#[cfg(test)]
mod tests;

use async_channel::Sender;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, RwLock};
use tokio::sync::watch::Sender as WatchSender;

struct DataCallbackEntry {
    id: i64,
    f: Arc<dyn Fn() + Send + Sync>,
}

type DataCallbacks = Arc<RwLock<Vec<DataCallbackEntry>>>;

#[derive(Debug, Clone, Default)]
pub struct MergeSemanticsConfig {
    pub persist: bool,
    pub reduce_diff: bool,
    pub prototype: Option<Value>,
}

/// 数据管理器配置
#[derive(Debug, Clone)]
pub struct DataManagerConfig {
    /// 默认视图宽度
    pub default_view_width: usize,
    /// 启用自动清理
    pub enable_auto_cleanup: bool,
    /// 路径监听通道容量
    pub watch_channel_capacity: usize,
    pub merge_semantics: MergeSemanticsConfig,
}

impl Default for DataManagerConfig {
    fn default() -> Self {
        Self {
            default_view_width: 10000,
            enable_auto_cleanup: true,
            watch_channel_capacity: 1024,
            merge_semantics: MergeSemanticsConfig::default(),
        }
    }
}

/// 路径监听器
struct PathWatcher {
    id: i64,
    path: Vec<String>,
    tx: Sender<Value>,
}

/// 数据管理器
///
/// 管理所有 DIFF 协议数据，支持：
/// - 递归合并
/// - 版本追踪
/// - 路径监听
pub struct DataManager {
    /// 数据存储
    data: Arc<RwLock<HashMap<String, Value>>>,
    /// 版本号（使用原子变量以提高性能）
    epoch: AtomicI64,
    /// merge 完成后的最终 epoch 通知
    epoch_tx: WatchSender<i64>,
    /// 配置
    config: DataManagerConfig,
    /// 路径监听器
    watchers: Arc<RwLock<HashMap<String, Vec<PathWatcher>>>>,
    /// 数据更新回调（使用 Arc 以支持异步触发）
    on_data_callbacks: DataCallbacks,
    /// 回调 ID 生成器
    next_callback_id: AtomicI64,
    /// watcher ID 生成器
    next_watcher_id: AtomicI64,
}

#[derive(Clone)]
struct MergeOptions {
    delete_null: bool,
    persist: bool,
    reduce_diff: bool,
    prototype: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrototypeBranch {
    Direct,
    Star,
    At,
    Hash,
    None,
}

impl MergeOptions {
    fn new(delete_null: bool, semantics: MergeSemanticsConfig) -> Self {
        let prototype = semantics.prototype.unwrap_or_else(|| Value::Object(Map::new()));
        Self {
            delete_null,
            persist: semantics.persist,
            reduce_diff: semantics.reduce_diff,
            prototype,
        }
    }
}
