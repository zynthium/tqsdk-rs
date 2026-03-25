use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolSettlement {
    pub datetime: String,
    pub symbol: String,
    pub settlement: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRanking {
    pub datetime: String,
    pub symbol: String,
    pub exchange_id: String,
    pub instrument_id: String,
    pub broker: String,
    pub volume: f64,
    pub volume_change: f64,
    pub volume_ranking: f64,
    pub long_oi: f64,
    pub long_change: f64,
    pub long_ranking: f64,
    pub short_oi: f64,
    pub short_change: f64,
    pub short_ranking: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingCalendarDay {
    pub date: String,
    pub trading: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingStatus {
    pub symbol: String,
    pub trade_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdbIndexData {
    pub date: String,
    pub values: HashMap<i32, f64>,
}
