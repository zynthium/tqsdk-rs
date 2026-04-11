# Breaking State-Driven API Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不考虑向后兼容的前提下，把 live API 激进收敛为 `Client` 单入口的 state-driven 公开模型，删除剩余入口分散、手动启动和 callback/channel 历史包袱。

**Architecture:** 保留当前已经成型的 Rusty 路线，但把它推到底：`Client` 成为唯一 live 入口，直接承载 `wait_update`、状态引用、序列订阅和查询能力；Quote/Series 继续使用快照式状态读取；`TradeSession` 彻底按状态与可靠事件分层；`ReplaySession` / `TqRuntime` 保持独立。吸收 `docs/unimplement/` 中“统一入口、减少生命周期噪音”的有效结论，但明确放弃 `TqClient`、市场数据 Stream fan-out、兼容层和 deprecated 过渡方案。

**Tech Stack:** Rust 2024, `tokio`, `watch`, `broadcast`, existing `Client`, `MarketDataState`, `QuoteSubscription`, `SeriesSubscription`, `TradeSession`, `ReplaySession`, `TqRuntime`, `cargo test`, `cargo clippy`, `cargo check --examples`.

---

## Decision Summary

本计划基于 `docs/unimplement/` 中的分析，但做如下 breaking 决策：

- 采纳：统一 live 入口，删除 `Client` / `TqApi` / `SeriesAPI` / `InsAPI` 的公开分层感。
- 采纳：移除 `QuoteSubscription` / `SeriesSubscription` 的显式 `start()`。
- 采纳：把 `TradeSession` 彻底切成状态读取与可靠事件两层，不再维持 callback/channel 混搭。
- 采纳：文档、README、示例只展示 canonical state-driven API，不再保留“旧写法也可以”。
- 放弃：新增 `TqClient` 名字。当前 crate 已有 `Client`，继续引入同义 facade 只会制造第二套名字。
- 放弃：为 Quote / Series 重新引入 `Stream` 推送模型。行情和序列是状态，不是事件日志。
- 放弃：异步字段 getter 膨胀，如 `quote.last_price().await`。这类 API 会增加表面积，但不改变状态模型。
- 放弃：任何 deprecated shim、兼容层、双轨文档。

## Target Public Model

cleanup 完成后的 canonical public model 收敛为四条主路径：

- `Client`
  live 行情、序列、查询、交易会话创建的唯一 live 入口。
- `TradeSession`
  交易状态读取与可靠交易事件入口；账户/持仓是状态，订单/成交/通知/异步错误是事件。
- `ReplaySession`
  唯一推荐回放/回测入口。
- `TqRuntime`
  目标持仓和调度器运行时入口。

对应删除或降级的公开表面：

- `Client::tqapi()`
- `Client::series()`
- `Client::ins()`
- `QuoteSubscription::start()`
- `SeriesSubscription::start()`
- `TradeSession::{on_account,on_position,on_notification,on_error,notification_channel}`
- `TqApi` 作为 root/prelude 的 canonical live 入口地位

## Rejected Proposals

以下提案已经明确否决，不再进入后续讨论：

- 新增 `TqClient`
  原因：只是给 `Client` 换名，不解决任何结构问题，反而制造双名词和迁移噪音。
- 为 Quote/Series 重新提供 `Stream` 推送 API
  原因：与当前 state-driven 架构冲突，也与 `watch` 的“只保留最新状态”语义不匹配。
- 添加大量异步字段 getter，如 `quote.last_price().await`
  原因：只会扩张表面积，不会减少入口分散，也不会简化状态模型。
- 保留 deprecated shim 或双轨文档
  原因：本次就是 breaking cleanup，过渡层只会拖慢清理完成时间。

## File Map

- `src/client/mod.rs`
  收口 `Client` 的 live API 能力边界，减少外露的次级 facade。
- `src/client/facade.rs`
  增加 `Client` 直达的行情状态、序列订阅和查询方法；删除 `tqapi()/series()/ins()` 相关依赖路径。
- `src/client/tests.rs`
  覆盖新的单入口和 auto-start 语义。
- `src/marketdata/mod.rs`
  下沉 `TqApi` 为 advanced/internal helper，移除其作为 live canonical facade 的定位。
- `src/quote/lifecycle.rs`
  把 Quote 订阅改为创建即生效，删除 `start()`。
- `src/quote/tests.rs`
  覆盖 auto-start、drop cleanup 和失败回滚。
- `src/series/api.rs`
  把 live `kline/tick/history` canonical 路径转移到 `Client` 使用；保留内部装配职责。
- `src/series/subscription.rs`
  删除 `start()`，改为创建即启动；保留 `refresh()` / `close()` / `wait_update()` / `load()`。
- `src/series/tests.rs`
  覆盖 eager-start 语义与错误回滚。
- `src/trade_session/core.rs`
  删除 snapshot callback/channel public surface；扩展可靠事件类型。
- `src/trade_session/watch.rs`
  只维护 snapshot epoch 与 reliable event 发布，不再支持 callback plumbing。
- `src/trade_session/events.rs`
  扩展 `TradeSessionEventKind`，纳入通知与异步错误事件。
- `src/trade_session/tests.rs`
  覆盖 `wait_update()`、snapshot getter、notification/error event 语义。
- `src/lib.rs`
  收紧 root exports，只保留 canonical path。
- `src/prelude.rs`
  更新 prelude，只保留高频类型。
- `README.md`
  重写 quick start 和核心示例，删除 `tqapi()` / `series()` / `start()` 叙事。
- `examples/quote.rs`
  改成 `Client` 直达行情与序列 API。
- `examples/history.rs`
  改成 `Client` 直达序列 API。
- `examples/trade.rs`
  改成 `wait_update()` + snapshot getter + reliable event stream。
- `docs/migration-remove-legacy-compat.md`
  记录本轮 breaking 移除项。
- `docs/architecture.md`
  更新 live API 分层说明。
- `AGENTS.md`
  更新 agent 约束，防止新代码继续依赖旧 public surface。

## Task 1: Freeze The Breaking Target API

**Files:**
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `docs/architecture.md`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Test: `cargo test --doc`

- [ ] 明确写入新的 canonical live model：`Client`、`TradeSession`、`ReplaySession`、`TqRuntime`。
- [ ] 在迁移文档中新增本轮删除项：`Client::tqapi`、`Client::series`、`Client::ins`、`QuoteSubscription::start`、`SeriesSubscription::start`、`TradeSession` snapshot callback/channel。
- [ ] 在 README 和架构文档中明确写死两个原则：
  Quote/Series 是状态，不新增 Stream fan-out。
  `TradeSession` 的订单/成交/通知/异步错误是事件，账户/持仓是状态。
- [ ] 删除 README 中所有“旧 API 也可以”“兼容模式”“显式 start 是推荐路径”的表述。
- [ ] 提交一次纯文档 commit，冻结目标 API 说明，防止后续实现过程中目标漂移。

## Task 2: Collapse Live Market Access Into `Client`

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/tests.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `README.md`
- Modify: `examples/quote.rs`
- Modify: `examples/history.rs`
- Test: `src/client/tests.rs`

- [ ] 给 `Client` 增加直达 live 状态方法：
  `quote(symbol)`, `kline_ref(symbol, duration)`, `tick_ref(symbol)`, `wait_update()`, `wait_update_and_drain()`。
- [ ] 给 `Client` 增加直达序列订阅方法：
  `kline(symbols, duration, data_length)`, `tick(symbol, data_length)`, `kline_history(...)`, `kline_history_with_focus(...)`。
- [ ] 保留当前已有的 query facade，删除 `Client::ins()` 与 `Client::series()` 两个公开逃生口。
- [ ] 删除 `Client::tqapi()`，不再要求用户先取出 `TqApi` 再读状态。
- [ ] 收紧 root/prelude export：`TqApi`、`SeriesAPI`、`InsAPI` 不再作为默认导出和文档主路径。
- [ ] `TqApi` 退回 `marketdata` 内部或 advanced path，不再出现在 crate root、prelude、README 快速开始和 examples 中。
- [ ] 把 README 和 examples 改成只展示 `Client` 单入口路径。
- [ ] 为 `Client` 单入口补一组回归测试：
  `client.quote()` / `client.wait_update()` 可工作。
  `client.kline()` / `client.tick()` 不再需要 `client.series()?`。
  `client.close()` 后这些入口全部失效。

## Task 3: Remove Manual `start()` From Quote And Series

**Files:**
- Modify: `src/client/facade.rs`
- Modify: `src/quote/lifecycle.rs`
- Modify: `src/quote/tests.rs`
- Modify: `src/series/api.rs`
- Modify: `src/series/subscription.rs`
- Modify: `src/series/tests.rs`
- Modify: `README.md`
- Modify: `examples/quote.rs`
- Modify: `examples/history.rs`
- Test: `src/quote/tests.rs`
- Test: `src/series/tests.rs`

- [ ] 让 `Client::subscribe_quote()` 返回已生效的 `QuoteSubscription`，删除 `QuoteSubscription::start()`。
- [ ] 让 live `kline/tick/history` 返回已启动的 `SeriesSubscription`，删除 `SeriesSubscription::start()`。
- [ ] 保留 `Drop` 自动清理和显式 `close()`，但把 `close()` 定义为“提前释放资源”，不是“正式完成启动流程”。
- [ ] 调整失败回滚测试：
  之前验证 `start()` 失败回滚。
  现在验证构造/创建阶段失败不会留下脏订阅。
- [ ] 更新 README 与示例，删除所有 `.start().await?`。
- [ ] 在迁移文档中明确：auto-start 只适用于 Quote/Series；`TradeSession::connect()` 仍然保留，因为它代表显式交易连接副作用。

## Task 4: Finish TradeSession State/Event Separation

**Files:**
- Modify: `src/trade_session/core.rs`
- Modify: `src/trade_session/watch.rs`
- Modify: `src/trade_session/events.rs`
- Modify: `src/trade_session/tests.rs`
- Modify: `examples/trade.rs`
- Modify: `README.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Test: `src/trade_session/tests.rs`

- [ ] 给 `TradeSession` 增加 `wait_update()`，语义是“等待交易 snapshot epoch 推进”。
- [ ] 保留 `get_account()` / `get_position()` / `get_positions()` / `get_orders()` / `get_trades()` 作为 snapshot getter。
- [ ] 删除 `on_account()` 与 `on_position()`；账户和持仓从此只走 `wait_update()` + getter。
- [ ] 删除 `on_notification()`、`on_error()`、`notification_channel()`。
- [ ] 扩展 `TradeSessionEventKind`：
  `NotificationReceived { notification }`
  `TransportError { message }`
- [ ] 统一规定：`subscribe_events()` 是交易侧唯一 push-style 公共入口；订单、成交、通知、异步错误都通过事件流消费。
- [ ] 更新 `examples/trade.rs`：
  用 `session.wait_update().await?` 后读取账户/持仓快照。
  用 `subscribe_order_events()` / `subscribe_trade_events()` 或 `subscribe_events()` 读取事件。
- [ ] 删除所有 README 中“先注册回调再 connect”的叙述。

## Task 5: Tighten Root And Prelude Exports For The Final API Shape

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Test: `cargo check --examples`

- [ ] root export 只保留高频 facade 和直接消费类型，不再把已经降级为 advanced/internal 的类型放在顶层。
- [ ] prelude 只保留当前 README 快速开始真正需要的最短路径类型。
- [ ] `TqApi` 不再从 crate root / prelude 导出；如果仍保留代码实体，也只作为 `tqsdk_rs::marketdata::TqApi` 的 advanced path。
- [ ] `SeriesAPI` / `InsAPI` 明确退回模块级 advanced path；主文档不再展示。
- [ ] 更新迁移文档中的 surface mapping 表和 current status。
- [ ] 最后做一轮 README / examples / prelude 一致性检查，避免公开 API 和示例脱节。

## Task 6: Final Documentation And Release Cutover

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/architecture.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `examples/quote.rs`
- Modify: `examples/history.rs`
- Modify: `examples/trade.rs`
- Modify: `Cargo.toml`

- [ ] README 的所有 live 示例统一改成新路径：
  `Client` 单入口、Quote/Series auto-start、TradeSession snapshot/event 分离。
- [ ] `AGENTS.md` 明确禁止为新代码继续新增 `tqapi()` / `series()` / `ins()` / `start()` / trade callbacks。
- [ ] `Cargo.toml` 准备 breaking minor 版本升级说明，例如 `0.1.x -> 0.2.0`。
- [ ] 在迁移文档首页加粗标注：
  这是 breaking cleanup，没有兼容 shim。
- [ ] 检查 examples 是否全都对齐最终 API，而不是保留过渡写法。

## Verification Matrix

每个任务结束后按改动范围最小化验证，最终切面必须完整跑完：

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo check --examples`
- `cargo test --doc`
- `cargo build --features polars`

额外约束：

- 每个 breaking 切片必须带文档迁移说明。
- 每个 public surface 删除都要同步 README、examples、prelude。
- 每个切片控制在单一主题内提交，不把“入口收口”“trade 事件扩展”“export 收紧”混成一个 commit。

## Recommended Execution Order

1. Task 1
2. Task 2
3. Task 3
4. Task 4
5. Task 5
6. Task 6

理由：

- 先冻结目标 API，避免实现过程中反复改口。
- 再做 `Client` 单入口收口，因为它决定 README 和 examples 的主叙事。
- 然后做 auto-start，因为它会影响 Quote/Series 的所有示例和测试。
- TradeSession 的 snapshot/event 彻底分层单独做，避免和 market cleanup 互相缠绕。
- 最后统一 root/prelude/doc 形状，完成版本切换。

## Non-Goals

- 不引入 `TqClient` 同义 facade。
- 不重新引入 Quote/Series 回调或 Stream 推送模型。
- 不添加大量异步字段 getter。
- 不保留 deprecated 兼容路径。
- 不在这一轮重写底层 `websocket` / `DataManager` merge 语义；本计划聚焦 public API shape。

## Expected Outcome

完成后，live 使用方式应稳定为：

```rust
let mut client = Client::builder(user, pass).build().await?;
client.init_market().await?;

let _quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
let quote = client.quote("SHFE.au2602");
let klines = client.kline("SHFE.au2602", std::time::Duration::from_secs(60), 128).await?;

quote.wait_update().await?;
let q = quote.load().await;

let snapshot = klines.wait_update().await?;
let rows = snapshot.data;

let session = client.create_trade_session("simnow", user, pass).await?;
session.connect().await?;
session.wait_update().await?;
let account = session.get_account().await?;
let mut events = session.subscribe_events();
```

这条路径有三个明确特征：

- live 入口只有 `Client`，不再先拆成 `Client` + `TqApi` + `SeriesAPI` + `InsAPI`。
- Quote / Series 没有 `start()`，只保留状态读取与资源释放。
- `TradeSession` 彻底按“状态 vs 事件”分层，不再保留 callback/channel 混搭。
