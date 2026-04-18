# 常见问题

当用户描述“没数据”“不更新”“报未初始化”“回测不动”“事件 lagged”这类问题时，先读本文件。

## 常见错误边界（typed errors）

- `MarketNotInitialized`: 忘记 `init_market()`
- `ClientClosed`: 在已关闭 client 上继续使用 live 功能
- `SubscriptionClosed`: 在已关闭的 `SeriesSubscription` 上继续等待/读取
- `TradeSessionNotConnected`: `TradeSession` 尚未 `connect()`
- `DataNotReady`: 数据尚未 ready，或内部通道已关闭导致当前读取路径暂不可用

## `subscribe_quote()` / `get_kline_serial()` / `query_*()` 报未初始化

先检查是不是漏了：

```rust
client.init_market().await?;
```

当前 live market / query 能力都依赖 `init_market()`。

同样地，`Client::{wait_update,wait_update_and_drain}` 与 `QuoteRef` / `KlineRef` /
`TickRef` 的 `wait_update()` 在未初始化时也会直接报这个错误，而不是一直等待。

## Quote / Series 创建后为什么没 `start()`

因为创建后就已经 auto-start 了。

- `QuoteSubscription`
- `SeriesSubscription`

都在创建后立即生效。`close()` 只负责提前释放资源。

## 为什么 `QuoteSubscription` 上拿不到数据对象

因为现在订阅和读取已经分离：

- 订阅生命周期：`QuoteSubscription`
- 数据读取：`Client::quote(symbol)` -> `QuoteRef`

## `SeriesSubscription::load()` 报错或拿不到数据

通常是首次快照还没到。

先：

```rust
sub.wait_update().await?;
let data = sub.load().await?;
```

## `TradeSession` 连接了但没有快照 / 没事件

先检查：

1. 有没有先 `connect()`
2. 有没有完成 `wait_ready()`
3. 有没有持续消费 `wait_update()` 或可靠事件流

`wait_ready()` 是进入 ready state 的 canonical gate；`is_ready()` 只适合做瞬时状态检查，不要把它当主流程 busy-poll。

账户 / 持仓等最新状态按“ready 后等待更新 + 最新状态读取”模型来解释。

## `TradeEventRecvError::Lagged`

说明可靠事件流消费者太慢，或者 retention 太小。

优先建议：

- 更快消费事件
- 增大 `TradeSessionOptions.reliable_events_max_retained`

不要把它解释成通知或异步错误事件挤掉了订单 / 成交事件；优先从消费速度或 retention 排查。

## target-pos 不执行

live / replay 下都先检查“驱动循环”有没有跑起来：

- live 手工交易：`TradeSession` 已连接并 ready
- replay：持续调用 `ReplaySession::step()`

如果没有驱动循环，任务不会推进。

## 回测不推进

`ReplaySession::step()` 是唯一时间推进入口。

如果用户只创建了 `quote()` / `kline()` / `runtime()` 但没循环 `step()`，回测就不会动。

## `runtime.account(...)` 找不到账户

先检查 account key：

- live：通常是 `broker:user_id`
- replay：是 `session.runtime([...])` 里传进去的名字

## 历史数据 / EDB 权限问题

- `get_kline_data_series()` / 历史下载相关能力依赖历史数据权限
- `examples/history.rs` 现在只演示 bounded serial；需要排查 `tq_dl` 权限时，应看 `examples/data_series.rs`
- `query_edb_data()` 依赖非价量数据权限

这类问题优先说权限边界，不要虚构本地绕过方案。
