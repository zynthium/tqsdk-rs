//! Series API 模块
//!
//! 提供 K 线与 Tick 订阅能力，支持：
//! - 单合约 K 线订阅
//! - 多合约对齐 K 线订阅
//! - Tick 订阅
//! - 历史窗口订阅与焦点跳转
//! - 回调与流式消费两种数据处理模式

use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::types::{ChartInfo, SeriesData, UpdateInfo};
use crate::websocket::TqQuoteWebsocket;
use async_stream::stream;
use chrono::{DateTime, Utc};
use futures::Stream;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};
use uuid::Uuid;

use crate::auth::Authenticator;

pub use crate::types::SeriesOptions;

type UpdateCallback =
    Arc<RwLock<Option<Arc<dyn Fn(Arc<SeriesData>, Arc<UpdateInfo>) + Send + Sync>>>>;
type SeriesCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<SeriesData>) + Send + Sync>>>>;
type SeriesErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<String>) + Send + Sync>>>>;

#[derive(Debug, Clone)]
pub struct KlineSymbols(Vec<String>);

impl KlineSymbols {
    fn into_vec(self) -> Vec<String> {
        self.0
    }
}

impl From<&str> for KlineSymbols {
    fn from(value: &str) -> Self {
        KlineSymbols(vec![value.to_string()])
    }
}

impl From<String> for KlineSymbols {
    fn from(value: String) -> Self {
        KlineSymbols(vec![value])
    }
}

impl From<&String> for KlineSymbols {
    fn from(value: &String) -> Self {
        KlineSymbols(vec![value.clone()])
    }
}

impl From<Vec<String>> for KlineSymbols {
    fn from(value: Vec<String>) -> Self {
        KlineSymbols(value)
    }
}

impl From<&[String]> for KlineSymbols {
    fn from(value: &[String]) -> Self {
        KlineSymbols(value.to_vec())
    }
}

impl From<&Vec<String>> for KlineSymbols {
    fn from(value: &Vec<String>) -> Self {
        KlineSymbols(value.clone())
    }
}

impl<'a> From<&'a [&'a str]> for KlineSymbols {
    fn from(value: &'a [&'a str]) -> Self {
        KlineSymbols(value.iter().map(|s| (*s).to_string()).collect())
    }
}

impl<'a> From<Vec<&'a str>> for KlineSymbols {
    fn from(value: Vec<&'a str>) -> Self {
        KlineSymbols(value.into_iter().map(|s| s.to_string()).collect())
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
}

impl SeriesAPI {
    /// 创建 Series API 实例。
    pub fn new(
        dm: Arc<DataManager>,
        ws: Arc<TqQuoteWebsocket>,
        auth: Arc<RwLock<dyn Authenticator>>,
    ) -> Self {
        SeriesAPI { dm, ws, auth }
    }

    /// 获取 K 线序列订阅（单合约/多合约对齐），对齐 tqsdk-python 的 `get_kline_serial`。
    ///
    /// # 参数
    /// - `symbols`: 单合约或合约列表。元素数量为 1 时订阅单合约，>1 时订阅多合约对齐序列
    /// - `duration`: K 线周期（整秒）；日线以上必须为 86400 的整数倍
    /// - `data_length`: 数据条数，上限 10000；多合约模式下最小为 30
    ///
    /// # 返回
    /// 返回未启动的订阅句柄 `SeriesSubscription`，需显式调用 `start()`。
    ///
    /// # 错误
    /// 当参数无效、权限不足或网络发送失败时返回错误。
    pub async fn kline<T>(
        &self,
        symbols: T,
        duration: StdDuration,
        data_length: usize,
    ) -> Result<Arc<SeriesSubscription>>
    where
        T: Into<KlineSymbols>,
    {
        let symbols = symbols.into().into_vec();
        let options = build_realtime_kline_options(symbols, duration, data_length)?;
        self.subscribe(options).await
    }

    /// 获取 Tick 序列订阅，对齐 tqsdk-python 的 `get_tick_serial`。
    ///
    /// # 参数
    /// - `symbol`: 合约代码
    /// - `data_length`: 数据条数，上限 10000
    ///
    /// # 返回
    /// 返回未启动的 Tick 订阅句柄 `SeriesSubscription`，需显式调用 `start()`。
    ///
    /// # 错误
    /// 当参数无效、权限不足或网络发送失败时返回错误。
    pub async fn tick(&self, symbol: &str, data_length: usize) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "symbol 不能为空字符串".to_string(),
            ));
        }
        let view_width = normalize_data_length(data_length)?;
        self.subscribe(SeriesOptions {
            symbols: vec![symbol.to_string()],
            duration: 0,
            view_width,
            chart_id: None,
            left_kline_id: None,
            focus_datetime: None,
            focus_position: None,
        })
        .await
    }

    /// 订阅历史 K 线（通过 `left_kline_id` 定位窗口左边界）。
    ///
    /// # 参数
    /// - `symbol`: 合约代码
    /// - `duration`: K 线周期
    /// - `view_width`: 图表窗口宽度
    /// - `left_kline_id`: 历史窗口左边界 ID
    ///
    /// # 返回
    /// 返回未启动的历史窗口订阅句柄 `SeriesSubscription`，需显式调用 `start()`。
    ///
    /// # 错误
    /// 当参数无效、权限不足或网络发送失败时返回错误。
    pub async fn kline_history(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        left_kline_id: i64,
    ) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "symbol 不能为空字符串".to_string(),
            ));
        }
        let view_width = normalize_data_length(data_length)?;
        let duration = normalize_kline_duration(duration)?;
        self.subscribe(SeriesOptions {
            symbols: vec![symbol.to_string()],
            duration,
            view_width,
            chart_id: None,
            left_kline_id: Some(left_kline_id),
            focus_datetime: None,
            focus_position: None,
        })
        .await
    }

    /// 订阅历史 K 线（通过时间焦点定位窗口）。
    ///
    /// # 参数
    /// - `symbol`: 合约代码
    /// - `duration`: K 线周期
    /// - `view_width`: 图表窗口宽度
    /// - `focus_time`: 焦点时间（UTC）
    /// - `focus_position`: 焦点在窗口中的位置，`1` 靠右，`-1` 靠左
    ///
    /// # 返回
    /// 返回未启动的历史窗口订阅句柄 `SeriesSubscription`，需显式调用 `start()`。
    ///
    /// # 错误
    /// 当参数无效、权限不足或网络发送失败时返回错误。
    pub async fn kline_history_with_focus(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        focus_time: DateTime<Utc>,
        focus_position: i32,
    ) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "symbol 不能为空字符串".to_string(),
            ));
        }
        let view_width = normalize_data_length(data_length)?;
        let duration = normalize_kline_duration(duration)?;
        self.subscribe(SeriesOptions {
            symbols: vec![symbol.to_string()],
            duration,
            view_width,
            chart_id: None,
            left_kline_id: None,
            focus_datetime: Some(focus_time),
            focus_position: Some(focus_position),
        })
        .await
    }

    /// 通用订阅方法
    async fn subscribe(&self, mut options: SeriesOptions) -> Result<Arc<SeriesSubscription>> {
        if options.symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbols 为空".to_string()));
        }
        if options.symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter(
                "symbols 不能包含空字符串".to_string(),
            ));
        }
        if options.view_width == 0 {
            return Err(TqError::InvalidParameter(
                "data_length 必须大于 0".to_string(),
            ));
        }
        if options.view_width > 10000 {
            options.view_width = 10000;
        }
        {
            // 权限校验
            let auth = self.auth.read().await;
            let symbol_refs: Vec<&str> = options.symbols.iter().map(|s| s.as_str()).collect();
            auth.has_md_grants(&symbol_refs)?;
        }

        // 生成 chart_id
        let has_chart_id = options
            .chart_id
            .as_ref()
            .is_some_and(|chart_id| !chart_id.is_empty());
        if !has_chart_id {
            options.chart_id = Some(generate_chart_id(&options));
        }

        // 创建新订阅
        let sub = Arc::new(SeriesSubscription::new(
            Arc::clone(&self.dm),
            Arc::clone(&self.ws),
            options,
        )?);

        Ok(sub)
    }
}

fn normalize_data_length(data_length: usize) -> Result<usize> {
    if data_length == 0 {
        return Err(TqError::InvalidParameter(
            "data_length 必须大于 0".to_string(),
        ));
    }
    Ok(data_length.min(10000))
}

fn normalize_kline_duration(duration: StdDuration) -> Result<i64> {
    if duration.is_zero() {
        return Err(TqError::InvalidParameter("duration 必须大于 0".to_string()));
    }
    if duration.subsec_nanos() != 0 {
        return Err(TqError::InvalidParameter("duration 必须为整秒".to_string()));
    }
    let secs = duration.as_secs();
    if secs == 0 {
        return Err(TqError::InvalidParameter("duration 必须大于 0".to_string()));
    }
    if secs > 86_400 && !secs.is_multiple_of(86_400) {
        return Err(TqError::InvalidParameter(
            "日线以上周期必须为 86400 的整数倍".to_string(),
        ));
    }
    let nanos = (secs as u128)
        .checked_mul(1_000_000_000u128)
        .ok_or_else(|| TqError::InvalidParameter("duration 过大".to_string()))?;
    i64::try_from(nanos).map_err(|_| TqError::InvalidParameter("duration 过大".to_string()))
}

fn build_realtime_kline_options(
    symbols: Vec<String>,
    duration: StdDuration,
    data_length: usize,
) -> Result<SeriesOptions> {
    if symbols.is_empty() {
        return Err(TqError::InvalidParameter("symbols 为空".to_string()));
    }
    if symbols.iter().any(|s| s.is_empty()) {
        return Err(TqError::InvalidParameter(
            "symbols 不能包含空字符串".to_string(),
        ));
    }
    let view_width = normalize_data_length(data_length)?;
    let duration = normalize_kline_duration(duration)?;
    let view_width = if symbols.len() > 1 {
        view_width.max(30)
    } else {
        view_width
    };
    Ok(SeriesOptions {
        symbols,
        duration,
        view_width,
        chart_id: None,
        left_kline_id: None,
        focus_datetime: None,
        focus_position: None,
    })
}

fn build_set_chart_request(options: &SeriesOptions, view_width: usize) -> serde_json::Value {
    let chart_id = options.chart_id.as_deref().unwrap_or("");
    let mut chart_req = serde_json::json!({
        "aid": "set_chart",
        "chart_id": chart_id,
        "ins_list": options.symbols.join(","),
        "duration": options.duration,
        "view_width": view_width,
    });

    if let Some(left_kline_id) = options.left_kline_id {
        chart_req["left_kline_id"] = serde_json::json!(left_kline_id);
    } else if let (Some(focus_datetime), Some(focus_position)) =
        (options.focus_datetime, options.focus_position)
    {
        chart_req["focus_datetime"] = serde_json::json!(focus_datetime.timestamp_nanos_opt());
        chart_req["focus_position"] = serde_json::json!(focus_position);
    }

    chart_req
}

/// 生成 chart_id
fn generate_chart_id(options: &SeriesOptions) -> String {
    let uid = Uuid::new_v4();
    if options.duration == 0 {
        format!("TQRS_tick_{}", uid)
    } else {
        format!("TQRS_kline_{}", uid)
    }
}

/// Series 订阅句柄。
///
/// 该类型封装单次订阅生命周期，支持：
/// - 启停与刷新
/// - 更新/新 Bar/错误回调注册
/// - 流式消费
/// - 主动关闭
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

    running: Arc<RwLock<bool>>,
    unsubscribe_sent: Arc<AtomicBool>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}

impl SeriesSubscription {
    /// 创建订阅
    fn new(
        dm: Arc<DataManager>,
        ws: Arc<TqQuoteWebsocket>,
        options: SeriesOptions,
    ) -> Result<Self> {
        let mut last_ids = HashMap::new();
        for symbol in &options.symbols {
            last_ids.insert(symbol.clone(), -1);
        }

        Ok(SeriesSubscription {
            dm,
            ws,
            options,
            last_ids: Arc::new(RwLock::new(last_ids)),
            last_left_id: Arc::new(RwLock::new(-1)),
            last_right_id: Arc::new(RwLock::new(-1)),
            chart_ready: Arc::new(RwLock::new(false)),
            has_chart_sync: Arc::new(RwLock::new(false)),
            on_update: Arc::new(RwLock::new(None)),
            on_new_bar: Arc::new(RwLock::new(None)),
            on_bar_update: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            running: Arc::new(RwLock::new(false)),
            unsubscribe_sent: Arc::new(AtomicBool::new(false)),
            data_cb_id: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// 发送 set_chart 请求
    async fn send_set_chart(&self) -> Result<()> {
        let mut view_width = self.options.view_width.min(10000);
        if view_width == 0 {
            view_width = 1;
        }
        let use_multi_init_view_width = self.options.duration != 0
            && self.options.symbols.len() > 1
            && self.options.left_kline_id.is_none()
            && self.options.focus_datetime.is_none()
            && self.options.focus_position.is_none();
        if use_multi_init_view_width {
            let ready = *self.chart_ready.read().await;
            if !ready {
                view_width = 10000;
            }
        }

        let chart_req = build_set_chart_request(&self.options, view_width);

        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        debug!(
            "发送 set_chart 请求: chart_id={}, symbols={:?}, view_width={}, duration={}",
            chart_id, self.options.symbols, view_width, self.options.duration
        );

        self.ws.send(&chart_req).await?;
        Ok(())
    }

    /// 在已存在的图表订阅上更新历史焦点位置。
    ///
    /// # 参数
    /// - `focus_time`: 焦点时间（UTC）
    /// - `focus_position`: 焦点位置，`1` 靠右，`-1` 靠左
    ///
    /// # 错误
    /// 当网络发送失败时返回错误。
    pub async fn update_focus(&self, focus_time: DateTime<Utc>, focus_position: i32) -> Result<()> {
        let view_width = if self.options.view_width > 10000 {
            10000
        } else {
            self.options.view_width
        };
        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        let chart_req = serde_json::json!({
            "aid": "set_chart",
            "chart_id": chart_id,
            "ins_list": self.options.symbols.join(","),
            "duration": self.options.duration,
            "view_width": view_width,
            "focus_datetime": focus_time.timestamp_nanos_opt(),
            "focus_position": focus_position
        });
        self.ws.send(&chart_req).await?;
        Ok(())
    }

    /// 启动订阅。
    ///
    /// 首次调用会注册数据监听并发送 `set_chart` 请求；重复调用是幂等的。
    ///
    /// # 错误
    /// 当发送订阅请求失败时返回错误。
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);
        self.unsubscribe_sent.store(false, Ordering::SeqCst);

        debug!(
            "启动 Series 订阅: {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );

        self.start_watching().await;
        if let Err(e) = self.send_set_chart().await {
            *self.running.write().await = false;
            self.detach_data_callback();
            return Err(e);
        }
        debug!(
            "send_set_chart done for {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );
        Ok(())
    }

    fn detach_data_callback(&self) {
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
    }

    /// 刷新订阅状态并重新发起 `set_chart` 请求。
    ///
    /// 该方法会重置内部增量状态（如 `last_id`、范围同步状态），
    /// 适合在需要强制重新对齐窗口时使用。
    ///
    /// # 错误
    /// 当发送刷新请求失败时返回错误。
    pub async fn refresh(&self) -> Result<()> {
        {
            let mut ids = self.last_ids.write().await;
            for value in ids.values_mut() {
                *value = -1;
            }
        }
        *self.last_left_id.write().await = -1;
        *self.last_right_id.write().await = -1;
        *self.chart_ready.write().await = false;
        *self.has_chart_sync.write().await = false;
        *self.running.write().await = true;
        self.unsubscribe_sent.store(false, Ordering::SeqCst);
        self.send_set_chart().await
    }

    fn build_cancel_chart_request(&self) -> serde_json::Value {
        let view_width = if self.options.view_width == 0 {
            1
        } else if self.options.view_width > 10000 {
            10000
        } else {
            self.options.view_width
        };
        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        serde_json::json!({
            "aid": "set_chart",
            "chart_id": chart_id,
            "ins_list": "",
            "duration": self.options.duration,
            "view_width": view_width
        })
    }

    /// 启动监听数据更新
    async fn start_watching(&self) {
        let dm_clone = Arc::clone(&self.dm);
        let ws = Arc::clone(&self.ws);
        let options = self.options.clone();
        let last_ids = Arc::clone(&self.last_ids);
        let last_left_id = Arc::clone(&self.last_left_id);
        let last_right_id = Arc::clone(&self.last_right_id);
        let chart_ready = Arc::clone(&self.chart_ready);
        let has_chart_sync = Arc::clone(&self.has_chart_sync);

        let on_update = Arc::clone(&self.on_update);
        let on_new_bar = Arc::clone(&self.on_new_bar);
        let on_bar_update = Arc::clone(&self.on_bar_update);
        let on_error = Arc::clone(&self.on_error);
        let running = Arc::clone(&self.running);
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_dirty = Arc::new(AtomicBool::new(false));
        let view_width_adjusted = Arc::new(AtomicBool::new(false));
        let last_processed_epoch = Arc::new(std::sync::Mutex::new(0i64));

        // 注册数据更新回调
        let dm_for_callback = Arc::clone(&dm_clone);
        let cb_id = dm_clone.on_data_register(move || {
            let chart_id = options.chart_id.as_deref().unwrap_or("");
            let duration_str = options.duration.to_string();

            let last_epoch = *last_processed_epoch.lock().unwrap();
            let chart_epoch = dm_for_callback.get_path_epoch(&["charts", chart_id]);
            let data_epoch = if options.duration == 0 {
                let symbol = &options.symbols[0];
                dm_for_callback.get_path_epoch(&["ticks", symbol])
            } else if options.symbols.len() > 1 {
                options
                    .symbols
                    .iter()
                    .map(|symbol| {
                        dm_for_callback.get_path_epoch(&["klines", symbol, &duration_str])
                    })
                    .max()
                    .unwrap_or(0)
            } else {
                let symbol = &options.symbols[0];
                dm_for_callback.get_path_epoch(&["klines", symbol, &duration_str])
            };

            if chart_epoch <= last_epoch && data_epoch <= last_epoch {
                return;
            }

            worker_dirty.store(true, Ordering::SeqCst);
            if worker_running.swap(true, Ordering::SeqCst) {
                return;
            }
            let dm = Arc::clone(&dm_for_callback);
            let options = options.clone();
            let last_ids = Arc::clone(&last_ids);
            let last_left_id = Arc::clone(&last_left_id);
            let last_right_id = Arc::clone(&last_right_id);
            let chart_ready = Arc::clone(&chart_ready);
            let has_chart_sync = Arc::clone(&has_chart_sync);
            let on_update = Arc::clone(&on_update);
            let on_new_bar = Arc::clone(&on_new_bar);
            let on_bar_update = Arc::clone(&on_bar_update);
            let on_error = Arc::clone(&on_error);
            let running = Arc::clone(&running);
            let worker_running = Arc::clone(&worker_running);
            let worker_dirty = Arc::clone(&worker_dirty);
            let ws = Arc::clone(&ws);
            let view_width_adjusted = Arc::clone(&view_width_adjusted);
            let last_processed_epoch = Arc::clone(&last_processed_epoch);

            tokio::spawn(async move {
                loop {
                    worker_dirty.store(false, Ordering::SeqCst);
                    let is_running = *running.read().await;
                    if is_running {
                        let chart_id = options.chart_id.as_deref().unwrap_or("");
                        let duration_str = options.duration.to_string();

                        let current_global_epoch = dm.get_epoch();
                        let last_epoch = *last_processed_epoch.lock().unwrap();

                        let chart_epoch = dm.get_path_epoch(&["charts", chart_id]);
                        let data_epoch = if options.duration == 0 {
                            let symbol = &options.symbols[0];
                            dm.get_path_epoch(&["ticks", symbol])
                        } else if options.symbols.len() > 1 {
                            options
                                .symbols
                                .iter()
                                .map(|symbol| dm.get_path_epoch(&["klines", symbol, &duration_str]))
                                .max()
                                .unwrap_or(0)
                        } else {
                            let symbol = &options.symbols[0];
                            dm.get_path_epoch(&["klines", symbol, &duration_str])
                        };

                        if chart_epoch > last_epoch || data_epoch > last_epoch {
                            if let Some(chart_data) = dm.get_by_path(&["charts", chart_id])
                                && let Ok(chart_info) =
                                    dm.convert_to_struct::<ChartInfo>(&chart_data)
                                && chart_info.ready
                                && !chart_info.more_data
                            {
                                match process_series_update(
                                    &dm,
                                    &options,
                                    &last_ids,
                                    &last_left_id,
                                    &last_right_id,
                                    &chart_ready,
                                    &has_chart_sync,
                                    last_epoch,
                                )
                                .await
                                {
                                    Ok((series_data, update_info)) => {
                                        if update_info.chart_ready {
                                            let series_data = Arc::new(series_data);
                                            let update_info = Arc::new(update_info);

                                            if let Some(callback) = on_update.read().await.as_ref()
                                            {
                                                callback(
                                                    Arc::clone(&series_data),
                                                    Arc::clone(&update_info),
                                                );
                                            }

                                            if update_info.has_new_bar
                                                && let Some(callback) =
                                                    on_new_bar.read().await.as_ref()
                                            {
                                                callback(Arc::clone(&series_data));
                                            }

                                            if update_info.has_bar_update
                                                && let Some(callback) =
                                                    on_bar_update.read().await.as_ref()
                                            {
                                                callback(Arc::clone(&series_data));
                                            }

                                            let use_multi_init_view_width = options.duration != 0
                                                && options.symbols.len() > 1
                                                && options.left_kline_id.is_none()
                                                && options.focus_datetime.is_none()
                                                && options.focus_position.is_none()
                                                && options.view_width < 10000;
                                            if use_multi_init_view_width
                                                && !view_width_adjusted.swap(true, Ordering::SeqCst)
                                            {
                                                let chart_req = build_set_chart_request(
                                                    &options,
                                                    options.view_width,
                                                );
                                                let _ = ws.send(&chart_req).await;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let err_str = e.to_string();
                                        if !err_str.contains("数据未更新") {
                                            warn!("处理 Series 更新失败: {}", e);
                                            if let Some(callback) = on_error.read().await.as_ref() {
                                                callback(Arc::new(err_str));
                                            }
                                        }
                                    }
                                }
                            }
                            *last_processed_epoch.lock().unwrap() = current_global_epoch;
                        }
                    }
                    if !worker_dirty.load(Ordering::SeqCst) {
                        worker_running.store(false, Ordering::SeqCst);
                        if worker_dirty.load(Ordering::SeqCst)
                            && !worker_running.swap(true, Ordering::SeqCst)
                        {
                            continue;
                        }
                        break;
                    }
                }
            });
        });
        *self.data_cb_id.lock().unwrap() = Some(cb_id);
    }

    /// 注册更新回调。
    ///
    /// 每次可消费的数据更新时触发，包含完整 `SeriesData` 和 `UpdateInfo`。
    /// 重复注册会覆盖旧回调。
    pub async fn on_update<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>, Arc<UpdateInfo>) + Send + Sync + 'static,
    {
        let mut guard = self.on_update.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册新 Bar 回调。
    ///
    /// 当检测到 `last_id` 前进时触发。Tick 模式下也会在新 Tick 到达时触发。
    /// 重复注册会覆盖旧回调。
    pub async fn on_new_bar<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>) + Send + Sync + 'static,
    {
        let mut guard = self.on_new_bar.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册 Bar 更新回调。
    ///
    /// 当最后一根 Bar 有更新或出现新 Bar 时触发。
    /// 重复注册会覆盖旧回调。
    pub async fn on_bar_update<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>) + Send + Sync + 'static,
    {
        let mut guard = self.on_bar_update.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册错误回调。
    ///
    /// 当更新处理过程中出现可上报错误时触发。
    /// 重复注册会覆盖旧回调。
    pub async fn on_error<F>(&self, handler: F)
    where
        F: Fn(Arc<String>) + Send + Sync + 'static,
    {
        let mut guard = self.on_error.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 获取异步数据流。
    ///
    /// 该方法内部通过 `on_update` 回调桥接为 `Stream`，适合与 `while let` 或
    /// `tokio_stream` 生态配合使用。
    ///
    /// 注意：调用该方法会覆盖此前注册的 `on_update` 处理器。
    pub async fn data_stream(&self) -> impl Stream<Item = Arc<SeriesData>> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        // 注册回调
        self.on_update(move |data, _info| {
            let _ = tx.send(data);
        })
        .await;

        stream! {
            while let Some(data) = rx.recv().await {
                yield data;
            }
        }
    }

    /// 关闭订阅并向服务端发送取消请求。
    ///
    /// 该操作幂等，多次调用仅第一次会实际发送取消请求。
    ///
    /// # 错误
    /// 当取消请求发送失败时返回错误。
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        self.detach_data_callback();

        info!(
            "关闭 Series 订阅: {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );

        if self.unsubscribe_sent.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let cancel_req = self.build_cancel_chart_request();
        self.ws.send(&cancel_req).await?;
        Ok(())
    }
}

impl Drop for SeriesSubscription {
    fn drop(&mut self) {
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
        if self.unsubscribe_sent.swap(true, Ordering::SeqCst) {
            return;
        }
        let chart_id = self.options.chart_id.clone().unwrap_or_default();
        info!(
            "销毁 Series 订阅: chart_id={}, symbols={:?}, duration={}",
            chart_id, self.options.symbols, self.options.duration
        );
        let ws = Arc::clone(&self.ws);
        let cancel_req = self.build_cancel_chart_request();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = ws.send(&cancel_req).await;
            });
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(async move {
                        let _ = ws.send(&cancel_req).await;
                    });
                }
            });
        }
    }
}

#[cfg(test)]
mod api_naming_tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    #[test]
    fn series_api_should_expose_kline_and_tick_methods() {
        fn _kline<'a>(
            api: &'a SeriesAPI,
            symbols: &'a str,
            duration: StdDuration,
            data_length: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
            Box::pin(api.kline(symbols, duration, data_length))
        }

        fn _tick<'a>(
            api: &'a SeriesAPI,
            symbol: &'a str,
            data_length: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
            Box::pin(api.tick(symbol, data_length))
        }

        let _ = (_kline, _tick);
    }
}

/// 处理 Series 更新
#[expect(
    clippy::too_many_arguments,
    reason = "序列更新计算需要共享多份增量状态"
)]
async fn process_series_update(
    dm: &DataManager,
    options: &SeriesOptions,
    last_ids: &Arc<RwLock<HashMap<String, i64>>>,
    last_left_id: &Arc<RwLock<i64>>,
    last_right_id: &Arc<RwLock<i64>>,
    chart_ready: &Arc<RwLock<bool>>,
    has_chart_sync: &Arc<RwLock<bool>>,
    last_epoch: i64,
) -> Result<(SeriesData, UpdateInfo)> {
    let is_multi = options.symbols.len() > 1;
    let is_tick = options.duration == 0;

    // 获取数据
    let series_data: SeriesData = if is_tick {
        get_tick_data(dm, options).await?
    } else if is_multi {
        get_multi_kline_data(dm, options).await?
    } else {
        get_single_kline_data(dm, options).await?
    };

    // 检测更新信息
    let mut update_info = UpdateInfo {
        has_new_bar: false,
        has_bar_update: false,
        chart_range_changed: false,
        has_chart_sync: false,
        chart_ready: false,
        new_bar_ids: HashMap::new(),
        old_left_id: 0,
        old_right_id: 0,
        new_left_id: 0,
        new_right_id: 0,
    };
    // 检测新 K线
    detect_new_bars(dm, &series_data, last_ids, &mut update_info, last_epoch).await;

    // 检测 Chart 范围变化
    detect_chart_range_change(
        dm,
        &series_data,
        last_left_id,
        last_right_id,
        chart_ready,
        has_chart_sync,
        &mut update_info,
    )
    .await;

    Ok((series_data, update_info))
}

/// 获取单合约 K线数据
async fn get_single_kline_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let symbol = &options.symbols[0];
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;

    // 获取 Chart 信息 - 直接从 JSON 转换为 ChartInfo
    let mut right_id = -1i64;
    let chart_info = dm
        .get_by_path(&["charts", chart_id])
        .and_then(|chart_data| dm.convert_to_struct::<ChartInfo>(&chart_data).ok())
        .map(|mut chart| {
            right_id = chart.right_id;
            chart.chart_id = chart_id.to_string();
            chart.view_width = options.view_width;
            chart
        });
    let mut kline_data =
        dm.get_klines_data(symbol, options.duration, options.view_width, right_id)?;

    // 设置 Chart 信息
    kline_data.chart_id = chart_id.to_string();
    kline_data.chart = chart_info;

    Ok(SeriesData {
        is_multi: false,
        is_tick: false,
        symbols: vec![symbol.clone()],
        single: Some(kline_data),
        multi: None,
        tick_data: None,
    })
}

/// 获取多合约 K线数据
async fn get_multi_kline_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;
    let multi_data = dm.get_multi_klines_data(
        &options.symbols,
        options.duration,
        chart_id,
        options.view_width,
    )?;

    Ok(SeriesData {
        is_multi: true,
        is_tick: false,
        symbols: options.symbols.clone(),
        single: None,
        multi: Some(multi_data),
        tick_data: None,
    })
}

/// 获取 Tick 数据
async fn get_tick_data(dm: &DataManager, options: &SeriesOptions) -> Result<SeriesData> {
    let symbol = &options.symbols[0];
    let chart_id = options
        .chart_id
        .as_deref()
        .ok_or_else(|| TqError::InternalError("SeriesOptions.chart_id 为空".to_string()))?;

    // 获取 Chart 信息 - 直接从 JSON 转换为 ChartInfo
    let mut right_id = -1i64;
    let chart_info = dm
        .get_by_path(&["charts", chart_id])
        .and_then(|chart_data| dm.convert_to_struct::<ChartInfo>(&chart_data).ok())
        .map(|mut chart| {
            right_id = chart.right_id;
            chart.chart_id = chart_id.to_string();
            chart.view_width = options.view_width;
            chart
        });

    let mut tick_data = dm.get_ticks_data(symbol, options.view_width, right_id)?;

    // 设置 Chart 信息
    tick_data.chart_id = chart_id.to_string();
    tick_data.chart = chart_info;

    Ok(SeriesData {
        is_multi: false,
        is_tick: true,
        symbols: vec![symbol.clone()],
        single: None,
        multi: None,
        tick_data: Some(tick_data),
    })
}

/// 检测新 K线
async fn detect_new_bars(
    dm: &DataManager,
    data: &SeriesData,
    last_ids: &Arc<RwLock<HashMap<String, i64>>>,
    info: &mut UpdateInfo,
    last_epoch: i64,
) {
    let mut ids = last_ids.write().await;

    // 获取 duration 字符串（用于后续检查 K线更新）
    let duration_str = if data.is_tick {
        String::new()
    } else if data.is_multi {
        data.multi
            .as_ref()
            .map(|m| m.duration.to_string())
            .unwrap_or_default()
    } else {
        data.single
            .as_ref()
            .map(|s| s.duration.to_string())
            .unwrap_or_default()
    };

    for symbol in &data.symbols {
        let current_id = if data.is_tick {
            data.tick_data.as_ref().map(|t| t.last_id).unwrap_or(-1)
        } else if data.is_multi {
            data.multi
                .as_ref()
                .and_then(|m| m.metadata.get(symbol))
                .map(|meta| meta.last_id)
                .unwrap_or(-1)
        } else {
            data.single.as_ref().map(|s| s.last_id).unwrap_or(-1)
        };
        let last_id = ids.get(symbol).copied().unwrap_or(-1);
        trace!("current_id = {}, last_id = {}", current_id, last_id);
        if current_id > last_id {
            info.has_new_bar = true;
            info.has_bar_update = true;
            info.new_bar_ids.insert(symbol.clone(), current_id);
        }
        ids.insert(symbol.clone(), current_id);
    }
    if !info.has_new_bar && !data.is_tick {
        for symbol in &data.symbols {
            if dm.get_path_epoch(&["klines", symbol, &duration_str]) > last_epoch {
                info.has_bar_update = true;
                break;
            }
        }
    }
}

/// 检测 Chart 范围变化
async fn detect_chart_range_change(
    dm: &DataManager,
    data: &SeriesData,
    last_left_id: &Arc<RwLock<i64>>,
    last_right_id: &Arc<RwLock<i64>>,
    chart_ready: &Arc<RwLock<bool>>,
    has_chart_sync: &Arc<RwLock<bool>>,
    info: &mut UpdateInfo,
) {
    let chart: Option<&ChartInfo> = if let Some(single) = &data.single {
        single.chart.as_ref()
    } else if let Some(tick) = &data.tick_data {
        tick.chart.as_ref()
    } else {
        None
    };

    let multi_chart = if let Some(multi) = &data.multi {
        if let Some(chart_data) = dm.get_by_path(&["charts", &multi.chart_id]) {
            dm.convert_to_struct::<ChartInfo>(&chart_data)
                .ok()
                .map(|mut c| {
                    c.chart_id = multi.chart_id.clone();
                    c.view_width = multi.view_width;
                    c
                })
        } else {
            None
        }
    } else {
        None
    };

    if let Some(chart) = chart.or(multi_chart.as_ref()) {
        let mut last_left = last_left_id.write().await;
        let mut last_right = last_right_id.write().await;
        let mut ready = chart_ready.write().await;
        let mut has_sync = has_chart_sync.write().await;

        trace!(
            "before compare -> last_left: {}, last_right: {}, chart.left_id: {}, chart.right_id: {}, ready: {}, chart.ready: {}, has_sync: {}",
            *last_left, *last_right, chart.left_id, chart.right_id, *ready, chart.ready, *has_sync,
        );
        if chart.left_id != *last_left || chart.right_id != *last_right {
            info.chart_range_changed = true;
            info.old_left_id = *last_left;
            info.old_right_id = *last_right;
            info.new_left_id = chart.left_id;
            info.new_right_id = chart.right_id;

            *last_left = chart.left_id;
            *last_right = chart.right_id;
        }

        if chart.ready && !*ready {
            // 首次完成同步
            *ready = true;
            *has_sync = true;

            info.has_chart_sync = true;
            info.has_bar_update = true;
            info.has_new_bar = true;
        }

        if chart.ready && !chart.more_data {
            info.chart_ready = true;
        }
        trace!(
            "after compare -> last_left: {}, last_right: {}, chart.left_id: {}, chart.right_id: {}, ready: {}, chart.ready: {}, has_sync: {}",
            *last_left, *last_right, chart.left_id, chart.right_id, *ready, chart.ready, *has_sync,
        );
        info.has_chart_sync = *has_sync
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamanager::DataManagerConfig;
    use crate::websocket::WebSocketConfig;
    use async_trait::async_trait;
    use reqwest::header::HeaderMap;

    #[derive(Default)]
    struct TestAuth;

    #[async_trait]
    impl Authenticator for TestAuth {
        fn base_header(&self) -> HeaderMap {
            HeaderMap::new()
        }

        async fn login(&mut self) -> Result<()> {
            Ok(())
        }

        async fn get_td_url(
            &self,
            _broker_id: &str,
            _account_id: &str,
        ) -> Result<crate::auth::BrokerInfo> {
            Err(TqError::NotLoggedIn)
        }

        async fn get_md_url(&self, _stock: bool, _backtest: bool) -> Result<String> {
            Ok("wss://example.com".to_string())
        }

        fn has_feature(&self, _feature: &str) -> bool {
            false
        }

        fn has_md_grants(&self, _symbols: &[&str]) -> Result<()> {
            Ok(())
        }

        fn has_td_grants(&self, _symbol: &str) -> Result<()> {
            Ok(())
        }

        fn get_auth_id(&self) -> &str {
            ""
        }

        fn get_access_token(&self) -> &str {
            ""
        }
    }

    #[tokio::test]
    async fn kline_returns_unstarted_subscription() {
        let dm = Arc::new(DataManager::new(
            HashMap::new(),
            DataManagerConfig::default(),
        ));
        let ws = Arc::new(TqQuoteWebsocket::new(
            "wss://example.com".to_string(),
            Arc::clone(&dm),
            WebSocketConfig::default(),
        ));
        let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
        let api = SeriesAPI::new(dm, ws, auth);

        let sub = api
            .kline("SHFE.au2602", StdDuration::from_secs(60), 32)
            .await
            .unwrap();

        assert!(!*sub.running.read().await);
        assert!(sub.data_cb_id.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn start_failure_rolls_back_series_callback_registration() {
        let dm = Arc::new(DataManager::new(
            HashMap::new(),
            DataManagerConfig::default(),
        ));
        let ws = Arc::new(TqQuoteWebsocket::new(
            "wss://example.com".to_string(),
            Arc::clone(&dm),
            WebSocketConfig::default(),
        ));
        ws.force_send_failure_for_test();

        let sub = SeriesSubscription::new(
            dm,
            ws,
            SeriesOptions {
                symbols: vec!["SHFE.au2602".to_string()],
                duration: 60_000_000_000,
                view_width: 32,
                chart_id: Some("chart_test".to_string()),
                left_kline_id: None,
                focus_datetime: None,
                focus_position: None,
            },
        )
        .unwrap();

        assert!(sub.start().await.is_err());
        assert!(!*sub.running.read().await);
        assert!(sub.data_cb_id.lock().unwrap().is_none());
    }
}
