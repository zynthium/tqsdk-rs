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
  负责目标持仓任务、调度器和 execution/market adapter 组合。

## Surface Mapping

| Legacy surface | Canonical replacement | Notes |
|---|---|---|
| `Client::init_market_backtest` | `Client::create_backtest_session` | 直接创建 `ReplaySession`，不再经过 `BacktestHandle` |
| `Client::switch_to_backtest` | `Client::close()` 后重建 `ReplaySession` | 行情订阅控制与回测 session 分离 |
| `BacktestHandle` | `ReplaySession` | `step()` / `finish()` / `runtime()` 是唯一推荐回测入口 |
| `compat::TargetPosTask` | `runtime.account(\"...\").target_pos(\"...\").build()` | Builder 是 canonical task 入口 |
| `compat::TargetPosScheduler` | `runtime.account(\"...\").target_pos_scheduler(\"...\").steps(...).build()` | 调度器同样走 Builder |
| `quote_channel` / `on_quote` | `client.tqapi().quote(symbol)` + `wait_update()` / `load()` | Quote 是最新状态，不是事件日志 |
| `DataManager::{on_data, on_data_register, off_data}` | `subscribe_epoch()` + `get_path_epoch()` + `watch/unwatch` | merge 通知改为 coalesced state signal，不再提供全局 callback plumbing |
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
- 已删除：legacy `BacktestHandle` 路径、`compat/` facade、Quote callback/channel fan-out、Series callback/stream fan-out、`DataManager` callback plumbing。
- 正在收敛：`BacktestExecutionAdapter` 残留。
- 约束：在 cleanup 完成前，不要为新代码新增 `BacktestHandle`、`on_quote`、`on_update`、`data_stream` 等依赖。
