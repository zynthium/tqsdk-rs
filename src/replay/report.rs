use serde::{Deserialize, Serialize};

use super::BacktestResult;

const TRADING_DAYS_OF_YEAR: f64 = 250.0;

fn balance_series(result: &BacktestResult) -> Vec<f64> {
    result
        .settlements
        .iter()
        .map(|settlement| settlement.account.balance)
        .collect()
}

fn initial_balance(result: &BacktestResult) -> Option<f64> {
    result
        .settlements
        .first()
        .map(|settlement| settlement.account.pre_balance)
}

fn finite_metric(value: f64) -> Option<f64> {
    if value.is_finite() { Some(value) } else { None }
}

fn return_rate(result: &BacktestResult) -> Option<f64> {
    let init_balance = initial_balance(result)?;
    let end_balance = result.settlements.last()?.account.balance;
    if init_balance <= 0.0 {
        return None;
    }
    finite_metric(end_balance / init_balance - 1.0)
}

fn annualized_return(result: &BacktestResult) -> Option<f64> {
    let init_balance = initial_balance(result)?;
    let end_balance = result.settlements.last()?.account.balance;
    let trading_days = result.settlements.len();
    if init_balance <= 0.0 || trading_days == 0 {
        return None;
    }
    finite_metric((end_balance / init_balance).powf(TRADING_DAYS_OF_YEAR / trading_days as f64) - 1.0)
}

fn max_drawdown(result: &BacktestResult) -> Option<f64> {
    let balances = balance_series(result);
    if balances.is_empty() {
        return None;
    }

    let mut max_balance = f64::NEG_INFINITY;
    let mut max_drawdown: f64 = 0.0;
    for balance in balances {
        max_balance = max_balance.max(balance);
        if max_balance > 0.0 {
            max_drawdown = max_drawdown.max((max_balance - balance) / max_balance);
        }
    }
    Some(max_drawdown)
}

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
            return_rate: return_rate(result),
            annualized_return: annualized_return(result),
            max_drawdown: max_drawdown(result),
            sharpe_ratio: None,
            sortino_ratio: None,
        }
    }
}
