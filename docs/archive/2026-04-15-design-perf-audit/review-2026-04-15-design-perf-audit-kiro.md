# tqsdk-rs 设计与性能缺陷审查报告

> 审查日期：2026-04-15
> 审查范围：全库（src/ 所有模块）
> 审查工具：Kiro
> 状态：待复核

---

## 一、严重问题（可能导致死锁或数据竞争）

### [S1] `notify_watchers` 同时持有两把锁 — 隐性锁顺序约定，潜在死锁

**位置**：`src/datamanager/watch.rs` — `notify_watchers()`

```rust
pub(crate) fn notify_watchers(&self) {
    let data = self.data.read().unwrap();          // 持有 data 读锁
    let mut watchers = self.watchers.write().unwrap(); // 再持有 watchers 写锁
    // ...
}
```

当前调用链（`merge_data_with_semantics` → drop data 写锁 → `notify_watchers`）不会死锁，因为两把锁不重叠。但这里建立了一个隐性约定：**任何持有 `watchers` 锁的路径都不能再去获取 `data` 锁**。这个约定没有任何注释或文档说明，未来维护者极易以相反顺序获取锁，造成死锁。

**建议**：在 `notify_watchers` 中先在 data 读锁内收集需要发送的 payload（`Vec<(Sender, Value)>`），drop 读锁，再在锁外发送。或者利用已有的 `epoch_tx` watch channel，让订阅者自行读取数据，彻底解耦两把锁。

---

### [S2] `subscribe_tail` 两次加锁之间存在竞争窗口

**位置**：`src/trade_session/events.rs` — `EventJournal::subscribe_tail()`

```rust
fn subscribe_tail(self: &Arc<Self>) -> TradeEventStream {
    let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
    let last_seen_seq = self
        .state.lock().unwrap()          // 第一次加锁，读取尾部 seq
        .events.back().map(|e| e.seq).unwrap_or(0);
    self.state.lock().unwrap().subscribers.insert( // 第二次加锁，插入订阅者
        subscriber_id,
        SubscriberState { last_seen_seq, invalidated_before_seq: None },
    );
    // ...
}
```

两次 `lock()` 之间，如果有新事件 `publish()`，`last_seen_seq` 就会落后于实际尾部，导致订阅者在第一次 `recv()` 时立即收到"间隙"里的事件——订阅者以为自己从尾部开始，实际上漏掉了这段间隙。这不会触发 `Lagged`，但语义上是错误的。

**建议**：合并为一次加锁完成读取 + 插入。

---

### [S3] `std::sync::Mutex` 在 tokio 异步上下文中持锁

**位置**：`src/websocket/core.rs` — `TqWebsocket` 结构体

```rust
last_peek_sent: Arc<std::sync::Mutex<std::time::Instant>>,
io: Arc<std::sync::Mutex<Option<WsIoHandle>>>,
```

`ws_io_actor_loop` 是 tokio 异步任务，`WsActorContext` 持有 `last_peek_sent` 的 Arc。同步 Mutex 在 tokio 多线程调度器下，如果持锁期间发生调度切换，会阻塞整个 worker thread，影响同 thread 上的其他任务。`io` 字段在 `force_reconnect_due_to_backpressure` 中也以同步方式持锁。

**建议**：`last_peek_sent` 改用 `AtomicU64`（存 Unix 纳秒）；`io` 改用 `tokio::sync::Mutex`。

---

## 二、性能问题

### [P1] `merge_data` 持写锁期间执行全量 `apply_python_data_semantics`

**位置**：`src/datamanager/merge.rs` — `merge_data_with_semantics()`

```rust
let mut data = self.data.write().unwrap();
for item in &source_arr {
    self.merge_into(&mut *data, ...);
}
self.apply_python_data_semantics(&mut data, current_epoch); // 全量遍历所有 quotes/positions/orders
drop(data);
```

`apply_python_data_semantics` 会遍历：
- 所有 quotes（计算 `expire_rest_days`）
- 所有 positions（计算 `pos_long/pos_short/pos`）
- 所有 orders（计算 `is_dead/is_online/is_error`）
- 所有 trades → orders（计算 `trade_price`）

这些全量遍历在持有全局写锁期间执行，完全阻塞所有并发读操作。行情高频更新时（每秒数百次 merge），这是最主要的吞吐量瓶颈。

**建议**：将 derive 字段改为惰性求值（读时计算），或者只对本次 merge 涉及的 symbol/user 做增量更新，而不是全量扫描。

---

### [P2] `apply_orders_trade_price` 每次 merge 都全量重算

**位置**：`src/datamanager/merge.rs` — `apply_orders_trade_price()`

```rust
fn apply_orders_trade_price(&self, user_map: &mut Map<String, Value>, epoch: i64) {
    // 遍历所有 trades 构建 order_trade_stats: HashMap<String, (i64, f64)>
    // 再遍历所有 orders 更新 trade_price
}
```

每次任何数据 merge（包括纯行情更新，与交易无关）都会触发这个全量遍历。账户有大量历史成交时，这是 O(trades + orders) 的开销，且完全在写锁内。

**建议**：只在 `trade` 路径下的数据发生变化时才重算，或者改为读时按需计算。

---

### [P3] `get_multi_klines_data` 持读锁期间构建大量中间数据结构

**位置**：`src/datamanager/query.rs` — `get_multi_klines_data()`

```rust
let data_guard = self.data.read().unwrap(); // 持读锁
// 构建 aligned_data_indexes: HashMap<String, HashMap<i64, &Value>>
// 构建 bindings: HashMap<String, HashMap<i64, i64>>
// 遍历所有 K 线 ID 做多合约对齐
// 整个函数结束才 drop data_guard
```

读锁持有时间覆盖了所有中间数据结构的构建和遍历。多合约对齐时（如 5 个合约 × 10000 根 K 线），这个时间可能相当长，期间 merge 写锁会被完全阻塞。

**建议**：先在读锁内提取必要的原始数据（clone 出来），drop 读锁，再在锁外做计算。

---

### [P4] `backpressure::drain_backlog` 存在 TOCTOU 竞争，可能产生多个并发 drain 任务

**位置**：`src/websocket/backpressure.rs` — `drain_backlog()`

```rust
None => {
    self.draining.store(false, Ordering::SeqCst);
    self.overflow_notified.store(false, Ordering::SeqCst);
    let has_more = !self.backlog.lock().unwrap().is_empty(); // TOCTOU 窗口
    if has_more && !self.draining.swap(true, Ordering::SeqCst) {
        continue;
    }
    break;
}
```

`draining.store(false)` 和 `has_more` 检查之间存在窗口：另一个 `enqueue` 调用可能在此间隙入队并发现 `draining == false`，于是 spawn 新的 drain task；而当前 drain task 也发现 `has_more == true` 并 continue，导致两个 drain task 同时运行。虽然 `Mutex<VecDeque>` 保证不会重复消费同一条消息，但会产生不必要的并发 drain 任务和额外的 tokio::spawn 开销。

**建议**：将 `draining.store(false)` 和 `has_more` 检查合并到同一个 Mutex 锁内原子完成。

---

### [P5] `merge_into` 中 `property.clone()` 频率过高

**位置**：`src/datamanager/merge.rs` — `merge_into()`

```rust
target.mt_insert(property.clone(), Value::Null);
target.mt_insert(property.clone(), transformed_value);
// 以及 quotes 路径中的 symbol.clone()
```

每个 JSON 属性在 merge 时都会 clone String。在高频行情更新场景下（每秒数百次 merge，每次涉及数十个字段），这会产生大量小字符串堆分配。

**建议**：热路径上的 symbol/key 字符串改用 `Arc<str>` 或 `compact_str`，避免重复堆分配。

---

## 三、设计缺陷

### [D1] `epoch_increase: false` 的 merge 路径不通知 `epoch_tx` 订阅者

**位置**：`src/datamanager/merge.rs` — `merge_data_with_semantics()`

```rust
let should_notify_watchers = if epoch_increase {
    // ... 发布 epoch_tx
    true
} else {
    // ... 修改数据，但不发布 epoch_tx，也不通知 watchers
    false
};
```

`epoch_increase: false` 的 merge 会修改数据，但 `subscribe_epoch()` 的消费者完全看不到这些变化。这个语义差异没有文档说明，调用方可能误以为所有 merge 都会触发通知。

**建议**：在代码和文档中明确说明 `epoch_increase: false` 的适用场景（仅用于初始化/默认值填充），并在 `DataManagerConfig` 或 `merge_data` 的 doc comment 中注明。

---

### [D2] `unwatch` 按路径字符串删除，会删除同路径的所有 watcher

**位置**：`src/datamanager/watch.rs` — `unwatch()`

```rust
pub fn unwatch(&self, path: &[String]) -> Result<()> {
    let path_key = path.join(".");
    let mut watchers = self.watchers.write().unwrap();
    if watchers.remove(&path_key).is_none() { ... } // 删除该路径下的全部 watcher
}
```

如果同一路径有多个订阅者，`unwatch` 会一次性删除全部。而精确取消单个 watcher 的 `unwatch_by_id` 是 `pub(crate)` 的，外部用户无法使用。这会导致意外的订阅取消。

**建议**：让 `watch()` 返回一个 RAII guard（实现 `Drop` 自动调用 `unwatch_by_id`），或者将 `unwatch_by_id` 提升为公开 API。

---

### [D3] 全局重连定时器 `RECONNECT_TIMER` 跨所有 WebSocket 实例共享

**位置**：`src/websocket/reconnect.rs`

```rust
static RECONNECT_TIMER: OnceLock<std::sync::Mutex<SharedReconnectTimer>> = OnceLock::new();
```

同一进程内的所有 WebSocket 连接（行情、交易）共享同一个重连节奏。如果行情连接频繁断线，会拖慢交易连接的重连速度，反之亦然。这是一个隐性的跨实例耦合，在多账户或行情+交易并发场景下会产生意外的延迟。

**建议**：将重连定时器改为实例级别，或者至少按连接类型（行情/交易）分开维护。

---

### [D4] `TqError` 缺少 `#[non_exhaustive]`

**位置**：`src/errors.rs`

```rust
pub enum TqError {
    PermissionDenied(String),
    WebSocketError(String),
    // ...
    Other(String),
}
```

作为公开 crate 的错误类型，没有 `#[non_exhaustive]` 意味着下游用户的 `match` 语句在新增变体时会编译失败，造成不必要的破坏性变更。

**建议**：添加 `#[non_exhaustive]`。

---

### [D5] `TradeEventHub::seen_trade_ids` 无上限增长

**位置**：`src/trade_session/events.rs` — `TradeEventHub`

```rust
struct PublishState {
    last_emitted_orders: HashMap<String, Order>,
    seen_trade_ids: HashSet<String>,  // 只增不减，永不清理
}
```

`seen_trade_ids` 用于成交去重，但永远不会清理。长期运行的交易会话（尤其是模拟盘压测）会导致这个集合无限增长，最终造成内存压力。

**建议**：在 `close()` 时清理；或者设置上限后改用 LRU 去重；或者依赖服务端保证成交 ID 唯一性，直接移除客户端去重。

---

## 四、汇总矩阵

| ID  | 类别     | 位置                                      | 描述                                          | 严重度 |
|-----|----------|-------------------------------------------|-----------------------------------------------|--------|
| S1  | 潜在死锁 | `datamanager/watch.rs`                    | `notify_watchers` 双锁顺序无文档约定          | 高     |
| S2  | 数据竞争 | `trade_session/events.rs`                 | `subscribe_tail` 两次加锁竞争窗口             | 高     |
| S3  | 异步阻塞 | `websocket/core.rs`                       | 同步 Mutex 在 tokio worker 中持锁             | 中     |
| P1  | 性能     | `datamanager/merge.rs`                    | 写锁内全量 `apply_python_data_semantics`      | 高     |
| P2  | 性能     | `datamanager/merge.rs`                    | `apply_orders_trade_price` 每次 merge 全量重算 | 中     |
| P3  | 性能     | `datamanager/query.rs`                    | `get_multi_klines_data` 持读锁做大量计算      | 中     |
| P4  | 并发     | `websocket/backpressure.rs`               | `drain_backlog` TOCTOU 竞争                   | 低     |
| P5  | 性能     | `datamanager/merge.rs`                    | `property.clone()` 频率过高                   | 低     |
| D1  | 设计     | `datamanager/merge.rs`                    | `epoch_increase: false` 不通知订阅者          | 中     |
| D2  | 设计     | `datamanager/watch.rs`                    | `unwatch` 删除同路径所有 watcher              | 中     |
| D3  | 设计     | `websocket/reconnect.rs`                  | 全局重连定时器跨实例耦合                      | 中     |
| D4  | 设计     | `errors.rs`                               | `TqError` 缺 `#[non_exhaustive]`              | 低     |
| D5  | 内存     | `trade_session/events.rs`                 | `seen_trade_ids` 无上限增长                   | 低     |

---

## 五、优先处理建议

1. **P1 + S1**：写锁内全量 derive 计算 + 双锁顺序，这两个在生产高频行情负载下最容易暴露，建议优先处理。
2. **S2**：`subscribe_tail` 竞争窗口，在高并发订阅场景下会产生语义错误，修复成本低（合并为一次加锁）。
3. **D3**：全局重连定时器，在多账户场景下会产生意外延迟，影响交易可靠性。
4. 其余问题可按迭代节奏处理。
