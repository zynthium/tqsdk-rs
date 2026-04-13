# 合约与基础数据

当用户问合约查询、主连、期权筛选、结算价、持仓排名、EDB、交易日历或交易状态时，读本文件。

## 当前主路径

这些能力都优先走 `Client` facade：

- `query_graphql()`
- `query_quotes()`
- `query_cont_quotes()`
- `query_options()`
- `query_atm_options()`
- `query_all_level_options()`
- `query_all_level_finance_options()`
- `query_symbol_info()`
- `query_symbol_settlement()`
- `query_symbol_ranking()`
- `query_edb_data()`
- `get_trading_calendar()`
- `get_trading_status()`

不要把 `InsAPI` 当作普通用户主路径。

## 使用前提

先完成：

```rust
let mut client = Client::builder(username, password)
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;

client.init_market().await?;
```

## 常见查询

### 合约 / 主连

```rust
let futures = client
    .query_quotes(Some("FUTURE"), Some("SHFE"), None, Some(false), None)
    .await?;

let cont = client.query_cont_quotes(Some("SHFE"), None, None).await?;
```

### 期权筛选

```rust
let options = client
    .query_options("SSE.510300", Some("CALL"), None, None, None, Some(false), None)
    .await?;

let atm = client
    .query_atm_options("SSE.510300", underlying_price, &[1, 0, -1], "CALL", None, None, None)
    .await?;
```

`examples/option_levels.rs` 是当前最接近的仓库例子。

### 基础数据服务

```rust
let info = client.query_symbol_info(&["SHFE.au2602"]).await?;
let settlement = client.query_symbol_settlement(&["SHFE.au2602"], 5, None).await?;
let ranking = client
    .query_symbol_ranking("SHFE.au2602", "volume", 5, None, None)
    .await?;
let calendar = client.get_trading_calendar(start_date, end_date).await?;
```

### EDB

```rust
let rows = client.query_edb_data(&[100000001], 30, Some("left"), Some("none")).await?;
```

这类接口受账号权限约束。若用户只是在排查权限问题，直接指出“接口存在，但账号可能缺少对应授权”，不要虚构代码层 workaround。

## `get_trading_status()`

`get_trading_status(symbol)` 返回一个 `async_channel::Receiver<TradingStatus>`。

使用时可以：

```rust
let rx = client.get_trading_status("SHFE.au2602").await?;
while let Ok(status) = rx.recv().await {
    println!("{:?}", status);
}
```

内部是按 symbol 聚合订阅意图的；receiver 释放后会自动减少引用计数。

## 当前不要再推荐的旧路径

- 把 `series()` / `ins()` 讲成对外主入口
- 让用户直接操作 `ins/` 内部模块来完成普通查询
- 在普通问答里展开 GraphQL 内部细节，除非用户明确要底层 query
