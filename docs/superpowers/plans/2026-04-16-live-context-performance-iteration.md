# Live Context And Performance Iteration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将审查确认的问题收敛为一条可交付的迭代路线：先把 `Client` 修正为唯一 live session owner，再治理行情热路径性能，最后清理 API 与长期维护风险。

**Architecture:** 本计划不把所有审查问题平铺成同优先级任务。第一阶段对齐 Python `TqApi` 的核心模型：`Client` 本身就是一次 live session owner，独占 `DataManager`、`MarketDataState`、quote websocket、series/ins API、runtime live adapter 和 close signal；`ClientBuilder` / `TqAuth` / `ClientConfig` 负责构造，不再引入 public `LiveClient` / `LiveSession` 双对象。第二阶段降低高频行情路径中的全局写锁、clone、反序列化和任务调度成本；第三阶段修补 watcher、事件、缓存、错误类型等局部 API 与资源治理问题。

**Tech Stack:** Rust 2024, `tokio`, `watch`, `broadcast`, `Arc`, `Mutex/RwLock`, existing `Client`, `MarketDataState`, `DataManager`, `TqQuoteWebsocket`, `SeriesAPI`, `InsAPI`, `TqRuntime`, `TradeSession`, `cargo test`, `cargo clippy`, `cargo check --examples`.

---

## Source Audit Baseline

本计划基于以下已复核文档：

- `docs/archive/2026-04-15-design-perf-audit/review-2026-04-15-design-perf-audit-final.md`
- `docs/archive/2026-04-15-design-perf-audit/review-2026-04-15-design-perf-audit-synthesis.md`
- `docs/archive/2026-04-15-design-perf-audit/2026-04-15-api-design-review.md`
- `docs/archive/2026-04-15-design-perf-audit/review-2026-04-15-design-perf-audit.md`
- `docs/archive/2026-04-15-design-perf-audit/review-2026-04-15-design-perf-audit-kiro.md`
- `docs/archive/2026-04-15-design-perf-audit/review-2026-04-15-design-perf-audit-trae.md`

最终裁决：

- 唯一体系级设计问题：live 生命周期不是一等公共契约，`Client` 还没有真正承担单一 live session owner 的职责。
- 第一优先级局部问题：`DataManager` merge 全量衍生计算、`sync_market_state()` 二次解析/调度、全局重连定时器、`TargetPosTaskInner::ensure_started()` 死任务检测。
- 第二优先级局部问题：watcher 粒度和满队列治理、`seen_trade_ids` 无界增长、query/cache 锁范围与重复构建、`TqError #[non_exhaustive]`。
- 不进入近期主线：全面替换 `SeqCst`、`Utc::now()` 单点优化、重复 serde 注解、`LogLevel Copy`、`SimBroker` 滑点/部分成交模型。

## Non-Goals

- 不重新引入 `TqApi` / `SeriesAPI` / `InsAPI` 作为公开 canonical 入口。
- 不新增 quote / series stream fan-out API。
- 不恢复 callback/channel 风格交易事件接口。
- 不把所有 `std::sync::Mutex` 机械替换成 async mutex。
- 不在第一阶段做大规模 replay、polars、serde 注解重构。
- 不运行 live 测试或需要真实账号的示例，除非后续得到明确授权。

## Delivery Strategy

建议拆成 5 个 PR 或 5 个连续任务批次：

1. `PR-A`: `Client` session owner 重构，direct live API 与 runtime live adapter 同时切到同一个 `Client` 私有 live context；本 PR 结束时不得保留 runtime 自建 live websocket 的中间态。
2. `PR-B`: `TargetPosTask` 死任务检测、全局重连 timer、事件 tail、watcher 生命周期等可靠性修补。
3. `PR-C`: 行情热路径和 `DataManager` merge 性能治理。
4. `PR-D`: 缓存/query 性能治理。
5. `PR-E`: 文档、迁移说明、benchmark 与长期清理。

每个 PR 必须保持可测试、可回滚，不要把所有阶段合并成一次巨大改动。TDD 中允许本地先写红灯测试，但不得提交会让 CI 红灯或编译失败的中间状态；每个 PR 合入点必须是 green。

---

## File Map

### Live Lifecycle / Context

- `src/client/mod.rs`
  将 `Client` 明确定义为 live session owner；删除“配置根 + live 会话”混合但不明确的模型，不新增 public `LiveClient` / `LiveSession` 类型。
- `src/client/market.rs`
  将 `init_market()` / `switch_to_live()` 收敛为 `Client` session 的初始化/重建逻辑；禁止在同一个 `Client` 上运行时换账号并复用状态池。
- `src/client/facade.rs`
  调整 `set_auth()`、`into_runtime()`、`close()`、trade session 入口；live 方法保留在 `Client`，但全部绑定同一个私有 live context。
- `src/client/builder.rs`
  构建阶段创建一个明确的 session owner；若支持 lazy live 初始化，也必须仍然属于同一个 `Client` session 状态域。
- `src/client/tests.rs`
  增加关闭后 wait/read/ref 失效、禁止 active session 原地替换 auth、runtime 复用同一 session 的测试。
- `src/marketdata/mod.rs`
  让 `TqApi` / refs 绑定具体 live session close signal，旧 refs 不得跨 session 复用。
- `src/runtime/market.rs`
  删除或降级 runtime 自建 quote websocket 的路径，改为消费 live context。
- `src/runtime/mod.rs`, `src/runtime/execution.rs`, `src/runtime/account.rs`
  如需调整 runtime 构造参数，在这里同步。

### Market Hot Path / DataManager

- `src/datamanager/merge.rs`
  收集 changed paths，增量化 `apply_python_data_semantics()` 和 `apply_orders_trade_price()`。
- `src/datamanager/query.rs`
  增加内部借用式查询 helper，缩短 `get_multi_klines_data()` 读锁范围。
- `src/websocket/quote.rs`
  移除 `payload.clone()`；重构 `sync_market_state()` 为单消费者或有界更新管线。
- `src/websocket/core.rs`
  为收包 debug 文本构造加 `tracing::enabled!` 守卫；保留交易关键发送路径。
- `src/marketdata/mod.rs`
  如 `sync_market_state()` 变为 actor，需要调整 `MarketDataState` 更新接口。

### Reliability / Local API Cleanup

- `src/websocket/reconnect.rs`
  去掉全局共享 `RECONNECT_TIMER`，改为实例级或连接类型级 timer。
- `src/websocket/core.rs`
  持有实例级 reconnect state，并在重连逻辑中使用。
- `src/datamanager/watch.rs`
  改 watcher 取消粒度；增加满队列治理。
- `src/trade_session/events.rs`
  修复 `subscribe_tail()` 原子性；治理 `seen_trade_ids` 上限或 close 清理。
- `src/runtime/tasks/target_pos.rs`
  修复 `ensure_started()` 对 finished task 的处理。
- `src/series/api.rs`
  将缓存缺口计算、网络下载、写回拆成两段或三段流程。
- `src/cache/data_series.rs`
  如需要，把写入 API 改为更明确的 locked writer 或内部锁封装。
- `src/errors.rs`
  给 public `TqError` 增加 `#[non_exhaustive]`。

### Docs / Examples / Agent Rules

- `README.md`
  删除运行时切账号的误导表述，新增 live context / close 语义。
- `docs/architecture.md`
  更新 live context owner 与 runtime 关系。
- `docs/migration-remove-legacy-compat.md`
  记录 breaking 或 behavioral changes。
- `AGENTS.md`, `CLAUDE.md`
  更新 canonical API 和安全边界，防止未来 agent 继续生成旧模型。
- `examples/`
  只在 public API 变化后同步。

---

## Phase 0: Baseline And Guard Tests

**Goal:** 在改架构前先设计缺失的行为测试，让后续重构有防回归边界。Phase 0 是本地 TDD 准备阶段，不作为单独可合并 PR；测试必须和对应实现一起进入 PR，保证提交点 green。

**Files:**

- Modify: `src/client/tests.rs`
- Modify: `src/runtime/tasks/tests.rs`
- Modify: `src/websocket/tests.rs` or colocated tests in `src/websocket/core.rs`
- Modify: `src/trade_session/events.rs`
- Modify: `src/datamanager/tests.rs`

### Task 0.1: Add close invalidation tests for direct market refs

- [ ] Add characterization tests proving the current lifecycle bug using existing `Client` APIs.
- [ ] In Phase 1, keep the assertions on `Client` and make them pass by fixing `Client` session ownership.

Required cases:

- Current API before refactor: `Client::wait_update()` returns a clear close error after `close()`.
- Current API before refactor: `Client::wait_update_and_drain()` returns a clear close error after `close()`.
- Final API after refactor: `Client::wait_update()` returns a clear close error after `Client::close()`.
- Final API after refactor: `Client::wait_update_and_drain()` returns a clear close error after `Client::close()`.
- `QuoteRef::wait_update()` returns a clear close error after owning live context closes.
- `KlineRef::wait_update()` returns a clear close error after owning live context closes.
- `TickRef::wait_update()` returns a clear close error after owning live context closes.

Local TDD check before implementation:

- At least one test should fail or hang unless wrapped in timeout, proving the lifecycle contract is currently incomplete.
- Do not commit this red state by itself. Land the test only in the same PR that makes it pass.

Run:

```bash
cargo test close_invalidates_market_interfaces -- --nocapture
```

Expected after Phase 1:

```text
test result: ok
```

### Task 0.2: Add runtime live context reuse test

- [ ] Add a characterization test proving current `Client::into_runtime()` creates or can create an independent live websocket / `MarketDataState`.
- [ ] In Phase 1, replace this with the final assertion that `Client::into_runtime()` / `Client::runtime()` consumes or shares the same private live context and does not create an independent websocket / `MarketDataState`.
- [ ] Use fake websocket / test hooks rather than network.
- [ ] Assert final runtime market adapter observes the same lifecycle close signal as direct `Client` refs.

Run:

```bash
cargo test runtime_reuses_client_live_context -- --nocapture
```

Local TDD check before implementation:

```text
FAILED until runtime construction is refactored
```

Do not commit this red state by itself. Land the test only in the same PR that makes it pass.

### Task 0.3: Add TargetPosTask finished-handle regression test

- [ ] Add a test where the spawned task is already finished and a later operation does not silently accept a dead handle.
- [ ] Acceptable final behavior: either restart the task or return `RuntimeError::TargetTaskFinished` with explicit reason.
- [ ] Prefer restart if the task is not explicitly closed and no failure is stored.

Run:

```bash
cargo test target_pos_restarts_or_errors_after_worker_finished -- --nocapture
```

### Task 0.4: Add independent reconnect timer test

- [ ] Add a deterministic test for two websocket instances with independent reconnect state.
- [ ] Avoid real network; test delay calculation state directly.
- [ ] Assert one instance's reconnect schedule cannot delay another instance.

Run:

```bash
cargo test reconnect_timer_is_per_connection -- --nocapture
```

### Phase 0 Exit Criteria

- [ ] New tests compile.
- [ ] Local TDD draft tests fail for the expected reason before implementation.
- [ ] No Phase 0-only commit is created; every committed test is paired with implementation that makes CI green.
- [ ] No live network tests added.
- [ ] Existing `cargo test client_ -- --nocapture` still passes at every commit boundary.

---

## Phase 1: Make `Client` The Single Live Session Owner

**Goal:** 让 `Client` 本身成为唯一 live session owner，替代散落在 `market_active`、`TqApi`、runtime adapter 中的隐式生命周期。

**Files:**

- Create or modify: `src/client/live.rs`
- Modify: `src/client/mod.rs`
- Modify: `src/client/market.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/builder.rs`
- Modify: `src/marketdata/mod.rs`
- Modify: `src/runtime/market.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/client/tests.rs`

### Design Target

Introduce a private session context owned by `Client`, similar to:

```rust
pub(crate) struct LiveContext {
    dm: Arc<DataManager>,
    market_state: Arc<MarketDataState>,
    quotes_ws: Arc<TqQuoteWebsocket>,
    series_api: Arc<SeriesAPI>,
    ins_api: Arc<InsAPI>,
    close_tx: tokio::sync::watch::Sender<bool>,
}
```

This exact shape may change during implementation, but the invariant must not:

- One `Client` represents one live market session.
- `DataManager` and `MarketDataState` are owned by that `Client` session, not by a reusable auth/config root.
- Direct `Client` live refs and runtime live adapter use the same private `LiveContext`.
- Closing the owner invalidates all wait/read paths that depend on live updates.
- Re-auth is not an in-place mutation of an active `Client`; switching accounts creates a new `Client`.
- `ClientBuilder`, `TqAuth`, and `ClientConfig` are construction-side concepts; `Client` is the session handle.

### Task 1.1: Add private `LiveContext` owned by `Client`

- [ ] Create or update `src/client/live.rs`.
- [ ] Define private `LiveContext` as the canonical internal owner for live state.
- [ ] Move ownership of `DataManager`, `MarketDataState`, `quotes_ws`, `series_api`, `ins_api`, and close signal into `LiveContext`, and make `Client` own exactly one context.
- [ ] Keep `LiveContext` internals private behind `Arc`.
- [ ] Do not keep parallel root-level fields like `market_active`, `quotes_ws`, `series_api`, `ins_api`, `live_api` outside the context as the final architecture.

Run:

```bash
cargo test client_ -- --nocapture
```

Expected:

```text
test result: ok
```

### Task 1.2: Bind all live APIs to `Client`'s private context

- [ ] Keep canonical live methods on `Client`: `quote`, `kline_ref`, `tick_ref`, `wait_update`, `wait_update_and_drain`, `subscribe_quote`, `get_kline_serial`, `get_tick_serial`, and live ins query methods.
- [ ] Make every live method go through the same private `LiveContext`.
- [ ] Do not leave a mixed model where some methods read root `Client` fields and others read `LiveContext`.
- [ ] If lazy initialization remains, it must initialize the single context once and never create a second market state domain.

Affected live methods:

- `quote`
- `kline_ref`
- `tick_ref`
- `wait_update`
- `wait_update_and_drain`
- `subscribe_quote`
- `get_kline_serial`
- `get_tick_serial`
- query methods that need `InsAPI`

### Task 1.3: Make close invalidate wait/read refs

- [ ] Add close observation to live-session-bound `TqApi` or `MarketDataState` refs.
- [ ] Ensure wait methods return explicit close errors rather than hanging.
- [ ] Existing snapshots may remain readable if they are already materialized, but wait-for-next-update must not hang.
- [ ] Creating a new `Client` after close must produce a fresh `DataManager` and `MarketDataState`; old refs must not observe new session data.

Run:

```bash
cargo test close_invalidates_market_interfaces -- --nocapture
```

Expected:

```text
test result: ok
```

### Task 1.4: Refactor runtime live adapter to consume `Client`'s `LiveContext`

- [ ] Remove lazy creation of `TqQuoteWebsocket` from `src/runtime/market.rs`.
- [ ] Construct live runtime adapter from `Client`'s private `LiveContext`.
- [ ] Keep `Client::into_runtime()` only if it consumes or shares the same context without creating hidden live resources.
- [ ] Update `ClientBuilder::build_runtime()` because it currently delegates to `build().await?.into_runtime()`.
- [ ] Keep `Client::create_backtest_session()` separate; replay/backtest must not require live context.

Decision point:

- If keeping `Client::into_runtime(self) -> Arc<TqRuntime>` would require hidden second context creation, remove or replace it with a fallible explicit flow.
- Do not silently create a second live market context.

Run:

```bash
cargo test runtime_reuses_client_live_context -- --nocapture
cargo test runtime_market_adapter_closes_with_live_context -- --nocapture
```

Expected:

```text
test result: ok
```

### Task 1.5: Redefine `set_auth()` semantics

- [ ] Decide implementation path before editing:
  - Preferred breaking path: remove `Client::set_auth()` or make it fail for any initialized/active `Client` session.
  - If keeping temporary compatibility: only allow it before live initialization, and document that it is not runtime account switching.
- [ ] Update README so account switching is described as creating a new `Client` from new auth.
- [ ] Remove docs implying `set_auth(); init_market()` mutates an active live session in place.

Required tests:

- `set_auth_rejects_active_client_session`
- `new_client_after_auth_change_uses_fresh_state`

Run:

```bash
cargo test set_auth_ -- --nocapture
```

### Task 1.6: Clarify `switch_to_live()` as mode transition only

- [ ] Treat `switch_to_live()` as market mode transition only, not as auth replacement or account switching.
- [ ] If `switch_to_live()` remains, ensure it closes and replaces the entire private `LiveContext`, not individual root fields.
- [ ] Do not reuse the new-account `Client` construction path as an in-place mode switching mechanism.
- [ ] Ensure old refs observe close if their context is closed.
- [ ] Ensure new refs bind to the new session state, not reused stale state.

Run:

```bash
cargo test switch_to_live_replaces_live_context -- --nocapture
```

### Phase 1 Exit Criteria

- [ ] All direct live APIs share the same lifecycle boundary.
- [ ] `close()` invalidates all wait paths.
- [ ] Runtime live adapter uses the same private context as direct `Client` APIs; no runtime self-owned quote websocket remains.
- [ ] Account switching docs no longer overpromise field-level live mutation.
- [ ] `Client` owns live `DataManager` / `MarketDataState` only through one private `LiveContext`, with no parallel root-level live fields.
- [ ] `cargo test client_ -- --nocapture` passes.
- [ ] `cargo test marketdata::tests -- --nocapture` passes.
- [ ] `cargo test runtime_reuses_client_live_context -- --nocapture` passes.

---

## Phase 2: TargetPos Reliability

**Goal:** Target-pos tasks recover or fail clearly when workers finish unexpectedly.

**Files:**

- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tasks/tests.rs`

### Task 2.1: Fix `TargetPosTaskInner::ensure_started()`

- [ ] If `started` is `Some(handle)` and `!handle.is_finished()`, keep current behavior.
- [ ] If `started` is `Some(handle)` and `handle.is_finished()`, inspect stored failure and closed state.
- [ ] If task is closed or failure exists, return explicit `RuntimeError::TargetTaskFinished` or existing failure result.
- [ ] If task finished without failure and task is not closed, clear handle and spawn a new worker.

Run:

```bash
cargo test target_pos_restarts_or_errors_after_worker_finished -- --nocapture
cargo test runtime::tasks -- --nocapture
```

### Phase 2 Exit Criteria

- [ ] `TargetPosTask` no longer silently accepts dead worker handles.
- [ ] `cargo test runtime::tasks -- --nocapture` passes.

---

## Phase 3: Reconnect And Event/Watcher Reliability

**Goal:** Remove real local reliability risks without changing the high-level public model again.

**Files:**

- Modify: `src/websocket/reconnect.rs`
- Modify: `src/websocket/core.rs`
- Modify: `src/websocket/quote.rs`
- Modify: `src/websocket/trade.rs`
- Modify: `src/datamanager/watch.rs`
- Modify: `src/datamanager/tests.rs`
- Modify: `src/trade_session/events.rs`
- Modify: `src/trade_session/tests.rs`

### Task 3.1: Replace global reconnect timer

- [ ] Move `SharedReconnectTimer` into websocket instance state or config-derived state.
- [ ] Remove static `RECONNECT_TIMER`.
- [ ] Keep jitter/backoff behavior equivalent per connection.
- [ ] Add test proving two timers do not affect each other.

Run:

```bash
cargo test reconnect_timer_is_per_connection -- --nocapture
```

### Task 3.2: Make `subscribe_tail()` atomic

- [ ] In `EventJournal::subscribe_tail()`, take the state lock once.
- [ ] Read `last_seen_seq` and insert subscriber in the same critical section.
- [ ] Keep existing retention and `Lagged` behavior unchanged.

Run:

```bash
cargo test trade_event_hub_subscriber_starts_at_tail -- --nocapture
cargo test trade_session::events -- --nocapture
```

### Task 3.3: Bound or clear `seen_trade_ids`

- [ ] At minimum, clear `publish_state.seen_trade_ids` in `TradeEventHub::close()`.
- [ ] Prefer bounded LRU if duplicate trade IDs can arrive over long sessions.
- [ ] Document the chosen semantics in code comments.

Run:

```bash
cargo test trade_event_hub_close_clears_dedup_state -- --nocapture
```

### Task 3.4: Fix watcher cancellation granularity

- [ ] Treat this as an internal API change unless a separate public API review explicitly approves exposing it.
- [ ] Add a `pub(crate)` guard-based watcher handle for internal users that need precise cancellation.
- [ ] Do not make `unwatch(path)` the only way to cancel one receiver.
- [ ] Keep existing public `watch(path) -> Receiver<Value>` behavior compatible.
- [ ] Do not expose a new public watcher guard in this phase.

Required internal shape:

```rust
pub(crate) struct WatchRegistration {
    path: Vec<String>,
    watcher_id: i64,
    rx: async_channel::Receiver<serde_json::Value>,
}
```

Acceptance:

- Two watchers on same path can cancel independently.
- Dropping a guard or calling explicit cancel does not remove the other watcher.
- No public `DataManager` API is added or changed without a separate migration note.

Run:

```bash
cargo test datamanager_watchers_same_path_cancel_independently -- --nocapture
```

### Task 3.5: Add slow watcher governance

- [ ] Track consecutive `TrySendError::Full` per watcher.
- [ ] After a threshold, remove the watcher and log a warning.
- [ ] Keep threshold configurable via `DataManagerConfig` or use a conservative constant if config churn is not worth it.

Run:

```bash
cargo test datamanager_drops_persistently_full_watcher -- --nocapture
```

### Phase 3 Exit Criteria

- [ ] Reconnect timer is instance-scoped.
- [ ] Trade event tail subscription is atomic.
- [ ] Trade dedup memory cannot grow unbounded for closed sessions.
- [ ] Watchers can be cancelled independently.
- [ ] Slow watchers do not cause permanent per-merge send attempts.
- [ ] `cargo test trade_session::events -- --nocapture` passes.
- [ ] `cargo test datamanager::tests -- --nocapture` passes.

---

## Phase 4: Market Hot Path Performance

**Goal:** Reduce high-frequency market update overhead without changing public behavior.

**Files:**

- Modify: `src/datamanager/merge.rs`
- Modify: `src/datamanager/tests.rs`
- Modify: `src/websocket/quote.rs`
- Modify: `src/websocket/core.rs`
- Modify: `src/marketdata/mod.rs`
- Create or modify: `benches/marketdata_state*.rs`

### Task 4.1: Track changed top-level paths during merge

- [ ] Extend merge internals to return changed path metadata.
- [ ] At minimum distinguish quote-only, trade-only, positions/orders/trades changed.
- [ ] Preserve epoch behavior and `reduce_diff` semantics.

Run:

```bash
cargo test datamanager::tests -- --nocapture
```

### Task 4.2: Incrementalize `apply_python_data_semantics()`

- [ ] Run quote expire derivation only for changed quote symbols or only when quote metadata changes.
- [ ] Run position/order/trade derivation only when trade subtree changes.
- [ ] Ensure pure quote `last_price` update does not scan all historical trades.

Required regression tests:

- Quote-only merge does not call trade derivation.
- Trade-only merge still updates `trade_price`.
- Position derived fields remain compatible with existing tests.

Run:

```bash
cargo test datamanager_merge_ -- --nocapture
```

### Task 4.3: Remove `handle_rtn_data()` payload clone

- [ ] Change `handle_rtn_data()` to consume owned payload where practical.
- [ ] If reconnect buffering needs a copy, clone only the reconnect slice, not the whole normal hot-path payload.
- [ ] Preserve reconnect buffering semantics.

Run:

```bash
cargo test websocket::quote -- --nocapture
```

### Task 4.4: Refactor `sync_market_state()`

- [ ] Replace per-batch unbounded `tokio::spawn` with a single consumer or bounded update queue.
- [ ] Avoid reading back from `DataManager` when diff contains enough data to update typed state.
- [ ] If direct diff-to-type is too large for one PR, first make a single actor and keep readback as interim.

Acceptance:

- No unbounded task fan-out under high-frequency rtn_data.
- `Client::wait_update()` still observes updates.
- Existing marketdata tests pass.

Run:

```bash
cargo test marketdata::tests -- --nocapture
cargo test client_ -- --nocapture
```

### Task 4.5: Guard debug payload string construction

- [ ] Wrap receive-side payload text creation in `tracing::enabled!`.
- [ ] Keep warning logs for JSON parse failures informative.

Run:

```bash
cargo test websocket::core -- --nocapture
```

### Task 4.6: Add or update performance baselines

- [ ] Add a benchmark for merge throughput with quote-only updates.
- [ ] Add a benchmark for merge with many trades/orders.
- [ ] Add a benchmark or test harness for market update fan-out latency.
- [ ] Document baseline numbers in the bench output or a short notes file if CI does not run benches.

Run:

```bash
cargo test
cargo bench --no-run
```

### Phase 4 Exit Criteria

- [ ] Quote-only merge no longer performs full trade/order derivation.
- [ ] Market state sync no longer spawns one task per update batch without bound.
- [ ] Hot debug logging no longer stringifies payload when debug is disabled.
- [ ] `cargo test` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.

---

## Phase 5: Query And Historical Cache Performance

**Goal:** Reduce long read locks, subtree clones, and cache lock hold time.

**Files:**

- Modify: `src/datamanager/query.rs`
- Modify: `src/datamanager/tests.rs`
- Modify: `src/series/api.rs`
- Modify: `src/cache/data_series.rs`
- Modify: `src/series/tests.rs`

### Task 5.1: Add internal borrowed query helper

- [ ] Keep public `get_by_path() -> Option<Value>` unchanged unless a breaking change is explicitly approved.
- [ ] Add internal helper that runs a closure under read lock and returns a computed result.
- [ ] Replace reconnect completeness and query hot paths that currently clone whole subtrees.

Example shape:

```rust
pub(crate) fn with_path_ref<R>(&self, path: &[&str], f: impl FnOnce(Option<&Value>) -> R) -> R
```

Run:

```bash
cargo test datamanager::tests -- --nocapture
```

### Task 5.2: Shorten `get_multi_klines_data()` read lock

- [ ] Snapshot only necessary raw K-line values or indexes under read lock.
- [ ] Drop read lock before sorting, binding alignment, and typed deserialization.
- [ ] Preserve existing output order and `has_new_bar` semantics.

Run:

```bash
cargo test multi_klines -- --nocapture
```

### Task 5.3: Cache numeric indexes by epoch or avoid rebuild

- [ ] Avoid parsing all numeric ids on every multi-kline query if epoch did not change.
- [ ] Keep cache invalidation simple and tied to `get_path_epoch()`.
- [ ] Do not introduce global unbounded cache.

Run:

```bash
cargo test datamanager::tests -- --nocapture
```

### Task 5.4: Move historical downloads outside cache lock

- [ ] Under cache lock, compute missing ranges.
- [ ] Release lock and download missing ranges.
- [ ] Reacquire lock, recompute missing ranges, write only still-missing data.
- [ ] Merge adjacent files and enforce limits after write.

Run:

```bash
cargo test series::tests -- --nocapture
```

### Phase 5 Exit Criteria

- [ ] Large query computation no longer holds global DM read lock.
- [ ] Historical cache lock is not held across network awaits.
- [ ] Existing history/cache tests pass.
- [ ] `cargo test` passes.

---

## Phase 6: Public Error And Documentation Cleanup

**Goal:** Align docs, examples, migration notes, and public error evolution with the new contract.

**Files:**

- Modify: `src/errors.rs`
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `skills/tqsdk-rs/` if present and relevant
- Modify: `examples/` if public API changed

### Task 6.1: Add `#[non_exhaustive]` to `TqError`

- [ ] Add `#[non_exhaustive]` above `pub enum TqError`.
- [ ] Update any internal exhaustive matches if compilation requires it.
- [ ] Mention this in migration notes as a downstream matching change.

Run:

```bash
cargo test
```

### Task 6.2: Update README canonical lifecycle docs

- [ ] Replace runtime switching docs with context replacement semantics.
- [ ] Document close behavior for wait/read/ref paths.
- [ ] Ensure examples do not call removed or discouraged APIs.

### Task 6.3: Update architecture and agent rules

- [ ] `docs/architecture.md` describes `Client` as the session owner, with private `LiveContext` and runtime consumption of that same context.
- [ ] `AGENTS.md` and `CLAUDE.md` prohibit reintroducing runtime self-owned live websocket paths.
- [ ] Update `skills/tqsdk-rs/` if it contains stale API guidance.

Run:

```bash
cargo check --examples
cargo test --doc
```

### Phase 6 Exit Criteria

- [ ] Docs match implemented public contract.
- [ ] Examples compile.
- [ ] Agent rules prevent regression to the old design philosophy.
- [ ] `cargo check --examples` passes.
- [ ] `cargo test --doc` passes.

---

## Final Verification Matrix

Run after all phases:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo check --examples
```

Run additionally if `polars_ext/` changes:

```bash
cargo build --features polars
```

Do not run live examples or ignored live tests without explicit authorization:

- `cargo run --example quote`
- `cargo run --example history`
- `cargo run --example data_series`
- `cargo run --example backtest`
- `cargo run --example trade`

---

## Recommended Commit Boundaries

1. `test: add lifecycle and reliability regression coverage`
2. `refactor(client): introduce single live context owner`
3. `refactor(runtime): reuse client live context`
4. `fix(runtime): handle finished target position workers`
5. `fix(websocket): make reconnect timers per connection`
6. `fix(trade-session): make tail subscriptions atomic`
7. `fix(datamanager): improve watcher lifecycle governance`
8. `perf(datamanager): incrementalize derived merge semantics`
9. `perf(websocket): reduce market update fanout overhead`
10. `perf(series): avoid cache lock during network downloads`
11. `docs: align lifecycle contract and migration guidance`

---

## Risk Register

### Risk 1: Live context refactor becomes too large

Mitigation:

- Keep Phase 1 focused on lifecycle ownership only.
- Do not optimize merge or watcher behavior in the same PR.
- Keep old internal helpers temporarily if they are not public and do not violate owner semantics.

### Risk 2: Runtime API needs a breaking fallible conversion

Mitigation:

- Prefer adding fallible `Client::try_into_runtime()` first if runtime construction can fail.
- Keep `into_runtime()` only on `Client` if it can preserve correct semantics without hidden second context creation.
- Document the decision in migration notes.

### Risk 3: Incremental merge semantics changes subtle behavior

Mitigation:

- Add regression tests for quotes, positions, orders, trades, `trade_price`, and `_epoch`.
- Land changed-path tracking before changing derivation behavior.

### Risk 4: Cache lock split can introduce duplicate downloads

Mitigation:

- Recompute missing ranges after reacquiring lock.
- Accept duplicate download as a safe race if only one writer commits.
- Prefer correctness over perfect network efficiency.

### Risk 5: Watcher governance breaks existing internal waiters

Mitigation:

- Keep `watch(path)` compatible.
- Add new guard API instead of deleting old API immediately.
- Use conservative full-queue threshold.

---

## Completion Definition

This iteration is complete when:

- `Client` direct live APIs and runtime live adapter share one private live context owner.
- `close()` invalidates all live wait paths consistently.
- Account/auth changes are documented as creating a fresh `Client`; market mode switching is documented as context replacement, not field mutation.
- Market hot path avoids avoidable whole-payload clone, unbounded sync spawn fan-out, and full trade derivation on quote-only merges.
- Reconnect timer is no longer global across all websocket instances.
- Target position tasks do not silently retain dead worker handles.
- Watchers and trade dedup state have bounded lifecycle behavior.
- Docs, examples, `AGENTS.md`, and `CLAUDE.md` match the final public contract.
- Full verification matrix passes without live network credentials.
