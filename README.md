# TQSDK-RS

天勤量化交易平台的 Rust SDK，提供行情订阅、K 线与 Tick 序列、合约查询、回测和实盘交易接口。

[![Crates.io](https://img.shields.io/crates/v/tqsdk-rs.svg)](https://crates.io/crates/tqsdk-rs)
[![Documentation](https://docs.rs/tqsdk-rs/badge.svg)](https://docs.rs/tqsdk-rs)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## 核心特性

- 类型安全：核心行情、K 线、Tick、账户、委托、成交等结构均为强类型定义。
- 异步优先：基于 `tokio`，统一行情、序列、交易与回测的异步接口。
- DIFF 协议：支持天勤增量数据合并、路径监听与 epoch 变化追踪。
- 状态驱动优先：`Client` 负责最新市场状态读取，`SeriesSubscription` 负责窗口态序列读取。
- 延迟启动：先创建订阅/会话句柄，再显式 `start()`/`connect()`，把生命周期控制与数据读取分离。
- 背压可控：关键通道与离线队列已改为有界缓冲，避免慢消费者无限堆积内存。
- 零拷贝共享：状态快照与事件对象尽量通过 `Arc<T>` 共享，降低多消费者场景开销。
- Polars 集成：可选启用 `polars` feature，将序列数据直接转换为 DataFrame。
- 回测支持：`ReplaySession` 是历史回放、回测推进与 runtime 驱动的唯一推荐入口。
- 连接入口收口：原始 WebSocket transport 保持为内部实现，公开连接路径统一走 `Client`、`TradeSession` 和 `ReplaySession`。

## Breaking Cleanup Direction

下一轮破坏性升级的目标已经冻结，不再考虑兼容层或 deprecated 过渡：

- live API 收口到 `Client` 单入口。
  `Client` 将直接承载行情状态读取、序列订阅与 query facade；
  `TqApi`、`SeriesAPI`、`InsAPI` 将退出 crate root / prelude / README 主路径。
- Quote / Series 继续坚持状态驱动，不重新引入 Stream fan-out。
  `QuoteSubscription` / `SeriesSubscription` 的显式 `start()` 将被移除，改为创建即生效。
- `TradeSession` 将彻底按“状态 vs 事件”分层。
  账户/持仓统一走 `wait_update()` + getter；
  订单/成交/通知/异步错误统一走可靠事件流。
- 本轮 cleanup 不会引入 `TqClient` 同义 facade，不会保留长期 shim。

当前 README 中的代码示例仍反映仓库现状；随着 breaking slices 落地，这些示例会同步切到新的 canonical API。

## 功能模块

### 行情数据

- Quote 实时行情订阅，支持多合约动态增删。
- 单合约和多合约对齐 K 线订阅。
- Tick 订阅与历史 K 线拉取。
- 合约、主连、期权、交易日历、交易状态查询。

### 交易功能

- `TradeSession` 实盘/模拟交易会话。
- `TqRuntime` + `AccountHandle::{target_pos,target_pos_scheduler}` Builder 目标持仓运行时。
- 账户、持仓、委托、成交实时更新与主动查询。
- 下单、撤单、登录就绪检测与自动重连。

### 数据管理

- DIFF 数据合并与默认值补全。
- `watch` / `unwatch` 路径监听。
- 数据变化 epoch 追踪与按路径读取。

### 分析与回测

- 历史回放与回测推进。
- 可选 Polars DataFrame 转换。
- DataManager 直接读取底层数据。

### 开发体验

- `ClientBuilder` 配置式构建。
- `tracing` 日志集成和自定义 Layer。
- 9 个示例程序覆盖主要使用路径。

## 近期修复与更新

- 修复行情 WebSocket 断线重连后未完成 `ins_query` 可能丢失或重复的问题。
- 修复 `query_cont_quotes` 在未提供 `has_night` 时仍发送该变量导致的超时问题。
- 修复 `TradeSession::connect()` 失败后后台任务仍可能继续重连或保活的问题。
- 将 `SeriesSubscription` 收敛为快照式状态接口，窗口消费统一改用 `wait_update()` / `load()`。
- 将 Quote、TradingStatus、DataManager watch 与 WebSocket 离线发送队列改为有界缓冲，慢消费者场景下以丢弃更新替代无限堆积。
- 修复 `has_md_grants` 的指数权限判断顺序：
  `SSE.000016` / `SSE.000300` / `SSE.000905` / `SSE.000852` 现在会优先校验 `lmt_idx` 权限。
- 优化交易状态订阅生命周期：
  多订阅者按引用计数聚合，receiver 释放后会自动减少订阅集合并回发 `subscribe_trading_status`。
- 日志与磁盘缓存初始化移除库级 `panic` 路径，改为可降级行为和告警输出。
- token 解析新增 claims 校验（`exp` / `nbf` / `azp`），减少异常 token 对权限边界的影响。

## 验证与排查

- 单元测试：`cargo test`
- 静态检查：`cargo clippy --all-targets --all-features -- -D warnings`
- 示例联调：`cargo run --example history`
- 账户权限：`query_edb_data` 需要账号具备非价量数据权限，否则会返回权限提示，不影响其他接口。
- 交易示例：`cargo run --example trade` 会连接交易环境，运行前请确认使用的是模拟账户并已正确配置环境变量。

## 快速开始

### 安装依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tqsdk-rs = { git = "https://github.com/zynthium/tqsdk-rs.git", tag = "v0.1.3" }

# 如需 Polars DataFrame 支持：
# tqsdk-rs = { git = "https://github.com/zynthium/tqsdk-rs.git", tag = "v0.1.3", features = ["polars"] }
```

### 环境变量

最少需要配置天勤账户；其他变量按示例或部署场景按需开启。

| 变量 | 必需 | 说明 |
|------|------|------|
| `TQ_AUTH_USER` | 是 | 天勤账号 |
| `TQ_AUTH_PASS` | 是 | 天勤密码 |
| `TQ_LOG_LEVEL` | 否 | 常见示例使用的日志级别，如 `info`、`debug` |
| `SIMNOW_USER_0` | 否 | `trade` 示例使用的 SimNow 账号 |
| `SIMNOW_PASS_0` | 否 | `trade` 示例使用的 SimNow 密码 |
| `TQ_START_DT` | 否 | `backtest` 示例起始日期，格式 `YYYY-MM-DD` |
| `TQ_END_DT` | 否 | `backtest` 示例结束日期，格式 `YYYY-MM-DD` |
| `TQ_TEST_SYMBOL` | 否 | `history` 示例联调用的测试合约 |
| `TQ_UNDERLYING` | 否 | `option_levels` 示例的标的合约 |

### 基础示例 - 行情订阅

```rust
use std::env;
use std::time::Duration;
use tqsdk_rs::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;

    let mut client = Client::builder(&username, &password)
        .config(ClientConfig::default())
        .endpoints(EndpointConfig::from_env())
        .build()
        .await?;
    client.init_market().await?;

    let symbol = "SHFE.au2602";
    
    let quote_sub = client.subscribe_quote(&[symbol]).await?;
    quote_sub.start().await?;

    let quote_ref = client.quote(symbol);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        
        quote_ref.wait_update().await?;
        let q = quote_ref.load().await;
        println!("{} 最新价 = {}", q.instrument_id, q.last_price);
    }

    Ok(())
}
```

### 使用 ClientBuilder（推荐）

```rust
use std::env;
use tqsdk_rs::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;

    let mut client = Client::builder(username, password)
        .log_level(env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()))
        .view_width(5000)
        .development(true)
        .message_queue_capacity(4096)
        .build()
        .await?;

    client.init_market().await?;
    Ok(())
}
```

### 目标持仓任务（Builder，推荐）

`TqRuntime` 由 `ClientBuilder::build_runtime()`、`Client::into_runtime()` 或
`ReplaySession::runtime()` 提供；`ExecutionAdapter` / `MarketAdapter` /
`TaskRegistry` 等装配细节不再作为推荐的 public extension surface。

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;

async fn demo(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let account = runtime
        .account("SIM")
        .expect("configured account should exist");
    let task = account.target_pos("SHFE.rb2601").build()?;

    task.set_target_volume(1)?;
    task.wait_target_reached().await?;
    task.cancel().await?;
    task.wait_finished().await?;
    Ok(())
}
```

### 时间分片调仓（Builder，推荐）

```rust
use std::sync::Arc;
use std::time::Duration;
use tqsdk_rs::prelude::*;

async fn demo_scheduler(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let account = runtime
        .account("SIM")
        .expect("configured account should exist");
    let scheduler = account
        .target_pos_scheduler("SHFE.rb2601")
        .steps(vec![
            TargetPosScheduleStep {
                interval: Duration::from_secs(10),
                target_volume: 1,
                price_mode: Some(PriceMode::Passive),
            },
            TargetPosScheduleStep {
                interval: Duration::from_secs(5),
                target_volume: 1,
                price_mode: Some(PriceMode::Active),
            },
        ])
        .build()?;

    scheduler.wait_finished().await?;
    println!("aggregated trades: {}", scheduler.execution_report().trades.len());
    Ok(())
}
```

### ReplaySession 回测（推荐）

```rust
use std::time::Duration;
use tqsdk_rs::prelude::*;

let config = ReplayConfig::new(start_dt, end_dt)?;
let mut session = client.create_backtest_session(config).await?;
let quote = session.quote("SHFE.rb2605").await?;
let klines = session.kline("SHFE.rb2605", Duration::from_secs(60), 64).await?;
let runtime = session.runtime(["TQSIM"]).await?;
let account = runtime.account("TQSIM").expect("configured account should exist");
let task = account.target_pos("SHFE.rb2605").build()?;

while let Some(step) = session.step().await? {
    if !step.updated_handles.iter().any(|id| id == klines.id()) {
        continue;
    }

    let rows = klines.rows().await;
    if rows.len() >= 2 {
        let last = &rows[rows.len() - 1];
        let prev = &rows[rows.len() - 2];
        if last.state.is_closed() {
            let target = if last.kline.close > prev.kline.close { 1 } else { 0 };
            task.set_target_volume(target)?;
        }
    }

    if let Some(snapshot) = quote.snapshot().await {
        println!("replay quote={:.2}", snapshot.last_price);
    }
}

task.cancel().await?;
task.wait_finished().await?;
let result = session.finish().await?;
println!("trades={}", result.trades.len());
```

日线开盘信号策略可参考 `examples/pivot_point.rs`。

### 覆盖服务端点

```rust
use std::env;
use tqsdk_rs::{Client, EndpointConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;

    let endpoints = EndpointConfig {
        auth_url: "https://auth.shinnytech.com".to_string(),
        md_url: Some("wss://example.com/md".to_string()),
        td_url: None,
        ins_url: "https://openmd.shinnytech.com/t/md/symbols/latest.json".to_string(),
        holiday_url: "https://files.shinnytech.com/shinny_chinese_holiday.json".to_string(),
    };

    let mut client = Client::builder(username, password)
        .endpoints(endpoints)
        .build()
        .await?;

    client.init_market().await?;
    Ok(())
}
```

## 核心功能详解

### 1. 行情数据 (状态驱动 API - 推荐)

`tqsdk-rs` 提供高性能的状态驱动行情订阅。通过 `Client` 直接获取数据引用（`QuoteRef`/`KlineRef`/`TickRef`），并使用 `wait_update()` 驱动策略循环。

#### Quote 订阅 - 实时行情

```rust
let symbol = "SHFE.au2602";

let quote_sub = client.subscribe_quote(&[symbol]).await?;
quote_sub.start().await?;

let quote_ref = client.quote(symbol);

loop {
    // 等待任意数据更新，并获取本次变化的集合
    let updates = client.wait_update_and_drain().await?;
    
    if updates.quotes.contains(&symbol.into()) {
        let q = quote_ref.load().await;
        println!("{} 最新价: {}", q.instrument_id, q.last_price);
    }
}
```

#### K 线订阅 - 单合约

```rust
use std::time::Duration;

let symbol = "SHFE.au2602";
let duration = Duration::from_secs(60);

let sub = client.kline(symbol, duration, 300).await?;
sub.start().await?;

let kline_ref = client.kline_ref(symbol, duration);

loop {
    kline_ref.wait_update().await?;
    let k = kline_ref.load().await;
    println!("最新 K 线: id={} close={}", k.id, k.close);
}
```

#### Quote 订阅的职责边界

`QuoteSubscription` 现在只负责向服务端声明订阅生命周期；真正的数据读取统一走 `Client::quote()` 返回的 `QuoteRef`。

```rust
let symbols = ["SHFE.au2602", "SHFE.ag2512"];

let quote_sub = client.subscribe_quote(&symbols).await?;
quote_sub.start().await?;

let au = client.quote("SHFE.au2602");
let ag = client.quote("SHFE.ag2512");

loop {
    let updates = client.wait_update_and_drain().await?;

    if updates.quotes.contains(&"SHFE.au2602".into()) {
        println!("au 最新价: {}", au.load().await.last_price);
    }
    if updates.quotes.contains(&"SHFE.ag2512".into()) {
        println!("ag 最新价: {}", ag.load().await.last_price);
    }
}
```

`QuoteRef` 是 `MarketDataState` 上的快照句柄，不拥有订阅本身；关闭 `QuoteSubscription` 后，已有 `QuoteRef` 不会失效，只是状态不再继续推进。

### 2. 交易功能

#### 创建交易会话

```rust
use tqsdk_rs::{TradeSessionEventKind, TradeSessionOptions};

let session = client
    .create_trade_session_with_options(
        "simnow",
        &sim_user_id,
        &sim_password,
        TradeSessionOptions {
            td_url_override: Some("wss://example.com/trade".to_string()),
            reliable_events_max_retained: 8_192,
        },
    )
    .await?;

session
    .on_account(|account| {
        println!("权益={} 可用={}", account.balance, account.available);
    })
    .await;
session
    .on_position(|symbol, position| {
        println!(
            "{} 多={} 空={}",
            symbol,
            position.volume_long_today + position.volume_long_his,
            position.volume_short_today + position.volume_short_his
        );
    })
    .await;

let mut order_events = session.subscribe_order_events();
tokio::spawn(async move {
    while let Ok(event) = order_events.recv().await {
        if let TradeSessionEventKind::OrderUpdated { order_id, order } = event.kind {
            println!("订单 {} 状态={}", order_id, order.status);
        }
    }
});

let mut trade_events = session.subscribe_trade_events();
tokio::spawn(async move {
    while let Ok(event) = trade_events.recv().await {
        if let TradeSessionEventKind::TradeCreated { trade_id, trade } = event.kind {
            println!("成交 {} 对应订单={}", trade_id, trade.order_id);
        }
    }
});

session.connect().await?;

while !session.is_ready() {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}
```

优先级为：`TradeSessionOptions.td_url_override` > `ClientBuilder::td_url` /
`EndpointConfig.td_url` > `TQ_TD_URL` > 鉴权返回的默认交易地址。

说明：

- 交易会话同样推荐先注册账户/持仓回调并先订阅可靠事件流，再 `connect()`。
- `order` / `trade` 的 public canonical API 是 `subscribe_events()` / `subscribe_order_events()` / `subscribe_trade_events()`。
- 可靠事件流只会看到订阅之后产生的事件；账户、持仓仍然按最新快照读取或回调消费。
- 需要等待某个订单出现后续更新时，优先使用 `wait_order_update_reliable(order_id)`。
- `connect()` 失败时会清理本次连接产生的后台状态，不会继续残留重连任务。

#### 下单操作

当前接口使用 `InsertOrderRequest`，不再是旧版的扁平参数列表：

```rust
use tqsdk_rs::InsertOrderRequest;

let order_id = session
    .insert_order(&InsertOrderRequest {
        symbol: "SHFE.au2602".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: "BUY".to_string(),
        offset: "OPEN".to_string(),
        price_type: "LIMIT".to_string(),
        limit_price: 500.0,
        volume: 1,
    })
    .await?;

println!("下单成功: {}", order_id);
session.cancel_order(&order_id).await?;
```

市价单示例：

```rust
let order_id = session
    .insert_order(&InsertOrderRequest {
        symbol: "SHFE.au2602".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: "SELL".to_string(),
        offset: "CLOSE".to_string(),
        price_type: "ANY".to_string(),
        limit_price: 0.0,
        volume: 1,
    })
    .await?;
```

#### 查询交易数据

```rust
let account = session.get_account().await?;
println!("权益={} 可用={}", account.balance, account.available);

let position = session.get_position("SHFE.au2602").await?;
println!(
    "多={} 空={}",
    position.volume_long_today + position.volume_long_his,
    position.volume_short_today + position.volume_short_his
);

let orders = session.get_orders().await?;
let trades = session.get_trades().await?;
println!("orders={}, trades={}", orders.len(), trades.len());
```

### 3. 数据管理器（DataManager）

DataManager 是底层 DIFF 合并与数据读取核心。大多数场景不需要直接操作；如果你在做协议调试、离线 merge 验证或自定义状态派生，可以直接使用：

```rust
use std::collections::HashMap;
use serde_json::json;
use tqsdk_rs::{DataManager, DataManagerConfig};

let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());

dm.merge_data(
    json!({
        "quotes": {
            "SHFE.au2602": {
                "instrument_id": "SHFE.au2602",
                "datetime": "2026-04-11 09:00:00.000000",
                "last_price": 500.0
            }
        }
    }),
    true,
    true,
);

// 路径访问（灵活访问任意数据）
if let Some(data) = dm.get_by_path(&["quotes", "SHFE.au2602"]) {
    println!("原始数据: {:?}", data);
}

// 检查数据是否在最近一次更新中发生了变化
if dm.is_changing(&["quotes", "SHFE.au2602"]) {
    println!("数据在最近一次更新中发生了变化");
}

// 获取指定路径的数据更新版本号（更推荐的增量更新检测方式）
let path_epoch = dm.get_path_epoch(&["quotes", "SHFE.au2602"]);
println!("路径 [\"quotes\", \"SHFE.au2602\"] 的 epoch: {}", path_epoch);

// 获取当前全局版本号
let epoch = dm.get_epoch();
println!("当前全局 epoch: {}", epoch);
```

如需在每次 merge 完成后拉取最新状态，优先使用 `subscribe_epoch()`，再结合 `get_path_epoch()` / `get_by_path()` 做状态轮询，而不是新增全局 callback：

```rust
let mut epoch_rx = dm.subscribe_epoch();
tokio::spawn(async move {
    while epoch_rx.changed().await.is_ok() {
        println!("merge 完成后的全局 epoch = {}", *epoch_rx.borrow_and_update());
    }
});

dm.merge_data(
    serde_json::json!({
        "quotes": {
            "SHFE.au2602": { "last_price": 500.0 }
        }
    }),
    true,
    true,
);
```

### 4. 回测与历史回放（推荐）

```rust
use std::time::Duration;
use tqsdk_rs::prelude::*;

let username = std::env::var("TQ_AUTH_USER")?;
let password = std::env::var("TQ_AUTH_PASS")?;

let mut client = Client::builder(username, password).build().await?;
let start = chrono::Utc::now() - chrono::Duration::days(7);
let end = chrono::Utc::now();
let mut session = client
    .create_backtest_session(ReplayConfig::new(start, end)?)
    .await?;

let quote = session.quote("SHFE.au2602").await?;
let bars = session
    .series()
    .kline("SHFE.au2602", Duration::from_secs(60), 32)
    .await?;

while let Some(step) = session.step().await? {
    if let Some(snapshot) = quote.snapshot().await {
        println!("推进到 {} last_price={}", step.current_dt, snapshot.last_price);
    }
}

println!("closed bars={}", bars.rows().await.len());
let result = session.finish().await?;
println!("final trades={}", result.trades.len());
```

### 5. 合约查询

```rust
use serde_json::json;

let quotes = client
    .query_quotes(Some("FUTURE"), Some("SHFE"), None, Some(false), None)
    .await?;
let cont = client.query_cont_quotes(Some("SHFE"), Some("cu"), None).await?;
let options = client
    .query_options("SHFE.cu2405", Some("CALL"), Some(2024), Some(12), None, Some(false), None)
    .await?;

let raw = client
    .query_graphql(
        r#"query($class_:[Class]) {
  multi_symbol_info(class: $class_) {
    ... on basic { instrument_id }
  }
}"#,
        Some(json!({ "class_": ["FUTURE"] })),
    )
    .await?;

println!("quotes={:?} cont={:?} options={:?} raw={:?}", quotes, cont, options, raw);
```

### 6. Polars DataFrame 集成（可选功能）

启用 `polars` feature 后，可以直接将序列数据转换为 DataFrame：

```rust
subscription.start().await?;
let snapshot = subscription.wait_update().await?;

if snapshot.update.chart_ready {
    let df = subscription.load().await?.to_dataframe()?;
    println!("shape={:?}", df.shape());
}
```

如果需要增量维护窗口，可以结合 `KlineBuffer` / `TickBuffer` 使用。

### 7. 认证管理

#### 切换账号（运行时）

支持在运行时动态切换账号：

```rust
use tqsdk_rs::auth::TqAuth;

// 创建新的认证器
let mut new_auth = TqAuth::new("user2".to_string(), "pass2".to_string());
new_auth.login().await?;

// 切换认证器
client.set_auth(new_auth).await;

// 重新初始化行情（使用新账号）
client.init_market().await?;

#### 权限检查

```rust
let auth = client.get_auth().await;

if auth.has_feature("futr") {
    println!("有期货权限");
}

match auth.has_md_grants(&["SHFE.au2602", "SHFE.ag2512"]) {
    Ok(()) => println!("有行情权限"),
    Err(err) => println!("权限不足: {}", err),
}

match auth.has_md_grants(&["SSE.000300"]) {
    Ok(()) => println!("有指数行情权限"),
    Err(err) => println!("指数权限不足(需 lmt_idx): {}", err),
}
```

## 示例程序

### 运行示例

```bash
# 行情订阅
cargo run --example quote

# 历史数据与接口联调
cargo run --example history

# 实盘/模拟交易
cargo run --example trade

# 回测
cargo run --example backtest

# 枢轴点回放策略
cargo run --example pivot_point

# DataManager 高级用法
cargo run --example datamanager

# 自定义 tracing Layer
cargo run --example custom_logger

# 期权平值/实值/虚值查询
cargo run --example option_levels
```

### 示例说明

| 示例文件 | 主要内容 | 额外环境变量 |
|------|------|------|
| `quote.rs` | Quote、单合约 K 线、多合约对齐 K 线、Tick | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_LOG_LEVEL`、`TQ_LOG` |
| `history.rs` | 历史 K 线、接口联调、交易状态查询 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_TEST_SYMBOL` |
| `trade.rs` | 可靠事件流、账户/持仓监听、订单等待 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`、`SIMNOW_USER_0`、`SIMNOW_PASS_0` |
| `backtest.rs` | `ReplaySession` 构建、K 线注册、runtime 驱动、结果汇总 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_TEST_SYMBOL`、`TQ_POSITION_SIZE`、`TQ_LOG_LEVEL` |
| `pivot_point.rs` | 基于 `ReplaySession` 的日线枢轴点反转策略示例 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_TEST_SYMBOL`、`TQ_POSITION_SIZE`、`TQ_LOG_LEVEL` |
| `datamanager.rs` | `watch` / `unwatch`、路径读取、epoch | 无 |
| `custom_logger.rs` | `create_logger_layer()` 与业务日志组合 | 无 |
| `option_levels.rs` | 平值/实值/虚值期权查询 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_UNDERLYING`、`TQ_LOG_LEVEL` |

### 扩展环境变量配置

以下变量主要用于认证、网络或特殊示例：

| 变量 | 说明 |
|------|------|
| `TQ_AUTH_URL` | 认证服务地址，默认 `https://auth.shinnytech.com` |
| `TQ_MD_URL` | 覆盖行情服务地址 |
| `TQ_TD_URL` | 覆盖交易服务地址 |
| `TQ_INS_URL` | 覆盖合约信息地址 |
| `TQ_CHINESE_HOLIDAY_URL` | 覆盖交易日历假期数据源 |

## 技术栈

| 依赖 | 版本 | 用途 |
|------|------|------|
| `tokio` | 1.50 | 异步运行时 |
| `yawc` | 0.3.3 | WebSocket 客户端 |
| `reqwest` | 0.12 | HTTP / GraphQL 请求 |
| `serde` / `serde_json` | 1.0 | 序列化与 JSON |
| `jsonwebtoken` | 10.3 | JWT 认证 |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | 日志与可观测性 |
| `async-channel` | 2.5 | 异步通道 |
| `chrono` | 0.4 | 时间处理 |
| `polars` | 0.53 | 可选列式分析能力 |

## 项目结构

```text
tqsdk-rs/
├── src/
│   ├── auth/              # 认证与权限
│   ├── cache/             # 本地 K 线磁盘缓存（分段写入/压缩/区间读取）
│   ├── client/            # ClientBuilder / facade / market
│   ├── datamanager/       # DIFF 合并与 watch
│   ├── ins/               # 合约、期权、交易状态等查询
│   ├── quote/             # Quote 订阅
│   ├── replay/            # ReplaySession、回放内核、仿真撮合
│   ├── runtime/           # TqRuntime、TargetPosTask / Scheduler
│   ├── series/            # K 线 / Tick / 历史序列
│   ├── trade_session/     # 交易会话
│   ├── types/             # 公共数据结构
│   ├── websocket/         # WebSocket 核心与背压
│   ├── lib.rs
│   └── prelude.rs
├── examples/
│   ├── quote.rs
│   ├── history.rs
│   ├── trade.rs
│   ├── backtest.rs
│   ├── pivot_point.rs
│   ├── datamanager.rs
│   ├── custom_logger.rs
│   └── option_levels.rs
├── docs/
└── README.md
```

## 核心设计

### DIFF 协议实现

- 递归合并嵌套对象，减少全量重建。
- 保留按路径读取和按路径变化判断能力。
- 通过 epoch 追踪每轮更新，便于上层做增量处理。

### 类型安全

- Quote、Kline、Tick、Account、Order、Trade 等结构完整定义。
- 查询与交易接口统一返回 `Result<T, TqError>`。
- `InsertOrderRequest` 等结构体接口比旧版字符串参数更稳定。

### 并发与背压

- 共享状态主要通过 `Arc` 与锁保护。
- Quote、TradingStatus、WebSocket 离线发送使用有界队列。
- 背压策略优先限制内存占用；当消费者过慢时，部分更新会被丢弃并记录日志。

### Canonical 接口

- Quote：`QuoteSubscription` 负责订阅生命周期，`Client::quote()` 负责读取最新状态。
- Series：`Client::{kline,tick,kline_history,kline_history_with_focus}` 负责发起序列订阅，`Client::{kline_ref,tick_ref}` 负责读取 latest bar/tick，`SeriesSubscription` 负责多合约对齐窗口与历史窗口，并通过 `wait_update()` / `load()` 暴露快照。
- TradeSession：最新账户/持仓走快照读取，订单/成交走可靠事件流。

仍保留的回调接口大量使用 `Arc<T>`，适合多任务共享而不重复拷贝。

## 最佳实践

### 1. 延迟启动模式

```rust
let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
let quote_ref = client.quote("SHFE.au2602");

quote_sub.start().await?;

loop {
    quote_ref.wait_update().await?;
    println!("最新价: {}", quote_ref.load().await.last_price);
}
```

先创建订阅句柄，再显式 `start()`，可以把“声明订阅”和“消费状态”分离开。

### 2. 背压与慢消费者

- 如果只是做实时展示，允许通道丢弃旧更新通常比无限堆积更安全。
- 如果你需要尽量保留更多更新，可以提高 `message_queue_capacity()`。
- 如果你需要严格串行处理，优先使用单一消费路径，不要同时堆叠太多慢回调。

### 3. 错误处理

```rust
match client.subscribe_quote(&["SHFE.au2602"]).await {
    Ok(sub) => {
        sub.start().await?;
    }
    Err(err) => {
        eprintln!("订阅失败: {}", err);
    }
}
```

对权限、网络、超时和数据缺失类错误建议显式处理，不要统一吞掉。

### 4. 日志配置

```rust
use tqsdk_rs::{create_logger_layer, init_logger};

init_logger("debug", false);

let layer = create_logger_layer("info", false);
```

- 开发阶段建议 `debug`。
- 线上长期运行建议 `info` 或 `warn`。
- 如果要与业务日志系统合并，优先使用 `create_logger_layer()`。

### 5. 合约代码格式

始终使用完整合约代码：

```rust
"SHFE.au2602"
"DCE.m2505"
"CZCE.SR505"
```

避免省略交易所前缀或交割合约月份。

### 6. ViewWidth 与队列容量

- `view_width` 决定本地维护的序列窗口大小。
- `message_queue_capacity` 决定 Quote / TradingStatus / 离线发送等缓冲上限。
- `series_disk_cache_enabled` 默认 `false`；开启后会启用与官方 Python SDK 兼容的 `DataSeries` 历史快照缓存。
- `series_disk_cache_max_bytes` 可限制 `~/.tqsdk/data_series_1` 下缓存总大小（字节），超限时会按文件修改时间优先清理旧文件。
- `series_disk_cache_retention_days` 可按保留天数清理 `DataSeries` 历史缓存文件。
- 实时策略通常不需要盲目拉大这两个值，先按默认值运行，再按吞吐瓶颈调优。

## 与 Go 版本对比

| 维度 | Go 版本 | Rust 版本 |
|------|---------|-----------|
| 类型安全 | 较弱 | 更强 |
| 内存安全 | GC | 所有权系统 |
| 并发正确性 | 依赖运行时约束 | 更多在编译期发现 |
| 背压控制 | 需自行约束 | 当前版本已内建更多有界队列 |
| 数据分析扩展 | 借助第三方库 | 可选 `polars` feature |

## 注意事项

### 重要提示

1. 合约代码必须使用完整格式，如 `SHFE.au2602`。
2. Quote 和 Series 都需要显式 `start()` 才会推进状态；TradeSession 需要 `connect()`。
3. `SeriesSubscription` 是 coalesced snapshot API；用 `wait_update()` 等待下一次窗口更新，再用 `load()` 读取当前 `SeriesData`。
4. 通道类接口已采用有界缓冲；如果你观察到丢更新日志，优先检查消费者速度和队列容量。
5. 交易示例会访问真实交易接口，请优先使用模拟环境验证。

### 常见问题

**Q: 为什么收不到数据？**

- 检查 Quote/Series 是否已调用 `start()`，TradeSession 是否已 `connect()`。
- 对 Quote/Series，检查你是在等状态更新还是误用了已经删除的 callback/stream 接口。
- 检查合约代码格式和账户权限。

**Q: 为什么看到“通道已满，丢弃一次更新”？**

- 说明当前带缓冲的消费者处理速度跟不上生产速度。
- 提高 `message_queue_capacity()`，或减少回调/通道里的阻塞操作。

**Q: `SeriesSubscription` 什么时候该用 `wait_update()`，什么时候该用 `load()`？**

- `wait_update()` 用来等待下一次窗口快照推进。
- `load()` 用来读取当前最新 `SeriesData`；通常在 `wait_update()` 返回后立即调用。

**Q: 如何调试网络或权限问题？**

- 将日志级别切到 `debug`。
- 先运行 `cargo run --example history` 验证认证、GraphQL 和交易状态接口。
- 单独检查账号是否有目标合约和扩展数据权限。

### 报告问题

如果发现 Bug 或需要补充接口，请提交 [GitHub Issue](https://github.com/pseudocodes/tqsdk-rs/issues)。

## 许可证

本项目采用 Apache License 2.0，详见 [LICENSE](LICENSE)。

## 相关项目

- [tqsdk-go](https://github.com/pseudocodes/tqsdk-go)
- [tqsdk-python](https://github.com/shinnytech/tqsdk-python)

## 免责声明

本项目仅供学习和研究使用。

作者和贡献者不对使用本软件进行交易、投资或其他操作造成的任何直接或间接损失承担责任。期货和期权交易具有高风险，使用前请确认你已理解相关规则与风险。
