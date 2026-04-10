use super::market::{ChartInfo, Kline, Tick};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

/// K线序列数据（带Chart信息）
#[derive(Debug, Clone)]
pub struct KlineSeriesData {
    pub symbol: String,
    pub duration: i64,
    pub chart_id: String,
    pub chart: Option<ChartInfo>,
    pub last_id: i64,
    pub trading_day_start_id: i64,
    pub trading_day_end_id: i64,
    pub data: Vec<Kline>,
    pub has_new_bar: bool,
}

/// Tick序列数据
#[derive(Debug, Clone)]
pub struct TickSeriesData {
    pub symbol: String,
    pub chart_id: String,
    pub chart: Option<ChartInfo>,
    pub last_id: i64,
    pub data: Vec<Tick>,
    pub has_new_bar: bool,
}

/// 对齐的K线集合（一个时间点的多个合约）
#[derive(Debug, Clone)]
pub struct AlignedKlineSet {
    pub main_id: i64,
    pub timestamp: DateTime<Utc>,
    pub klines: HashMap<String, Kline>,
}

/// K线元数据
#[derive(Debug, Clone)]
pub struct KlineMetadata {
    pub symbol: String,
    pub last_id: i64,
    pub trading_day_start_id: i64,
    pub trading_day_end_id: i64,
}

/// 多合约K线序列数据（已对齐）
#[derive(Debug, Clone)]
pub struct MultiKlineSeriesData {
    pub chart_id: String,
    pub duration: i64,
    pub main_symbol: String,
    pub symbols: Vec<String>,
    pub left_id: i64,
    pub right_id: i64,
    pub view_width: usize,
    pub data: Vec<AlignedKlineSet>,
    pub has_new_bar: bool,
    pub metadata: HashMap<String, KlineMetadata>,
}

/// 序列数据（统一接口）
#[derive(Debug, Clone)]
pub struct SeriesData {
    pub is_multi: bool,
    pub is_tick: bool,
    pub symbols: Vec<String>,
    pub single: Option<KlineSeriesData>,
    pub multi: Option<MultiKlineSeriesData>,
    pub tick_data: Option<TickSeriesData>,
}

impl SeriesData {
    /// 获取指定合约的K线数据
    pub fn get_symbol_klines(&self, symbol: &str) -> Option<&KlineSeriesData> {
        if self.is_multi || self.symbols.is_empty() || self.symbols[0] != symbol {
            return None;
        }
        self.single.as_ref()
    }
}

/// 序列订阅的最新快照
#[derive(Debug, Clone)]
pub struct SeriesSnapshot {
    pub data: Arc<SeriesData>,
    pub update: Arc<UpdateInfo>,
    pub epoch: i64,
}

/// 数据更新信息
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub has_new_bar: bool,
    pub new_bar_ids: HashMap<String, i64>,
    pub has_bar_update: bool,
    pub chart_range_changed: bool,
    pub old_left_id: i64,
    pub old_right_id: i64,
    pub new_left_id: i64,
    pub new_right_id: i64,
    pub has_chart_sync: bool,
    pub chart_ready: bool,
}

impl Default for UpdateInfo {
    fn default() -> Self {
        Self {
            has_new_bar: false,
            new_bar_ids: HashMap::new(),
            has_bar_update: false,
            chart_range_changed: false,
            old_left_id: -1,
            old_right_id: -1,
            new_left_id: -1,
            new_right_id: -1,
            has_chart_sync: false,
            chart_ready: false,
        }
    }
}

/// 序列订阅选项
#[derive(Debug, Clone)]
pub struct SeriesOptions {
    pub symbols: Vec<String>,
    pub duration: i64,
    pub view_width: usize,
    pub chart_id: Option<String>,
    pub left_kline_id: Option<i64>,
    pub focus_datetime: Option<DateTime<Utc>>,
    pub focus_position: Option<i32>,
}

impl Default for SeriesOptions {
    fn default() -> Self {
        Self {
            symbols: Vec::new(),
            duration: 0,
            view_width: 10000,
            chart_id: None,
            left_kline_id: None,
            focus_datetime: None,
            focus_position: None,
        }
    }
}
