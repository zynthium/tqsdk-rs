# tqsdk-rs 设计与性能审查报告

**审查日期**: 2026-04-15
**审查范围**: 全部 12 个核心模块，约 15,000+ 行 Rust 源码
**审查工具**: Claude Opus 4.6 静态分析 + 代码走查

---

## 一、CRITICAL 级问题

### 1. DataManager merge → notify 锁序不一致（潜在死锁）

**文件**: `src/datamanager/merge.rs:87-104` + `src/datamanager/watch.rs:63-66`

`merge_data` 先获取 `data.write()`，释放后调用 `notify_watchers()`。而 `notify_watchers()` 内部先获取 `data.read()`，再获取 `watchers.write()`。同时 `unwatch()` 直接获取 `watchers.write()`。

**死锁场景**：
1. 线程 A：`merge_data()` 释放 data 锁后，进入 `notify_watchers()` 准备获取 `data.read()`
2. 线程 B：`unwatch()` 持有 `watchers.write()`，如果同时有其他路径试图获取 `data` 锁
3. 虽然当前代码中 `unwatch` 不读 `data`，但锁序未被文档化或强制保证，未来修改极易引入真正的死锁

**实际风险**：当前代码路径下不会死锁（因为 `unwatch` 只操作 watchers），但这是一个脆弱的隐含不变量。

**建议**：统一锁序为 `data → watchers`，或在 `notify_watchers` 中先 clone 需要通知的数据再释放锁。

---

### 2. OpenLimit 预算竞态（runtime 模块）

**文件**: `src/runtime/tasks/target_pos.rs:331-340`

```rust
let open_limit_budget = if quote.open_limit > 0 {
    let used_volume = used_volume_for_trading_day(&quote, &trades)?;
    Some(OpenLimitBudget { ... remaining_limit: (quote.open_limit - used_volume).max(0) })
} else { None };
```

多个 `TargetPosTask` 可以并发读取相同的 `trades` 列表并计算相同的 `remaining_limit`，然后各自下单。总下单量可能超出交易所限额。

**建议**：在 `TaskRegistry` 或 `AccountHandle` 层增加 per-(account, symbol, trading_day) 的原子预算跟踪。

---

### 3. 全局共享重连定时器

**文件**: `src/websocket/reconnect.rs:8-12`

```rust
static RECONNECT_TIMER: OnceLock<std::sync::Mutex<SharedReconnectTimer>> = OnceLock::new();
```

**所有** WebSocket 实例（行情、交易、多账户）共享同一个重连定时器。如果行情 WS 刚重连完毕设置了较长的 `next_reconnect_at`，交易 WS 断线时被迫等待这个无关的延迟。

**建议**：改为 per-connection 或 per-connection-type 的重连定时器。

---

## 二、HIGH 级问题

### 4. 全局 59 处 `.lock().unwrap()` / `.lock().expect()`

**统计**: 59 处分布在 13 个文件中，其中 `runtime/registry.rs`(16处)、`websocket/core.rs`(10处) 最密集。

任何一次 panic 发生在锁持有期间，该 Mutex 就会中毒，后续所有操作永久崩溃。对于交易 SDK 这是不可接受的可靠性风险。

**建议**：关键路径改为 `lock().map_err(|_| TqError::InternalError(...))` 返回 Result。

---

### 5. 递归 merge 无深度限制

**文件**: `src/datamanager/merge.rs:151-242`

`merge_into` 递归处理嵌套 JSON，无最大深度检查。恶意或异常的深层 JSON 数据（>1000 层）可导致栈溢出。

**建议**：增加 `max_depth` 参数，超限返回错误。

---

### 6. 历史下载全量内存化

**文件**: `src/series/api.rs` 中的 K 线/Tick 下载循环

所有历史数据下载后全量 collect 到 `Vec`，再通过 `BTreeMap` 去重，最后转回 `Vec`。对于大时间范围（如 1 年分钟线 ≈ 120K 条），峰值内存可达数百 MB。

**建议**：实现流式写入或分批处理，避免全量 materialization。

---

### 7. 缓存锁持有期间做网络 I/O

**文件**: `src/series/api.rs` + `src/cache/data_series.rs`

`lock_series()` 获取排他锁后，在锁内执行网络下载（`download_kline_range`）。其他需要同一合约数据的请求全部阻塞。

**建议**：先下载到临时缓冲，再获取锁写入。

---

### 8. TargetPosTask 死任务检测缺失

**文件**: `src/runtime/tasks/target_pos.rs:230-244`

`ensure_started` 检查 `started.is_some()` 就返回 OK。但如果 spawned task 已 panic 退出，handle 依然存在，后续 `set_target_volume` / `cancel` 会永远阻塞在 watch channel 上。

**建议**：检查 `handle.is_finished()`，如果已完成则清理并重建。

---

### 9. Token 过期无运行时监控

**文件**: `src/auth/token.rs`

Token claims 校验只在 `login()` 时执行一次。长时间运行的回测或策略（数小时）期间 token 过期后，后续行情/合约请求会得到莫名的认证错误，没有提前告警或自动刷新。

**建议**：增加后台 token 刷新任务或至少在关键操作前校验过期时间。

---

### 10. Replay 步进中的 Clone 风暴

**文件**: `src/replay/` — 95 处 `.clone()` 分布在 7 个文件中

每次 `step()` 调用会 clone 多个 `Kline`、`Tick`、`Vec<ReplayQuote>` 和路径数据。对于长周期回测（25 万步），累积分配量巨大，给 allocator 带来不必要的压力。

**建议**：使用 `Arc` 共享不可变数据，或在 API 层改为接受引用。

---

## 三、MEDIUM 级问题

### 11. SeqCst 过度使用（111 处）

全局 111 处 `Ordering::SeqCst`，绝大多数场景 `Acquire/Release` 即可满足。`SeqCst` 在 x86 上开销不大，但在 ARM 上会产生额外的内存屏障。

---

### 12. `get_by_path()` 深拷贝整个子树

**文件**: `src/datamanager/query.rs`

```rust
pub fn get_by_path(&self, path: &[&str]) -> Option<Value> {
    let data = self.data.read().unwrap();
    get_by_path_ref(&data, path).cloned()   // Clone entire subtree
}
```

对于包含 500+ symbols 的 quotes 对象，每次查询都会 clone 整个 Map。`is_md_reconnect_complete()` 等函数多次调用此方法。

**建议**：对内部使用场景提供 `get_by_path_ref` 返回借用引用，或使用 `Arc<Value>` 减少深拷贝。

---

### 13. `build_numeric_value_index` 每次重建

**文件**: `src/datamanager/query.rs`

多合约 K 线对齐查询中，每次调用都会对每个 symbol 的 data map 重新 parse 所有 key 为 i64 并构建 HashMap。500 symbols × 5000 bars = 250 万次 parse。

**建议**：缓存 numeric index，仅在 epoch 变化时重建。

---

### 14. Watch channel 满时静默保留 watcher

**文件**: `src/datamanager/watch.rs:75-79`

```rust
Err(TrySendError::Full(_)) => true,   // Keep watcher even if queue is full
```

如果消费者持续慢速处理，watcher 永远不会被清理，每次 merge 都会尝试向已满的 channel 发送（并失败），浪费 CPU。

**建议**：增加连续 Full 次数计数，超过阈值后移除并记录告警。

---

### 15. `apply_python_data_semantics` 在每次 merge 时全量执行

**文件**: `src/datamanager/merge.rs:307-424`

每次 `merge_data` 都会遍历所有 quotes 计算 `expire_rest_days`，遍历所有 trades 重算 `trade_price`，遍历所有 positions 重算衍生字段。即使本次 merge 只更新了一个 quote 的 `last_price`。

**建议**：只对本次 epoch 中实际变化的路径执行衍生计算。

---

### 16. TradeSession 状态由 3 个独立 AtomicBool 表示

**文件**: `src/trade_session/core.rs:180-183`

`running`、`logged_in`、`snapshot_ready` 是三个独立的原子变量。在 `connect()` 设置 `running=true` 和 `send_login()` 之间存在窗口期，此时 `is_ready()` 可能返回不一致的状态。

**建议**：改为单一状态枚举 `enum SessionState { Disconnected, Connecting, LoggedIn, Ready }`。

---

### 17. 大型类型的 Clone derive

**类型统计**：
- `Quote`: 46 字段，每次 load 都 clone
- `Position`: 43 字段
- `MultiKlineSeriesData`: 含 `Vec<AlignedKlineSet>` + `HashMap`

在回测热路径中频繁 clone 这些大型结构会显著影响性能。

**建议**：热路径改用 `Arc<T>` 共享；`SeriesSnapshot` 已正确使用此模式，应推广。

---

### 18. Polars 缓冲区 push 时不必要的 String clone

**文件**: `src/polars_ext/tabular.rs`

`RankingBuffer::push()` 中 5 个 String 字段逐一 clone。`SettlementBuffer::push()` 中 2 个 String 字段 clone。

**建议**：改为 `push(row: SymbolRanking)` 接受 ownership，移动而非 clone。

---

## 四、LOW 级问题

| # | 问题 | 文件 |
|---|------|------|
| 19 | `Utc::now()` 在每次 merge 中调用，即使无 expire_datetime | `merge.rs:313` |
| 20 | 缓存 `write_kline_segment` 不要求调用者持有锁 | `cache/data_series.rs` |
| 21 | reqwest::Client 在复权下载时每次重新创建 | `series/api.rs` |
| 22 | SimBroker 无滑点/部分成交模型 | `replay/sim.rs` |
| 23 | BTreeMap 去重后再转 Vec，可改为 in-place dedup | `series/api.rs` |
| 24 | `LogLevel` 未 derive `Copy` | `logger.rs` |

---

## 五、设计亮点（值得保持）

1. **I/O Actor 模式**（websocket）：读写通过单所有者 actor 隔离，正确避免了跨 `await` 持锁。
2. **SeriesSnapshot 的 Arc 共享**：多消费者场景下高效共享，是正确的零拷贝模式。
3. **MergeTarget trait 抽象**：统一了 `HashMap` 和 `Map` 的合并操作，代码复用良好。
4. **有界缓冲 + 丢弃策略**：在行情通道上的设计意图正确——慢消费者丢弃旧数据而非无限堆积。
5. **TaskRegistry 冲突检测**：防止同一 (account, symbol) 上的多个 TargetPosTask 冲突。
6. **交易状态订阅引用计数**（ins 模块）：receiver 释放后自动回收订阅意图。

---

## 六、优先修复建议

| 优先级 | 问题编号 | 描述 | 预期工作量 |
|--------|----------|------|-----------|
| P0 | #2 | OpenLimit 预算竞态 | 1-2 天 |
| P0 | #3 | 全局重连定时器 | 半天 |
| P1 | #4 | lock().unwrap() → Result | 2-3 天（渐进式） |
| P1 | #5 | 递归 merge 深度限制 | 半天 |
| P1 | #8 | TargetPosTask 死任务检测 | 半天 |
| P2 | #6 | 历史下载流式化 | 2 天 |
| P2 | #12 | get_by_path 避免深拷贝 | 1 天 |
| P2 | #15 | apply_python_data_semantics 增量化 | 1 天 |
| P2 | #9 | Token 过期监控 | 1 天 |
| P3 | #10 | Replay clone 优化 | 2 天 |
| P3 | #11 | SeqCst → Acquire/Release | 1 天 |

---

## 七、WebSocket 模块补充发现

### 消息队列静默丢弃

**文件**: `src/websocket/core.rs`

待发送队列满时 `pop_front()` 丢弃最旧消息，仅记录 warn 日志，不通知调用方。对于交易指令这可能导致下单丢失。

### 背压 backlog 4x 乘数

**文件**: `src/websocket/backpressure.rs`

`backlog_max = capacity × 4`，默认 `2048 × 4 = 8192`。在行情高峰期这个缓冲可能不够，导致消息丢弃；但设置更大又浪费内存。

### drain_backlog 竞态

**文件**: `src/websocket/backpressure.rs`

`draining` 标志使用 `AtomicBool::swap`，但在 spawn 任务和实际 drain 之间存在窗口期，可能触发重复 drain。

---

## 八、Quote / Series 模块补充发现

### 重连缓冲完成检测非原子

**文件**: `src/quote/lifecycle.rs`

重连完成检测分三步读取 `dm_temp`、`charts`、`subscribe_quote`，各自使用独立锁。在读取间隙数据可能变化，导致误判重连完成/未完成。

### SeriesSubscription watch 任务 abort 无优雅关闭

**文件**: `src/series/subscription.rs`

`abort_watch_task()` 直接调用 `handle.abort()`，如果 watch 任务正在持有锁或处理中间状态，可能导致状态不一致。

### SeriesSubscription refresh 状态重置非原子

**文件**: `src/series/subscription.rs`

`refresh()` 分多步重置 `last_ids`、`last_left_id`、`last_right_id` 等，每步获取独立锁。并发 refresh 可能导致部分重置。

---

## 九、Trade Session 模块补充发现

### 事件保留窗口默认值偏小

**文件**: `src/trade_session/events.rs`

默认 `reliable_events_max_retained = 512`。高频交易场景下（每秒数百笔订单），消费者稍有延迟就会收到 `Lagged` 错误。

### snapshot_ready 标志去同步

**文件**: `src/trade_session/core.rs` + `watch.rs`

通知线程可能重置 `snapshot_ready = false`，而 watch 循环线程同时检测到 `trade_more_data = false` 并设回 `true`，产生短暂的状态闪烁。

---

## 十、Types 模块补充发现

### 重复的 serde 注解

**文件**: `src/types/market.rs`

Quote 结构 46 个字段中约 40 个带有重复的 `#[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]` 注解。可通过 newtype wrapper（如 `NanF64`）简化。

### Order / Trade 手写 PartialEq

**文件**: `src/types/trading.rs`

Order（19 字段）和 Trade（13 字段）均手写 PartialEq 逐字段比较。如果新增字段忘记更新 PartialEq 实现，会产生微妙的 bug。

### Chart.state 使用 `HashMap<String, Value>`

**文件**: `src/types/market.rs`

动态类型的 state 字段逃逸了类型安全，clone 开销高。

---

*审查完成。等待 Codex 复核后确定修复优先级和执行方案。*
