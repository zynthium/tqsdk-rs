use super::{KlineSymbols, SeriesAPI, SeriesCachePolicy, SeriesSubscription};
use crate::cache::{DataSeriesCache, PAGE_VIEW_WIDTH, trim_last_datetime_range};
use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, SeriesOptions, Tick, rangeset_difference};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{debug, warn};
use uuid::Uuid;

#[cfg(not(test))]
const HISTORY_CHUNK_FETCH_TIMEOUT: StdDuration = StdDuration::from_secs(30);
#[cfg(test)]
const HISTORY_CHUNK_FETCH_TIMEOUT: StdDuration = StdDuration::from_millis(500);
const HISTORY_MANUAL_PEEK_RETRY_INTERVAL: StdDuration = StdDuration::from_millis(500);

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
        dm: Arc<crate::datamanager::DataManager>,
        ws: Arc<crate::websocket::TqQuoteWebsocket>,
        auth: Arc<RwLock<dyn crate::auth::Authenticator>>,
    ) -> Self {
        Self::new_with_cache_policy(dm, ws, auth, SeriesCachePolicy::default())
    }

    /// 创建 Series API 实例（可自定义磁盘缓存策略）。
    pub fn new_with_cache_policy(
        dm: Arc<crate::datamanager::DataManager>,
        ws: Arc<crate::websocket::TqQuoteWebsocket>,
        auth: Arc<RwLock<dyn crate::auth::Authenticator>>,
        cache_policy: SeriesCachePolicy,
    ) -> Self {
        Self {
            dm,
            ws,
            auth,
            data_series_cache: Arc::new(DataSeriesCache::new(None)),
            cache_policy,
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

    /// 获取指定时间窗口的历史 K 线快照（不随行情更新）。
    ///
    /// 语义为 `[start_dt, end_dt)`，缓存行为与 Python 官方 `DataSeries` 对齐。
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

        if !self.cache_policy.enabled {
            return self.download_kline_range(symbol, duration, start_nano, end_nano).await;
        }

        let result = {
            let _lock = self.data_series_cache.lock_series(symbol, duration_nano)?;
            let requested_range = Range::new(start_nano, end_nano);
            let cached_id_ranges = self.data_series_cache.get_rangeset_id(symbol, duration_nano)?;
            let mut cached_dt_ranges =
                self.data_series_cache
                    .get_rangeset_dt(symbol, duration_nano, &cached_id_ranges)?;
            trim_last_datetime_range(&mut cached_dt_ranges, duration_nano);

            let missing_dt_ranges = rangeset_difference(&vec![requested_range], &cached_dt_ranges);
            for missing in missing_dt_ranges {
                if missing.is_empty() {
                    continue;
                }
                let rows = self
                    .download_kline_range(symbol, duration, missing.start, missing.end)
                    .await?;
                if rows.is_empty() {
                    continue;
                }
                self.data_series_cache
                    .write_kline_segment(symbol, duration_nano, &rows)?;
            }

            self.data_series_cache.merge_adjacent_files(symbol, duration_nano)?;
            self.data_series_cache
                .read_kline_window(symbol, duration_nano, start_nano, end_nano)?
        };
        self.data_series_cache
            .enforce_limits(self.cache_policy.max_bytes, self.cache_policy.retention_days)?;
        Ok(result)
    }

    /// 获取指定时间窗口的历史 Tick 快照（不随行情更新）。
    pub async fn tick_data_series(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Tick>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
        }
        let start_nano = start_dt
            .timestamp_nanos_opt()
            .ok_or_else(|| TqError::InvalidParameter("start_dt 超出可表示范围".to_string()))?;
        let end_nano = end_dt
            .timestamp_nanos_opt()
            .ok_or_else(|| TqError::InvalidParameter("end_dt 超出可表示范围".to_string()))?;
        if end_nano <= start_nano {
            return Err(TqError::InvalidParameter("end_dt 必须晚于 start_dt".to_string()));
        }

        if !self.cache_policy.enabled {
            return self.download_tick_range(symbol, start_nano, end_nano).await;
        }

        let result = {
            let _lock = self.data_series_cache.lock_series(symbol, 0)?;
            let requested_range = Range::new(start_nano, end_nano);
            let cached_id_ranges = self.data_series_cache.get_rangeset_id(symbol, 0)?;
            let mut cached_dt_ranges = self.data_series_cache.get_rangeset_dt(symbol, 0, &cached_id_ranges)?;
            trim_last_datetime_range(&mut cached_dt_ranges, 100);

            let missing_dt_ranges = rangeset_difference(&vec![requested_range], &cached_dt_ranges);
            for missing in missing_dt_ranges {
                if missing.is_empty() {
                    continue;
                }
                let rows = self.download_tick_range(symbol, missing.start, missing.end).await?;
                if rows.is_empty() {
                    continue;
                }
                self.data_series_cache.write_tick_segment(symbol, &rows)?;
            }

            self.data_series_cache.merge_adjacent_files(symbol, 0)?;
            self.data_series_cache.read_tick_window(symbol, start_nano, end_nano)?
        };
        self.data_series_cache
            .enforce_limits(self.cache_policy.max_bytes, self.cache_policy.retention_days)?;
        Ok(result)
    }

    async fn tick_history_by_id(
        &self,
        symbol: &str,
        data_length: usize,
        left_id: i64,
    ) -> Result<Arc<SeriesSubscription>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
        }
        let view_width = normalize_data_length(data_length)?;
        self.subscribe(SeriesOptions {
            symbols: vec![symbol.to_string()],
            duration: 0,
            view_width,
            chart_id: None,
            left_kline_id: Some(left_id),
            focus_datetime: None,
            focus_position: None,
        })
        .await
    }

    async fn tick_history_with_focus(
        &self,
        symbol: &str,
        data_length: usize,
        focus_time: DateTime<Utc>,
        focus_position: i32,
    ) -> Result<Arc<SeriesSubscription>> {
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
            focus_datetime: Some(focus_time),
            focus_position: Some(focus_position),
        })
        .await
    }

    async fn download_kline_range(
        &self,
        symbol: &str,
        duration: StdDuration,
        start_nano: i64,
        end_nano: i64,
    ) -> Result<Vec<Kline>> {
        let focus_time = DateTime::<Utc>::from_timestamp_nanos(start_nano);
        let mut rows = Vec::new();
        let mut next_id = None;
        let mut use_focus = true;

        loop {
            let page = if use_focus {
                self.fetch_kline_page_by_focus(symbol, duration, focus_time).await?
            } else {
                self.fetch_kline_page_by_id(symbol, duration, next_id.unwrap_or_default())
                    .await?
            };
            let page_len = page.len();
            let Some(new_next) = extend_klines_until_end(&mut rows, page, end_nano) else {
                break;
            };
            if next_id == Some(new_next) || page_len < PAGE_VIEW_WIDTH {
                break;
            }
            next_id = Some(new_next);
            use_focus = false;
        }

        Ok(dedup_sort_klines_by_id(rows))
    }

    async fn download_tick_range(&self, symbol: &str, start_nano: i64, end_nano: i64) -> Result<Vec<Tick>> {
        let focus_time = DateTime::<Utc>::from_timestamp_nanos(start_nano);
        let mut rows = Vec::new();
        let mut next_id = None;
        let mut use_focus = true;

        loop {
            let page = if use_focus {
                self.fetch_tick_page_by_focus(symbol, focus_time).await?
            } else {
                self.fetch_tick_page_by_id(symbol, next_id.unwrap_or_default()).await?
            };
            let page_len = page.len();
            let Some(new_next) = extend_ticks_until_end(&mut rows, page, end_nano) else {
                break;
            };
            if next_id == Some(new_next) || page_len < PAGE_VIEW_WIDTH {
                break;
            }
            next_id = Some(new_next);
            use_focus = false;
        }

        Ok(dedup_sort_ticks_by_id(rows))
    }

    async fn fetch_kline_page_by_id(
        &self,
        symbol: &str,
        duration: StdDuration,
        left_kline_id: i64,
    ) -> Result<Vec<Kline>> {
        let sub = self
            .kline_history(symbol, duration, PAGE_VIEW_WIDTH, left_kline_id)
            .await?;
        match self.fetch_history_page_with_subscription(sub).await? {
            HistoryPage::Kline(rows) => Ok(rows),
            HistoryPage::Tick(_) => Err(TqError::InternalError("历史 K 线下载返回了 Tick 数据".to_string())),
        }
    }

    async fn fetch_kline_page_by_focus(
        &self,
        symbol: &str,
        duration: StdDuration,
        focus_time: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        let focus_position = history_focus_position(self.ws.auto_peek_enabled(), PAGE_VIEW_WIDTH);
        let sub = self
            .kline_history_with_focus(symbol, duration, PAGE_VIEW_WIDTH, focus_time, focus_position)
            .await?;
        match self.fetch_history_page_with_subscription(sub).await? {
            HistoryPage::Kline(rows) => Ok(rows),
            HistoryPage::Tick(_) => Err(TqError::InternalError("历史 K 线下载返回了 Tick 数据".to_string())),
        }
    }

    async fn fetch_tick_page_by_id(&self, symbol: &str, left_id: i64) -> Result<Vec<Tick>> {
        let sub = self.tick_history_by_id(symbol, PAGE_VIEW_WIDTH, left_id).await?;
        match self.fetch_history_page_with_subscription(sub).await? {
            HistoryPage::Tick(rows) => Ok(rows),
            HistoryPage::Kline(_) => Err(TqError::InternalError("历史 Tick 下载返回了 K 线数据".to_string())),
        }
    }

    async fn fetch_tick_page_by_focus(&self, symbol: &str, focus_time: DateTime<Utc>) -> Result<Vec<Tick>> {
        let focus_position = history_focus_position(self.ws.auto_peek_enabled(), PAGE_VIEW_WIDTH);
        let sub = self
            .tick_history_with_focus(symbol, PAGE_VIEW_WIDTH, focus_time, focus_position)
            .await?;
        match self.fetch_history_page_with_subscription(sub).await? {
            HistoryPage::Tick(rows) => Ok(rows),
            HistoryPage::Kline(_) => Err(TqError::InternalError("历史 Tick 下载返回了 K 线数据".to_string())),
        }
    }

    async fn fetch_history_page_with_subscription(&self, sub: Arc<SeriesSubscription>) -> Result<HistoryPage> {
        sub.start().await?;
        let fetched = if self.ws.auto_peek_enabled() {
            match tokio::time::timeout(HISTORY_CHUNK_FETCH_TIMEOUT, sub.wait_update()).await {
                Ok(Ok(snapshot)) => history_page_from_snapshot(&snapshot),
                Ok(Err(err)) => Err(err),
                Err(_) => Err(TqError::Timeout),
            }
        } else {
            let deadline = Instant::now() + HISTORY_CHUNK_FETCH_TIMEOUT;
            loop {
                self.ws.send(&json!({"aid": "peek_message"})).await?;

                let now = Instant::now();
                if now >= deadline {
                    break Err(TqError::Timeout);
                }
                let wait_duration = (deadline - now).min(HISTORY_MANUAL_PEEK_RETRY_INTERVAL);
                match tokio::time::timeout(wait_duration, sub.wait_update()).await {
                    Ok(Ok(snapshot)) => break history_page_from_snapshot(&snapshot),
                    Ok(Err(err)) => break Err(err),
                    Err(_) => continue,
                }
            }
        };
        if let Err(e) = sub.close().await {
            warn!("历史下载临时订阅关闭失败: {:?}", e);
        }
        fetched
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

        debug!(
            "创建 SeriesSubscription: chart_id={}, symbols={:?}, duration={}, view_width={}",
            options.chart_id.as_deref().unwrap_or(""),
            options.symbols,
            options.duration,
            options.view_width
        );

        Ok(Arc::new(SeriesSubscription::new(
            Arc::clone(&self.dm),
            Arc::clone(&self.ws),
            options,
        )?))
    }
}

fn history_page_from_snapshot(snapshot: &crate::types::SeriesSnapshot) -> Result<HistoryPage> {
    if let Some(single) = &snapshot.data.single {
        return Ok(HistoryPage::Kline(single.data.clone()));
    }
    if let Some(tick_data) = &snapshot.data.tick_data {
        return Ok(HistoryPage::Tick(tick_data.data.clone()));
    }
    Err(TqError::InternalError(
        "历史下载完成后未得到单合约 K 线或 Tick 数据".to_string(),
    ))
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

fn generate_chart_id(options: &SeriesOptions) -> String {
    let uid = Uuid::new_v4();
    if options.duration == 0 {
        format!("TQRS_tick_{}", uid)
    } else {
        format!("TQRS_kline_{}", uid)
    }
}

fn history_focus_position(auto_peek_enabled: bool, view_width: usize) -> i32 {
    if auto_peek_enabled {
        0
    } else {
        view_width.min(i32::MAX as usize) as i32
    }
}

fn extend_klines_until_end(target: &mut Vec<Kline>, page: Vec<Kline>, end_nano: i64) -> Option<i64> {
    let mut next_id = None;
    for row in page {
        if row.datetime == 0 || row.datetime >= end_nano {
            break;
        }
        next_id = row.id.checked_add(1);
        target.push(row);
    }
    next_id
}

fn extend_ticks_until_end(target: &mut Vec<Tick>, page: Vec<Tick>, end_nano: i64) -> Option<i64> {
    let mut next_id = None;
    for row in page {
        if row.datetime == 0 || row.datetime >= end_nano {
            break;
        }
        next_id = row.id.checked_add(1);
        target.push(row);
    }
    next_id
}

fn dedup_sort_klines_by_id(rows: Vec<Kline>) -> Vec<Kline> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        by_id.insert(row.id, row);
    }
    by_id.into_values().collect()
}

fn dedup_sort_ticks_by_id(rows: Vec<Tick>) -> Vec<Tick> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        by_id.insert(row.id, row);
    }
    by_id.into_values().collect()
}

enum HistoryPage {
    Kline(Vec<Kline>),
    Tick(Vec<Tick>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_focus_position_uses_zero_in_live_mode() {
        assert_eq!(history_focus_position(true, PAGE_VIEW_WIDTH), 0);
    }

    #[test]
    fn history_focus_position_uses_view_width_in_backtest_mode() {
        assert_eq!(history_focus_position(false, PAGE_VIEW_WIDTH), PAGE_VIEW_WIDTH as i32);
    }
}
