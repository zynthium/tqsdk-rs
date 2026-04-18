# Replay Backtest Implementation Plan

> Historical plan note:
> This file records an early replay/backtest implementation plan from April 2026.
> It may reference transitional surfaces that are no longer current, including `BacktestHandle`, `BacktestExecutionAdapter`, and `compat::` facades.
> For current repository truth, prefer `README.md`, `docs/architecture.md`, and `docs/migration-remove-legacy-compat.md`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a new Rust-native replay backtest subsystem centered on `ReplaySession`, preserving the Python backtest semantics that affect strategy results while dropping Python’s `wait_update` / diff / chart transport scaffolding.

**Architecture:** Add a new `src/replay/` subsystem with four layers: `HistoricalSource` for time-bounded data loading, `ReplayKernel` for one-market-timestamp-at-a-time stepping, `QuoteSynthesizer` + `SeriesStore` for public market state, and `SimBroker` + runtime adapters for local matching and settlement. Reuse the existing runtime task logic by refactoring `MarketAdapter` away from `DataManager`, then attach `TargetPosTask` to replay through typed adapters rather than through the legacy `BacktestHandle`.

**Tech Stack:** Rust 2024, `tokio`, `chrono`, `serde`, existing `SeriesAPI` / `InsAPI`, existing runtime task layer, `VecDeque`, `BTreeMap`, `uuid`

---

Reference spec: `docs/superpowers/specs/2026-04-09-replay-backtest-design.md`

## Status

- [x] Plan implementation complete as of 2026-04-10.
- [x] `ReplaySession` shipped as the preferred Rust-native backtest entrypoint.
- [x] Legacy `BacktestHandle` path kept as compatibility-only surface.
- [x] Examples and README updated to the new replay entrypoint.
- [x] Final verification passed:
  `cargo fmt --check`
  `cargo check --examples`
  `cargo test`
  `cargo clippy --all-targets --all-features -- -D warnings`
  `cargo run --example pivot_point`

## Planned File Structure

### New Replay Subsystem

- `src/replay/mod.rs`
  Exports the replay public surface and hides internal helpers.
- `src/replay/types.rs`
  Public typed API: config, step, bar state, metadata, quote snapshot, settlement logs, final result.
- `src/replay/feed.rs`
  `HistoricalSource`, `FeedCursor`, and `FeedEvent`.
- `src/replay/providers.rs`
  Continuous-contract day mapping and future auxiliary timeline providers.
- `src/replay/series.rs`
  In-memory fixed-width windows for kline, tick, and aligned-kline handles.
- `src/replay/quote.rs`
  Quote precedence, implicit 1-minute feed registration, and broker price-path generation.
- `src/replay/kernel.rs`
  One-step time merge, handle update tracking, and per-step orchestration.
- `src/replay/sim.rs`
  Local matching engine, account/position/order state, and daily settlement.
- `src/replay/runtime.rs`
  `ReplayMarketAdapter` and `ReplayExecutionAdapter`.
- `src/replay/session.rs`
  Public `ReplaySession` facade and `Client::create_backtest_session` construction helpers.
- `src/replay/tests.rs`
  Shared fake historical source and replay integration tests.

### Existing Files To Modify

- `src/runtime/market.rs`
  Remove the hard dependency on `DataManager` so replay can provide market data directly.
- `src/runtime/core.rs`
  Stop storing a `DataManager` just to satisfy the current market trait.
- `src/runtime/mod.rs`
  Export replay-capable runtime pieces.
- `src/runtime/tests.rs`
  Update fake market adapters to the new trait boundary.
- `src/client/market.rs`
  Reuse market bootstrap helpers for an isolated replay history source.
- `src/client/facade.rs`
  Add `Client::create_backtest_session`.
- `src/client/mod.rs`
  Register the replay module in the public client surface.
- `src/lib.rs`
  Export new replay types.
- `src/prelude.rs`
  Re-export the replay public API.
- `README.md`
  Document the new preferred backtest entrypoint and demote the old handle to legacy status.
- `examples/backtest.rs`
  Rewrite around `ReplaySession`.
- `examples/pivot_point.rs`
  Update the example to use the new replay API instead of the legacy backtest shim.

### Legacy Boundary

- `src/backtest/core.rs`
- `src/backtest/mod.rs`

Do not expand these files into the new implementation. Keep them as legacy compatibility surface until the new replay stack is shipped and documented.

## Chunk 1: Runtime Boundary

### Task 1: Decouple `MarketAdapter` from `DataManager`

**Files:**
- Modify: `src/runtime/market.rs`
- Modify: `src/runtime/core.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/runtime/tests.rs`

- [x] **Step 1: Write the failing test**

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::runtime::{BacktestExecutionAdapter, MarketAdapter, RuntimeMode, RuntimeResult, TqRuntime};
use crate::types::Quote;

#[derive(Default)]
struct StubMarket;

#[async_trait]
impl MarketAdapter for StubMarket {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        Ok(Quote {
            instrument_id: symbol.to_string(),
            ..Quote::default()
        })
    }

    async fn wait_quote_update(&self, _symbol: &str) -> RuntimeResult<()> {
        Ok(())
    }

    async fn trading_time(&self, _symbol: &str) -> RuntimeResult<Option<Value>> {
        Ok(None)
    }
}

#[test]
fn runtime_accepts_market_adapter_without_datamanager() {
    let runtime = TqRuntime::new(
        RuntimeMode::Backtest,
        Arc::new(StubMarket),
        Arc::new(BacktestExecutionAdapter::new(vec!["TQSIM".to_string()])),
    );

    assert_eq!(runtime.mode(), RuntimeMode::Backtest);
    assert!(runtime.id().starts_with("runtime-"));
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test runtime_accepts_market_adapter_without_datamanager --lib`

Expected: FAIL because `MarketAdapter` still requires `fn dm(&self) -> Arc<DataManager>` and `TqRuntime` still derives its state from that method.

- [x] **Step 3: Write the minimal implementation**

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::datamanager::DataManager;
use crate::types::Quote;

use super::{RuntimeError, RuntimeResult};

#[async_trait]
pub trait MarketAdapter: Send + Sync {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote>;

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()>;

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>>;
}

pub struct LiveMarketAdapter {
    dm: Arc<DataManager>,
}

#[async_trait]
impl MarketAdapter for LiveMarketAdapter {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        Ok(self.dm.get_quote_data(symbol)?)
    }

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()> {
        let rx = self.dm.watch(vec!["quotes".to_string(), symbol.to_string()]);
        rx.recv()
            .await
            .map(|_| ())
            .map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "market quote updates",
            })
    }

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>> {
        Ok(self.dm.get_by_path(&["quotes", symbol, "trading_time"]))
    }
}

pub struct TqRuntime {
    id: String,
    mode: RuntimeMode,
    registry: Arc<TaskRegistry>,
    market: Arc<dyn MarketAdapter>,
    execution: Arc<dyn ExecutionAdapter>,
    engine: Arc<ExecutionEngine>,
    accounts: HashSet<String>,
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test runtime_accepts_market_adapter_without_datamanager --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/runtime/market.rs src/runtime/core.rs src/runtime/mod.rs src/runtime/tests.rs
git commit -m "refactor: decouple runtime market adapter from datamanager"
```

## Chunk 2: Replay Domain Types

### Task 2: Add the replay public types and exports

**Files:**
- Create: `src/replay/mod.rs`
- Create: `src/replay/types.rs`
- Create: `src/replay/tests.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`

- [x] **Step 1: Write the failing tests**

```rust
use chrono::{DateTime, Utc};

use crate::replay::{BarState, InstrumentMetadata, ReplayConfig};

#[test]
fn replay_config_rejects_inverted_range() {
    let start_dt = DateTime::<Utc>::from_timestamp(1_700_000_100, 0).unwrap();
    let end_dt = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

    let err = ReplayConfig::new(start_dt, end_dt).unwrap_err();
    assert!(err.to_string().contains("end_dt must be after start_dt"));
}

#[test]
fn instrument_metadata_serialization_excludes_dynamic_quote_fields() {
    let value = serde_json::to_value(InstrumentMetadata::default()).unwrap();

    assert!(value.get("last_price").is_none());
    assert!(value.get("ask_price1").is_none());
}

#[test]
fn bar_state_helper_methods_match_semantics() {
    assert!(BarState::Opening.is_opening());
    assert!(BarState::Closed.is_closed());
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_config_rejects_inverted_range --lib`

Expected: FAIL because the `replay` module and its public types do not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::errors::{Result, TqError};
use crate::types::{Account, Position, Trade};

#[derive(Debug, Clone)]
pub struct ReplayConfig {
    pub start_dt: DateTime<Utc>,
    pub end_dt: DateTime<Utc>,
    pub initial_balance: f64,
}

impl ReplayConfig {
    pub fn new(start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Result<Self> {
        if end_dt <= start_dt {
            return Err(TqError::InvalidParameter("end_dt must be after start_dt".to_string()));
        }

        Ok(Self {
            start_dt,
            end_dt,
            initial_balance: 10_000_000.0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarState {
    Opening,
    Closed,
}

impl BarState {
    pub fn is_opening(self) -> bool {
        matches!(self, Self::Opening)
    }

    pub fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstrumentMetadata {
    pub symbol: String,
    pub exchange_id: String,
    pub instrument_id: String,
    pub class: String,
    pub underlying_symbol: String,
    pub price_tick: f64,
    pub volume_multiple: i32,
    pub margin: f64,
    pub commission: f64,
    pub open_min_market_order_volume: i32,
    pub open_min_limit_order_volume: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplayQuote {
    pub symbol: String,
    pub datetime_nanos: i64,
    pub last_price: f64,
    pub ask_price1: f64,
    pub ask_volume1: i64,
    pub bid_price1: f64,
    pub bid_volume1: i64,
    pub highest: f64,
    pub lowest: f64,
    pub average: f64,
    pub volume: i64,
    pub amount: f64,
    pub open_interest: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplayHandleId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub current_dt: DateTime<Utc>,
    pub updated_handles: Vec<ReplayHandleId>,
    pub updated_quotes: Vec<String>,
    pub settled_trading_day: Option<NaiveDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySettlementLog {
    pub trading_day: NaiveDate,
    pub account: Account,
    pub positions: Vec<Position>,
    pub trades: Vec<Trade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub settlements: Vec<DailySettlementLog>,
    pub final_accounts: Vec<Account>,
    pub final_positions: Vec<Position>,
    pub trades: Vec<Trade>,
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_config_rejects_inverted_range --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/mod.rs src/replay/types.rs src/replay/tests.rs src/lib.rs src/prelude.rs
git commit -m "feat: add replay public types"
```

## Chunk 3: Historical Feed Primitives

### Task 3: Add `HistoricalSource`, `FeedCursor`, and continuous-contract day mapping

**Files:**
- Create: `src/replay/feed.rs`
- Create: `src/replay/providers.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
use chrono::NaiveDate;

use crate::replay::{ContinuousContractProvider, ContinuousMapping, FeedCursor, FeedEvent};
use crate::types::Kline;

#[test]
fn feed_cursor_emits_open_then_close_for_one_kline() {
    let bars = vec![Kline {
        id: 7,
        datetime: 1_000,
        open: 10.0,
        high: 12.0,
        low: 9.0,
        close: 11.0,
        open_oi: 100,
        close_oi: 105,
        volume: 6,
        epoch: None,
    }];

    let mut cursor = FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars);

    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarOpen { .. }));
    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarClose { .. }));
    assert!(cursor.next_event().is_none());
}

#[test]
fn continuous_provider_only_reveals_requested_day() {
    let provider = ContinuousContractProvider::from_rows(vec![
        ContinuousMapping {
            trading_day: NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
            symbol: "KQ.m@SHFE.rb".to_string(),
            underlying_symbol: "SHFE.rb2605".to_string(),
        },
        ContinuousMapping {
            trading_day: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            symbol: "KQ.m@SHFE.rb".to_string(),
            underlying_symbol: "SHFE.rb2610".to_string(),
        },
    ]);

    let current = provider.mapping_for(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap());
    assert_eq!(current["KQ.m@SHFE.rb"], "SHFE.rb2605");
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test feed_cursor_emits_open_then_close_for_one_kline --lib`

Expected: FAIL because `FeedCursor`, `FeedEvent`, and `ContinuousContractProvider` do not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use crate::errors::Result;
use crate::replay::InstrumentMetadata;
use crate::types::{Kline, Tick};

#[async_trait]
pub trait HistoricalSource: Send + Sync {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata>;

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>>;

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Tick>>;
}

#[derive(Debug, Clone)]
pub enum FeedEvent {
    Tick { symbol: String, tick: Tick },
    BarOpen { symbol: String, duration_nanos: i64, kline: Kline },
    BarClose { symbol: String, duration_nanos: i64, kline: Kline },
}

#[derive(Debug, Clone)]
pub struct FeedCursor {
    pending: VecDeque<FeedEvent>,
}

impl FeedCursor {
    pub fn from_kline_rows(symbol: &str, duration_nanos: i64, rows: Vec<Kline>) -> Self {
        let mut pending = VecDeque::new();

        for row in rows {
            pending.push_back(FeedEvent::BarOpen {
                symbol: symbol.to_string(),
                duration_nanos,
                kline: Kline {
                    high: row.open,
                    low: row.open,
                    close: row.open,
                    ..row.clone()
                },
            });
            pending.push_back(FeedEvent::BarClose {
                symbol: symbol.to_string(),
                duration_nanos,
                kline: row,
            });
        }

        Self { pending }
    }

    pub fn from_tick_rows(symbol: &str, rows: Vec<Tick>) -> Self {
        let pending = rows
            .into_iter()
            .map(|tick| FeedEvent::Tick {
                symbol: symbol.to_string(),
                tick,
            })
            .collect();
        Self { pending }
    }

    pub fn peek_timestamp(&self) -> Option<i64> {
        match self.pending.front() {
            Some(FeedEvent::Tick { tick, .. }) => Some(tick.datetime),
            Some(FeedEvent::BarOpen { kline, .. }) => Some(kline.datetime),
            Some(FeedEvent::BarClose {
                kline,
                duration_nanos,
                ..
            }) => Some(kline.datetime + duration_nanos - 1),
            None => None,
        }
    }

    pub fn next_event(&mut self) -> Option<FeedEvent> {
        self.pending.pop_front()
    }
}

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

impl ContinuousContractProvider {
    pub fn from_rows(rows: Vec<ContinuousMapping>) -> Self {
        let mut days = BTreeMap::new();
        for row in rows {
            days.entry(row.trading_day)
                .or_insert_with(HashMap::new)
                .insert(row.symbol, row.underlying_symbol);
        }
        Self { days }
    }

    pub fn mapping_for(&self, trading_day: NaiveDate) -> HashMap<String, String> {
        self.days.get(&trading_day).cloned().unwrap_or_default()
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test feed_cursor_emits_open_then_close_for_one_kline --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/feed.rs src/replay/providers.rs src/replay/mod.rs src/replay/tests.rs
git commit -m "feat: add replay feed primitives"
```

## Chunk 4: Kernel And Series Store

### Task 4: Add `SeriesStore` and `ReplayKernel` with one-timestamp stepping

**Files:**
- Create: `src/replay/series.rs`
- Create: `src/replay/kernel.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::replay::{BarState, FeedCursor, ReplayKernel};
use crate::types::Kline;

#[tokio::test]
async fn replay_kernel_commits_bar_open_and_close_as_two_steps() {
    let bars = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 13.0,
        low: 9.0,
        close: 12.0,
        open_oi: 100,
        close_oi: 110,
        volume: 8,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![
        ("kline:SHFE.rb2605:60000000000".to_string(), FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars)),
    ]);

    let first = kernel.step().await.unwrap().unwrap();
    assert_eq!(first.current_dt, DateTime::<Utc>::from_timestamp_nanos(1_000));

    let opening = kernel
        .series_store()
        .kline_rows("SHFE.rb2605", 60_000_000_000)
        .last()
        .unwrap()
        .clone();
    assert_eq!(opening.state, BarState::Opening);

    let second = kernel.step().await.unwrap().unwrap();
    assert_eq!(second.current_dt, DateTime::<Utc>::from_timestamp_nanos(60_000_000_999));

    let closed = kernel
        .series_store()
        .kline_rows("SHFE.rb2605", 60_000_000_000)
        .last()
        .unwrap()
        .clone();
    assert_eq!(closed.state, BarState::Closed);
}

#[tokio::test]
async fn replay_kernel_aligns_secondary_symbol_to_main_bar_time() {
    let main = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 11.0,
        low: 9.0,
        close: 10.5,
        open_oi: 100,
        close_oi: 101,
        volume: 5,
        epoch: None,
    }];
    let secondary = vec![Kline {
        id: 8,
        datetime: 1_000,
        open: 20.0,
        high: 21.0,
        low: 19.0,
        close: 20.5,
        open_oi: 200,
        close_oi: 201,
        volume: 6,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![
        ("kline:SHFE.rb2605:60000000000".to_string(), FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, main)),
        ("kline:SHFE.hc2605:60000000000".to_string(), FeedCursor::from_kline_rows("SHFE.hc2605", 60_000_000_000, secondary)),
    ]);
    let aligned = kernel.register_aligned_kline(
        &["SHFE.rb2605", "SHFE.hc2605"],
        60_000_000_000,
        32,
    );

    let _ = kernel.step().await.unwrap().unwrap();
    let rows = aligned.rows().await;
    let last = rows.last().unwrap();

    assert_eq!(last.datetime_nanos, 1_000);
    assert!(last.bars["SHFE.rb2605"].is_some());
    assert!(last.bars["SHFE.hc2605"].is_some());
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test replay_kernel_commits_bar_open_and_close_as_two_steps --lib`

Expected: FAIL because `ReplayKernel` and `SeriesStore` do not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::errors::Result;
use crate::replay::{BarState, FeedCursor, FeedEvent, ReplayHandleId, ReplayStep};
use crate::types::{Kline, Tick};

#[derive(Debug, Clone)]
pub struct ReplayBar {
    pub kline: Kline,
    pub state: BarState,
}

#[derive(Debug, Clone)]
pub struct AlignedReplayRow {
    pub datetime_nanos: i64,
    pub bars: HashMap<String, Option<ReplayBar>>,
}

#[derive(Clone)]
pub struct ReplayKlineHandle {
    pub id: ReplayHandleId,
    rows: Arc<RwLock<VecDeque<ReplayBar>>>,
}

impl ReplayKlineHandle {
    pub async fn rows(&self) -> Vec<ReplayBar> {
        self.rows.read().await.iter().cloned().collect()
    }

    pub fn updated_in(&self, step: &ReplayStep) -> bool {
        step.updated_handles.contains(&self.id)
    }
}

#[derive(Clone)]
pub struct AlignedKlineHandle {
    pub id: ReplayHandleId,
    rows: Arc<RwLock<VecDeque<AlignedReplayRow>>>,
}

impl AlignedKlineHandle {
    pub async fn rows(&self) -> Vec<AlignedReplayRow> {
        self.rows.read().await.iter().cloned().collect()
    }

    pub fn updated_in(&self, step: &ReplayStep) -> bool {
        step.updated_handles.contains(&self.id)
    }
}

#[derive(Debug, Default)]
pub struct SeriesStore {
    klines: HashMap<(String, i64), VecDeque<ReplayBar>>,
    ticks: HashMap<String, VecDeque<Tick>>,
    aligned: HashMap<(Vec<String>, i64), VecDeque<AlignedReplayRow>>,
}

impl SeriesStore {
    pub fn apply_event(&mut self, event: &FeedEvent, width: usize) {
        match event {
            FeedEvent::Tick { symbol, tick } => {
                let rows = self.ticks.entry(symbol.clone()).or_default();
                rows.push_back(tick.clone());
                while rows.len() > width {
                    rows.pop_front();
                }
            }
            FeedEvent::BarOpen {
                symbol,
                duration_nanos,
                kline,
            } => {
                let rows = self.klines.entry((symbol.clone(), *duration_nanos)).or_default();
                rows.push_back(ReplayBar {
                    kline: kline.clone(),
                    state: BarState::Opening,
                });
                while rows.len() > width {
                    rows.pop_front();
                }
            }
            FeedEvent::BarClose {
                symbol,
                duration_nanos,
                kline,
            } => {
                let rows = self.klines.entry((symbol.clone(), *duration_nanos)).or_default();
                if let Some(last) = rows.back_mut() {
                    last.kline = kline.clone();
                    last.state = BarState::Closed;
                }

                for ((symbols, duration), aligned_rows) in &mut self.aligned {
                    if *duration != *duration_nanos || !symbols.contains(symbol) {
                        continue;
                    }

                    let mut bars = HashMap::new();
                    for member in symbols {
                        let bar = self
                            .klines
                            .get(&(member.clone(), *duration_nanos))
                            .and_then(|rows| rows.back().cloned());
                        bars.insert(member.clone(), bar);
                    }

                    aligned_rows.push_back(AlignedReplayRow {
                        datetime_nanos: kline.datetime,
                        bars,
                    });
                    while aligned_rows.len() > width {
                        aligned_rows.pop_front();
                    }
                }
            }
        }
    }

    pub fn kline_rows(&self, symbol: &str, duration_nanos: i64) -> Vec<ReplayBar> {
        self.klines
            .get(&(symbol.to_string(), duration_nanos))
            .map(|rows| rows.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn aligned_rows(&self, symbols: &[String], duration_nanos: i64) -> Vec<AlignedReplayRow> {
        self.aligned
            .get(&(symbols.to_vec(), duration_nanos))
            .map(|rows| rows.iter().cloned().collect())
            .unwrap_or_default()
    }
}

pub struct ReplayKernel {
    feeds: BTreeMap<String, FeedCursor>,
    series_store: SeriesStore,
    width: usize,
    kline_handles: HashMap<(String, i64), ReplayKlineHandle>,
    aligned_handles: HashMap<(Vec<String>, i64), AlignedKlineHandle>,
}

impl ReplayKernel {
    pub fn for_test(feeds: Vec<(String, FeedCursor)>) -> Self {
        Self {
            feeds: feeds.into_iter().collect(),
            series_store: SeriesStore::default(),
            width: 128,
            kline_handles: HashMap::new(),
            aligned_handles: HashMap::new(),
        }
    }

    pub fn series_store(&self) -> &SeriesStore {
        &self.series_store
    }

    pub fn register_kline_feed(
        &mut self,
        symbol: &str,
        duration_nanos: i64,
        width: usize,
        bars: Vec<Kline>,
    ) -> Result<ReplayKlineHandle> {
        let feed_id = format!("kline:{}:{}", symbol, duration_nanos);
        self.feeds
            .insert(feed_id.clone(), FeedCursor::from_kline_rows(symbol, duration_nanos, bars));
        self.width = width;

        let handle = ReplayKlineHandle {
            id: ReplayHandleId(feed_id),
            rows: Arc::new(RwLock::new(VecDeque::new())),
        };
        self.kline_handles
            .insert((symbol.to_string(), duration_nanos), handle.clone());
        Ok(handle)
    }

    pub fn register_aligned_kline(
        &mut self,
        symbols: &[&str],
        duration_nanos: i64,
        _width: usize,
    ) -> AlignedKlineHandle {
        let id = ReplayHandleId(format!("aligned:{}:{}", symbols.join(","), duration_nanos));
        let key = (symbols.iter().map(|symbol| symbol.to_string()).collect::<Vec<_>>(), duration_nanos);
        self.series_store.aligned.entry(key.clone()).or_default();

        let handle = AlignedKlineHandle {
            id,
            rows: Arc::new(RwLock::new(VecDeque::new())),
        };
        self.aligned_handles.insert(key, handle.clone());
        handle
    }

    pub async fn step(&mut self) -> Result<Option<ReplayStep>> {
        let Some(next_ts) = self.feeds.values().filter_map(FeedCursor::peek_timestamp).min() else {
            return Ok(None);
        };

        let mut updated_handles = Vec::new();
        let mut updated_quotes = Vec::new();

        for (feed_id, cursor) in &mut self.feeds {
            while cursor.peek_timestamp() == Some(next_ts) {
                if let Some(event) = cursor.next_event() {
                    self.series_store.apply_event(&event, self.width);
                    updated_handles.push(ReplayHandleId(feed_id.clone()));

                    match &event {
                        FeedEvent::Tick { symbol, .. }
                        | FeedEvent::BarOpen { symbol, .. }
                        | FeedEvent::BarClose { symbol, .. } => {
                            updated_quotes.push(symbol.clone());
                        }
                    }
                }
            }
        }

        for ((symbol, duration_nanos), handle) in &self.kline_handles {
            let rows = self.series_store.kline_rows(symbol, *duration_nanos);
            let mut guard = handle.rows.write().await;
            *guard = rows.into_iter().collect();
        }

        for ((symbols, duration_nanos), handle) in &self.aligned_handles {
            let rows = self.series_store.aligned_rows(symbols, *duration_nanos);
            let mut guard = handle.rows.write().await;
            *guard = rows.into_iter().collect();
            updated_handles.push(handle.id.clone());
        }

        Ok(Some(ReplayStep {
            current_dt: DateTime::<Utc>::from_timestamp_nanos(next_ts),
            updated_handles,
            updated_quotes,
            settled_trading_day: None,
        }))
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test replay_kernel_commits_bar_open_and_close_as_two_steps --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/series.rs src/replay/kernel.rs src/replay/mod.rs src/replay/tests.rs
git commit -m "feat: add replay kernel and series store"
```

## Chunk 5: Quote Synthesis

### Task 5: Add quote precedence, implicit 1-minute feed, and broker price paths

**Files:**
- Create: `src/replay/quote.rs`
- Modify: `src/replay/kernel.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
use crate::replay::{InstrumentMetadata, QuoteSelection, QuoteSynthesizer};
use crate::types::{Kline, Tick};

#[test]
fn quote_synthesizer_prefers_tick_over_kline() {
    let mut synth = QuoteSynthesizer::default();
    let meta = InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        price_tick: 1.0,
        ..InstrumentMetadata::default()
    };

    synth.register_symbol("SHFE.rb2605", meta, QuoteSelection::Kline { duration_nanos: 60_000_000_000, implicit: false });
    synth.register_symbol("SHFE.rb2605", InstrumentMetadata::default(), QuoteSelection::Tick);

    let update = synth.apply_tick(
        "SHFE.rb2605",
        &Tick {
            datetime: 2_000,
            last_price: 12.0,
            ask_price1: 13.0,
            ask_volume1: 1,
            bid_price1: 11.0,
            bid_volume1: 1,
            ..Tick::default()
        },
    );

    assert_eq!(update.visible.last_price, 12.0);
}

#[test]
fn quote_synthesizer_builds_ohlc_price_path_for_smallest_kline() {
    let mut synth = QuoteSynthesizer::default();
    synth.register_symbol(
        "SHFE.rb2605",
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            price_tick: 1.0,
            ..InstrumentMetadata::default()
        },
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );

    let update = synth.apply_bar_close(
        "SHFE.rb2605",
        60_000_000_000,
        &Kline {
            datetime: 1_000,
            open: 10.0,
            high: 14.0,
            low: 9.0,
            close: 12.0,
            open_oi: 100,
            close_oi: 110,
            volume: 7,
            id: 1,
            epoch: None,
        },
    );

    let prices: Vec<f64> = update.path.iter().map(|step| step.last_price).collect();
    assert_eq!(prices, vec![10.0, 14.0, 9.0, 12.0]);
}

#[test]
fn quote_synthesizer_marks_implicit_minute_feed_as_lowest_priority() {
    let mut synth = QuoteSynthesizer::default();

    synth.register_symbol(
        "SHFE.rb2605",
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            price_tick: 1.0,
            ..InstrumentMetadata::default()
        },
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: true,
        },
    );

    assert_eq!(
        synth.selection_for("SHFE.rb2605"),
        Some(QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: true,
        })
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test quote_synthesizer_prefers_tick_over_kline --lib`

Expected: FAIL because `QuoteSynthesizer` and `QuoteSelection` do not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use std::collections::HashMap;

use crate::replay::{InstrumentMetadata, ReplayQuote};
use crate::types::{Kline, Tick};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSelection {
    Tick,
    Kline { duration_nanos: i64, implicit: bool },
}

#[derive(Debug, Clone)]
pub struct QuoteUpdate {
    pub visible: ReplayQuote,
    pub path: Vec<ReplayQuote>,
}

#[derive(Debug, Default)]
pub struct QuoteSynthesizer {
    meta: HashMap<String, InstrumentMetadata>,
    selected: HashMap<String, QuoteSelection>,
    visible: HashMap<String, ReplayQuote>,
}

impl QuoteSynthesizer {
    pub fn register_symbol(&mut self, symbol: &str, meta: InstrumentMetadata, selection: QuoteSelection) {
        self.meta.insert(symbol.to_string(), meta);
        self.selected.insert(symbol.to_string(), selection);
    }

    pub fn selection_for(&self, symbol: &str) -> Option<QuoteSelection> {
        self.selected.get(symbol).copied()
    }

    pub fn apply_tick(&mut self, symbol: &str, tick: &Tick) -> QuoteUpdate {
        let visible = ReplayQuote {
            symbol: symbol.to_string(),
            datetime_nanos: tick.datetime,
            last_price: tick.last_price,
            ask_price1: tick.ask_price1,
            ask_volume1: tick.ask_volume1,
            bid_price1: tick.bid_price1,
            bid_volume1: tick.bid_volume1,
            highest: tick.highest,
            lowest: tick.lowest,
            average: tick.average,
            volume: tick.volume,
            amount: tick.amount,
            open_interest: tick.open_interest,
        };
        self.visible.insert(symbol.to_string(), visible.clone());
        QuoteUpdate {
            visible: visible.clone(),
            path: vec![visible],
        }
    }

    pub fn apply_bar_open(&mut self, symbol: &str, duration_nanos: i64, kline: &Kline) -> QuoteUpdate {
        let _ = duration_nanos;
        let visible = self.quote_from_price(symbol, kline.datetime, kline.open, kline.open_oi);
        self.visible.insert(symbol.to_string(), visible.clone());
        QuoteUpdate {
            visible: visible.clone(),
            path: vec![visible],
        }
    }

    pub fn apply_bar_close(&mut self, symbol: &str, duration_nanos: i64, kline: &Kline) -> QuoteUpdate {
        let _ = duration_nanos;
        let path = vec![
            self.quote_from_price(symbol, kline.datetime, kline.open, kline.open_oi),
            self.quote_from_price(symbol, kline.datetime, kline.high, kline.close_oi),
            self.quote_from_price(symbol, kline.datetime, kline.low, kline.close_oi),
            self.quote_from_price(symbol, kline.datetime, kline.close, kline.close_oi),
        ];
        let visible = path.last().cloned().unwrap();
        self.visible.insert(symbol.to_string(), visible.clone());
        QuoteUpdate { visible, path }
    }

    fn quote_from_price(&self, symbol: &str, datetime_nanos: i64, price: f64, open_interest: i64) -> ReplayQuote {
        let meta = self.meta.get(symbol).cloned().unwrap_or_default();
        ReplayQuote {
            symbol: symbol.to_string(),
            datetime_nanos,
            last_price: price,
            ask_price1: price + meta.price_tick,
            ask_volume1: 1,
            bid_price1: price - meta.price_tick,
            bid_volume1: 1,
            highest: f64::NAN,
            lowest: f64::NAN,
            average: f64::NAN,
            volume: 0,
            amount: f64::NAN,
            open_interest,
        }
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test quote_synthesizer_prefers_tick_over_kline --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/quote.rs src/replay/kernel.rs src/replay/mod.rs src/replay/tests.rs
git commit -m "feat: add replay quote synthesis"
```

## Chunk 6: Local Matching And Settlement

### Task 6: Add `SimBroker` with Python-aligned fill rules and daily settlement

**Files:**
- Create: `src/replay/sim.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/replay/tests.rs`

- [x] **Step 1: Write the failing tests**

```rust
use chrono::NaiveDate;

use crate::replay::{DailySettlementLog, ReplayQuote, SimBroker};
use crate::types::{
    DIRECTION_BUY, InsertOrderRequest, OFFSET_OPEN, PRICE_TYPE_ANY, PRICE_TYPE_LIMIT,
};

fn limit_buy(symbol: &str, price: f64, volume: i64) -> InsertOrderRequest {
    InsertOrderRequest {
        symbol: symbol.to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_LIMIT.to_string(),
        limit_price: price,
        volume,
    }
}

fn market_buy(symbol: &str, volume: i64) -> InsertOrderRequest {
    InsertOrderRequest {
        symbol: symbol.to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_ANY.to_string(),
        limit_price: 0.0,
        volume,
    }
}

#[test]
fn sim_broker_fills_limit_buy_when_price_path_crosses() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    let order_id = broker.insert_order("TQSIM", &limit_buy("SHFE.rb2605", 11.0, 2)).unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[
                ReplayQuote { symbol: "SHFE.rb2605".to_string(), datetime_nanos: 1_000, last_price: 10.0, ask_price1: 11.0, ask_volume1: 1, bid_price1: 9.0, bid_volume1: 1, ..ReplayQuote::default() },
                ReplayQuote { symbol: "SHFE.rb2605".to_string(), datetime_nanos: 2_000, last_price: 12.0, ask_price1: 13.0, ask_volume1: 1, bid_price1: 11.0, bid_volume1: 1, ..ReplayQuote::default() },
            ],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(order.volume_left, 0);
}

#[test]
fn sim_broker_cancels_market_order_without_opponent_quote() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    let order_id = broker.insert_order("TQSIM", &market_buy("SHFE.rb2605", 1)).unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[ReplayQuote {
                symbol: "SHFE.rb2605".to_string(),
                datetime_nanos: 1_000,
                last_price: 10.0,
                ask_price1: f64::NAN,
                ask_volume1: 0,
                bid_price1: 9.0,
                bid_volume1: 1,
                ..ReplayQuote::default()
            }],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(order.volume_left, 1);
    assert_eq!(order.status, "FINISHED");
}

#[test]
fn sim_broker_records_daily_settlement() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    let settlement = broker
        .settle_day(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
        .unwrap()
        .unwrap();

    let DailySettlementLog { trading_day, .. } = settlement;
    assert_eq!(trading_day, NaiveDate::from_ymd_opt(2026, 4, 9).unwrap());
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test sim_broker_fills_limit_buy_when_price_path_crosses --lib`

Expected: FAIL because `SimBroker` does not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use chrono::NaiveDate;
use std::collections::HashMap;

use crate::errors::{Result, TqError};
use crate::replay::{BacktestResult, DailySettlementLog, ReplayQuote};
use crate::types::{
    Account, DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, OFFSET_OPEN, ORDER_STATUS_ALIVE,
    ORDER_STATUS_FINISHED, Order, Position, PRICE_TYPE_ANY, Trade,
};

pub struct SimBroker {
    accounts: HashMap<String, Account>,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    positions: HashMap<(String, String), Position>,
    settlements: Vec<DailySettlementLog>,
    next_order_seq: u64,
    next_trade_seq: u64,
}

impl SimBroker {
    pub fn new(account_keys: Vec<String>, initial_balance: f64) -> Self {
        let accounts = account_keys
            .into_iter()
            .map(|account_key| {
                (
                    account_key.clone(),
                    Account {
                        user_id: account_key,
                        balance: initial_balance,
                        available: initial_balance,
                        static_balance: initial_balance,
                        ..Account::default()
                    },
                )
            })
            .collect();

        Self {
            accounts,
            orders: HashMap::new(),
            trades_by_order: HashMap::new(),
            positions: HashMap::new(),
            settlements: Vec::new(),
            next_order_seq: 0,
            next_trade_seq: 0,
        }
    }

    pub fn insert_order(&mut self, account_key: &str, req: &InsertOrderRequest) -> Result<String> {
        self.next_order_seq += 1;
        let order_id = format!("sim-order-{}", self.next_order_seq);
        self.orders.insert(
            order_id.clone(),
            Order {
                order_id: order_id.clone(),
                user_id: account_key.to_string(),
                exchange_id: req.get_exchange_id(),
                instrument_id: req.get_instrument_id(),
                direction: req.direction.clone(),
                offset: req.offset.clone(),
                volume_orign: req.volume,
                price_type: req.price_type.clone(),
                limit_price: req.limit_price,
                status: ORDER_STATUS_ALIVE.to_string(),
                volume_left: req.volume,
                time_condition: "GFD".to_string(),
                volume_condition: "ANY".to_string(),
                ..Order::default()
            },
        );
        Ok(order_id)
    }

    pub fn apply_quote_path(&mut self, symbol: &str, path: &[ReplayQuote]) -> Result<()> {
        for quote in path {
            let order_ids: Vec<String> = self
                .orders
                .iter()
                .filter(|(_, order)| order.instrument_id == symbol.split('.').nth(1).unwrap_or(symbol))
                .map(|(order_id, _)| order_id.clone())
                .collect();

            for order_id in order_ids {
                let Some(order) = self.orders.get_mut(&order_id) else {
                    continue;
                };
                if order.status == ORDER_STATUS_FINISHED {
                    continue;
                }

                let fill_price = if order.price_type == PRICE_TYPE_ANY {
                    if order.direction == DIRECTION_BUY {
                        if quote.ask_price1.is_finite() { Some(quote.ask_price1) } else { None }
                    } else if quote.bid_price1.is_finite() {
                        Some(quote.bid_price1)
                    } else {
                        None
                    }
                } else if order.direction == DIRECTION_BUY && order.limit_price >= quote.ask_price1 {
                    Some(order.limit_price)
                } else if order.direction == DIRECTION_SELL && order.limit_price <= quote.bid_price1 {
                    Some(order.limit_price)
                } else {
                    None
                };

                match fill_price {
                    Some(price) => {
                        order.status = ORDER_STATUS_FINISHED.to_string();
                        order.volume_left = 0;
                        self.next_trade_seq += 1;
                        self.trades_by_order.entry(order_id.clone()).or_default().push(Trade {
                            trade_id: format!("sim-trade-{}", self.next_trade_seq),
                            order_id: order_id.clone(),
                            user_id: order.user_id.clone(),
                            exchange_id: order.exchange_id.clone(),
                            instrument_id: order.instrument_id.clone(),
                            direction: order.direction.clone(),
                            offset: order.offset.clone(),
                            price,
                            volume: order.volume_orign,
                            trade_date_time: quote.datetime_nanos,
                            ..Trade::default()
                        });
                    }
                    None if order.price_type == PRICE_TYPE_ANY => {
                        order.status = ORDER_STATUS_FINISHED.to_string();
                    }
                    None => {}
                }
            }
        }
        Ok(())
    }

    pub fn order(&self, account_key: &str, order_id: &str) -> Result<Order> {
        self.orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| TqError::DataNotFound(format!("order not found: {}:{}", account_key, order_id)))
    }

    pub fn position(&self, account_key: &str, symbol: &str) -> Result<Position> {
        Ok(self
            .positions
            .get(&(account_key.to_string(), symbol.to_string()))
            .cloned()
            .unwrap_or_default())
    }

    pub fn trades_by_order(&self, order_id: &str) -> Vec<Trade> {
        self.trades_by_order.get(order_id).cloned().unwrap_or_default()
    }

    pub fn settle_day(&mut self, trading_day: NaiveDate) -> Result<Option<DailySettlementLog>> {
        let Some(account) = self.accounts.values().next().cloned() else {
            return Ok(None);
        };

        let settlement = DailySettlementLog {
            trading_day,
            account,
            positions: self.positions.values().cloned().collect(),
            trades: self.trades_by_order.values().flat_map(|trades| trades.clone()).collect(),
        };
        self.settlements.push(settlement.clone());
        Ok(Some(settlement))
    }

    pub fn finish(self) -> BacktestResult {
        BacktestResult {
            settlements: self.settlements,
            final_accounts: self.accounts.into_values().collect(),
            final_positions: self.positions.into_values().collect(),
            trades: self
                .trades_by_order
                .into_values()
                .flat_map(|trades| trades.into_iter())
                .collect(),
        }
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test sim_broker_fills_limit_buy_when_price_path_crosses --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/sim.rs src/replay/mod.rs src/replay/tests.rs
git commit -m "feat: add replay sim broker"
```

## Chunk 7: Session, Runtime Bridge, And Client Entry

### Task 7: Add `ReplaySession`, replay runtime adapters, and `Client::create_backtest_session`

**Files:**
- Create: `src/replay/runtime.rs`
- Create: `src/replay/session.rs`
- Modify: `src/replay/feed.rs`
- Modify: `src/replay/kernel.rs`
- Modify: `src/replay/mod.rs`
- Modify: `src/client/market.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `src/replay/tests.rs`

- [x] **Step 1: Write the failing integration test**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::compat::{TargetPosTask, TargetPosTaskOptions};
use crate::replay::{HistoricalSource, InstrumentMetadata, ReplayConfig, ReplaySession};
use crate::types::{Kline, Tick};

struct FakeHistoricalSource {
    meta: HashMap<String, InstrumentMetadata>,
    klines: HashMap<(String, i64), Vec<Kline>>,
}

#[async_trait]
impl HistoricalSource for FakeHistoricalSource {
    async fn instrument_metadata(&self, symbol: &str) -> crate::Result<InstrumentMetadata> {
        Ok(self.meta.get(symbol).cloned().unwrap())
    }

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Kline>> {
        Ok(self
            .klines
            .get(&(symbol.to_string(), duration.as_nanos() as i64))
            .cloned()
            .unwrap_or_default())
    }

    async fn load_ticks(
        &self,
        _symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Tick>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn replay_session_runtime_can_drive_target_pos_task() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            "SHFE.rb2605".to_string(),
            InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                price_tick: 1.0,
                volume_multiple: 10,
                margin: 1000.0,
                commission: 2.0,
                ..InstrumentMetadata::default()
            },
        )]),
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![
                Kline { id: 1, datetime: 1_000, open: 10.0, high: 12.0, low: 9.0, close: 11.0, open_oi: 10, close_oi: 11, volume: 5, epoch: None },
                Kline { id: 2, datetime: 60_000_001_000, open: 11.0, high: 13.0, low: 10.0, close: 12.0, open_oi: 11, close_oi: 12, volume: 6, epoch: None },
            ],
        )]),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let _quote = session.quote("SHFE.rb2605").await.unwrap();
    let _klines = session
        .series()
        .kline("SHFE.rb2605", Duration::from_secs(60), 32)
        .await
        .unwrap();

    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let task = TargetPosTask::new(
        runtime.clone(),
        "TQSIM",
        "SHFE.rb2605",
        TargetPosTaskOptions::default(),
    )
    .await
    .unwrap();

    task.set_target_volume(1).unwrap();

    while session.step().await.unwrap().is_some() {}

    let position = runtime
        .execution()
        .position("TQSIM", "SHFE.rb2605")
        .await
        .unwrap();
    assert_eq!(position.volume_long, 1);
}
```

- [x] **Step 2: Run the integration test to verify it fails**

Run: `cargo test replay_session_runtime_can_drive_target_pos_task --lib`

Expected: FAIL because `ReplaySession`, `ReplayMarketAdapter`, and `ReplayExecutionAdapter` do not exist.

- [x] **Step 3: Write the minimal implementation**

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, broadcast};

use crate::client::Client;
use crate::errors::{Result, TqError};
use crate::replay::{
    BacktestResult, HistoricalSource, InstrumentMetadata, QuoteSynthesizer, ReplayConfig,
    ReplayKlineHandle, ReplayKernel, ReplayQuote, ReplayStep, SimBroker,
};
use crate::runtime::{ExecutionAdapter, MarketAdapter, RuntimeMode, TqRuntime};
use crate::series::{SeriesAPI, SeriesCachePolicy};
use crate::ins::InsAPI;
use crate::types::{InsertOrderRequest, Order, Position, Quote, Trade};
use crate::websocket::TqQuoteWebsocket;
use crate::datamanager::{DataManager, DataManagerConfig};

#[derive(Clone)]
pub struct ReplayMarketAdapter {
    quotes: Arc<RwLock<HashMap<String, ReplayQuote>>>,
    updates_tx: broadcast::Sender<String>,
}

#[derive(Clone)]
pub struct ReplayQuoteHandle {
    symbol: String,
    quotes: Arc<RwLock<HashMap<String, ReplayQuote>>>,
}

impl ReplayQuoteHandle {
    pub async fn snapshot(&self) -> Result<ReplayQuote> {
        let quotes = self.quotes.read().await;
        quotes
            .get(&self.symbol)
            .cloned()
            .ok_or_else(|| TqError::DataNotFound(format!("quote not ready: {}", self.symbol)))
    }
}

#[async_trait]
impl MarketAdapter for ReplayMarketAdapter {
    async fn latest_quote(&self, symbol: &str) -> crate::runtime::RuntimeResult<Quote> {
        let quotes = self.quotes.read().await;
        let Some(snapshot) = quotes.get(symbol).cloned() else {
            return Err(crate::runtime::RuntimeError::QuoteNotReady {
                symbol: symbol.to_string(),
            });
        };

        Ok(Quote {
            instrument_id: symbol.split('.').nth(1).unwrap_or(symbol).to_string(),
            exchange_id: symbol.split('.').next().unwrap_or_default().to_string(),
            datetime: snapshot.datetime_nanos.to_string(),
            last_price: snapshot.last_price,
            ask_price1: snapshot.ask_price1,
            ask_volume1: snapshot.ask_volume1,
            bid_price1: snapshot.bid_price1,
            bid_volume1: snapshot.bid_volume1,
            volume: snapshot.volume,
            amount: snapshot.amount,
            open_interest: snapshot.open_interest,
            ..Quote::default()
        })
    }

    async fn wait_quote_update(&self, symbol: &str) -> crate::runtime::RuntimeResult<()> {
        let mut rx = self.updates_tx.subscribe();
        loop {
            let updated = rx.recv().await.map_err(|_| crate::runtime::RuntimeError::AdapterChannelClosed {
                resource: "replay quote updates",
            })?;
            if updated == symbol {
                return Ok(());
            }
        }
    }

    async fn trading_time(&self, _symbol: &str) -> crate::runtime::RuntimeResult<Option<Value>> {
        Ok(None)
    }
}

#[derive(Clone)]
pub struct ReplayExecutionAdapter {
    accounts: Vec<String>,
    broker: Arc<Mutex<SimBroker>>,
}

#[async_trait]
impl ExecutionAdapter for ReplayExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }

    async fn insert_order(&self, account_key: &str, req: &InsertOrderRequest) -> crate::runtime::RuntimeResult<String> {
        self.broker.lock().await.insert_order(account_key, req).map_err(Into::into)
    }

    async fn order(&self, account_key: &str, order_id: &str) -> crate::runtime::RuntimeResult<Order> {
        self.broker.lock().await.order(account_key, order_id).map_err(Into::into)
    }

    async fn trades_by_order(&self, _account_key: &str, order_id: &str) -> crate::runtime::RuntimeResult<Vec<Trade>> {
        Ok(self.broker.lock().await.trades_by_order(order_id))
    }

    async fn position(&self, account_key: &str, symbol: &str) -> crate::runtime::RuntimeResult<Position> {
        self.broker.lock().await.position(account_key, symbol).map_err(Into::into)
    }
}

pub struct SdkHistoricalSource {
    series: Arc<SeriesAPI>,
    ins: Arc<InsAPI>,
}

impl SdkHistoricalSource {
    pub async fn connect(client: &Client) -> Result<Self> {
        let bootstrap = client.load_market_bootstrap(false).await?;
        let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
        let ws = Arc::new(TqQuoteWebsocket::new(
            bootstrap.md_url,
            Arc::clone(&dm),
            client.build_market_ws_config(bootstrap.headers, false),
        ));
        ws.init(false).await?;

        let series = Arc::new(SeriesAPI::new_with_cache_policy(
            Arc::clone(&dm),
            Arc::clone(&ws),
            Arc::clone(&client.auth),
            SeriesCachePolicy {
                enabled: client.config.series_disk_cache_enabled,
                max_bytes: client.config.series_disk_cache_max_bytes,
                retention_days: client.config.series_disk_cache_retention_days,
            },
        ));
        let ins = Arc::new(InsAPI::new(
            dm,
            ws,
            None,
            Arc::clone(&client.auth),
            client.config.stock,
            client.endpoints.holiday_url.clone(),
        ));

        Ok(Self { series, ins })
    }
}

#[async_trait]
impl HistoricalSource for SdkHistoricalSource {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata> {
        let row = self
            .ins
            .query_symbol_info(&[symbol])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| TqError::DataNotFound(format!("symbol not found: {}", symbol)))?;

        Ok(InstrumentMetadata {
            symbol: symbol.to_string(),
            exchange_id: row.get("exchange_id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            instrument_id: row.get("instrument_id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            class: row.get("ins_class").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            underlying_symbol: row.get("underlying_symbol").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            price_tick: row.get("price_tick").and_then(|v| v.as_f64()).unwrap_or(0.0),
            volume_multiple: row.get("volume_multiple").and_then(|v| v.as_i64()).unwrap_or_default() as i32,
            margin: row.get("margin").and_then(|v| v.as_f64()).unwrap_or(0.0),
            commission: row.get("commission").and_then(|v| v.as_f64()).unwrap_or(0.0),
            open_min_market_order_volume: row
                .get("open_min_market_order_volume")
                .and_then(|v| v.as_i64())
                .unwrap_or_default() as i32,
            open_min_limit_order_volume: row
                .get("open_min_limit_order_volume")
                .and_then(|v| v.as_i64())
                .unwrap_or_default() as i32,
        })
    }

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: chrono::DateTime<Utc>,
        end_dt: chrono::DateTime<Utc>,
    ) -> Result<Vec<crate::types::Kline>> {
        self.series.kline_data_series(symbol, duration, start_dt, end_dt).await
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: chrono::DateTime<Utc>,
        end_dt: chrono::DateTime<Utc>,
    ) -> Result<Vec<crate::types::Tick>> {
        self.series.tick_data_series(symbol, start_dt, end_dt).await
    }
}

pub struct ReplaySession {
    config: ReplayConfig,
    source: Arc<dyn HistoricalSource>,
    kernel: ReplayKernel,
    quote_synth: QuoteSynthesizer,
    market: ReplayMarketAdapter,
    execution: ReplayExecutionAdapter,
}

impl ReplaySession {
    pub async fn from_source(config: ReplayConfig, source: Arc<dyn HistoricalSource>) -> Result<Self> {
        let (updates_tx, _) = broadcast::channel(256);
        let quotes = Arc::new(RwLock::new(HashMap::new()));
        let broker = Arc::new(Mutex::new(SimBroker::new(Vec::new(), config.initial_balance)));

        Ok(Self {
            config,
            source,
            kernel: ReplayKernel::for_test(Vec::new()),
            quote_synth: QuoteSynthesizer::default(),
            market: ReplayMarketAdapter { quotes, updates_tx },
            execution: ReplayExecutionAdapter {
                accounts: vec!["TQSIM".to_string()],
                broker,
            },
        })
    }

    pub async fn quote(&mut self, symbol: &str) -> Result<ReplayQuoteHandle> {
        let meta = self.source.instrument_metadata(symbol).await?;
        self.quote_synth
            .register_symbol(symbol, meta, crate::replay::QuoteSelection::Kline { duration_nanos: 60_000_000_000, implicit: false });
        self.market.quotes.write().await.insert(symbol.to_string(), ReplayQuote {
            symbol: symbol.to_string(),
            ..ReplayQuote::default()
        });
        Ok(ReplayQuoteHandle {
            symbol: symbol.to_string(),
            quotes: Arc::clone(&self.market.quotes),
        })
    }

    pub fn series(&mut self) -> &mut Self {
        self
    }

    pub async fn kline(&mut self, symbol: &str, duration: Duration, width: usize) -> Result<ReplayKlineHandle> {
        let bars = self
            .source
            .load_klines(symbol, duration, self.config.start_dt, self.config.end_dt)
            .await?;
        self.kernel
            .register_kline_feed(symbol, duration.as_nanos() as i64, width, bars)
    }

    pub async fn runtime<const N: usize>(&self, accounts: [&str; N]) -> crate::runtime::RuntimeResult<Arc<TqRuntime>> {
        let execution = ReplayExecutionAdapter {
            accounts: accounts.iter().map(|account| account.to_string()).collect(),
            broker: Arc::clone(&self.execution.broker),
        };

        Ok(Arc::new(TqRuntime::new(
            RuntimeMode::Backtest,
            Arc::new(self.market.clone()),
            Arc::new(execution),
        )))
    }

    pub async fn step(&mut self) -> Result<Option<ReplayStep>> {
        self.kernel.step().await
    }

    pub async fn finish(self) -> Result<BacktestResult> {
        Ok(self.execution.broker.lock().await.finish())
    }
}

impl Client {
    pub async fn create_backtest_session(&self, config: ReplayConfig) -> Result<ReplaySession> {
        let source = SdkHistoricalSource::connect(self).await?;
        ReplaySession::from_source(config, Arc::new(source)).await
    }
}
```

- [x] **Step 4: Run the integration test to verify it passes**

Run: `cargo test replay_session_runtime_can_drive_target_pos_task --lib`

Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/replay/runtime.rs src/replay/session.rs src/replay/feed.rs src/replay/kernel.rs src/replay/mod.rs src/client/market.rs src/client/facade.rs src/client/mod.rs src/lib.rs src/prelude.rs src/replay/tests.rs
git commit -m "feat: add replay session and runtime bridge"
```

## Chunk 8: Public Examples, Docs, And Verification

### Task 8: Update examples and docs to use the new replay API

**Files:**
- Modify: `examples/backtest.rs`
- Modify: `examples/pivot_point.rs`
- Modify: `README.md`

- [x] **Step 1: Rewrite `examples/backtest.rs` around `ReplaySession`**

```rust
use std::env;
use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let username = env::var("TQ_AUTH_USER").expect("missing TQ_AUTH_USER");
    let password = env::var("TQ_AUTH_PASS").expect("missing TQ_AUTH_PASS");
    let symbol = env::var("TQ_TEST_SYMBOL").unwrap_or_else(|_| "SHFE.rb2605".to_string());

    let start_dt = Utc.from_utc_datetime(&NaiveDate::from_ymd_opt(2026, 4, 8).unwrap().and_hms_opt(0, 0, 0).unwrap());
    let end_dt = Utc.from_utc_datetime(&NaiveDate::from_ymd_opt(2026, 4, 9).unwrap().and_hms_opt(23, 59, 59).unwrap());

    let client = Client::builder(&username, &password).build().await?;
    let mut replay = client.create_backtest_session(ReplayConfig::new(start_dt, end_dt)?).await?;

    let _quote = replay.quote(&symbol).await?;
    let _bars = replay.series().kline(&symbol, Duration::from_secs(60), 300).await?;

    let runtime = replay.runtime(["TQSIM"]).await?;
    let task = TargetPosTask::new(
        Arc::clone(&runtime),
        "TQSIM",
        &symbol,
        TargetPosTaskOptions::default(),
    )
    .await?;

    while let Some(step) = replay.step().await? {
        if step.current_dt.timestamp() % 3600 == 0 {
            task.set_target_volume(1)?;
        }
    }

    let result = replay.finish().await?;
    println!("trades: {}", result.trades.len());
    Ok(())
}
```

- [x] **Step 2: Update `examples/pivot_point.rs` and `README.md` to the same public entrypoint**

```rust
let client = Client::builder(&username, &password).build().await?;
let mut replay = client
    .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
    .await?;

let quote = replay.quote(&symbol).await?;
let daily = replay
    .series()
    .kline(&symbol, Duration::from_secs(86_400), 20)
    .await?;

while let Some(step) = replay.step().await? {
    if daily.updated_in(&step) {
        let last_bar = daily.rows().await.last().unwrap().clone();
        let quote_snapshot = quote.snapshot().await?;
        let pivot = (last_bar.kline.high + last_bar.kline.low + last_bar.kline.close) / 3.0;
        println!("{} pivot={} last={}", step.current_dt, pivot, quote_snapshot.last_price);
    }
}
```

- [x] **Step 3: Run targeted replay tests**

Run: `cargo test replay_ --lib`

Expected: PASS for the replay unit and integration tests added in `src/replay/tests.rs`

- [x] **Step 4: Run the verification matrix**

Run: `cargo test`
Expected: PASS

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS

Run: `cargo fmt --check`
Expected: PASS

Run: `cargo check --examples`
Expected: PASS

If this execution session already has the required env vars injected, add:

Run: `cargo run --example pivot_point`
Expected: The example advances through replay data and prints pivot-point diagnostics without falling back to the legacy backtest warning path.

- [x] **Step 5: Commit**

```bash
git add examples/backtest.rs examples/pivot_point.rs README.md
git commit -m "docs: move examples to replay session backtest"
```
