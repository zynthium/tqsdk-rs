# 常见问题与排查

## “为什么没有某个我预期中的统一更新入口？”

直接把回答收束到当前仓库真实存在的入口：

- `QuoteSubscription` / `SeriesSubscription` 回调
- channel / stream 消费
- 回测场景下用 `BacktestHandle`

不要为了迁就预期而虚构仓库中不存在的公共 API。

## `series()` / `ins()` 报未初始化

最常见原因：

- 还没调用 `init_market()`
- 或者还没调用 `init_market_backtest()`

优先把答案收束到初始化顺序，不要一开始就讲底层连接细节。

## 订阅创建了但没有回调

优先检查：

1. 是否已经 `start()`
2. 是否先注册回调再 `start()`
3. 是否订阅了有效合约

## `TradeSession` 没有回调或查不到数据

优先检查：

1. 是否已经 `connect()`
2. 是否在 `connect()` 前注册了回调
3. 账户参数是否正确

## 用户问旧 auth 环境变量

不要继续扩散旧设计。优先解释新的公开端点变量：

- `TQ_AUTH_URL`
- `TQ_MD_URL`
- `TQ_TD_URL`
- `TQ_INS_URL`
- `TQ_CHINESE_HOLIDAY_URL`

## 用户把行情初始化和交易初始化混为一谈

直接指出：

- 行情链路：`init_market` / `init_market_backtest`
- 交易链路：`create_trade_session*` + `connect`

它们相关，但不是同一个开关。

## 权限问题

有些接口会因为权限不足而失败，尤其是扩展基础数据类接口。回答时不要把权限错误说成 SDK bug。

## 服务地址覆盖怎么讲

优先顺序：

1. `ClientBuilder` 上的端点方法
2. `EndpointConfig::from_env()`
3. `TradeSessionOptions`

不要把 auth 内部变量重新包装成公开解法。
