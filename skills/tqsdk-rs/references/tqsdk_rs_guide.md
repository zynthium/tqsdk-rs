# TqSdk-RS 参考指南

## 先记住：以 Rust 源码为准

回答用户时优先遵守 Rust 源码的真实公共 API、examples 和公开导出的类型。

最重要的心智是：

- 订阅消费围绕 `QuoteSubscription` / `SeriesSubscription`
- 手工交易入口围绕 `TradeSession`
- 配置入口围绕 `Client::builder + EndpointConfig`
- 回测推进围绕 `BacktestHandle`
- 目标持仓任务入口围绕 `TqRuntime` / `AccountHandle`，但这不是大多数问题的默认起点
- target-pos 回测执行围绕 `BacktestHandle + TqRuntime + BacktestExecutionAdapter`

## 快速定位

### 我应该从哪个入口开始？

| 目标 | 入口 |
|------|------|
| 创建客户端 | `Client::builder` |
| 覆盖服务地址 | `EndpointConfig::from_env` 或 `ClientBuilder::{auth_url, md_url, td_url, ins_url, holiday_url}` |
| 初始化行情 | `init_market` / `init_market_backtest` |
| 实时行情 | `subscribe_quote` |
| K线 / Tick / 历史序列 | `series()` |
| 合约、结算价、排名、交易日历 | `ins()` |
| 创建交易会话 | `create_trade_session` / `create_trade_session_with_options` |
| 创建目标持仓 runtime | `Client::into_runtime`（仅目标持仓相关时展开） / `ClientBuilder::build_runtime`（runtime 外壳） |
| 创建目标持仓任务 | `runtime.account(...)? .target_pos(...)` / `TargetPosTask::new(...)` |
| 创建调仓 scheduler | `runtime.account(...)? .target_pos_scheduler(...)` / `TargetPosScheduler::new(...)` |
| 控制回测推进 | `BacktestHandle` |

## 推荐初始化模板

```rust
use tqsdk_rs::{Client, ClientConfig, EndpointConfig};

let mut client = Client::builder(username, password)
    .config(ClientConfig::default())
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;
```

如果要做行情、K线、Tick、历史、合约查询，后续还需要：

```rust
client.init_market().await?;
```

## 端点与环境变量

公开推荐的端点环境变量只有：

- `TQ_AUTH_URL`
- `TQ_MD_URL`
- `TQ_TD_URL`
- `TQ_INS_URL`
- `TQ_CHINESE_HOLIDAY_URL`

推荐优先级：

```text
显式 builder 参数
> EndpointConfig::from_env()
> 默认值
```

交易地址优先级：

```text
TradeSessionOptions.td_url_override
> ClientBuilder::td_url / EndpointConfig.td_url
> TQ_TD_URL
> 鉴权返回的默认交易地址
```

## 行情与订阅

### Quote

```rust
let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;

quote_sub
    .on_quote(|quote| {
        println!("{} 最新价={}", quote.instrument_id, quote.last_price);
    })
    .await;

quote_sub.start().await?;
```

要点：

- 先 `init_market`
- 先注册回调，再 `start()`
- 可以用 `quote_channel()` 改成 channel 消费
- 可以用 `add_symbols()` / `remove_symbols()` 动态调整

### Series

```rust
use std::time::Duration;

let series_api = client.series()?;
let sub = series_api.kline("SHFE.au2602", Duration::from_secs(60), 300).await?;

sub.on_update(|data, info| {
    if let Some(single) = &data.single {
        if let Some(last) = single.data.last() {
            if info.has_new_bar {
                println!("新K线");
            }
            println!("close={}", last.close);
        }
    }
})
.await;

sub.start().await?;
```

常用入口：

- `kline`
- `tick`
- `kline_history`
- `kline_history_with_focus`

### 多合约对齐 K 线

```rust
use std::time::Duration;

let symbols = vec!["SHFE.au2602".to_string(), "SHFE.ag2602".to_string()];
let sub = client
    .series()?
    .kline(&symbols, Duration::from_secs(60), 100)
    .await?;
```

## 合约与基础数据

先拿到 `InsAPI`：

```rust
let ins = client.ins()?;
```

然后再按需求调用：

- `query_quotes`
- `query_cont_quotes`
- `query_options`
- `query_symbol_info`
- `query_symbol_settlement`
- `query_symbol_ranking`
- `query_edb_data`
- `get_trading_calendar`
- `get_trading_status`

如果用户问“为什么 `ins()` 报错”，优先检查是否已经 `init_market()`。

## 交易会话

```rust
use tqsdk_rs::{TradeSessionOptions, types::*};

let session = client
    .create_trade_session_with_options(
        "simnow",
        "user_id",
        "password",
        TradeSessionOptions {
            td_url_override: None,
        },
    )
    .await?;

session.on_account(|account| {
    println!("权益={} 可用={}", account.balance, account.available);
}).await;

session.on_position(|symbol, position| {
    println!("{} 净持仓={}", symbol, position.volume_long_today);
}).await;

session.connect().await?;

let order_id = session.insert_order(&InsertOrderRequest {
    symbol: "SHFE.au2602".to_string(),
    exchange_id: None,
    instrument_id: None,
    direction: DIRECTION_BUY.to_string(),
    offset: OFFSET_OPEN.to_string(),
    price_type: PRICE_TYPE_LIMIT.to_string(),
    limit_price: 650.0,
    volume: 1,
}).await?;

session.cancel_order(&order_id).await?;
```

关键顺序：

1. 创建会话
2. 注册回调 / channel
3. `connect()`
4. 下单 / 撤单 / 主动查询

## Runtime / TargetPos

这是特定能力分支，不是全局默认入口。

如果用户问的是 Python `TargetPosTask` 对齐、目标净持仓、时间分片调仓，不要继续讲 `TradeSession` 主路径，直接切到：

- `Client::into_runtime`
- `ClientBuilder::build_runtime`（仅在用户明确问这个 API 时解释其当前语义）
- `runtime.account(account_key)?`
- `account.target_pos(symbol)`
- `account.target_pos_scheduler(symbol)`
- `TargetPosTask::new(...)`
- `TargetPosScheduler::new(...)`

## 回测

入口：

- `init_market_backtest`
- `BacktestHandle::next`
- `BacktestHandle::peek`
- `BacktestHandle::current_dt`
- `switch_to_backtest`
- `switch_to_live`

如果用户要“回测 + K线策略”，优先建议：

```text
builder -> build -> init_market_backtest -> series/quote/ins -> BacktestHandle
```

如果用户要“回测 + 目标持仓任务”，要补一句：

```text
BacktestHandle 推进市场时间
+ TqRuntime::with_id(..., RuntimeMode::Backtest, LiveMarketAdapter, BacktestExecutionAdapter)
```

## 常见坑

### 1. `series()` / `ins()` 不可用
通常是没有先 `init_market()` 或 `init_market_backtest()`。

### 2. 回调没有触发
通常是少了 `start()` 或 `connect()`，或者注册顺序反了。

### 3. 用户还在问旧环境变量
优先把答案收束到端点变量，不要继续放大 auth 内部配置。

### 4. 想用交易地址覆盖
优先给 `TradeSessionOptions { td_url_override }` 或 `ClientBuilder::td_url(...)`。

### 5. 想做 DataFrame 分析
提醒用户需要启用 `polars` feature。

### 6. 用户问 Python `TargetPosTask` 在 Rust 里的对应物
直接回答 `TqRuntime` + `TargetPosTask` / `TargetPosScheduler`，不要再说“Rust 暂时没有”。
