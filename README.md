# TQSDK-RS

天勤量化交易平台的 Rust SDK，提供行情订阅、K 线与 Tick 序列、合约查询、回测和实盘交易接口。

[![Crates.io](https://img.shields.io/crates/v/tqsdk-rs.svg)](https://crates.io/crates/tqsdk-rs)
[![Documentation](https://docs.rs/tqsdk-rs/badge.svg)](https://docs.rs/tqsdk-rs)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## 核心特性

- 类型安全：核心行情、K 线、Tick、账户、委托、成交等结构均为强类型定义。
- 异步优先：基于 `tokio`，统一行情、序列、交易与回测的异步接口。
- DIFF 协议：支持天勤增量数据合并、路径监听与 epoch 变化追踪。
- 多种消费方式：Quote/Series 同时支持回调、通道和流式消费。
- 延迟启动：先注册回调、再 `start()`/`connect()`，减少初始化阶段数据丢失。
- 背压可控：关键通道与离线队列已改为有界缓冲，避免慢消费者无限堆积内存。
- 零拷贝回调：大量更新路径通过 `Arc<T>` 分发，降低多消费者场景开销。
- Polars 集成：可选启用 `polars` feature，将序列数据直接转换为 DataFrame。
- 回测支持：支持历史回放、回测推进和 DataManager 直接读取。

## 功能模块

### 行情数据

- Quote 实时行情订阅，支持多合约动态增删。
- 单合约和多合约对齐 K 线订阅。
- Tick 订阅与历史 K 线拉取。
- 合约、主连、期权、交易日历、交易状态查询。

### 交易功能

- `TradeSession` 实盘/模拟交易会话。
- `TqRuntime` + `TargetPosTask` / `TargetPosScheduler` 目标持仓运行时与 Python 风格兼容 facade。
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
- 7 个示例程序覆盖主要使用路径。

## 近期修复与更新

- 修复行情 WebSocket 断线重连后未完成 `ins_query` 可能丢失或重复的问题。
- 修复 `query_cont_quotes` 在未提供 `has_night` 时仍发送该变量导致的超时问题。
- 修复 `TradeSession::connect()` 失败后后台任务仍可能继续重连或保活的问题。
- 修复 `SeriesSubscription::data_stream()` 覆盖 `on_update()` 的接口行为：
  `data_stream()` 现在与 `on_update()` 可并存，不再互相覆盖。
- 将 Quote、TradingStatus、Series stream、DataManager watch 与 WebSocket 离线发送队列改为有界缓冲，慢消费者场景下以丢弃更新替代无限堆积。
- 修复 `has_md_grants` 的指数权限判断顺序：
  `SSE.000016` / `SSE.000300` / `SSE.000905` / `SSE.000852` 现在会优先校验 `lmt_idx` 权限。
- 修复 `BacktestHandle` 生命周期中回调未注销的问题，实例释放后会自动取消 DataManager 回调注册。
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

    let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;

    quote_sub
        .on_quote(|quote| {
            println!("{} 最新价 = {}", quote.instrument_id, quote.last_price);
        })
        .await;

    quote_sub.start().await?;
    tokio::signal::ctrl_c().await?;
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

### 目标持仓任务（compat facade）

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;

async fn demo(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let task = TargetPosTask::new(
        runtime,
        "SIM",
        "SHFE.rb2601",
        TargetPosTaskOptions::default(),
    )
    .await?;

    task.set_target_volume(1)?;
    task.wait_target_reached().await?;
    task.cancel().await?;
    task.wait_finished().await?;
    Ok(())
}
```

### 时间分片调仓（compat facade）

```rust
use std::sync::Arc;
use std::time::Duration;
use tqsdk_rs::prelude::*;

async fn demo_scheduler(runtime: Arc<TqRuntime>) -> RuntimeResult<()> {
    let scheduler = TargetPosScheduler::new(
        runtime,
        "SIM",
        "SHFE.rb2601",
        vec![
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
        ],
        TargetPosSchedulerOptions::default(),
    )
    .await?;

    scheduler.wait_finished().await?;
    println!("aggregated trades: {}", scheduler.execution_report().trades.len());
    Ok(())
}
```

### 回测 Runtime 调仓

```rust
use std::sync::Arc;
use tqsdk_rs::prelude::*;
use tqsdk_rs::runtime::LiveMarketAdapter;

fn build_backtest_runtime(backtest: &BacktestHandle) -> Arc<TqRuntime> {
    Arc::new(TqRuntime::with_id(
        "backtest-runtime",
        RuntimeMode::Backtest,
        Arc::new(LiveMarketAdapter::new(backtest.dm())),
        Arc::new(BacktestExecutionAdapter::new(vec!["TQSIM".to_string()])),
    ))
}
```

`BacktestExecutionAdapter` 当前提供的是 runtime 任务可复用的内存内成交模型，可用于 `TargetPosTask` / `TargetPosScheduler` 回测驱动，不是完整交易所撮合模拟器。

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

### 1. 行情订阅

#### Quote 订阅 - 实时行情

```rust
let quote_sub = client
    .subscribe_quote(&["SHFE.au2602", "SHFE.ag2512"])
    .await?;

quote_sub
    .on_quote(|quote| {
        println!("{} 最新价: {}", quote.instrument_id, quote.last_price);
    })
    .await;

let quote_rx = quote_sub.quote_channel();
tokio::spawn(async move {
    while let Ok(quote) = quote_rx.recv().await {
        println!("[channel] {} {}", quote.instrument_id, quote.last_price);
    }
});

quote_sub.start().await?;
quote_sub.add_symbols(&["DCE.m2505"]).await?;
quote_sub.remove_symbols(&["SHFE.ag2512"]).await?;
```

说明：

- 先注册回调，再 `start()`。
- `quote_channel()` 现在是有界通道；消费者长期跟不上时，旧更新可能被丢弃。
- 默认容量由 `ClientBuilder::message_queue_capacity()` 控制。

#### K 线订阅 - 单合约

```rust
use std::time::Duration;

let series_api = client.series()?;
let sub = series_api
    .kline("SHFE.au2602", Duration::from_secs(60), 300)
    .await?;

sub.on_update(|data, info| {
    if let Some(klines) = data.get_symbol_klines("SHFE.au2602") {
        if info.has_new_bar && let Some(last) = klines.data.last() {
            println!("新 K 线: id={} close={}", last.id, last.close);
        }

        if info.has_bar_update {
            println!("最新 bar 已更新，当前总数={}", klines.data.len());
        }
    }
})
.await;

sub.start().await?;
```

补充：

- `SeriesSubscription::on_update()` 仍然是单回调语义，后注册会覆盖先注册。
- `SeriesSubscription::data_stream()` 现在是独立流接口，可与 `on_update()` 同时使用。
- `data_stream()` 也是有界缓冲；如果处理速度跟不上，会丢弃部分更新并输出告警。

#### 多合约对齐 K 线

```rust
use std::time::Duration;

let symbols = vec!["SHFE.au2602".to_string(), "SHFE.ag2512".to_string()];
let sub = series_api.kline(&symbols, Duration::from_secs(60), 120).await?;

sub.on_update(|data, info| {
    if info.has_new_bar && let Some(multi) = &data.multi {
        println!("主合约: {}", multi.main_symbol);
        if let Some(row) = multi.data.last() {
            for (symbol, kline) in &row.klines {
                println!("{} close={}", symbol, kline.close);
            }
        }
    }
})
.await;

sub.start().await?;
```

#### Tick 订阅 - 逐笔成交

```rust
let sub = series_api.tick("SHFE.au2602", 200).await?;

sub.on_update(|data, _info| {
    if let Some(tick_data) = &data.tick_data
        && let Some(last_tick) = tick_data.data.last()
    {
        println!(
            "tick id={} last_price={} volume={}",
            last_tick.id, last_tick.last_price, last_tick.volume
        );
    }
})
.await;

sub.start().await?;
```

#### 历史数据获取

```rust
use chrono::Utc;
use std::time::Duration;

let sub = series_api
    .kline_history("SHFE.au2602", Duration::from_secs(60), 8000, 105761)
    .await?;

sub.on_update(|data, info| {
    if info.chart_ready && let Some(klines) = data.get_symbol_klines("SHFE.au2602") {
        println!("历史数据加载完成，共 {} 根", klines.data.len());
    }
})
.await;

sub.start().await?;

let focus_time = Utc::now() - chrono::Duration::days(7);
let sub_with_focus = series_api
    .kline_history_with_focus(
        "SHFE.au2602",
        Duration::from_secs(60),
        1000,
        focus_time,
        50,
    )
    .await?;

sub_with_focus.start().await?;
```

常用场景：

- `kline_history(..., left_kline_id)`：按已知 K 线 ID 精确回溯。
- `kline_history_with_focus(..., focus_datetime, focus_position)`：按时间定位，便于围绕某个时间点取窗口。
- `kline_data_series(..., start_dt, end_dt)`：按时间区间获取 K 线快照，语义为 `[start_dt, end_dt)`。
- `tick_data_series(..., start_dt, end_dt)`：按时间区间获取 Tick 快照，语义为 `[start_dt, end_dt)`。
- 当启用 Series 磁盘缓存时，Rust SDK 会使用与官方 Python SDK 兼容的 `~/.tqsdk/data_series_1` 缓存目录，可直接共用缓存文件。

```rust
use chrono::Utc;

let end_dt = Utc::now();
let start_dt = end_dt - chrono::Duration::hours(2);
let snapshot_by_dt = series_api
    .kline_data_series("SHFE.au2602", Duration::from_secs(60), start_dt, end_dt)
    .await?;
println!("时间窗口快照条数: {}", snapshot_by_dt.len());

let tick_snapshot = series_api
    .tick_data_series("SHFE.au2602", start_dt, end_dt)
    .await?;
println!("Tick 快照条数: {}", tick_snapshot.len());
```

### 2. 交易功能

#### 创建交易会话

```rust
let session = client
    .create_trade_session("simnow", &sim_user_id, &sim_password)
    .await?;
```

如需覆盖交易地址，可以使用 `TradeSessionOptions`：

```rust
use tqsdk_rs::TradeSessionOptions;

let session = client
    .create_trade_session_with_options(
        "simnow",
        &sim_user_id,
        &sim_password,
        TradeSessionOptions {
            td_url_override: Some("wss://example.com/trade".to_string()),
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

session
    .on_order(|order| {
        println!("订单 {} 状态={}", order.order_id, order.status);
    })
    .await;

session.connect().await?;

while !session.is_ready() {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}
```

优先级为：`TradeSessionOptions.td_url_override` > `ClientBuilder::td_url` /
`EndpointConfig.td_url` > `TQ_TD_URL` > 鉴权返回的默认交易地址。

说明：

- 交易会话同样推荐先注册所有回调，再 `connect()`。
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

DataManager 是底层 DIFF 合并与数据读取核心。大多数场景不需要直接操作，但回测或高级扩展时可以直接使用：

```rust
use chrono::Utc;
use tqsdk_rs::{BacktestConfig, Client};

let username = std::env::var("TQ_AUTH_USER")?;
let password = std::env::var("TQ_AUTH_PASS")?;

let mut client = Client::builder(username, password).build().await?;
let start = Utc::now() - chrono::Duration::days(7);
let end = Utc::now();
let backtest = client
    .init_market_backtest(BacktestConfig::new(start, end))
    .await?;

let dm = backtest.dm();

// 获取 Quote 数据
if let Ok(quote) = dm.get_quote_data("SHFE.au2602") {
    println!("最新价: {}", quote.last_price);
    println!("买一价: {}", quote.bid_price1);
    println!("卖一价: {}", quote.ask_price1);
}

// 获取 K线数据
// 参数：合约代码, 周期(纳秒), 数量, right_id(-1表示最新)
if let Ok(klines) = dm.get_klines_data("SHFE.au2602", 60_000_000_000, 100, -1) {
    println!("K线数量: {}", klines.data.len());
}

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

### 4. 回测与历史回放

```rust
use chrono::Utc;
use tqsdk_rs::{BacktestConfig, BacktestEvent};

let username = std::env::var("TQ_AUTH_USER")?;
let password = std::env::var("TQ_AUTH_PASS")?;

let mut client = Client::builder(username, password).build().await?;
let start = Utc::now() - chrono::Duration::days(7);
let end = Utc::now();

let backtest = client
    .init_market_backtest(BacktestConfig::new(start, end))
    .await?;

let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
quote_sub.start().await?;

loop {
    match backtest.next().await? {
        BacktestEvent::Tick { current_dt } => {
            println!("推进到 {}", current_dt);
        }
        BacktestEvent::Finished { current_dt } => {
            println!("回测结束 {}", current_dt);
            break;
        }
    }
}
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
subscription
    .on_update(|series_data, _| {
        if let Ok(df) = series_data.to_dataframe() {
            println!("shape={:?}", df.shape());
        }
    })
    .await;

subscription.start().await?;
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
| `trade.rs` | 交易回调、账户/持仓/委托/成交监听 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`、`SIMNOW_USER_0`、`SIMNOW_PASS_0` |
| `backtest.rs` | 回测推进、区间参数、结果汇总 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_START_DT`、`TQ_END_DT`、`TQ_MAX_UPDATES`、`TQ_POSITION_SIZE` |
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
│   ├── backtest/          # 回测实现
│   ├── cache/             # 本地 K 线磁盘缓存（分段写入/压缩/区间读取）
│   ├── client/            # ClientBuilder / facade / market
│   ├── datamanager/       # DIFF 合并与 watch
│   ├── ins/               # 合约、期权、交易状态等查询
│   ├── quote/             # Quote 订阅
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
- Quote、TradingStatus、Series stream、WebSocket 离线发送使用有界队列。
- 背压策略优先限制内存占用；当消费者过慢时，部分更新会被丢弃并记录日志。

### 灵活接口

- Quote：回调 + Channel。
- Series：回调 + `data_stream()`。
- TradeSession：回调 + 主动查询。

### 零拷贝回调设计

```rust
quote_sub.on_quote(|quote| {
    println!("{}", quote.last_price);
}).await;

series_sub.on_update(|data, info| {
    if info.has_new_bar {
        println!("新 K 线");
    }
}).await;

// 注册回调后，别忘了显式启动
quote_sub.start().await?;
series_sub.start().await?;
```

回调参数大量使用 `Arc<T>`，适合多任务共享而不重复拷贝。

## 最佳实践

### 1. 延迟启动模式

```rust
let sub = series_api
    .kline("SHFE.au2602", std::time::Duration::from_secs(60), 100)
    .await?;

sub.on_update(|data, info| {
    println!("收到更新: has_new_bar={}", info.has_new_bar);
}).await;

sub.start().await?;
```

不推荐先 `start()` 再注册回调，否则初始化阶段可能错过首批数据。

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
- `message_queue_capacity` 决定 Quote / Series stream / 离线发送等缓冲上限。
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
2. 先注册回调，再 `start()` 或 `connect()`。
3. `on_update()` 只有一个槽位；如需额外消费，请使用 `data_stream()` 或自行转发。
4. 通道和流式接口已采用有界缓冲；如果你观察到丢更新日志，优先检查消费者速度和队列容量。
5. 交易示例会访问真实交易接口，请优先使用模拟环境验证。

### 常见问题

**Q: 为什么收不到数据？**

- 检查是否已调用 `start()`。
- 检查回调是否在 `start()` 之前注册。
- 检查合约代码格式和账户权限。

**Q: 为什么看到“通道已满，丢弃一次更新”？**

- 说明当前消费者处理速度跟不上生产速度。
- 提高 `message_queue_capacity()`，或减少回调/通道里的阻塞操作。

**Q: `data_stream()` 会覆盖 `on_update()` 吗？**

- 不会。当前版本两者可以同时工作。
- 但 `on_update()` 仍然只有一个回调槽位，后注册会覆盖先注册。

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
