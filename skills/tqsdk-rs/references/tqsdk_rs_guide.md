# TqSdk-RS 开发指南

`tqsdk-rs` 是天勤量化交易平台的 Rust SDK，提供高性能、类型安全的期货交易接口。

## 1. 核心特性

- **类型安全**: 90+ 字段完整定义，编译时消除运行时错误。
- **并发安全**: 基于 `Arc + RwLock`，无数据竞争。
- **异步优化**: 基于 `tokio` 异步运行时。
- **DIFF 协议**: 完整实现增量数据更新和递归合并。
- **灵活接口**: 支持 Channel、Callback、Stream 三种数据订阅方式。
- **零拷贝回调**: Arc 参数优化，多回调场景性能提升 50-100x。
- **Polars 集成**: 高性能列式数据分析（可选功能）。

## 2. 安装依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
tqsdk-rs = { git = "https://github.com/zynthium/tqsdk-rs.git", tag = "v0.1.2" }
tokio = { version = "1", features = ["full"] }

# 可选：启用 Polars 数据分析
# tqsdk-rs = { git = "https://github.com/zynthium/tqsdk-rs.git", tag = "v0.1.2", features = ["polars"] }
```

## 3. 功能模块概览

### 3.1 行情数据
| 功能 | 说明 |
|------|------|
| Quote | 实时行情订阅 |
| Kline | K线数据订阅（单合约/多合约对齐）|
| Tick | Tick 数据订阅 |
| History | 历史数据获取 |

### 3.2 交易功能
| 功能 | 说明 |
|------|------|
| TradeSession | 实盘/模拟交易会话 |
| Account | 账户资金查询 |
| Position | 持仓查询 |
| Order | 委托单管理 |
| Trade | 成交记录查询 |

## 4. 核心 API 参考

### 4.1 Client (客户端)
```rust
Client::new(username, password, config).await?     // 创建客户端
client.init_market().await?                        // 初始化行情连接
client.subscribe_quote(&symbols).await?            // 订阅行情
client.series()?                                   // 获取 Series API
client.ins()?                                      // 获取 Ins API
client.create_trade_session(broker, user, pwd)     // 创建交易会话
```

### 4.2 QuoteSubscription (行情订阅)
```rust
quote_sub.on_quote(|quote| { ... }).await          // 注册行情回调
quote_sub.start().await?                           // 启动订阅
```

### 4.3 SeriesApi (K线接口)
```rust
series_api.kline(symbol, duration, length).await?  // 订阅K线
sub.on_update(|data, info| { ... }).await          // K线更新回调
```

### 4.4 TradeSession (交易会话)
```rust
// 回调注册（先注册再 connect）
session.on_account(|account| { ... }).await        // 账户变动回调：Fn(Account)
session.on_position(|symbol, pos| { ... }).await   // 持仓变动回调：Fn(String, Position)，第一个参数是 symbol
session.on_order(|order| { ... }).await            // 委托变动回调：Fn(Order)
session.on_trade(|trade| { ... }).await            // 成交记录回调：Fn(Trade)
session.on_notification(|notif| { ... }).await     // 通知回调：Fn(Notification)
session.on_error(|err| { ... }).await              // 错误回调：Fn(String)

// Channel 风格（适合 select!/spawn 场景）
session.account_channel()       // Receiver<Account>
session.position_channel()      // Receiver<PositionUpdate>
session.order_channel()         // Receiver<Order>
session.trade_channel()         // Receiver<Trade>
session.notification_channel()  // Receiver<Notification>

// 连接与操作
session.connect().await?        // 连接交易服务器
session.is_ready()              // 检查是否已就绪
session.close().await?          // 关闭会话

// 下单：接受 &InsertOrderRequest 结构体
use tqsdk_rs::types::{InsertOrderRequest, DIRECTION_BUY, OFFSET_OPEN, PRICE_TYPE_LIMIT};
let order_id = session.insert_order(&InsertOrderRequest {
    symbol: "SHFE.au2602".to_string(),
    exchange_id: None,          // None 时从 symbol 自动解析
    instrument_id: None,        // None 时从 symbol 自动解析
    direction: DIRECTION_BUY.to_string(),
    offset: OFFSET_OPEN.to_string(),
    price_type: PRICE_TYPE_LIMIT.to_string(),
    limit_price: 650.0,
    volume: 1,
}).await?;

session.cancel_order(&order_id).await?  // 撤单

// 主动查询
session.get_account().await?                    // 账户资金
session.get_position("SHFE.au2602").await?      // 指定合约持仓
session.get_positions().await?                  // 所有持仓 HashMap<String, Position>
session.get_orders().await?                     // 所有委托单 HashMap<String, Order>
session.get_trades().await?                     // 所有成交记录 HashMap<String, Trade>
```

## 5. 常用代码模板

### 5.1 基础行情订阅
```rust
use std::env;
use tqsdk_rs::{Client, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let mut client = Client::new(&username, &password, ClientConfig::default()).await?;
    client.init_market().await?;
    
    let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
    
    quote_sub.on_quote(|quote| {
        println!("行情更新: {} 最新价={}", quote.instrument_id, quote.last_price);
    }).await;
    
    quote_sub.start().await?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

### 5.2 K线数据订阅
```rust
use std::time::Duration;

let series_api = client.series()?;
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 100).await?;

sub.on_update(|data, info| {
    if let Some(kline_data) = &data.single {
        if let Some(last_kline) = kline_data.data.last() {
            if info.has_new_bar {
                println!("新K线生成！");
            }
            println!("收盘价: {}", last_kline.close);
        }
    }
}).await;

sub.start().await?;
```

### 5.3 交易下单
```rust
use tqsdk_rs::types::{InsertOrderRequest, DIRECTION_BUY, OFFSET_OPEN, PRICE_TYPE_LIMIT};

// 创建交易会话
let session = client.create_trade_session("simnow", "user_id", "password").await?;

// 注册回调（先注册再 connect）
session.on_account(|account| {
    println!("账户余额: {}, 可用: {}", account.balance, account.available);
}).await;

// on_position 第一个参数是 symbol
session.on_position(|symbol, position| {
    println!("持仓更新: {} 净持仓={}", symbol, position.volume_long_today);
}).await;

session.on_order(|order| {
    println!("委托状态: {} - {}", order.order_id, order.status);
}).await;

session.on_trade(|trade| {
    println!("成交: {}", trade.trade_id);
}).await;

// 连接并下单
session.connect().await?;

// 使用 InsertOrderRequest 结构体下单
let order_id = session.insert_order(&InsertOrderRequest {
    symbol: "SHFE.au2602".to_string(),
    exchange_id: None,          // None 时从 symbol 自动解析
    instrument_id: None,        // None 时从 symbol 自动解析
    direction: DIRECTION_BUY.to_string(),     // "BUY" / "SELL"
    offset: OFFSET_OPEN.to_string(),          // "OPEN" / "CLOSE" / "CLOSETODAY"
    price_type: PRICE_TYPE_LIMIT.to_string(), // "LIMIT" / "ANY"(市价/IOC)
    limit_price: 650.0,
    volume: 1,
}).await?;

// 撤单
session.cancel_order(&order_id).await?;

// 主动查询
let account = session.get_account().await?;
let positions = session.get_positions().await?;
let orders = session.get_orders().await?;
```

### 5.4 多合约K线对齐
```rust
use std::time::Duration;

let series_api = client.series()?;
let symbols = vec!["SHFE.au2602".to_string(), "SHFE.ag2602".to_string()];
let sub = series_api.kline(&symbols, Duration::from_secs(60), 100).await?;

sub.on_update(|data, _| {
    if let Some(multi_data) = &data.multi {
        if let Some(aligned) = multi_data.data.last() {
            for (symbol, kline) in aligned.klines.iter() {
                println!("{}: close={}", symbol, kline.close);
            }
        }
    }
}).await;
```

### 5.5 Polars 数据分析（需启用 feature）
```rust
use std::time::Duration;

let series_api = client.series()?;
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 100).await?;

sub.on_update(|series_data, _| {
    if let Ok(df) = series_data.to_dataframe() {
        println!("{}", df);
    }
}).await;
```

## 6. 技术栈与注意事项

### 技术栈
| 依赖 | 版本 | 用途 |
|------|------|------|
| tokio | 1.48 | 异步运行时 |
| yawc | 0.2.7 | WebSocket 客户端 |
| reqwest | 0.12 | HTTP 客户端 |
| serde | 1.0 | 序列化 |
| polars | 0.44 | 数据分析（可选）|

### 注意事项
1.  **异步编程**: 所有 API 均为异步，需在 `tokio` 运行时中使用。
2.  **数据传递**: 回调函数使用 `Arc` 传递数据，支持零拷贝，高性能。
3.  **自动重连**: 内置自动重连机制，网络波动自动恢复。
4.  **优雅退出**: 使用 `tokio::signal::ctrl_c().await?` 监听退出信号。
5.  **合约格式**: 格式为 `交易所.品种代码月份`，如 `SHFE.au2602`, `DCE.m2401`, `CFFEX.IF2312`。
