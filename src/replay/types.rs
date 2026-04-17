use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::{Result, TqError};
use crate::types::{Account, Position, Trade};

#[derive(Debug, Clone)]
pub struct ReplayConfig {
    pub start_dt: DateTime<Utc>,
    pub end_dt: DateTime<Utc>,
    pub initial_balance: f64,
}

impl ReplayConfig {
    pub fn new(start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Result<Self> {
        if end_dt <= start_dt {
            return Err(TqError::InvalidParameter("end_dt must be after start_dt".to_string()));
        }

        Ok(Self {
            start_dt,
            end_dt,
            initial_balance: 10_000_000.0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarState {
    Opening,
    Closed,
}

impl BarState {
    pub fn is_opening(self) -> bool {
        matches!(self, Self::Opening)
    }

    pub fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstrumentMetadata {
    pub symbol: String,
    pub exchange_id: String,
    pub instrument_id: String,
    pub class: String,
    pub underlying_symbol: String,
    pub option_class: String,
    pub strike_price: f64,
    pub trading_time: Value,
    pub price_tick: f64,
    pub volume_multiple: i32,
    pub margin: f64,
    pub commission: f64,
    pub open_min_market_order_volume: i32,
    pub open_min_limit_order_volume: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplayQuote {
    pub symbol: String,
    pub datetime_nanos: i64,
    pub last_price: f64,
    pub ask_price1: f64,
    pub ask_volume1: i64,
    pub bid_price1: f64,
    pub bid_volume1: i64,
    pub highest: f64,
    pub lowest: f64,
    pub average: f64,
    pub volume: i64,
    pub amount: f64,
    pub open_interest: i64,
    pub open_limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplayHandleId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub current_dt: DateTime<Utc>,
    pub updated_handles: Vec<ReplayHandleId>,
    pub updated_quotes: Vec<String>,
    pub settled_trading_day: Option<NaiveDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySettlementLog {
    pub trading_day: NaiveDate,
    pub account: Account,
    pub positions: Vec<Position>,
    pub trades: Vec<Trade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub settlements: Vec<DailySettlementLog>,
    pub final_accounts: Vec<Account>,
    pub final_positions: Vec<Position>,
    pub trades: Vec<Trade>,
    pub symbol_volume_multipliers: BTreeMap<String, i32>,
}
