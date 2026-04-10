# TqSdk-Rs 彻底移除历史包袱与重构计划

## 1. Summary (概述)
本次重构的目的是“褪去旧版本的影子，更新大版本甩掉历史包袱，重新启航”，同时对照 Python 版 `tqsdk`，**保留其核心能力（多合约对齐、K 线/Tick 序列、DataFrame 转换、TargetPosTask 等），但以纯粹 Rusty、极高性能的状态驱动（State-driven）模式实现**。

我们将完全移除所有为了兼容 Python 习惯而保留的冗余结构（如旧版 mock 回测、基于闭包的底层回调、易丢失消息的推送式行情通道）。通过拥抱 `tokio::sync::watch` 机制，不仅让代码结构更加符合 Rust 的并发最佳实践，还能大幅精简 `DataManager` 的内部复杂度。

## 2. Current State Analysis (当前状态分析)
目前代码库中仍然存在以下历史包袱与非 Rusty 的设计：
1. **老旧回测引擎**：`src/backtest/` 下保留了基于 JSON Diff 模拟合并的旧版回测逻辑，这已被 `src/replay/` 高性能回放引擎取代。
2. **闭包注册器（非 Rusty）**：`DataManager` 依赖 `on_data_register` 和内部的锁列表来触发更新通知，这是老旧的事件驱动设计，完全可以被 `tokio::sync::watch` 替代。
3. **老旧的推送式订阅接口（性能瓶颈）**：`QuoteSubscription` 和 `SeriesSubscription` 仍然保留了 `on_quote`、`quote_channel`、`on_update`、`data_stream` 等通道分发接口。这不仅增加了并发开销，还经常导致通道满时的丢弃警告，违背了最新 `TqApi` 的状态轮询设计。
4. **Python 兼容包装器**：`src/compat/` 下的 `TargetPosTask` 是为了模仿 Python 构造函数而保留的包装器，阻碍了用户使用原生的 Builder API。

## 3. Proposed Changes (建议的修改)

### 3.1 彻底删除废弃的模块与适配器
*   **删除 `src/backtest/` 目录**：完全移除 `BacktestHandle` 相关的旧版回测代码。
*   **删除 `src/compat/` 目录**：移除 Python 兼容层。为保留 Python 用户的核心概念认知，将 `src/runtime/tasks/target_pos.rs` 中的 `TargetPosHandle` 重命名为 `TargetPosTask`，但只暴露 Rusty 的 Builder API（`runtime.account("").target_pos("").build()`）。
*   **清理 `src/client/`**：删除 `init_market_backtest` 与 `switch_to_backtest` 方法。
*   **清理 `src/runtime/execution.rs`**：完全删除 `BacktestExecutionAdapter` 和 `BacktestExecutionState`。

### 3.2 重构 DataManager 为 Watch 机制
*   **修改 `src/datamanager/core.rs` 和 `src/datamanager/merge.rs`**：
    *   移除 `DataCallbackEntry` 结构以及 `on_data_callbacks` 字段。
    *   彻底删除 `on_data`、`on_data_register`、`off_data` 方法。
    *   引入 `merge_tx: watch::Sender<i64>`，提供 `pub fn subscribe_merge(&self) -> watch::Receiver<i64>`。
    *   在 `merge_data` 结束时，统一调用 `self.merge_tx.send_replace(epoch)`。

### 3.3 重构 SeriesSubscription 为高性能状态驱动
**保留多合约对齐、序列数据与 DataFrame 转换的核心能力，但改用 Rusty 的轮询模式：**
*   **修改 `src/series/subscription.rs`**：
    *   删除所有回调注册与通道：`on_update`、`on_new_bar`、`on_bar_update`、`on_error`、`data_stream`。
    *   添加状态驱动接口：`pub async fn wait_update(&self) -> Result<()>` 和 `pub async fn load(&self) -> Arc<SeriesData>`。
    *   内部使用 `watch::Sender<Arc<SeriesData>>` 保存最新状态。
    *   `start_watching` 任务改为等待 `dm.subscribe_merge()`，构建完 `SeriesData` 后通过 `watch` 发送最新快照。

### 3.4 清理 QuoteSubscription
*   **修改 `src/quote/lifecycle.rs`**：删除 `on_quote`、`on_error`、`quote_channel` 方法。
*   **删除 `src/quote/watch.rs`**：彻底删除该文件。因为 `TqApi::quote()` 已经提供了完美的 `QuoteRef` 状态引用，`QuoteSubscription` 只需负责向服务器声明生命周期，不再需要内部的常驻分发任务。

### 3.5 适配 TradeSession 的内部监听
*   **修改 `src/trade_session/watch.rs`**：`TradeSession` 作为交易事件流（订单、成交不能丢弃），保留其 `order_channel` 等接口。但底层的 `start_watching` 改为监听 `dm.subscribe_merge()`，而不是注册 `on_data_register` 闭包。

### 3.6 修复和更新示例与文档
*   **更新 `examples/`**：
    *   更新 `datamanager.rs`：演示 `subscribe_merge()` 替代闭包。
    *   更新 `backtest.rs`、`pivot_point.rs`：使用 `TargetPosTask` 的 Builder 模式。
    *   更新 `history.rs` 等：展示 `sub.wait_update()` 和 `sub.load()` 的新版 Series 消费方式。
*   **清理 `README.md` 与 `WIKI.md`**：删除“兼容模式”段落，全面展示新一代纯正 Rust 风格的 `TqApi`、`SeriesSubscription` 和 `TargetPosTask` 接口。

## 4. Assumptions & Decisions (假设与决策)
*   **决策：区分状态与事件**：行情数据（Quote、Series）是状态，采用 `watch` + `load`（零积压、极高性能）；交易数据（Order、Trade）是事件，继续采用 `mpsc/broadcast` channel（确保不丢单）。这不仅符合 Rust 哲学，也是高性能量化系统的最佳实践。
*   **决策：保留核心能力**：虽然去除了 Python 兼容的语法糖，但 Python SDK 最强大的 `get_kline_serial`（多合约对齐、转 DataFrame）被完美保留在 `SeriesSubscription::load().to_dataframe()` 中。

## 5. Verification steps (验证步骤)
1. 运行 `cargo clippy --all-targets --all-features -- -D warnings`，确保删除闭包、通道、旧目录后无未解析引用。
2. 运行 `cargo build --features polars`，验证 `SeriesData` 转 DataFrame 的能力依旧完好。
3. 运行 `cargo test` 确保所有核心逻辑通过。
4. 运行 `cargo check --examples`，验证所有重构后的示例代码完全通过编译，特别是 `backtest.rs`。