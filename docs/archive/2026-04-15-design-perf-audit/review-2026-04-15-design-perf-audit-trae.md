# tqsdk-rs 设计/性能审查报告（2026-04-15）

> 审查范围：`src/` 全库（重点：`websocket/`、`datamanager/`、`series/`、`marketdata/`）
> 审查目标：识别设计缺陷、性能热点、并发风险与可维护性问题
> 审查工具：Trae

---

## 1. 结论摘要

整体架构方向正确，特别是：
- 外部 API 收口到 `Client/TradeSession/ReplaySession/TqRuntime`
- WebSocket I/O actor + 有界背压策略
- `DataManager` 作为 DIFF 状态中心
- `SeriesSubscription` 的状态驱动消费模式（`wait_update/snapshot/load`）

本轮未发现阻断级正确性缺陷，但识别出 **3 个高优先级性能/设计问题（P1）**，主要集中在行情热路径上的 JSON clone、重复反序列化和异步更新链路延迟。

---

## 2. 验证结果

已执行并通过：
- `cargo fmt --check`
- `cargo test`（272 passed）
- `cargo clippy --all-targets --all-features -- -D warnings`

说明：
- 当前代码在测试与 lint 层面健康；
- 报告中的问题主要属于运行时性能与可扩展性风险，需要压测/基准进一步量化。

---

## 3. Findings（按严重度）

### P1-1 行情 `rtn_data` 热路径存在大块 JSON 深拷贝

**位置**
- `src/websocket/quote.rs` `handle_rtn_data()`

**现象**
- `runtime.dm.merge_data(payload.clone(), true, true)` 会对整段 `data` payload 做 clone。
- 在高频行情下，这会持续放大 CPU 与内存分配成本。

**影响**
- 增加每条消息处理时延；
- 抬高 allocator 压力，降低峰值吞吐。

**建议**
- 让处理函数消费 owned payload（避免 `payload.clone()`）；
- 需要多处复用时改为更细粒度复制，而不是整包复制。

---

### P1-2 MarketDataState 更新链路存在重复读取/反序列化与任务堆积风险

**位置**
- `src/websocket/quote.rs` `sync_market_state()`

**现象**
- 先从 diff 收集 key，再异步 `spawn`；
- 任务内对每个 symbol/duration 再从 `DataManager` 读取并反序列化成 typed struct。

**影响**
- 高频更新下重复 `Value -> struct` 转换成本高；
- 每批次 spawn 一次，慢消费时可能产生任务堆积；
- `Client::wait_update()` 依赖 `MarketDataState` epoch，可能出现“DM 已更新但可见更新延后”。

**建议**
- 将 market_state 更新收敛为单消费者/有界队列，避免 spawn 风暴；
- 尽量基于 diff 直接更新 typed 状态，减少回查 `DataManager`。

---

### P1-3 WebSocket 收包日志在非 debug 场景也可能付出字符串构造成本

**位置**
- `src/websocket/core.rs` `ws_io_actor_loop()`

**现象**
- 收包分支构造 `String::from_utf8_lossy(frame.payload())` 用于 debug 输出。

**影响**
- 在高频消息下，字符串构造成为隐藏热开销；
- 即使线上日志级别较高（非 debug），若未做条件保护，仍可能先构造再丢弃。

**建议**
- 用 `tracing::enabled!` 或等价方式包裹文本构造；
- 热路径日志优先结构化字段，避免大 payload 字符串化。

---

### P2-1 异步场景中存在较多 `std::sync::{Mutex,RwLock}` 共享状态

**位置**
- 多处（如 `websocket/core.rs`、`websocket/quote.rs`、`datamanager/*`）

**影响**
- 锁竞争上升时会阻塞 Tokio worker；
- 当前持锁普遍较短，但在大 diff 或重连期间风险放大。

**建议**
- 优先替换热点共享状态为 async-friendly 原语，或缩短临界区并减少锁嵌套。

---

### P2-2 `DataManager` merge 路径对 `serde_json::Value` 的 clone 倾向明显

**位置**
- `src/datamanager/merge.rs`（如 `transform_value_by_prototype`）

**影响**
- 大量小对象更新时，clone 成本累积显著。

**建议**
- 热路径减少不必要 clone，优先 move/引用；
- 若需要长期优化，可考虑 typed diff 或对象池策略。

---

### P2-3 `TqApi::drain_updates()` 每轮都做 HashSet 聚合与排序

**位置**
- `src/marketdata/mod.rs` `drain_updates()`

**影响**
- 高频 `wait_update + drain` 使用模式下，会形成持续分配与哈希成本。

**建议**
- 评估是否可增量维护排序结构，或提供“无排序快速路径”。

---

### P3-1 `flush_queue()` 恢复发送时仍有额外 clone 成本

**位置**
- `src/websocket/core.rs` `flush_queue()`

**影响**
- 重连恢复窗口期有额外字符串复制；
- 相比 P1 项影响较小，但属于可顺手优化项。

**建议**
- 改为按 `pop_front` move 发送，避免 `iter().cloned()`。

---

## 4. 优先级建议（可执行）

### 第一批（高收益、低风险）
1. 去掉 `handle_rtn_data()` 的整包 `payload.clone()`。
2. 给收包 debug 文本构造加日志级别守卫。

### 第二批（结构性优化）
3. 重构 `sync_market_state()`：单消费者更新管线 + 减少回查反序列化。

### 第三批（持续治理）
4. 评估 async 锁替换与 `merge` clone 降低策略。
5. 基于 `benches/marketdata_state*.rs` 建立基线与回归门槛（吞吐、p95、分配次数）。

---

## 5. 建议的复核关注点（给 codex）

- 是否同意将 `sync_market_state()` 视为当前最主要结构性瓶颈；
- 是否先做“无行为变更”的小步优化（clone/log guard）再推进管线改造；
- 是否在 CI 引入轻量性能回归检查（至少基准对比阈值告警）。
