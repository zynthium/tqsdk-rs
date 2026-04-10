# tqsdk-rs Legacy Cleanup And API Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在下一次 breaking release 中，把 `tqsdk-rs` 收敛为一套清晰、单一、可维护的公开模型：`Client` 负责连接与订阅控制，`TqApi` 负责实时状态读取，`SeriesSubscription` 负责对齐窗口/历史窗口状态，`ReplaySession` 负责回测，`TqRuntime` 负责任务执行；删除多余的 legacy public surface、兼容 facade 和通用回调管线。

**Architecture:** 不再把“订阅控制”和“数据读取”混在一个对象里。`QuoteSubscription` 收敛成纯生命周期控制句柄，行情读取统一走 `TqApi`。`SeriesSubscription` 继续存在，因为多合约对齐窗口、历史窗口和 DataFrame 转换无法由 `TqApi` 的单点 `QuoteRef/KlineRef/TickRef` 取代，但它改成快照式状态读取，不再暴露 callback/stream fan-out。回测只保留 `ReplaySession` 主路径；runtime 的真实任务类型拿回 `TargetPosTask` / `TargetPosScheduler` 的 canonical 名字，从而删除 `compat/`。

**Tech Stack:** Rust 2024, `tokio::sync::watch` / `broadcast` / bounded channel, existing `DataManager`, `MarketDataState`, `ReplaySession`, `TqRuntime`.

---

## Working Assumptions

- 这是一次明确的 breaking cleanup，而不是加 `deprecated` 的温和过渡。
- 按当前 crate 仍处于 `0.x` 的事实处理，这次 breaking release 应至少升级到下一 breaking minor，例如 `0.2.0`；除非维护者明确要求双版本过渡，否则不额外保留一整轮 deprecated shim。
- “彻底移除历史包袱”指删除重复接口和失焦的兼容层，不指推翻 `TqApi`、`ReplayKernel`、`SimBroker` 这些已经形成主路径的实现。
- `TradeSession` 不能被这份 cleanup 计划顺手收敛成 `watch` 式状态通知。根据 `tqsdk-python` 调研，公开交易主模型虽然是 `wait_update()` + live refs，但 `TargetPosTask` / `InsertOrderTask` 内部用非 `last_only` 的 `TqChan` 传递成交事件，并显式依赖“把本次委托的成交都送达”的语义；因此 Rust 侧必须把“交易状态快照”和“订单/成交事件”分开设计。
- `SeriesSubscription` 不是 legacy 垃圾桶，它承载的是“窗口态序列”能力；真正 redundant 的是它目前的 callback/channel public surface。

## Keep / Remove / Rename Matrix

### Keep As Canonical

- `Client`
- `Client::tqapi()`
- `marketdata::{TqApi, QuoteRef, KlineRef, TickRef}`
- `SeriesAPI`
- `SeriesSubscription` 的生命周期与窗口状态能力
- `ReplaySession`
- `TqRuntime`
- `TargetPosBuilder`
- `TargetPosSchedulerBuilder`

### Remove Entirely

- `src/backtest/`
- `Client::init_market_backtest`
- `Client::switch_to_backtest`
- `DataManager::{on_data, on_data_register, off_data}`
- `QuoteSubscription::{quote_channel, on_quote, on_error}`
- `SeriesSubscription::{on_update, on_new_bar, on_bar_update, on_error, data_stream}`
- `src/compat/`
- `pub use runtime::BacktestExecutionAdapter`
- `pub use backtest::{BacktestConfig, BacktestEvent, BacktestHandle, BacktestTime}`

### Rename / Consolidate

- `runtime::TargetPosHandle` -> `runtime::TargetPosTask`
- `runtime::TargetPosSchedulerHandle` -> `runtime::TargetPosScheduler`
- `compat::TargetPosTask` 删除，调用方直接使用 runtime 主类型
- `compat::TargetPosScheduler` 删除，调用方直接使用 runtime 主类型

## Non-Goals

- 不把 `SeriesSubscription` 粗暴替换成 `TqApi` 单点引用。
- 不让 `QuoteSubscription` 再承担数据分发职责。
- 不公开 `ReplayExecutionAdapter` 作为新的 public replacement；它保持 replay 内部实现细节。
- 不顺手重写 `DataManager` merge 语义、prototype 语义或 websocket 重连完整性逻辑。

## Semantic Contracts To Preserve

- `QuoteSubscription` / `TqApi::quote` / `SeriesSubscription` 提供的是“最新状态快照”语义，不承诺保留每一个中间 tick。当前 callback/channel 实现本身已经带 bounded queue、coalescing 和 latest-state 读取特征；新 API 必须把这一点写清楚，而不是伪装成事件日志。
- `TradeSession` 不能退化成单一的 coalesced state/watch 模型。账户、持仓、委托、成交的“最新状态读取”可以走 epoch/snapshot；但订单状态变化和成交事件必须保留独立事件通路，不能混进 `watch` 或“只看最终状态”的 merge 通知里。
- `TqRuntime`、`TargetPosTask`、调度器和事件驱动策略如果要等待某笔单的变化，不能依赖会 overflow 丢弃的全局 `order_channel` / `trade_channel` 才能被唤醒；必须改成显式的可靠等待机制，例如 per-order waiter、带序号的事件日志，或其他不会把“最后一次关键成交”直接吞掉的实现。
- `ReplaySession::step()` 与 `ReplayStep.updated_handles` 必须原样保留。回测 stepping 的精确 handle-level 更新集合不属于 `DataManager` epoch 迁移范围，不能退化成“有人 merge 了”这种粗粒度通知。
- `SeriesData::to_dataframe()` 必须继续保留；新的 pull API 只是改变获取 `SeriesData` 的方式，不改变 Polars 转换入口。

## Task 1: Freeze The Target Public API

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Create: `docs/migration-remove-legacy-compat.md`

- [ ] 写一份明确的 public API 决策表，逐项列出 canonical surface、删除项、重命名项、不会保留 shim 的 breaking changes。
- [ ] 先更新文档中的“推荐路径”，把 `Client + TqApi + ReplaySession + TqRuntime` 定义为唯一主路径。
- [ ] 新建迁移文档，逐项覆盖：
  `BacktestHandle -> ReplaySession`
  `compat::TargetPosTask -> runtime::TargetPosTask`
  `quote_channel/on_quote -> TqApi::quote`
  `Series callback/stream -> SeriesSubscription snapshot API`
- [ ] 在迁移文档和 release note 草案中明确这次是 breaking release，并给出 crate 版本升级策略。
- [ ] 同步更新 `AGENTS.md`，把新的 canonical API、文档/示例同步义务和移除项写进去，避免后续 agent/贡献者继续沿用旧接口。
- [ ] 在这一任务完成前，不删除任何 public export；先把目标状态写清楚，避免中途重构方向漂移。

**Acceptance:**
- README 不再把 callback/channel/legacy backtest/compat facade 描述为等价推荐方案。
- 迁移文档可以让现有用户看懂“删了什么、为什么删、怎么改”。
- 文档明确说明 Quote/Series 是 snapshot API，而不是保序事件日志。

## Task 2: Remove The Legacy Backtest Control Path

**Files:**
- Delete: `src/backtest/mod.rs`
- Delete: `src/backtest/core.rs`
- Delete: `src/backtest/parsing.rs`
- Modify: `src/client/market.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `README.md`
- Modify: `examples/backtest.rs`
- Modify: `examples/pivot_point.rs`
- Modify: `src/runtime/tests.rs`
- Modify: `src/runtime/tasks/tests.rs`

- [ ] 先把所有示例、README、对外说明全部切到 `Client::create_backtest_session` + `ReplaySession`。
- [ ] 删除 `Client::init_market_backtest` 与 `Client::switch_to_backtest`，不再维持“双回测入口”。
- [ ] 删除 `src/backtest/` 模块和全部 re-export。
- [ ] 把仍依赖 `BacktestHandle` 的测试改为：
  1. 用 `ReplaySession` 做端到端回测测试；
  2. 或提取 test-only mock execution adapter，避免公开 legacy type 只是为了测试。
- [ ] 删除 README 中“Legacy BacktestHandle（兼容）”章节。

**Acceptance:**
- 仓库只剩一个回测 public surface：`ReplaySession`。
- README、examples、exports、tests 不再引用 `BacktestHandle`。

## Task 3: Replace Generic DataManager Callback Plumbing With Epoch Subscription

**Files:**
- Modify: `src/datamanager/mod.rs`
- Modify: `src/datamanager/core.rs`
- Modify: `src/datamanager/merge.rs`
- Modify: `src/datamanager/tests.rs`
- Modify: `examples/datamanager.rs`

- [ ] 在 `DataManager` 增加内部 epoch 订阅能力，建议命名为 `subscribe_epoch()`，返回 `watch::Receiver<i64>`。
- [ ] 保证 receiver 看到的 epoch 对应“merge 已完成、衍生字段已写回”的最终状态，而不是半成品状态。
- [ ] 为新机制补测试，覆盖：
  1. receiver 不会读到未完成 merge；
  2. 多次 merge 可被 coalesce，但不会丢失最终 epoch；
  3. 慢消费者仍能通过比较 epoch/路径 epoch 得到正确最终状态。
- [ ] 在内部消费者全部迁走之前，暂时保留旧 callback API；此阶段只引入新机制，不立刻删旧接口。
- [ ] 示例 `examples/datamanager.rs` 不再示范 `on_data`，改成示范 `watch/unwatch` 与 `subscribe_epoch()`。

**Acceptance:**
- `DataManager` 具备明确的 epoch 订阅机制。
- 没有任何新代码继续依赖 `on_data_register`。

## Task 4: Migrate Internal Consumers Off `on_data_register`

**Files:**
- Modify: `src/trade_session/watch.rs`
- Modify: `src/trade_session/core.rs`
- Modify: `src/series/subscription.rs`
- Modify: `src/quote/watch.rs` or delete it in Task 5
- Modify: `src/runtime/execution.rs`
- Modify: `src/trade_session/tests.rs`
- Modify: `src/series/tests.rs`

- [ ] `TradeSession` 改为监听 `DataManager` epoch receiver，再按路径 epoch 判定账户、持仓、委托、成交的最新状态是否变化。
- [ ] 不要把“监听 epoch”误当成交易事件模型的替代品。迁移时显式拆分：
  1. 账户/持仓/订单/成交最新状态的 snapshot 更新；
  2. 订单状态变化 / 成交事件的独立通知或等待机制。
- [ ] `runtime::LiveExecutionAdapter::wait_order_update()` 不再依赖可能 overflow 丢弃的全局 `order_channel` / `trade_channel` 才能苏醒；改成可靠的 per-order 等待路径，或者至少消费带序号、不丢关键变化的事件源。
- [ ] `SeriesSubscription` 改为监听 epoch receiver，而不是注册全局 callback。
- [ ] 把 worker dirty/running 去重逻辑保留在消费者侧；`watch` 只负责“有新 epoch”通知，不替代现有增量过滤逻辑。
- [ ] 迁移完成后，删除 `data_cb_id`、注销逻辑和旧 callback 依赖。
- [ ] 当所有内部消费者都脱离 `on_data_register` 后，再删除 `DataManager::{on_data, on_data_register, off_data}` 及相关测试。

**Acceptance:**
- `src/` 中不再有 `on_data_register` / `off_data` 调用。
- `DataManager` 的 public API 只保留路径查询、watch/unwatch、epoch 读取/订阅等状态能力。
- `TradeSession` 的状态更新路径与交易事件路径已经分离；runtime 不再把“bounded 全局通知通道”当作订单完成性的可靠依据。

## Task 5: Collapse Quote Into Lifecycle-Only Subscription + `TqApi` Data Reads

**Files:**
- Modify: `src/quote/mod.rs`
- Modify: `src/quote/lifecycle.rs`
- Delete: `src/quote/watch.rs`
- Modify: `src/quote/tests.rs`
- Modify: `src/client/tests.rs`
- Modify: `README.md`
- Modify: `examples/quote.rs`

- [ ] 保留 `QuoteSubscription::{start, add_symbols, remove_symbols, close}`，只让它负责向服务端声明订阅生命周期。
- [ ] 删除 `quote_channel`、`on_quote`、`on_error` 以及与之对应的内部队列和 watcher worker。
- [ ] 把所有数据消费示例、测试改成：
  1. `client.subscribe_quote(...).start().await?` 发起订阅；
  2. `client.tqapi().quote(symbol)` 读取最新状态。
- [ ] 保留“显式 `start()`”语义；不要把订阅控制隐式塞回 `TqApi`。
- [ ] 在迁移文档中明确 `QuoteRef` 与 `QuoteSubscription` 的关系：
  `QuoteRef` 是对 `MarketDataState` 的快照句柄，不拥有订阅；订阅关闭后它不会悬空，只是状态停止推进。

**Acceptance:**
- `QuoteSubscription` 不再承担任何数据 fan-out。
- README 中 Quote 的唯一读取方式是 `TqApi::quote` / `wait_update` / `load`。
- 生命周期语义已写清楚，用户不会把 `QuoteRef` 误解成“拥有订阅的句柄”。

## Task 6: Refactor Series Into Snapshot-Based Window State

**Files:**
- Modify: `src/types/series.rs`
- Modify: `src/series/mod.rs`
- Modify: `src/series/subscription.rs`
- Modify: `src/series/api.rs`
- Modify: `src/series/tests.rs`
- Modify: `README.md`
- Modify: `examples/history.rs`

- [ ] 为 `SeriesSubscription` 定义新的 pull-model public surface，建议新增：
  `SeriesSnapshot { data: Arc<SeriesData>, update: Arc<UpdateInfo>, epoch: i64 }`
  `wait_update(&self) -> Result<SeriesSnapshot>`
  `snapshot(&self) -> Option<SeriesSnapshot>`
- [ ] 删除 `on_update`、`on_new_bar`、`on_bar_update`、`on_error`、`data_stream`。
- [ ] 保留 `start`、`refresh`、`update_focus`、`close`，因为它们承担的是图表与历史窗口控制，而不是 legacy fan-out。
- [ ] `SeriesAPI::fetch_history_page_with_subscription` 改成等待 `wait_update()`，直到拿到 `chart_ready` 的 snapshot，而不是注册临时回调。
- [ ] 显式保留 `SeriesData` 的 Polars 转换链路，文档和示例改为：
  `let snapshot = sub.wait_update().await?;`
  `let df = snapshot.data.to_dataframe()?;`
- [ ] 更新 README 和 `examples/history.rs`，用 snapshot 模型演示历史窗口完成与多合约对齐窗口读取。

**Acceptance:**
- `SeriesSubscription` 不再暴露 callback/stream，但仍完整保留窗口态序列能力。
- `TqApi` 继续服务于 latest quote/latest bar/latest tick，`SeriesSubscription` 服务于 aligned/history window，两者边界清晰。
- Polars 用户仍能直接从 `SeriesData` 做 DataFrame 转换，迁移路径明确。
- 文档明确说明 `SeriesSnapshot` 是 coalesced state update，而不是逐条 tick 事件。

## Task 7: Remove `compat/` By Making Runtime Types Canonical

**Files:**
- Delete: `src/compat/mod.rs`
- Delete: `src/compat/target_pos_task.rs`
- Delete: `src/compat/target_pos_scheduler.rs`
- Modify: `src/runtime/tasks/target_pos.rs`
- Modify: `src/runtime/tasks/scheduler.rs`
- Modify: `src/runtime/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `src/runtime/tests.rs`
- Modify: `src/replay/tests.rs`
- Modify: `examples/backtest.rs`
- Modify: `examples/pivot_point.rs`
- Modify: `examples/trade.rs`
- Modify: `README.md`

- [ ] 把 runtime 主类型重命名为公开语义正确的名字：
  `TargetPosHandle -> TargetPosTask`
  `TargetPosSchedulerHandle -> TargetPosScheduler`
- [ ] 保留 builder 名称 `TargetPosBuilder` / `TargetPosSchedulerBuilder`，因为 builder 本身不是冗余层。
- [ ] 删除 `compat::TargetPosTask` / `compat::TargetPosScheduler` 包装器。
- [ ] 把所有示例、README、tests 改为直接使用 runtime canonical types。
- [ ] 如果需要减少 rename 噪音，可先在 runtime 内部完成 type rename，再统一改 export；不要保留第二套 facade。

**Acceptance:**
- 仓库只存在一套 `TargetPosTask` / `TargetPosScheduler` 类型。
- 用户不再需要理解 “builder 输出是 handle，compat 再包一层 task” 这种双重命名。

## Task 8: Final Removal Sweep And Release Readiness

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/architecture.md`
- Modify: `docs/migration-marketdata-state.md`
- Modify: `examples/*.rs` (按实际引用更新)
- Modify: `Cargo.toml` if example metadata needs cleanup

- [ ] 清理所有对以下概念的残余引用：
  `BacktestHandle`
  `BacktestExecutionAdapter`
  `compat::`
  `quote_channel`
  `on_quote`
  `on_update`
  `data_stream`
  `init_market_backtest`
  `switch_to_backtest`
- [ ] 补一份 breaking release 说明，列出删除项、替代项和迁移示例。
- [ ] 升级 crate 版本到下一 breaking minor，并在 release 说明中明确这次删除的是旧 public surface，而不是内部小修。
- [ ] 核对 `AGENTS.md`、`README`、`prelude`、examples、`lib.rs` re-export 保持一致。
- [ ] 如果 examples 中仍有多种写法，只保留 canonical 写法，不再展示 legacy 兼容入口。

**Acceptance:**
- `rg` 搜索不到已删除 surface 的剩余公开文档和示例引用。
- `AGENTS.md` 不再指导 agent 或贡献者使用已删除的 legacy API，并明确要求 public API 变更时同步更新 examples。
- 仓库叙事从“支持多种风格”收敛为“有一条推荐主路径，其他旧路已删除”。

## Verification Matrix

### Per-Phase Minimum

- Task 2: `cargo test backtest replay runtime -- --nocapture` 或等价的目标测试集
- Task 3-4: `cargo test datamanager trade_session series`
- Task 4: 额外增加针对 `runtime::LiveExecutionAdapter::wait_order_update` 的测试，覆盖“队列拥塞时仍能等到目标 order/trade 变化”。
- Task 5: `cargo test quote client`
- Task 6: `cargo test series`
- Task 7: `cargo test runtime replay`
- Task 8: `cargo check --examples`

### Full Gate Before Merge

- [ ] `cargo fmt --check`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo build --features polars`
- [ ] `cargo check --examples`

## Reviewer Checklist

- `SeriesSubscription` 是否仍然保留了多合约对齐窗口、历史窗口、DataFrame 转换能力，而不是被错误简化成 latest-bar wrapper。
- `QuoteSubscription` 是否已经彻底退出数据分发职责。
- `ReplaySession` 是否成为唯一回测入口，README/examples/tests 是否一致。
- `TargetPosTask` / `TargetPosScheduler` 是否只剩一套 canonical 类型。
- `DataManager` 是否已经不再暴露通用 callback 注册 API。
- 文档、exports、prelude、examples 是否已经完全同步。
