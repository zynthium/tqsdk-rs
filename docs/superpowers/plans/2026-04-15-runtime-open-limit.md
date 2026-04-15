# Runtime `open_limit` Enforcement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `TqRuntime` / `TargetPosTask` reject target-position plans that would exceed the exchange daily `Quote.open_limit`, using true same-account same-symbol same-trading-day consumed volume.

**Architecture:** Keep the policy in runtime planning. Extend the crate-internal `ExecutionAdapter` to expose per-symbol trades, add one runtime-side trading-day helper derived from `quote.datetime`, compute the remaining daily budget before planning, and return a dedicated runtime error instead of silently truncating the plan.

**Tech Stack:** Rust 2024, `tokio`, `chrono`, existing runtime execution adapters, existing runtime/replay test suites

---

## File Map

- Modify: `src/runtime/execution.rs`
  Add a crate-internal `trades_for_symbol(account_key, symbol)` adapter hook and implement it for live execution.
- Modify: `src/runtime/mod.rs`
  Re-export the new test-only planner budget type so `src/runtime/tasks/tests.rs` can compile cleanly.
- Modify: `src/runtime/tasks/mod.rs`
  Re-export the budget type from `target_pos` for test-only runtime imports.
- Modify: `src/replay/runtime.rs`
  Implement the new execution-adapter hook for replay.
- Modify: `src/runtime/errors.rs`
  Add a dedicated `RuntimeError::OpenLimitExceeded`.
- Modify: `src/runtime/tasks/target_pos.rs`
  Add trading-day parsing helpers, daily used-volume calculation, budget enforcement, and planner wiring.
- Modify: `src/runtime/tasks/tests.rs`
  Add failing tests first for planning/budget behavior and extend `FakeExecutionAdapter`.
- Modify: `README.md`
  Mention runtime target-pos enforcement of `Quote.open_limit`.
- Modify: `skills/tqsdk-rs/references/market-and-series.md`
  Keep agent knowledge aligned with runtime behavior.

### Task 1: Write Failing Runtime Tests

**Files:**
- Modify: `src/runtime/tasks/tests.rs`
- Test: `src/runtime/tasks/tests.rs`

- [ ] **Step 1: Add a helper to build dated trades for planner tests**

```rust
fn shanghai_nanos(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap()
}

fn make_trade_at(
    order_id: &str,
    trade_id: &str,
    volume: i64,
    price: f64,
    trade_date_time: i64,
) -> Trade {
    Trade {
        seqno: 0,
        user_id: "SIM".to_string(),
        trade_id: trade_id.to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2601".to_string(),
        order_id: order_id.to_string(),
        exchange_trade_id: String::new(),
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        volume,
        price,
        trade_date_time,
        commission: 0.0,
        epoch: None,
    }
}
```

- [ ] **Step 2: Add a failing test that `open_limit == 0` keeps planning unchanged**

```rust
#[test]
fn planning_ignores_open_limit_when_quote_does_not_provide_it() {
    let priorities = parse_offset_priority("开").expect("priority should parse");
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        open_limit: 0,
        ..Quote::default()
    };
    let position = make_position("SHFE", "rb2601");

    let plan = compute_plan(&quote, &position, 2, &priorities, None)
        .expect("missing open_limit should not block planning");

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].orders[0].offset, PlannedOffset::Open);
    assert_eq!(plan[0].orders[0].volume, 2);
}
```

- [ ] **Step 3: Add a failing test that planning rejects requests above the remaining daily budget**

```rust
#[test]
fn planning_rejects_when_requested_volume_exceeds_remaining_open_limit() {
    let priorities = parse_offset_priority("开").expect("priority should parse");
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        open_limit: 10,
        ..Quote::default()
    };
    let position = make_position("SHFE", "rb2601");

    let err = compute_plan(&quote, &position, 4, &priorities, Some(OpenLimitBudget {
        open_limit: 10,
        used_volume: 8,
        remaining_limit: 2,
    }))
    .expect_err("plan should be rejected once it exceeds remaining daily limit");

    assert!(matches!(
        err,
        RuntimeError::OpenLimitExceeded {
            open_limit: 10,
            used_volume: 8,
            remaining_limit: 2,
            requested_plan_volume: 4,
            ..
        }
    ));
}
```

- [ ] **Step 4: Add a failing async test that previous trading-day trades do not consume today’s budget**

```rust
#[tokio::test]
async fn target_pos_uses_only_current_trading_day_trades_for_open_limit() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-15 09:00:00.000000".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        open_limit: 5,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        true,
    ));
    execution
        .set_symbol_trades(vec![
            make_trade_at("old-order", "old-trade", 4, 100.0, shanghai_nanos(2026, 4, 14, 9, 1, 0)),
        ])
        .await;
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution.clone()));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account.target_pos("SHFE.rb2601").build().expect("task should build");

    task.set_target_volume(1).expect("target should be accepted");
    task.wait_target_reached().await.expect("prior-day trades must not consume today's budget");

    assert_eq!(execution.inserted_prices().await, vec![101.0]);
}
```

- [ ] **Step 5: Add a failing async test that same-day trades do consume the budget and fail the task**

```rust
#[tokio::test]
async fn target_pos_fails_when_same_day_trades_exhaust_open_limit() {
    let market = Arc::new(FakeMarketAdapter::new(Quote {
        instrument_id: "rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        datetime: "2026-04-15 09:00:00.000000".to_string(),
        ask_price1: 101.0,
        bid_price1: 100.0,
        last_price: 100.0,
        open_limit: 5,
        ..Quote::default()
    }));
    let execution = Arc::new(FakeExecutionAdapter::with_position(
        make_position("SHFE", "rb2601"),
        false,
    ));
    execution
        .set_symbol_trades(vec![
            make_trade_at("same-day-order", "same-day-trade", 5, 100.0, shanghai_nanos(2026, 4, 15, 9, 1, 0)),
        ])
        .await;
    let runtime = Arc::new(TqRuntime::with_id("runtime-1", RuntimeMode::Live, market, execution));
    let account = runtime.account("SIM").expect("account should exist");
    let task = account.target_pos("SHFE.rb2601").build().expect("task should build");

    task.set_target_volume(1).expect("target should be accepted");

    let err = task
        .wait_target_reached()
        .await
        .expect_err("same-day trades should exhaust the daily open_limit budget");

    assert!(matches!(err, RuntimeError::TargetTaskFailed { .. }));
    assert!(err.to_string().contains("open_limit"));
}
```

- [ ] **Step 6: Run the focused tests to verify they fail for the right reason**

Run:

```bash
cargo test planning_ignores_open_limit_when_quote_does_not_provide_it planning_rejects_when_requested_volume_exceeds_remaining_open_limit target_pos_uses_only_current_trading_day_trades_for_open_limit target_pos_fails_when_same_day_trades_exhaust_open_limit
```

Expected: compile/test failure because `compute_plan` does not accept the budget argument yet, `OpenLimitBudget` / `RuntimeError::OpenLimitExceeded` do not exist, and `FakeExecutionAdapter` does not expose symbol trades.

- [ ] **Step 7: Commit the red-state tests**

```bash
git add src/runtime/tasks/tests.rs
git commit -m "test: cover runtime open_limit planning"
```

### Task 2: Extend Execution Adapters With Per-Symbol Trade Reads

**Files:**
- Modify: `src/runtime/execution.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/tasks/mod.rs`
- Modify: `src/replay/runtime.rs`
- Modify: `src/runtime/tasks/tests.rs`
- Test: `src/runtime/tasks/tests.rs`

- [ ] **Step 1: Add the new trait method to `ExecutionAdapter`**

```rust
async fn trades_for_symbol(&self, _account_key: &str, _symbol: &str) -> RuntimeResult<Vec<Trade>> {
    Err(RuntimeError::Unsupported("trades_for_symbol"))
}
```

- [ ] **Step 2: Implement live filtering by `exchange_id + instrument_id`**

```rust
async fn trades_for_symbol(&self, account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>> {
    let session = self.session(account_key).await?;
    let trades = session.get_trades().await?;
    let (exchange_id, instrument_id) = symbol
        .split_once('.')
        .ok_or(RuntimeError::Unsupported("invalid runtime symbol"))?;

    Ok(trades
        .into_values()
        .filter(|trade| trade.exchange_id == exchange_id && trade.instrument_id == instrument_id)
        .collect())
}
```

- [ ] **Step 3: Re-export the new budget type through `tasks/mod.rs`**

```rust
#[cfg(test)]
pub(crate) use target_pos::{OpenLimitBudget, compute_plan, parse_offset_priority, validate_quote_constraints};
```

- [ ] **Step 4: Re-export the new budget type for runtime tests**

```rust
#[cfg(test)]
pub(crate) use tasks::{
    OpenLimitBudget, ChildOrderRunner, OffsetAction, PlannedOffset, compute_plan, parse_offset_priority,
    validate_quote_constraints,
};
```

- [ ] **Step 5: Implement replay filtering through broker `trades_for_symbol`**

```rust
async fn trades_for_symbol(&self, account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>> {
    Ok(self
        .state
        .broker
        .lock()
        .expect("replay broker lock poisoned")
        .trades_for_symbol(symbol)
        .into_iter()
        .filter(|trade| trade.user_id == account_key)
        .collect())
}
```

- [ ] **Step 6: Extend `FakeExecutionAdapter` state and helpers**

```rust
struct FakeExecutionState {
    next_order_seq: usize,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    symbol_trades: Vec<Trade>,
    inserted_prices: Vec<f64>,
    cancelled_order_ids: Vec<String>,
    position: Position,
    autofill_on_insert: bool,
}

async fn set_symbol_trades(&self, trades: Vec<Trade>) {
    self.state.lock().await.symbol_trades = trades;
}

async fn trades_for_symbol(&self, _account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>> {
    let (exchange_id, instrument_id) = symbol.split_once('.').unwrap_or_default();
    Ok(self
        .state
        .lock()
        .await
        .symbol_trades
        .iter()
        .filter(|trade| trade.exchange_id == exchange_id && trade.instrument_id == instrument_id)
        .cloned()
        .collect())
}
```

- [ ] **Step 7: Run the focused tests to verify adapter work compiles and remaining failures are planner-only**

Run:

```bash
cargo test target_pos_uses_only_current_trading_day_trades_for_open_limit target_pos_fails_when_same_day_trades_exhaust_open_limit
```

Expected: tests still fail, but now due to missing trading-day / planner budget enforcement rather than missing adapter methods.

- [ ] **Step 8: Commit the adapter slice**

```bash
git add src/runtime/execution.rs src/runtime/mod.rs src/runtime/tasks/mod.rs src/replay/runtime.rs src/runtime/tasks/tests.rs
git commit -m "feat: expose runtime trades by symbol"
```

### Task 3: Add Trading-Day Budget Enforcement To Target-Pos Planning

**Files:**
- Modify: `src/runtime/errors.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tasks/tests.rs`
- Test: `src/runtime/tasks/tests.rs`

- [ ] **Step 1: Add a dedicated runtime error**

```rust
#[error(
    "交易所规定 {symbol} 当日开平仓手数限额为 {open_limit}，已使用 {used_volume}，剩余 {remaining_limit}，本次规划需要 {requested_plan_volume}，TargetPosTask 不会静默截断"
)]
OpenLimitExceeded {
    symbol: String,
    open_limit: i64,
    used_volume: i64,
    remaining_limit: i64,
    requested_plan_volume: i64,
},
```

- [ ] **Step 2: Add runtime-side helper types and trading-day parsing**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenLimitBudget {
    pub open_limit: i64,
    pub used_volume: i64,
    pub remaining_limit: i64,
}

const DAY_NANOS: i64 = 86_400_000_000_000;
const EIGHTEEN_HOURS_NS: i64 = 64_800_000_000_000;
const BEGIN_MARK_NANOS: i64 = 631_123_200_000_000_000;

fn quote_trading_day(quote: &Quote) -> RuntimeResult<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(&quote.datetime, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| RuntimeError::Unsupported("invalid target_pos quote datetime"))?;
    let cst = chrono::FixedOffset::east_opt(8 * 3600)
        .ok_or(RuntimeError::Unsupported("invalid CST offset"))?;
    let timestamp = cst
        .from_local_datetime(&naive)
        .single()
        .ok_or(RuntimeError::Unsupported("invalid target_pos quote datetime"))?
        .timestamp_nanos_opt()
        .ok_or(RuntimeError::Unsupported("target_pos quote datetime out of range"))?;
    Ok(trading_day_from_timestamp(timestamp))
}

fn trading_day_from_timestamp(timestamp: i64) -> i64 {
    let mut days = (timestamp - BEGIN_MARK_NANOS) / DAY_NANOS;
    if (timestamp - BEGIN_MARK_NANOS) % DAY_NANOS >= EIGHTEEN_HOURS_NS {
        days += 1;
    }
    let week_day = days % 7;
    if week_day >= 5 {
        days += 7 - week_day;
    }
    BEGIN_MARK_NANOS + days * DAY_NANOS
}
```

- [ ] **Step 3: Add helpers for same-day consumed volume and total planned volume**

```rust
fn used_volume_for_trading_day(quote: &Quote, trades: &[Trade]) -> RuntimeResult<i64> {
    let trading_day = quote_trading_day(quote)?;
    Ok(trades
        .iter()
        .filter(|trade| trading_day_from_timestamp(trade.trade_date_time) == trading_day)
        .map(|trade| trade.volume)
        .sum())
}

fn total_planned_volume(batches: &[PlannedBatch]) -> i64 {
    batches
        .iter()
        .flat_map(|batch| batch.orders.iter())
        .map(|order| order.volume)
        .sum()
}
```

- [ ] **Step 4: Extend `compute_plan` to accept an optional budget and reject over-limit plans**

```rust
pub(crate) fn compute_plan(
    quote: &Quote,
    position: &Position,
    target_volume: i64,
    offset_priority: &[Vec<OffsetAction>],
    open_limit_budget: Option<OpenLimitBudget>,
) -> RuntimeResult<Vec<PlannedBatch>> {
    validate_quote_constraints(quote)?;

    let mut remaining = target_volume - net_position(position);
    if remaining == 0 {
        return Ok(Vec::new());
    }

    let mut batches = Vec::new();
    // existing batch-building logic unchanged

    if let Some(budget) = open_limit_budget {
        let requested_plan_volume = total_planned_volume(&batches);
        if requested_plan_volume > budget.remaining_limit {
            return Err(RuntimeError::OpenLimitExceeded {
                symbol: quote.instrument_id.clone(),
                open_limit: budget.open_limit,
                used_volume: budget.used_volume,
                remaining_limit: budget.remaining_limit,
                requested_plan_volume,
            });
        }
    }

    Ok(batches)
}
```

- [ ] **Step 5: Wire `TargetPosTask::drive_target()` to compute the budget before planning**

```rust
let trades = runtime
    .execution()
    .trades_for_symbol(self.account.account_key(), &self.symbol)
    .await?;
let open_limit_budget = if quote.open_limit > 0 {
    let used_volume = used_volume_for_trading_day(&quote, &trades)?;
    Some(OpenLimitBudget {
        open_limit: quote.open_limit,
        used_volume,
        remaining_limit: (quote.open_limit - used_volume).max(0),
    })
} else {
    None
};
let plan = compute_plan(&quote, &position, request.volume, &self.offset_priority, open_limit_budget)?;
```

- [ ] **Step 6: Update old planner tests to pass `None` for the new budget argument**

```rust
let plan = compute_plan(&quote, &position, 2, &priorities, None).expect("plan should succeed");
```

- [ ] **Step 7: Run the focused tests to verify green**

Run:

```bash
cargo test planning_ignores_open_limit_when_quote_does_not_provide_it planning_rejects_when_requested_volume_exceeds_remaining_open_limit target_pos_uses_only_current_trading_day_trades_for_open_limit target_pos_fails_when_same_day_trades_exhaust_open_limit
```

Expected: PASS

- [ ] **Step 8: Commit the planner slice**

```bash
git add src/runtime/errors.rs src/runtime/tasks/target_pos.rs src/runtime/tasks/tests.rs
git commit -m "feat: enforce runtime open_limit budget"
```

### Task 4: Sync Docs And Run Full Verification

**Files:**
- Modify: `README.md`
- Modify: `skills/tqsdk-rs/references/market-and-series.md`

- [ ] **Step 1: Add a short README note for runtime behavior**

```md
`TargetPosTask` 现在会在 `Quote.open_limit > 0` 时校验交易所当日 `开仓 + 平仓` 手数限额。
它会按账户、合约、交易日统计已成交手数；如果剩余额度不足，本次目标仓位规划会显式报错，而不会静默截断。
```

- [ ] **Step 2: Add the same rule to the skill reference**

```md
当用户问 target-pos / runtime 时，要说明：
- `Quote.open_limit > 0` 时，`TargetPosTask` 会按账户 + 合约 + 交易日统计已成交手数
- 若剩余额度不足，会报错，不会自动缩单
```

- [ ] **Step 3: Run formatting and the full verification matrix**

Run:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Expected:

- `cargo fmt --check`: exit 0
- `cargo test`: all existing tests pass, only pre-existing ignored tests remain ignored
- `cargo clippy --all-targets --all-features -- -D warnings`: exit 0

- [ ] **Step 4: Commit the docs and verification slice**

```bash
git add README.md skills/tqsdk-rs/references/market-and-series.md
git commit -m "docs: describe runtime open_limit enforcement"
```

- [ ] **Step 5: Capture the final review diff**

Run:

```bash
git status --short
git log --oneline -5
```

Expected: only the intended task commits are present and the worktree is clean or contains only unrelated pre-existing changes.
