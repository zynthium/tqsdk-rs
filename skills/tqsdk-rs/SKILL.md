---
name: tqsdk-rs
description: Use when working with the tqsdk-rs Rust SDK, Rust Tianqin/TQSDK workflows, 行情订阅, K线, Tick, 合约查询, TradeSession, ReplaySession, TqRuntime, target_pos, or backtests.
---

# tqsdk-rs

在回答 `tqsdk-rs` 问题时，优先使用用户当前提供的上下文；如果没有，再使用本 skill 的打包资料，并以本 skill 描述的 canonical API 为准。

如果问题实际上是 Python `tqsdk` / `TqApi` / `wait_update()` 生态，而不是这里讨论的 Rust SDK，不要使用本 skill 的 API 结论去回答。

## 先认源头

本 skill 应被视为可独立安装、独立分发的知识包。

默认来源顺序如下：

1. 本 skill 自带的 `references/*.md`
2. 本 skill 自带的 `assets/*`
3. 用户在当前对话里明确提供的代码、文档、错误输出、配置片段

额外规则：

- 不要默认假设原始项目文件在当前环境中可读。
- 不要把原始项目文件路径当成相对 skill 目录存在的路径。
- 如果用户在当前任务里提供了更新的代码或文档，应把这些任务内证据放在 skill 自带资料之上。
- 如果答案只基于 skill 打包资料，应明确告诉用户：这是基于 skill 参考资料的结论，不保证与最新版本完全一致。
- 如果问题依赖最近 public API 变化、精确源码位置或最新实现细节，应要求用户提供相关代码、文档或错误输出。

如果 skill 自带资料与用户当前提供的内容冲突，以用户当前提供的内容为准，并直接给出应使用的 API 名称。

## 先路由问题

只读与问题直接相关的参考文件：

1. `ClientBuilder`、`ClientConfig`、`EndpointConfig`、`init_market()`、端点覆盖、`build_runtime()`：读 [references/client-and-endpoints.md](references/client-and-endpoints.md)
2. Quote、K 线、Tick、`wait_update()`、`wait_update_and_drain()`、`QuoteRef`、`SeriesSubscription`、历史下载：读 [references/market-and-series.md](references/market-and-series.md)
3. 合约查询、期权筛选、结算价、持仓排名、EDB、交易日历、交易状态：读 [references/ins-and-reference-data.md](references/ins-and-reference-data.md)
4. `TradeSession`、可靠事件流、下单撤单、snapshot getter：读 [references/trading-and-orders.md](references/trading-and-orders.md)
5. `ReplaySession`、`ReplayConfig`、`step()`、`finish()`、回放句柄：读 [references/simulation-and-backtest.md](references/simulation-and-backtest.md)
6. `TqRuntime`、`runtime.account(...).target_pos(...).build()`、scheduler builder：读 [references/runtime-and-target-pos.md](references/runtime-and-target-pos.md)
7. 示例映射与入口提示：读 [references/example-map.md](references/example-map.md)
8. 未初始化、无更新、权限、`Lagged`、回放不推进等常见坑：读 [references/error-faq.md](references/error-faq.md)
9. 用户只要总览或 API 地图：读 [references/tqsdk_rs_guide.md](references/tqsdk_rs_guide.md)

## 当前 canonical 心智模型

1. `Client` 是 live API 单入口。行情、序列和合约查询都优先走 `Client` facade。
2. `init_market()` 是 live market/query 能力的前置条件。没初始化就不要推荐 `subscribe_quote()`、`get_kline_serial()`、`get_tick_serial()`、`query_*()`。
3. `QuoteSubscription` 只负责服务端订阅生命周期；Quote 读取统一走 `Client::quote()` 返回的 `QuoteRef`。
4. `SeriesSubscription` 是 coalesced snapshot API；窗口消费统一走 `wait_update()` / `snapshot()` / `load()`。
5. `TradeSession` 分成两层：
   - 账户 / 持仓 / 订单 / 成交的最新状态：`wait_update()` + getter
   - 订单 / 成交 / 通知 / 异步错误事件：可靠事件流 `subscribe_events()` / `subscribe_order_events()` / `subscribe_trade_events()`
6. `ReplaySession` 是唯一推荐的回测 / 回放入口；`step()` 是唯一时间推进入口。
7. `TqRuntime` 只在目标持仓 / scheduler / runtime 问题里展开；入口是 `runtime.account("...")`。

## 优先推荐的公开 API

- `Client::builder(...)`, `Client::new(...)`
- `Client::init_market()`
- `Client::subscribe_quote()`, `Client::quote()`, `Client::wait_update()`, `Client::wait_update_and_drain()`
- `Client::get_kline_serial()`, `Client::get_tick_serial()`, `Client::kline_ref()`, `Client::tick_ref()`
- `Client::get_kline_data_series()`, `Client::get_tick_data_series()`
- `Client::query_*()`, `Client::get_trading_calendar()`, `Client::get_trading_status()`
- `Client::create_trade_session*()`, `TradeSession::connect()`, `TradeSession::wait_update()`, `TradeSession::subscribe_*_events()`
- `Client::create_backtest_session()`, `ReplaySession::quote()`, `ReplaySession::kline()`, `ReplaySession::tick()`, `ReplaySession::aligned_kline()`, `ReplaySession::step()`, `ReplaySession::finish()`
- `ClientBuilder::build_runtime()`, `Client::into_runtime()`, `TqRuntime::account()`, `AccountHandle::target_pos()`, `AccountHandle::target_pos_scheduler()`

## 避免的非 canonical surface

- 不要把额外 helper facade 讲成普通用户主路径
- 不要给 Quote / Series 订阅增加额外启动步骤
- 不要把 Quote 讲成 fan-out callback / channel 模型
- 不要把 Series 讲成 update callback 或 stream fan-out 模型
- 回测入口统一讲 `ReplaySession`
- target-pos 入口统一讲 account builder
- 不要把 `TradeSession` 的公开叙事写成账户 / 持仓 callback 或 best-effort channel 模型
- 不要默认把底层 `websocket/`、`SeriesAPI`、`InsAPI` 讲成普通用户主路径
- 不要暗示 `ClientBuilder::build()` 会自动初始化 SDK 日志；日志应显式使用 `init_logger()` 或 `create_logger_layer()`

## 回答风格

- 优先给最小可运行 Rust 代码，不要只给伪代码
- 先说前置条件，再说下一步该调用哪个 API
- 明确区分 live market、trade session、replay backtest、runtime 四个上下文
- 如果用户使用的写法偏离本 skill 推荐路径，直接给出 canonical 写法
- 如果问题与实现细节有关，优先引用用户当前提供的代码或文档；如果没有，再引用相关 `references/*.md`
- 如果答案要涉及内部实现，先给 public API，再解释内部原因

## 最后自检

回答前快速检查：

- 我有没有把 `Client` 当成 live 主入口？
- 我有没有错误地要求用户 `start()` Quote/Series 订阅？
- 我有没有把 `TradeSession` 的状态读取和可靠事件流混在一起？
- 我有没有把 `ReplaySession::step()` 之外的东西说成时间推进入口？
- 我有没有把非 canonical 的 callback / channel 或其他旁路 surface 讲成主路径？

## 参考资料

- [API 地图](references/tqsdk_rs_guide.md)
- [客户端与端点](references/client-and-endpoints.md)
- [行情与序列](references/market-and-series.md)
- [合约与基础数据](references/ins-and-reference-data.md)
- [交易与下单](references/trading-and-orders.md)
- [回放与回测](references/simulation-and-backtest.md)
- [Runtime 与 TargetPos](references/runtime-and-target-pos.md)
- [示例索引](references/example-map.md)
- [常见问题](references/error-faq.md)
- [Cargo 模板](assets/Cargo.toml)
- [main.rs 模板](assets/main.rs)
