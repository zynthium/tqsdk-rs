use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::replay::{BarState, ReplayHandleId};
use crate::types::Kline;

#[derive(Debug, Clone)]
pub struct KlineSeriesRow {
    pub kline: Kline,
    pub state: BarState,
}

#[derive(Debug, Default, Clone)]
pub struct SeriesStore {
    klines: HashMap<(String, i64), Vec<KlineSeriesRow>>,
}

impl SeriesStore {
    pub fn push_kline(&mut self, symbol: &str, duration_nanos: i64, kline: Kline, state: BarState, width: usize) {
        let rows = self.klines.entry((symbol.to_string(), duration_nanos)).or_default();
        rows.push(KlineSeriesRow { kline, state });

        while rows.len() > width {
            rows.remove(0);
        }
    }

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
