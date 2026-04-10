# 迁移指南：从 Channel/Callback 到高性能状态驱动 API

本指南说明了在 `tqsdk-rs` 中从旧的 Channel 和 Callback 行情订阅模型迁移到全新“高性能状态驱动 API”（TqApi + QuoteRef/KlineRef/TickRef）的原因、概念和具体步骤。

## 为什么需要迁移？

在旧版本中，`tqsdk-rs` 采用类似 Node.js/Go 的强异步消息流模型：每次行情更新都会触发一次回调（Callback）或推入一个消息通道（Channel）。虽然这符合典型的 Rust 异步范式，但在复杂的**全品种/跨合约量化策略**中带来了极大的心智负担：
1. **跨流同步噩梦**：策略通常需要在一个时间切片内计算所有关注的合约。由于 K 线和 Quote 的通道是分离的，你需要自己手写大量的 `Arc<RwLock<T>>` 和 `tokio::select!` 去拼接这些数据。
2. **高频风暴下的唤醒开销**：在开盘瞬间，上百个合约的 Tick 涌入会导致大量的 Channel 接收和 CPU 唤醒，阻塞策略主线程的计算。
3. **Live/Backtest 语义割裂**：回测时的事件流难以与实时流完美对齐，导致策略在实盘和回测中表现不一。

**新版状态驱动 API 的优势**：
- **类似 Python TqSdk 的极简体验**：提供 `api.wait_update().await` 和 `q.is_changing()`。
- **O(1) 无锁读取**：底层采用 `ArcSwap` 实现了零锁竞争的数据更新。策略端读取最新价格开销为纳秒级。
- **O(changed) 批量唤醒**：内置增量更新集合（UpdateSet），策略只需处理本轮跳动的合约，支持高吞吐全品种扫描。

---

## 核心概念映射

| 概念 | 旧版 Channel/Callback 模型 | 新版 State-driven API |
|---|---|---|
| **获取入口** | `Client` | `client.tqapi()` 返回 `TqApi` |
| **发起订阅** | `client.subscribe_quote(...)`<br>`client.series()?.kline(...)` | 保持不变（仍作为驱动网络下发的入口） |
| **数据读取** | `rx.recv().await`<br>`sub.on_new_bar(...)` | `api.quote("SHFE.cu2605")` 获取 `QuoteRef`<br>通过 `quote_ref.load()` 读取快照 |
| **状态追踪** | 需用户手工维护 `HashMap` 缓存 | `quote_ref.is_changing()` 检测本地是否过期 |
| **等待更新** | `select! { msg = rx1.recv() => {}, msg = rx2.recv() => {} }` | `api.wait_update().await`（全局批量等待）<br>或 `quote_ref.wait_update().await`（单点精准等待） |
| **批量获取** | 无法直接获取本次变更全集 | `api.wait_update_and_drain().await` 直接返回本轮发生变化的 key 集合 |

---

## 迁移步骤

### 1. 初始化 TqApi

获取 `TqApi` 句柄，这是你访问强类型、一致性状态存储的唯一入口。

**旧代码：**
```rust
let mut client = Client::builder("user", "pass").build().await?;
client.init_market().await?;
```

**新代码：**
```rust
let mut client = Client::builder("user", "pass").build().await?;
client.init_market().await?;

// 新增：获取状态驱动 API
let api = client.tqapi();
```

### 2. 行情（Quote）订阅迁移

以前你必须监听一个 channel 或者注册回调；现在你只需要拿到 `QuoteRef` 并在主循环中读取。

**旧代码：**
```rust
let sub = client.subscribe_quote(&["SHFE.cu2605"]).await?;
let rx = sub.quote_channel();

tokio::spawn(async move {
    while let Ok(quote) = rx.recv().await {
        println!("最新价: {}", quote.last_price);
    }
});
sub.start().await?;
```

**新代码：**
```rust
// 1. 发起网络订阅请求
let sub = client.subscribe_quote(&["SHFE.cu2605"]).await?;
sub.start().await?;

// 2. 获取本地句柄
let cu = api.quote("SHFE.cu2605");

// 3. 在单线程主循环中处理（像 Python 一样）
loop {
    api.wait_update().await?; // 阻塞，直到世界发生变化
    
    if cu.is_changing() {     // 判断 cu 是否是本次更新的主角
        let q = cu.load();    // 无锁读取 O(1)
        println!("最新价: {}", q.last_price);
    }
}
```

### 3. 全品种策略的高性能写法

如果你订阅了 500 个合约，挨个判断 `is_changing()` 虽然很快，但还有更优雅的高性能写法：**直接消费本轮的更新集合（UpdateSet）**。

**新代码：**
```rust
let sub = client.subscribe_quote(&symbols).await?;
sub.start().await?;

loop {
    // 阻塞并直接返回这一轮被触碰过的 keys
    let updates = api.wait_update_and_drain().await?;
    
    // updates.quotes 是一个 HashSet<SymbolId>
    for symbol_id in updates.quotes {
        // 直接构造该特定合约的引用并读取
        let q_ref = api.quote(symbol_id.as_str());
        let q = q_ref.load();
        
        println!("{} 最新价: {}", symbol_id, q.last_price);
    }
}
```
*提示：使用 `wait_update_and_drain` 可以将复杂度降到严格的 O(changed_count)，是高频多品种策略的推荐写法。*

### 4. K 线（Kline）与 Tick 订阅迁移

历史和实时 K 线现在共享完全一致的语义接口 `KlineRef`。它内部会自动合并网络分片，你读取的永远是当前视角的“最新一根”或“历史截面”。

**旧代码：**
```rust
let series_api = client.series()?;
let sub = series_api.kline("SHFE.cu2605", Duration::from_secs(60), 1000).await?;

sub.on_new_bar(|data| {
    // 繁琐的事件拆解
}).await;
sub.start().await?;
```

**新代码：**
```rust
let series_api = client.series()?;
let sub = series_api.kline("SHFE.cu2605", Duration::from_secs(60), 1000).await?;
sub.start().await?;

let cu_kline = api.kline("SHFE.cu2605", Duration::from_secs(60));

loop {
    api.wait_update().await?;
    
    if cu_kline.is_changing() {
        let k = cu_kline.load();
        println!("当前 K线 C: {}, V: {}", k.close, k.volume);
    }
}
```

### 5. 跨合约套利计算

由于你不再被局限于闭包（Callback）或单一通道（Channel）中，你可以随时无阻塞地读取任意合约的数据：

```rust
// 假设同时有 cu_ref 和 al_ref
loop {
    api.wait_update().await?;
    
    // 如果铜价动了
    if cu_ref.is_changing() {
        let cu_price = cu_ref.load().last_price;
        // 直接读取铝价（不会阻塞，O(1) 开销）
        let al_price = al_ref.load().last_price; 
        
        let spread = cu_price - al_price;
        if spread > THRESHOLD {
            // 做空价差...
        }
    }
}
```

## 注意事项与常见问题

1. **`wait_update` 不返回任何网络包**：它纯粹是一个调度屏障（Barrier）。底层网络线程已经在它返回前，把所有收到的 JSON 数据合并成了强类型的内存结构体（`Quote`/`Kline`）。
2. **`is_changing()` 会消耗状态**：每次调用 `is_changing()` 如果返回 `true`，内部会推进本地的 `seen_epoch`。在同一个策略循环里，同一个 `QuoteRef` 第二次调用会返回 `false`。
3. **回测模式 (Backtest)**：无需修改任何策略层代码！底层 `TqApi` 会自动识别，当你在回测中调用 `wait_update().await` 时，它会驱动历史数据流快进。
4. **兼容期说明**：旧的 `.on_update()` 和 `quote_channel()` 接口目前仍然保留（数据依然会流经它们），但不再推荐在复杂的交易策略中使用。在未来的主版本更新中，它们可能会被彻底移除或移动到底层 `DataManager` 模块中。
