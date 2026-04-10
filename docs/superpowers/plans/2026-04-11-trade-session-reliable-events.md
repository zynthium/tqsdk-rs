# TradeSession Reliable Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 `TradeSession` 增加进程内可靠的 `order` / `trade` 事件 API，迁移 `TqRuntime` 到这条新路径，并删除旧的 best-effort 交易事件回调与通道接口。

**Architecture:** 保留 `DataManager` / `TradeSession` 的最新状态读取模型，把可靠事件单独放进 `src/trade_session/events.rs` 的 append-only journal + per-subscriber cursor。`runtime::LiveExecutionAdapter` 只用新事件流做可靠唤醒，不再依赖会溢出丢消息的全局 `order_channel` / `trade_channel`。文档、`AGENTS.md`、README 和示例统一收敛到这条 canonical 事件 API。

**Tech Stack:** Rust 2024, `tokio::sync::Notify`, `Arc`, `std::sync::Mutex`, `VecDeque`, existing `TradeSession`, `DataManager`, `TqRuntime`, `cargo test`, `cargo check --examples`, `cargo clippy`.

---

## File Map

- `src/trade_session/events.rs`
  负责可靠事件 journal、订阅者 cursor、`Lagged` / `Closed` 语义、过滤流封装。
- `src/trade_session/mod.rs`
  挂接 `events` 模块、持有 `TradeEventHub`、删除旧 `order` / `trade` 回调和通道字段。
- `src/trade_session/core.rs`
  构造 `TradeEventHub`、暴露 `subscribe_events()` / `subscribe_order_events()` / `subscribe_trade_events()` / `wait_order_update_reliable()`。
- `src/trade_session/watch.rs`
  在 merge 完成后从订单 / 成交最新快照生产可靠事件，不再写入 best-effort 的 `order_tx` / `trade_tx`。
- `src/client/endpoints.rs`
  给 `TradeSessionOptions` 增加 `reliable_events_max_retained`。
- `src/client/facade.rs`
  把 `TradeSessionOptions.reliable_events_max_retained` 传给 `TradeSession::new(...)`。
- `src/types/trading.rs`
  为 `Order` / `Trade` 增加 `PartialEq`，支持完整快照去重。
- `src/runtime/execution.rs`
  用 `wait_order_update_reliable()` 替换对 `order_channel()` / `trade_channel()` 的依赖。
- `src/lib.rs`
  从 crate root 导出新的交易事件类型。
- `src/prelude.rs`
  在 prelude 中导出新的交易事件类型。
- `src/trade_session/tests.rs`
  覆盖事件 hub 与 `TradeSession` 事件生产 / waiter 语义。
- `src/client/tests.rs`
  覆盖 `TradeSessionOptions` 的新字段和 plumbing。
- `README.md`
  把交易事件示例从回调模式改成可靠事件订阅模式。
- `AGENTS.md`
  更新仓库执行准则，明确旧交易事件 API 已删除，公共 API 变更必须同步示例。
- `examples/trade.rs`
  改成新可靠事件 API 的 canonical 示例。

## Task 1: Add Reliable Event Primitives And Session Options

**Files:**
- Create: `src/trade_session/events.rs`
- Modify: `src/trade_session/mod.rs`
- Modify: `src/client/endpoints.rs`
- Modify: `src/types/trading.rs`
- Modify: `src/lib.rs`
- Modify: `src/prelude.rs`
- Test: `src/trade_session/events.rs`

- [ ] **Step 1: Write the failing event-hub tests in `src/trade_session/events.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Order, Trade};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::time::{Duration, timeout};

    fn order(order_id: &str, status: &str) -> Order {
        serde_json::from_value(json!({
            "order_id": order_id,
            "status": status,
            "direction": "BUY",
            "offset": "OPEN",
            "volume_orign": 1,
            "volume_left": if status == "FINISHED" { 0 } else { 1 },
            "price_type": "LIMIT",
            "limit_price": 520.0
        }))
        .unwrap()
    }

    fn trade(order_id: &str, trade_id: &str) -> Trade {
        serde_json::from_value(json!({
            "order_id": order_id,
            "trade_id": trade_id,
            "direction": "BUY",
            "offset": "OPEN",
            "volume": 1,
            "price": 520.0
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn trade_event_hub_subscriber_starts_at_tail() {
        let hub = Arc::new(TradeEventHub::new(8));
        hub.publish_order("o0".to_string(), order("o0", "ALIVE"));

        let mut stream = hub.subscribe_tail();

        hub.publish_order("o1".to_string(), order("o1", "ALIVE"));
        let event = timeout(Duration::from_millis(100), stream.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(event.seq, 2);
        assert!(matches!(
            event.kind,
            TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
        ));
    }

    #[tokio::test]
    async fn trade_event_hub_marks_lagged_subscriber_when_retention_is_exceeded() {
        let hub = Arc::new(TradeEventHub::new(2));
        let mut stream = hub.subscribe_tail();

        hub.publish_order("o1".to_string(), order("o1", "ALIVE"));
        hub.publish_trade("t1".to_string(), trade("o1", "t1"));
        hub.publish_order("o1".to_string(), order("o1", "FINISHED"));

        let err = stream.recv().await.unwrap_err();
        assert!(matches!(err, TradeEventRecvError::Lagged { .. }));
    }

    #[tokio::test]
    async fn trade_event_hub_close_wakes_waiters() {
        let hub = Arc::new(TradeEventHub::new(8));
        let mut stream = hub.subscribe_tail();

        let waiter = tokio::spawn(async move { stream.recv().await });
        hub.close();

        let result = timeout(Duration::from_millis(100), waiter).await.unwrap().unwrap();
        assert!(matches!(result, Err(TradeEventRecvError::Closed)));
    }
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test trade_event_hub_ -- --nocapture`

Expected: compile failure because `src/trade_session/events.rs`, `TradeEventHub`, `TradeSessionEvent`, and `TradeEventRecvError` do not exist yet.

- [ ] **Step 3: Add the new public event types and the new trade-session option**

```rust
// src/client/endpoints.rs
#[derive(Debug, Clone)]
pub struct TradeSessionOptions {
    pub td_url_override: Option<String>,
    pub reliable_events_max_retained: usize,
}

impl Default for TradeSessionOptions {
    fn default() -> Self {
        Self {
            td_url_override: None,
            reliable_events_max_retained: 8_192,
        }
    }
}
```

```rust
// src/types/trading.rs
-#[derive(Debug, Clone, Serialize, Deserialize)]
+#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// apply this derive change to `Order`

-#[derive(Debug, Clone, Serialize, Deserialize)]
+#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// apply this derive change to `Trade`
```

```rust
// src/trade_session/mod.rs
mod events;

pub use events::{
    OrderEventStream, TradeEventHub, TradeEventRecvError, TradeOnlyEventStream, TradeSessionEvent,
    TradeSessionEventKind, TradeEventStream,
};
```

```rust
// src/lib.rs / src/prelude.rs
pub use trade_session::{
    OrderEventStream, TradeEventRecvError, TradeOnlyEventStream, TradeSession, TradeSessionEvent,
    TradeSessionEventKind, TradeEventStream,
};
```

- [ ] **Step 4: Implement `TradeEventHub`, `TradeEventStream`, filtered stream wrappers, and retention semantics**

```rust
// src/trade_session/events.rs
use crate::types::{Order, Trade};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Debug, Clone)]
pub struct TradeSessionEvent {
    pub seq: u64,
    pub kind: TradeSessionEventKind,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TradeSessionEventKind {
    OrderUpdated { order_id: String, order: Order },
    TradeCreated { trade_id: String, trade: Trade },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeEventRecvError {
    Closed,
    Lagged { missed_before_seq: u64 },
}

struct SubscriberState {
    last_seen_seq: u64,
    invalidated_before_seq: Option<u64>,
}

struct HubState {
    events: VecDeque<Arc<TradeSessionEvent>>,
    subscribers: HashMap<u64, SubscriberState>,
    last_emitted_orders: HashMap<String, Order>,
    seen_trade_ids: HashSet<String>,
}

pub struct TradeEventHub {
    next_seq: AtomicU64,
    next_subscriber_id: AtomicU64,
    max_retained_events: usize,
    closed: AtomicBool,
    notify: Notify,
    state: Mutex<HubState>,
}

impl TradeEventHub {
    pub fn new(max_retained_events: usize) -> Self {
        Self {
            next_seq: AtomicU64::new(0),
            next_subscriber_id: AtomicU64::new(1),
            max_retained_events: max_retained_events.max(1),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
            state: Mutex::new(HubState {
                events: VecDeque::new(),
                subscribers: HashMap::new(),
                last_emitted_orders: HashMap::new(),
                seen_trade_ids: HashSet::new(),
            }),
        }
    }

    pub fn subscribe_tail(self: &Arc<Self>) -> TradeEventStream {
        let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let last_seen_seq = self.next_seq.load(Ordering::SeqCst);
        self.state.lock().unwrap().subscribers.insert(
            subscriber_id,
            SubscriberState {
                last_seen_seq,
                invalidated_before_seq: None,
            },
        );
        TradeEventStream {
            hub: Arc::clone(self),
            subscriber_id,
        }
    }

    pub fn subscribe_orders(self: &Arc<Self>) -> OrderEventStream {
        OrderEventStream {
            inner: self.subscribe_tail(),
        }
    }

    pub fn subscribe_trades(self: &Arc<Self>) -> TradeOnlyEventStream {
        TradeOnlyEventStream {
            inner: self.subscribe_tail(),
        }
    }

    pub fn publish_order(&self, order_id: String, order: Order) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if state.last_emitted_orders.get(&order_id).is_some_and(|prev| prev == &order) {
            return None;
        }
        state.last_emitted_orders.insert(order_id.clone(), order.clone());
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        state.events.push_back(Arc::new(TradeSessionEvent {
            seq,
            kind: TradeSessionEventKind::OrderUpdated { order_id, order },
        }));
        self.trim_locked(&mut state);
        drop(state);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub fn publish_trade(&self, trade_id: String, trade: Trade) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if !state.seen_trade_ids.insert(trade_id.clone()) {
            return None;
        }
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        state.events.push_back(Arc::new(TradeSessionEvent {
            seq,
            kind: TradeSessionEventKind::TradeCreated { trade_id, trade },
        }));
        self.trim_locked(&mut state);
        drop(state);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn max_retained_events_for_test(&self) -> usize {
        self.max_retained_events
    }
}

pub struct TradeEventStream {
    hub: Arc<TradeEventHub>,
    subscriber_id: u64,
}

impl TradeEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError> {
        loop {
            let notified = self.hub.notify.notified();
            {
                let mut state = self.hub.state.lock().unwrap();
                let Some(subscriber) = state.subscribers.get_mut(&self.subscriber_id) else {
                    return Err(TradeEventRecvError::Closed);
                };
                if let Some(missed_before_seq) = subscriber.invalidated_before_seq.take() {
                    return Err(TradeEventRecvError::Lagged { missed_before_seq });
                }
                if let Some(event) = state
                    .events
                    .iter()
                    .find(|event| event.seq > subscriber.last_seen_seq)
                    .cloned()
                {
                    subscriber.last_seen_seq = event.seq;
                    return Ok((*event).clone());
                }
                if self.hub.closed.load(Ordering::SeqCst) {
                    return Err(TradeEventRecvError::Closed);
                }
            }
            notified.await;
        }
    }
}

impl Drop for TradeEventStream {
    fn drop(&mut self) {
        if let Ok(mut state) = self.hub.state.lock() {
            state.subscribers.remove(&self.subscriber_id);
        }
    }
}
```

- [ ] **Step 5: Run the event-hub tests again**

Run: `cargo test trade_event_hub_ -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/trade_session/events.rs src/trade_session/mod.rs src/client/endpoints.rs src/types/trading.rs src/lib.rs src/prelude.rs
git commit -m "feat: add trade session reliable event primitives"
```

## Task 2: Wire The Event Hub Into TradeSession And Produce Reliable Events

**Files:**
- Modify: `src/trade_session/mod.rs`
- Modify: `src/trade_session/core.rs`
- Modify: `src/trade_session/watch.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/tests.rs`
- Modify: `src/trade_session/tests.rs`

- [ ] **Step 1: Write the failing `TradeSession` event-production tests**

```rust
#[tokio::test]
async fn trade_session_emits_order_then_trade_events() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        Arc::clone(&dm),
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    );

    let mut stream = session.subscribe_events();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "orders": {
                        "o1": {
                            "order_id": "o1",
                            "status": "ALIVE",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume_orign": 1,
                            "volume_left": 1,
                            "price_type": "LIMIT",
                            "limit_price": 520.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "trades": {
                        "t1": {
                            "trade_id": "t1",
                            "order_id": "o1",
                            "direction": "BUY",
                            "offset": "OPEN",
                            "volume": 1,
                            "price": 520.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    let first = stream.recv().await.unwrap();
    let second = stream.recv().await.unwrap();

    assert!(matches!(first.kind, TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"));
    assert!(matches!(second.kind, TradeSessionEventKind::TradeCreated { ref trade_id, .. } if trade_id == "t1"));
}

#[tokio::test]
async fn trade_session_deduplicates_identical_order_snapshots() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        Arc::clone(&dm),
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    );

    let mut stream = session.subscribe_events();

    dm.merge_data(json!({"trade":{"u":{"orders":{"o1":{"order_id":"o1","status":"ALIVE"}}}}}), true, true);
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(json!({"trade":{"u":{"orders":{"o1":{"order_id":"o1","status":"ALIVE"}}}}}), true, true);
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    let _ = stream.recv().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), stream.recv()).await.is_err());
}

#[tokio::test]
async fn trade_session_deduplicates_replayed_trade_ids() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let session = TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        Arc::clone(&dm),
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    );

    let mut stream = session.subscribe_events();

    dm.merge_data(json!({"trade":{"u":{"trades":{"t1":{"trade_id":"t1","order_id":"o1","volume":1,"price":520.0}}}}}), true, true);
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    dm.merge_data(json!({"trade":{"u":{"trades":{"t1":{"trade_id":"t1","order_id":"o1","volume":1,"price":520.0}}}}}), true, true);
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    let _ = stream.recv().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), stream.recv()).await.is_err());
}
```

- [ ] **Step 2: Run the new `TradeSession` tests to verify they fail**

Run: `cargo test trade_session_ -- --nocapture`

Expected: compile failure because `TradeSession::new` does not accept a retention argument, `TradeSession::subscribe_events()` does not exist, and `process_order_update` / `process_trade_update` are not wired to an event hub.

- [ ] **Step 3: Add `TradeEventHub` to `TradeSession` and plumb the new option through the client**

```rust
// src/trade_session/mod.rs
pub struct TradeSession {
    broker: String,
    user_id: String,
    password: String,
    dm: Arc<DataManager>,
    ws: Arc<TqTradeWebsocket>,
    trade_events: Arc<TradeEventHub>,
    account_tx: Sender<Account>,
    account_rx: Receiver<Account>,
    position_tx: Sender<PositionUpdate>,
    position_rx: Receiver<PositionUpdate>,
    order_tx: Sender<Order>,
    order_rx: Receiver<Order>,
    trade_tx: Sender<Trade>,
    trade_rx: Receiver<Trade>,
    notification_tx: Sender<Notification>,
    notification_rx: Receiver<Notification>,
    on_account: AccountCallback,
    on_position: PositionCallback,
    on_order: OrderCallback,
    on_trade: TradeCallback,
    on_notification: NotificationCallback,
    on_error: ErrorCallback,
    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}
```

```rust
// src/trade_session/core.rs
pub fn new(
    broker: String,
    user_id: String,
    password: String,
    dm: Arc<DataManager>,
    ws_url: String,
    ws_config: WebSocketConfig,
    reliable_events_max_retained: usize,
) -> Self {
    let trade_events = Arc::new(TradeEventHub::new(reliable_events_max_retained));
    Self {
        broker,
        user_id,
        password,
        dm,
        ws,
        trade_events,
        account_tx,
        account_rx,
        position_tx,
        position_rx,
        order_tx,
        order_rx,
        trade_tx,
        trade_rx,
        notification_tx,
        notification_rx,
        on_account: Arc::new(RwLock::new(None)),
        on_position: Arc::new(RwLock::new(None)),
        on_order: Arc::new(RwLock::new(None)),
        on_trade: Arc::new(RwLock::new(None)),
        on_notification,
        on_error,
        logged_in: Arc::new(AtomicBool::new(false)),
        running: Arc::new(AtomicBool::new(false)),
        data_cb_id: Arc::new(std::sync::Mutex::new(None)),
    }
}

pub fn subscribe_events(&self) -> TradeEventStream {
    self.trade_events.subscribe_tail()
}

pub fn subscribe_order_events(&self) -> OrderEventStream {
    self.trade_events.subscribe_orders()
}

pub fn subscribe_trade_events(&self) -> TradeOnlyEventStream {
    self.trade_events.subscribe_trades()
}
```

```rust
// src/client/facade.rs
let session = Arc::new(TradeSession::new(
    broker.to_string(),
    user_id.to_string(),
    password.to_string(),
    Arc::clone(&self.dm),
    td_url,
    ws_config,
    options.reliable_events_max_retained,
));
```

```rust
// src/client/tests.rs
#[tokio::test]
async fn create_trade_session_plumbs_reliable_event_retention() {
    let client = build_client_with_market();

    let session = client
        .create_trade_session_with_options(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
            },
        )
        .await
        .unwrap();

    assert_eq!(session.reliable_events_max_retained_for_test(), 32);
}
```

```rust
// src/trade_session/core.rs
#[cfg(test)]
pub(crate) fn reliable_events_max_retained_for_test(&self) -> usize {
    self.trade_events.max_retained_events_for_test()
}
```

- [ ] **Step 4: Emit reliable events from `process_order_update()` and `process_trade_update()`**

```rust
// src/trade_session/watch.rs
pub(crate) async fn process_order_update(
    dm: &Arc<DataManager>,
    user_id: &str,
    trade_events: &Arc<TradeEventHub>,
    last_epoch: i64,
) {
    if let Some(serde_json::Value::Object(orders_map)) = dm.get_by_path(&["trade", user_id, "orders"]) {
        for (order_id, order_data) in &orders_map {
            if order_id.starts_with('_') {
                continue;
            }
            if dm.get_path_epoch(&["trade", user_id, "orders", order_id]) <= last_epoch {
                continue;
            }
            if let Ok(order) = serde_json::from_value::<Order>(order_data.clone()) {
                let _ = trade_events.publish_order(order_id.clone(), order);
            }
        }
    }
}

pub(crate) async fn process_trade_update(
    dm: &Arc<DataManager>,
    user_id: &str,
    trade_events: &Arc<TradeEventHub>,
    last_epoch: i64,
) {
    if let Some(serde_json::Value::Object(trades_map)) = dm.get_by_path(&["trade", user_id, "trades"]) {
        for (trade_id, trade_data) in &trades_map {
            if trade_id.starts_with('_') {
                continue;
            }
            if dm.get_path_epoch(&["trade", user_id, "trades", trade_id]) <= last_epoch {
                continue;
            }
            if let Ok(trade) = serde_json::from_value::<Trade>(trade_data.clone()) {
                let _ = trade_events.publish_trade(trade_id.clone(), trade);
            }
        }
    }
}
```

- [ ] **Step 5: Run the focused `TradeSession` and client plumbing tests**

Run: `cargo test trade_session_ -- --nocapture`
Expected: PASS for the new event-production tests.

Run: `cargo test create_trade_session_plumbs_reliable_event_retention -- --exact`

Expected: PASS for the client plumbing test.

- [ ] **Step 6: Commit**

```bash
git add src/trade_session/mod.rs src/trade_session/core.rs src/trade_session/watch.rs src/client/facade.rs src/client/tests.rs src/trade_session/tests.rs
git commit -m "feat: emit reliable trade session events"
```

## Task 3: Migrate Runtime To Reliable Waiters And Remove Legacy Trade Event APIs

**Files:**
- Modify: `src/trade_session/mod.rs`
- Modify: `src/trade_session/core.rs`
- Modify: `src/trade_session/watch.rs`
- Modify: `src/runtime/execution.rs`
- Modify: `src/trade_session/tests.rs`

- [ ] **Step 1: Write the failing waiter tests**

```rust
#[tokio::test]
async fn trade_session_wait_order_update_reliable_wakes_on_order_event() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let session = Arc::new(TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        Arc::clone(&dm),
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    ));

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_order_update_reliable("o1").await })
    };

    dm.merge_data(
        json!({"trade":{"u":{"orders":{"o1":{"order_id":"o1","status":"ALIVE","direction":"BUY","offset":"OPEN","volume_orign":1,"volume_left":1,"price_type":"LIMIT","limit_price":520.0}}}}}),
        true,
        true,
    );
    TradeSession::process_order_update(&dm, "u", &session.trade_events, -1).await;

    assert!(waiter.await.unwrap().is_ok());
}

#[tokio::test]
async fn trade_session_wait_order_update_reliable_wakes_on_trade_event() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let session = Arc::new(TradeSession::new(
        "simnow".to_string(),
        "u".to_string(),
        "p".to_string(),
        Arc::clone(&dm),
        "wss://example.com".to_string(),
        WebSocketConfig::default(),
        8,
    ));

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_order_update_reliable("o1").await })
    };

    dm.merge_data(
        json!({"trade":{"u":{"trades":{"t1":{"trade_id":"t1","order_id":"o1","direction":"BUY","offset":"OPEN","volume":1,"price":520.0}}}}}),
        true,
        true,
    );
    TradeSession::process_trade_update(&dm, "u", &session.trade_events, -1).await;

    assert!(waiter.await.unwrap().is_ok());
}
```

- [ ] **Step 2: Run the waiter tests to verify they fail**

Run: `cargo test wait_order_update_reliable -- --nocapture`

Expected: compile failure because `wait_order_update_reliable()` does not exist yet.

- [ ] **Step 3: Implement the reliable waiter and switch `LiveExecutionAdapter` to it**

```rust
// src/trade_session/core.rs
pub async fn wait_order_update_reliable(&self, order_id: &str) -> Result<()> {
    let mut stream = self.subscribe_events();
    loop {
        match stream.recv().await {
            Ok(TradeSessionEvent {
                kind: TradeSessionEventKind::OrderUpdated { order_id: updated, .. },
                ..
            }) if updated == order_id => return Ok(()),
            Ok(TradeSessionEvent {
                kind: TradeSessionEventKind::TradeCreated { trade, .. },
                ..
            }) if trade.order_id == order_id => return Ok(()),
            Ok(_) => continue,
            Err(TradeEventRecvError::Closed) => {
                return Err(crate::TqError::InternalError("trade event stream closed".to_string()));
            }
            Err(TradeEventRecvError::Lagged { missed_before_seq }) => {
                return Err(crate::TqError::InternalError(format!(
                    "trade event stream lagged before seq {missed_before_seq}"
                )));
            }
        }
    }
}
```

```rust
// src/runtime/execution.rs
async fn wait_order_update(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
    let session = self.session(account_key).await?;
    session
        .wait_order_update_reliable(order_id)
        .await
        .map_err(RuntimeError::Tq)
}
```

- [ ] **Step 4: Delete the legacy best-effort trade event surface**

```rust
// src/trade_session/mod.rs
// delete:
// - type OrderCallback
// - type TradeCallback
// - order_tx / order_rx
// - trade_tx / trade_rx
// - on_order / on_trade fields
```

```rust
// src/trade_session/core.rs
// delete:
// pub async fn on_order<F>(&self, handler: F)
// pub async fn on_trade<F>(&self, handler: F)
// pub fn order_channel(&self) -> Receiver<Order>
// pub fn trade_channel(&self) -> Receiver<Trade>
```

```rust
// src/trade_session/watch.rs
// delete all order_tx / trade_tx try_send paths and callback invocation paths;
// keep account / position / notification behavior unchanged.
```

- [ ] **Step 5: Run the trade-session and runtime tests after removing the old API**

Run: `cargo test trade_session -- --nocapture`

Expected: PASS, with no remaining compile references to `on_order`, `on_trade`, `order_channel`, or `trade_channel`.

- [ ] **Step 6: Commit**

```bash
git add src/trade_session/mod.rs src/trade_session/core.rs src/trade_session/watch.rs src/runtime/execution.rs src/trade_session/tests.rs
git commit -m "refactor: replace legacy trade callbacks with reliable event streams"
```

## Task 4: Update Public Docs, AGENTS, And Examples To The New Canonical API

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `examples/trade.rs`
- Modify: `src/client/tests.rs`

- [ ] **Step 1: Verify the old example surface now fails to compile**

Run: `cargo check --example trade`

Expected: FAIL with missing methods such as `on_order`, `on_trade`, `order_channel`, or other removed trade-event APIs.

- [ ] **Step 2: Rewrite the README trade section to use reliable event streams**

```rust
let session = client
    .create_trade_session_with_options(
        "simnow",
        &sim_user_id,
        &sim_password,
        TradeSessionOptions {
            td_url_override: Some("wss://example.com/trade".to_string()),
            reliable_events_max_retained: 8_192,
        },
    )
    .await?;

session.connect().await?;

let mut order_events = session.subscribe_order_events();
tokio::spawn(async move {
    while let Ok(event) = order_events.recv().await {
        if let TradeSessionEventKind::OrderUpdated { order_id, order } = event.kind {
            println!("订单 {} 状态={}", order_id, order.status);
        }
    }
});

let mut trade_events = session.subscribe_trade_events();
tokio::spawn(async move {
    while let Ok(event) = trade_events.recv().await {
        if let TradeSessionEventKind::TradeCreated { trade_id, trade } = event.kind {
            println!("成交 {} 对应订单={}", trade_id, trade.order_id);
        }
    }
});
```

- [ ] **Step 3: Update `AGENTS.md` and `examples/trade.rs`**

```md
## 修改代码时的工作准则

- 修改公开 API 时，同时检查 `README.md`、`AGENTS.md`、示例代码、`prelude` re-export 是否需要同步。
- `TradeSession` 的 `order` / `trade` 事件以可靠事件流为 canonical API；不要新增或恢复 `on_order`、`on_trade`、`order_channel`、`trade_channel` 这类 best-effort public surface。
- 示例代码是对外接口的一部分；如果 API 变了，示例必须同步更新。
```

```rust
// examples/trade.rs
//! 演示以下功能：
//! - 实盘交易（可靠事件流）
//! - 账户 / 持仓快照查询
//! - `wait_order_update_reliable()` 的推荐用法

let mut order_events = trader.subscribe_order_events();
tokio::spawn(async move {
    while let Ok(event) = order_events.recv().await {
        if let TradeSessionEventKind::OrderUpdated { order_id, order } = event.kind {
            info!("📝 订单 {} 状态={} 剩余={}", order_id, order.status, order.volume_left);
        }
    }
});

let mut trade_events = trader.subscribe_trade_events();
tokio::spawn(async move {
    while let Ok(event) = trade_events.recv().await {
        if let TradeSessionEventKind::TradeCreated { trade_id, trade } = event.kind {
            info!("✅ 成交 {}: {}@{:.2}", trade_id, trade.volume, trade.price);
        }
    }
});
```

- [ ] **Step 4: Update the client test that constructs `TradeSessionOptions`**

```rust
let session = client
    .create_trade_session_with_options(
        "simnow",
        "user",
        "password",
        TradeSessionOptions {
            td_url_override: Some("wss://example.com/trade".to_string()),
            reliable_events_max_retained: 32,
        },
    )
    .await;
```

- [ ] **Step 5: Run docs/example/public-surface verification**

Run: `cargo check --examples`

Expected: PASS, and `examples/trade.rs` compiles against `subscribe_order_events()` / `subscribe_trade_events()` instead of the removed APIs.

- [ ] **Step 6: Commit**

```bash
git add README.md AGENTS.md examples/trade.rs src/client/tests.rs
git commit -m "docs: adopt reliable trade event api"
```

## Task 5: Full Verification And Final Consistency Sweep

**Files:**
- Modify: `src/trade_session/tests.rs` if verification exposes gaps
- Modify: `README.md` if examples drift from the code
- Modify: `AGENTS.md` if public-surface guidance still mentions removed APIs

- [ ] **Step 1: Search for stale references to removed trade-event APIs**

Run: `rg -n "on_order|on_trade|order_channel\\(|trade_channel\\(" README.md AGENTS.md examples src`

Expected: only spec / plan docs may mention removed symbols; production code, README, AGENTS, and examples should return no hits.

- [ ] **Step 2: Run the focused crates/tests**

Run: `cargo test trade_session_ -- --nocapture`
Expected: PASS for the focused `TradeSession` suite.

Run: `cargo test create_trade_session_plumbs_reliable_event_retention -- --exact`
Expected: PASS for the client-side plumbing test.

Run: `cargo test runtime -- --nocapture`

Expected: PASS, including the updated runtime wait path.

- [ ] **Step 3: Run the full repo checks**

Run: `cargo fmt --check`
Expected: PASS

Run: `cargo test`
Expected: PASS

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS

Run: `cargo check --examples`
Expected: PASS

- [ ] **Step 4: Commit any final verification-driven fixes**

```bash
git add src/trade_session/tests.rs README.md AGENTS.md examples/trade.rs
git commit -m "test: finalize reliable trade event verification"
```

## Self-Review

- Spec coverage:
  - public reliable event API: Task 1-3
  - multi-consumer cursor model: Task 1
  - full-snapshot order dedupe + seen-trade-id dedupe: Task 2
  - runtime migration: Task 3
  - AGENTS / README / examples sync: Task 4
  - explicit stale-reference and full-gate verification: Task 5
- Placeholder scan:
  - no `TODO` / `TBD`
  - each code-changing step includes concrete code blocks
  - each validation step includes exact commands and expected outcomes
- Type consistency:
  - `TradeSessionEvent`, `TradeSessionEventKind`, `TradeEventRecvError`, `TradeEventStream`, `OrderEventStream`, `TradeOnlyEventStream`, `wait_order_update_reliable`, and `TradeSessionOptions.reliable_events_max_retained` are used consistently across all tasks
