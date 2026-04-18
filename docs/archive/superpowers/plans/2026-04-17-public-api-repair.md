# Public API Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 `tqsdk-rs` 当前公开 API 在状态错误表达、`DataManager` watch ownership、`ReplaySession` runtime 初始化一致性和 market ref 显式 readiness 上的不清晰 contract，同时保持本轮改动 non-breaking。

**Architecture:** 第一阶段只做 additive / behavior-tightening 修复，不删除也不重命名现有 public entry points。核心策略是：新增 typed state errors 与 explicit helper API，把危险语义变成“现有接口仍兼容，但新接口与文档给出更清晰 contract”，再用测试和 README / agent 文档把推荐路径固化。

**Tech Stack:** Rust 2024, `tokio`, `thiserror`, `async-channel`, `cargo test`, `cargo clippy`

---

## Scope Inputs

- Review findings confirmed on `2026-04-17`
- Canonical contract docs: `README.md`, `AGENTS.md`, `CLAUDE.md`
- Live API files: `src/client/mod.rs`, `src/client/facade.rs`, `src/client/market.rs`
- Market ref files: `src/marketdata/mod.rs`, `src/marketdata/tests.rs`
- DataManager files: `src/datamanager/mod.rs`, `src/datamanager/watch.rs`, `src/datamanager/tests.rs`
- Replay files: `src/replay/session.rs`, `src/replay/tests.rs`
- Existing contract tests: `src/client/tests.rs`, `src/series/tests.rs`, `src/trade_session/tests.rs`
- Skill docs to sync: `skills/tqsdk-rs/references/client-and-endpoints.md`, `skills/tqsdk-rs/references/market-and-series.md`, `skills/tqsdk-rs/references/error-faq.md`

## File Map

- `src/errors.rs`: 统一新增 typed state errors，替代当前过载的 `InternalError`
- `src/client/mod.rs`: 保留现有 `quote/kline_ref/tick_ref`，新增 checked getter
- `src/client/facade.rs`: query / series / quote facade 的 market gating 改成 typed errors
- `src/client/market.rs`: `init_market()` 对 closed client 的错误改成 typed state error
- `src/marketdata/mod.rs`: `QuoteRef` / `KlineRef` / `TickRef` 增加 `snapshot/is_ready/try_load`，并把 `load()` 文档降格为 legacy convenience
- `src/datamanager/mod.rs`: 新增并导出 `DataWatchHandle`
- `src/datamanager/watch.rs`: `watch_handle()` public API、legacy `watch()` 兼容实现、`unwatch()` legacy 说明
- `src/replay/session.rs`: 规范化 runtime account set 并拒绝二次不一致初始化
- `src/lib.rs`: root re-export `DataWatchHandle`
- `src/client/tests.rs`: typed error 与 checked getter 行为测试
- `src/marketdata/tests.rs`: market ref readiness helper 测试
- `src/datamanager/tests.rs`: `DataWatchHandle` ownership 测试
- `src/replay/tests.rs`: `ReplaySession::runtime(accounts)` 一致性测试
- `README.md`: public contract 推荐用法与 legacy 说明
- `AGENTS.md`, `CLAUDE.md`: agent 记忆同步
- `skills/tqsdk-rs/references/*.md`: 子技能知识包同步

## Non-Goals

- 本轮不把 `QuoteRef::load()` / `KlineRef::load()` / `TickRef::load()` 改成 breaking signature
- 本轮不删除 `DataManager::watch()` / `DataManager::unwatch()`
- 本轮不引入 typestate `Client<Initialized>` 重构
- 本轮不重写 replay/runtime 架构

## Chunk 1: Typed State Errors

### Task 1: 用 typed state errors 替换当前 generic `InternalError`

**Files:**
- Modify: `src/errors.rs`
- Modify: `src/client/facade.rs`
- Modify: `src/client/market.rs`
- Modify: `src/marketdata/mod.rs`
- Modify: `src/series/subscription.rs`
- Modify: `src/trade_session/core.rs`
- Test: `src/client/tests.rs`
- Test: `src/series/tests.rs`
- Test: `src/trade_session/tests.rs`

- [ ] **Step 1: 先写失败测试，锁定新的错误 contract**

在 `src/client/tests.rs` 新增：

```rust
#[tokio::test]
async fn close_invalidates_market_interfaces_with_typed_errors() {
    let client = build_client_with_market();
    client.close().await.unwrap();

    let query_err = client
        .query_quotes(None, None, None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(query_err, TqError::ClientClosed { .. }));

    let subscribe_err = client.subscribe_quote(&["SHFE.au2602"]).await.unwrap_err();
    assert!(matches!(subscribe_err, TqError::ClientClosed { .. }));

    let series_err = client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await
        .unwrap_err();
    assert!(matches!(series_err, TqError::ClientClosed { .. }));
}
```

在 `src/series/tests.rs` 新增：

```rust
#[tokio::test]
async fn series_wait_update_after_close_returns_subscription_closed() {
    let sub = build_series_subscription(16).await;
    sub.close().await.unwrap();

    let err = sub.wait_update().await.unwrap_err();
    assert!(matches!(err, TqError::SubscriptionClosed { .. }));
}
```

在 `src/trade_session/tests.rs` 新增：

```rust
#[tokio::test]
async fn trade_session_wait_update_before_connect_returns_not_connected() {
    let session = build_trade_session_for_test();

    let err = session.wait_update().await.unwrap_err();
    assert!(matches!(err, TqError::TradeSessionNotConnected));
}
```

- [ ] **Step 2: 运行测试，确认它们先失败**

Run:

```bash
cargo test close_invalidates_market_interfaces_with_typed_errors -- --nocapture
cargo test series_wait_update_after_close_returns_subscription_closed -- --nocapture
cargo test trade_session_wait_update_before_connect_returns_not_connected -- --nocapture
```

Expected:

- FAIL
- 至少出现一种错误：`no variant named ClientClosed` / `no variant named SubscriptionClosed` / `no variant named TradeSessionNotConnected`

- [ ] **Step 3: 在 `src/errors.rs` 定义新的状态错误，并提供简短 helper**

把下面这些 variant 加进 `TqError`：

```rust
#[error("行情功能尚未初始化；请先调用 Client::init_market() 再使用 {capability}")]
MarketNotInitialized { capability: &'static str },

#[error("Client live session 已关闭；不能继续使用 {capability}")]
ClientClosed { capability: &'static str },

#[error("订阅已关闭: {kind}")]
SubscriptionClosed { kind: &'static str },

#[error("TradeSession 尚未连接或已关闭")]
TradeSessionNotConnected,

#[error("数据尚未就绪: {resource}")]
DataNotReady(String),
```

并在 `impl TqError` 中补 helper，避免每个 call site 自己拼字符串：

```rust
pub fn market_not_initialized(capability: &'static str) -> Self {
    Self::MarketNotInitialized { capability }
}

pub fn client_closed(capability: &'static str) -> Self {
    Self::ClientClosed { capability }
}

pub fn subscription_closed(kind: &'static str) -> Self {
    Self::SubscriptionClosed { kind }
}
```

- [ ] **Step 4: 用新错误替换最关键 call site**

按下面方式收口：

```rust
// src/client/facade.rs
fn series_api(&self) -> Result<Arc<SeriesAPI>> {
    if self.live.market_state.is_closed() {
        return Err(TqError::client_closed("series API"));
    }
    self.live
        .series_api
        .clone()
        .ok_or_else(|| TqError::market_not_initialized("series API"))
}

fn ins_api(&self) -> Result<Arc<InsAPI>> {
    if self.live.market_state.is_closed() {
        return Err(TqError::client_closed("ins/query API"));
    }
    self.live
        .ins_api
        .clone()
        .ok_or_else(|| TqError::market_not_initialized("ins/query API"))
}

pub async fn subscribe_quote(&self, symbols: &[&str]) -> Result<Arc<QuoteSubscription>> {
    if self.live.market_state.is_closed() {
        return Err(TqError::client_closed("quote subscription"));
    }
    if !self.live.is_active() || self.live.quotes_ws.is_none() {
        return Err(TqError::market_not_initialized("quote subscription"));
    }
    // 其余逻辑保持不变
}
```

```rust
// src/client/market.rs
pub async fn init_market(&mut self) -> Result<()> {
    if self.live.market_state.is_closed() {
        return Err(TqError::client_closed("init_market"));
    }
    let _ = self.initialize_market_runtime(false).await?;
    Ok(())
}
```

```rust
// src/marketdata/mod.rs
if *close_rx.borrow() {
    return Err(TqError::client_closed("market data session"));
}
changed.map_err(|_| TqError::subscription_closed("quote epoch"))?;
```

```rust
// src/series/subscription.rs
if !*self.running.read().await {
    return Err(TqError::subscription_closed("series snapshot"));
}
if rx.changed().await.is_err() {
    return Err(TqError::subscription_closed("series snapshot"));
}
```

```rust
// src/trade_session/core.rs
if !self.running.load(Ordering::SeqCst) {
    return Err(TqError::TradeSessionNotConnected);
}
```

- [ ] **Step 5: 运行针对性测试，确认 typed errors 行为闭合**

Run:

```bash
cargo test close_invalidates_market_interfaces_with_typed_errors -- --nocapture
cargo test close_invalidates_market_wait_paths -- --nocapture
cargo test series_wait_update_after_close_returns_subscription_closed -- --nocapture
cargo test trade_session_wait_update_before_connect_returns_not_connected -- --nocapture
```

Expected:

- PASS
- 旧测试 `close_invalidates_market_wait_paths` 继续通过，不出现回归

- [ ] **Step 6: Commit**

```bash
git add src/errors.rs src/client/facade.rs src/client/market.rs src/marketdata/mod.rs src/series/subscription.rs src/trade_session/core.rs src/client/tests.rs src/series/tests.rs src/trade_session/tests.rs
git commit -m "fix(api): add typed state errors for market and session lifecycle"
```

## Chunk 2: DataManager Watch Ownership

### Task 2: 公开 `DataWatchHandle`，保留 `watch()` 兼容但不再把它当推荐路径

**Files:**
- Modify: `src/datamanager/mod.rs`
- Modify: `src/datamanager/watch.rs`
- Modify: `src/lib.rs`
- Test: `src/datamanager/tests.rs`

- [ ] **Step 1: 先写失败测试，锁定 handle-based ownership**

在 `src/datamanager/tests.rs` 新增：

```rust
#[test]
fn data_watch_handle_cancel_is_per_registration() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let mut first = dm.watch_handle(path.clone());
    let second = dm.watch_handle(path.clone());

    assert!(first.cancel());

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

    assert!(first.receiver().try_recv().is_err());
    assert!(second.receiver().try_recv().is_ok());
}
```

再新增：

```rust
#[test]
fn data_watch_handle_drop_unregisters_its_own_watcher_only() {
    let mut initial_data = HashMap::new();
    initial_data.insert("quotes".to_string(), json!({}));
    let dm = DataManager::new(initial_data, DataManagerConfig::default());

    let path = vec!["quotes".to_string(), "SHFE.au2602".to_string()];
    let path_key = path.join(".");
    let first = dm.watch_handle(path.clone());
    let second = dm.watch_handle(path.clone());

    drop(first);
    assert_eq!(dm.watchers.read().unwrap().get(&path_key).map(Vec::len), Some(1));

    drop(second);
    assert!(!dm.watchers.read().unwrap().contains_key(&path_key));
}
```

- [ ] **Step 2: 运行测试，确认它们先失败**

Run:

```bash
cargo test data_watch_handle_cancel_is_per_registration -- --nocapture
cargo test data_watch_handle_drop_unregisters_its_own_watcher_only -- --nocapture
```

Expected:

- FAIL
- 编译错误类似 `no method named watch_handle found for struct DataManager`

- [ ] **Step 3: 新增 public `DataWatchHandle` 并让它包装现有 `WatchRegistration`**

在 `src/datamanager/mod.rs` 增加：

```rust
pub struct DataWatchHandle {
    inner: watch::WatchRegistration,
}
```

在 `src/datamanager/watch.rs` 增加 public API：

```rust
impl DataManager {
    pub fn watch_handle(&self, path: Vec<String>) -> DataWatchHandle {
        DataWatchHandle {
            inner: self.watch_register(path),
        }
    }

    /// Legacy convenience API.
    /// 对同一路径存在多个 watcher 时，不要再依赖 `unwatch(path)` 做精确释放。
    pub fn watch(&self, path: Vec<String>) -> Receiver<Value> {
        self.watch_handle(path).into_receiver()
    }
}

impl DataWatchHandle {
    pub fn receiver(&self) -> &Receiver<Value> {
        self.inner.receiver()
    }

    pub fn cancel(&mut self) -> bool {
        self.inner.cancel()
    }

    pub fn into_receiver(self) -> Receiver<Value> {
        self.inner.into_receiver()
    }
}
```

注意：

- `WatchRegistration` 继续保持内部类型
- 本轮不要给 `watch()` / `unwatch()` 加 `#[deprecated]` 属性，避免 `-D warnings` 下把现有测试和知识包一起打爆

- [ ] **Step 4: 把 `DataWatchHandle` 加到 root export**

修改 `src/lib.rs`：

```rust
pub use datamanager::{DataManager, DataManagerConfig, DataWatchHandle};
```

- [ ] **Step 5: 运行 DataManager 测试，确认 legacy 和 new API 共存**

Run:

```bash
cargo test test_watch_allows_multiple_receivers_for_same_path -- --nocapture
cargo test datamanager_watchers_same_path_cancel_independently -- --nocapture
cargo test data_watch_handle_cancel_is_per_registration -- --nocapture
cargo test data_watch_handle_drop_unregisters_its_own_watcher_only -- --nocapture
```

Expected:

- PASS
- 旧测试继续证明内部 watcher 粒度是独立的
- 新测试证明 public handle 现在也能表达这套 ownership

- [ ] **Step 6: Commit**

```bash
git add src/datamanager/mod.rs src/datamanager/watch.rs src/lib.rs src/datamanager/tests.rs
git commit -m "feat(api): expose datamanager watch handles without breaking legacy watch"
```

## Chunk 3: Replay Runtime Consistency

### Task 3: 让 `ReplaySession::runtime(accounts)` 对 account set 一致性变成显式 contract

**Files:**
- Modify: `src/replay/session.rs`
- Test: `src/replay/tests.rs`
- Modify: `README.md`

- [ ] **Step 1: 先写失败测试，锁定 account-set contract**

在 `src/replay/tests.rs` 新增：

```rust
#[tokio::test]
async fn replay_session_runtime_accepts_same_account_set_in_different_order() {
    let mut session = build_replay_session_for_test().await;

    let first = session.runtime(["TQSIM", "SIM2"]).await.unwrap();
    let second = session.runtime(["SIM2", "TQSIM", "TQSIM"]).await.unwrap();

    assert!(Arc::ptr_eq(&first, &second));
}
```

再新增：

```rust
#[tokio::test]
async fn replay_session_runtime_rejects_mismatched_account_sets() {
    let mut session = build_replay_session_for_test().await;

    let _ = session.runtime(["TQSIM"]).await.unwrap();
    let err = session.runtime(["SIM2"]).await.unwrap_err();

    assert!(matches!(err, TqError::InvalidParameter(_)));
}
```

- [ ] **Step 2: 运行测试，确认第二个测试先失败**

Run:

```bash
cargo test replay_session_runtime_accepts_same_account_set_in_different_order -- --nocapture
cargo test replay_session_runtime_rejects_mismatched_account_sets -- --nocapture
```

Expected:

- 第一条可能已经 PASS，也可能 FAIL
- 第二条必须 FAIL，因为当前实现会静默返回旧 runtime

- [ ] **Step 3: 给 `ReplaySession` 记录规范化后的 account set**

在 `src/replay/session.rs` 增加字段：

```rust
pub struct ReplaySession {
    config: ReplayConfig,
    bootstrap: ReplayBootstrapper,
    kernel: Arc<TokioMutex<ReplayKernel>>,
    market: Arc<ReplayMarketState>,
    execution: Option<Arc<ReplayExecutionState>>,
    runtime: Option<Arc<TqRuntime>>,
    runtime_account_keys: Option<Vec<String>>,
    active_trading_day: Option<NaiveDate>,
    active_trading_day_end_nanos: Option<i64>,
}
```

再增加 helper：

```rust
fn normalize_runtime_accounts<I, S>(accounts: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut keys = accounts
        .into_iter()
        .map(|account| account.as_ref().trim().to_string())
        .filter(|account| !account.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    if keys.is_empty() {
        return Err(TqError::InvalidParameter(
            "ReplaySession::runtime accounts 不能为空".to_string(),
        ));
    }

    Ok(std::mem::take(&mut keys))
}
```

- [ ] **Step 4: 在 `runtime()` 里显式校验一致性**

把 `runtime()` 起始部分改成：

```rust
let account_keys = normalize_runtime_accounts(accounts)?;

if let Some(runtime) = &self.runtime {
    let existing = self
        .runtime_account_keys
        .as_ref()
        .expect("cached runtime must have cached account keys");
    if existing != &account_keys {
        return Err(TqError::InvalidParameter(format!(
            "ReplaySession::runtime 已用账户集 {:?} 初始化，不能再切换到 {:?}",
            existing, account_keys
        )));
    }
    return Ok(Arc::clone(runtime));
}
```

首次初始化成功后记住：

```rust
self.runtime_account_keys = Some(account_keys.clone());
```

- [ ] **Step 5: 运行 replay 针对性测试**

Run:

```bash
cargo test replay_session_runtime_accepts_same_account_set_in_different_order -- --nocapture
cargo test replay_session_runtime_rejects_mismatched_account_sets -- --nocapture
cargo test replay_session_runtime_can_drive_target_pos_task -- --nocapture
```

Expected:

- PASS
- 原有 replay runtime 主路径测试不回归

- [ ] **Step 6: Commit**

```bash
git add src/replay/session.rs src/replay/tests.rs README.md
git commit -m "fix(replay): enforce stable account set for replay runtime"
```

## Chunk 4: Market Ref Readiness API

### Task 4: 为 market refs 增加显式 readiness helper，并新增 checked Client getter

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/marketdata/mod.rs`
- Test: `src/client/tests.rs`
- Test: `src/marketdata/tests.rs`

- [ ] **Step 1: 先写失败测试，锁定 explicit readiness 行为**

在 `src/client/tests.rs` 新增：

```rust
#[test]
fn checked_market_getters_require_initialized_market() {
    let client = build_inactive_client();

    let err = client.try_quote("SHFE.au2602").unwrap_err();
    assert!(matches!(err, TqError::MarketNotInitialized { .. }));
}
```

在 `src/marketdata/tests.rs` 新增：

```rust
#[tokio::test]
async fn quote_ref_try_load_reports_not_ready_before_first_snapshot() {
    let state = Arc::new(MarketDataState::default());
    let quote_ref = QuoteRef::new_for_test(Arc::clone(&state), "SHFE.au2602");

    assert!(!quote_ref.is_ready().await);
    assert!(quote_ref.snapshot().await.is_none());

    let err = quote_ref.try_load().await.unwrap_err();
    assert!(matches!(err, TqError::DataNotReady(_)));
}
```

再新增：

```rust
#[tokio::test]
async fn quote_ref_try_load_returns_live_snapshot_after_update() {
    let state = Arc::new(MarketDataState::default());
    let quote_ref = QuoteRef::new_for_test(Arc::clone(&state), "SHFE.au2602");

    state
        .update_quote(
            "SHFE.au2602".into(),
            Quote {
                instrument_id: "au2602".to_string(),
                last_price: 520.5,
                ..Default::default()
            },
        )
        .await;

    assert!(quote_ref.is_ready().await);
    let quote = quote_ref.try_load().await.unwrap();
    assert_eq!(quote.last_price, 520.5);
}
```

- [ ] **Step 2: 运行测试，确认它们先失败**

Run:

```bash
cargo test checked_market_getters_require_initialized_market -- --nocapture
cargo test quote_ref_try_load_reports_not_ready_before_first_snapshot -- --nocapture
cargo test quote_ref_try_load_returns_live_snapshot_after_update -- --nocapture
```

Expected:

- FAIL
- 编译错误类似 `no method named try_quote found for struct Client`
- 编译错误类似 `no method named try_load found for struct QuoteRef`

- [ ] **Step 3: 给 `Client` 增加 checked getter，但保留现有 infallible getter**

在 `src/client/mod.rs` 增加：

```rust
impl Client {
    fn ensure_market_ref_capability(&self, capability: &'static str) -> Result<()> {
        if self.live.market_state.is_closed() {
            return Err(TqError::client_closed(capability));
        }
        if !self.live.is_active() {
            return Err(TqError::market_not_initialized(capability));
        }
        Ok(())
    }

    pub fn try_quote(&self, symbol: &str) -> Result<QuoteRef> {
        self.ensure_market_ref_capability("quote ref")?;
        Ok(self.quote(symbol))
    }

    pub fn try_kline_ref(&self, symbol: &str, duration: Duration) -> Result<KlineRef> {
        self.ensure_market_ref_capability("kline ref")?;
        Ok(self.kline_ref(symbol, duration))
    }

    pub fn try_tick_ref(&self, symbol: &str) -> Result<TickRef> {
        self.ensure_market_ref_capability("tick ref")?;
        Ok(self.tick_ref(symbol))
    }
}
```

注意：

- 本轮不要改 `quote/kline_ref/tick_ref` 的原签名
- 现有 getter 继续作为 lightweight convenience API

- [ ] **Step 4: 给三种 ref 增加 `snapshot/is_ready/try_load`**

在 `src/marketdata/mod.rs` 对三种 ref 统一加下面这组方法：

```rust
impl QuoteRef {
    pub async fn snapshot(&self) -> Option<Arc<Quote>> {
        self.state.quote_snapshot(&self.symbol).await
    }

    pub async fn is_ready(&self) -> bool {
        self.snapshot().await.is_some()
    }

    pub async fn try_load(&self) -> Result<Arc<Quote>> {
        self.snapshot().await.ok_or_else(|| {
            TqError::DataNotReady(format!("quote {}", self.symbol()))
        })
    }

    /// Legacy convenience API.
    /// 在首帧到达前会返回默认值；新代码优先使用 `snapshot()` / `is_ready()` / `try_load()`
    pub async fn load(&self) -> Arc<Quote> {
        self.snapshot().await.unwrap_or_else(|| Arc::new(Quote::default()))
    }
}
```

`KlineRef` / `TickRef` 保持相同结构，只把 `Quote` 和资源名替换成对应类型。

在 `src/marketdata/tests.rs` 里补一个 `new_for_test()` helper，仅限测试：

```rust
#[cfg(test)]
impl QuoteRef {
    pub(crate) fn new_for_test(state: Arc<MarketDataState>, symbol: &str) -> Self {
        Self::new(state, symbol.into())
    }
}
```

- [ ] **Step 5: 运行 market ref 针对性测试**

Run:

```bash
cargo test checked_market_getters_require_initialized_market -- --nocapture
cargo test quote_ref_try_load_reports_not_ready_before_first_snapshot -- --nocapture
cargo test quote_ref_try_load_returns_live_snapshot_after_update -- --nocapture
cargo test client_exposes_market_state_refs_directly -- --nocapture
cargo test close_invalidates_market_wait_paths -- --nocapture
```

Expected:

- PASS
- 旧 contract 继续保留：现有 `load()` / `wait_update()` 测试不回归
- 新 contract 生效：现在可以显式表达 readiness

- [ ] **Step 6: Commit**

```bash
git add src/client/mod.rs src/marketdata/mod.rs src/client/tests.rs src/marketdata/tests.rs
git commit -m "feat(api): add explicit market ref readiness helpers"
```

## Chunk 5: Docs and Agent Memory Sync

### Task 5: 同步 README、agent 规则和技能知识包

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `skills/tqsdk-rs/references/client-and-endpoints.md`
- Modify: `skills/tqsdk-rs/references/market-and-series.md`
- Modify: `skills/tqsdk-rs/references/error-faq.md`

- [ ] **Step 1: 更新 README 的 canonical 说明**

把以下内容加入 `README.md` 的 live / market / replay 相关章节：

```md
> `Client::build()` 只负责认证与构造 session owner；
> live 行情能力在 `client.init_market().await?` 后才可用。
> 如需显式错误而不是延迟到后续调用时再失败，优先使用
> `Client::{try_quote,try_kline_ref,try_tick_ref}`。

> `QuoteRef` / `KlineRef` / `TickRef` 的 `load()` 是 legacy convenience API：
> 在首帧到达前会返回默认值。新代码优先使用
> `snapshot()` / `is_ready()` / `try_load()`。

> `DataManager::watch_handle()` 是推荐的 watcher 生命周期 API；
> `watch()` / `unwatch()` 仅保留为 legacy convenience，`unwatch(path)`
> 仍然是 path-wide 行为，不适合多个同路径 watcher 并存时做精确释放。

> `ReplaySession::runtime(accounts)` 在第一次成功初始化后会固定 account set；
> 后续只有传入同一组账户（顺序无关、重复忽略）才会复用同一个 runtime。
```

- [ ] **Step 2: 在 `AGENTS.md` 和 `CLAUDE.md` 补 public contract 规则**

在两份 agent 文档都补以下约束：

```md
- market ref 新代码优先使用 `try_load()` / `snapshot()` / `is_ready()`；
  `load()` 仅作为兼容 convenience API
- `DataManager` watcher 推荐使用 `watch_handle()`；不要再把 `unwatch(path)` 当成精确释放单个 watcher 的 canonical 路径
- `ReplaySession::runtime(accounts)` 初始化后账户集合固定；需要换账户集合时重新创建 `ReplaySession`
- `Client::build()` 后若要用 live 行情 / serial / query facade，仍需显式 `init_market()`
```

- [ ] **Step 3: 同步 skills 知识包**

在 `skills/tqsdk-rs/references/client-and-endpoints.md` 加：

```md
- `Client::build()` != `init_market()`
- 需要显式 market precondition 时用 `try_quote` / `try_kline_ref` / `try_tick_ref`
```

在 `skills/tqsdk-rs/references/market-and-series.md` 加：

```md
- `QuoteRef` / `KlineRef` / `TickRef` 新推荐路径：
  `wait_update()` + `try_load()`，或 `snapshot()` / `is_ready()`
- `load()` 为 legacy convenience：首帧前返回默认值
- `DataManager::watch_handle()` 优先于 `watch()` / `unwatch()`
```

在 `skills/tqsdk-rs/references/error-faq.md` 加：

```md
- `MarketNotInitialized`: 忘记 `init_market()`
- `ClientClosed`: 在已关闭 client 上继续使用 live 功能
- `SubscriptionClosed`: 在已关闭 quote/series watcher 上继续等待
- `TradeSessionNotConnected`: `TradeSession` 尚未 `connect()`
- `DataNotReady`: market ref 首帧尚未到达
```

- [ ] **Step 4: 验证文档同步是否命中关键字**

Run:

```bash
rg -n "try_quote|try_load|watch_handle|DataNotReady|ReplaySession::runtime" README.md AGENTS.md CLAUDE.md skills/tqsdk-rs/references
```

Expected:

- 每个关键字至少在一处 README / agent doc / skill reference 中出现
- 没有只改 README、忘改 agent docs 的情况

- [ ] **Step 5: Commit**

```bash
git add README.md AGENTS.md CLAUDE.md skills/tqsdk-rs/references/client-and-endpoints.md skills/tqsdk-rs/references/market-and-series.md skills/tqsdk-rs/references/error-faq.md
git commit -m "docs(api): document explicit readiness and watch handle guidance"
```

## Chunk 6: Full Verification

### Task 6: 运行完整验证矩阵并以 clean tree 收尾

**Files:**
- Verify: `src/errors.rs`
- Verify: `src/client/mod.rs`
- Verify: `src/client/facade.rs`
- Verify: `src/client/market.rs`
- Verify: `src/marketdata/mod.rs`
- Verify: `src/datamanager/mod.rs`
- Verify: `src/datamanager/watch.rs`
- Verify: `src/replay/session.rs`
- Verify: `README.md`
- Verify: `AGENTS.md`
- Verify: `CLAUDE.md`
- Verify: `skills/tqsdk-rs/references/client-and-endpoints.md`
- Verify: `skills/tqsdk-rs/references/market-and-series.md`
- Verify: `skills/tqsdk-rs/references/error-faq.md`

- [ ] **Step 1: 先跑完整测试**

Run:

```bash
cargo test
```

Expected:

- PASS
- 现有 293 个测试继续通过，新增 public API repair 测试一并通过
- 现有 live `#[ignore]` 测试保持 `ignored`

- [ ] **Step 2: 再跑严格 lint**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected:

- PASS
- 不出现 deprecated warning、unused import、doc note 相关噪音

- [ ] **Step 3: 如果你在执行中改了示例文件，再补跑 example 编译检查**

Run:

```bash
cargo check --examples
```

Expected:

- PASS
- 如果本计划严格按文件范围执行，通常可以不需要这一步

- [ ] **Step 4: 确认工作树干净，只剩计划外显式决定保留的变更**

Run:

```bash
git status --short
```

Expected:

- 空输出，或仅剩你明确决定延后处理的文件

- [ ] **Step 5: 收尾说明**

在最终交付说明里明确写出：

```text
- phase-1 仍然 non-breaking
- 现有 `load()` / `watch()` / `unwatch()` 还在
- 新推荐路径是 `try_load/is_ready/snapshot`、`watch_handle()`、稳定 account set 的 `ReplaySession::runtime(accounts)`
```

## Self-Review

- 本计划覆盖了 4 个已确认问题：typed state errors、watch ownership、replay runtime consistency、market ref explicit readiness
- 所有步骤都给出了具体文件、测试代码、运行命令与预期结果
- 本计划有意保持 phase-1 non-breaking，没有混入 breaking cleanup
- 若后续要做 breaking wave，再单独起一份 plan，把 `load()` -> `load_or_default()`、`watch()` -> `watch_handle()`、`unwatch()` 删除等动作放进去
