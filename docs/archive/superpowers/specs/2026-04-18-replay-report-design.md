# Replay Report Design

## Context

`ReplaySession::finish()` already returns a raw [`BacktestResult`](../../../src/replay/types.rs) containing daily settlements, final account/position snapshots, and the full trade list. That raw result is sufficient for correctness, but it does not yet provide the headline statistics that Python users expect from `TqReport`.

Track D should add replay result metrics without changing replay execution semantics, without introducing GUI dependencies, and without expanding the current step-driven replay contract into a larger analytics framework.

The immediate goal is a small, explicit, post-processing API that computes Python-aligned headline metrics from `BacktestResult`.

## Scope

In scope:

- a new public `ReplayReport` type
- a new replay report module under `src/replay/`
- post-processing metrics derived from `BacktestResult`
- Python-first semantic alignment for the chosen headline metrics
- unit tests using fixed Rust fixtures
- golden-style semantic tests that encode Python-compatible expectations
- README / architecture / skill-reference documentation for the new report API

Out of scope:

- GUI rendering
- web charts
- a Rust equivalent of Python `draw_report()`
- time-driven replay work
- stock replay / dividend timeline work
- a configurable analytics builder or plugin framework
- per-symbol, per-strategy, or per-trade visualization layers

## Requirements

1. Reporting must be a pure post-processing step over `BacktestResult`.
2. `BacktestResult` must remain the raw fact container and must not be mutated or overloaded with derived metrics.
3. The public entrypoint must be `ReplayReport::from_result(&BacktestResult)`.
4. v1 must expose only headline metrics plus a small number of basic counts.
5. Metrics with insufficient or mathematically unstable samples must use `Option<f64>`, returning `None` instead of `NaN` or `0.0`.
6. `winning_rate` and `profit_loss_ratio` must be based on completed round trips, not raw trades or trading days.
7. Where Python `TqReport` semantics differ from more generic finance formulas, v1 must prefer Python compatibility.
8. v1 must not introduce any GUI or plotting dependency.

## Options Considered

### Option A: Independent post-processing report type

```rust
pub struct ReplayReport { ... }

impl ReplayReport {
    pub fn from_result(result: &BacktestResult) -> Self;
}
```

Pros:

- keeps replay correctness and analytics clearly separated
- preserves `BacktestResult` as a stable raw result type
- easy to test from fixed fixtures
- easy to document as an optional follow-on step

Cons:

- one extra API call after `finish()`

### Option B: Put derived metrics directly on `BacktestResult`

Examples:

- compute metrics during `finish()`
- add `result.report()` and/or embed report fields into `BacktestResult`

Pros:

- shorter calling pattern

Cons:

- couples replay result facts with analytics policy
- makes future formula changes more invasive
- increases the chance that analytics concerns leak into replay core code

### Option C: General analytics builder

Examples:

- `ReplayAnalyticsBuilder`
- configurable metric subsets
- pluggable outputs / exporters

Pros:

- flexible long term

Cons:

- clearly over-scoped for the current gap
- larger public surface
- unnecessary abstraction before the Python-parity metric set is stable

## Decision

Adopt Option A.

Track D v1 will add a single, explicit post-processing type: `ReplayReport`. Users will continue to call `ReplaySession::finish()` to obtain raw results, and may then derive headline metrics with `ReplayReport::from_result(&result)`.

This preserves the existing replay contract and keeps analytics logic outside `ReplaySession` and `SimBroker`.

## Public API

Add a new replay module file:

- `src/replay/report.rs`

Export a new public type from `src/replay/mod.rs`:

```rust
pub use report::ReplayReport;
```

Suggested public shape for v1:

```rust
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
    pub fn from_result(result: &BacktestResult) -> Self;
}
```

Public API non-goals for v1:

- no `BacktestResult::report()` shortcut
- no chart payload
- no daily equity/time-series export in the public struct
- no per-symbol breakdowns

## Internal Structure

Keep v1 implementation in a single new module file, `src/replay/report.rs`, with small internal helper functions instead of additional submodules.

The module should have three internal layers:

1. Raw input access
   - reads `BacktestResult`
   - never mutates input

2. Derived series / pairing helpers
   - derive daily account-equity observations from `settlements`
   - derive completed round trips from `trades`

3. Metric calculators
   - one helper per metric family
   - returns `Option<f64>` where samples are insufficient or unstable

This structure is intentionally small for v1. If formulas stabilize and the module grows, it can later be split into helper files without changing the public surface.

## Metric Groups

### Basic counts

- `trade_count`
  - source: `result.trades.len()`
- `trading_day_count`
  - source: `result.settlements.len()`

These are always defined and do not use `Option`.

### Round-trip metrics

These metrics are computed from completed round trips:

- `winning_rate`
- `profit_loss_ratio`

Definition of sample:

- a sample is a completed open/close round trip
- not a single fill
- not a trading day aggregate

The round-trip pairing logic must prefer Python `TqReport` semantics over generic assumptions.

### Equity-series metrics

These metrics are computed from the daily settlement/equity sequence:

- `return_rate`
- `annualized_return`
- `max_drawdown`
- `sharpe_ratio`
- `sortino_ratio`

The report should derive these from settlement-account snapshots rather than re-simulating account state from raw trades.

## Sample-Insufficiency Rules

Use `Option<f64>` for all floating metrics.

Return `None` when:

- there are no completed round trips for `winning_rate` / `profit_loss_ratio`
- the equity sequence does not provide enough information for the metric
- the denominator or variance term needed by the metric is undefined or unstable

Do not return:

- `NaN`
- `inf`
- forced `0.0` placeholders

This keeps the public semantics explicit and easier to consume safely.

## Python-Compatibility Rule

Track D must align its metric meaning with Python `TqReport` semantics.

Compatibility policy:

- metric names and conceptual meaning should match Python
- completed-round-trip semantics should match Python
- if a Python formula differs from a textbook finance formula, prefer Python
- tests should encode the chosen semantics explicitly so future refactors do not silently drift

This does not require byte-for-byte implementation similarity with Python source, but it does require result semantics to track Python behavior for the supported metric set.

## Data Flow

```text
ReplaySession::finish()
  -> BacktestResult
  -> ReplayReport::from_result(&result)
       -> derive trade_count / trading_day_count
       -> derive completed round trips from trades
       -> derive daily settlement equity sequence
       -> compute Python-aligned headline metrics
       -> ReplayReport
```

## Testing Strategy

Tests live in `src/replay/tests.rs`.

### Fixture-driven metric tests

Add deterministic unit tests for:

1. empty `BacktestResult`
2. settlements without trades
3. one completed profitable round trip
4. one completed losing round trip
5. multiple round trips with mixed wins and losses
6. monotonic equity growth
7. equity drawdown sequence

### Python-semantic golden tests

For a small number of fixed fixtures:

- encode expected metric values that reflect Python `TqReport` semantics
- keep them as ordinary Rust unit tests
- do not require a live Python runtime during normal test runs

The important part is that the test names and comments clearly explain which Python-compatible rule is being checked.

## Files Expected To Change

- `src/replay/report.rs`
- `src/replay/mod.rs`
- `src/replay/tests.rs`
- `README.md`
- `docs/architecture.md`
- `skills/tqsdk-rs/references/simulation-and-backtest.md`
- `AGENTS.md` / `CLAUDE.md` are not required in v1 unless the implementation changes canonical replay entrypoint guidance; the current design keeps `ReplaySession` as the replay entrypoint and adds only a post-processing report type

## Risks

- round-trip pairing semantics may drift from Python if the pairing rule is underspecified
- annualized and risk-adjusted metrics can be surprisingly sensitive to short sample windows
- overly broad v1 scope could turn a small post-processing layer into a large analytics subsystem
- exposing too much intermediate data in the public struct would make future cleanup harder

## Acceptance Criteria

- `ReplayReport` is publicly constructible via `ReplayReport::from_result(&BacktestResult)`
- `BacktestResult` remains a raw result container with no embedded metric policy
- v1 exposes only headline metrics plus basic counts
- unstable metrics use `Option<f64>`
- `winning_rate` and `profit_loss_ratio` are based on completed round trips
- tests encode Python-first semantics for the supported metrics
- no GUI or plotting dependency is introduced
