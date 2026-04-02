---
name: tqsdk-rs
description: 使用 tqsdk-rs（天勤 Rust SDK）进行 Rust 量化开发、行情订阅、K线/Tick 序列、合约查询、交易会话、回测与端点配置的实战指南。只要用户提到 tqsdk-rs、天勤 Rust SDK、Rust 期货/期权策略、行情订阅、TradeSession、SeriesAPI、InsAPI、回测、TQ_AUTH_URL/TQ_MD_URL/TQ_TD_URL/TQ_INS_URL 这类主题，就应该使用此 skill。
---

# tqsdk-rs 开发指南

## 适用场景

- 用户要用 Rust 编写天勤量化策略、行情订阅程序或交易脚本
- 用户在 `Client` / `ClientBuilder` / `EndpointConfig` / `TradeSessionOptions` 上卡住
- 用户要区分 `subscribe_quote`、`series()`、`ins()`、`create_trade_session*()` 的职责
- 用户要理解天勤 Rust SDK 的推荐初始化顺序、回调注册顺序、回测入口和端点覆盖方式
- 用户要把旧写法迁移到 builder + endpoint 风格

## 先路由问题

只读取与问题直接相关的参考文件：

1. 行情快照、Quote 回调、动态加减订阅、基础订阅生命周期：读 [references/market-and-series.md](references/market-and-series.md)
2. K线、Tick、历史序列、多合约对齐、`SeriesSubscription` 更新语义：读 [references/market-and-series.md](references/market-and-series.md)
3. `Client`、`ClientBuilder`、`EndpointConfig`、环境变量、端点覆盖：读 [references/client-and-endpoints.md](references/client-and-endpoints.md)
4. `InsAPI`、合约查询、结算价、持仓排名、EDB、交易日历、交易状态：读 [references/ins-and-reference-data.md](references/ins-and-reference-data.md)
5. `TradeSession`、下单撤单、账户/持仓/委托/成交、交易地址覆盖：读 [references/trading-and-orders.md](references/trading-and-orders.md)
6. 回测、回放、`BacktestHandle`、live/backtest 切换：读 [references/simulation-and-backtest.md](references/simulation-and-backtest.md)
7. 用户要“找一个仓库里现成例子照着改”：读 [references/example-map.md](references/example-map.md)
8. 用户遇到未初始化、没回调、权限、地址覆盖、生命周期等常见坑：读 [references/error-faq.md](references/error-faq.md)
9. 用户只是要整体概览或 API 地图：读 [references/tqsdk_rs_guide.md](references/tqsdk_rs_guide.md)

## 核心心智模型

按下面的层级理解并给用户建议：

1. **Client / ClientBuilder**：统一入口，负责运行时参数和服务端点
2. **init_market / init_market_backtest**：激活行情链路；`series()`、`ins()`、`subscribe_quote()` 依赖它
3. **Quote / Series / Ins**：分别处理实时行情、K线/Tick/历史序列、合约与基础数据
4. **TradeSession**：独立交易链路，先建会话、注册回调，再 `connect`
5. **BacktestHandle**：回测推进与时间控制
6. **EndpointConfig / TradeSessionOptions**：负责端点覆盖

## 以 Rust 公共 API 为准

回答内容必须优先遵守 Rust 源码、公开导出的类型和仓库 examples。

- 优先使用 `Client` / `ClientBuilder` / `EndpointConfig` / `TradeSessionOptions`
- 订阅消费优先讲：
  - `QuoteSubscription` / `SeriesSubscription` 回调
  - channel / stream 消费
  - `BacktestHandle` 推进回测
- 交易入口优先讲 `TradeSession`
- 如果文档表述、经验写法和当前仓库源码冲突，**以 Rust 源码和 Rust examples 为准**

## 首选写法

优先推荐 builder + endpoint 风格：

```rust
use tqsdk_rs::{Client, ClientConfig, EndpointConfig};

let mut client = Client::builder(username, password)
    .config(ClientConfig::default())
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;
```

除非用户明确只想要最短写法，否则优先展示上面的风格，而不是把 `Client::new(...)` 作为主推荐。

## 最常用流程

### 行情 / 序列 / 合约

```text
Client::builder -> build -> init_market / init_market_backtest -> subscribe_quote / series / ins
```

### 交易

```text
Client::builder -> build -> create_trade_session* -> 注册回调或 channel -> connect
```

### 回测

```text
Client::builder -> build -> init_market_backtest -> quote / series / ins -> BacktestHandle
```

## 高频 API

- 客户端与配置：`Client::builder`、`Client::new`、`ClientConfig`、`EndpointConfig`、`TradeSessionOptions`
- 行情与订阅：`init_market`、`init_market_backtest`、`subscribe_quote`、`series`
- 合约与基础数据：`ins`、`query_quotes`、`query_cont_quotes`、`query_options`、`query_symbol_info`、`query_symbol_settlement`、`query_symbol_ranking`、`query_edb_data`、`get_trading_calendar`、`get_trading_status`
- 交易会话：`create_trade_session`、`create_trade_session_with_options`、`register_trade_session`、`get_trade_session`
- 回测与切换：`switch_to_backtest`、`switch_to_live`、`BacktestHandle::{next, peek, current_dt}`

## 关键规则

### QuoteSubscription
- `QuoteSubscription::on_quote` 注册回调
- `QuoteSubscription::quote_channel` 获取 Channel
- `add_symbols` / `remove_symbols` 动态调整订阅
- `Client::subscribe_quote` 返回的是**延迟启动订阅**，先注册回调再 `start()`

### SeriesSubscription
- `SeriesAPI::kline`（单合约/多合约对齐）/ `tick`
- `kline_history` / `kline_history_with_focus` 历史数据定位
- `SeriesSubscription::on_update` / `on_new_bar` / `on_bar_update`
- `SeriesSubscription::data_stream` 获取 Stream
- 新建订阅后先注册回调，再 `start()`

### TradeSession
- 创建会话后先注册回调或 Channel，再 `connect`
- `on_account` / `on_position` / `on_order` / `on_trade` / `on_notification` / `on_error`
- `on_position` 回调签名为 `Fn(String, Position)`，第一个参数是 symbol
- `account_channel` / `position_channel` / `order_channel` / `trade_channel` / `notification_channel`
- `insert_order` 接受 `&InsertOrderRequest` 结构体（含 `symbol`, `exchange_id`, `instrument_id`, `direction`, `offset`, `price_type`, `limit_price`, `volume`）
- `cancel_order` 传 `&order_id`
- 主动查询：`get_account` / `get_position` / `get_positions` / `get_orders` / `get_trades`

### 端点覆盖

优先向用户说明新的公开端点配置，而不是 auth 内部细节：

- `TQ_AUTH_URL`
- `TQ_MD_URL`
- `TQ_TD_URL`
- `TQ_INS_URL`
- `TQ_CHINESE_HOLIDAY_URL`

推荐用法：

```rust
let endpoints = EndpointConfig::from_env();
let client = Client::builder(username, password)
    .config(ClientConfig::default())
    .endpoints(endpoints)
    .build()
    .await?;
```

交易地址优先级：

```text
TradeSessionOptions.td_url_override
> ClientBuilder::td_url / EndpointConfig.td_url
> TQ_TD_URL
> 鉴权返回的默认交易地址
```

### 不要误导用户的点

- 不要再把 `TQ_NS_URL`、`TQ_CLIENT_ID`、`TQ_CLIENT_SECRET`、`TQ_AUTH_PROXY`、`TQ_AUTH_VERIFY_JWT` 讲成公开推荐配置
- 不要默认建议直接改 auth 内部实现
- 不要省略“先 init_market，再用 series/ins/quote”的依赖关系
- 不要省略“先注册回调，再 start/connect”的顺序
- 不要把 `TradeSession` 和行情初始化绑死，它们是两条相关但不同的链路
- 不要虚构仓库里并不存在的公共 API 或工作流

## 回答风格

- 优先给**最小可运行 Rust 代码**，不要只给伪代码
- 优先说清楚“前置条件 + 下一步该调用哪个 API”
- 如果问题本质是生命周期顺序问题，要直接指出，不要只堆代码
- 如果问题涉及行情、交易、回测多个上下文，明确区分，不要混答
- 如果用户问配置或环境变量，优先给 builder/endpoint 解法，再补环境变量
- 如果用户要仓库例子，尽量给出对应 example 名称，而不是笼统说“看 examples”
- 如果回答要涉及底层实现，先给公共 API 方案，再解释底层原因

## 回答用户时的组织方式

优先使用下面的回答结构：

1. **先判断用户在做哪类事**
   - 行情订阅
   - K线/Tick 序列
   - 合约查询
   - 交易会话
   - 回测
   - 端点配置 / 环境变量
2. **给出推荐入口**
   - 用哪个 API
   - 初始化前置条件
   - 是否需要 `start()` / `connect()`
3. **给最小可运行示例**
4. **补充常见坑**
5. **必要时再引导去参考资料**

## 快速答法

- `series()` / `ins()` 报未初始化：通常是没先调用 `init_market()` 或 `init_market_backtest()`
- 没有行情 / K线回调：先检查是否 `start()`，以及是否在 `start()` 前注册了回调
- 交易地址怎么改：优先用 `TradeSessionOptions { td_url_override }` 或 `ClientBuilder::td_url(...)`
- 想切换服务地址：优先讲 `EndpointConfig::from_env()` 与 `ClientBuilder` 上的端点方法
- 该看哪个模块：行情看 `quote/series`，基础数据看 `ins`，交易看 `trade_session`，认证问题再进 `auth`

## 参考资料
- [参考指南](references/tqsdk_rs_guide.md)
- [客户端与端点](references/client-and-endpoints.md)
- [行情与序列](references/market-and-series.md)
- [合约与基础数据](references/ins-and-reference-data.md)
- [交易与下单](references/trading-and-orders.md)
- [回测与回放](references/simulation-and-backtest.md)
- [常见问题](references/error-faq.md)
- [示例索引](references/example-map.md)
- [Cargo 模板](assets/Cargo.toml)
- [main.rs 模板](assets/main.rs)
