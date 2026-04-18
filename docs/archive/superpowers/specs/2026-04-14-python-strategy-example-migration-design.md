# Python Strategy Example Migration Design

## Summary

This spec defines the first batch of strategy example migrations from
`tqsdk-python/tqsdk/demo/example/` into `tqsdk-rs/examples/`.

The batch is intentionally limited to replay/backtest-friendly strategies that
map cleanly onto the current Rust public surface:

- `doublema.py` -> `examples/doublema.rs`
- `dualthrust.py` -> `examples/dualthrust.rs`
- `rbreaker.py` -> `examples/rbreaker.rs`

The goal is not to invent more idiomatic Rust strategies. The goal is to make
the Rust examples feel familiar to users coming from `tqsdk-python`, while
keeping all example entrypoints on the canonical Rust path:

- `Client::create_backtest_session(...)`
- `ReplaySession`
- `runtime.account(...).target_pos(...).build()`

## Goals

- Add three representative strategy examples covering trend, breakout, and
  reversal styles.
- Keep strategy logic aligned with the corresponding Python demos.
- Keep all new examples replay/backtest-friendly by default.
- Make the examples readable as public API documentation, not internal tests or
  framework demos.
- Update README example listings so the new examples are discoverable.

## Non-Goals

- No live-trading-first examples in this batch.
- No migration of intraday close-time controlled strategies such as
  `fairy_four_price.py` in this batch.
- No new public helper modules or compatibility facades.
- No attempt to reproduce every Python logging side effect from the SDK runtime.
  Alignment is defined at the strategy event level, not by byte-for-byte log
  identity.

## Source Material

The migration sources are:

- `tqsdk-python/tqsdk/demo/example/doublema.py`
- `tqsdk-python/tqsdk/demo/example/dualthrust.py`
- `tqsdk-python/tqsdk/demo/example/rbreaker.py`

Each migrated Rust example will keep the corresponding Python source in
`examples/python/` as a nearby reference, following the existing
`examples/python/pivot_point.py` pattern.

## Why These Three

This batch is selected for coverage, not just implementation convenience.

- `doublema` provides the simplest trend-following pattern and gives Rust users
  an obvious equivalent to the Python "new bar -> recompute indicator ->
  rebalance target position" workflow.
- `dualthrust` covers breakout logic driven by daily threshold calculation plus
  intraday quote updates.
- `rbreaker` covers a more stateful reversal strategy with stop loss, breakout,
  and reversal branches in the same example.

Together, the batch demonstrates the main strategy shapes users expect from the
Python examples without dragging in more complex time-of-day shutdown behavior.

## Public API Constraints

All examples in this batch must stay within the current canonical API described
by the repository rules:

- Use `Client` / `ClientBuilder` as the public entrypoint.
- Use `ReplaySession` as the only public replay/backtest entrypoint.
- Use `runtime.account("...").target_pos("...").build()` for target position
  tasks.
- Do not introduce `compat::` facades, callback/channel plumbing, or deprecated
  entrypoints.

These examples are public surface. They must not teach users old APIs.

## Shared Example Structure

Each Rust example will remain a single standalone file in `examples/`, with
four predictable sections:

1. Strategy constants and environment parsing
2. Pure indicator/threshold calculation helpers
3. Explicit strategy state
4. `main` with replay setup and the step loop

This keeps the examples easy to read and avoids hidden helper modules that users
would have to chase across the repository.

## Replay Execution Model

All three examples will use replay/backtest execution by default.

Shared setup pattern:

1. Read auth and optional example env vars.
2. Build `Client` with `EndpointConfig::from_env()`.
3. Create `ReplaySession` from a configurable replay date range.
4. Register the series handles needed by the strategy.
5. Register `session.quote(&symbol)` and keep the handle alive even when the
   strategy is primarily driven by bar updates.
6. Create runtime and target position task via
   `runtime.account(ACCOUNT_KEY).target_pos(&symbol).build()`.
7. Advance replay with `while let Some(step) = session.step().await? { ... }`.
8. Cancel the task, wait for it to finish, then `session.finish().await?`.

The explicit quote registration is required even for strategies whose signals
are computed from bars. Without an active replay quote feed, `TargetPosTask`
can fail to progress between signal evaluations. This was already observed and
corrected in `examples/pivot_point.rs`.

## Alignment Rule With Python Examples

Alignment is defined as:

- identical default symbols, thresholds, and target volumes where the Rust API
  can support them
- equivalent decision order
- equivalent state transitions
- equivalent strategy event sequence

The most important output to align is:

- when indicators or thresholds are recomputed
- when a long/short/flat target is requested
- whether the final position returns to the expected state

Exact runtime logs from Python's own simulator are not part of the contract.
Rust examples may omit those SDK-emitted details as long as the strategy-level
behavior matches.

## Example 1: `doublema.rs`

### Strategy Shape

Trend-following moving-average cross strategy.

### Data Inputs

- one symbol
- minute K-line serial
- replay quote feed kept alive for order matching

### Default Behavior

- short window: `30`
- long window: `60`
- target volume on bullish cross: `+3`
- target volume on bearish cross: `-3`

### Decision Flow

On each new closed minute bar:

1. recompute short and long moving averages
2. compare previous-bar relationship and latest-bar relationship
3. if the short MA crosses above the long MA, set target volume to `+3`
4. if the long MA crosses above the short MA, set target volume to `-3`

No extra filtering is added in Rust. The point is to preserve the Python demo's
teaching value.

### Rust Implementation Notes

- Keep MA calculation in a pure helper over close prices.
- Trigger only on new bar progression, not every replay step.
- Print simple event messages analogous to the Python demo.

## Example 2: `dualthrust.rs`

### Strategy Shape

Breakout strategy using daily thresholds and intraday price triggers.

### Data Inputs

- one symbol
- daily K-line serial to compute the rails
- replay quote feed for intraday breakout checks

### Default Behavior

- lookback days: `5`
- `K1 = 0.2`
- `K2 = 0.2`
- breakout above upper rail: target `+3`
- breakout below lower rail: target `-3`

### Decision Flow

1. Compute upper and lower rails from the latest daily context.
2. Recompute rails when a new trading day appears or the current day's open is
   refreshed.
3. On quote updates:
   - above upper rail -> long target
   - below lower rail -> short target
   - otherwise print that no adjustment is made

### Rust Implementation Notes

- The rail calculation stays in a pure helper.
- Day-boundary recomputation remains explicit in the example rather than hidden
  behind a framework helper.
- Strategy output should show the current open and the two rails, matching the
  Python tutorial style.

## Example 3: `rbreaker.rs`

### Strategy Shape

Stateful reversal strategy combining stop loss, breakout entries, and reversal
entries.

### Data Inputs

- one symbol
- daily K-line serial for the seven R-Breaker levels
- replay quote feed for intraday state transitions

### Default Behavior

- stop-loss points: `10`
- target volumes: `+3`, `0`, `-3`

### Decision Flow

Maintain explicit strategy state:

- `target_pos_value`
- `open_position_price`
- latest R-Breaker levels

On each relevant replay update:

1. recompute the seven levels on new daily bar progression
2. on quote updates:
   - apply stop-loss rule first
   - if holding long, allow long-to-short reversal
   - if holding short, allow short-to-long reversal
   - if flat, allow breakout long/short entry

The branch order matters and must follow the Python demo so the strategy does
not drift semantically.

### Rust Implementation Notes

- Keep the seven-level calculation in a pure helper returning a typed struct.
- Keep strategy state local to the example rather than inferred from runtime
  snapshots.
- Print branch-specific messages using the same conceptual wording as Python.

## Reference Python Files In `examples/python/`

This batch adds the following reference files:

- `examples/python/doublema.py`
- `examples/python/dualthrust.py`
- `examples/python/rbreaker.py`

These files are not the canonical Rust API. They exist so future maintainers
can compare Rust behavior against the Python originals without leaving the
repository.

## README Changes

`README.md` will be updated in the examples section to:

- list the three new strategy examples
- label them as replay/backtest-oriented strategy demos
- document their required env vars, primarily
  `TQ_AUTH_USER`, `TQ_AUTH_PASS`, and optional replay configuration

The README text should not imply that these are live-trading examples.

## Verification Plan

Because these are example migrations, verification must cover both compile-time
health and behavioral alignment.

Minimum verification for this batch:

- `cargo check --examples`
- `cargo test`
- targeted `cargo run --example doublema`
- targeted `cargo run --example dualthrust`
- targeted `cargo run --example rbreaker`

Behavior checks:

- each example must produce strategy event output without runtime errors
- each example must reach replay completion cleanly
- each example must issue target-position changes consistent with the Python
  source logic

Example-local tests:

- `doublema`: test MA-cross predicate on a minimal close-price sequence
- `dualthrust`: test upper/lower rail calculation on fixed daily inputs
- `rbreaker`: test the seven-line R-Breaker formula on fixed daily inputs

The tests are not intended to prove full strategy profitability. They are there
to pin the migration contract and prevent silent drift away from the Python
examples.

## Risks

### Replay Feed Semantics

Strategies that appear bar-driven may still need quote registration for order
matching and runtime task progress. Forgetting this creates false success where
signals print but trades do not complete.

### Day-Boundary Semantics

Python demos often rely on `api.is_changing(...)` semantics that are slightly
different from a naive Rust replay loop. Each migrated example must preserve the
same decision cadence instead of just reusing a generic helper.

### Output Drift

It is easy to keep the strategy "roughly similar" while changing branch order or
state updates. This spec explicitly rejects that. The migration target is the
Python example behavior, not a new Rust reinterpretation.

## Out of Scope Follow-Up

After this batch lands, the next reasonable batch is the time-of-day controlled
strategy set, starting with `fairy_four_price.py`. That work is intentionally
deferred so this batch can stay focused on replay-friendly strategy shapes.
