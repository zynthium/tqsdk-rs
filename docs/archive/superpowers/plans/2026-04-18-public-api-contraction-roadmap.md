# Public API Contraction Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把当前 `tqsdk-rs` public API 审查发现整理成一条可执行的收敛路线，明确哪些问题先用 non-breaking 方式修补，哪些需要进入下一轮 breaking slice，哪些应先通过文档和迁移说明收口认知。

**Architecture:** 先修复会直接误导调用者或造成行为陷阱的 contract 问题，再通过 additive API 为未来 breaking 迁移铺路，最后集中处理真正影响 semver 的 surface 压缩。整个路线保持 `Client` / `ReplaySession` / `TradeSession` / `TqRuntime` 四条主路径不变，不引入新的主入口对象。

**Tech Stack:** Rust 2024, `tokio`, `thiserror`, `tracing`, `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo check --examples`

---

## Scope Inputs

- Review findings captured on `2026-04-18`
- Existing implementation plans:
  - `docs/superpowers/plans/2026-04-17-public-api-consolidation.md`
  - `docs/superpowers/plans/2026-04-17-public-api-repair.md`
- Canonical docs:
  - `README.md`
  - `AGENTS.md`
  - `CLAUDE.md`
- Public surface roots:
  - `src/lib.rs`
  - `src/prelude.rs`
  - `src/client/mod.rs`
  - `src/client/facade.rs`
  - `src/replay/mod.rs`
  - `src/runtime/mod.rs`
  - `src/trade_session/mod.rs`
  - `src/types/mod.rs`

## File Map

- `src/client/mod.rs`: live session owner 的 public contract，重点关注 market 初始化前后的行为
- `src/client/facade.rs`: live query/series/trade/replay 主 facade
- `src/client/builder.rs`: runtime / trade session 构造收口
- `src/marketdata/mod.rs`: market refs、`MarketDataUpdates` 以及隐藏类型泄漏
- `src/trade_session/ops.rs`: snapshot getter、connect lifecycle、下单 readiness
- `src/trade_session/events.rs`: 事件流 item typing
- `src/runtime/account.rs`: live runtime 手工下单/事件消费入口
- `src/runtime/execution.rs`: runtime 到 live trade session 的桥接
- `src/download/mod.rs`: request/target 模型耦合
- `src/datamanager/watch.rs`: watcher ownership 与 legacy `unwatch(path)` 语义
- `src/types/trading.rs`: stringly-typed order request 与残留 legacy types
- `src/utils.rs`, `src/cache/mod.rs`, `src/cache/data_series.rs`: 非 canonical surface 的外溢
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `docs/migration-remove-legacy-compat.md`: 文档与迁移约束同步

## Findings To Roadmap Mapping

### Docs-First

- `Client::quote/kline_ref/tick_ref/wait_update` 在未 `init_market()` 时的半可用语义需要先写清楚，避免用户把 blocking 行为误解为网络慢。
- `build_runtime()` / `into_runtime()` 的 live trade contract 需要先写清楚当前限制，避免 README 把“可交易 runtime”讲成已闭合能力。
- `DataManager::watch_handle()` 已是推荐路径，`unwatch(path)` 需要降级为 legacy 说明。
- `QuoteRef::load()` / `KlineRef::load()` / `TickRef::load()` 已是 legacy convenience API，需要在 README 和 agent 文档继续降权。

### Non-Breaking

- 通过 additive API 或 behavior-tightening 修补用户会直接踩到的问题。
- 给 breaking slice 预埋新入口、typed wrapper 和迁移说明。

### Breaking

- 压缩不该继续承诺的 public surface。
- 让 canonical API 和真实 semver contract 对齐。

## Phase 0: Docs-First Guardrail Pass

### Task 1: 先把当前 contract 写对

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `docs/migration-remove-legacy-compat.md`

- [ ] **Step 1: 更新 live market 初始化说明**

在 `README.md` 的 `Client::builder(...).build()` / `init_market()` 附近补三条明确说明：

- `quote()` / `kline_ref()` / `tick_ref()` 当前会返回句柄，但未初始化前不保证有更新
- 如需 fail-fast，优先 `try_quote()` / `try_kline_ref()` / `try_tick_ref()`
- `client.wait_update()` 当前仅在 `init_market()` 后才属于 canonical 用法

- [ ] **Step 2: 更新 live runtime 交易边界**

在 `README.md` 的 runtime 段落补充：

- `build_runtime()` / `into_runtime()` 当前稳定承诺的是 runtime market + target-pos orchestration
- live trade session 若要作为 runtime execution backend 使用，必须满足显式连接语义；当前 crate 尚未把该路径完全收口成单一推荐入口

- [ ] **Step 3: 同步 agent memory**

把下列规则同步到 `AGENTS.md` 与 `CLAUDE.md`：

- `build_runtime()` 不是“默认可直接发单的 live trading runtime”同义词
- market refs 的 canonical 读取路径是 `try_load()` / `snapshot()` / `is_ready()`
- `DataManager::watch_handle()` 是 canonical watcher path；`unwatch(path)` 属于 legacy surface

- [ ] **Step 4: 在迁移文档登记 legacy surface**

在 `docs/migration-remove-legacy-compat.md` 新增待迁移条目：

- market ref legacy `load()`
- `DataManager::unwatch(path)`
- stringly-typed `InsertOrderRequest`
- `ClientOption`
- `NotifyEvent`
- `PositionUpdate`

- [ ] **Step 5: 文档验证**

Run:

```bash
rg -n "build_runtime|into_runtime|init_market|try_quote|watch_handle|unwatch" README.md AGENTS.md CLAUDE.md docs/migration-remove-legacy-compat.md
```

Expected:

- 上述关键路径在四份文档中的表述一致

## Phase 1: Non-Breaking Repair Slice

### Task 2: 修复 live runtime trade 不可达，但先不 breaking

**Files:**
- Modify: `src/client/builder.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/runtime/account.rs`
- Modify: `README.md`
- Test: `src/client/tests.rs`

- [ ] **Step 1: 新增一个显式的 connected runtime 构造入口**

新增 additive API，二选一实现，但只选一种：

- `ClientBuilder::build_connected_runtime() -> Result<Arc<TqRuntime>>`
- `Client::into_runtime_and_connect_trades() -> Result<Arc<TqRuntime>>`

要求：

- 只自动连接 builder 预配置的 trade sessions
- 若某个 trade session 连接失败，整个构造失败
- 不改变现有 `build_runtime()` / `into_runtime()` 语义

- [ ] **Step 2: 写测试锁定 additive contract**

在 `src/client/tests.rs` 增加：

- 预配置 trade session 时，新的 connected runtime builder 会尝试连接 session
- 不预配置 trade session 时，connected runtime 构造仍成功
- 旧 `build_runtime()` 仍保持现状，不做隐式连接

- [ ] **Step 3: README 只推荐新的 connected runtime 路径**

把 live runtime trade 示例改成新 API，保留旧 API 为 market/runtime orchestration 用法。

- [ ] **Step 4: 验证**

Run:

```bash
cargo test builder_can_preconfigure_trade_sessions_for_runtime -- --nocapture
cargo test runtime_picks_up_trade_sessions_registered_after_creation -- --nocapture
cargo test --lib client::tests
```

Expected:

- 旧测试不回归
- 新测试证明 live runtime trade 有显式可达路径

### Task 3: 为 market 初始化前的半可用行为提供 additive escape hatch

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/tests.rs`
- Modify: `README.md`

- [ ] **Step 1: 不改旧签名，新增显式检查 helper**

新增 additive helper，至少覆盖：

- `Client::market_is_initialized() -> bool`
- `Client::check_market_initialized(capability: &'static str) -> Result<()>`

可选补充：

- `Client::try_wait_update() -> Result<()>`

- [ ] **Step 2: 测试新 helper**

在 `src/client/tests.rs` 增加：

- inactive client 时 `market_is_initialized()` 为 `false`
- `check_market_initialized("...")` 返回 `MarketNotInitialized`
- close 后返回 `ClientClosed`

- [ ] **Step 3: README 改写等待更新示例**

所有 live example 用法统一为：

- `init_market()`
- `try_quote()` 或 `quote() + try_load()`
- `wait_update_and_drain()` 仅出现在 `init_market()` 之后

### Task 4: 用 additive typed surface 为 stringly API 铺迁移路

**Files:**
- Modify: `src/types/trading.rs`
- Modify: `src/trade_session/ops.rs`
- Modify: `src/client/facade.rs`
- Modify: `README.md`
- Test: `src/trade_session/tests.rs`

- [ ] **Step 1: 为下单方向/开平/价格类型增加 typed enums**

新增但不替换旧字段：

- `OrderSide`
- `OrderOffset`
- `OrderPriceType`

- [ ] **Step 2: 新增 typed request builder**

新增 additive 类型：

- `InsertOrderSpec`

要求：

- 能从 typed enums 构造
- 内部可转换到旧 `InsertOrderRequest`
- 非法状态尽量在构造期排除

- [ ] **Step 3: 为 symbol info / query 设计 typed wrapper，而不是直接改旧 API**

至少新增：

- `Client::query_symbol_info_typed(...) -> Result<Vec<Quote>>` 或专门 metadata DTO
- 一个 query options struct，用于替代一串 `Option<_>` 位置参数

- [ ] **Step 4: README 主路径切换到 typed wrapper**

旧 stringly API 保留，但从 canonical 示例中移除。

### Task 5: 拆掉 downloader request/target 耦合的桥接层

**Files:**
- Modify: `src/download/mod.rs`
- Modify: `src/client/facade.rs`
- Test: `src/download/tests.rs`

- [ ] **Step 1: 新增 target-aware additive API**

新增一种不要求 `csv_file` 的 writer 路径，例如：

- `DataDownloadSpec` 只描述 symbols/time window
- `spawn_data_downloader_to_writer_spec(...)`

或：

- `DataDownloadRequest::for_writer(...)`

要求旧 API 继续保留。

- [ ] **Step 2: 测试 writer 模式不再依赖伪造路径**

新增测试，证明 writer download 不需要填一个无意义的 `csv_file`。

### Task 6: 给 filtered trade stream 补 typed recv

**Files:**
- Modify: `src/trade_session/events.rs`
- Modify: `src/trade_session/mod.rs`
- Test: `src/trade_session/tests.rs`

- [ ] **Step 1: 保留旧 `recv()`，新增 typed `recv_order()` / `recv_trade()`**

要求：

- `OrderEventStream::recv_order()` 返回 `Result<(String, Order), TradeEventRecvError>` 或等价 typed payload
- `TradeOnlyEventStream::recv_trade()` 返回 `Result<(String, Trade), TradeEventRecvError>`

- [ ] **Step 2: README 事件流示例切到 typed recv**

旧 `recv()` 保留为兼容路径，不再当 canonical 示例。

## Phase 2: Breaking API Contraction Slice

### Task 7: 真正收口 market ref 和 updates contract

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `src/marketdata/mod.rs`
- Modify: `src/client/mod.rs`
- Test: `src/client/tests.rs`
- Test: `src/prelude.rs`

- [ ] **Step 1: 在 breaking slice 二选一**

方案 A：

- 正式把 `marketdata` 变成稳定 public module
- 文档化 `SymbolId` / `KlineKey` / `MarketDataUpdates`

方案 B：

- 把 `MarketDataUpdates` 改成不依赖私有内部类型的新 DTO
- `quote()` / `kline_ref()` / `tick_ref()` 全部返回 root 可闭合的 public types

推荐：

- 若目标是继续压缩 internal machinery，选方案 B

- [ ] **Step 2: 让未初始化时的旧 convenience 入口 fail-fast**

breaking 目标：

- `Client::quote()` / `kline_ref()` / `tick_ref()` 返回 `Result<_>`
- `Client::wait_update()` 未初始化时直接报错，不再悬空等待

### Task 8: 压缩 public config surface

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/client/endpoints.rs`
- Modify: `src/replay/types.rs`
- Modify: `src/datamanager/mod.rs`
- Modify: `README.md`
- Test: `src/client/tests.rs`

- [ ] **Step 1: 将核心 config 改为 `#[non_exhaustive]` 或私有字段 + builder**

目标类型：

- `ClientConfig`
- `EndpointConfig`
- `TradeSessionOptions`
- `ReplayConfig`
- `DataManagerConfig`

- [ ] **Step 2: 让 `Default` 语义纯化**

breaking 方向：

- `EndpointConfig::default()` 不再读环境变量
- 保留 `from_env()` 作为显式入口

### Task 9: 收口 trade session 创建组合爆炸

**Files:**
- Modify: `src/client/builder.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/trade_session/mod.rs`
- Modify: `README.md`

- [ ] **Step 1: 引入唯一的 trade session config object / builder**

breaking 目标：

- builder 侧只保留 1 个入口
- client 侧只保留 1 个入口

- [ ] **Step 2: 把旧四组重载迁移到文档和迁移说明**

### Task 10: 同步压缩 non-canonical public modules

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/utils.rs`
- Modify: `src/cache/mod.rs`
- Modify: `src/types/mod.rs`
- Modify: `docs/migration-remove-legacy-compat.md`

- [ ] **Step 1: internalize 或 doc-hide 非 canonical surface**

优先处理：

- `utils`
- `cache`
- `ClientOption`
- `NotifyEvent`
- `PositionUpdate`

- [ ] **Step 2: 若暂时不能删除，先 root 不再鼓励，并在迁移文档中列出 removal window**

## Execution Order

1. Phase 0 docs-first
2. Phase 1 Task 2
3. Phase 1 Task 3
4. Phase 1 Task 4
5. Phase 1 Task 5
6. Phase 1 Task 6
7. Phase 2 Task 7
8. Phase 2 Task 8
9. Phase 2 Task 9
10. Phase 2 Task 10

## Risk Notes

- live runtime trade reachability如果直接修改 `build_runtime()` 语义，风险较高；本路线先走 additive API。
- market ref fail-fast 若直接修改旧签名，会影响 README、examples、tests 与下游用户；因此先 docs-first，再进入 breaking slice。
- `MarketDataUpdates` 的 hidden type leak 必须在下一轮 breaking 里彻底收口，否则 root contract 仍不闭合。
- `EndpointConfig::default()` 去环境副作用属于 semver-sensitive 变更，不应混入 non-breaking repair。

## Verification Matrix

- Docs-first only:

```bash
rg -n "build_runtime|into_runtime|init_market|try_quote|watch_handle|unwatch" README.md AGENTS.md CLAUDE.md docs/migration-remove-legacy-compat.md
```

- Non-breaking repairs:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo check --examples
```

- Breaking slices touching root/public exports:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo check --examples
cargo build --features polars
```

## Relationship To Existing Plans

- `2026-04-17-public-api-consolidation.md`:
  关注 root/prelude/export closure，本路线把它放进更大的 API 收敛排序里。
- `2026-04-17-public-api-repair.md`:
  关注 non-breaking repair，本路线补上 runtime trade reachability、downloader coupling、filtered stream typing 和后续 breaking slice 的接法。

## Recommended Starting Slice

若只做一轮低风险改动，建议从下面四项开始：

1. docs-first sync
2. additive connected runtime entry
3. market initialization helper
4. typed filtered trade recv

这四项都能改善外部体验，同时不会立刻扩大 breaking 面。
