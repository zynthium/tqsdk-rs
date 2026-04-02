# 行情与序列

当用户问下面这些问题时，优先读本文件：

- `subscribe_quote` 怎么用
- `QuoteSubscription`、`SeriesAPI`、`SeriesSubscription` 的区别
- K线、Tick、历史数据怎么订阅
- 为什么没收到回调 / 为什么 `series()` 不可用
- 多合约对齐 K 线怎么写

## 总顺序

```text
Client::builder
  -> build
  -> init_market / init_market_backtest
  -> subscribe_quote / series()
  -> 注册回调
  -> start()
```

## Quote

```rust
let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;

quote_sub
    .on_quote(|quote| {
        println!("{} 最新价={}", quote.instrument_id, quote.last_price);
    })
    .await;

quote_sub.start().await?;
```

### Quote 要点

- 先 `init_market()`
- 先 `on_quote(...)`，再 `start()`
- `quote_channel()` 适合 `select!` / channel 消费
- `add_symbols()` / `remove_symbols()` 可动态调整订阅

## Series

### 单合约 K 线

```rust
use std::time::Duration;

let sub = client
    .series()?
    .kline("SHFE.au2602", Duration::from_secs(60), 300)
    .await?;
```

### Tick

```rust
let sub = client.series()?.tick("SHFE.au2602", 200).await?;
```

### 历史 K 线

```rust
let sub = client
    .series()?
    .kline_history("SHFE.au2602", Duration::from_secs(60), 8000, left_kline_id)
    .await?;
```

### 带 focus 的历史 K 线

```rust
let sub = client
    .series()?
    .kline_history_with_focus(symbol, duration, data_length, focus_datetime, focus_position)
    .await?;
```

### 多合约对齐 K 线

```rust
use std::time::Duration;

let symbols = vec!["SHFE.au2602".to_string(), "SHFE.ag2602".to_string()];
let sub = client.series()?.kline(&symbols, Duration::from_secs(60), 100).await?;
```

## Series 回调

常见入口：

- `on_update`
- `on_new_bar`
- `on_bar_update`
- `data_stream`

推荐顺序：

```text
创建订阅
-> 注册回调 / stream 消费
-> start()
```

## 常见坑

### “`series()` 报未初始化”

通常是还没调 `init_market()` 或 `init_market_backtest()`。

### “K线没有回调”

先检查：

1. 是否已经 `start()`
2. 是否在 `start()` 前注册了回调
3. 是否合约和周期参数写对

### “想拿很多历史数据”

优先说明：

- 历史序列接口有自己的定位方式
- 不要把实时订阅接口讲成无限历史下载器

### “Quote 和 Series 该选哪个？”

- 只关心最新行情快照：`subscribe_quote`
- 关心 K线、Tick、历史定位、对齐数据：`series()`
