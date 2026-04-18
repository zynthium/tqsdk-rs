# Runtime `open_limit` Enforcement Design

## Context

TqSdk 3.9.3 adds `Quote.open_limit`, defined as the exchange-imposed daily limit on the total number of lots for `OPEN + CLOSE` trades on a contract. The repository already exposes this field on `Quote`, but `TqRuntime` and `TargetPosTask` do not consume it yet.

The goal of this change is to make runtime target-position planning respect that exchange rule without expanding the public runtime entrypoints or silently changing planning semantics.

## Scope

In scope:

- `TargetPosTask` / `compute_plan()` enforcement for `Quote.open_limit`
- live and replay execution adapters providing per-symbol trade snapshots
- runtime-side trading-day calculation based on `quote.datetime`
- runtime errors and tests
- minimal README / skill reference sync for the new runtime behavior

Out of scope:

- changing manual order APIs
- changing `TargetPosTask` to auto-truncate plans
- introducing new public facades
- changing exchange/order routing logic outside target-position planning

## Requirements

1. Runtime only enforces `open_limit` when `quote.open_limit > 0`.
2. Enforcement must use the true account-level daily consumed volume for the same contract, not just the current task's local history.
3. Consumed volume means the sum of same-account, same-symbol trade volumes for the current trading day, regardless of direction or offset.
4. Planning must fail explicitly when the remaining limit cannot cover the newly planned lots.
5. Runtime must not silently truncate the plan to fit the remaining limit.
6. Trading-day boundaries must follow the existing China futures trading-day semantics already used in replay/scheduler logic.

## Options Considered

### Option A: Runtime planner queries per-symbol trades and computes the budget

Add an execution-adapter method that returns trades for one account and one symbol. `TargetPosTask` computes the current trading day from `quote.datetime`, filters trades to that day, sums consumed volume, computes remaining capacity, and passes that budget into planning.

Pros:

- keeps the exchange rule centralized in runtime planning
- works for both live and replay
- counts trades from manual orders, other tasks, and current-task history
- does not require new public APIs

Cons:

- adds one more adapter read before each replan

### Option B: Execution adapters return already-computed used quota

Each execution adapter would own the trading-day rule and expose a daily used-volume helper.

Pros:

- thinner target-position planner

Cons:

- duplicates trading-day semantics across live/replay
- makes future behavior changes harder to reason about

### Option C: TargetPosTask tracks only its own trade stream

The task subscribes to trade events and keeps a local consumed counter.

Pros:

- low per-replan read cost

Cons:

- misses trades that happened before task startup
- misses manual orders and other runtime tasks
- does not satisfy the account-level requirement

## Decision

Adopt Option A.

`TargetPosTask` remains the policy layer. Execution adapters expose raw trade snapshots by account and symbol; runtime computes the current trading-day budget and rejects plans that exceed the remaining exchange limit.

## Design

### Execution Adapter

Add:

```rust
async fn trades_for_symbol(&self, account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>>;
```

Behavior:

- live adapter: read `TradeSession::get_trades()` and filter by `exchange_id + instrument_id`
- replay adapter: use broker-side per-symbol trade snapshots already maintained by replay sim state
- test fakes: return deterministic vectors for planner tests

This stays crate-internal and does not change the public runtime API surface.

### Trading-Day Helper

Add a runtime-side helper that converts `quote.datetime` (`%Y-%m-%d %H:%M:%S%.f`, CST) into the exchange trading day used by replay/scheduler.

The helper should live in runtime code rather than live/replay adapters so the rule stays in one place.

### Planning Contract

Extend the planning flow as follows:

1. `TargetPosTask::drive_target()` fetches the latest `Quote` and current `Position`.
2. It loads same-account, same-symbol trades from the execution adapter.
3. It computes `used_volume_for_trading_day`.
4. It computes `remaining_open_limit = quote.open_limit - used_volume_for_trading_day`.
5. It passes that remaining budget into planning.
6. `compute_plan()` returns an explicit error if the newly planned total volume exceeds the remaining budget.

Planning semantics:

- if `quote.open_limit <= 0`, treat as "no usable limit info", so planning proceeds unchanged
- if the plan volume is within remaining capacity, planning proceeds unchanged
- if the plan volume exceeds remaining capacity, return an error

### Error Handling

Add a dedicated runtime error:

- `RuntimeError::OpenLimitExceeded`

Suggested payload:

- `symbol`
- `open_limit`
- `used_volume`
- `remaining_limit`
- `requested_plan_volume`

The error message should clearly say that the exchange daily `OPEN + CLOSE` limit has been exhausted or is insufficient, and that `TargetPosTask` does not silently truncate target planning.

### Why Error Instead Of Truncation

`TargetPosTask` means "drive position toward the requested target." Silent truncation would create a misleading state where the task keeps running or appears partially successful but can never reach the requested target. An explicit failure is easier to observe, test, and recover from.

## Data Flow

```text
latest quote
  -> trading-day helper
  -> trades_for_symbol(account, symbol)
  -> filter same trading day
  -> sum trade.volume
  -> remaining_open_limit
  -> compute_plan(...)
  -> either plan batches or return OpenLimitExceeded
```

## Test Plan

Add regression tests for:

1. `quote.open_limit == 0` does not block planning.
2. planning succeeds when `used_today + requested_plan_volume <= open_limit`.
3. planning fails with `OpenLimitExceeded` when `used_today + requested_plan_volume > open_limit`.
4. previous trading-day trades do not consume the new trading-day budget.
5. replay/live adapter filtering returns only same-symbol trades.

## Files Expected To Change

- `src/runtime/execution.rs`
- `src/runtime/errors.rs`
- `src/runtime/tasks/target_pos.rs`
- `src/runtime/tasks/tests.rs`
- possibly a small runtime helper module or `src/runtime/tasks/target_pos.rs` helper section
- `README.md`
- `skills/tqsdk-rs/references/market-and-series.md`

## Risks

- incorrect trading-day conversion would overcount or undercount used volume around night sessions
- symbol matching in live trades must align with `exchange_id + instrument_id` rather than a synthetic symbol field that may be absent
- explicit failure changes runtime behavior for users who previously ignored exchange limits, so the error message must be specific

## Acceptance Criteria

- runtime target-position planning rejects over-limit requests with a dedicated error
- same-day trades from the same account and symbol are counted regardless of source
- previous trading-day trades do not consume the current day budget
- all existing tests remain green
- README / skill docs mention runtime enforcement behavior
