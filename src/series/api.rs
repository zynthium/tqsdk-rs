use super::{KlineSymbols, SeriesAPI, SeriesSubscription};
use crate::cache::kline::DiskKlineCache;
use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, SeriesOptions, rangeset_intersection, rangeset_union};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use uuid::Uuid;

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
        if let Some(left_kline_id) = options.left_kline_id {
            let symbol = options.symbols[0].clone();
            let duration = options.duration;
            let view_width = options.view_width as i64;
            let requested_range = Range::new(left_kline_id, left_kline_id + view_width);

            let mut disk_cached_ranges = Vec::new();
            let mut disk_klines = Vec::new();
            match self.disk_cache.read_metadata(&symbol, duration) {
                Ok(Some(metadata)) => {
                    disk_cached_ranges = metadata.ranges;
                    let ranges_to_read = rangeset_intersection(&vec![requested_range.clone()], &disk_cached_ranges);
                    for range in ranges_to_read {
                        match self.disk_cache.read_klines(&symbol, duration, &range) {
                            Ok(klines) => disk_klines.extend(klines),
                            Err(e) => warn!(
                                "读取磁盘缓存失败: symbol={}, duration={}, range={:?}, error={}",
                                symbol, duration, range, e
                            ),
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => warn!(
                    "读取磁盘缓存元数据失败: symbol={}, duration={}, error={}",
                    symbol, duration, e
                ),
            }

            if !disk_klines.is_empty()
                && let Some(payload) = merge_cached_klines_into_datamanager_payload(&symbol, duration, &disk_klines)
            {
                self.dm.merge_data(payload, true, false);
                debug!(
                    "已从磁盘缓存注入 K线到 DataManager: symbol={}, duration={}, count={}",
                    symbol,
                    duration,
                    disk_klines.len()
                );
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

fn merge_cached_klines_into_datamanager_payload(symbol: &str, duration: i64, klines: &[Kline]) -> Option<Value> {
    let mut by_id = std::collections::BTreeMap::<i64, Kline>::new();
    for kline in klines {
        by_id.insert(kline.id, kline.clone());
    }

    let mut kline_data_map = Map::new();
    let mut max_id: Option<i64> = None;
    for (id, kline) in by_id {
        let Ok(mut value) = serde_json::to_value(kline) else {
            continue;
        };
        if let Value::Object(ref mut obj) = value {
            obj.remove("id");
        }
        kline_data_map.insert(id.to_string(), value);
        max_id = Some(max_id.map_or(id, |current| current.max(id)));
    }

    let max_id = max_id?;
    if kline_data_map.is_empty() {
        return None;
    }

    let duration_key = duration.to_string();
    Some(json!({
        "klines": {
            symbol: {
                duration_key: {
                    "last_id": max_id + 1,
                    "data": Value::Object(kline_data_map)
                }
            }
        }
    }))
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
