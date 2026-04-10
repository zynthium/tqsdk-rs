# 迁移指南：收敛到 Canonical State API

本指南说明 `tqsdk-rs` 在 breaking cleanup 中保留哪些主路径、删除哪些 legacy surface，以及现有代码应该如何迁移。

## Canonical Public Model

清理后的公开模型收敛为五条主路径：

- `Client`
  负责认证、连接、订阅控制和 high-level session 构造。
- `TqApi`
  负责最新行情状态读取，例如 `quote()` / `kline()` / `tick()`。
- `SeriesSubscription`
  负责多合约对齐窗口、历史窗口和 `SeriesData` / DataFrame 转换。
- `ReplaySession`
  负责历史回放、回测推进、回测 runtime 和最终结果汇总。
- `TqRuntime`
  由 `ClientBuilder::build_runtime()`、`Client::into_runtime()` 或 `ReplaySession::runtime()`
  提供，负责目标持仓任务和调度器；底层 adapter / registry 装配不再对外暴露。

## Surface Mapping

| Legacy surface | Canonical replacement | Notes |
|---|---|---|
| `Client::init_market_backtest` | `Client::create_backtest_session` | 直接创建 `ReplaySession`，不再经过 `BacktestHandle` |
| `Client::switch_to_backtest` | `Client::close()` 后重建 `ReplaySession` | 行情订阅控制与回测 session 分离 |
| `BacktestHandle` | `ReplaySession` | `step()` / `finish()` / `runtime()` 是唯一推荐回测入口 |
| `replay::HistoricalSource` / `ReplaySession::from_source` | `Client::create_backtest_session` | 自定义 replay source 装配退回 crate 内部；公开回测入口统一收敛到 `Client` facade |
| `InstrumentMetadata` root/prelude export | 无 direct replacement | 该类型不再作为顶层公开 replay API 暴露；它属于内部回放装配细节 |
| `ReplaySeriesSession` / `session.series().kline(...)` | `ReplaySession::kline(...)` | 单方法 wrapper 被移除，ReplaySession 直接提供 kline 注册入口 |
| `ReplayQuoteHandle` / `ReplayStep` / `ReplayQuote` / `ReplayHandleId` / `BarState` / `DailySettlementLog` root/prelude export | `tqsdk_rs::replay::{...}` | replay 细节类型仍保留模块级 public，但不再占用 crate root / prelude |
| `runtime::BacktestExecutionAdapter` | 无 public replacement | 回测执行 adapter 收回为内部实现；回放请使用 `ReplaySession::runtime()` |
| `runtime::{ExecutionAdapter, MarketAdapter, LiveExecutionAdapter, LiveMarketAdapter, TaskRegistry, TaskId, ChildOrderRunner, ...}` | `ClientBuilder::build_runtime()` / `Client::into_runtime()` / `ReplaySession::runtime()` | runtime 装配层收口为内部实现，公开 API 聚焦 ready-to-use `TqRuntime` 与 Builder task |
| `TargetPosBuilder` / `TargetPosSchedulerBuilder` / `TargetPosSchedulerConfig` / `TargetPosExecutionReport` / `RuntimeMode` / `RuntimeError` / `PriceResolver` root/prelude export | `tqsdk_rs::runtime::{...}` | runtime 细节类型与显式 builder 名称保留在 `runtime` 命名空间，不再污染 crate root / prelude |
| `websocket::*` 和 raw constructor（`QuoteSubscription::new`、`SeriesAPI::new`、`InsAPI::new`、`TradeSession::new`） | `Client` / `ClientBuilder` / `TradeSession` factory methods | transport wiring 收回 crate 内部；公开连接入口统一走高层 facade |
| `compat::TargetPosTask` | `runtime.account(\"...\").target_pos(\"...\").build()` | Builder 是 canonical task 入口 |
| `compat::TargetPosScheduler` | `runtime.account(\"...\").target_pos_scheduler(\"...\").steps(...).build()` | 调度器同样走 Builder |
| `quote_channel` / `on_quote` | `client.tqapi().quote(symbol)` + `wait_update()` / `load()` | Quote 是最新状态，不是事件日志 |
| `DataManager::{on_data, on_data_register, off_data}` | `subscribe_epoch()` + `get_path_epoch()` + `watch/unwatch` | merge 通知改为 coalesced state signal，不再提供全局 callback plumbing |
| `MergeSemanticsConfig` root/prelude export | `tqsdk_rs::datamanager::MergeSemanticsConfig` | 进阶 merge 语义调优收回到 `datamanager` 命名空间，避免污染顶层入口 |
| `SeriesSubscription` callback / stream fan-out | `SeriesSubscription` snapshot / window state API | 迁移方向是 pull-model，不再新增 callback/channel 用法 |

## Quote Lifecycle Note

- `QuoteSubscription` 只负责向服务端声明订阅生命周期：`start()` / `add_symbols()` / `remove_symbols()` / `close()`。
- `QuoteRef` 是 `MarketDataState` 上的快照句柄，不拥有订阅本身。
- 关闭 `QuoteSubscription` 后，已有 `QuoteRef` 不会失效，只是状态停止继续推进。

## Series Snapshot Note

- `SeriesSubscription` 负责窗口态 K 线/Tick 订阅生命周期。
- `wait_update()` 返回 coalesced `SeriesSnapshot`，其中包含 `SeriesData`、`UpdateInfo` 和完成态 epoch。
- `load()` 返回当前最新 `SeriesData`；`snapshot()` 可直接读取完整快照。

## Backtest Migration

旧代码：

```rust
let backtest = client
    .init_market_backtest(BacktestConfig::new(start_dt, end_dt))
    .await?;

loop {
    match backtest.next().await? {
        BacktestEvent::Tick { .. } => {}
        BacktestEvent::Finished { .. } => break,
    }
}
```

新代码：

```rust
let mut session = client
    .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
    .await?;

while let Some(step) = session.step().await? {
    println!("replay dt={}", step.current_dt);
}

let result = session.finish().await?;
println!("trades={}", result.trades.len());
```

回放结束后的账户/持仓汇总直接读取
`BacktestResult::{final_accounts, final_positions}`，不要再通过 `runtime.execution()` 访问内部 execution adapter。

## Target Position Migration

旧代码：

```rust
let task = TargetPosTask::new(runtime, "SIM", "SHFE.rb2601", TargetPosTaskOptions::default()).await?;
task.set_target_volume(1)?;
```

新代码：

```rust
let account = runtime.account("SIM").expect("configured account should exist");
let task = account.target_pos("SHFE.rb2601").build()?;
task.set_target_volume(1)?;
```

调度器同理：

```rust
let scheduler = account
    .target_pos_scheduler("SHFE.rb2601")
    .steps(vec![TargetPosScheduleStep {
        interval: std::time::Duration::from_secs(10),
        target_volume: 1,
        price_mode: None,
    }])
    .build()?;
```

## Release Policy

- 这是 breaking cleanup，不保留长期 deprecated shim。
- 仍处于 `0.x` 阶段时，建议按下一 breaking minor 发布，例如从 `0.1.x` 升到 `0.2.0`。
- 删除 public surface 时，README、examples、`prelude`、迁移文档必须同步更新。

## Current Status

- 已落地：`ReplaySession` 已成为唯一推荐回测路径，`TradeSession` watcher 已迁到 `DataManager::subscribe_epoch()`。
- 已删除：legacy `BacktestHandle` 路径、`compat/` facade、Quote callback/channel fan-out、Series callback/stream fan-out、`DataManager` callback plumbing、`BacktestExecutionAdapter` public surface。
- 已收口：`ReplayExecutionAdapter` / `ReplayMarketAdapter` 只保留为 replay 内部实现，不再作为 public replacement 暴露。
- 已收口：`ReplayKernel`、`QuoteSynthesizer`、`SeriesStore`、`SimBroker` 等 replay 拼装件不再作为根级 public replay API 暴露。
- 已收口：`HistoricalSource` 与 `ReplaySession::from_source` 只保留为 crate 内部回放装配点，不再作为公开扩展入口暴露。
- 已收口：`TqRuntime::new/with_id`、runtime adapter/registry/planning types、以及 `TqRuntime::{market,execution,registry,engine}` 等装配接口都退回 crate 内部。
- 已收口：`websocket` transport 模块与 raw constructor 不再作为公开装配入口，连接生命周期统一走 `Client` / `TradeSession` / `ReplaySession`。
- 已收口：`ReplayQuoteHandle`、`ReplayStep`、`ReplayQuote`、`ReplayHandleId`、`BarState`、`DailySettlementLog` 与 `MergeSemanticsConfig` 不再从 crate root / prelude 直接导出；需要显式类型名时请从对应模块导入。
- 已收口：`TargetPosBuilder`、`TargetPosSchedulerBuilder`、`TargetPosSchedulerConfig`、`TargetPosExecutionReport`、`RuntimeMode`、`RuntimeError` 与 `PriceResolver` 不再从 crate root / prelude 直接导出；显式类型引用请走 `tqsdk_rs::runtime::{...}`。
- 约束：在 cleanup 完成前，不要为新代码新增 `BacktestHandle`、`on_quote`、`on_update`、`data_stream` 等依赖。
