---
name: tqsdk-rs
description: 使用 tqsdk-rs（天勤 Rust SDK）开发高性能、类型安全的期货/期权策略与行情分析的指南，适用于 Rust 量化策略、行情订阅、交易与合约查询。
---

# tqsdk-rs 策略开发指南

## 概览

`tqsdk-rs` 是天勤量化交易平台的 Rust SDK，提供行情订阅、合约查询、交易会话与回测回放能力。整体基于 Tokio 异步运行时，适合高并发、低延迟的 Rust 策略开发。

## 关键特性

- **类型安全**：结构体与字段完整定义，编译期发现问题
- **并发安全**：基于 `Arc + RwLock` 与无锁标志，适配多任务并发
- **异步 I/O**：Tokio 驱动，非阻塞网络与数据处理
- **多种订阅模式**：回调、Channel、Stream 三种消费方式
- **延迟启动**：先注册回调再启动订阅，减少初始数据丢失
- **回测支持**：回放与实盘切换，支持 BacktestHandle 控制
- **Polars 集成**：可选 DataFrame 分析支持

## 快速开始

1. **添加依赖**：参见 [Cargo.toml](assets/Cargo.toml)
2. **编写入口**：参见 [main.rs](assets/main.rs)

## 使用流程

- **创建 Client**：`Client::new` 或 `Client::builder`
- **初始化行情**：`init_market` 或 `init_market_backtest`
- **订阅数据**：`subscribe_quote` 或 `series()`
- **查询合约**：`ins()` 或快捷查询接口
- **创建交易会话**：`create_trade_session` 后 `connect`

## Client 接口清单

**创建与配置**
- `Client::new` / `Client::builder`
- `ClientBuilder::log_level` / `view_width` / `development` / `config` / `auth` / `build`
- `ClientConfig`：`log_level` / `view_width` / `development` / `stock`

**行情与订阅**
- `init_market`
- `subscribe_quote`（返回 `QuoteSubscription`）
- `series`（获取 `SeriesAPI`）

**合约查询（GraphQL）**
- `ins`（获取 Ins API）
- `query_graphql`
- `query_quotes`
- `query_cont_quotes`
- `query_options`
- `query_symbol_info`
- `query_symbol_settlement`
- `query_symbol_ranking`
- `query_edb_data`
- `get_trading_calendar`
- `get_trading_status`

**交易会话**
- `create_trade_session`
- `register_trade_session`
- `get_trade_session`

**回测与历史回放**
- `init_market_backtest`
- `switch_to_backtest` / `switch_to_live`

**认证与生命周期**
- `set_auth`
- `get_auth` / `get_auth_id`
- `close`

## 订阅与回调要点

### Quote 订阅（实时行情）
- `QuoteSubscription::on_quote` 注册回调
- `QuoteSubscription::quote_channel` 获取 Channel
- `add_symbols` / `remove_symbols` 动态调整订阅
- `Client::subscribe_quote` 内部会启动订阅，重复 `start()` 是安全的

### Series 订阅（K线 / Tick / 历史）
- `SeriesAPI::kline`（单合约/多合约对齐）/ `tick`
- `kline_history` / `kline_history_with_focus` 历史数据定位
- `SeriesSubscription::on_update` / `on_new_bar` / `on_bar_update`
- `SeriesSubscription::data_stream` 获取 Stream
- `view_width` 超过 10000 会被收敛到 10000

### 交易会话（TradeSession）
- 创建会话后先注册回调或 Channel，再 `connect`
- `on_account` / `on_position` / `on_order` / `on_trade` / `on_notification` / `on_error`
- `on_position` 回调签名为 `Fn(String, Position)`，第一个参数是 symbol
- `account_channel` / `position_channel` / `order_channel` / `trade_channel` / `notification_channel`
- `insert_order` 接受 `&InsertOrderRequest` 结构体（含 `symbol`, `exchange_id`, `instrument_id`, `direction`, `offset`, `price_type`, `limit_price`, `volume`）
- `cancel_order` 传 `&order_id`
- 主动查询：`get_account` / `get_position` / `get_positions` / `get_orders` / `get_trades`

### 回测控制
- `BacktestHandle::next` 获取 `BacktestEvent`
- `BacktestHandle::peek` 主动拉取回测数据
- `BacktestHandle::current_dt` 获取当前回测时间

## 近期修复与注意事项
- 行情 WebSocket 断线重连后会自动重发未完成的 `ins_query`，并在收到非空结果后清理缓存
- `query_cont_quotes` 在未提供 `has_night` 时不再发送该变量，避免服务端变量校验导致超时
- `query_edb_data` 需要非价量数据权限，未开通会返回权限提示但不影响其他接口
- `series()` / `ins()` 在初始化行情（`init_market` / `init_market_backtest`）后可用

## 参考资料
- [TqSdk-RS 开发指南](references/tqsdk_rs_guide.md)
