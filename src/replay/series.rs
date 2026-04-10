use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::{Kline, Tick};

use super::types::{BarState, ReplayHandleId};

#[derive(Debug, Clone)]
pub struct KlineSeriesRow {
    pub kline: Kline,
    pub state: BarState,
}

#[derive(Debug, Clone)]
pub struct TickSeriesRow {
    pub tick: Tick,
}

#[derive(Debug, Default, Clone)]
pub struct SeriesStore {
    klines: HashMap<(String, i64), Vec<KlineSeriesRow>>,
    ticks: HashMap<String, Vec<TickSeriesRow>>,
}

impl SeriesStore {
    pub fn push_kline(&mut self, symbol: &str, duration_nanos: i64, kline: Kline, state: BarState, width: usize) {
        let rows = self.klines.entry((symbol.to_string(), duration_nanos)).or_default();
        rows.push(KlineSeriesRow { kline, state });

        while rows.len() > width {
            rows.remove(0);
        }
    }

    pub fn close_kline(&mut self, symbol: &str, duration_nanos: i64, kline: Kline, width: usize) {
        let rows = self.klines.entry((symbol.to_string(), duration_nanos)).or_default();

        if let Some(last) = rows.last_mut()
            && last.kline.id == kline.id
            && last.kline.datetime == kline.datetime
        {
            last.kline = kline;
            last.state = BarState::Closed;
            return;
        }

        rows.push(KlineSeriesRow {
            kline,
            state: BarState::Closed,
        });
        while rows.len() > width {
            rows.remove(0);
        }
    }

    #[cfg(test)]
    pub fn kline_rows(&self, symbol: &str, duration_nanos: i64) -> &[KlineSeriesRow] {
        self.klines
            .get(&(symbol.to_string(), duration_nanos))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn latest_kline(&self, symbol: &str, duration_nanos: i64) -> Option<&KlineSeriesRow> {
        self.klines
            .get(&(symbol.to_string(), duration_nanos))
            .and_then(|rows| rows.last())
    }

    pub fn push_tick(&mut self, symbol: &str, tick: Tick, width: usize) {
        let rows = self.ticks.entry(symbol.to_string()).or_default();
        rows.push(TickSeriesRow { tick });

        while rows.len() > width {
            rows.remove(0);
        }
    }

    #[cfg(test)]
    pub fn tick_rows(&self, symbol: &str) -> &[TickSeriesRow] {
        self.ticks.get(symbol).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Debug, Clone)]
pub struct AlignedKlineRow {
    pub datetime_nanos: i64,
    pub bars: HashMap<String, Option<KlineSeriesRow>>,
}

#[derive(Debug, Clone)]
pub struct AlignedKlineHandle {
    id: ReplayHandleId,
    symbols: Arc<[String]>,
    duration_nanos: i64,
    width: usize,
    rows: Arc<RwLock<VecDeque<AlignedKlineRow>>>,
}

impl AlignedKlineHandle {
    #[cfg(test)]
    pub(crate) fn new(id: ReplayHandleId, symbols: Vec<String>, duration_nanos: i64, width: usize) -> Self {
        Self {
            id,
            symbols: Arc::from(symbols),
            duration_nanos,
            width,
            rows: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    pub fn id(&self) -> &ReplayHandleId {
        &self.id
    }

    pub(crate) async fn push_row(&self, row: AlignedKlineRow) {
        let mut rows = self.rows.write().await;
        rows.push_back(row);
        while rows.len() > self.width {
            rows.pop_front();
        }
    }

    pub(crate) fn symbols(&self) -> &[String] {
        &self.symbols
    }

    pub(crate) fn duration_nanos(&self) -> i64 {
        self.duration_nanos
    }

    pub async fn rows(&self) -> Vec<AlignedKlineRow> {
        self.rows.read().await.iter().cloned().collect()
    }
}

#[derive(Debug, Clone)]
pub struct ReplayKlineHandle {
    id: ReplayHandleId,
    symbol: String,
    duration_nanos: i64,
    width: usize,
    rows: Arc<RwLock<VecDeque<KlineSeriesRow>>>,
}

impl ReplayKlineHandle {
    pub(crate) fn new(id: ReplayHandleId, symbol: String, duration_nanos: i64, width: usize) -> Self {
        Self {
            id,
            symbol,
            duration_nanos,
            width,
            rows: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    pub fn id(&self) -> &ReplayHandleId {
        &self.id
    }

    pub(crate) fn matches(&self, symbol: &str, duration_nanos: i64) -> bool {
        self.symbol == symbol && self.duration_nanos == duration_nanos
    }

    pub(crate) async fn apply_kline(&self, kline: Kline, state: BarState) {
        let mut rows = self.rows.write().await;

        if state.is_closed()
            && let Some(last) = rows.back_mut()
            && last.kline.id == kline.id
            && last.kline.datetime == kline.datetime
        {
            last.kline = kline;
            last.state = BarState::Closed;
            return;
        }

        rows.push_back(KlineSeriesRow { kline, state });
        while rows.len() > self.width {
            rows.pop_front();
        }
    }

    pub async fn rows(&self) -> Vec<KlineSeriesRow> {
        self.rows.read().await.iter().cloned().collect()
    }
}
