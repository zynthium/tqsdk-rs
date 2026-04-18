# Python History API Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Rust live 序列 API 破坏式迁移到 Python 对齐命名：`get_*_serial` 表示最近最多 `10000` 条的更新序列，`get_*_data_series` 保持显式时间范围下载，示例和文档不再把下载接口讲成普通“历史序列”。

**Architecture:** 复用当前 `SeriesSubscription` 和 `normalize_data_length()` 的实现，不改底层协议、不改 `tq_dl` 权限边界，只替换 public naming 和文档叙事。`Client` 与公开的 `SeriesAPI` 一起切到 `get_kline_serial` / `get_tick_serial`，旧的 `kline` / `tick` 在同一次改动中直接删除，不保留 shim。

**Tech Stack:** Rust 2024, `tokio`, existing `Client`, `SeriesAPI`, `SeriesSubscription`, `README`/migration docs, `cargo test`, `cargo check --examples`, `cargo clippy`.

---

## Scope Locks

- 这是 breaking migration，不保留 `Client::kline` / `Client::tick` 或 `SeriesAPI::kline` / `SeriesAPI::tick` 兼容别名。
- `ReplaySession::{kline,tick,aligned_kline}` 不在本次改动范围内，避免把 replay 命名一并卷进来。
- `get_kline_data_series` / `get_tick_data_series` 的 `tq_dl` 语义、缓存逻辑、分页抓取逻辑保持不变。
- `DataDownloader` 保持现状，只在文档和示例地图里重新归类为显式下载工作流。

## File Map

- `src/client/facade.rs`
  `Client` 的 live 序列公开入口；这里会把 `kline` / `tick` 替换成 `get_kline_serial` / `get_tick_serial`。
- `src/client/tests.rs`
  facade 级回归测试，覆盖新命名、关闭后失效、直接暴露 `SeriesSubscription`。
- `src/series/api.rs`
  公开 `SeriesAPI` 命名切换的核心实现；复用现有 serial 逻辑，不改 data series 下载路径。
- `src/series/mod.rs`
  模块级文档说明，明确 serial vs data_series 分工。
- `src/series/tests.rs`
  覆盖 `SeriesAPI` 新方法暴露、auto-start 行为、`data_length <= 10000` 归一化。
- `examples/history.rs`
  改成 bounded serial 示例，不再触发 `tq_dl` 下载权限。
- `examples/quote.rs`
  改用 `client.get_kline_serial(...)` / `client.get_tick_serial(...)`。
- `examples/data_series.rs`
  新增显式下载示例，承接旧 `history.rs` 的 one-shot 下载演示。
- `README.md`
  改写 canonical API 说明、示例表、命令说明和代码片段。
- `docs/architecture.md`
  重新表述 Series 部分的 canonical split。
- `docs/migration-remove-legacy-compat.md`
  加入 breaking rename 说明，明确无 shim。
- `docs/migration-marketdata-state.md`
  把状态驱动迁移文档中的 live 序列入口换成 `get_*_serial`。
- `AGENTS.md`
  更新仓库级 agent 规则，防止后续继续生成旧 API。
- `CLAUDE.md`
  更新 Anthropic 记忆中的 canonical serial API。
- `skills/tqsdk-rs/SKILL.md`
  更新 skill 主说明里的 canonical live API。
- `skills/tqsdk-rs/references/client-and-endpoints.md`
  更新 init_market 后的 live market 入口提示。
- `skills/tqsdk-rs/references/market-and-series.md`
  更新 live serial vs data_series split 和代码示例。
- `skills/tqsdk-rs/references/example-map.md`
  更新 `history.rs` / `data_series.rs` 映射。
- `skills/tqsdk-rs/references/error-faq.md`
  更新“未初始化”和“权限”章节中的方法名。
- `skills/tqsdk-rs/references/tqsdk_rs_guide.md`
  更新 API 地图里的 serial/data_series 术语。

### Task 1: Freeze The Breaking Serial API In Tests

**Files:**
- Modify: `src/client/tests.rs`
- Modify: `src/series/tests.rs`

- [ ] **Step 1: Rewrite the facade tests to the new names**

```rust
#[tokio::test]
async fn client_exposes_serial_subscriptions_directly() {
    let client = build_client_with_market();

    let kline = client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await;
    let tick = client.get_tick_serial("SHFE.au2602", 64).await;

    assert!(kline.is_ok());
    assert!(tick.is_ok());
}

#[tokio::test]
async fn close_invalidates_market_interfaces() {
    let client = build_client_with_market();
    client.close().await.unwrap();

    assert!(client.query_quotes(None, None, None, None, None).await.is_err());
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
    assert!(client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await
        .is_err());
    assert!(client.get_tick_serial("SHFE.au2602", 64).await.is_err());
}

#[tokio::test]
async fn create_backtest_session_does_not_activate_client_market_state() {
    let client = Client::builder("tester", "secret")
        .auth(TestAuth)
        .build()
        .await
        .expect("test client should build without network");

    let start = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
    let end = chrono::DateTime::<chrono::Utc>::from_timestamp(60, 0).unwrap();
    let _session = client
        .create_backtest_session(crate::ReplayConfig::new(start, end).unwrap())
        .await
        .expect("creating replay session should not initialize live market state");

    assert!(!client.market_active.load(std::sync::atomic::Ordering::SeqCst));
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
    assert!(client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await
        .is_err());
}
```

- [ ] **Step 2: Rewrite the public `SeriesAPI` shape tests and add a width-cap regression**

```rust
#[test]
fn series_api_should_expose_get_serial_methods() {
    fn _get_kline_serial<'a>(
        api: &'a SeriesAPI,
        symbols: &'a str,
        duration: StdDuration,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.get_kline_serial(symbols, duration, data_length))
    }

    fn _get_tick_serial<'a>(
        api: &'a SeriesAPI,
        symbol: &'a str,
        data_length: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<SeriesSubscription>>> + Send + 'a>> {
        Box::pin(api.get_tick_serial(symbol, data_length))
    }

    let _ = (_get_kline_serial, _get_tick_serial);
}

#[tokio::test]
async fn get_kline_serial_returns_started_subscription() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(dm, ws, auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 32)
        .await
        .unwrap();

    assert!(*sub.running.read().await);
    assert!(sub.watch_task_active_for_test());
    assert!(sub.snapshot().await.is_none());
    sub.close().await.unwrap();
}

#[tokio::test]
async fn get_kline_serial_creation_failure_rolls_back_series_watch_task() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(Arc::clone(&dm), Arc::clone(&ws), auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 32)
        .await;

    assert!(sub.is_err());
}

#[tokio::test]
async fn get_kline_serial_caps_data_length_at_10000() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth::default()));
    let api = SeriesAPI::new(dm, ws, auth);

    let sub = api
        .get_kline_serial("SHFE.au2602", StdDuration::from_secs(60), 20_000)
        .await
        .unwrap();

    assert_eq!(sub.options.view_width, 10_000);
    sub.close().await.unwrap();
}
```

- [ ] **Step 3: Run the focused tests to prove the rename is still missing**

Run: `cargo test client_exposes_serial_subscriptions_directly -- --nocapture`
Expected: FAIL with `no method named 'get_kline_serial' found for struct 'client::Client'`

Run: `cargo test series_api_should_expose_get_serial_methods -- --nocapture`
Expected: FAIL with `no method named 'get_kline_serial' found for reference '&SeriesAPI'`

- [ ] **Step 4: Commit the red test state**

```bash
git add src/client/tests.rs src/series/tests.rs
git commit -m "test: lock python-style serial api rename"
```

### Task 2: Rename The Public Rust Serial APIs

**Files:**
- Modify: `src/client/facade.rs`
- Modify: `src/series/api.rs`
- Modify: `src/series/mod.rs`

- [ ] **Step 1: Replace the public `SeriesAPI` methods and delete the old names**

```rust
/// 获取 K 线 serial 订阅（单合约/多合约对齐），对齐 tqsdk-python 的 `get_kline_serial`。
pub async fn get_kline_serial<T>(
    &self,
    symbols: T,
    duration: StdDuration,
    data_length: usize,
) -> Result<Arc<SeriesSubscription>>
where
    T: Into<KlineSymbols>,
{
    let symbols = symbols.into().into_vec();
    let options = build_realtime_kline_options(symbols, duration, data_length)?;
    self.subscribe(options).await
}

/// 获取 Tick serial 订阅，对齐 tqsdk-python 的 `get_tick_serial`。
pub async fn get_tick_serial(
    &self,
    symbol: &str,
    data_length: usize,
) -> Result<Arc<SeriesSubscription>> {
    if symbol.is_empty() {
        return Err(TqError::InvalidParameter("symbol 不能为空字符串".to_string()));
    }
    let view_width = normalize_data_length(data_length)?;
    self.subscribe(SeriesOptions {
        symbols: vec![symbol.to_string()],
        duration: 0,
        view_width,
        chart_id: None,
        left_kline_id: None,
        focus_datetime: None,
        focus_position: None,
    })
    .await
}
```

- [ ] **Step 2: Replace the public `Client` wrappers and keep data series unchanged**

```rust
/// 订阅 K 线 serial（单合约/多合约对齐）。
pub async fn get_kline_serial<T>(
    &self,
    symbols: T,
    duration: StdDuration,
    data_length: usize,
) -> Result<Arc<SeriesSubscription>>
where
    T: Into<KlineSymbols>,
{
    self.series_api()?
        .get_kline_serial(symbols, duration, data_length)
        .await
}

/// 订阅 Tick serial。
pub async fn get_tick_serial(
    &self,
    symbol: &str,
    data_length: usize,
) -> Result<Arc<SeriesSubscription>> {
    self.series_api()?.get_tick_serial(symbol, data_length).await
}

/// 一次性获取按时间窗口的历史 K 线快照（不随行情更新），语义为 `[start_dt, end_dt)`。
pub async fn get_kline_data_series(
    &self,
    symbol: &str,
    duration: StdDuration,
    start_dt: DateTime<Utc>,
    end_dt: DateTime<Utc>,
) -> Result<Vec<Kline>> {
    self.series_api()?
        .kline_data_series(symbol, duration, start_dt, end_dt)
        .await
}
```

- [ ] **Step 3: Update the module docs so the split is visible at the module boundary**

```rust
//! 提供 K 线与 Tick 序列能力，支持：
//! - Python 对齐的 bounded serial 订阅（`get_kline_serial` / `get_tick_serial`）
//! - 一次性历史 K 线 / Tick 下载（`*_data_series`）
//! - 快照式窗口状态读取
```

- [ ] **Step 4: Run the focused tests again**

Run: `cargo test client_exposes_serial_subscriptions_directly -- --nocapture`
Expected: PASS

Run: `cargo test get_kline_serial_caps_data_length_at_10000 -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit the public API rename**

```bash
git add src/client/facade.rs src/series/api.rs src/series/mod.rs src/client/tests.rs src/series/tests.rs
git commit -m "feat: align serial api names with python sdk"
```

### Task 3: Rewrite The Examples Around Serial vs Download

**Files:**
- Modify: `examples/history.rs`
- Modify: `examples/quote.rs`
- Create: `examples/data_series.rs`

- [ ] **Step 1: Rewrite `examples/history.rs` to use bounded serial semantics**

```rust
fn derive_data_length(bar_secs: u64, lookback_minutes: i64) -> usize {
    let lookback_secs = (lookback_minutes.max(1) as u64) * 60;
    let bars = (lookback_secs + bar_secs.saturating_sub(1)) / bar_secs.max(1);
    bars.clamp(1, 10_000) as usize
}

let data_length = derive_data_length(bar_secs, lookback_minutes);

let sub = client
    .get_kline_serial(symbol.as_str(), Duration::from_secs(bar_secs), data_length)
    .await?;

loop {
    let snapshot = sub.wait_update().await?;
    if snapshot.update.chart_ready {
        break;
    }
}

let window = sub.load().await?;
let series = window
    .get_symbol_klines(symbol.as_str())
    .expect("single-symbol serial should exist");

info!(
    "K 线 serial 准备完成 symbol={} duration={}s count={} capped_at=10000",
    symbol,
    bar_secs,
    series.data.len()
);
if let (Some(first), Some(last)) = (series.data.first(), series.data.last()) {
    info!(
        "样本 first_id={} first_dt={} last_id={} last_dt={} last_close={}",
        first.id,
        first.datetime,
        last.id,
        last.datetime,
        last.close
    );
}
```

- [ ] **Step 2: Rewrite `examples/quote.rs` to the new names**

```rust
let kline_sub = client
    .get_kline_serial(au.as_str(), kline_duration, 256)
    .await?;

let tick_sub = client.get_tick_serial(au.as_str(), 256).await?;
```

- [ ] **Step 3: Add `examples/data_series.rs` as the explicit download example**

```rust
let rows = client
    .get_kline_data_series(symbol.as_str(), Duration::from_secs(bar_secs), start_dt, end_dt)
    .await?;

info!(
    "历史 K 线 data_series 下载完成 symbol={} duration={}s count={}",
    symbol,
    bar_secs,
    rows.len()
);
```

- [ ] **Step 4: Compile-check each affected example before broad verification**

Run: `cargo check --example history`
Expected: PASS

Run: `cargo check --example quote`
Expected: PASS

Run: `cargo check --example data_series`
Expected: PASS

- [ ] **Step 5: Commit the example migration**

```bash
git add examples/history.rs examples/quote.rs examples/data_series.rs
git commit -m "refactor: split serial and data-series examples"
```

### Task 4: Sync README, Migration Docs, Agent Memory, And Skill Pack

**Files:**
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `docs/migration-marketdata-state.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `skills/tqsdk-rs/SKILL.md`
- Modify: `skills/tqsdk-rs/references/client-and-endpoints.md`
- Modify: `skills/tqsdk-rs/references/market-and-series.md`
- Modify: `skills/tqsdk-rs/references/example-map.md`
- Modify: `skills/tqsdk-rs/references/error-faq.md`
- Modify: `skills/tqsdk-rs/references/tqsdk_rs_guide.md`

- [ ] **Step 1: Rewrite the canonical README wording and example table**

```markdown
- Series：`Client::{get_kline_serial,get_tick_serial}` 负责发起 Python 对齐的 bounded serial 订阅，`Client::{get_kline_data_series,get_tick_data_series}` 负责一次性时间范围下载，`Client::{kline_ref,tick_ref}` 负责读取 latest bar/tick，`SeriesSubscription` 负责多合约对齐窗口并通过 `wait_update()` / `snapshot()` / `load()` 暴露快照。
- Downloader：`Client::spawn_data_downloader()` 负责后台历史下载与 CSV 导出工作流。

# 最近 `10000` 根以内 serial 与接口联调
cargo run --example history

# 显式时间范围历史下载
cargo run --example data_series

| `history.rs` | 最近 `10000` 根以内的 K 线 serial、接口联调 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_TEST_SYMBOL`、`TQ_HISTORY_BAR_SECONDS`、`TQ_HISTORY_LOOKBACK_MINUTES` |
| `data_series.rs` | 一次性时间范围历史 K 线下载 | `TQ_AUTH_USER`、`TQ_AUTH_PASS`，可选 `TQ_TEST_SYMBOL`、`TQ_HISTORY_BAR_SECONDS`、`TQ_HISTORY_LOOKBACK_MINUTES`；要求账户具备 `tq_dl` 历史下载权限 |

- 先运行 `cargo run --example quote` 验证认证和行情链路；如果需要验证历史下载权限，再运行 `cargo run --example data_series`。
```

- [ ] **Step 2: Update the architecture and migration docs to call out the breaking rename**

```markdown
- Series serial：`Client::{get_kline_serial,get_tick_serial}` 走更新中的 bounded sequence 路径，`data_length` 归一化到 `1..=10000`。
- 历史下载：`Client::{get_kline_data_series,get_tick_data_series}` 走 one-shot 下载路径，语义为 `[start_dt, end_dt)`，并在入口检查 `tq_dl`。

| 旧 API | 新 API | 说明 |
| --- | --- | --- |
| `Client::{kline,tick}` | `Client::{get_kline_serial,get_tick_serial}` | breaking rename；无 deprecated shim |
```

- [ ] **Step 3: Update agent memory so future assistants stop generating the old names**

```markdown
- `SeriesSubscription` 的 canonical 发起入口应使用 `Client::{get_kline_serial,get_tick_serial}`。
- `Client::{get_kline_data_series,get_tick_data_series}` 是显式时间范围下载接口，不要把它们讲成普通“历史窗口”。
- `cargo run --example history` 现在是 bounded serial 示例；显式下载示例改为 `cargo run --example data_series`。
```

```markdown
2. `init_market()` 是 live market/query 能力的前置条件。没初始化就不要推荐 `subscribe_quote()`、`get_kline_serial()`、`get_tick_serial()`、`query_*()`。

- `Client::get_kline_serial()`, `Client::get_tick_serial()`, `Client::kline_ref()`, `Client::tick_ref()`
- `Client::get_kline_data_series()`, `Client::get_tick_data_series()`
```

- [ ] **Step 4: Sweep the skill reference pack for stale `Client::kline()` / `Client::tick()` guidance**

Run: `rg -n "Client::kline\\(|Client::tick\\(|client\\.kline\\(|client\\.tick\\(" README.md docs AGENTS.md CLAUDE.md skills/tqsdk-rs`
Expected: only replay-specific or archived/spec references remain; no canonical live `Client::kline` / `Client::tick` mentions

- [ ] **Step 5: Commit the docs and memory sync**

```bash
git add README.md docs/architecture.md docs/migration-remove-legacy-compat.md docs/migration-marketdata-state.md AGENTS.md CLAUDE.md skills/tqsdk-rs
git commit -m "docs: align history terminology with python serial api"
```

### Task 5: Run Final Verification And Cut The Branch Cleanly

**Files:**
- Modify: none

- [ ] **Step 1: Format the tree**

Run: `cargo fmt`
Expected: exits `0`

- [ ] **Step 2: Run the full test suite**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: Verify examples still compile**

Run: `cargo check --examples`
Expected: PASS

- [ ] **Step 4: Run strict clippy on the final public surface**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 5: Prove the old live names are gone**

Run: `rg -n "\\bpub async fn (kline|tick)\\b|client\\.kline\\(|client\\.tick\\(|api\\.kline\\(|api\\.tick\\(" src examples README.md docs AGENTS.md CLAUDE.md skills/tqsdk-rs --glob '!docs/superpowers/specs/*' --glob '!docs/superpowers/plans/*' --glob '!docs/archive/*'`
Expected: no matches for removed live market-data API names

- [ ] **Step 6: Commit the verified migration**

```bash
git add -A
git commit -m "feat: adopt python-style serial history api"
```

## Recommended Execution Order

1. Task 1 first, because it locks the breaking API in tests before any production rename happens.
2. Task 2 second, because `Client` and `SeriesAPI` must switch together in the same slice.
3. Task 3 third, because examples should compile against the new API before docs are rewritten around it.
4. Task 4 fourth, because canonical wording must be updated everywhere the SDK teaches future users or agents.
5. Task 5 last, because final verification should run on the complete migration rather than on partial slices.
