//! Series API 模块
//!
//! 提供 K 线与 Tick 订阅能力，支持：
//! - 单合约 K 线订阅
//! - 多合约对齐 K 线订阅
//! - Tick 订阅
//! - 历史窗口订阅与焦点跳转
//! - 回调与流式消费两种数据处理模式

mod api;
mod processing;
mod subscription;

#[cfg(test)]
mod tests;

use crate::auth::Authenticator;
use crate::cache::data_series::DataSeriesCache;
use crate::datamanager::DataManager;
use crate::types::{SeriesData, UpdateInfo};
use crate::websocket::TqQuoteWebsocket;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use crate::types::SeriesOptions;

type UpdateCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<SeriesData>, Arc<UpdateInfo>) + Send + Sync>>>>;
type SeriesCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<SeriesData>) + Send + Sync>>>>;
type SeriesErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<String>) + Send + Sync>>>>;
type SeriesStreamSubscribers = Arc<RwLock<Vec<tokio::sync::mpsc::Sender<Arc<SeriesData>>>>>;

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
pub struct SeriesAPI {
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
/// - 更新/新 Bar/错误回调注册
/// - 流式消费
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

    // 回调（使用 Arc 避免数据克隆）
    on_update: UpdateCallback,
    on_new_bar: SeriesCallback,
    on_bar_update: SeriesCallback,
    on_error: SeriesErrorCallback,
    stream_subscribers: SeriesStreamSubscribers,

    running: Arc<RwLock<bool>>,
    unsubscribe_sent: Arc<std::sync::atomic::AtomicBool>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}
