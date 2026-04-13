# 行情与序列

当用户问 Quote、K 线、Tick、`wait_update()`、`SeriesSubscription`、历史快照或下载时，读本文件。

## 当前 live 行情主路径

```rust
use std::time::Duration;
use tqsdk_rs::prelude::*;

let mut client = Client::builder(username, password)
    .endpoints(EndpointConfig::from_env())
    .build()
    .await?;
client.init_market().await?;

let symbol = "SHFE.au2602";
let _quote_sub = client.subscribe_quote(&[symbol]).await?;
let quote = client.quote(symbol);

loop {
    quote.wait_update().await?;
    let q = quote.load().await;
    println!("{} {}", q.instrument_id, q.last_price);
}
```

也可以用 `client.wait_update_and_drain()` 做统一 update loop，再按 `updates.quotes / updates.klines / updates.ticks` 分发。

## `QuoteSubscription` 的当前语义

- 创建后立即生效，已经 auto-start
- 只负责服务端订阅生命周期
- 读取行情统一走 `Client::quote()`
- 动态改订阅用 `add_symbols()` / `remove_symbols()`
- 提前释放资源用 `close()`

不要再推荐：

- `start()`
- `on_quote(...)`
- `quote_channel()`

## latest Quote / K 线 / Tick

- Quote：`Client::quote(symbol)` -> `QuoteRef`
- latest K 线：`Client::kline_ref(symbol, duration)` -> `KlineRef`
- latest Tick：`Client::tick_ref(symbol)` -> `TickRef`

这些 ref 适合状态驱动策略循环；通常与 `client.wait_update()` 或 `client.wait_update_and_drain()` 配合使用。

## live 窗口序列

```rust
use std::time::Duration;

let sub = client.kline("SHFE.au2602", Duration::from_secs(60), 256).await?;

let snapshot = sub.wait_update().await?;
if snapshot.update.has_new_bar {
    let data = sub.load().await?;
    let series = data.single.as_ref().expect("single-symbol kline");
    if let Some(last) = series.data.last() {
        println!("last close = {}", last.close);
    }
}
```

关键点：

- `client.kline(...)` / `client.tick(...)` 返回 `Arc<SeriesSubscription>`
- 创建后立即生效，不需要 `start()`
- `wait_update()` 等下一次 coalesced snapshot
- `snapshot()` 读取当前快照（可能是 `None`）
- `load()` 读取当前 `Arc<SeriesData>`；首次快照前可能报错
- `close()` 提前释放窗口订阅

## 多合约对齐 K 线

```rust
let sub = client
    .kline(["SHFE.au2602", "SHFE.ag2512"], Duration::from_secs(60), 256)
    .await?;

let snapshot = sub.wait_update().await?;
let data = sub.load().await?;
let aligned = data.multi.as_ref().expect("multi-symbol kline");
println!("aligned bars = {}", aligned.data.len());
println!("has_new_bar = {}", snapshot.update.has_new_bar);
```

## 一次性历史快照

- K 线：`get_kline_data_series(symbol, duration, start_dt, end_dt)`
- Tick：`get_tick_data_series(symbol, start_dt, end_dt)`

语义都是 `[start_dt, end_dt)`，它们不是持续更新订阅。

最接近的仓库例子是 `examples/history.rs`。

## 长时间历史导出

如果用户要“后台下载”“导出 CSV”“长时间区间异步落盘”，优先讲：

- `spawn_data_downloader()`
- `spawn_data_downloader_with_options()`
- `spawn_data_downloader_to_writer()`

不要把 `get_kline_data_series()` 讲成无限长历史导出器。

## 当前不要再推荐的旧写法

- `SeriesSubscription::start()`
- `SeriesSubscription::on_update()`
- `SeriesSubscription::on_new_bar()`
- `SeriesSubscription::data_stream()`
- 直接从 `SeriesAPI` / `InsAPI` 作为普通用户主路径出发

## 什么时候选哪条路径

- 只想读最新行情：`subscribe_quote()` + `quote()`
- 只想读 latest bar / tick：`kline_ref()` / `tick_ref()`
- 想读窗口态序列：`kline()` / `tick()` + `SeriesSubscription`
- 想拿固定时间范围历史：`get_*_data_series()`
- 想后台导出：`spawn_data_downloader*()`
