# Python Backtest Capability Alignment Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this roadmap task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the highest-value capability gaps between Rust `ReplaySession` and Python `TqBacktest`/`TqReplay` without regressing the current explicit `Client -> ReplaySession -> step() -> finish()` public contract.

**Architecture:** Keep the existing Rust replay design: explicit session owner, typed historical source, `ReplayKernel` time merge, `QuoteSynthesizer`, and `SimBroker`. Promote Python-only helper semantics such as continuous contract day switches and future auxiliary timeline data into typed providers, and split stock/corporate-action replay, time-driven replay, and reporting into separate follow-on projects instead of contaminating the current futures-first core.

**Tech Stack:** Rust 2024, `tokio`, `chrono`, existing `src/replay/`, `src/runtime/`, `src/client/market.rs`, `src/download/`, `src/series/`

---

## Scope Split

This capability gap is too large for one safe implementation batch. Split it into four tracks:

1. **Track A: Futures parity hardening**
   Continuous contract day-switch support in production replay, plus auxiliary replay timeline plumbing.
2. **Track B: Stock replay foundation**
   Corporate actions, stock quote fields, stock account/order/position model, and `TqSimStock`-like semantics.
3. **Track C: Time-driven replay**
   A Rust equivalent of Python `TqReplay`: single-day, speed-controlled, time-driven delivery.
4. **Track D: Report metrics**
   Raw-result post-processing, benchmark metrics, and optional report rendering.

Recommendation: implement **Track A first**, because it improves Python parity for the current futures/commodity-option scope without forcing a public-type explosion.

## Current State Snapshot

- Already covered in production:
  - explicit replay session entry
  - one-step market-time advancement
  - kline open/close two-phase visibility
  - tick vs kline quote driver priority
  - OHLC price path for bar-based matching
  - futures / commodity-option local matching
  - day settlement logs
  - runtime / `TargetPosTask` integration
- Present only in tests or design notes:
  - continuous contract provider
- Not present in replay production path:
  - stock dividend / split timeline injection
  - stock T+1 simulator
  - time-driven replay mode
  - `TqReport`-style metrics

## File Map

### Track A: Futures parity hardening

- Modify: `src/replay/providers.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/feed.rs`
- Modify: `src/replay/session.rs`
- Modify: `src/replay/runtime.rs`
- Modify: `src/client/market.rs`
- Test: `src/replay/tests.rs`
- Docs: `README.md`
- Docs: `docs/architecture.md`
- Docs: `AGENTS.md`
- Docs: `CLAUDE.md`

### Track B: Stock replay foundation

- Modify: `src/replay/feed.rs`
- Modify: `src/replay/session.rs`
- Modify: `src/replay/runtime.rs`
- Modify: `src/replay/sim.rs`
- Modify: `src/replay/types.rs`
- Modify: `src/types/market.rs`
- Modify: `src/types/trading.rs`
- Modify: `src/runtime/market.rs`
- Modify: `src/runtime/execution.rs`
- Test: `src/replay/tests.rs`
- Test: `src/runtime/tests.rs`
- Docs: `README.md`
- Docs: `docs/architecture.md`
- Docs: `docs/migration-remove-legacy-compat.md`
- Docs: `AGENTS.md`
- Docs: `CLAUDE.md`

### Track C: Time-driven replay

- Create: `src/replay/replay_day.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/feed.rs`
- Modify: `src/replay/session.rs`
- Test: `src/replay/tests.rs`
- Docs: `README.md`
- Docs: `docs/architecture.md`

### Track D: Report metrics

- Create: `src/replay/report.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/types.rs`
- Test: `src/replay/tests.rs`
- Docs: `README.md`

## Track A Plan

### Task 1: Productionize Continuous Contract Provider

**Files:**
- Modify: `src/replay/providers.rs`
- Modify: `src/replay/mod.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Keep the existing provider API, but remove the `#[cfg(test)]` wall**

Move the provider out of test-only compilation so replay production code can depend on it:

```rust
#[derive(Debug, Clone)]
pub struct ContinuousMapping {
    pub trading_day: NaiveDate,
    pub symbol: String,
    pub underlying_symbol: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContinuousContractProvider {
    days: BTreeMap<NaiveDate, HashMap<String, String>>,
}
```

- [ ] **Step 2: Add a session-level failing test for day-switching `underlying_symbol`**

Add a replay test that:

```rust
// pseudo-shape
let mut session = ReplaySession::from_source(...).await?;
let _quote = session.quote("KQ.m@SHFE.rb").await?;
let runtime = session.runtime(["TQSIM"]).await?;

let day1 = session.step().await?.unwrap();
let q1 = runtime.account("TQSIM")?.runtime().market().latest_quote("KQ.m@SHFE.rb").await?;
assert_eq!(q1.underlying_symbol, "SHFE.rb2605");

while let Some(_step) = session.step().await? {}

let result = session.finish().await?;
// during day 2 the visible underlying must become SHFE.rb2610
```

- [ ] **Step 3: Run the replay test and verify it fails for the right reason**

Run: `cargo test replay_session_continuous_contract_switches_underlying_symbol -- --nocapture`

Expected: FAIL because production replay never updates continuous mappings across trading days.

- [ ] **Step 4: Wire provider exposure into replay production modules**

Export the provider from `src/replay/mod.rs`:

```rust
pub use providers::{ContinuousContractProvider, ContinuousMapping};
```

and keep `ReplaySession` free of Python callback-style plumbing.

- [ ] **Step 5: Commit**

```bash
git add src/replay/providers.rs src/replay/mod.rs src/replay/tests.rs
git commit -m "feat: productionize replay continuous contract provider"
```

### Task 2: Add Auxiliary Timeline Support To HistoricalSource

**Files:**
- Modify: `src/replay/feed.rs`
- Modify: `src/client/market.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Extend the historical source contract with replay auxiliary data**

Use a typed auxiliary event instead of stuffing day-switch metadata into quote snapshots:

```rust
#[derive(Debug, Clone)]
pub enum ReplayAuxiliaryEvent {
    ContinuousMapping {
        trading_day: NaiveDate,
        symbol: String,
        underlying_symbol: String,
    },
}

#[async_trait]
pub(crate) trait HistoricalSource: Send + Sync {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata>;
    async fn load_klines(... ) -> Result<Vec<Kline>>;
    async fn load_ticks(... ) -> Result<Vec<Tick>>;
    async fn load_auxiliary_events(
        &self,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<ReplayAuxiliaryEvent>>;
}
```

- [ ] **Step 2: Add a failing test for empty vs populated auxiliary timelines**

Add one test where `FakeHistoricalSource` returns a `ContinuousMapping` for day 2 and assert that replay exposes a metadata patch only when the timeline event is reached.

- [ ] **Step 3: Run the focused test**

Run: `cargo test replay_auxiliary_timeline_applies_only_after_boundary -- --nocapture`

Expected: FAIL because `HistoricalSource` currently has no auxiliary event channel.

- [ ] **Step 4: Implement a no-op default for the SDK source first**

In `src/client/market.rs`, return an empty vector until a real upstream historical continuous-mapping fetch path exists:

```rust
async fn load_auxiliary_events(
    &self,
    _start_dt: DateTime<Utc>,
    _end_dt: DateTime<Utc>,
) -> Result<Vec<ReplayAuxiliaryEvent>> {
    Ok(Vec::new())
}
```

This keeps the interface stable while making the production data-source gap explicit.

- [ ] **Step 5: Commit**

```bash
git add src/replay/feed.rs src/client/market.rs src/replay/tests.rs
git commit -m "refactor: add replay auxiliary timeline contract"
```

### Task 3: Apply Continuous Mapping Patches On Trading-Day Transition

**Files:**
- Modify: `src/replay/session.rs`
- Modify: `src/replay/runtime.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: Add market-state metadata patch support**

Add a minimal metadata patch method instead of rebuilding all symbols:

```rust
impl ReplayMarketState {
    pub(crate) async fn patch_underlying_symbol(&self, symbol: &str, underlying_symbol: String) {
        if let Some(metadata) = self
            .metadata
            .write()
            .expect("replay market metadata lock poisoned")
            .get_mut(symbol)
        {
            metadata.underlying_symbol = underlying_symbol;
        }
    }
}
```

- [ ] **Step 2: Load auxiliary events once during session creation**

Store ordered auxiliary events inside `ReplaySession`, indexed by trading day:

```rust
pending_auxiliary: BTreeMap<NaiveDate, Vec<ReplayAuxiliaryEvent>>,
```

- [ ] **Step 3: Apply same-day auxiliary events when the active trading day is established or changed**

Inside `advance_trading_day_clock`, after switching `active_trading_day`, apply all `ContinuousMapping` patches for that day before publishing the next visible quote.

- [ ] **Step 4: Verify runtime-facing quotes now reflect the new `underlying_symbol`**

Run: `cargo test replay_session_continuous_contract_switches_underlying_symbol -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/replay/session.rs src/replay/runtime.rs src/replay/tests.rs
git commit -m "feat: apply replay continuous contract patches on trading day switch"
```

### Task 4: Fill The Production Data-Source Gap For Historical Continuous Mappings

**Files:**
- Modify: `src/client/market.rs`
- Modify: `src/ins/services.rs`
- Modify: `src/ins/query.rs`
- Test: `src/replay/tests.rs`
- Docs: `README.md`

- [ ] **Step 1: Confirm the upstream source**

Before coding, verify whether the upstream Shinnytech APIs already expose historical continuous mappings. Do not guess. If no production source exists, stop this task here and keep Track A limited to injectable providers plus tests.

- [ ] **Step 2: If a source exists, add a single-purpose fetcher**

Preferred shape:

```rust
pub async fn query_cont_mapping_history(
    &self,
    start_dt: DateTime<Utc>,
    end_dt: DateTime<Utc>,
) -> Result<Vec<ContinuousMapping>>
```

- [ ] **Step 3: Feed those rows into `SdkHistoricalSource::load_auxiliary_events`**

Convert upstream rows into:

```rust
ReplayAuxiliaryEvent::ContinuousMapping { ... }
```

- [ ] **Step 4: Add an integration test guarded behind fake source coverage**

Do not add a live test by default. Keep the production path covered by unit tests using the fake source.

- [ ] **Step 5: Commit**

```bash
git add src/client/market.rs src/ins/services.rs src/ins/query.rs README.md src/replay/tests.rs
git commit -m "feat: load historical continuous mappings for replay"
```

### Task 5: Update Canonical Docs For Track A

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update replay capability bullets**

Document that production replay now supports:

```text
- futures / commodity option replay
- implicit quote driver
- day settlement
- continuous contract underlying_symbol day switch
```

- [ ] **Step 2: Keep explicit non-goals explicit**

Document that replay still does **not** yet cover:

```text
- stock replay / T+1 stock simulator
- stock dividend timeline injection
- time-driven TqReplay equivalent
- built-in report metrics
```

- [ ] **Step 3: Run doc consistency review**

Check:

```bash
rg -n "ReplaySession|continuous|dividend|TqReplay|report" README.md docs/architecture.md AGENTS.md CLAUDE.md
```

- [ ] **Step 4: Commit**

```bash
git add README.md docs/architecture.md AGENTS.md CLAUDE.md
git commit -m "docs: clarify replay capability boundaries and continuous mapping support"
```

## Track B Plan

This is a separate sub-project. Do **not** batch it into Track A.

### Deliverables

- stock-specific replay market model
- corporate action provider
- `cash_dividend_ratio` / `stock_dividend_ratio` public surface decision
- stock account / order / position behavior
- T+1 / no-offset semantics

### Why It Must Be Separate

- `Quote` currently does not expose corporate-action fields.
- `SimBroker` explicitly supports only futures and commodity options.
- current runtime execution model assumes futures-style offsets and positions.

### Required Pre-Plan Decisions

- whether to extend existing public `Quote` or add a stock-only replay quote shape
- whether stock replay shares `SimBroker` or introduces `SimBrokerStock`
- whether replay stock execution reuses current `Account` / `Position` / `Order` / `Trade` types or adds stock-specific public types

## Track C Plan

This is a separate sub-project. Do **not** batch it into Track A.

### Deliverables

- one-day time-driven replay session
- speed control
- pause / resume
- server-faithful kline/tick update ordering

### Suggested Public Shape

Prefer a separate public type instead of overloading `ReplaySession`:

```rust
pub struct ReplayDaySession { ... }
```

This keeps current step-driven backtest semantics stable.

## Track D Plan

This is a separate sub-project. Do **not** batch it into Track A.

### Deliverables

- replay result metrics from raw settlements / trades
- win rate / PnL ratio / return / annualized return / drawdown / Sharpe / Sortino
- no GUI dependency in v1

### Suggested Shape

```rust
pub struct ReplayReport { ... }

impl ReplayReport {
    pub fn from_result(result: &BacktestResult) -> Self { ... }
}
```

## Acceptance Matrix

Track A is done when:

- `ReplaySession` still passes existing replay tests.
- continuous contract day-switching is covered by a new unit test.
- no live market state is activated by `create_backtest_session`.
- runtime and replay quote snapshots remain explicit and step-driven.
- docs state the exact current replay boundary.

Track B is done when:

- stock replay works without futures offsets.
- corporate actions are visible only on or after the correct historical boundary.
- no future dividend information leaks at session init.

Track C is done when:

- the new public replay type is time-driven instead of step-driven.
- a user can slow down or speed up one replay day without changing strategy code.

Track D is done when:

- metrics derive only from raw replay result data.
- no report logic is required for replay correctness.

## Self-Review

- Spec coverage:
  - futures replay parity gaps: covered by Track A
  - continuous mapping: covered by Track A
  - stock dividend and stock replay: isolated into Track B
  - `TqReplay` time-driven replay: isolated into Track C
  - `TqReport` metrics: isolated into Track D
- Placeholder scan:
  - one real blocker remains: production historical continuous-mapping upstream source is not present in current codebase; the roadmap marks this explicitly instead of hand-waving it
- Type consistency:
  - Track A preserves existing `ReplaySession` / `ReplayStep` / `BacktestResult`
  - Track B and Track C are explicitly split to avoid accidental mutation of the current futures-first contract
