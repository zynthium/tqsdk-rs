use crate::errors::{Result, TqError};
use crate::types::{EdbIndexData, SymbolRanking, SymbolSettlement};
use polars::prelude::*;

/// 结算价 DataFrame 缓冲区
#[derive(Debug, Clone, Default)]
pub struct SettlementBuffer {
    pub datetimes: Vec<String>,
    pub symbols: Vec<String>,
    pub settlements: Vec<f64>,
}

impl SettlementBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: &[SymbolSettlement]) -> Self {
        let mut buffer = Self::new();
        for row in rows {
            buffer.push(row);
        }
        buffer
    }

    pub fn push(&mut self, row: &SymbolSettlement) {
        self.datetimes.push(row.datetime.clone());
        self.symbols.push(row.symbol.clone());
        self.settlements.push(row.settlement);
    }

    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.datetimes.is_empty() {
            return Err(TqError::Other("结算价缓冲区为空".to_string()));
        }
        let df = DataFrame::new(
            self.datetimes.len(),
            vec![
                Column::new("datetime".into(), &self.datetimes),
                Column::new("symbol".into(), &self.symbols),
                Column::new("settlement".into(), &self.settlements),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;
        Ok(df)
    }
}

/// 持仓排名 DataFrame 缓冲区
#[derive(Debug, Clone, Default)]
pub struct RankingBuffer {
    pub datetimes: Vec<String>,
    pub symbols: Vec<String>,
    pub exchange_ids: Vec<String>,
    pub instrument_ids: Vec<String>,
    pub brokers: Vec<String>,
    pub volumes: Vec<f64>,
    pub volume_changes: Vec<f64>,
    pub volume_rankings: Vec<f64>,
    pub long_ois: Vec<f64>,
    pub long_changes: Vec<f64>,
    pub long_rankings: Vec<f64>,
    pub short_ois: Vec<f64>,
    pub short_changes: Vec<f64>,
    pub short_rankings: Vec<f64>,
}

impl RankingBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: &[SymbolRanking]) -> Self {
        let mut buffer = Self::new();
        for row in rows {
            buffer.push(row);
        }
        buffer
    }

    pub fn push(&mut self, row: &SymbolRanking) {
        self.datetimes.push(row.datetime.clone());
        self.symbols.push(row.symbol.clone());
        self.exchange_ids.push(row.exchange_id.clone());
        self.instrument_ids.push(row.instrument_id.clone());
        self.brokers.push(row.broker.clone());
        self.volumes.push(row.volume);
        self.volume_changes.push(row.volume_change);
        self.volume_rankings.push(row.volume_ranking);
        self.long_ois.push(row.long_oi);
        self.long_changes.push(row.long_change);
        self.long_rankings.push(row.long_ranking);
        self.short_ois.push(row.short_oi);
        self.short_changes.push(row.short_change);
        self.short_rankings.push(row.short_ranking);
    }

    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.datetimes.is_empty() {
            return Err(TqError::Other("持仓排名缓冲区为空".to_string()));
        }
        let df = DataFrame::new(
            self.datetimes.len(),
            vec![
                Column::new("datetime".into(), &self.datetimes),
                Column::new("symbol".into(), &self.symbols),
                Column::new("exchange_id".into(), &self.exchange_ids),
                Column::new("instrument_id".into(), &self.instrument_ids),
                Column::new("broker".into(), &self.brokers),
                Column::new("volume".into(), &self.volumes),
                Column::new("volume_change".into(), &self.volume_changes),
                Column::new("volume_ranking".into(), &self.volume_rankings),
                Column::new("long_oi".into(), &self.long_ois),
                Column::new("long_change".into(), &self.long_changes),
                Column::new("long_ranking".into(), &self.long_rankings),
                Column::new("short_oi".into(), &self.short_ois),
                Column::new("short_change".into(), &self.short_changes),
                Column::new("short_ranking".into(), &self.short_rankings),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;
        Ok(df)
    }
}

/// EDB DataFrame 缓冲区
#[derive(Debug, Clone, Default)]
pub struct EdbBuffer {
    pub dates: Vec<String>,
    pub ids: Vec<i32>,
    pub values: Vec<f64>,
}

impl EdbBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: &[EdbIndexData]) -> Self {
        let mut buffer = Self::new();
        for row in rows {
            for (id, value) in row.values.iter() {
                buffer.dates.push(row.date.clone());
                buffer.ids.push(*id);
                buffer.values.push(*value);
            }
        }
        buffer
    }

    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.dates.is_empty() {
            return Err(TqError::Other("EDB 缓冲区为空".to_string()));
        }
        let df = DataFrame::new(
            self.dates.len(),
            vec![
                Column::new("date".into(), &self.dates),
                Column::new("id".into(), &self.ids),
                Column::new("value".into(), &self.values),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;
        Ok(df)
    }
}
