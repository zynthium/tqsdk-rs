use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::BacktestResult;

const TRADING_DAYS_OF_YEAR: f64 = 250.0;
const RISK_FREE_RATE: f64 = 0.025;

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

#[derive(Debug, Clone)]
struct LotTrade {
    symbol: String,
    direction: String,
    offset: String,
    price: f64,
}

fn normalize_offset(offset: &str) -> &str {
    if offset == "CLOSETODAY" { "CLOSE" } else { offset }
}

fn expand_lot_trades(result: &BacktestResult) -> Vec<LotTrade> {
    let mut trades = Vec::new();
    for trade in &result.trades {
        let symbol = format!("{}.{}", trade.exchange_id, trade.instrument_id);
        for _ in 0..trade.volume.max(0) {
            trades.push(LotTrade {
                symbol: symbol.clone(),
                direction: trade.direction.clone(),
                offset: normalize_offset(&trade.offset).to_string(),
                price: trade.price,
            });
        }
    }
    trades
}

fn trade_metrics(result: &BacktestResult) -> (Option<f64>, Option<f64>) {
    let lot_trades = expand_lot_trades(result);
    let all_symbols = lot_trades
        .iter()
        .map(|trade| trade.symbol.clone())
        .collect::<BTreeSet<_>>();

    let mut profit_volumes = 0usize;
    let mut loss_volumes = 0usize;
    let mut profit_value = 0.0;
    let mut loss_value = 0.0;

    for symbol in all_symbols {
        let volume_multiple = *result.symbol_volume_multipliers.get(&symbol).unwrap_or(&1) as f64;
        for direction in ["BUY", "SELL"] {
            let close_direction = if direction == "BUY" { "SELL" } else { "BUY" };
            let open_prices = lot_trades
                .iter()
                .filter(|trade| trade.symbol == symbol && trade.direction == direction && trade.offset == "OPEN")
                .map(|trade| trade.price);
            let close_prices = lot_trades
                .iter()
                .filter(|trade| trade.symbol == symbol && trade.direction == close_direction && trade.offset == "CLOSE")
                .map(|trade| trade.price);

            for (open_price, close_price) in open_prices.zip(close_prices) {
                let profit = (close_price - open_price) * if direction == "BUY" { 1.0 } else { -1.0 };
                if profit >= 0.0 {
                    profit_volumes += 1;
                    profit_value += profit * volume_multiple;
                } else {
                    loss_volumes += 1;
                    loss_value += profit * volume_multiple;
                }
            }
        }
    }

    let total = profit_volumes + loss_volumes;
    let winning_rate = if total > 0 {
        Some(profit_volumes as f64 / total as f64)
    } else {
        None
    };

    let profit_loss_ratio = if profit_volumes > 0 && loss_volumes > 0 {
        let profit_per_volume = profit_value / profit_volumes as f64;
        let loss_per_volume = loss_value / loss_volumes as f64;
        if loss_per_volume == 0.0 {
            None
        } else {
            finite_metric((profit_per_volume / loss_per_volume).abs())
        }
    } else {
        None
    };

    (winning_rate, profit_loss_ratio)
}

fn daily_yields(result: &BacktestResult) -> Vec<f64> {
    let mut prev = match initial_balance(result) {
        Some(value) if value > 0.0 => value,
        _ => return Vec::new(),
    };

    let mut yields = Vec::new();
    for settlement in &result.settlements {
        let balance = settlement.account.balance;
        if prev > 0.0 {
            yields.push(balance / prev - 1.0);
        }
        prev = balance;
    }
    yields
}

fn daily_risk_free() -> f64 {
    (1.0 + RISK_FREE_RATE).powf(1.0 / TRADING_DAYS_OF_YEAR) - 1.0
}

fn population_stddev(values: &[f64], mu: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let variance = values.iter().map(|value| (value - mu).powi(2)).sum::<f64>() / values.len() as f64;
    let stddev = variance.sqrt();
    if stddev == 0.0 { None } else { Some(stddev) }
}

fn sharpe_ratio(result: &BacktestResult) -> Option<f64> {
    let yields = daily_yields(result);
    if yields.len() < 2 {
        return None;
    }
    let rf = daily_risk_free();
    let mean = yields.iter().sum::<f64>() / yields.len() as f64;
    let stddev = population_stddev(&yields, mean)?;
    finite_metric(TRADING_DAYS_OF_YEAR.sqrt() * (mean - rf) / stddev)
}

fn sortino_ratio(result: &BacktestResult) -> Option<f64> {
    let yields = daily_yields(result);
    if yields.len() < 2 {
        return None;
    }
    let rf = daily_risk_free();
    let mean = yields.iter().sum::<f64>() / yields.len() as f64;
    let downside = yields.iter().copied().filter(|value| *value < rf).collect::<Vec<_>>();
    let downside_stddev = population_stddev(&downside, rf)?;
    finite_metric(
        (TRADING_DAYS_OF_YEAR * downside.len() as f64 / yields.len() as f64).sqrt() * (mean - rf) / downside_stddev,
    )
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
        let (winning_rate, profit_loss_ratio) = trade_metrics(result);

        Self {
            trade_count: result.trades.len(),
            trading_day_count: result.settlements.len(),
            winning_rate,
            profit_loss_ratio,
            return_rate: return_rate(result),
            annualized_return: annualized_return(result),
            max_drawdown: max_drawdown(result),
            sharpe_ratio: sharpe_ratio(result),
            sortino_ratio: sortino_ratio(result),
        }
    }
}
