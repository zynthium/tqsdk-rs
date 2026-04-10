# DataManager Epoch Subscription And TradeSession Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 `DataManager` 增加完成态 epoch 订阅能力，并把 `TradeSession` 的内部 watcher 从 `on_data_register` 迁移到这条新路径，为后续删除全局 callback plumbing 铺路。

**Architecture:** `DataManager` 新增 `tokio::sync::watch::Sender<i64>`，只在一次 `merge_data(..., epoch_increase = true, ...)` 完整落地之后发布最终 epoch。`TradeSession` 不再向 `DataManager` 注册全局 callback，而是持有一个长期运行的 epoch watch task：收到新 epoch 后，再用既有 `get_path_epoch()` / `get_*_data()` 逻辑拆分账户、持仓和可靠 `order` / `trade` 事件。旧 `on_data` API 暂时保留给其他模块，直到 Quote / Series / legacy backtest 全部迁走。

**Tech Stack:** Rust 2024, `tokio::sync::watch`, `tokio::task::JoinHandle`, existing `DataManager`, `TradeSession`, `TradeEventHub`, `cargo test`, `cargo check --examples`, `cargo clippy`.

---

## File Map

- `src/datamanager/mod.rs`
  存放 `DataManager` 新的 `epoch_tx` 字段，保持现有 callback/watch/path-query 结构不动。
- `src/datamanager/core.rs`
  初始化 epoch channel，并暴露 `subscribe_epoch()`。
- `src/datamanager/merge.rs`
  在 merge 完成、衍生字段写回、同步 callback 跑完之后发布最终 epoch。
- `src/datamanager/tests.rs`
  覆盖 `subscribe_epoch()` 的最终态、coalesce、慢消费者补偿语义。
- `src/trade_session/mod.rs`
  删除 `data_cb_id`，改成 watcher task handle。
- `src/trade_session/core.rs`
  构造 watcher task handle，并提供 `stop_watch_task()` 生命周期清理。
- `src/trade_session/watch.rs`
  用 `subscribe_epoch()` 改写 `start_watching()`，保留现有按 path epoch 过滤的账户 / 持仓 / 订单 / 成交处理逻辑。
- `src/trade_session/ops.rs`
  连接失败与关闭路径改为清理 watcher task，而不是注销 callback id。
- `src/trade_session/tests.rs`
  覆盖“不再注册 DataManager callback”以及 epoch-driven watcher 仍然正确分发 snapshot / reliable event。
- `examples/datamanager.rs`
  把 `on_data` 示例改成 `subscribe_epoch()` 示例。
- `README.md`
  在 DataManager 段落中加入 `subscribe_epoch()` 用法。
- `AGENTS.md`
  把 `subscribe_epoch()` 写成新的内部首选通知机制，避免继续扩散 `on_data_register` 依赖。
- `docs/architecture.md`
  更新 `datamanager` / `trade_session` 架构描述，反映 epoch 订阅路径。

### Task 1: Add `DataManager::subscribe_epoch()`

**Files:**
- Modify: `src/datamanager/mod.rs`
- Modify: `src/datamanager/core.rs`
- Modify: `src/datamanager/merge.rs`
- Test: `src/datamanager/tests.rs`

- [ ] **Step 1: Write the failing epoch-subscription tests in `src/datamanager/tests.rs`**

```rust
#[tokio::test]
async fn subscribe_epoch_observes_completed_merge_state() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );

    rx.changed().await.unwrap();

    assert_eq!(*rx.borrow(), 1);
    assert_eq!(
        dm.get_by_path(&["quotes", "SHFE.au2602", "last_price"]),
        Some(json!(500.0))
    );
    assert_eq!(dm.get_path_epoch(&["quotes", "SHFE.au2602"]), 1);
}

#[tokio::test]
async fn subscribe_epoch_coalesces_multiple_merges_but_keeps_latest_epoch() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );

    rx.changed().await.unwrap();

    assert_eq!(*rx.borrow(), 2);
    assert_eq!(dm.get_quote_data("SHFE.au2602").unwrap().last_price, 501.0);
}

#[tokio::test]
async fn subscribe_epoch_slow_consumer_can_reconcile_with_path_epoch() {
    let dm = DataManager::new(HashMap::new(), DataManagerConfig::default());
    let mut rx = dm.subscribe_epoch();

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": {
                            "balance": 1000.0,
                            "available": 900.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );
    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": {
                            "balance": 1002.0,
                            "available": 901.0
                        }
                    }
                }
            }
        }),
        true,
        true,
    );

    rx.changed().await.unwrap();

    let seen_epoch = *rx.borrow();
    let path_epoch = dm.get_path_epoch(&["trade", "u", "accounts", "CNY"]);
    let account = dm.get_account_data("u", "CNY").unwrap();

    assert_eq!(seen_epoch, 2);
    assert_eq!(path_epoch, 2);
    assert_eq!(account.balance, 1002.0);
    assert_eq!(account.available, 901.0);
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test subscribe_epoch --lib -- --nocapture`

Expected: compile failure because `DataManager::subscribe_epoch()` and the internal epoch watch channel do not exist yet.

- [ ] **Step 3: Add the epoch watch channel and publish final merge epochs**

```rust
// src/datamanager/mod.rs
use tokio::sync::watch;

pub struct DataManager {
    data: Arc<RwLock<HashMap<String, Value>>>,
    epoch: AtomicI64,
    epoch_tx: watch::Sender<i64>,
    config: DataManagerConfig,
    watchers: Arc<RwLock<HashMap<String, Vec<PathWatcher>>>>,
    on_data_callbacks: DataCallbacks,
    next_callback_id: AtomicI64,
    next_watcher_id: AtomicI64,
}
```

```rust
// src/datamanager/core.rs
impl DataManager {
    pub fn new(initial_data: HashMap<String, serde_json::Value>, config: DataManagerConfig) -> Self {
        let (epoch_tx, _) = tokio::sync::watch::channel(0i64);
        Self {
            data: Arc::new(RwLock::new(initial_data)),
            epoch: std::sync::atomic::AtomicI64::new(0),
            epoch_tx,
            config,
            watchers: Arc::new(RwLock::new(HashMap::new())),
            on_data_callbacks: Arc::new(RwLock::new(Vec::new())),
            next_callback_id: std::sync::atomic::AtomicI64::new(1),
            next_watcher_id: std::sync::atomic::AtomicI64::new(1),
        }
    }

    pub fn subscribe_epoch(&self) -> tokio::sync::watch::Receiver<i64> {
        self.epoch_tx.subscribe()
    }
}
```

```rust
// src/datamanager/merge.rs
if epoch_increase {
    let current_epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    // merge source items into self.data
    self.apply_python_data_semantics(&mut data, current_epoch);
    drop(data);

    let callbacks = self.on_data_callbacks.read().unwrap();
    for entry in callbacks.iter() {
        (entry.f)();
    }
    drop(callbacks);

    self.epoch_tx.send_replace(current_epoch);
    true
} else {
    // existing non-epoch branch unchanged
}
```

- [ ] **Step 4: Run the targeted tests to verify the new semantics**

Run: `cargo test subscribe_epoch --lib -- --nocapture`

Expected: PASS, including:
- receiver sees the merged state, not partial data
- multiple merges coalesce to the latest epoch
- slow consumers can reconcile using `get_path_epoch()`

- [ ] **Step 5: Commit the DataManager epoch primitives**

```bash
git add src/datamanager/mod.rs src/datamanager/core.rs src/datamanager/merge.rs src/datamanager/tests.rs
git commit -m "feat: add datamanager epoch subscription"
```

### Task 2: Migrate `TradeSession` Off `on_data_register`

**Files:**
- Modify: `src/trade_session/mod.rs`
- Modify: `src/trade_session/core.rs`
- Modify: `src/trade_session/watch.rs`
- Modify: `src/trade_session/ops.rs`
- Test: `src/trade_session/tests.rs`

- [ ] **Step 1: Add failing migration tests in `src/trade_session/tests.rs`**

```rust
fn account_json(balance: f64, available: f64) -> serde_json::Value {
    json!({
        "user_id": "u",
        "currency": "CNY",
        "balance": balance,
        "available": available
    })
}

fn position_json() -> serde_json::Value {
    json!({
        "user_id": "u",
        "exchange_id": "SHFE",
        "instrument_id": "au2602",
        "volume_long_today": 1,
        "volume_long": 1
    })
}

#[tokio::test]
async fn trade_session_start_watching_uses_epoch_subscription_instead_of_datamanager_callbacks() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));

    session.running.store(true, Ordering::SeqCst);
    assert_eq!(dm.callback_count_for_test(), 0);

    session.start_watching().await;

    assert_eq!(dm.callback_count_for_test(), 0);
    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_epoch_watcher_still_delivers_snapshot_and_reliable_event_updates() {
    let dm = build_dm();
    let session = build_session(Arc::clone(&dm));
    let account_rx = session.account_channel();
    let position_rx = session.position_channel();
    let mut events = session.subscribe_events();

    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "accounts": {
                        "CNY": account_json(1000.0, 900.0)
                    },
                    "positions": {
                        "SHFE.au2602": position_json()
                    },
                    "orders": {
                        "o1": order_json("o1", "ALIVE")
                    },
                    "trades": {
                        "t1": trade_json("o1", "t1")
                    }
                }
            }
        }),
        true,
        true,
    );

    let account = timeout(Duration::from_secs(1), account_rx.recv()).await.unwrap().unwrap();
    let position = timeout(Duration::from_secs(1), position_rx.recv()).await.unwrap().unwrap();
    let first = timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap();
    let second = timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap();

    assert_eq!(account.balance, 1000.0);
    assert_eq!(position.symbol, "SHFE.au2602");
    assert!(session.logged_in.load(Ordering::SeqCst));
    assert!(matches!(
        first.kind,
        TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
    ));
    assert!(matches!(
        second.kind,
        TradeSessionEventKind::TradeCreated { ref trade_id, .. } if trade_id == "t1"
    ));

    session.close().await.unwrap();
}
```

- [ ] **Step 2: Run the new tests to verify the current callback-based watcher fails the migration assertion**

Run: `cargo test trade_session_start_watching_ --lib -- --nocapture`

Expected: FAIL at `callback_count_for_test() == 0` because `TradeSession::start_watching()` still calls `DataManager::on_data_register(...)`.

- [ ] **Step 3: Replace callback-registration lifecycle with an epoch watch task**

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
    notification_tx: Sender<Notification>,
    notification_rx: Receiver<Notification>,
    on_account: AccountCallback,
    on_position: PositionCallback,
    on_notification: NotificationCallback,
    on_error: ErrorCallback,
    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    watch_task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}
```

```rust
// src/trade_session/core.rs
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
    notification_tx,
    notification_rx,
    on_account: Arc::new(RwLock::new(None)),
    on_position: Arc::new(RwLock::new(None)),
    on_notification,
    on_error,
    logged_in: Arc::new(AtomicBool::new(false)),
    running: Arc::new(AtomicBool::new(false)),
    watch_task: Arc::new(std::sync::Mutex::new(None)),
}

pub(super) fn stop_watch_task(&self) {
    if let Some(handle) = self.watch_task.lock().unwrap().take() {
        handle.abort();
    }
}
```

```rust
// src/trade_session/ops.rs
if let Err(e) = self.send_login().await {
    self.running.store(false, Ordering::SeqCst);
    self.logged_in.store(false, Ordering::SeqCst);
    self.stop_watch_task();
    let _ = self.ws.close().await;
    return Err(e);
}

if let Err(e) = self.send_confirm_settlement().await {
    self.running.store(false, Ordering::SeqCst);
    self.logged_in.store(false, Ordering::SeqCst);
    self.stop_watch_task();
    let _ = self.ws.close().await;
    return Err(e);
}

pub async fn close(&self) -> Result<()> {
    if self
        .running
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        self.logged_in.store(false, Ordering::SeqCst);
        return Ok(());
    }

    info!("关闭交易会话");
    self.logged_in.store(false, Ordering::SeqCst);
    self.stop_watch_task();
    self.trade_events.close();
    self.ws.close().await?;
    Ok(())
}
```

```rust
// src/trade_session/watch.rs
pub(super) async fn start_watching(&self) {
    let mut guard = self.watch_task.lock().unwrap();
    if guard.as_ref().is_some_and(|handle| !handle.is_finished()) {
        return;
    }

    let dm = Arc::clone(&self.dm);
    let user_id = self.user_id.clone();
    let logged_in = Arc::clone(&self.logged_in);
    let running = Arc::clone(&self.running);
    let account_tx = self.account_tx.clone();
    let position_tx = self.position_tx.clone();
    let trade_events = Arc::clone(&self.trade_events);
    let on_account = Arc::clone(&self.on_account);
    let on_position = Arc::clone(&self.on_position);
    let mut epoch_rx = dm.subscribe_epoch();

    *guard = Some(tokio::spawn(async move {
        let mut last_processed_epoch = 0i64;
        loop {
            if !running.load(Ordering::SeqCst) {
                break;
            }

            let current_global_epoch = *epoch_rx.borrow_and_update();
            if current_global_epoch <= last_processed_epoch {
                if epoch_rx.changed().await.is_err() {
                    break;
                }
                continue;
            }

            if let Some(serde_json::Value::Object(session_map)) = dm.get_by_path(&["trade", &user_id, "session"])
                && session_map
                    .get("trading_day")
                    .is_some_and(|trading_day| !trading_day.is_null())
                && logged_in
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                info!("交易会话已登录: user_id={}", user_id);
            }

            if dm.get_path_epoch(&["trade", &user_id, "accounts", "CNY"]) > last_processed_epoch {
                match dm.get_account_data(&user_id, "CNY") {
                    Ok(account) => {
                        match account_tx.try_send(account.clone()) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => warn!("TradeSession 账户队列已满，丢弃一次更新"),
                            Err(TrySendError::Closed(_)) => {}
                        }
                        if let Some(callback) = on_account.read().await.clone() {
                            callback(account);
                        }
                    }
                    Err(e) => error!("获取账户数据失败（这不应该发生）: {}", e),
                }
            }

            if dm.get_path_epoch(&["trade", &user_id, "positions"]) > last_processed_epoch {
                Self::process_position_update(&dm, &user_id, &position_tx, &on_position, last_processed_epoch).await;
            }
            if dm.get_path_epoch(&["trade", &user_id, "orders"]) > last_processed_epoch {
                Self::process_order_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
            }
            if dm.get_path_epoch(&["trade", &user_id, "trades"]) > last_processed_epoch {
                Self::process_trade_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
            }

            last_processed_epoch = current_global_epoch;
        }
    }));
}
```

- [ ] **Step 4: Run the targeted migration tests**

Run: `cargo test trade_session_start_watching_ --lib -- --nocapture`

Expected: PASS, including:
- `TradeSession` no longer increments `DataManager` callback count
- epoch-driven watcher still produces account / position snapshot updates
- reliable `order` / `trade` events still flow through `TradeEventHub`

- [ ] **Step 5: Commit the TradeSession migration**

```bash
git add src/trade_session/mod.rs src/trade_session/core.rs src/trade_session/watch.rs src/trade_session/ops.rs src/trade_session/tests.rs
git commit -m "refactor: migrate trade session to datamanager epoch"
```

### Task 3: Update Examples And Documentation

**Files:**
- Modify: `examples/datamanager.rs`
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/architecture.md`

- [ ] **Step 1: Replace the `on_data` example in `examples/datamanager.rs` with an epoch subscription example**

```rust
//! DataManager 高级功能示例
//!
//! 演示以下功能：
//! - 路径监听 (Watch/UnWatch)
//! - 数据访问 (Get/GetByPath)
//! - 多路径同时监听
//! - merge 完成后的 epoch 订阅

async fn epoch_subscription_example() {
    info!("==================== DataManager Epoch 示例 ====================");

    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));

    let dm = Arc::new(DataManager::new(initial_data, DataManagerConfig::default()));
    let mut epoch_rx = dm.subscribe_epoch();
    let dm_for_task = Arc::clone(&dm);

    tokio::spawn(async move {
        while epoch_rx.changed().await.is_ok() {
            let epoch = *epoch_rx.borrow_and_update();
            info!("🔔 merge 完成，当前 epoch: {}", epoch);
            if let Some(data) = dm_for_task.get_by_path(&["quotes", "SHFE.au2512"]) {
                info!("   SHFE.au2512 数据: {:?}", data);
            }
        }
    });

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 500.0,
                    "volume": 1000
                }
            }
        }),
        true,
        false,
    );

    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2512": {
                    "last_price": 501.5,
                    "volume": 1200
                }
            }
        }),
        true,
        false,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    info!("epoch 示例结束");
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    watch_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    data_access_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    multi_watch_example().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    epoch_subscription_example().await;
    info!("\n所有示例运行完成!");
}
```

- [ ] **Step 2: Document `subscribe_epoch()` in `README.md` and `docs/architecture.md`**

```rust
// README.md DataManager section
let mut epoch_rx = dm.subscribe_epoch();
tokio::spawn(async move {
    while epoch_rx.changed().await.is_ok() {
        println!("merge 完成后的全局 epoch = {}", *epoch_rx.borrow_and_update());
    }
});

dm.merge_data(
    serde_json::json!({
        "quotes": {
            "SHFE.au2602": { "last_price": 500.0 }
        }
    }),
    true,
    true,
);
```

```markdown
<!-- docs/architecture.md -->
- `datamanager/core`：创建、epoch 订阅、legacy callback 注册
- `trade_session/watch`：监听 DataManager epoch，再按 path epoch 生成 snapshot / reliable event
```

- [ ] **Step 3: Update `AGENTS.md` so future contributors prefer epoch subscription over new callback registrations**

```markdown
- `DataManager` 的 merge 通知现在优先使用 `subscribe_epoch()`；除非是在迁移旧代码，不要继续为新逻辑扩散 `on_data_register` 依赖。
- `TradeSession` 内部 watcher 已经基于 epoch 订阅；后续 Quote / Series / legacy backtest 清理应沿用这条路径，而不是新增 callback plumbing。
```

- [ ] **Step 4: Run example and docs compilation checks**

Run: `cargo check --example datamanager`

Expected: PASS, and the example compiles against `subscribe_epoch()` instead of `on_data(...)`.

- [ ] **Step 5: Commit the example and doc updates**

```bash
git add examples/datamanager.rs README.md AGENTS.md docs/architecture.md
git commit -m "docs: document datamanager epoch subscription"
```

### Task 4: Full Verification Sweep

**Files:**
- Modify: none

- [ ] **Step 1: Run the focused regression tests first**

Run: `cargo test subscribe_epoch --lib -- --nocapture`

Expected: PASS

Run: `cargo test trade_session_start_watching --lib -- --nocapture`

Expected: PASS

- [ ] **Step 2: Run the full unit/integration suite**

Run: `cargo test`

Expected: PASS

- [ ] **Step 3: Run strict linting**

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS

- [ ] **Step 4: Re-check examples after the migration**

Run: `cargo check --examples`

Expected: PASS

## Self-Review

- Spec coverage:
  - `DataManager::subscribe_epoch()` is implemented and tested in Task 1.
  - `TradeSession` is migrated off `on_data_register` in Task 2.
  - public example / README / AGENTS / architecture updates are covered in Task 3.
  - repo-required verification matrix is covered in Task 4.
- Placeholder scan:
  - no `TODO` / `TBD` / “similar to above” references remain.
  - every code-changing task has concrete code snippets and exact commands.
- Type consistency:
  - `subscribe_epoch()` is the only new public API introduced here.
  - `stop_watch_task()` consistently replaces `detach_data_callback()` inside `TradeSession`.
  - reliable event APIs remain `subscribe_events()` / `subscribe_order_events()` / `subscribe_trade_events()` / `wait_order_update_reliable()`.
