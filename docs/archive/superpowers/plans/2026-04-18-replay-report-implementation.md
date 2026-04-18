# Replay Report Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Python-aligned, GUI-free `ReplayReport` post-processing API that computes headline replay metrics from `BacktestResult` while keeping replay execution semantics unchanged.

**Architecture:** Keep replay correctness and analytics separate. First extend `BacktestResult` with the minimum raw symbol facts needed for report math, then add a new `src/replay/report.rs` module that computes counts, balance-based metrics, round-trip trade metrics, and risk-adjusted metrics from the raw result. Public access stays explicit: `let report = ReplayReport::from_result(&result);`.

**Tech Stack:** Rust 2024, `chrono`, `serde`, existing `src/replay/`, `src/types/`, Python reference formulas from `/Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/report.py` and `/Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/tafunc.py`

---

## File Structure

- Modify: `src/replay/types.rs`
  - Add the raw per-symbol multiplier map needed by report math.
- Modify: `src/replay/sim.rs`
  - Populate the raw multiplier map when producing `BacktestResult`.
- Create: `src/replay/report.rs`
  - Hold `ReplayReport` and all internal metric helpers.
- Modify: `src/replay/mod.rs`
  - Export `ReplayReport`.
- Modify: `src/lib.rs`
  - Re-export `ReplayReport` at crate root.
- Modify: `src/prelude.rs`
  - Re-export `ReplayReport` from the prelude.
- Modify: `src/replay/tests.rs`
  - Add replay report fixture helpers and metric tests.
- Modify: `README.md`
  - Document `ReplayReport::from_result(&BacktestResult)` and the no-GUI boundary.
- Modify: `docs/architecture.md`
  - Document replay result vs report layering.
- Modify: `skills/tqsdk-rs/SKILL.md`
  - Update routing and preferred public replay APIs.
- Modify: `skills/tqsdk-rs/references/simulation-and-backtest.md`
  - Add report usage and semantics.
- Modify: `skills/tqsdk-rs/references/tqsdk_rs_guide.md`
  - Add `ReplayReport` to the replay/backtest API map.

## Python Reference Semantics To Mirror

Implementation should follow these upstream formulas and rules:

- `TqReport._get_account_stat_metrics()` in `/Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/report.py`
  - `ror = end_balance / init_balance - 1`
  - `annual_yield = (end_balance / init_balance) ** (250 / trading_days) - 1`
  - `max_drawdown = max((cummax(balance) - balance) / cummax(balance))`
- `TqReport._get_trades_stat_metrics()` in `/Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/report.py`
  - expand each `Trade.volume` into one-lot records
  - normalize `CLOSETODAY` to `CLOSE`
  - pair `OPEN` and `CLOSE` lots by symbol and direction order
  - multiply per-lot price difference by `volume_multiple`
- `get_sharp()` / `get_sortino()` in `/Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/tafunc.py`
  - annualization base: `250`
  - risk-free rate: `0.025`
  - standard deviation is population stddev, not sample stddev

Rust public behavior differs intentionally at the boundary:

- Python `nan` or `inf` outputs become `None`
- counts remain concrete integers

### Task 1: Extend `BacktestResult` With Raw Symbol Multipliers

**Files:**
- Modify: `src/replay/types.rs`
- Modify: `src/replay/sim.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Write the failing test**

Add this test near the existing `sim_broker_finish_returns_accumulated_settlements_and_final_state` coverage in `src/replay/tests.rs`:

```rust
#[test]
fn sim_broker_finish_includes_symbol_volume_multipliers_for_reports() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2605".to_string(),
        class: "FUTURE".to_string(),
        volume_multiple: 10,
        margin: 1_000.0,
        commission: 2.0,
        ..InstrumentMetadata::default()
    });

    let result = broker.finish().unwrap();

    assert_eq!(result.symbol_volume_multipliers.get("SHFE.rb2605"), Some(&10));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test sim_broker_finish_includes_symbol_volume_multipliers_for_reports -- --nocapture`

Expected: FAIL with a compile error because `BacktestResult` has no `symbol_volume_multipliers` field yet.

- [ ] **Step 3: Write the minimal implementation**

In `src/replay/types.rs`, extend `BacktestResult`:

```rust
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub settlements: Vec<DailySettlementLog>,
    pub final_accounts: Vec<Account>,
    pub final_positions: Vec<Position>,
    pub trades: Vec<Trade>,
    pub symbol_volume_multipliers: BTreeMap<String, i32>,
}
```

In `src/replay/sim.rs`, populate the new field inside `finish()`:

```rust
pub fn finish(&self) -> Result<BacktestResult> {
    let symbol_volume_multipliers = self
        .metadata
        .iter()
        .map(|(symbol, metadata)| (symbol.clone(), metadata.volume_multiple))
        .collect();

    Ok(BacktestResult {
        settlements: self.settlements.clone(),
        final_accounts: self.sorted_accounts_snapshot(),
        final_positions: self.sorted_positions_snapshot(),
        trades: self.all_trades.clone(),
        symbol_volume_multipliers,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test sim_broker_finish_includes_symbol_volume_multipliers_for_reports -- --nocapture`

Expected: PASS

Then run: `cargo test sim_broker_finish_returns_accumulated_settlements_and_final_state -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/replay/types.rs src/replay/sim.rs src/replay/tests.rs
git commit -m "feat(replay): retain symbol multipliers in backtest results"
```

### Task 2: Add `ReplayReport` Surface And Root/Prelude Exports

**Files:**
- Create: `src/replay/report.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Test: `src/replay/tests.rs`
- Test: `src/prelude.rs`

- [ ] **Step 1: Write the failing tests**

Add a new replay-report count test to `src/replay/tests.rs`:

```rust
fn sample_trade(
    exchange_id: &str,
    instrument_id: &str,
    direction: &str,
    offset: &str,
    volume: i64,
    price: f64,
    trade_date_time: i64,
) -> Trade {
    Trade {
        seqno: 0,
        user_id: "TQSIM".to_string(),
        trade_id: String::new(),
        exchange_id: exchange_id.to_string(),
        instrument_id: instrument_id.to_string(),
        order_id: String::new(),
        exchange_trade_id: String::new(),
        direction: direction.to_string(),
        offset: offset.to_string(),
        volume,
        price,
        trade_date_time,
        commission: 0.0,
        epoch: None,
    }
}

fn account_with_balance(pre_balance: f64, balance: f64) -> Account {
    Account {
        user_id: "TQSIM".to_string(),
        currency: "CNY".to_string(),
        available: balance,
        balance,
        close_profit: 0.0,
        commission: 0.0,
        ctp_available: balance,
        ctp_balance: balance,
        deposit: 0.0,
        float_profit: 0.0,
        frozen_commission: 0.0,
        frozen_margin: 0.0,
        frozen_premium: 0.0,
        margin: 0.0,
        market_value: 0.0,
        position_profit: 0.0,
        pre_balance,
        premium: 0.0,
        risk_ratio: 0.0,
        static_balance: balance,
        withdraw: 0.0,
        epoch: None,
    }
}

#[test]
fn replay_report_counts_days_and_trades() {
    let result = BacktestResult {
        settlements: vec![
            DailySettlementLog {
                trading_day: NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
                account: account_with_balance(10_000_000.0, 10_000_000.0),
                positions: vec![],
                trades: vec![],
            },
            DailySettlementLog {
                trading_day: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
                account: account_with_balance(10_100_000.0, 10_100_000.0),
                positions: vec![],
                trades: vec![],
            },
        ],
        final_accounts: vec![],
        final_positions: vec![],
        trades: vec![
            sample_trade("SHFE", "rb2605", "BUY", "OPEN", 1, 3_200.0, 1_000),
            sample_trade("SHFE", "rb2605", "SELL", "CLOSE", 1, 3_210.0, 2_000),
        ],
        symbol_volume_multipliers: BTreeMap::from([("SHFE.rb2605".to_string(), 10)]),
    };

    let report = ReplayReport::from_result(&result);

    assert_eq!(report.trade_count, 2);
    assert_eq!(report.trading_day_count, 2);
}
```

Add this export test to `src/prelude.rs`:

```rust
#[test]
fn root_and_prelude_export_replay_report() {
    let _root_report: Option<crate::ReplayReport> = None;

    use crate::prelude::*;
    let _prelude_report: Option<ReplayReport> = None;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_report_counts_days_and_trades -- --nocapture`

Expected: FAIL because `ReplayReport` does not exist yet.

Then run: `cargo test root_and_prelude_export_replay_report -- --nocapture`

Expected: FAIL because root/prelude exports do not exist yet.

- [ ] **Step 3: Write the minimal implementation**

Create `src/replay/report.rs`:

```rust
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
```

Wire exports:

`src/replay/mod.rs`

```rust
mod report;

pub use report::ReplayReport;
```

`src/lib.rs`

```rust
pub use replay::{ReplayConfig, ReplayReport, ReplaySession};
```

`src/prelude.rs`

```rust
pub use crate::{
    // ...
    ReplayConfig, ReplayReport, ReplaySession,
    // ...
};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_report_counts_days_and_trades -- --nocapture`

Expected: PASS

Then run: `cargo test root_and_prelude_export_replay_report -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/replay/report.rs src/replay/mod.rs src/lib.rs src/prelude.rs src/replay/tests.rs
git commit -m "feat(replay): add replay report surface"
```

### Task 3: Add Balance-Based Metrics (`return_rate`, `annualized_return`, `max_drawdown`)

**Files:**
- Modify: `src/replay/report.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Write the failing tests**

Add reusable helpers to `src/replay/tests.rs`:

```rust
fn settlement_with_balance(trading_day: NaiveDate, pre_balance: f64, balance: f64) -> DailySettlementLog {
    DailySettlementLog {
        trading_day,
        account: account_with_balance(pre_balance, balance),
        positions: vec![],
        trades: vec![],
    }
}

fn empty_backtest_result() -> BacktestResult {
    BacktestResult {
        settlements: vec![],
        final_accounts: vec![],
        final_positions: vec![],
        trades: vec![],
        symbol_volume_multipliers: BTreeMap::new(),
    }
}
```

Add these metric tests:

```rust
#[test]
fn replay_report_returns_none_for_balance_metrics_without_settlements() {
    let report = ReplayReport::from_result(&empty_backtest_result());

    assert_eq!(report.return_rate, None);
    assert_eq!(report.annualized_return, None);
    assert_eq!(report.max_drawdown, None);
}

#[test]
fn replay_report_computes_return_and_annualized_yield_from_daily_settlements() {
    let result = BacktestResult {
        settlements: vec![
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(), 10_000_000.0, 10_100_000.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(), 10_100_000.0, 10_500_000.0),
        ],
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);
    let ror = 10_500_000.0 / 10_000_000.0 - 1.0;
    let annual = (10_500_000.0 / 10_000_000.0).powf(250.0 / 2.0) - 1.0;

    assert_close(report.return_rate.unwrap(), ror);
    assert_close(report.annualized_return.unwrap(), annual);
}

#[test]
fn replay_report_computes_max_drawdown_from_running_peak_balance() {
    let result = BacktestResult {
        settlements: vec![
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(), 10_000_000.0, 10_000_000.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(), 10_000_000.0, 11_000_000.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(), 11_000_000.0, 9_900_000.0),
        ],
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_close(report.max_drawdown.unwrap(), 0.1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_report_returns_none_for_balance_metrics_without_settlements -- --nocapture`

Expected: PASS because the Task 2 stub already returns `None` for these fields.

Then run: `cargo test replay_report_computes_return_and_annualized_yield_from_daily_settlements -- --nocapture`

Expected: FAIL

Then run: `cargo test replay_report_computes_max_drawdown_from_running_peak_balance -- --nocapture`

Expected: FAIL

- [ ] **Step 3: Write the minimal implementation**

In `src/replay/report.rs`, add Python-compatible constants and helpers:

```rust
const TRADING_DAYS_OF_YEAR: f64 = 250.0;

fn balance_series(result: &BacktestResult) -> Vec<f64> {
    result
        .settlements
        .iter()
        .map(|settlement| settlement.account.balance)
        .collect()
}

fn initial_balance(result: &BacktestResult) -> Option<f64> {
    result.settlements.first().map(|settlement| settlement.account.pre_balance)
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
    let mut max_drawdown = 0.0;
    for balance in balances {
        max_balance = max_balance.max(balance);
        if max_balance > 0.0 {
            max_drawdown = max_drawdown.max((max_balance - balance) / max_balance);
        }
    }
    Some(max_drawdown)
}

fn finite_metric(value: f64) -> Option<f64> {
    if value.is_finite() {
        Some(value)
    } else {
        None
    }
}
```

Then wire them into `ReplayReport::from_result()`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_report_ -- --nocapture`

Expected: the three balance-metric tests above PASS, while round-trip and risk-adjusted report tests do not exist yet.

- [ ] **Step 5: Commit**

```bash
git add src/replay/report.rs src/replay/tests.rs
git commit -m "feat(replay): compute replay balance metrics"
```

### Task 4: Add Round-Trip Trade Metrics (`winning_rate`, `profit_loss_ratio`)

**Files:**
- Modify: `src/replay/report.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Write the failing tests**

Reuse the `sample_trade(...)` helper added in Task 2 and add these round-trip tests:

```rust
#[test]
fn replay_report_uses_python_style_lot_pairing_for_win_rate_and_profit_loss_ratio() {
    let result = BacktestResult {
        trades: vec![
            sample_trade("SHFE", "rb2605", "BUY", "OPEN", 2, 3_200.0, 1_000),
            sample_trade("SHFE", "rb2605", "SELL", "CLOSE", 1, 3_220.0, 2_000),
            sample_trade("SHFE", "rb2605", "SELL", "CLOSE", 1, 3_180.0, 3_000),
        ],
        symbol_volume_multipliers: BTreeMap::from([("SHFE.rb2605".to_string(), 10)]),
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_close(report.winning_rate.unwrap(), 0.5);
    assert_close(report.profit_loss_ratio.unwrap(), 1.0);
}

#[test]
fn replay_report_returns_none_for_round_trip_metrics_without_completed_pairs() {
    let result = BacktestResult {
        trades: vec![sample_trade("SHFE", "rb2605", "BUY", "OPEN", 1, 3_200.0, 1_000)],
        symbol_volume_multipliers: BTreeMap::from([("SHFE.rb2605".to_string(), 10)]),
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_eq!(report.winning_rate, None);
    assert_eq!(report.profit_loss_ratio, None);
}

#[test]
fn replay_report_normalizes_closetoday_to_close_before_pairing() {
    let result = BacktestResult {
        trades: vec![
            sample_trade("SHFE", "rb2605", "BUY", "OPEN", 1, 3_200.0, 1_000),
            sample_trade("SHFE", "rb2605", "SELL", "CLOSETODAY", 1, 3_210.0, 2_000),
        ],
        symbol_volume_multipliers: BTreeMap::from([("SHFE.rb2605".to_string(), 10)]),
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_close(report.winning_rate.unwrap(), 1.0);
    assert_eq!(report.profit_loss_ratio, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_report_uses_python_style_lot_pairing_for_win_rate_and_profit_loss_ratio -- --nocapture`

Expected: FAIL because round-trip logic is not implemented yet.

Then run: `cargo test replay_report_normalizes_closetoday_to_close_before_pairing -- --nocapture`

Expected: FAIL

- [ ] **Step 3: Write the minimal implementation**

In `src/replay/report.rs`, add helpers that mirror Python `TqReport._get_trades_stat_metrics()`:

```rust
#[derive(Debug, Clone)]
struct LotTrade {
    symbol: String,
    direction: String,
    offset: String,
    price: f64,
}

fn normalize_offset(offset: &str) -> &str {
    if offset == "CLOSETODAY" {
        "CLOSE"
    } else {
        offset
    }
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
            let closes_against = if direction == "BUY" { "SELL" } else { "BUY" };
            let open_prices = lot_trades
                .iter()
                .filter(|trade| trade.symbol == symbol && trade.direction == direction && trade.offset == "OPEN")
                .map(|trade| trade.price);
            let close_prices = lot_trades
                .iter()
                .filter(|trade| trade.symbol == symbol && trade.direction == closes_against && trade.offset == "CLOSE")
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
            Some((profit_per_volume / loss_per_volume).abs())
        }
    } else {
        None
    };

    (winning_rate, profit_loss_ratio)
}
```

Wire into `ReplayReport::from_result()`:

```rust
let (winning_rate, profit_loss_ratio) = trade_metrics(result);

Self {
    // ...
    winning_rate,
    profit_loss_ratio,
    // ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_report_ -- --nocapture`

Expected: all round-trip and balance report tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/replay/report.rs src/replay/tests.rs
git commit -m "feat(replay): add replay round-trip metrics"
```

### Task 5: Add Risk-Adjusted Metrics (`sharpe_ratio`, `sortino_ratio`) And Python-Style Daily Yield Helpers

**Files:**
- Modify: `src/replay/report.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Write the failing tests**

Add these risk-adjusted metric tests:

```rust
#[test]
fn replay_report_returns_none_for_risk_adjusted_metrics_without_enough_daily_yields() {
    let result = BacktestResult {
        settlements: vec![settlement_with_balance(
            NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
            10_000_000.0,
            10_100_000.0,
        )],
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_eq!(report.sharpe_ratio, None);
    assert_eq!(report.sortino_ratio, None);
}

#[test]
fn replay_report_returns_none_when_python_formula_would_divide_by_zero_stddev() {
    let result = BacktestResult {
        settlements: vec![
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(), 10_000_000.0, 11_000_000.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(), 11_000_000.0, 12_100_000.0),
        ],
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);

    assert_eq!(report.sharpe_ratio, None);
}

#[test]
fn replay_report_matches_python_sharpe_and_sortino_formulas() {
    let result = BacktestResult {
        settlements: vec![
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(), 100.0, 110.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(), 110.0, 99.0),
            settlement_with_balance(NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(), 99.0, 108.9),
        ],
        ..empty_backtest_result()
    };

    let report = ReplayReport::from_result(&result);
    let rf = (1.0_f64 + 0.025).powf(1.0 / 250.0) - 1.0;
    let yields = [0.10_f64, -0.10_f64, 0.10_f64];
    let mean = yields.iter().sum::<f64>() / yields.len() as f64;
    let stddev = (yields.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / yields.len() as f64).sqrt();
    let expected_sharpe = 250.0_f64.sqrt() * (mean - rf) / stddev;

    let downside = yields.into_iter().filter(|v| *v < rf).collect::<Vec<_>>();
    let downside_stddev = (downside.iter().map(|v| (v - rf).powi(2)).sum::<f64>() / downside.len() as f64).sqrt();
    let expected_sortino = (250.0_f64 * downside.len() as f64 / 3.0).sqrt() * (mean - rf) / downside_stddev;

    assert_close(report.sharpe_ratio.unwrap(), expected_sharpe);
    assert_close(report.sortino_ratio.unwrap(), expected_sortino);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_report_matches_python_sharpe_and_sortino_formulas -- --nocapture`

Expected: FAIL because `sharpe_ratio` and `sortino_ratio` are not implemented yet.

- [ ] **Step 3: Write the minimal implementation**

In `src/replay/report.rs`, add Python-compatible daily-yield and population-stddev helpers:

```rust
const RISK_FREE_RATE: f64 = 0.025;

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
    if stddev == 0.0 {
        None
    } else {
        Some(stddev)
    }
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
    finite_metric((TRADING_DAYS_OF_YEAR * downside.len() as f64 / yields.len() as f64).sqrt() * (mean - rf) / downside_stddev)
}
```

Then wire both into `ReplayReport::from_result()`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_report_ -- --nocapture`

Expected: all report tests PASS, including risk-adjusted metrics.

- [ ] **Step 5: Commit**

```bash
git add src/replay/report.rs src/replay/tests.rs
git commit -m "feat(replay): add replay risk-adjusted metrics"
```

### Task 6: Sync Docs And Skill References, Then Run Full Verification

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `skills/tqsdk-rs/SKILL.md`
- Modify: `skills/tqsdk-rs/references/simulation-and-backtest.md`
- Modify: `skills/tqsdk-rs/references/tqsdk_rs_guide.md`

- [ ] **Step 1: Update user-facing docs**

In `README.md`, add report usage after the `finish()` example:

```rust
let result = session.finish().await?;
let report = ReplayReport::from_result(&result);
println!("trades={}", report.trade_count);
println!("max_drawdown={:?}", report.max_drawdown);
```

Also document:

- `ReplayReport` is a post-processing API
- v1 is GUI-free
- unstable metrics use `Option<f64>`

In `docs/architecture.md`, add a replay layering note:

```text
ReplaySession / SimBroker produce raw BacktestResult facts.
ReplayReport is a separate post-processing layer that computes Python-aligned headline metrics from those raw results.
```

Update `skills/tqsdk-rs/SKILL.md` and `skills/tqsdk-rs/references/tqsdk_rs_guide.md` to mention:

- `ReplayReport::from_result(&BacktestResult)`
- replay result vs report separation

Update `skills/tqsdk-rs/references/simulation-and-backtest.md` with a short `finish() -> ReplayReport` example.

- [ ] **Step 2: Run doc consistency review**

Run: `rg -n "ReplayReport|BacktestResult|from_result|report" README.md docs/architecture.md skills/tqsdk-rs/SKILL.md skills/tqsdk-rs/references/simulation-and-backtest.md skills/tqsdk-rs/references/tqsdk_rs_guide.md`

Expected: all updated files mention the new API consistently, with no stale wording that implies GUI rendering.

- [ ] **Step 3: Run full verification**

Run: `cargo fmt --check`

Expected: PASS

Run: `cargo test`

Expected: PASS

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add README.md docs/architecture.md skills/tqsdk-rs/SKILL.md skills/tqsdk-rs/references/simulation-and-backtest.md skills/tqsdk-rs/references/tqsdk_rs_guide.md
git commit -m "docs(replay): document replay report API"
```

## Self-Review

### Spec coverage

- independent `ReplayReport` type: covered by Task 2
- pure post-processing from `BacktestResult`: Tasks 1-5 preserve this split
- headline metrics only: Tasks 3-5 cover counts, return, annualized return, drawdown, winning rate, profit/loss ratio, Sharpe, Sortino
- Python-first semantics: Tasks 3-5 explicitly follow formulas from `tqsdk-python`
- `Option<f64>` for unstable metrics: Tasks 3-5 test and implement `None`
- no GUI dependency: docs updates in Task 6 keep that boundary explicit

### Placeholder scan

- No `TODO` / `TBD`
- Every task lists exact files
- Every code-writing step includes concrete snippets
- Every test step includes exact commands and expected results

### Type consistency

- `ReplayReport::from_result(&BacktestResult)` is the only public constructor used throughout
- `symbol_volume_multipliers` is introduced once in `BacktestResult` and reused consistently
- `trade_count` / `trading_day_count` stay `usize`
- all floating metrics stay `Option<f64>`
