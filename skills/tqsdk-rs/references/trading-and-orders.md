# 交易与下单

当用户问 `TradeSession`、下单撤单、账户 / 持仓 / 委托 / 成交读取、可靠事件流或交易地址覆盖时，读本文件。

## 当前推荐路径

```rust
use tqsdk_rs::prelude::*;

let client = Client::builder(username, password)
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;

let session = client
    .create_trade_session("simnow", user_id, trade_password)
    .await?;
```

如果要覆盖交易地址或 retention，走 `create_trade_session_with_options(...)`。

## `TradeSession` 的当前心智模型

分两层：

1. snapshot / latest state
   - `wait_update()`
   - `get_account()`
   - `get_position()` / `get_positions()`
   - `get_order()` / `get_orders()`
   - `get_trade()` / `get_trades()`
2. reliable event stream
   - `subscribe_events()`
   - `subscribe_order_events()`
   - `subscribe_trade_events()`
   - `wait_order_update_reliable(order_id)`

把账户 / 持仓讲成最新状态读取，不要讲成 callback 或 best-effort channel fan-out。

## 正确连接顺序

先建立消费路径，再 `connect()`：

```rust
let mut events = session.subscribe_events();

tokio::spawn(async move {
    while let Ok(event) = events.recv().await {
        println!("{:?}", event.kind);
    }
});

session.connect().await?;
session.wait_ready().await?;
```

`wait_ready()` 是进入 ready state 的 canonical gate。`is_ready()` 只用于瞬时状态探针，不推荐当主流程 busy-poll。

如果用户问“为什么连上了但还没快照 / 没事件”，先检查是不是还没 `connect()` 或还没完成 `wait_ready()`。

## snapshot 读取

完成 `connect()` + `wait_ready()` 后，再进入 snapshot 读取。

```rust
session.wait_update().await?;

let account = session.get_account().await?;
let positions = session.get_positions().await?;
let orders = session.get_orders().await?;
let trades = session.get_trades().await?;
```

推荐读取路径：

- ready 后 `wait_update()` + getter（等待下一次状态推进）
- ready 后直接 getter（读取当前最新快照）

不要暗示“未 ready 前就可以安全进入 snapshot wait”。

## 可靠事件流

```rust
let mut orders = session.subscribe_order_events();
let mut trades = session.subscribe_trade_events();
```

- `subscribe_order_events()` 只给订单事件
- `subscribe_trade_events()` 只给成交事件
- `subscribe_events()` 给完整可靠事件视图，包括订单、成交、通知、异步错误

如果用户遇到 `Lagged`，说明消费端太慢或 retention 太小；优先建议更快消费或调大 `TradeSessionOptions.reliable_events_max_retained`。

## 下单 / 撤单

```rust
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

session.wait_order_update_reliable(&order_id).await?;
session.cancel_order(&order_id).await?;
```

扩展成交时效 / 数量条件时，用 `insert_order_with_options(...)`。

## `TradeSessionOptions`

当前普通用户最常问的是：

- `td_url_override`
- `reliable_events_max_retained`
- `use_sm`

优先级见 `client-and-endpoints.md`。

## `TradeLoginOptions`

只有当用户明确问这些交易登录报文字段时再展开：

- `confirm_settlement`
- `client_mac_address`
- `client_system_info`
- `client_app_id`
- `front`

默认问答不要把这些细节堆给普通用户。

## 避免的非 canonical surface

- 把账户 / 持仓讲成 callback / channel fan-out
- 把订单 / 成交讲成 best-effort channel 模型
- 把 `TargetPosTask` 说成 `TradeSession` 的方法
