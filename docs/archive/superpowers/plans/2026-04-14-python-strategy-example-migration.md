# Python Strategy Example Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add replay/backtest-friendly Rust migrations of the Python `doublema`, `dualthrust`, and `rbreaker` strategy demos, plus the nearby Python reference files and example documentation updates.

**Architecture:** Keep each Rust example as a single standalone file in `examples/`, following the existing `ReplaySession` + `session.quote(&symbol)` + `runtime.account("TQSIM").target_pos(&symbol).build()` pattern already used by `backtest.rs` and `pivot_point.rs`. Strategy math stays in pure helper functions inside each example, and strategy state stays explicit in the example file so the public usage remains easy to read and aligned with the Python demos.

**Tech Stack:** Rust 2024, `tokio`, `chrono`, `tqsdk-rs` replay/runtime APIs, Cargo example tests, README and skill reference docs.

---

## File Structure

- Create: `examples/doublema.rs`
  Purpose: replay/backtest-friendly moving-average crossover strategy example with inline unit tests for SMA crossover logic.
- Create: `examples/dualthrust.rs`
  Purpose: replay/backtest-friendly breakout strategy example with inline unit tests for Dual Thrust rail calculation.
- Create: `examples/rbreaker.rs`
  Purpose: replay/backtest-friendly reversal strategy example with inline unit tests for the seven R-Breaker levels.
- Create: `examples/python/doublema.py`
  Purpose: keep the original Python reference demo in-repo next to the Rust migration.
- Create: `examples/python/dualthrust.py`
  Purpose: keep the original Python reference demo in-repo next to the Rust migration.
- Create: `examples/python/rbreaker.py`
  Purpose: keep the original Python reference demo in-repo next to the Rust migration.
- Modify: `README.md`
  Purpose: add the three new example commands and rows in the example table.
- Modify: `skills/tqsdk-rs/references/example-map.md`
  Purpose: teach future agents which replay/backtest scenarios map to the new examples.

No new public modules, helper crates, or compatibility facades are added in this plan.

### Task 1: Import The Python Reference Demos

**Files:**
- Create: `examples/python/doublema.py`
- Create: `examples/python/dualthrust.py`
- Create: `examples/python/rbreaker.py`

- [ ] **Step 1: Copy the Python source files into `examples/python/`**

Run:

```bash
install -m 644 /Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/demo/example/doublema.py examples/python/doublema.py
install -m 644 /Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/demo/example/dualthrust.py examples/python/dualthrust.py
install -m 644 /Users/joeslee/Projects/GitHub/tqsdk-python/tqsdk/demo/example/rbreaker.py examples/python/rbreaker.py
```

Expected: the three files exist under `examples/python/` and preserve the original shebang/comments.

- [ ] **Step 2: Verify the copied Python files are syntactically valid**

Run:

```bash
python3 -m py_compile examples/python/doublema.py examples/python/dualthrust.py examples/python/rbreaker.py
```

Expected: no output and exit code `0`.

- [ ] **Step 3: Commit the reference-file import**

Run:

```bash
git add examples/python/doublema.py examples/python/dualthrust.py examples/python/rbreaker.py
git commit -m "docs: add python strategy example references"
```

Expected: one commit containing only the three Python reference files.

### Task 2: Implement `examples/doublema.rs`

**Files:**
- Create: `examples/doublema.rs`
- Test: `examples/doublema.rs`

- [ ] **Step 1: Create the example file with a failing crossover test**

Write this initial file:

```rust
fn main() {}

#[cfg(test)]
mod tests {
    use super::cross_signal;

    #[test]
    fn bullish_cross_sets_long_target() {
        let closes = vec![10.0, 10.0, 10.0, 9.0, 11.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(3));
    }

    #[test]
    fn bearish_cross_sets_short_target() {
        let closes = vec![10.0, 10.0, 10.0, 11.0, 9.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(-3));
    }
}
```

- [ ] **Step 2: Run the new example test target and verify it fails**

Run:

```bash
cargo test --example doublema
```

Expected: FAIL with a compile error that `cross_signal` does not exist.

- [ ] **Step 3: Implement the pure SMA and crossover helpers**

Replace the file contents with this helper layer and keep the tests:

```rust
use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const SHORT: usize = 30;
const LONG: usize = 60;
const TARGET_VOLUME: i64 = 3;
const BAR_DURATION: Duration = Duration::from_secs(60);
const DEFAULT_SYMBOL: &str = "SHFE.bu2012";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 9, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 11, 30);

fn sma(values: &[f64], window: usize) -> Option<f64> {
    if values.len() < window {
        return None;
    }
    let tail = &values[values.len() - window..];
    Some(tail.iter().sum::<f64>() / window as f64)
}

fn cross_signal(closes: &[f64], short: usize, long: usize, target: i64) -> Option<i64> {
    if closes.len() < long + 1 {
        return None;
    }
    let prev = &closes[..closes.len() - 1];
    let prev_short = sma(prev, short)?;
    let prev_long = sma(prev, long)?;
    let curr_short = sma(closes, short)?;
    let curr_long = sma(closes, long)?;

    if prev_long < prev_short && curr_long > curr_short {
        Some(-target)
    } else if prev_short < prev_long && curr_short > curr_long {
        Some(target)
    } else {
        None
    }
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> StdResult<NaiveDate, Box<dyn Error>> {
    match env::var(name) {
        Ok(raw) => Ok(NaiveDate::parse_from_str(&raw, "%Y-%m-%d")?),
        Err(_) => Ok(NaiveDate::from_ymd_opt(default.0, default.1, default.2).ok_or("invalid default date")?),
    }
}

fn shanghai_range(
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StdResult<(DateTime<Utc>, DateTime<Utc>), Box<dyn Error>> {
    let tz = FixedOffset::east_opt(8 * 3600).ok_or("unable to build Asia/Shanghai offset")?;
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or("invalid start time")?)
        .single()
        .ok_or("ambiguous start time")?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or("invalid end time")?)
        .single()
        .ok_or("ambiguous end time")?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

fn main() {}

#[cfg(test)]
mod tests {
    use super::cross_signal;

    #[test]
    fn bullish_cross_sets_long_target() {
        let closes = vec![10.0, 10.0, 10.0, 9.0, 11.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(3));
    }

    #[test]
    fn bearish_cross_sets_short_target() {
        let closes = vec![10.0, 10.0, 10.0, 11.0, 9.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(-3));
    }
}
```

- [ ] **Step 4: Run the example tests and verify the helper logic passes**

Run:

```bash
cargo test --example doublema
```

Expected: PASS with `2 passed`.

- [ ] **Step 5: Replace the stub `main()` with the full replay example**

Update `main()` to this shape:

```rust
#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;
    let _quote = session.quote(&symbol).await?;
    let bars = session.kline(&symbol, BAR_DURATION, LONG + 4).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut processed_bar_id = None;
    while let Some(step) = session.step().await? {
        if !step.updated_handles.iter().any(|id| id == bars.id()) {
            continue;
        }

        let rows = bars.rows().await;
        if rows.len() < LONG + 1 {
            continue;
        }

        let last = rows.last().expect("rows checked");
        if !last.state.is_closed() || processed_bar_id == Some(last.kline.id) {
            continue;
        }

        processed_bar_id = Some(last.kline.id);
        let closes = rows.iter().map(|row| row.kline.close).collect::<Vec<_>>();
        if let Some(target) = cross_signal(&closes, SHORT, LONG, TARGET_VOLUME) {
            if target > 0 {
                println!("均线上穿，做多");
            } else {
                println!("均线下穿，做空");
            }
            task.set_target_volume(target)?;
        }
    }

    task.cancel().await?;
    task.wait_finished().await?;
    session.finish().await?;
    println!("回测结束");
    Ok(())
}
```

- [ ] **Step 6: Run the new example end-to-end**

Run:

```bash
cargo run --example doublema
```

Expected: replay starts, prints `策略开始运行`, emits at least one moving-average signal, and exits with `回测结束`.

- [ ] **Step 7: Commit the `doublema` migration**

Run:

```bash
git add examples/doublema.rs
git commit -m "feat: add replay doublema example"
```

Expected: one commit containing only the new Rust example.

### Task 3: Implement `examples/dualthrust.rs`

**Files:**
- Create: `examples/dualthrust.rs`
- Test: `examples/dualthrust.rs`

- [ ] **Step 1: Create the example file with a failing rail-calculation test**

Write this initial file:

```rust
fn main() {}

#[cfg(test)]
mod tests {
    use super::{dual_thrust_lines, DualThrustLines};

    #[test]
    fn calculates_dual_thrust_rails() {
        let lines: DualThrustLines = dual_thrust_lines(100.0, &[110.0, 108.0], &[107.0, 106.0], &[109.0, 105.0], 0.2, 0.2)
            .expect("lines should exist");
        assert!((lines.buy_line - 101.0).abs() < 1e-9);
        assert!((lines.sell_line - 99.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run the new example test target and verify it fails**

Run:

```bash
cargo test --example dualthrust
```

Expected: FAIL with missing `DualThrustLines` / `dual_thrust_lines`.

- [ ] **Step 3: Implement the pure rail-calculation helper**

Replace the file contents with this helper layer and keep the tests:

```rust
use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const NDAY: usize = 5;
const K1: f64 = 0.2;
const K2: f64 = 0.2;
const TARGET_VOLUME: i64 = 3;
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const DEFAULT_SYMBOL: &str = "DCE.jd2011";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 8, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 10, 31);

#[derive(Debug, Clone, Copy)]
struct DualThrustLines {
    current_open: f64,
    buy_line: f64,
    sell_line: f64,
}

fn dual_thrust_lines(
    current_open: f64,
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
    k1: f64,
    k2: f64,
) -> Option<DualThrustLines> {
    if highs.is_empty() || lows.is_empty() || closes.is_empty() {
        return None;
    }
    let hh = highs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let hc = closes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lc = closes.iter().copied().fold(f64::INFINITY, f64::min);
    let ll = lows.iter().copied().fold(f64::INFINITY, f64::min);
    let range = (hh - lc).max(hc - ll);
    Some(DualThrustLines {
        current_open,
        buy_line: current_open + range * k1,
        sell_line: current_open - range * k2,
    })
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> StdResult<NaiveDate, Box<dyn Error>> {
    match env::var(name) {
        Ok(raw) => Ok(NaiveDate::parse_from_str(&raw, "%Y-%m-%d")?),
        Err(_) => Ok(NaiveDate::from_ymd_opt(default.0, default.1, default.2).ok_or("invalid default date")?),
    }
}

fn shanghai_range(
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StdResult<(DateTime<Utc>, DateTime<Utc>), Box<dyn Error>> {
    let tz = FixedOffset::east_opt(8 * 3600).ok_or("unable to build Asia/Shanghai offset")?;
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or("invalid start time")?)
        .single()
        .ok_or("ambiguous start time")?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or("invalid end time")?)
        .single()
        .ok_or("ambiguous end time")?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

fn main() {}

#[cfg(test)]
mod tests {
    use super::{dual_thrust_lines, DualThrustLines};

    #[test]
    fn calculates_dual_thrust_rails() {
        let lines: DualThrustLines = dual_thrust_lines(
            100.0,
            &[110.0, 108.0],
            &[107.0, 106.0],
            &[109.0, 105.0],
            0.2,
            0.2,
        )
        .expect("lines should exist");
        assert!((lines.buy_line - 101.0).abs() < 1e-9);
        assert!((lines.sell_line - 99.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 4: Run the example tests and verify the rail math passes**

Run:

```bash
cargo test --example dualthrust
```

Expected: PASS with `1 passed`.

- [ ] **Step 5: Replace the stub `main()` with the full replay example**

Update `main()` to this shape:

```rust
#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;
    let quote = session.quote(&symbol).await?;
    let daily_bars = session.kline(&symbol, DAILY_BAR, NDAY + 2).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut current_lines = None;
    let mut processed_daily_bar = None;
    while let Some(step) = session.step().await? {
        if step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            let rows = daily_bars.rows().await;
            if rows.len() >= NDAY + 1 {
                let current = rows.last().expect("rows checked");
                if current.state.is_opening() && processed_daily_bar != Some(current.kline.id) {
                    processed_daily_bar = Some(current.kline.id);
                    let history = &rows[rows.len() - (NDAY + 1)..rows.len() - 1];
                    let highs = history.iter().map(|row| row.kline.high).collect::<Vec<_>>();
                    let lows = history.iter().map(|row| row.kline.low).collect::<Vec<_>>();
                    let closes = history.iter().map(|row| row.kline.close).collect::<Vec<_>>();
                    current_lines = dual_thrust_lines(current.kline.open, &highs, &lows, &closes, K1, K2);
                    if let Some(lines) = current_lines {
                        println!(
                            "当前开盘价: {:.6}, 上轨: {:.6}, 下轨: {:.6}",
                            lines.current_open, lines.buy_line, lines.sell_line
                        );
                    }
                }
            }
        }

        if let Some(lines) = current_lines {
            if let Some(snapshot) = quote.snapshot().await {
                let last_price = snapshot.last_price;
                if last_price.is_finite() {
                    if last_price > lines.buy_line {
                        println!("高于上轨,目标持仓 多头3手");
                        task.set_target_volume(TARGET_VOLUME)?;
                    } else if last_price < lines.sell_line {
                        println!("低于下轨,目标持仓 空头3手");
                        task.set_target_volume(-TARGET_VOLUME)?;
                    } else if step.updated_handles.iter().any(|id| id == quote.id()) {
                        println!("未穿越上下轨,不调整持仓");
                    }
                }
            }
        }
    }

    task.cancel().await?;
    task.wait_finished().await?;
    session.finish().await?;
    println!("回测结束");
    Ok(())
}
```

- [ ] **Step 6: Run the new example end-to-end**

Run:

```bash
cargo run --example dualthrust
```

Expected: replay starts, prints rail recomputations, emits breakout messages when rails are crossed, and exits with `回测结束`.

- [ ] **Step 7: Commit the `dualthrust` migration**

Run:

```bash
git add examples/dualthrust.rs
git commit -m "feat: add replay dualthrust example"
```

Expected: one commit containing only the new Rust example.

### Task 4: Implement `examples/rbreaker.rs`

**Files:**
- Create: `examples/rbreaker.rs`
- Test: `examples/rbreaker.rs`

- [ ] **Step 1: Create the example file with a failing line-calculation test**

Write this initial file:

```rust
fn main() {}

#[cfg(test)]
mod tests {
    use super::{rbreaker_lines, RBreakerLines};

    #[test]
    fn calculates_rbreaker_lines() {
        let lines: RBreakerLines = rbreaker_lines(110.0, 90.0, 100.0);
        assert!((lines.pivot - 100.0).abs() < 1e-9);
        assert!((lines.b_break - 130.0).abs() < 1e-9);
        assert!((lines.s_break - 70.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run the new example test target and verify it fails**

Run:

```bash
cargo test --example rbreaker
```

Expected: FAIL with missing `RBreakerLines` / `rbreaker_lines`.

- [ ] **Step 3: Implement the pure R-Breaker helper and strategy state**

Replace the file contents with this helper layer and keep the tests:

```rust
use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const TARGET_VOLUME: i64 = 3;
const STOP_LOSS_PRICE: f64 = 10.0;
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const DEFAULT_SYMBOL: &str = "SHFE.au2006";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 3, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 5, 31);

#[derive(Debug, Clone, Copy)]
struct RBreakerLines {
    pivot: f64,
    b_break: f64,
    s_setup: f64,
    s_enter: f64,
    b_enter: f64,
    b_setup: f64,
    s_break: f64,
}

#[derive(Debug, Default)]
struct StrategyState {
    target_pos_value: i64,
    open_position_price: f64,
    lines: Option<RBreakerLines>,
}

fn rbreaker_lines(high: f64, low: f64, close: f64) -> RBreakerLines {
    let pivot = (high + low + close) / 3.0;
    RBreakerLines {
        pivot,
        b_break: high + 2.0 * (pivot - low),
        s_setup: pivot + (high - low),
        s_enter: 2.0 * pivot - low,
        b_enter: 2.0 * pivot - high,
        b_setup: pivot - (high - low),
        s_break: low - 2.0 * (high - pivot),
    }
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> StdResult<NaiveDate, Box<dyn Error>> {
    match env::var(name) {
        Ok(raw) => Ok(NaiveDate::parse_from_str(&raw, "%Y-%m-%d")?),
        Err(_) => Ok(NaiveDate::from_ymd_opt(default.0, default.1, default.2).ok_or("invalid default date")?),
    }
}

fn shanghai_range(
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StdResult<(DateTime<Utc>, DateTime<Utc>), Box<dyn Error>> {
    let tz = FixedOffset::east_opt(8 * 3600).ok_or("unable to build Asia/Shanghai offset")?;
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or("invalid start time")?)
        .single()
        .ok_or("ambiguous start time")?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or("invalid end time")?)
        .single()
        .ok_or("ambiguous end time")?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

fn main() {}

#[cfg(test)]
mod tests {
    use super::{rbreaker_lines, RBreakerLines};

    #[test]
    fn calculates_rbreaker_lines() {
        let lines: RBreakerLines = rbreaker_lines(110.0, 90.0, 100.0);
        assert!((lines.pivot - 100.0).abs() < 1e-9);
        assert!((lines.b_break - 130.0).abs() < 1e-9);
        assert!((lines.s_break - 70.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 4: Run the example tests and verify the line math passes**

Run:

```bash
cargo test --example rbreaker
```

Expected: PASS with `1 passed`.

- [ ] **Step 5: Replace the stub `main()` with the full replay example**

Update `main()` to this shape:

```rust
#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;
    let quote = session.quote(&symbol).await?;
    let daily_bars = session.kline(&symbol, DAILY_BAR, 3).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    let mut state = StrategyState::default();
    let mut processed_daily_bar = None;
    println!("策略开始运行");

    while let Some(step) = session.step().await? {
        if step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            let rows = daily_bars.rows().await;
            if rows.len() >= 2 {
                let current = rows.last().expect("rows checked");
                if current.state.is_opening() && processed_daily_bar != Some(current.kline.id) {
                    processed_daily_bar = Some(current.kline.id);
                    let previous = &rows[rows.len() - 2];
                    let lines = rbreaker_lines(previous.kline.high, previous.kline.low, previous.kline.close);
                    println!(
                        "已计算新标志线, 枢轴点: {:.6}, 突破买入价: {:.6}, 观察卖出价: {:.6}, 反转卖出价: {:.6}, 反转买入价: {:.6}, 观察买入价: {:.6}, 突破卖出价: {:.6}",
                        lines.pivot, lines.b_break, lines.s_setup, lines.s_enter, lines.b_enter, lines.b_setup, lines.s_break
                    );
                    state.lines = Some(lines);
                }
            }
        }

        let Some(lines) = state.lines else {
            continue;
        };
        let Some(snapshot) = quote.snapshot().await else {
            continue;
        };
        let last_price = snapshot.last_price;
        println!("最新价: {:.6}", last_price);

        if (state.target_pos_value > 0 && state.open_position_price - last_price >= STOP_LOSS_PRICE)
            || (state.target_pos_value < 0 && last_price - state.open_position_price >= STOP_LOSS_PRICE)
        {
            state.target_pos_value = 0;
        }

        if state.target_pos_value > 0 {
            if snapshot.highest > lines.s_setup && last_price < lines.s_enter {
                println!("多头持仓,当日内最高价超过观察卖出价后跌破反转卖出价: 反手做空");
                state.target_pos_value = -TARGET_VOLUME;
                state.open_position_price = last_price;
            }
        } else if state.target_pos_value < 0 {
            if snapshot.lowest < lines.b_setup && last_price > lines.b_enter {
                println!("空头持仓,当日最低价低于观察买入价后超过反转买入价: 反手做多");
                state.target_pos_value = TARGET_VOLUME;
                state.open_position_price = last_price;
            }
        } else if last_price > lines.b_break {
            println!("空仓,盘中价格超过突破买入价: 开仓做多");
            state.target_pos_value = TARGET_VOLUME;
            state.open_position_price = last_price;
        } else if last_price < lines.s_break {
            println!("空仓,盘中价格跌破突破卖出价: 开仓做空");
            state.target_pos_value = -TARGET_VOLUME;
            state.open_position_price = last_price;
        }

        task.set_target_volume(state.target_pos_value)?;
    }

    task.cancel().await?;
    task.wait_finished().await?;
    session.finish().await?;
    println!("回测结束");
    Ok(())
}
```

- [ ] **Step 6: Run the new example end-to-end**

Run:

```bash
cargo run --example rbreaker
```

Expected: replay starts, prints new R-Breaker line calculations, emits breakout/reversal messages as conditions are met, and exits with `回测结束`.

- [ ] **Step 7: Commit the `rbreaker` migration**

Run:

```bash
git add examples/rbreaker.rs
git commit -m "feat: add replay rbreaker example"
```

Expected: one commit containing only the new Rust example.

### Task 5: Update Example Documentation And Run Full Verification

**Files:**
- Modify: `README.md`
- Modify: `skills/tqsdk-rs/references/example-map.md`

- [ ] **Step 1: Add the three commands to the README example command list**

Update the command block in `README.md` to include:

```md
# 双均线回放策略
cargo run --example doublema

# Dual Thrust 回放策略
cargo run --example dualthrust

# R-Breaker 回放策略
cargo run --example rbreaker
```

- [ ] **Step 2: Add the three rows to the README example table**

Insert rows matching this structure:

```md
| `doublema.rs` | 基于 `ReplaySession` 的双均线交叉策略示例 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_TEST_SYMBOL`、`TQ_LOG_LEVEL` |
| `dualthrust.rs` | 基于 `ReplaySession` 的 Dual Thrust 突破策略示例 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_TEST_SYMBOL`、`TQ_LOG_LEVEL` |
| `rbreaker.rs` | 基于 `ReplaySession` 的 R-Breaker 反转策略示例 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_TEST_SYMBOL`、`TQ_LOG_LEVEL` |
```

- [ ] **Step 3: Teach the skill reference map about the new replay examples**

Update `skills/tqsdk-rs/references/example-map.md` replay section to include:

```md
- `examples/pivot_point.rs` 演示日线开盘触发的反转策略
- `examples/doublema.rs` 演示分钟 K 线上的双均线交叉策略
- `examples/dualthrust.rs` 演示日线阈值 + quote 突破触发的策略
- `examples/rbreaker.rs` 演示带止损和反转分支的日内策略状态机
```

- [ ] **Step 4: Run the full verification matrix for the batch**

Run:

```bash
cargo check --examples
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo run --example doublema
cargo run --example dualthrust
cargo run --example rbreaker
```

Expected:

- `cargo check --examples` succeeds
- `cargo test` succeeds
- `cargo clippy --all-targets --all-features -- -D warnings` succeeds
- each example exits cleanly after replay and prints strategy events

- [ ] **Step 5: Commit the documentation and verification-backed final batch**

Run:

```bash
git add README.md skills/tqsdk-rs/references/example-map.md examples/doublema.rs examples/dualthrust.rs examples/rbreaker.rs examples/python/doublema.py examples/python/dualthrust.py examples/python/rbreaker.py
git commit -m "feat: add replay strategy example migrations"
```

Expected: the final commit contains the finished example files plus README and skill-reference updates. If the earlier task-level commits already exist, this step can be skipped in favor of keeping the per-task commits.

## Self-Review

### Spec Coverage

- `doublema`, `dualthrust`, and `rbreaker` Rust examples are each covered by a dedicated implementation task.
- Python reference-file retention is covered by Task 1.
- README example discoverability is covered by Task 5.
- `skills/tqsdk-rs/references/example-map.md` sync is covered by Task 5.
- Replay/backtest canonical path and quote-feed matching are built into Tasks 2, 3, and 4.
- Example-local formula tests are built into Tasks 2, 3, and 4.
- Full verification is covered by Task 5.

No spec requirement is left without a task.

### Placeholder Scan

- No placeholder shortcuts remain in the plan body.
- Every code-changing task includes concrete code or shell content.
- Every verification step includes explicit commands and expected outcomes.

### Type Consistency

- `doublema` uses `cross_signal(closes, short, long, target) -> Option<i64>` consistently in tests and runtime logic.
- `dualthrust` uses `DualThrustLines` and `dual_thrust_lines(current_open, highs, lows, closes, k1, k2)` consistently in tests and runtime logic.
- `rbreaker` uses `RBreakerLines` and `rbreaker_lines(high, low, close)` consistently in tests and runtime logic.
- All runtime examples use the same canonical `ReplaySession` -> `session.quote(&symbol)` -> `runtime.account("TQSIM").target_pos(&symbol).build()` path.
