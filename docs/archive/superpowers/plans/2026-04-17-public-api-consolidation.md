# Public API Consolidation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 收口 `tqsdk-rs` 的公开接口，使 crate root、`prelude`、README canonical 路径与实际 public contract 完全一致，并消除隐藏类型泄漏、假 public surface 与构造期全局副作用。

**Architecture:** 先修补 public contract 不闭合问题，再删除或内部化不应继续承诺的 advanced surface，最后同步 README / 示例 / agent 文档。整个改造保持现有 `Client` / `TradeSession` / `ReplaySession` / `TqRuntime` 四条主路径不变，不重写底层 live/replay/runtime 架构。

**Tech Stack:** Rust 2024, `tokio`, `tracing`, `cargo test`, `cargo clippy`, `cargo check --examples`

---

## Scope Inputs

- Spec: `docs/superpowers/specs/2026-04-17-public-api-consolidation-design.md`
- Root export files: `src/lib.rs`, `src/prelude.rs`
- Live API files: `src/client/mod.rs`, `src/client/facade.rs`, `src/client/builder.rs`
- Internal market state files: `src/marketdata/mod.rs`
- Advanced module files: `src/series/mod.rs`, `src/series/api.rs`, `src/ins/mod.rs`, `src/ins/query.rs`, `src/replay/mod.rs`, `src/runtime/mod.rs`
- Trade event files: `src/trade_session/mod.rs`, `src/trade_session/events.rs`
- Docs and examples: `README.md`, `examples/`, `AGENTS.md`, `CLAUDE.md`, `docs/migration-remove-legacy-compat.md`

## File Map

- `src/lib.rs`: 最终 canonical root surface 与 crate-level 示例
- `src/prelude.rs`: 高频主路径导入集合
- `src/client/mod.rs`: `Client` 的对外签名，重点排查 hidden type leak
- `src/client/facade.rs`: live/replay/trade facade 对外入口
- `src/client/builder.rs`: 去除构造期全局日志副作用
- `src/marketdata/mod.rs`: 若仍需保留 `QuoteRef` / `KlineRef` / `TickRef` / `MarketDataUpdates`，则提升为稳定 public contract；否则改写上层签名避开它们
- `src/trade_session/mod.rs`: trade 模块对外 re-export 闭合
- `src/trade_session/events.rs`: `TradeSessionEvent` 事件本体保持稳定
- `src/series/mod.rs`: 判断 `SeriesAPI` 是否继续 public；默认收回内部
- `src/ins/mod.rs`: 判断 `InsAPI` 是否继续 public；默认收回内部
- `src/replay/mod.rs`: 清理多余 public surface，只保留稳定回放句柄与结果
- `src/runtime/mod.rs`: 清理多余 public surface，只保留稳定 runtime contract
- `README.md`: canonical API 叙事、日志初始化说明、advanced module 说明
- `examples/`: 确认示例只依赖 canonical path
- `docs/migration-remove-legacy-compat.md`: 记录 breaking / deprecated surface 迁移

## Chunk 1: Canonical Surface Inventory

### Task 1: 固化 root canonical 清单

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Reference: `README.md`
- Reference: `docs/superpowers/specs/2026-04-17-public-api-consolidation-design.md`

- [ ] **Step 1: 写一个 root surface 快照清单**

在 `src/lib.rs` 顶部旁注或临时草稿中列出最终 canonical 类型，确认至少包含：

```rust
pub use client::{Client, ClientBuilder, ClientConfig, EndpointConfig, TradeSessionOptions};
pub use errors::{Result, TqError};
pub use trade_session::{TradeSession, TradeSessionEvent, TradeSessionEventKind, TradeEventRecvError};
pub use replay::{ReplayConfig, ReplaySession};
pub use runtime::{AccountHandle, TqRuntime, TargetPosTask, TargetPosScheduler};
```

- [ ] **Step 2: 比对 README canonical 示例依赖**

Run:

```bash
grep -n "use tqsdk_rs" README.md
```

Expected:

- 能快速确认 README 主路径只依赖计划中的 canonical surface

- [ ] **Step 3: 精简 `src/prelude.rs`**

将 `prelude` 限定为高频主路径集合，补上遗漏的事件类型，避免 advanced surface 滥入：

```rust
pub use crate::{
    Client, ClientConfig, EndpointConfig, QuoteSubscription, SeriesSubscription,
    TradeSession, TradeSessionEvent, TradeSessionEventKind, TradeEventRecvError,
    ReplayConfig, ReplaySession, TqRuntime, AccountHandle,
    Result, TqError,
};
```

- [ ] **Step 4: 运行最小编译检查**

Run:

```bash
cargo check --lib
```

Expected:

- PASS
- 如果 root / prelude 少导出了实际被 public 签名引用的类型，会在此阶段暴露

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/prelude.rs
git commit -m "refactor(api): define canonical root and prelude surface"
```

### Task 2: 补齐 trade 事件 contract

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Reference: `src/trade_session/mod.rs`
- Reference: `src/trade_session/events.rs`

- [ ] **Step 1: 为事件流返回值补 root re-export**

确保 `TradeSessionEvent` 与其配套类型在 crate root 可命名：

```rust
pub use trade_session::{
    TradeSession, TradeSessionEvent, TradeSessionEventKind,
    TradeEventRecvError, TradeEventStream, OrderEventStream, TradeOnlyEventStream,
};
```

- [ ] **Step 2: 更新 `prelude`**

确保 `TradeSessionEvent` 进入 `prelude`，保证 README 推荐路径无需退回模块路径。

- [ ] **Step 3: 验证 README 中的事件流用法**

Run:

```bash
grep -n "subscribe_events\\|recv" README.md
```

Expected:

- README 中的事件流叙事与 root/prelude 导出一致

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/prelude.rs
git commit -m "fix(api): export trade session event contract"
```

## Chunk 2: Hidden Type Leak Repair

### Task 3: 移除 `cfg(clippy)` 可见性分叉

**Files:**
- Modify: `src/lib.rs`
- Reference: `src/marketdata/mod.rs`

- [ ] **Step 1: 删除可见性特判**

把当前：

```rust
#[cfg(clippy)]
pub mod marketdata;
#[cfg(not(clippy))]
pub(crate) mod marketdata;
```

改成一种真实发布态可见性，而不是随 lint 改变 public surface。

- [ ] **Step 2: 决定策略**

二选一：

```rust
// 方案 A：marketdata 彻底内部化
mod marketdata;

// 方案 B：若 QuoteRef / KlineRef / TickRef / MarketDataUpdates 被承诺为稳定 contract
pub mod marketdata;
```

Expected:

- 最终选择必须与 `Client` public 签名匹配

- [ ] **Step 3: 跑 contract lint**

Run:

```bash
cargo clippy --lib --all-features -- -D warnings -W private_bounds -W private_interfaces
```

Expected:

- PASS
- 不再依赖 `cfg(clippy)` 才通过 public API 检查

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs
git commit -m "fix(api): remove clippy-only public surface split"
```

### Task 4: 修复 `Client` live 主路径的签名闭合

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Possible Modify: `src/marketdata/mod.rs`
- Test: `src/client/tests.rs`

- [ ] **Step 1: 为 `Client` public 方法列签名清单**

重点检查：

```rust
pub fn quote(&self, symbol: impl Into<crate::marketdata::SymbolId>) -> QuoteRef
pub fn kline_ref(&self, symbol: impl Into<crate::marketdata::SymbolId>, duration: Duration) -> KlineRef
pub fn tick_ref(&self, symbol: impl Into<crate::marketdata::SymbolId>) -> TickRef
pub async fn wait_update_and_drain(&self) -> Result<MarketDataUpdates>
```

- [ ] **Step 2: 选择闭合方式**

推荐优先级：

1. 让 `QuoteRef` / `KlineRef` / `TickRef` / `MarketDataUpdates` 成为明确稳定 public type
2. 若 `SymbolId` 只是技术实现细节，则将参数收窄为 `&str` 或 `impl AsRef<str>`

目标示意：

```rust
pub fn quote(&self, symbol: impl AsRef<str>) -> QuoteRef
pub fn tick_ref(&self, symbol: impl AsRef<str>) -> TickRef
pub async fn wait_update_and_drain(&self) -> Result<MarketDataUpdates>
```

- [ ] **Step 3: 先写或更新测试，锁定 contract**

在 `src/client/tests.rs` 增加面向 public API 的编译/行为测试，至少覆盖：

```rust
#[tokio::test]
async fn client_public_quote_api_accepts_str_symbol() {
    // 目标：主路径不要求调用方感知内部 SymbolId
}
```

- [ ] **Step 4: 实现最小改动让测试通过**

只改对外签名和必要桥接，不重写 `marketdata` 内部存储模型。

- [ ] **Step 5: 跑聚焦测试**

Run:

```bash
cargo test client_public_quote_api_accepts_str_symbol -- --nocapture
```

Expected:

- PASS

- [ ] **Step 6: 跑全库编译与 lint**

Run:

```bash
cargo check --lib
cargo clippy --lib --all-features -- -D warnings -W private_bounds -W private_interfaces
```

Expected:

- PASS

- [ ] **Step 7: Commit**

```bash
git add src/client/mod.rs src/lib.rs src/prelude.rs src/client/tests.rs src/marketdata/mod.rs
git commit -m "refactor(api): close client live signature surface"
```

## Chunk 3: Advanced Surface Internalization

### Task 5: 内部化 `SeriesAPI`

**Files:**
- Modify: `src/series/mod.rs`
- Modify: `src/series/api.rs`
- Reference: `src/client/facade.rs`
- Test: `src/series/tests.rs`

- [ ] **Step 1: 先写一个失败检查**

确认 `Client::{get_kline_serial,get_tick_serial,get_kline_data_series,get_tick_data_series}` 仍是唯一 canonical 发起入口。

```rust
#[tokio::test]
async fn client_series_facade_remains_canonical() {
    // 目标：调用方无需直接持有 SeriesAPI
}
```

- [ ] **Step 2: 将 `SeriesAPI` 从 public type 改为内部类型**

目标形态：

```rust
pub(crate) struct SeriesAPI { ... }
```

同时保留：

```rust
pub struct SeriesSubscription { ... }
pub use crate::types::SeriesOptions;
```

- [ ] **Step 3: 跑聚焦测试**

Run:

```bash
cargo test client_series_facade_remains_canonical -- --nocapture
```

Expected:

- PASS

- [ ] **Step 4: Commit**

```bash
git add src/series/mod.rs src/series/api.rs src/series/tests.rs
git commit -m "refactor(series): internalize series api type"
```

### Task 6: 内部化 `InsAPI`

**Files:**
- Modify: `src/ins/mod.rs`
- Reference: `src/client/facade.rs`
- Test: `src/ins/tests.rs`

- [ ] **Step 1: 锁定 facade 用法**

新增或更新测试，证明查询仍通过 `Client` facade 使用：

```rust
#[tokio::test]
async fn client_query_facade_remains_public_entry() {
    // 目标：不要求调用方直接持有 InsAPI
}
```

- [ ] **Step 2: 收回类型可见性**

目标形态：

```rust
pub(crate) struct InsAPI { ... }
```

- [ ] **Step 3: 跑相关测试**

Run:

```bash
cargo test client_query_facade_remains_public_entry -- --nocapture
```

Expected:

- PASS

- [ ] **Step 4: Commit**

```bash
git add src/ins/mod.rs src/ins/tests.rs
git commit -m "refactor(ins): internalize ins api type"
```

### Task 7: 清理 `runtime` / `replay` 冗余 `pub use`

**Files:**
- Modify: `src/runtime/mod.rs`
- Modify: `src/replay/mod.rs`
- Reference: `README.md`
- Test: `src/runtime/tests.rs`
- Test: `src/replay/tests.rs`

- [ ] **Step 1: 列出当前 public use 清单**

重点检查是否仍有仅供内部协作的类型处于 public：

```rust
pub use account::AccountHandle;
pub use core::TqRuntime;
pub use errors::{RuntimeError, RuntimeResult};
pub use modes::RuntimeMode;
```

- [ ] **Step 2: 按 spec 收口**

保留稳定 contract，删除或降级内部件，确保 README 和示例仍能表达全部推荐路径。

- [ ] **Step 3: 运行模块测试**

Run:

```bash
cargo test runtime -- --nocapture
cargo test replay -- --nocapture
```

Expected:

- PASS

- [ ] **Step 4: Commit**

```bash
git add src/runtime/mod.rs src/replay/mod.rs
git commit -m "refactor(api): trim runtime and replay exports"
```

## Chunk 4: Explicit Logging and Docs Sync

### Task 8: 去除构造期日志副作用

**Files:**
- Modify: `src/client/builder.rs`
- Modify: `README.md`
- Modify: `examples/custom_logger.rs`
- Test: `src/client/tests.rs`

- [ ] **Step 1: 先补行为测试**

增加一条测试，锁定“构建客户端不应隐式初始化全局日志”的目标。

```rust
#[test]
fn client_builder_does_not_init_global_logger_implicitly() {
    // 目标：build 过程不调用 init_logger()
}
```

- [ ] **Step 2: 删除 `build()` 中的日志初始化**

删除：

```rust
crate::logger::init_logger(&self.config.log_level, true);
```

保留显式 API：

```rust
pub fn init_logger(level: &str, filter_crate_only: bool)
pub fn create_logger_layer<S>(...)
```

- [ ] **Step 3: README 补显式说明**

把日志说明写成：

```rust
use tqsdk_rs::init_logger;

init_logger("info", true);
let client = Client::builder(username, password).build().await?;
```

- [ ] **Step 4: 跑聚焦测试**

Run:

```bash
cargo test client_builder_does_not_init_global_logger_implicitly -- --nocapture
```

Expected:

- PASS

- [ ] **Step 5: Commit**

```bash
git add src/client/builder.rs README.md examples/custom_logger.rs src/client/tests.rs
git commit -m "fix(logger): make sdk logging opt-in"
```

### Task 9: 同步 README、示例、迁移文档与 agent 文档

**Files:**
- Modify: `README.md`
- Modify: `src/lib.rs`
- Modify: `examples/*.rs`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `skills/tqsdk-rs/` 下相关文档（如存在）

- [ ] **Step 1: 更新 README 的 Canonical API 章节**

明确三件事：

- root / `prelude` 是推荐主路径
- `SeriesAPI` / `InsAPI` 不再作为 public extension surface
- 日志需要显式初始化

- [ ] **Step 2: 更新 crate-level 文档示例**

让 `src/lib.rs` 顶层示例与 README 完全一致。

- [ ] **Step 3: 更新迁移文档**

至少写清：

- `TradeSessionEvent` 现在进入 root / `prelude`
- `SeriesAPI` / `InsAPI` 已不再公开
- 若旧代码依赖这些 type import，应改为 `Client` facade
- `ClientBuilder::build()` 不再隐式初始化日志

- [ ] **Step 4: 跑示例编译检查**

Run:

```bash
cargo check --examples
```

Expected:

- PASS

- [ ] **Step 5: Commit**

```bash
git add README.md src/lib.rs examples AGENTS.md CLAUDE.md docs/migration-remove-legacy-compat.md skills/tqsdk-rs
git commit -m "docs(api): align docs with consolidated public surface"
```

## Chunk 5: Final Verification

### Task 10: 跑完整验证矩阵并整理收尾

**Files:**
- Reference: 全仓库

- [ ] **Step 1: 运行测试**

Run:

```bash
cargo test
```

Expected:

- PASS

- [ ] **Step 2: 运行严格 lint**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected:

- PASS

- [ ] **Step 3: 运行格式检查**

Run:

```bash
cargo fmt --check
```

Expected:

- PASS

- [ ] **Step 4: 人工复核 contract**

检查项：

- README canonical 示例是否只依赖 canonical surface
- `prelude::*` 是否足够支撑推荐写法
- public 方法签名是否完全闭合
- 是否仍存在“public type 但拿不到实例”的模块类型

- [ ] **Step 5: 最终提交**

```bash
git status --short
git commit -m "refactor(api): consolidate public surface"
```

Expected:

- 工作区干净
- 提交信息准确反映最终改造结果

## Notes for Execution

- 优先做“小范围可验证”的 API 调整，不要一口气同时改 root export、README、测试和内部化多个模块后再找错误。
- 对 `marketdata` 的处理必须先定策略，再动 `Client` 签名；否则很容易在中途产生新的 contract 断裂。
- `SeriesAPI` / `InsAPI` 内部化后，要立刻跑 README 对应入口与示例编译检查，避免 facade 漏洞。
- 修改 public surface 后，任何文档与示例不同步都算未完成。
- 如果在执行过程中发现有下游兼容诉求无法接受当前收口幅度，应暂停并先回到 spec 层更新迁移策略。

Plan complete and saved to `docs/superpowers/plans/2026-04-17-public-api-consolidation.md`. Ready to execute?
