# 迁移指南：收敛到 Canonical State API

> Breaking cleanup target:
> 本文档记录的是下一轮破坏性升级要收敛到的 canonical public model。
> 某些删除项已经落地，某些仍在推进中；在所有切片完成前，请把这里视为目标接口形状，而不是当前代码的完整现状快照。

本指南说明 `tqsdk-rs` 在 breaking cleanup 中保留哪些主路径、删除哪些 legacy surface，以及 live API 将如何迁移到最终的 state-driven 模型。

## Canonical Public Model

breaking cleanup 完成后的公开模型收敛为四条主路径：

- `Client`
  负责 live 认证、连接、行情状态读取、序列订阅、查询和 high-level session 构造。
- `TradeSession`
  负责交易状态读取与可靠交易事件；账户/持仓属于状态，订单/成交/通知/异步错误属于事件。
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
| `prelude::*` 中的 `DataManager` / `InsAPI` / `SeriesAPI` / `TqApi` / `SeriesData` 等高级类型 | 显式 `use tqsdk_rs::{...}` 或对应模块路径 | prelude 聚焦常用 `Client` / `ReplaySession` / `TqRuntime` / `TradeSession` 主路径；`TradeSessionEvent` 与 `MarketDataUpdates` 仍保留在 root/prelude 便于直接消费，其余高级接口改为显式导入 |
| `TradeEventStream` / `OrderEventStream` / `TradeOnlyEventStream` / `TradeEventRecvError` / `TradeSessionEvent` root export | 优先 `tqsdk_rs::trade_session::{...}` | 交易可靠事件的显式 stream/event 类型推荐从 `trade_session` 命名空间导入；当前代码仍暂时保留 crate root / prelude re-export 以兼容现有调用方 |
| `InsAPI` / `SeriesAPI` / `SeriesCachePolicy` / `KlineKey` root export | `Client` facade / `SeriesSubscription` / `DataDownloader` | query/series 高级装配类型不再作为对外扩展点保留；下游统一改走 `Client` 主路径和直接消费句柄 |
| `tqsdk_rs::marketdata::{TqApi, MarketDataState, SymbolId}` | 无 direct replacement；外部统一走 `Client::{market_is_initialized,check_market_initialized,try_quote,try_kline_ref,try_tick_ref}`，其中 Quote 消费主路径是 `try_quote()` + `wait_update()` + `try_load()` | `marketdata` 已退回 crate 内部实现细节，避免形成第二套 live session 入口 |
| `Authenticator` / `ClientOption` / `BacktestResult` root/prelude export | `tqsdk_rs::{auth, client, replay}::{...}` | trait/type alias/result detail 继续保留模块级 public，但不再占用 crate root / prelude |
| `websocket::*` 和 raw constructor（`QuoteSubscription::new`、`SeriesAPI::new`、`InsAPI::new`、`TradeSession::new`） | `Client` / `ClientBuilder` / `TradeSession` factory methods | transport wiring 收回 crate 内部；公开连接入口统一走高层 facade |
| `compat::TargetPosTask` | `runtime.account(\"...\").target_pos(\"...\").build()` | Builder 是 canonical task 入口 |
| `compat::TargetPosScheduler` | `runtime.account(\"...\").target_pos_scheduler(\"...\").steps(...).build()` | 调度器同样走 Builder |
| `quote_channel` / `on_quote` | `client.try_quote(symbol)?` + `wait_update()` / `try_load()` | Quote 是最新状态，不是事件日志；`load()` 仅保留兼容语义 |
| `Client::market_state()` | `Client::{market_is_initialized,check_market_initialized,try_quote,try_kline_ref,try_tick_ref}`，其中 Quote 主路径是 `try_quote()` + `wait_update()` + `try_load()` | 不再暴露底层 `MarketDataState` 容器，也不再要求先取 `TqApi` facade |
| `Client::set_auth()` / `Client::get_auth()` | `Client::{auth_id,has_feature,check_md_grants}`；如需切账号，关闭旧 `Client` 后创建新 `Client` | 公开 API 不再暴露内部锁 guard 或运行时替换 auth 的入口 |
| `Client::tqapi()` | `Client::{market_is_initialized,check_market_initialized,try_quote,try_kline_ref,try_tick_ref}`，其中 Quote 主路径是 `try_quote()` + `wait_update()` + `try_load()` | live 市场状态入口收口到 `Client` |
| `Client::series()` | `Client::{get_kline_serial,get_tick_serial,get_kline_data_series,get_tick_data_series}` | live 序列订阅与历史快照下载入口收口到 `Client` |
| `Client::{kline_history,kline_history_with_focus}` | `Client::{get_kline_data_series,get_tick_data_series}` | 历史窗口 `set_chart` 协议退回内部实现，不再作为公开稳定接口 |
| `Client::ins()` | `Client` 上的 query facade | 合约与基础数据查询不再通过显式 `InsAPI` 逃生口公开 |
| `ClientBuilder::build()` 隐式初始化 tracing | 显式调用 `init_logger()` 或组合 `create_logger_layer()` | SDK 不再抢占宿主应用的全局 subscriber |
| `DataManager::{on_data, on_data_register, off_data}` | `subscribe_epoch()` + `get_path_epoch()` + `watch_handle()` | merge 通知改为 coalesced state signal，不再提供全局 callback plumbing |
| `MergeSemanticsConfig` root/prelude export | `tqsdk_rs::datamanager::MergeSemanticsConfig` | 进阶 merge 语义调优收回到 `datamanager` 命名空间，避免污染顶层入口 |
| `SeriesSubscription` callback / stream fan-out | `SeriesSubscription` snapshot / window state API | 迁移方向是 pull-model，不再新增 callback/channel 用法 |
| `TradeSession::{account_channel, position_channel}` | `wait_update()` + `get_account()` / `get_position()` / `get_positions()` | 账户与持仓属于最新状态读取，不再提供 best-effort snapshot channel |
| `TradeSession::{on_account, on_position}` | `wait_update()` + snapshot getters | 交易快照统一走 pull-model，不再保留状态回调 |
| `TradeSession::{on_notification, on_error, notification_channel}` | `subscribe_events()` | 通知与异步错误并入可靠事件流 |
| `connect()` + `while !session.is_ready()` | `connect()` + `wait_ready()` | `wait_ready()` 是进入 ready state 的 canonical gate；`is_ready()` 只保留瞬时状态检查 |
| `QuoteSubscription::start()` | `Client::subscribe_quote()` | Quote 订阅已改为创建即生效 |
| `SeriesSubscription::start()` | `Client::{get_kline_serial,get_tick_serial}` | Series 订阅已改为创建即启动；历史下载改为 one-shot API |

## Lifecycle Note

- `Client::close()` 现在会让该 live session 绑定的 `wait_update()` / `wait_update_and_drain()` / `QuoteRef::wait_update()` / `KlineRef::wait_update()` / `TickRef::wait_update()` 返回关闭错误，而不是继续等待下一次行情。
- 已关闭的 `Client` session 不应再调用 `init_market()` 复活；需要重新进入 live 模式时应创建新 `Client`，或在 mode 切换内部先替换整个 private live context。
- `Client::into_runtime()` 派生出的 live runtime 必须复用同一个 private live context；关闭该 session 后，runtime market wait 路径同样应被关闭信号终止。

## Error Evolution Note

- `TqError` 现在标记为 `#[non_exhaustive]`。
- 下游如果对 `TqError` 做 variant `match`，需要保留 `_ => ...` 兜底分支，避免未来新增错误变体时编译失败。
- 推荐优先按语义分组处理，如权限、超时、数据缺失，其余错误统一走 fallback 分支。

## Quote Lifecycle Note

- cleanup 目标下，`QuoteSubscription` 只负责声明订阅生命周期与资源释放：`add_symbols()` / `remove_symbols()` / `close()`。
- Quote 状态读取的主叙事统一走 `Client::try_quote(symbol)?` 返回的快照句柄，消费时优先 `wait_update()` + `try_load()`。
- 关闭 `QuoteSubscription` 后，已有 Quote 引用不会失效，只是状态停止继续推进。

## TradeSession Readiness Migration

旧写法（busy-poll）：

```rust
session.connect().await?;
while !session.is_ready() {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}
```

新写法（canonical gate）：

```rust
session.connect().await?;
session.wait_ready().await?;
```

`is_ready()` 保留为瞬时状态探针，不再作为进入 ready state 的主流程等待 API。

## Series Snapshot Note

- `SeriesSubscription` 负责窗口态 K 线/Tick 订阅生命周期。
- `wait_update()` 返回 coalesced `SeriesSnapshot`，其中包含 `SeriesData`、`UpdateInfo` 和完成态 epoch。
- `load()` 返回当前最新 `SeriesData`；`snapshot()` 可直接读取完整快照。
- cleanup 目标下，live `SeriesSubscription` 将由 `Client` 直接创建，并默认自动启动。
- breaking rename：live bounded 序列入口已从 `Client::{kline,tick}` 切换到 `Client::{get_kline_serial,get_tick_serial}`，不保留 deprecated shim。
- 历史下载不再通过公开历史窗口订阅暴露；请使用 `Client::{get_kline_data_series,get_tick_data_series}`，且需要 `tq_dl` 权限。

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
- 已收口：`prelude::*` 不再囊括 `DataManager`、`InsAPI`、`SeriesAPI`、`SeriesData` 等高级类型；交易事件流类型当前仍保留在 prelude 中，其余高级接口请显式导入。
- 当前状态：`TradeEventStream`、`OrderEventStream`、`TradeOnlyEventStream` 与 `TradeEventRecvError` 仍暂时保留 crate root / prelude re-export；新文档与显式类型引用仍建议走 `tqsdk_rs::trade_session::{...}`。
- 已收口：`InsAPI`、`SeriesAPI`、`SeriesCachePolicy` 与 `KlineKey` 不再从 crate root 直接导出；显式类型引用请走对应模块命名空间。
- 已收口：`InsAPI` 与 `SeriesAPI` 已退回 crate 内部装配类型；query/series/downloader 主路径统一收口到 `Client`。
- 已收口：`Authenticator`、`ClientOption` 与 `BacktestResult` 不再从 crate root / prelude 直接导出；显式类型引用请走 `tqsdk_rs::{auth, client, replay}::{...}`。
- 已收口：`Client::market_state()` 与 `Client::tqapi()` 已删除；从 `Client` 读取行情状态请统一通过 `Client::{market_is_initialized,check_market_initialized,try_quote,try_kline_ref,try_tick_ref}`，其中 Quote 主路径是 `try_quote()` + `wait_update()` + `try_load()`。
- 已收口：`TradeSessionEvent` 与 `MarketDataUpdates` 已进入 crate root / prelude，便于主路径直接消费可靠交易事件和批量 market 更新。
- 已收口：`Client::ins()` 已删除；query facade 统一直接挂在 `Client` 上。
- 已收口：`Client::{kline,tick}` 已删除；live bounded 序列请统一使用 `Client::{get_kline_serial,get_tick_serial}`。
- 已收口：`marketdata` 模块已退回 crate 内部，`TqApi` / `MarketDataState` / `MarketDataUpdates` / `SymbolId` 不再作为公开扩展入口保留。
- 已收口：`ClientBuilder::build()` 不再隐式初始化 tracing；如需 SDK 日志，请显式调用 `init_logger()` 或组合 `create_logger_layer()`。
- 已收口：`TradeSession::{account_channel, position_channel}` 已删除；账户与持仓请走 `wait_update()` + snapshot getter。
- 已收口：`Client::{kline_history,kline_history_with_focus}` 已删除；历史快照下载统一走 `Client::{get_kline_data_series,get_tick_data_series}`，并在入口检查 `tq_dl`。
- 已收口：Quote / Series 已移除显式 `start()`；`close()` 仅表示提前释放资源。
- 已收口：`TradeSession` 账户/持仓统一走 `wait_update()` + getter；通知与异步错误并入 `subscribe_events()`；`subscribe_order_events()` / `subscribe_trade_events()` 维持独立可靠 retention，不受通知/错误事件流量污染。
- 约束：在 cleanup 完成前，不要为新代码新增 `BacktestHandle`、`on_quote`、`on_update`、`data_stream` 等依赖。
