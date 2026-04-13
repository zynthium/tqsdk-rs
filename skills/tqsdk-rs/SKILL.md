---
name: tqsdk-rs
description: Use when working with the tqsdk-rs Rust SDK, Rust Tianqin/TQSDK workflows, 行情订阅, K线, Tick, 合约查询, TradeSession, ReplaySession, TqRuntime, target_pos, backtests, or this repository's examples.
---

# tqsdk-rs

在回答 `tqsdk-rs` 问题时，以仓库当前公开代码为准，不以旧回答、旧 callback/channel 习惯或已删除的 compat facade 为准。

如果问题实际上是 Python `tqsdk` / `TqApi` / `wait_update()` 生态，而不是本仓库的 Rust SDK，不要使用本 skill 的 API 结论去回答。

## 先认源头

优先信任这些来源，顺序从高到低：

1. `src/lib.rs` 的公开 re-export
2. `README.md` 的 canonical 示例
3. `examples/`
4. 根 `AGENTS.md`
5. `docs/architecture.md`

如果 skill 里的旧说法与上面冲突，以上面为准，并直接纠正旧 API 名称。

## 先路由问题

只读与问题直接相关的参考文件：

1. `ClientBuilder`、`ClientConfig`、`EndpointConfig`、`init_market()`、端点覆盖、`build_runtime()`：读 [references/client-and-endpoints.md](references/client-and-endpoints.md)
2. Quote、K 线、Tick、`wait_update()`、`wait_update_and_drain()`、`QuoteRef`、`SeriesSubscription`、历史下载：读 [references/market-and-series.md](references/market-and-series.md)
3. 合约查询、期权筛选、结算价、持仓排名、EDB、交易日历、交易状态：读 [references/ins-and-reference-data.md](references/ins-and-reference-data.md)
4. `TradeSession`、可靠事件流、下单撤单、snapshot getter：读 [references/trading-and-orders.md](references/trading-and-orders.md)
5. `ReplaySession`、`ReplayConfig`、`step()`、`finish()`、回放句柄：读 [references/simulation-and-backtest.md](references/simulation-and-backtest.md)
6. `TqRuntime`、`runtime.account(...).target_pos(...).build()`、scheduler builder：读 [references/runtime-and-target-pos.md](references/runtime-and-target-pos.md)
7. 仓库现成例子或源码入口：读 [references/example-map.md](references/example-map.md)
8. 未初始化、无更新、权限、`Lagged`、回放不推进等常见坑：读 [references/error-faq.md](references/error-faq.md)
9. 用户只要总览或 API 地图：读 [references/tqsdk_rs_guide.md](references/tqsdk_rs_guide.md)

## 当前 canonical 心智模型

1. `Client` 是 live API 单入口。行情、序列和合约查询都优先走 `Client` facade。
2. `init_market()` 是 live market/query 能力的前置条件。没初始化就不要推荐 `subscribe_quote()`、`kline()`、`tick()`、`query_*()`。
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
- `Client::kline()`, `Client::tick()`, `Client::kline_ref()`, `Client::tick_ref()`
- `Client::get_kline_data_series()`, `Client::get_tick_data_series()`
- `Client::query_*()`, `Client::get_trading_calendar()`, `Client::get_trading_status()`
- `Client::create_trade_session*()`, `TradeSession::connect()`, `TradeSession::wait_update()`, `TradeSession::subscribe_*_events()`
- `Client::create_backtest_session()`, `ReplaySession::quote()`, `ReplaySession::kline()`, `ReplaySession::tick()`, `ReplaySession::aligned_kline()`, `ReplaySession::step()`, `ReplaySession::finish()`
- `ClientBuilder::build_runtime()`, `Client::into_runtime()`, `TqRuntime::account()`, `AccountHandle::target_pos()`, `AccountHandle::target_pos_scheduler()`

## 不要再教用户的旧 surface

- 不要推荐 `Client::tqapi()`, `Client::series()`, `Client::ins()`
- 不要推荐 `QuoteSubscription::start()` / `SeriesSubscription::start()`
- 不要推荐 `QuoteSubscription::on_quote`, `quote_channel`
- 不要推荐 `SeriesSubscription::on_update`, `on_new_bar`, `data_stream`
- 不要把 `BacktestHandle` 当公开回测入口
- 不要把 `TargetPosTask::new(...)` / `TargetPosScheduler::new(...)` 当 canonical 入口
- 不要为 `TradeSession` 新答案恢复账户 / 持仓 callback 或 best-effort channel 叙事
- 不要默认把底层 `websocket/`、`SeriesAPI`、`InsAPI` 讲成普通用户主路径

## 回答风格

- 优先给最小可运行 Rust 代码，不要只给伪代码
- 先说前置条件，再说下一步该调用哪个 API
- 明确区分 live market、trade session、replay backtest、runtime 四个上下文
- 如果用户说的是旧 API，直接指出新 canonical 写法
- 如果问题与仓库实现有关，优先引用 `README.md`、具体 `examples/*.rs`、或相关 `src/*` 模块
- 如果答案要涉及内部实现，先给 public API，再解释内部原因

## 最后自检

回答前快速检查：

- 我有没有把 `Client` 当成 live 主入口？
- 我有没有错误地要求用户 `start()` Quote/Series 订阅？
- 我有没有把 `TradeSession` 的状态读取和可靠事件流混在一起？
- 我有没有把 `ReplaySession::step()` 之外的东西说成时间推进入口？
- 我有没有把已经移除的 compat / callback / channel surface 重新讲回来？

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
