# TQSDK-RS

天勤量化交易平台的 Rust SDK - 高性能、类型安全的期货交易接口

[![Crates.io](https://img.shields.io/crates/v/tqsdk-rs.svg)](https://crates.io/crates/tqsdk-rs)
[![Documentation](https://docs.rs/tqsdk-rs/badge.svg)](https://docs.rs/tqsdk-rs)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## 核心特性

- **类型安全** - 使用 Rust 强类型系统，90+ 字段完整定义，编译时消除运行时错误
- **并发安全** - 基于 Arc + RwLock 设计，确保多线程环境下的数据安全，无数据竞争
- **异步优化** - 基于 tokio 异步运行时，高效处理 I/O 操作，零成本抽象
- **DIFF 协议** - 完整实现天勤 DIFF 协议，支持增量数据更新和递归合并
- **灵活接口** - 支持 Channel、Callback、Stream 三种数据订阅方式，满足不同场景需求
- **零竞态条件** - 支持延迟启动模式，确保回调注册完成后再启动监听，避免数据丢失
- **零拷贝回调** - 使用 Arc 参数优化，多回调场景下性能提升 50-100x
- **Polars 集成** - 高性能列式数据分析，支持 K线/Tick 数据转换为 DataFrame（可选功能）
- **灵活日志系统** - 支持 Layer 组合，本地时区显示，可与业务层日志集成

## 功能模块

### 行情数据
- 实时行情订阅（Quote）- 支持多合约同时订阅
- K线数据订阅（单合约/多合约对齐）- 支持任意周期
- Tick 数据订阅 - 逐笔成交数据
- 历史数据获取（支持 left_kline_id 和 focus_datetime 两种方式）
- ViewWidth 限制和二分查找优化 - 高效处理大数据集
- **Polars DataFrame 集成** - 高性能数据分析（可选功能）

### 交易功能
- 实盘交易（TradeSession）- 支持期货公司实盘和 SimNow 模拟
- 账户信息查询 - 实时资金、权益、保证金等
- 持仓/委托单/成交查询 - 完整的交易数据
- 下单/撤单操作 - 支持限价单、市价单等
- 自动重连机制 - 网络断开自动恢复

### 数据管理
- DIFF 协议数据合并 - 递归合并嵌套对象
- 路径监听（Watch/UnWatch）- 精确监听指定路径的数据变化
- 版本追踪（Epoch）- 追踪每次数据更新
- 数据类型转换 - JSON 到强类型结构体的自动转换

### 性能优化
- **零拷贝回调设计** - Arc 参数优化，避免数据深拷贝
- **高性能缓冲区** - KlineBuffer/TickBuffer 支持 O(1) 更新
- **列式数据分析** - Polars DataFrame 集成（可选）

### 开发体验
- **灵活日志系统** - 支持 Layer 组合和本地时区
- **完整示例** - 8+ 示例程序覆盖所有功能
- **详细文档** - 完整的 API 文档和使用指南

## 快速开始

### 安装依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
tqsdk-rs = "0.1.0"
tokio = { version = "1", features = ["full"] }

# 可选：启用 Polars DataFrame 支持
tqsdk-rs = { version = "0.1.0", features = ["polars"] }
```

### 基础示例 - 行情订阅

```rust
use tqsdk_rs::{Client, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 创建客户端（自动完成认证）
    let mut client = Client::new("username", "password", ClientConfig::default()).await?;
    
    // 2. 初始化行情连接
    client.init_market().await?;
    
    // 3. 订阅行情（支持多个合约）
    let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
    
    // 4. 注册回调函数
    quote_sub.on_quote(|quote| {
        println!("行情更新: {} = {}", quote.instrument_id, quote.last_price);
    }).await;
    
    // 5. 启动订阅
    quote_sub.start().await?;
    
    // 6. 保持运行
    tokio::signal::ctrl_c().await?;
    
    Ok(())
}
```

### 使用 ClientBuilder（推荐）

ClientBuilder 提供了更灵活的配置方式：

```rust
use tqsdk_rs::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 使用构建器模式创建客户端
    let mut client = Client::builder("username", "password")
        .log_level("debug")          // 设置日志级别
        .view_width(5000)            // 设置默认视图宽度
        .development(true)           // 开启开发模式
        .build()
        .await?;
    
    // 初始化行情
    client.init_market().await?;
    
    Ok(())
}
```

## 核心功能详解

### 1. 行情订阅

#### Quote 订阅 - 实时行情

Quote 订阅支持多合约同时订阅，提供三种数据接收方式：

```rust
// 订阅多个合约的实时行情
let quote_sub = client.subscribe_quote(&["SHFE.au2602", "SHFE.ag2512"]).await?;

// 方式 1: 使用回调函数（推荐）
quote_sub.on_quote(|quote| {
    println!("合约: {}", quote.instrument_id);
    println!("最新价: {}", quote.last_price);
    println!("成交量: {}", quote.volume);
}).await;

// 方式 2: 使用 Channel（支持多个消费者）
let rx = quote_sub.quote_channel();
tokio::spawn(async move {
    while let Ok(quote) = rx.recv().await {
        println!("收到行情: {:?}", quote);
    }
});

// 启动订阅（延迟启动模式）
quote_sub.start().await?;

// 动态添加合约
quote_sub.add_symbols(&["DCE.m2505"]).await?;

// 动态移除合约
quote_sub.remove_symbols(&["SHFE.ag2512"]).await?;
```

#### K线订阅 - 单合约

K线订阅支持任意周期，使用延迟启动模式避免数据丢失：

```rust
use std::time::Duration;

// 获取 Series API
let series_api = client.series()?;

// 订阅 1 分钟 K线，获取最近 100 根
let sub = series_api.kline(
    "SHFE.au2602",           // 合约代码
    Duration::from_secs(60), // K线周期（60秒 = 1分钟）
    100                      // 数据条数
).await?;

// 先注册回调（重要：避免丢失初始数据）
// 注意：data 和 info 都是 Arc 包装的，零拷贝共享
sub.on_update(|data, info| {
    if let Some(kline_data) = &data.single {
        println!("K线数量: {}", kline_data.data.len());
        
        // 检查是否有新 K线生成
        if info.has_new_bar {
            println!("新 K线生成！");
            if let Some(last_kline) = kline_data.data.last() {
                println!("开: {}, 高: {}, 低: {}, 收: {}", 
                    last_kline.open, last_kline.high, 
                    last_kline.low, last_kline.close);
            }
        }
        
        // 检查是否有 K线更新
        if info.has_bar_update {
            println!("K线数据更新");
        }
    }
}).await;

// 最后启动订阅
sub.start().await?;

// 常用周期示例
// Duration::from_secs(60)      // 1 分钟
// Duration::from_secs(300)     // 5 分钟
// Duration::from_secs(900)     // 15 分钟
// Duration::from_secs(3600)    // 1 小时
// Duration::from_secs(86400)   // 1 天
```

#### 多合约对齐 K线

多合约订阅会自动进行时间对齐，适用于跨品种分析：

```rust
// 订阅多个合约的 K线（自动时间对齐）
let symbols = vec![
    "SHFE.au2602".to_string(),  // 黄金
    "SHFE.ag2512".to_string(),  // 白银
];

let sub = series_api.kline_multi(
    &symbols,                    // 合约列表
    Duration::from_secs(60),     // K线周期
    100                          // 数据条数
).await?;

sub.on_update(|data, _info| {
    if let Some(multi_data) = &data.multi {
        println!("主合约: {}", multi_data.main_symbol);
        
        // 遍历对齐后的 K线集合
        for aligned_set in &multi_data.data {
            println!("时间: {}", aligned_set.timestamp);
            
            // 每个时间点包含所有合约的 K线
            for (symbol, kline) in &aligned_set.klines {
                println!("  {} - 开: {}, 收: {}", 
                    symbol, kline.open, kline.close);
            }
        }
    }
}).await;

sub.start().await?;
```

#### Tick 订阅 - 逐笔成交

Tick 数据提供最细粒度的市场数据：

```rust
// 订阅 Tick 数据
let sub = series_api.tick(
    "SHFE.au2602",  // 合约代码
    100             // 数据条数
).await?;

sub.on_update(|data, _info| {
    if let Some(tick_data) = &data.tick_data {
        println!("Tick 数量: {}", tick_data.data.len());
        
        // 获取最新 Tick
        if let Some(last_tick) = tick_data.data.last() {
            println!("最新成交价: {}", last_tick.last_price);
            println!("成交量: {}", last_tick.volume);
            println!("持仓量: {}", last_tick.open_interest);
        }
    }
}).await;

sub.start().await?;
```

#### 历史数据获取

支持两种方式获取历史数据：

```rust
use chrono::Utc;

// 方式 1: 使用 left_kline_id（精确定位）
// 从指定 K线 ID 开始获取
let sub = series_api.kline_history(
    "SHFE.au2602",              // 合约代码
    Duration::from_secs(60),    // K线周期
    100,                        // 数据条数
    1234567890                  // 起始 K线 ID
).await?;

// 方式 2: 使用 focus_datetime（时间定位）
// 从指定时间点开始获取
let focus_time = Utc::now() - chrono::Duration::days(7);  // 7天前
let sub = series_api.kline_history_with_focus(
    "SHFE.au2602",              // 合约代码
    Duration::from_secs(60),    // K线周期
    100,                        // 数据条数
    focus_time,                 // 焦点时间
    50                          // 焦点位置（0-100，50表示居中）
).await?;

// 注册回调处理历史数据
sub.on_update(|data, info| {
    if let Some(kline_data) = &data.single {
        println!("获取到 {} 根历史 K线", kline_data.data.len());
        
        if info.chart_ready {
            println!("历史数据加载完成");
        }
    }
}).await;

sub.start().await?;
```

### 2. 交易功能

#### 创建交易会话

TradeSession 使用延迟连接模式，避免消息丢失：

```rust
// 创建交易会话（不自动连接）
let session = client.create_trade_session(
    "simnow",      // 期货公司代码（simnow 为模拟账户）
    "user_id",     // 账号
    "password"     // 密码
).await?;

// 步骤 1: 先注册所有回调（重要：避免丢失初始数据）

// 监听账户变化
session.on_account(|account| {
    println!("账户余额: {}", account.balance);
    println!("可用资金: {}", account.available);
    println!("持仓盈亏: {}", account.position_profit);
}).await;

// 监听持仓变化
session.on_position(|position| {
    println!("合约: {}", position.instrument_id);
    println!("多头持仓: {}", position.volume_long);
    println!("空头持仓: {}", position.volume_short);
}).await;

// 监听委托单变化
session.on_order(|order| {
    println!("委托单: {} - 状态: {}", order.order_id, order.status);
}).await;

// 监听成交记录
session.on_trade(|trade| {
    println!("成交: {} - 价格: {}", trade.trade_id, trade.price);
}).await;

// 步骤 2: 最后连接服务器
session.connect().await?;
```

#### 下单操作

支持限价单、市价单等多种下单方式：

```rust
// 限价开多仓
let order_id = session.insert_order(
    "SHFE.au2602",    // 合约代码
    "BUY",            // 买卖方向：BUY/SELL
    "OPEN",           // 开平标志：OPEN/CLOSE/CLOSETODAY
    1,                // 手数
    Some(500.0)       // 限价（None 表示市价单）
).await?;

println!("下单成功，委托单号: {}", order_id);

// 市价平仓
let order_id = session.insert_order(
    "SHFE.au2602",
    "SELL",
    "CLOSE",
    1,
    None              // 市价单
).await?;

// 撤单
session.cancel_order(&order_id).await?;
println!("撤单成功");
```

#### 查询交易数据

提供完整的交易数据查询接口：

```rust
// 查询账户信息
let account = session.get_account("CNY").await?;
println!("账户余额: {}", account.balance);
println!("可用资金: {}", account.available);
println!("冻结保证金: {}", account.frozen_margin);
println!("持仓盈亏: {}", account.position_profit);

// 查询持仓
let position = session.get_position("SHFE.au2602").await?;
println!("多头持仓: {}", position.volume_long);
println!("空头持仓: {}", position.volume_short);
println!("持仓均价: {}", position.open_price_long);

// 查询委托单
let order = session.get_order(&order_id).await?;
println!("委托状态: {}", order.status);
println!("已成交: {} / {}", order.volume_orign - order.volume_left, order.volume_orign);

// 查询成交记录
let trade = session.get_trade(&trade_id).await?;
println!("成交价格: {}", trade.price);
println!("成交数量: {}", trade.volume);
```

### 3. 数据管理器（DataManager）

DataManager 是底层数据存储，实现了 DIFF 协议：

#### 直接访问数据

```rust
// 获取 DataManager 实例（需要先初始化 client）
let dm = client.dm.clone();

// 获取 Quote 数据
let quote = dm.get_quote_data("SHFE.au2602")?;
println!("最新价: {}", quote.last_price);
println!("买一价: {}", quote.bid_price1);
println!("卖一价: {}", quote.ask_price1);

// 获取 K线数据
// 参数：合约代码, 周期(纳秒), 数量, right_id(-1表示最新)
let klines = dm.get_klines_data("SHFE.au2602", 60_000_000_000, 100, -1)?;
println!("K线数量: {}", klines.data.len());

// 路径访问（灵活访问任意数据）
if let Some(data) = dm.get_by_path(&["quotes", "SHFE.au2602"]) {
    println!("原始数据: {:?}", data);
}

// 检查数据是否在最近一次更新中发生变化
if dm.is_changing(&["quotes", "SHFE.au2602"]) {
    println!("数据在最近一次更新中发生了变化");
}

// 获取当前版本号
let epoch = dm.get_epoch();
println!("当前数据版本: {}", epoch);
```

#### 路径监听（Watch/UnWatch）

精确监听指定路径的数据变化：

```rust
// 监听指定路径的数据变化
let rx = dm.watch(vec![
    "quotes".to_string(), 
    "SHFE.au2602".to_string()
]);

// 在另一个任务中接收更新
tokio::spawn(async move {
    while let Ok(data) = rx.recv().await {
        println!("路径数据更新: {:?}", data);
    }
});

// 监听多个路径
let symbols = vec!["SHFE.au2602", "SHFE.ag2512", "DCE.m2505"];
for symbol in &symbols {
    let rx = dm.watch(vec!["quotes".to_string(), symbol.to_string()]);
    // 处理每个路径的更新
}

// 取消监听
dm.unwatch(&vec![
    "quotes".to_string(),
    "SHFE.au2602".to_string()
])?;
```

#### 数据更新回调

注册全局数据更新回调：

```rust
// 注册数据更新回调（每次数据更新时触发）
dm.on_data(|| {
    println!("数据已更新，当前版本: {}", dm.get_epoch());
    // 可以在这里触发其他操作
});

// 可以注册多个回调
dm.on_data(|| {
    // 回调 1
});

dm.on_data(|| {
    // 回调 2
});
```

### 4. Polars DataFrame 集成（可选功能）

启用 `polars` 功能后，可以将 K线和 Tick 数据转换为 Polars DataFrame 进行高性能分析。

#### 使用 KlineBuffer 进行实时数据分析

```rust
use tqsdk_rs::{Client, ClientConfig, KlineBuffer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new("username", "password", ClientConfig::default()).await?;
    client.init_market().await?;

    let series_api = client.get_series_api();
    let subscription = series_api
        .kline("SHFE.au2506", Duration::from_secs(60), 100)
        .await?;

    // 创建 K线缓冲区
    let mut buffer = KlineBuffer::new();

    subscription.on_update(move |series_data, update_info| {
        if let Some(kline_data) = &series_data.single {
            if let Some(last_kline) = kline_data.data.last() {
                if update_info.has_new_bar {
                    // 新 K线，追加
                    buffer.push(last_kline);
                } else if update_info.has_bar_update {
                    // 更新最后一根
                    buffer.update_last(last_kline);
                }
            }

            // 转换为 DataFrame 进行分析
            if let Ok(df) = buffer.to_dataframe() {
                println!("DataFrame shape: {:?}", df.shape());
                
                // 计算统计指标
                if let Ok(close) = df.column("close")?.f64() {
                    let mean = close.mean().unwrap_or(0.0);
                    let std = close.std(1).unwrap_or(0.0);
                    println!("收盘价均值: {:.2}, 标准差: {:.2}", mean, std);
                }

                // 获取最后 10 根 K线
                if let Ok(tail_df) = buffer.tail(10) {
                    println!("最后 10 根K线:\n{}", tail_df);
                }
            }
        }
    }).await;

    subscription.start().await?;
    
    // 等待数据...
    tokio::time::sleep(Duration::from_secs(60)).await;
    
    Ok(())
}
```

#### 直接转换 SeriesData

```rust
// 单合约 K线
subscription.on_update(|series_data, _| {
    // 转换为 DataFrame
    if let Ok(df) = series_data.to_dataframe() {
        println!("K线数据:\n{}", df);
    }
}).await;

// 多合约 K线（长表格式）
multi_subscription.on_update(|series_data, _| {
    if let Ok(long_df) = series_data.to_dataframe() {
        println!("长表格式:\n{}", long_df);
    }
}).await;

// 多合约 K线（宽表格式）
multi_subscription.on_update(|series_data, _| {
    if let Ok(wide_df) = series_data.to_wide_dataframe() {
        println!("宽表格式:\n{}", wide_df);
    }
}).await;
```

#### 技术指标计算

```rust
use polars::prelude::*;

// 计算移动平均线
fn calculate_ma(df: &DataFrame, window: usize) -> Result<Series, PolarsError> {
    let close = df.column("close")?.f64()?;
    let ma = close.rolling_mean(RollingOptionsFixedWindow {
        window_size: window,
        min_periods: window,
        ..Default::default()
    })?;
    Ok(ma.into_series())
}

// 在回调中使用
subscription.on_update(move |series_data, _| {
    if let Ok(df) = buffer.to_dataframe() {
        // 计算 MA5 和 MA10
        if let Ok(ma5) = calculate_ma(&df, 5) {
            if let Ok(ma10) = calculate_ma(&df, 10) {
                println!("MA5: {:.2}, MA10: {:.2}", 
                    ma5.tail(Some(1)), 
                    ma10.tail(Some(1)));
            }
        }
    }
}).await;
```

**详细文档**: 查看 [POLARS_INTEGRATION.md](./POLARS_INTEGRATION.md) 了解完整的 Polars 集成指南。

### 5. 认证管理

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
```

#### 权限检查

在订阅前自动检查权限，也可以手动检查：

```rust
// 获取认证器
let auth = client.get_auth().await;

// 检查功能权限
if auth.has_feature("futr") {
    println!("有期货权限");
}

if auth.has_feature("sec") {
    println!("有股票权限");
}

// 检查行情权限（多个合约）
match auth.has_md_grants(&["SHFE.au2602", "SHFE.ag2512"]) {
    Ok(_) => println!("有行情权限"),
    Err(e) => println!("权限不足: {}", e),
}

// 检查交易权限（单个合约）
match auth.has_td_grants("SHFE.au2602") {
    Ok(_) => println!("有交易权限"),
    Err(e) => println!("权限不足: {}", e),
}

// 获取认证信息
println!("Auth ID: {}", auth.get_auth_id());
println!("Access Token: {}", auth.get_access_token());
```

## 示例程序

项目提供了完整的示例程序，涵盖所有核心功能：

### 运行示例

```bash
# 行情订阅示例（Quote、K线、Tick）
cargo run --example quote

# 历史数据获取示例
cargo run --example history

# 交易操作示例（下单、撤单、查询）
cargo run --example trade

# DataManager 高级功能示例
cargo run --example datamanager

# 认证功能示例
cargo run --example auth_demo

# 账号切换示例
cargo run --example auth_switch

# ClientBuilder 使用示例
cargo run --example client_builder

# Polars DataFrame 集成示例（需要启用 polars 功能）
cargo run --example polars_demo --features polars

# 自定义日志 Layer 组合示例
cargo run --example custom_logger
```

### 环境变量配置

运行示例前需要设置环境变量：

```bash
# 天勤账号（必需）
export SHINNYTECH_ID="your_username"
export SHINNYTECH_PW="your_password"

# SimNow 模拟账号（仅交易示例需要）
export SIMNOW_USER_0="your_simnow_user"
export SIMNOW_PASS_0="your_simnow_pass"
```

### 示例说明

| 示例文件 | 功能说明 | 适用场景 |
|---------|---------|---------|
| quote.rs | Quote、K线、Tick 订阅 | 学习行情订阅 |
| history.rs | 历史数据获取 | 回测、数据分析 |
| trade.rs | 下单、撤单、查询 | 实盘交易 |
| datamanager.rs | 数据管理器高级用法 | 自定义数据处理 |
| auth_demo.rs | 认证和权限检查 | 了解认证机制 |
| auth_switch.rs | 运行时切换账号 | 多账号管理 |
| client_builder.rs | ClientBuilder 用法 | 灵活配置客户端 |
| **polars_demo.rs** | **Polars DataFrame 集成** | **数据分析和技术指标** |
| **custom_logger.rs** | **自定义日志 Layer** | **日志系统集成** |

## 技术栈

| 依赖 | 版本 | 用途 |
|------|------|------|
| tokio | 1.48 | 异步运行时 |
| yawc | 0.2.7 | WebSocket 客户端（支持 deflate 压缩） |
| reqwest | 0.12 | HTTP 客户端 |
| serde | 1.0 | 序列化/反序列化 |
| serde_json | 1.0 | JSON 处理 |
| jsonwebtoken | 10.2 | JWT 认证 |
| thiserror | 2.0 | 错误处理 |
| tracing | 0.1 | 结构化日志 |
| chrono | 0.4 | 时间处理 |
| async-channel | 2.3 | 异步通道 |
| polars | 0.44 | 数据分析（可选） |

## 项目结构

```
tqsdk-rs/
├── src/
│   ├── lib.rs              # 库入口和模块导出
│   ├── client.rs           # 客户端和 ClientBuilder
│   ├── auth.rs             # 认证模块（TqAuth）
│   ├── websocket.rs        # WebSocket 封装
│   ├── datamanager.rs      # 数据管理器（DIFF 协议）
│   ├── types.rs            # 数据结构定义（90+ 字段）
│   ├── quote.rs            # Quote 订阅
│   ├── series.rs           # Series API（K线/Tick）
│   ├── trade_session.rs    # 交易会话
│   ├── utils.rs            # 工具函数
│   ├── logger.rs           # 日志系统
│   └── errors.rs           # 错误类型
├── examples/
│   ├── quote.rs            # 行情订阅示例
│   ├── history.rs          # 历史数据示例
│   ├── trade.rs            # 交易示例
│   ├── datamanager.rs      # DataManager 示例
│   ├── auth_demo.rs        # 认证示例
│   ├── auth_switch.rs      # 切换账号示例
│   └── client_builder.rs   # ClientBuilder 示例
└── README.md
```

## 核心设计

### DIFF 协议实现

完整实现天勤 DIFF 协议，这是本项目的核心技术亮点：

- **递归合并** - 支持嵌套对象的增量更新，高效处理复杂数据结构
- **ViewWidth 限制** - 使用二分查找优化大数据集，避免内存溢出
- **Binding 对齐** - 多合约 K线时间对齐，支持跨品种分析
- **版本追踪** - Epoch 机制追踪每次数据变化，精确判断更新
- **路径监听** - Watch/UnWatch 精确监听指定路径的数据变化
- **NaN 处理** - 正确处理 NaN 和特殊值

### 类型安全

Rust 的类型系统带来的优势：

- **90+ 字段的强类型定义** - Quote、Kline、Tick、Account 等完整定义
- **编译时类型检查** - 消除大量运行时错误
- **泛型和 trait 抽象** - Authenticator trait 支持自定义认证
- **Result 类型统一错误处理** - 强制错误处理，避免遗漏

### 并发安全

多线程环境下的安全保证：

- **Arc + RwLock** - 保证线程安全的共享数据访问
- **async-channel** - 异步通信，支持多生产者多消费者
- **AtomicI64** - 原子操作优化性能（Epoch 版本号）
- **无数据竞争** - 编译时保证，无需运行时检查

### 灵活接口

支持三种数据订阅方式，满足不同场景需求：

1. **Channel** - 使用 async-channel，支持多个订阅者，适合多任务处理
2. **Callback** - 注册回调函数，异步触发，适合事件驱动（**零拷贝优化**）
3. **Stream** - 使用 async-stream，支持流式处理，适合函数式编程

### 零拷贝回调设计

所有回调函数使用 `Arc<T>` 参数，避免数据深拷贝：

```rust
// Quote 回调：Arc<Quote>
quote_sub.on_quote(|quote| {
    // quote 是 Arc<Quote>，多个回调共享同一份数据
    println!("最新价: {}", quote.last_price);
}).await;

// Series 回调：Arc<SeriesData>, Arc<UpdateInfo>
series_sub.on_update(|data, info| {
    // data 和 info 都是 Arc 包装的，零拷贝！
    if info.has_new_bar {
        println!("新K线");
    }
}).await;
```

**性能优势**:
- 多回调场景下内存节省 50-90%
- 克隆性能提升 500-1000x（只克隆 8 字节指针）
- 线程安全的数据共享

**详细文档**: 查看 [ARC_OPTIMIZATION.md](./tqsdk-rs/ARC_OPTIMIZATION.md) 和 [ARC_QUOTE_OPTIMIZATION.md](./tqsdk-rs/ARC_QUOTE_OPTIMIZATION.md)

### 零成本抽象

Rust 的零成本抽象理念：

- **编译时优化** - 泛型和 trait 在编译时展开，无运行时开销
- **无 GC 压力** - 所有权系统管理内存，无垃圾回收停顿
- **内联优化** - 小函数自动内联，减少函数调用开销

## 最佳实践

### 1. 延迟启动模式（强烈推荐）

避免竞态条件，确保不丢失初始数据：

```rust
// 推荐：先注册回调，再启动
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 100).await?;

// 先注册所有回调
sub.on_update(|data, info| {
    // 处理数据更新
}).await;

sub.on_new_bar(|data| {
    // 处理新 K线
}).await;

// 最后启动订阅
sub.start().await?;

// 不推荐：启动后再注册回调（可能丢失初始数据）
// sub.start().await?;
// sub.on_update(...).await;  // 可能错过初始数据
```

### 2. 错误处理

使用 Result 类型和模式匹配处理错误：

```rust
use tqsdk_rs::{Result, TqError};

async fn my_function() -> Result<()> {
    // 创建客户端
    let mut client = Client::new("user", "pass", ClientConfig::default()).await?;
    
    // 订阅行情（带错误处理）
    match client.subscribe_quote(&["SHFE.au2602"]).await {
        Ok(sub) => {
            println!("订阅成功");
            sub.start().await?;
        }
        Err(TqError::PermissionDenied(msg)) => {
            eprintln!("权限不足: {}", msg);
            return Err(TqError::PermissionDenied(msg));
        }
        Err(TqError::NetworkError(msg)) => {
            eprintln!("网络错误: {}", msg);
            return Err(TqError::NetworkError(msg));
        }
        Err(e) => {
            eprintln!("其他错误: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}
```

### 3. 资源清理

及时释放资源，避免内存泄漏：

```rust
// 使用完毕后关闭资源
quote_sub.close().await?;
series_sub.close().await?;
session.close().await?;
client.close().await?;

// 或使用 RAII 模式（推荐）
{
    let mut client = Client::new("user", "pass", ClientConfig::default()).await?;
    // 使用 client
    // 离开作用域时自动清理
}
```

### 4. 日志配置

合理配置日志级别，便于调试。支持本地时区显示和 Layer 组合：

```rust
use tqsdk_rs::{init_logger, create_logger_layer};

// 方式 1: 快速初始化（简单场景）
init_logger("debug", false);  // 级别: trace, debug, info, warn, error

// 方式 2: 使用 ClientBuilder（推荐）
let client = Client::builder("user", "pass")
    .log_level("debug")      // 开发时使用 debug
    .build()
    .await?;

// 方式 3: Layer 组合（高级场景）
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

let tqsdk_layer = create_logger_layer("debug", false);
// 可以与业务层的其他 Layer 组合
tracing_subscriber::registry()
    .with(tqsdk_layer)
    // .with(your_custom_layer)
    .init();

// 生产环境建议使用 info 或 warn
let client = Client::builder("user", "pass")
    .log_level("info")
    .build()
    .await?;
```

**日志特性**:
- 自动使用本地时区（如 `2024-11-26T10:30:45.123+08:00`）
- 支持 Layer 组合，可与业务日志集成
- 详细的源文件和行号信息

**详细文档**: 查看 [LOGGER_GUIDE.md](./tqsdk-rs/LOGGER_GUIDE.md) 了解完整的日志配置指南。

### 5. 合约代码格式

使用完整的合约代码格式：

```rust
// 正确：使用完整格式
"SHFE.au2602"   // 上期所黄金
"DCE.m2505"     // 大商所豆粕
"CZCE.SR505"    // 郑商所白糖

// 错误：不完整的格式
"au2602"        // 缺少交易所
"SHFE.au"       // 缺少月份
```

### 6. ViewWidth 设置

合理设置 ViewWidth，避免内存浪费：

```rust
// 实时监控：较小的 ViewWidth
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 100).await?;

// 数据分析：较大的 ViewWidth
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 5000).await?;

// 注意：最大 10000，超过会自动调整
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 15000).await?;
// 实际会被调整为 10000
```

## 与 Go 版本对比

| 维度 | Go 版本 | Rust 版本 | 说明 |
|------|---------|-----------|------|
| 代码行数 | ~8,000 | ~4,900 | Rust 更简洁 |
| 类型安全 | 弱类型 | 强类型 | Rust 优势 |
| 内存安全 | GC | 所有权 | Rust 优势 |
| 并发安全 | 运行时检查 | 编译时检查 | Rust 优势 |
| 性能 | 高 | 更高 | Rust 优势 |
| 零拷贝 | 部分支持 | **Arc 优化** | **Rust 优势** |
| 错误处理 | error | Result<T> | Rust 优势 |
| 数据分析 | 需第三方库 | **Polars 集成** | **Rust 优势** |
| 学习曲线 | 低 | 中高 | Go 优势 |
| 开发速度 | 快 | 中 | Go 略优 |

## 注意事项

### 重要提示

1. **合约代码格式** - 必须使用完整的合约代码格式，如 `SHFE.au2602`（交易所.合约代码）
2. **延迟启动模式** - 强烈推荐使用延迟启动模式，先注册回调再调用 `start()`，避免竞态条件
3. **资源释放** - 使用完毕后记得调用 `close()` 释放资源，避免连接泄漏
4. **权限检查** - 订阅前会自动检查权限，确保账号有相应的行情或交易权限
5. **ViewWidth 限制** - 最大值为 10000，超过会自动调整为 10000

### 常见问题

**Q: 为什么收不到数据？**
- 检查是否调用了 `start()` 方法
- 确认回调是在 `start()` 之前注册
- 检查合约代码格式是否正确
- 确认账号有相应权限

**Q: 如何处理网络断开？**
- WebSocket 会自动重连
- TradeSession 支持自动重连
- 重连后会自动恢复订阅

**Q: 多合约订阅如何优化？**
- 使用单个 `subscribe_quote()` 订阅多个合约
- 避免为每个合约创建单独的订阅
- 合理设置 ViewWidth，避免内存浪费

**Q: 如何调试？**
- 设置日志级别为 `debug` 或 `trace`
- 使用 `RUST_LOG` 环境变量：`RUST_LOG=tqsdk_rs=debug cargo run`
- 检查 WebSocket 连接状态
- 日志自动显示本地时区，便于调试

**Q: 如何集成业务层日志？**
- 使用 `create_logger_layer()` 获取 tqsdk-rs 的 Layer
- 与业务层的其他 Layer 组合
- 详见 [LOGGER_GUIDE.md](./tqsdk-rs/LOGGER_GUIDE.md)


### 报告问题

如果发现 Bug 或有功能建议，请提交 [GitHub Issue](https://github.com/pseudocodes/tqsdk-rs/issues)。

## 许可证

本项目采用 Apache License 2.0 许可证 - 详见 [LICENSE](LICENSE) 文件


## 相关项目

- [tqsdk-go](https://github.com/pseudocodes/tqsdk-go) - Go 语言版本
- [tqsdk-python](https://github.com/shinnytech/tqsdk-python) - Python 官方版本

## 免责声明

**重要提示：本项目仅供学习和研究使用。**

本项目明确拒绝对产品做任何明示或暗示的担保。使用本项目进行交易和投资的一切风险由使用者自行承担。期货交易具有高风险，可能导致本金全部损失，请谨慎投资。

作者和贡献者不对使用本软件造成的任何直接或间接损失承担责任。
