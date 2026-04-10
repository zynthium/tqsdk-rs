# TargetPos Runtime Implementation Plan

> Historical plan note:
> This file records an early runtime build-out plan from April 2026.
> It may reference interim compatibility layers and adapter types that have since been removed or renamed, including `compat::TargetPosTask` and `BacktestExecutionAdapter`.
> For the current public runtime surface, prefer `README.md`, `docs/architecture.md`, and `docs/migration-remove-legacy-compat.md`.

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a unified runtime for `tqsdk-rs` and implement a Rust-native + Python-compatible `TargetPosTask`, with a clear path to `TargetPosScheduler` and future backtest execution.

**Architecture:** Add a new `runtime/` layer that sits above the existing WebSocket actor and `DataManager` stack, with explicit `MarketAdapter`, `ExecutionAdapter`, `TaskRegistry`, and task engines. Keep `Client` and `TradeSession` working, but gradually demote them to facades over the new runtime.

**Tech Stack:** Rust 2024, `tokio`, `yawc`, `async-channel`, `serde_json`, existing `DataManager`, existing WebSocket actor model, `tracing`

---

Reference spec: `docs/superpowers/specs/2026-04-09-target-pos-runtime-design.md`

## Chunk 1: Runtime Skeleton

### Task 1: Add the runtime surface, shared types, and exports

**Files:**
- Create: `src/runtime/mod.rs`
- Create: `src/runtime/types.rs`
- Create: `src/runtime/errors.rs`
- Create: `src/runtime/tests.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`

- [x] **Step 1: Write the failing test**

```rust
use crate::runtime::{OffsetPriority, PriceMode, TargetPosConfig};

#[test]
fn target_pos_config_default_uses_active_price_mode() {
    let cfg = TargetPosConfig::default();
    assert!(matches!(cfg.price_mode, PriceMode::Active));
    assert!(matches!(
        cfg.offset_priority,
        OffsetPriority::TodayYesterdayThenOpenWait
    ));
    assert!(cfg.split_policy.is_none());
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test target_pos_config_default_uses_active_price_mode --lib`

Expected: FAIL with unresolved imports for `crate::runtime::*`

- [x] **Step 3: Write the minimal implementation**

```rust
pub enum PriceMode {
    Active,
    Passive,
    Custom(std::sync::Arc<dyn Fn(String, &crate::Quote) -> crate::Result<f64> + Send + Sync>),
}

pub enum OffsetPriority {
    TodayYesterdayThenOpenWait,
    TodayYesterdayThenOpen,
    YesterdayThenOpen,
    OpenOnly,
}

pub struct VolumeSplitPolicy {
    pub min_volume: i64,
    pub max_volume: i64,
}

pub struct TargetPosConfig {
    pub price_mode: PriceMode,
    pub offset_priority: OffsetPriority,
    pub split_policy: Option<VolumeSplitPolicy>,
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test target_pos_config_default_uses_active_price_mode --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/mod.rs src/runtime/types.rs src/runtime/errors.rs src/runtime/tests.rs src/lib.rs src/prelude.rs
git commit -m "feat: add runtime surface types"
```

### Task 2: Add `TaskRegistry` for task uniqueness and order ownership

**Files:**
- Create: `src/runtime/registry.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn task_registry_reuses_same_key_same_config() {
    // same account + symbol + same config -> same logical task id
}

#[test]
fn task_registry_rejects_same_key_different_config() {
    // same account + symbol + different config -> error
}

#[test]
fn task_registry_tracks_order_owner() {
    // register order_id -> task_id and verify lookup
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test task_registry_ --lib`

Expected: FAIL because `TaskRegistry` and tests are not implemented

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct TaskRegistry {
    // key: runtime_id + account_key + symbol
    // value: task metadata + config fingerprint
}

impl TaskRegistry {
    pub fn register_target_task(...) -> Result<RegisteredTask> { ... }
    pub fn unregister_task(...) { ... }
    pub fn bind_order_owner(&self, order_id: &str, task_id: TaskId) { ... }
    pub fn order_owner(&self, order_id: &str) -> Option<TaskId> { ... }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test task_registry_ --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/registry.rs src/runtime/mod.rs src/runtime/tests.rs
git commit -m "feat: add runtime task registry"
```

### Task 3: Add `TqRuntime`, `AccountHandle`, and the live execution bridge

**Files:**
- Create: `src/runtime/core.rs`
- Create: `src/runtime/account.rs`
- Create: `src/runtime/engine.rs`
- Create: `src/runtime/market.rs`
- Create: `src/runtime/execution.rs`
- Create: `src/runtime/modes/mod.rs`
- Create: `src/runtime/modes/live.rs`
- Modify: `src/client/builder.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/tests.rs`

- [x] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn runtime_exposes_account_handle_for_registered_account() {
    // construct a runtime with a fake live adapter and assert account() returns a handle
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test runtime_exposes_account_handle_for_registered_account --lib`

Expected: FAIL because `TqRuntime` and `AccountHandle` do not exist

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct TqRuntime {
    dm: Arc<DataManager>,
    registry: Arc<TaskRegistry>,
    market: Arc<dyn MarketAdapter>,
    execution: Arc<dyn ExecutionAdapter>,
}

pub struct AccountHandle {
    runtime: Arc<TqRuntime>,
    account_key: String,
}
```

- [x] **Step 4: Add builder/facade wiring**

Run: make `ClientBuilder::build_runtime()` and `Client::into_runtime()` or equivalent compile

Expected: `cargo test` compiles the new entry points without touching networked tests

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test runtime_exposes_account_handle_for_registered_account --lib`

Expected: PASS

- [x] **Step 6: Commit**

```bash
git add src/runtime/core.rs src/runtime/account.rs src/runtime/engine.rs src/runtime/market.rs src/runtime/execution.rs src/runtime/modes/mod.rs src/runtime/modes/live.rs src/runtime/mod.rs src/runtime/tests.rs src/client/builder.rs src/client/facade.rs src/client/mod.rs src/lib.rs
git commit -m "feat: add runtime core and live bridge"
```

## Chunk 2: TargetPosTask V1

### Task 4: Extend quote/instrument data for official `TargetPosTask` constraints

**Files:**
- Modify: `src/types/market.rs`
- Modify: `src/client/market.rs`
- Modify: `src/ins/parse.rs`
- Modify: `src/ins/tests.rs`

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn quote_parses_open_min_order_volume_fields() {
    // deserialize quote JSON containing open_min_market_order_volume and open_min_limit_order_volume
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test quote_parses_open_min_order_volume_fields --lib`

Expected: FAIL because the fields are missing from `Quote`

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct Quote {
    pub open_min_market_order_volume: i32,
    pub open_min_limit_order_volume: i32,
    pub open_max_market_order_volume: i32,
    pub open_max_limit_order_volume: i32,
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test quote_parses_open_min_order_volume_fields --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/types/market.rs src/client/market.rs src/ins/parse.rs src/ins/tests.rs
git commit -m "feat: expose quote open order volume fields"
```

### Task 5: Implement `TargetPosTask` config parsing and planning primitives

**Files:**
- Create: `src/runtime/tasks/mod.rs`
- Create: `src/runtime/tasks/common.rs`
- Create: `src/runtime/tasks/target_pos.rs`
- Create: `src/runtime/tasks/tests.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/account.rs`

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn offset_priority_parser_accepts_python_compatible_forms() {
    // "今昨,开", "今昨开", "昨开", "开"
}

#[test]
fn planning_rejects_contracts_with_open_min_volume_gt_one() {
    // official compatibility rule
}

#[test]
fn planning_respects_shfe_close_today_vs_close_yesterday() {
    // BUY/SELL with SHFE/INE vs non-SHFE behavior
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test offset_priority_parser_accepts_python_compatible_forms --lib`

Expected: FAIL because task parser/planner does not exist

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct TargetPosBuilder { ... }
pub struct TargetPosHandle { ... }

fn parse_offset_priority(raw: &str) -> Result<Vec<Vec<OffsetAction>>> { ... }
fn validate_quote_constraints(quote: &Quote) -> Result<()> { ... }
fn compute_plan(...) -> Vec<PlannedOrder> { ... }
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test planning_ --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/tasks/mod.rs src/runtime/tasks/common.rs src/runtime/tasks/target_pos.rs src/runtime/tasks/tests.rs src/runtime/mod.rs src/runtime/account.rs
git commit -m "feat: add target position planning primitives"
```

### Task 6: Implement child order runner, repricing, and completion semantics

**Files:**
- Modify: `src/runtime/tasks/common.rs`
- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tasks/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn child_order_runner_reprices_when_market_turns_worse() {
    // BUY order should cancel and resubmit if the active price increases
}

#[tokio::test]
async fn child_order_runner_waits_for_trade_aggregation_after_finished_order() {
    // FINISHED is not enough; trade aggregation must be visible
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test child_order_runner_ --lib`

Expected: FAIL because repricing/completion logic is not implemented

- [x] **Step 3: Write the minimal implementation**

```rust
struct ChildOrderRunner { ... }

impl ChildOrderRunner {
    async fn run_until_all_traded(&self) -> Result<()> { ... }
    async fn monitor_price_and_cancel(&self, order_id: &str, order_price: f64) -> Result<()> { ... }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test child_order_runner_ --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/tasks/common.rs src/runtime/tasks/target_pos.rs src/runtime/tasks/tests.rs
git commit -m "feat: add target position child order runner"
```

### Task 7: Implement task lifecycle, conflict enforcement, and account-facing APIs

**Files:**
- Modify: `src/runtime/engine.rs`
- Modify: `src/runtime/account.rs`
- Modify: `src/runtime/registry.rs`
- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tests.rs`
- Modify: `src/runtime/tasks/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn target_pos_handle_latest_target_overrides_previous_target() {
    // set -1 then 1 and verify the latest target is authoritative
}

#[tokio::test]
async fn manual_insert_order_is_blocked_while_target_task_owns_symbol() {
    // default manual path should reject
}

#[tokio::test]
async fn target_pos_cancel_transitions_to_finished_after_owned_orders_are_closed() {
    // cancel -> cancelling -> finished
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test target_pos_handle_ --lib`

Expected: FAIL because lifecycle/conflict rules are incomplete

- [x] **Step 3: Write the minimal implementation**

```rust
impl TargetPosHandle {
    pub fn set_target_volume(&self, volume: i64) -> Result<()> { ... }
    pub async fn cancel(&self) -> Result<()> { ... }
    pub async fn wait_finished(&self) -> Result<()> { ... }
    pub async fn wait_target_reached(&self) -> Result<()> { ... }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test target_pos_handle_ --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/engine.rs src/runtime/account.rs src/runtime/registry.rs src/runtime/tasks/target_pos.rs src/runtime/tests.rs src/runtime/tasks/tests.rs
git commit -m "feat: finalize target position lifecycle and conflicts"
```

## Chunk 3: Compat Layer And Scheduler

### Task 8: Add Python-compatible `TargetPosTask` facade and public exports

**Files:**
- Create: `src/compat/mod.rs`
- Create: `src/compat/target_pos_task.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `README.md`

- [x] **Step 1: Write the failing compile test**

```rust
#[tokio::test]
async fn compat_target_pos_task_wraps_runtime_handle() {
    // TargetPosTask::new(...).await should compile and delegate to runtime
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test compat_target_pos_task_wraps_runtime_handle --lib`

Expected: FAIL because `compat::TargetPosTask` does not exist

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct TargetPosTask {
    inner: TargetPosHandle,
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test compat_target_pos_task_wraps_runtime_handle --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/compat/mod.rs src/compat/target_pos_task.rs src/lib.rs src/prelude.rs README.md
git commit -m "feat: add compat target position facade"
```

### Task 9: Implement `TargetPosScheduler`

**Files:**
- Create: `src/runtime/tasks/scheduler.rs`
- Create: `src/compat/target_pos_scheduler.rs`
- Modify: `src/runtime/tasks/mod.rs`
- Modify: `src/compat/mod.rs`
- Modify: `src/runtime/tasks/tests.rs`
- Modify: `README.md`

- [x] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn scheduler_runs_deadline_bounded_steps_then_finishes_last_step_on_target() {
    // mirrors Python scheduler semantics
}

#[tokio::test]
async fn scheduler_collects_trade_records_into_execution_report() {
    // aggregate trade stream across steps
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test scheduler_ --lib`

Expected: FAIL because scheduler does not exist

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct TargetPosScheduler { ... }

impl TargetPosScheduler {
    pub async fn run(&self) -> Result<()> { ... }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test scheduler_ --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/tasks/scheduler.rs src/compat/target_pos_scheduler.rs src/runtime/tasks/mod.rs src/compat/mod.rs src/runtime/tasks/tests.rs README.md
git commit -m "feat: add target position scheduler"
```

## Chunk 4: Backtest Execution

### Task 10: Add `BacktestExecutionAdapter` and runtime mode integration

**Files:**
- Create: `src/runtime/modes/backtest.rs`
- Modify: `src/runtime/modes/mod.rs`
- Modify: `src/runtime/core.rs`
- Modify: `src/client/market.rs`
- Modify: `src/backtest/core.rs`
- Modify: `src/runtime/tests.rs`

- [x] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn runtime_can_be_constructed_with_backtest_execution_mode() {
    // build runtime in backtest mode without live execution adapter
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test runtime_can_be_constructed_with_backtest_execution_mode --lib`

Expected: FAIL because backtest execution mode is not wired

- [x] **Step 3: Write the minimal implementation**

```rust
pub struct BacktestExecutionAdapter { ... }
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test runtime_can_be_constructed_with_backtest_execution_mode --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/modes/backtest.rs src/runtime/modes/mod.rs src/runtime/core.rs src/client/market.rs src/backtest/core.rs src/runtime/tests.rs
git commit -m "feat: add backtest execution adapter skeleton"
```

### Task 11: Enable `TargetPosTask` and scheduler under backtest execution

**Files:**
- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tasks/scheduler.rs`
- Modify: `src/runtime/tasks/tests.rs`
- Modify: `examples/backtest.rs`
- Modify: `README.md`

- [x] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn target_pos_task_runs_under_backtest_execution() {
    // set target volume and observe simulated execution
}

#[tokio::test]
async fn scheduler_runs_under_backtest_execution() {
    // deadline table executes with backtest market time
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test backtest_execution --lib`

Expected: FAIL because task execution is not connected to backtest mode

- [x] **Step 3: Write the minimal implementation**

```rust
// Reuse the same task logic; switch only the execution adapter and market time source.
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test backtest_execution --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/tasks/target_pos.rs src/runtime/tasks/scheduler.rs src/runtime/tasks/tests.rs examples/backtest.rs README.md
git commit -m "feat: enable target position tasks in backtest mode"
```

## Chunk 5: Final Validation And Documentation Sync

### Task 12: Run the full verification matrix and sync docs/examples

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `examples/trade.rs`
- Modify: `examples/backtest.rs`

- [x] **Step 1: Run formatting check**

Run: `cargo fmt --check`

Expected: PASS

- [x] **Step 2: Run unit and integration tests**

Run: `cargo test`

Expected: PASS

- [x] **Step 3: Run strict lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS

- [x] **Step 4: Verify examples compile**

Run: `cargo check --examples`

Expected: PASS

- [x] **Step 5: Update public docs to match shipped APIs**

Run: verify every README/example snippet compiles against the new exports

Expected: no stale `TradeSession`-only guidance where runtime is now the preferred path

- [x] **Step 6: Commit**

```bash
git add README.md docs/architecture.md examples/trade.rs examples/backtest.rs
git commit -m "docs: document target position runtime"
```
