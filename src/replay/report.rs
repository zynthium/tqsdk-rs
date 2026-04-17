use serde::{Deserialize, Serialize};

use super::BacktestResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub trade_count: usize,
    pub trading_day_count: usize,
    pub winning_rate: Option<f64>,
    pub profit_loss_ratio: Option<f64>,
    pub return_rate: Option<f64>,
    pub annualized_return: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub sharpe_ratio: Option<f64>,
    pub sortino_ratio: Option<f64>,
}

impl ReplayReport {
    pub fn from_result(result: &BacktestResult) -> Self {
        Self {
            trade_count: result.trades.len(),
            trading_day_count: result.settlements.len(),
            winning_rate: None,
            profit_loss_ratio: None,
            return_rate: None,
            annualized_return: None,
            max_drawdown: None,
            sharpe_ratio: None,
            sortino_ratio: None,
        }
    }
}
