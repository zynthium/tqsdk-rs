use super::{KlineSymbols, SeriesAPI, SeriesSubscription};
use crate::cache::kline::DiskKlineCache;
use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, SeriesOptions, rangeset_difference, rangeset_intersection, rangeset_union};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use uuid::Uuid;

#[cfg(not(test))]
const HISTORY_CHUNK_FETCH_TIMEOUT: StdDuration = StdDuration::from_secs(30);
#[cfg(test)]
const HISTORY_CHUNK_FETCH_TIMEOUT: StdDuration = StdDuration::from_millis(500);

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

impl SeriesAPI {
    /// 创建 Series API 实例。
    pub fn new(
        dm: std::sync::Arc<crate::datamanager::DataManager>,
        ws: std::sync::Arc<crate::websocket::TqQuoteWebsocket>,
        auth: std::sync::Arc<tokio::sync::RwLock<dyn crate::auth::Authenticator>>,
    ) -> Self {
        Self {
            dm,
            ws,
            auth,
            kline_cache: Arc::new(RwLock::new(HashMap::new())),
            disk_cache: Arc::new(DiskKlineCache::new(None)),
        }
    }

    /// 获取 K 线序列订阅（单合约/多合约对齐），对齐 tqsdk-python 的 `get_kline_serial`。
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
    pub async fn tick(&self, symbol: &str, data_length: usize) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
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
    pub async fn kline_history(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        left_kline_id: i64,
    ) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
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
    pub async fn kline_history_with_focus(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        focus_time: DateTime<Utc>,
        focus_position: i32,
    ) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
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

    /// 获取固定窗口的历史 K 线快照（不随行情更新）。
    ///
    /// 与 `kline_history` 的区别是：
    /// - 本接口返回一次性 `Vec<Kline>`，而不是订阅句柄。
    /// - 优先复用磁盘缓存，仅下载缺失区间。
    pub async fn kline_data_series_by_id(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        left_kline_id: i64,
    ) -> Result<Vec<Kline>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
        }
        if left_kline_id < 0 {
            return Err(TqError::InvalidParameter("left_kline_id 必须 >= 0".to_string()));
        }
        let view_width = normalize_data_length(data_length)?;
        let duration_nano = normalize_kline_duration(duration)?;
        let requested_end = left_kline_id
            .checked_add(view_width as i64)
            .ok_or_else(|| TqError::InvalidParameter("请求区间过大".to_string()))?;
        let requested_range = Range::new(left_kline_id, requested_end);

        let mut cached_ranges = match self.disk_cache.read_metadata(symbol, duration_nano) {
            Ok(Some(metadata)) => metadata.ranges,
            Ok(None) => Vec::new(),
            Err(e) => {
                warn!(
                    "读取磁盘缓存元数据失败，将回退到在线下载: symbol={}, duration={}, error={}",
                    symbol, duration_nano, e
                );
                Vec::new()
            }
        };

        let missing_ranges = rangeset_difference(&vec![requested_range.clone()], &cached_ranges);
        for missing in missing_ranges {
            if missing.is_empty() {
                continue;
            }
            let missing_width = usize::try_from(missing.len())
                .map_err(|_| TqError::InvalidParameter("请求区间长度非法".to_string()))?;
            let fetched = self
                .fetch_history_chunk_by_id(symbol, duration, missing.start, missing_width)
                .await?;
            if fetched.is_empty() {
                continue;
            }

            append_klines_to_disk(
                Arc::clone(&self.disk_cache),
                symbol.to_string(),
                duration_nano,
                fetched.clone(),
            )
            .await?;
            if let Some(added_range) = range_from_klines(&fetched) {
                cached_ranges = rangeset_union(&cached_ranges, &vec![added_range]);
            }
        }

        // 最终只返回请求窗口内的数据，按 id 去重并排序，避免磁盘中历史重复片段影响结果。
        let intersected_ranges = rangeset_intersection(&vec![requested_range], &cached_ranges);
        let mut result = Vec::new();
        for range in intersected_ranges {
            match self.disk_cache.read_klines(symbol, duration_nano, &range) {
                Ok(mut klines) => result.append(&mut klines),
                Err(e) => {
                    warn!(
                        "读取磁盘缓存区间失败: symbol={}, duration={}, range={:?}, error={}",
                        symbol, duration_nano, range, e
                    );
                }
            }
        }
        Ok(dedup_sort_klines_by_id(result))
    }

    /// 获取指定时间窗口的历史 K 线快照（不随行情更新）。
    ///
    /// 语义为 `[start_dt, end_dt)`，即包含 `start_dt`，不包含 `end_dt`。
    /// 缓存命中时直接从磁盘读取；缓存缺失时按时间焦点增量下载并回填缓存。
    pub async fn kline_data_series(
        &self,
        symbol: &str,
        duration: StdDuration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
        }
        let duration_nano = normalize_kline_duration(duration)?;
        let start_nano = start_dt
            .timestamp_nanos_opt()
            .ok_or_else(|| TqError::InvalidParameter("start_dt 超出可表示范围".to_string()))?;
        let end_nano = end_dt
            .timestamp_nanos_opt()
            .ok_or_else(|| TqError::InvalidParameter("end_dt 超出可表示范围".to_string()))?;
        if end_nano <= start_nano {
            return Err(TqError::InvalidParameter("end_dt 必须晚于 start_dt".to_string()));
        }
        let requested_dt_range = Range::new(start_nano, end_nano);

        let mut cached_id_ranges = match self.disk_cache.read_metadata(symbol, duration_nano) {
            Ok(Some(metadata)) => metadata.ranges,
            Ok(None) => Vec::new(),
            Err(e) => {
                warn!(
                    "读取磁盘缓存元数据失败，将回退到在线下载: symbol={}, duration={}, error={}",
                    symbol, duration_nano, e
                );
                Vec::new()
            }
        };
        let mut cached_dt_ranges =
            self.derive_datetime_ranges_from_cached_ids(symbol, duration_nano, &cached_id_ranges)?;
        trim_last_bar_from_cached_datetime_ranges(&mut cached_dt_ranges, duration_nano);

        let missing_dt_ranges = rangeset_difference(&vec![requested_dt_range.clone()], &cached_dt_ranges);
        for missing in missing_dt_ranges {
            if missing.is_empty() {
                continue;
            }

            let estimated = estimate_view_width_from_datetime_range(&missing, duration_nano)?;
            let focus = DateTime::<Utc>::from_timestamp_nanos(missing.start);
            let fetched = self
                .fetch_history_chunk_by_focus(symbol, duration, estimated, focus, 0)
                .await?;
            let filtered: Vec<Kline> = fetched
                .into_iter()
                .filter(|k| missing.start <= k.datetime && k.datetime < missing.end)
                .collect();
            if filtered.is_empty() {
                continue;
            }

            append_klines_to_disk(
                Arc::clone(&self.disk_cache),
                symbol.to_string(),
                duration_nano,
                filtered.clone(),
            )
            .await?;

            if let Some(id_range) = range_from_klines(&filtered) {
                cached_id_ranges = rangeset_union(&cached_id_ranges, &vec![id_range]);
            }
            if let Some(dt_range) = datetime_range_from_klines(&filtered, duration_nano) {
                cached_dt_ranges = rangeset_union(&cached_dt_ranges, &vec![dt_range]);
            }
        }

        let mut result = Vec::new();
        let scan_id_ranges = if cached_id_ranges.is_empty() {
            Vec::new()
        } else {
            cached_id_ranges.clone()
        };
        for range in scan_id_ranges {
            match self.disk_cache.read_klines(symbol, duration_nano, &range) {
                Ok(klines) => {
                    result.extend(
                        klines
                            .into_iter()
                            .filter(|k| start_nano <= k.datetime && k.datetime < end_nano),
                    );
                }
                Err(e) => warn!(
                    "按时间窗口读取磁盘缓存失败: symbol={}, duration={}, range={:?}, error={}",
                    symbol, duration_nano, range, e
                ),
            }
        }
        Ok(dedup_sort_klines_by_id(result))
    }

    async fn fetch_history_chunk_by_id(
        &self,
        symbol: &str,
        duration: StdDuration,
        left_kline_id: i64,
        data_length: usize,
    ) -> Result<Vec<Kline>> {
        let sub = self.kline_history(symbol, duration, data_length, left_kline_id).await?;

        let (tx, rx) = oneshot::channel::<Vec<Kline>>();
        let sender = Arc::new(Mutex::new(Some(tx)));
        let sender_for_cb = Arc::clone(&sender);
        sub.on_update(move |series_data, update_info| {
            if !update_info.chart_ready {
                return;
            }
            let Some(single) = &series_data.single else {
                return;
            };
            if let Some(tx) = sender_for_cb.lock().ok().and_then(|mut guard| guard.take()) {
                let _ = tx.send(single.data.clone());
            }
        })
        .await;

        sub.start().await?;
        let fetched = match tokio::time::timeout(HISTORY_CHUNK_FETCH_TIMEOUT, rx).await {
            Ok(Ok(klines)) => Ok(klines),
            Ok(Err(_)) => Err(TqError::InternalError("历史下载通道提前关闭".to_string())),
            Err(_) => Err(TqError::Timeout),
        };
        let close_res = sub.close().await;
        if let Err(e) = close_res {
            warn!("历史下载临时订阅关闭失败: {:?}", e);
        }
        fetched
    }

    async fn fetch_history_chunk_by_focus(
        &self,
        symbol: &str,
        duration: StdDuration,
        data_length: usize,
        focus_time: DateTime<Utc>,
        focus_position: i32,
    ) -> Result<Vec<Kline>> {
        let sub = self
            .kline_history_with_focus(symbol, duration, data_length, focus_time, focus_position)
            .await?;
        self.fetch_history_chunk_with_subscription(sub).await
    }

    async fn fetch_history_chunk_with_subscription(&self, sub: Arc<SeriesSubscription>) -> Result<Vec<Kline>> {
        let (tx, rx) = oneshot::channel::<Vec<Kline>>();
        let sender = Arc::new(Mutex::new(Some(tx)));
        let sender_for_cb = Arc::clone(&sender);
        sub.on_update(move |series_data, update_info| {
            if !update_info.chart_ready {
                return;
            }
            let Some(single) = &series_data.single else {
                return;
            };
            if let Some(tx) = sender_for_cb.lock().ok().and_then(|mut guard| guard.take()) {
                let _ = tx.send(single.data.clone());
            }
        })
        .await;

        sub.start().await?;
        let fetched = match tokio::time::timeout(HISTORY_CHUNK_FETCH_TIMEOUT, rx).await {
            Ok(Ok(klines)) => Ok(klines),
            Ok(Err(_)) => Err(TqError::InternalError("历史下载通道提前关闭".to_string())),
            Err(_) => Err(TqError::Timeout),
        };
        let close_res = sub.close().await;
        if let Err(e) = close_res {
            warn!("历史下载临时订阅关闭失败: {:?}", e);
        }
        fetched
    }

    fn derive_datetime_ranges_from_cached_ids(
        &self,
        symbol: &str,
        duration_nano: i64,
        id_ranges: &[Range],
    ) -> Result<Vec<Range>> {
        let mut ranges = Vec::new();
        for range in id_ranges {
            let klines = self.disk_cache.read_klines(symbol, duration_nano, range)?;
            if let Some(dt_range) = datetime_range_from_klines(&klines, duration_nano) {
                ranges.push(dt_range);
            }
        }
        Ok(rangeset_union(&Vec::new(), &ranges))
    }

    /// 通用订阅方法
    async fn subscribe(&self, mut options: SeriesOptions) -> Result<Arc<SeriesSubscription>> {
        if options.symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbols 为空".to_string()));
        }
        if options.symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter("symbols 不能包含空字符串".to_string()));
        }
        if options.view_width == 0 {
            return Err(TqError::InvalidParameter("data_length 必须大于 0".to_string()));
        }
        if options.view_width > 10000 {
            options.view_width = 10000;
        }
        {
            let auth = self.auth.read().await;
            let symbol_refs: Vec<&str> = options.symbols.iter().map(|s| s.as_str()).collect();
            auth.has_md_grants(&symbol_refs)?;
        }

        let has_chart_id = options.chart_id.as_ref().is_some_and(|chart_id| !chart_id.is_empty());
        if !has_chart_id {
            options.chart_id = Some(generate_chart_id(&options));
        }

        let sub = Arc::new(SeriesSubscription::new(
            Arc::clone(&self.dm),
            Arc::clone(&self.ws),
            options.clone(),
            Arc::clone(&self.kline_cache),
            Arc::clone(&self.disk_cache),
        )?);

        // 历史窗口：仅进行本地缓存预热，不发网络请求，保持显式 start() 语义。
        if options.left_kline_id.is_some() {
            let symbol = options.symbols[0].clone();
            let duration = options.duration;

            let mut disk_cached_ranges = Vec::new();
            match self.disk_cache.read_metadata(&symbol, duration) {
                Ok(Some(metadata)) => {
                    disk_cached_ranges = metadata.ranges;
                }
                Ok(None) => {}
                Err(e) => warn!(
                    "读取磁盘缓存元数据失败: symbol={}, duration={}, error={}",
                    symbol, duration, e
                ),
            }

            if !disk_cached_ranges.is_empty() {
                let mut cache = self.kline_cache.write().await;
                let cached_ranges_entry = cache.entry((symbol.clone(), duration)).or_insert_with(Vec::new);
                *cached_ranges_entry = rangeset_union(cached_ranges_entry, &disk_cached_ranges);
                debug!("预热缓存范围: key=({symbol}, {duration}), ranges={cached_ranges_entry:?}");
            }
        }

        Ok(sub)
    }
}

fn normalize_data_length(data_length: usize) -> Result<usize> {
    if data_length == 0 {
        return Err(TqError::InvalidParameter("data_length 必须大于 0".to_string()));
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
        return Err(TqError::InvalidParameter("symbols 不能包含空字符串".to_string()));
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

/// 生成 chart_id
fn generate_chart_id(options: &SeriesOptions) -> String {
    let uid = Uuid::new_v4();
    if options.duration == 0 {
        format!("TQRS_tick_{}", uid)
    } else {
        format!("TQRS_kline_{}", uid)
    }
}

fn range_from_klines(klines: &[Kline]) -> Option<Range> {
    let min_id = klines.iter().map(|k| k.id).min()?;
    let max_id = klines.iter().map(|k| k.id).max()?;
    max_id.checked_add(1).map(|end| Range::new(min_id, end))
}

fn dedup_sort_klines_by_id(klines: Vec<Kline>) -> Vec<Kline> {
    let mut by_id = BTreeMap::new();
    for k in klines {
        by_id.insert(k.id, k);
    }
    by_id.into_values().collect()
}

fn datetime_range_from_klines(klines: &[Kline], duration_nano: i64) -> Option<Range> {
    let first_dt = klines.iter().map(|k| k.datetime).min()?;
    let last_dt = klines.iter().map(|k| k.datetime).max()?;
    let end = last_dt.checked_add(duration_nano)?;
    Some(Range::new(first_dt, end))
}

fn trim_last_bar_from_cached_datetime_ranges(ranges: &mut Vec<Range>, duration_nano: i64) {
    let Some(last) = ranges.last_mut() else {
        return;
    };
    let new_end = last.end.saturating_sub(duration_nano);
    last.end = new_end.max(last.start);
    if last.is_empty() {
        let _ = ranges.pop();
    }
}

fn estimate_view_width_from_datetime_range(range: &Range, duration_nano: i64) -> Result<usize> {
    let len = i128::from(range.end) - i128::from(range.start);
    if len <= 0 {
        return Ok(1);
    }
    let dur = i128::from(duration_nano.max(1));
    let mut bars = (len + dur - 1) / dur;
    bars += 2; // 容纳边界 bar 与服务端对齐误差
    if bars > 10_000 {
        return Err(TqError::InvalidParameter(
            "时间窗口过大，预计超过 10000 根 K 线，请缩小区间".to_string(),
        ));
    }
    usize::try_from(bars).map_err(|_| TqError::InvalidParameter("时间窗口估算失败".to_string()))
}

async fn append_klines_to_disk(
    disk_cache: Arc<DiskKlineCache>,
    symbol: String,
    duration: i64,
    klines: Vec<Kline>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || disk_cache.append_klines(&symbol, duration, &klines))
        .await
        .map_err(|e| TqError::Other(format!("历史磁盘缓存写入任务失败: {e}")))?
}
