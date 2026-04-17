//! Series API 模块
//!
//! 提供 K 线与 Tick 序列能力，支持：
//! - Python 对齐的 bounded serial 订阅（`get_kline_serial` / `get_tick_serial`）
//! - 一次性历史 K 线 / Tick 下载（`*_data_series`）
//! - 快照式窗口状态读取

mod api;
mod processing;
mod subscription;

#[cfg(test)]
mod tests;

use crate::auth::Authenticator;
use crate::cache::DataSeriesCache;
use crate::datamanager::DataManager;
use crate::types::SeriesSnapshot;
use crate::websocket::TqQuoteWebsocket;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

pub use crate::types::SeriesOptions;

/// Series 磁盘缓存策略（作用于 Python-compatible DataSeries 缓存）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SeriesCachePolicy {
    pub enabled: bool,
    pub max_bytes: Option<u64>,
    pub retention_days: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct KlineSymbols(Vec<String>);

impl KlineSymbols {
    fn into_vec(self) -> Vec<String> {
        self.0
    }
}

/// Series API 入口。
///
/// 该类型负责创建并管理 `SeriesSubscription`，用于按图表维度持续接收
/// K 线或 Tick 数据更新。
#[derive(Clone)]
pub(crate) struct SeriesAPI {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    auth: Arc<RwLock<dyn Authenticator>>,
    data_series_cache: Arc<DataSeriesCache>,
    cache_policy: SeriesCachePolicy,
}

/// Series 订阅句柄。
///
/// 该类型封装单次订阅生命周期，支持：
/// - 启停与刷新
/// - 快照式状态等待与读取
/// - 主动关闭
#[derive(Clone)]
pub struct SeriesSubscription {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    options: SeriesOptions,

    // 状态跟踪
    last_ids: Arc<RwLock<HashMap<String, i64>>>,
    last_left_id: Arc<RwLock<i64>>,
    last_right_id: Arc<RwLock<i64>>,
    chart_ready: Arc<RwLock<bool>>,
    has_chart_sync: Arc<RwLock<bool>>,

    running: Arc<RwLock<bool>>,
    unsubscribe_sent: Arc<std::sync::atomic::AtomicBool>,
    snapshot_tx: tokio::sync::watch::Sender<Option<SeriesSnapshot>>,
    wait_rx: Arc<tokio::sync::Mutex<tokio::sync::watch::Receiver<Option<SeriesSnapshot>>>>,
    latest_snapshot: Arc<RwLock<Option<SeriesSnapshot>>>,
    watch_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}
