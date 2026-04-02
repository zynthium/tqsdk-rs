# 合约与基础数据

当用户问下面这些问题时，优先读本文件：

- `ins()` 是什么
- 合约查询、期权筛选、主连查询怎么做
- 结算价、持仓排名、EDB、交易日历、交易状态怎么查
- 为什么 `ins()` 不能用

## 入口

```rust
client.init_market().await?;
let ins = client.ins()?;
```

先说明：`InsAPI` 依赖行情初始化，没做 `init_market()` 或 `init_market_backtest()` 时不要直接调用。

## 常用查询

### 报价 / 合约

- `query_quotes`
- `query_cont_quotes`
- `query_options`
- `query_symbol_info`

### 基础数据

- `query_symbol_settlement`
- `query_symbol_ranking`
- `query_edb_data`
- `get_trading_calendar`
- `get_trading_status`

## 回答策略

### 用户问“怎么查某个交易所全部合约”

优先给 `query_quotes`。

### 用户问“怎么查主连”

优先给 `query_cont_quotes`。

### 用户问“怎么筛期权”

优先给 `query_options`，并说明筛选维度来自标的、近月、价位层等参数。

### 用户问“怎么查字段含义”

不要一次性解释所有字段，只解释用户当前真正在用的那几个字段。

## 交易日历与交易状态

### 交易日历

```rust
let days = ins.get_trading_calendar(start_dt, end_dt).await?;
```

### 交易状态

```rust
let status = ins.get_trading_status("SHFE.au2602").await?;
```

如果用户问“状态一直拿不到”：

- 先检查是否有权限
- 再检查是否已经完成市场初始化

## 常见坑

### “`ins()` 报未初始化”

最常见原因是没先 `init_market()`。

### “为什么某些接口报权限不足”

不同基础数据接口可能受权限控制，尤其是：

- `query_edb_data`
- 部分交易状态或扩展行情

### “这是不是 GraphQL？”

可以说底层会经过相关查询链路，但回答时优先讲公共 API，不要先把重点放在底层协议实现上。
