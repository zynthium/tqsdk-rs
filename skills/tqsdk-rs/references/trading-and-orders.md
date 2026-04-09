# 交易与下单

当用户问下面这些问题时，优先读本文件：

- `TradeSession` 怎么创建和连接
- 如何下单、撤单、查账户、查持仓、查委托、查成交
- `TradeSessionOptions` / `TQ_TD_URL` 怎么覆盖交易地址
- 为什么交易回调没有触发

如果用户问的是“目标净持仓”“Python `TargetPosTask` 对齐”“分时调仓 scheduler”，不要停在本文件，改读 `runtime-and-target-pos.md`。

## 交易链路

```text
Client::builder
  -> build
  -> create_trade_session / create_trade_session_with_options
  -> 注册回调 / channel
  -> connect
  -> insert_order / cancel_order / get_*
```

这条链路讲的是手工交易，不是 target-pos 任务运行时。

## 创建交易会话

### 默认地址

```rust
let session = client
    .create_trade_session("simnow", user_id, password)
    .await?;
```

### 覆盖交易地址

```rust
use tqsdk_rs::TradeSessionOptions;

let session = client
    .create_trade_session_with_options(
        "simnow",
        user_id,
        password,
        TradeSessionOptions {
            td_url_override: Some("wss://example.com/trade".to_string()),
        },
    )
    .await?;
```

## 地址优先级

```text
TradeSessionOptions.td_url_override
> ClientBuilder::td_url / EndpointConfig.td_url
> TQ_TD_URL
> 鉴权返回的默认交易地址
```

## 回调与 channel

常见回调：

- `on_account`
- `on_position`
- `on_order`
- `on_trade`
- `on_notification`
- `on_error`

常见 channel：

- `account_channel`
- `position_channel`
- `order_channel`
- `trade_channel`
- `notification_channel`

## 顺序非常重要

推荐顺序：

1. 创建 `TradeSession`
2. 注册回调或取 channel
3. `connect()`
4. 执行交易与主动查询

## 下单模板

```rust
use tqsdk_rs::types::{InsertOrderRequest, DIRECTION_BUY, OFFSET_OPEN, PRICE_TYPE_LIMIT};

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
```

撤单：

```rust
session.cancel_order(&order_id).await?;
```

## 主动查询

- `get_account`
- `get_position`
- `get_positions`
- `get_orders`
- `get_trades`

## 常见坑

### “没有任何交易回调”

优先检查：

1. 是否已经 `connect()`
2. 是否在 `connect()` 之前就注册好了回调
3. broker / user / password 是否正确

### “怎么改交易地址”

优先给 `TradeSessionOptions`，其次讲 `ClientBuilder::td_url(...)`。

### “交易是不是依赖 `init_market()`”

不要混答。`TradeSession` 和行情初始化是相关但独立的两条链路。

### “我想用目标持仓，不想手动算开平今昨”

不要继续堆 `TradeSession::insert_order` 示例。直接切到 `TqRuntime` + `TargetPosTask` / `TargetPosScheduler`。
