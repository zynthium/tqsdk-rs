# Public API Redesign Phase 1 Contract Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 以 non-breaking 方式完成 `0.2` public API redesign 的第一批 contract repair：补齐 `Client` 的显式 market precondition helper、补齐 `TradeSession` 的一等 readiness gate，并把 README / rustdoc / 迁移文档 / agent memory / skill refs 全部对齐到同一套 canonical 用法。

**Architecture:** 这一阶段只做 additive surface repair，不改四条主入口，也不改底层 transport / session ownership。`Client` 继续作为 live session owner，只是把已经存在的 “market 是否已初始化” 状态公开成显式 helper；`TradeSession` 继续沿用现有 `snapshot_epoch` / `wait_update()` 机制，只是在其上补一个 `wait_ready()` 封装，避免主路径继续 busy-poll `is_ready()`。

**Tech Stack:** Rust 2024, `tokio`, `thiserror`, crate rustdoc, Markdown docs, `cargo test`, `cargo check --examples`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo doc --no-deps`

---

## Scope Boundary

这份 RFC 太大，不能直接落成一张“全量 0.2”执行计划。本计划只覆盖第一批可独立落地、可单独验证、且对下游认知收益最大的 additive contract repair：

- `Client::market_is_initialized()` / `Client::check_market_initialized(...)`
- `TradeSession::wait_ready()`
- README / rustdoc / 示例 / 架构文档 / 迁移文档 / agent memory / `skills/tqsdk-rs/` 同步到新 contract

明确留到后续计划的内容：

- typed trading public model（`InsertOrderSpec` / enums）
- replay cheap snapshot/view API
- root / `prelude` contraction
- `EndpointConfig::default()` 改为 deterministic default

## File Map

- `src/client/mod.rs`: 补公开 market-init helper，并把现有私有检查复用到新 helper。
- `src/client/tests.rs`: 为 helper 增加 signature + inactive/closed 行为测试。
- `src/trade_session/ops.rs`: 新增 `wait_ready()`，并把 `connect()` / readiness rustdoc 改成一致叙事。
- `src/trade_session/tests.rs`: 为 `wait_ready()` 增加 not-connected、snapshot-complete、close-unblock 测试。
- `README.md`: 更新 canonical client / trade session 用法，去掉 `while !is_ready()` 主示例。
- `examples/trade.rs`: 用 `wait_ready()` 替换 busy-poll。
- `src/lib.rs`: crate-level quick start 与 rustdoc landing text 对齐新的 market helper / quote 读取路径。
- `docs/architecture.md`: 记录 `Client` market helper 与 `TradeSession::wait_ready()` 的 canonical contract。
- `docs/migration-remove-legacy-compat.md`: 把 `connect() + while !is_ready()` 降级为旧写法，并把 market ref 读取改成 `try_load()` 叙事。
- `AGENTS.md`, `CLAUDE.md`: 维护者执行规则同步。
- `skills/tqsdk-rs/SKILL.md`: 高层规则同步。
- `skills/tqsdk-rs/references/client-and-endpoints.md`: client/init/runtime 说明同步。
- `skills/tqsdk-rs/references/trading-and-orders.md`: trade readiness 说明同步。
- `skills/tqsdk-rs/references/tqsdk_rs_guide.md`: 主路径总览同步。
- `skills/tqsdk-rs/references/example-map.md`: 示例映射同步。
- `skills/tqsdk-rs/assets/main.rs`: 资产示例改成 `try_quote()` / `try_load()`。

### Task 1: Expose Client Market Preconditions

**Files:**
- Modify: `src/client/mod.rs`
- Modify: `src/client/tests.rs`

- [ ] **Step 1: Write the failing tests for the new helper surface**

```rust
#[test]
fn client_exposes_market_init_helpers() {
    let _market_is_initialized: fn(&Client) -> bool = Client::market_is_initialized;
    let _check_market_initialized: fn(&Client, &'static str) -> Result<()> = Client::check_market_initialized;
}

#[test]
fn market_init_helpers_report_inactive_state() {
    let client = build_inactive_client();

    assert!(!client.market_is_initialized());
    assert!(matches!(
        client.check_market_initialized("quote ref"),
        Err(TqError::MarketNotInitialized {
            capability: "quote ref"
        })
    ));
}

#[tokio::test]
async fn market_init_helpers_report_closed_state() {
    let client = build_client_with_market();
    assert!(client.market_is_initialized());

    client.close().await.unwrap();

    assert!(!client.market_is_initialized());
    assert!(matches!(
        client.check_market_initialized("quote ref"),
        Err(TqError::ClientClosed {
            capability: "quote ref"
        })
    ));
}
```

- [ ] **Step 2: Run the targeted client tests and confirm the surface is still missing**

Run:

```bash
cargo test market_init_helpers -- --nocapture
```

Expected:

- 编译失败，提示 `Client` 还没有 `market_is_initialized` / `check_market_initialized`

- [ ] **Step 3: Implement the minimal additive helper API**

```rust
impl Client {
    /// 当前 live market/query facade 是否已初始化且尚未关闭。
    pub fn market_is_initialized(&self) -> bool {
        self.live.is_active() && !self.live.market_state.is_closed()
    }

    /// 对依赖 live market 的能力做显式前置检查。
    pub fn check_market_initialized(&self, capability: &'static str) -> Result<()> {
        if self.live.market_state.is_closed() {
            return Err(TqError::client_closed(capability));
        }
        if !self.live.is_active() {
            return Err(TqError::market_not_initialized(capability));
        }
        Ok(())
    }

    fn ensure_market_ref_capability(&self, capability: &'static str) -> Result<()> {
        self.check_market_initialized(capability)
    }
}
```

- [ ] **Step 4: Re-run the targeted client tests**

Run:

```bash
cargo test market_init_helpers -- --nocapture
```

Expected:

- `client_exposes_market_init_helpers`
- `market_init_helpers_report_inactive_state`
- `market_init_helpers_report_closed_state`

全部通过。

- [ ] **Step 5: Commit the helper slice**

```bash
git add src/client/mod.rs src/client/tests.rs
git commit -m "feat: add client market init helpers"
```

### Task 2: Add TradeSession Readiness Gate

**Files:**
- Modify: `src/trade_session/ops.rs`
- Modify: `src/trade_session/tests.rs`

- [ ] **Step 1: Write failing tests for `TradeSession::wait_ready()`**

```rust
#[tokio::test]
async fn trade_session_wait_ready_before_connect_returns_not_connected() {
    let dm = build_dm();
    let session = build_session(dm);

    let err = session
        .wait_ready()
        .await
        .expect_err("wait_ready should fail before connect");
    assert!(matches!(err, TqError::TradeSessionNotConnected));
}

#[tokio::test]
async fn trade_session_wait_ready_blocks_until_trade_snapshot_ready() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.ws.force_missing_io_actor_for_test();
    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_ready().await })
    };
    tokio::task::yield_now().await;

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "session": {
                        "trading_day": "20260411"
                    },
                    "trade_more_data": true
                }
            }
        }),
        true,
        true,
    );

    sleep(Duration::from_millis(20)).await;
    assert!(
        !waiter.is_finished(),
        "wait_ready should keep waiting until the initial trade snapshot completes"
    );

    dm.merge_data(
        json!({
            "trade": {
                "u": {
                    "accounts": {
                        "CNY": account_json(1000.0, 900.0)
                    },
                    "trade_more_data": false
                }
            }
        }),
        true,
        true,
    );

    timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(session.is_ready());
    session.close().await.unwrap();
}

#[tokio::test]
async fn trade_session_wait_ready_unblocks_with_not_connected_after_close() {
    let dm = build_dm();
    let session = Arc::new(build_session(Arc::clone(&dm)));

    session.ws.force_missing_io_actor_for_test();
    session.running.store(true, Ordering::SeqCst);
    session.start_watching().await;

    let waiter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.wait_ready().await })
    };
    tokio::task::yield_now().await;

    session.close().await.unwrap();

    let err = timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .expect_err("wait_ready should stop waiting after close");
    assert!(matches!(err, TqError::TradeSessionNotConnected));
}
```

- [ ] **Step 2: Run the targeted trade-session tests and confirm the method is missing**

Run:

```bash
cargo test trade_session_wait_ready -- --nocapture
```

Expected:

- 编译失败，提示 `TradeSession` 还没有 `wait_ready`

- [ ] **Step 3: Implement `wait_ready()` on top of the existing epoch-driven watcher**

```rust
impl TradeSession {
    /// 连接交易服务器并启动登录/快照同步流程。
    ///
    /// 如需等到可以安全读取快照与发单的 ready state，调用 `wait_ready()`
    /// 而不是在主路径里 busy-poll `is_ready()`。
    pub async fn connect(&self) -> Result<()> {
        // 保持现有实现
    }

    /// 等待交易会话进入 ready 状态。
    ///
    /// ready 的定义保持不变：已登录、初始 trade snapshot 已完成、websocket 仍处于可用状态。
    pub async fn wait_ready(&self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(TqError::TradeSessionNotConnected);
        }

        if self.is_ready() {
            return Ok(());
        }

        loop {
            self.wait_update().await?;
            if self.is_ready() {
                return Ok(());
            }
        }
    }
}
```

- [ ] **Step 4: Re-run the targeted readiness tests**

Run:

```bash
cargo test trade_session_wait_ready -- --nocapture
```

Expected:

- `trade_session_wait_ready_before_connect_returns_not_connected`
- `trade_session_wait_ready_blocks_until_trade_snapshot_ready`
- `trade_session_wait_ready_unblocks_with_not_connected_after_close`

全部通过。

- [ ] **Step 5: Commit the readiness slice**

```bash
git add src/trade_session/ops.rs src/trade_session/tests.rs
git commit -m "feat: add trade session wait_ready"
```

### Task 3: Align Public Examples And Rustdoc

**Files:**
- Modify: `README.md`
- Modify: `examples/trade.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Locate stale canonical examples before editing**

Run:

```bash
rg -n "while !session\\.is_ready\\(|quote\\.load\\(|Client::builder\\(.*init_market" README.md examples/trade.rs src/lib.rs
```

Expected:

- 能定位到 README 和 `examples/trade.rs` 里仍在 busy-poll `is_ready()` 的片段
- 如有 `quote.load()` 主示例，也一并记录为本次替换点

- [ ] **Step 2: Update README to teach the new canonical path**

```markdown
> `Client::builder(...).build()` / `ClientBuilder::build()` 只负责认证与构造 session owner；live 行情能力在
> `client.init_market().await?` 后才可用。
> 如需显式检查 live market 前置条件，优先使用
> `client.market_is_initialized()` / `client.check_market_initialized("quote ref")`；
> 如需直接拿 fail-fast 的句柄，继续优先 `Client::{try_quote,try_kline_ref,try_tick_ref}`。
> 在 `init_market()` 之前调用 `Client::{wait_update,wait_update_and_drain}` 或
> `QuoteRef` / `KlineRef` / `TickRef` 的 `wait_update()`，会直接返回
> `MarketNotInitialized`；`quote()` / `kline_ref()` / `tick_ref()` 仍可提前取句柄，
> 但 canonical 用法是先初始化 market，再进入等待循环。
```

```rust
session.connect().await?;
session.wait_ready().await?;

session.wait_update().await?;
let account = session.get_account().await?;
let positions = session.get_positions().await?;
println!("权益={} 可用={}", account.balance, account.available);
println!("持仓数={}", positions.len());
```

```markdown
- `connect()` 只负责启动连接 / 登录 / 快照同步流程。
- `wait_ready()` 是进入“可安全读取快照与发单”状态的 canonical gate。
- `is_ready()` 保留为瞬时状态检查，不再作为主示例里的 busy-poll 入口。
```

- [ ] **Step 3: Update the compiled example and crate-level rustdoc**

```rust
// examples/trade.rs
info!("连接交易服务器...");
trader.connect().await.expect("连接失败");

info!("等待交易就绪...");
trader.wait_ready().await.expect("交易会话未就绪");
info!("✅ 已登录，交易就绪!");
```

```rust
//! // src/lib.rs
//! let mut client = Client::builder("username", "password")
//!     .config(ClientConfig::default())
//!     .endpoints(EndpointConfig::from_env())
//!     .build()
//!     .await?;
//!
//! client.init_market().await?;
//! client.check_market_initialized("quote ref")?;
//! let _quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
//! let quote = client.try_quote("SHFE.au2602")?;
//! let _snapshot = quote.try_load().await?;
```

- [ ] **Step 4: Verify public examples and rustdoc build**

Run:

```bash
cargo check --examples
cargo doc --no-deps
rg -n "while !session\\.is_ready\\(" README.md examples/trade.rs src/lib.rs
```

Expected:

- `cargo check --examples` 通过
- `cargo doc --no-deps` 通过
- `rg` 对 `while !session.is_ready()` 不再命中上述 public example/rustdoc 文件

- [ ] **Step 5: Commit the public-doc slice**

```bash
git add README.md examples/trade.rs src/lib.rs
git commit -m "docs: align public examples with readiness helpers"
```

### Task 4: Sync Maintainer Docs And Agent Skills

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/migration-remove-legacy-compat.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `skills/tqsdk-rs/SKILL.md`
- Modify: `skills/tqsdk-rs/references/client-and-endpoints.md`
- Modify: `skills/tqsdk-rs/references/trading-and-orders.md`
- Modify: `skills/tqsdk-rs/references/tqsdk_rs_guide.md`
- Modify: `skills/tqsdk-rs/references/example-map.md`
- Modify: `skills/tqsdk-rs/assets/main.rs`

- [ ] **Step 1: Update architecture and migration docs to reflect the repaired contract**

```markdown
| `client` | `Client`, `ClientBuilder`, `ClientConfig`, `ClientOption` | 统一 live session owner；`init_market()` 之后启用 market/query facade，`market_is_initialized()` / `check_market_initialized()` 提供显式前置检查 |
| `trade_session` | `TradeSession` | 显式 `connect()`，以 `wait_ready()` 进入 ready state，再通过 snapshot getter / reliable events 消费交易状态 |
```

```markdown
| `connect() + while !session.is_ready()` | `connect() + wait_ready()` | readiness gate 收口到一等 API，不再鼓励主路径 busy-poll |
| `quote_channel` / `on_quote` | `client.try_quote(symbol)?` + `wait_update()` / `try_load()` | Quote 是最新状态，不是事件日志；`load()` 仅保留 legacy convenience 语义 |
```

- [ ] **Step 2: Update maintainer memory (`AGENTS.md` / `CLAUDE.md`)**

```markdown
- market 显式前置检查优先 `Client::market_is_initialized()` / `Client::check_market_initialized("...")`；拿 fail-fast ref 仍可用 `try_quote()` / `try_kline_ref()` / `try_tick_ref()`。
- `TradeSession` 连接后的 canonical readiness gate 是 `connect()` + `wait_ready()`；`is_ready()` 仅作为瞬时状态检查，不再作为主示例 busy-poll 路径。
```

- [ ] **Step 3: Update `skills/tqsdk-rs/` references and example assets**

```markdown
// skills/tqsdk-rs/SKILL.md
2. `init_market()` 是 live market/query 能力的前置条件。需要显式 precondition check 时，优先
   `market_is_initialized()` / `check_market_initialized()`；拿 fail-fast ref 时用
   `try_quote()` / `try_kline_ref()` / `try_tick_ref()`。
5. `TradeSession` 连接后的 canonical readiness gate 是 `connect()` + `wait_ready()`；
   `is_ready()` 仅用于瞬时状态判断，不再作为主示例里的 busy-poll 路径。
```

```markdown
// skills/tqsdk-rs/references/client-and-endpoints.md
- 需要显式 market precondition 时，用 `client.market_is_initialized()` / `client.check_market_initialized("quote ref")`
- 需要直接拿 fail-fast 句柄时，用 `try_quote` / `try_kline_ref` / `try_tick_ref`
```

```rust
// skills/tqsdk-rs/references/trading-and-orders.md
session.connect().await?;
session.wait_ready().await?;
```

```markdown
// skills/tqsdk-rs/references/example-map.md
- `connect()` / `wait_ready()`
```

```markdown
// skills/tqsdk-rs/references/tqsdk_rs_guide.md
| live TradeSession | `create_trade_session()` -> `connect()` -> `wait_ready()` -> `wait_update()` | snapshot / reliable events 分层 |
```

```rust
// skills/tqsdk-rs/assets/main.rs
let _quote_sub = client.subscribe_quote(&[symbol]).await?;
let quote = client.try_quote(symbol)?;

let updates = match tokio::time::timeout(remaining, client.wait_update_and_drain()).await {
    Ok(result) => result?,
    Err(_) => break,
};

if updates.quotes.iter().any(|item| item.as_str() == symbol) {
    let q = quote.try_load().await?;
    println!("{} = {}", q.instrument_id, q.last_price);
}
```

- [ ] **Step 4: Run text-level consistency checks across maintainer docs and skill refs**

Run:

```bash
rg -n "while !session\\.is_ready\\(|quote\\.load\\(" AGENTS.md CLAUDE.md docs/architecture.md docs/migration-remove-legacy-compat.md skills/tqsdk-rs
rg -n "wait_ready|market_is_initialized|check_market_initialized" AGENTS.md CLAUDE.md docs/architecture.md docs/migration-remove-legacy-compat.md skills/tqsdk-rs
```

Expected:

- 第一条 `rg` 不再命中 canonical example / skill 资产中的旧 busy-poll 与 `quote.load()` 写法
- 第二条 `rg` 能稳定命中新 helper / 新 readiness gate 的说明

- [ ] **Step 5: Commit the maintainer-doc and skill sync slice**

```bash
git add docs/architecture.md docs/migration-remove-legacy-compat.md AGENTS.md CLAUDE.md skills/tqsdk-rs
git commit -m "docs: sync maintainer guidance for contract repair"
```

### Task 5: Run The Release-Gate Verification Matrix

**Files:**
- Modify: none
- Test: `src/client/tests.rs`
- Test: `src/trade_session/tests.rs`

- [ ] **Step 1: Run the full unit/integration test suite**

Run:

```bash
cargo test
```

Expected:

- 全部测试通过；尤其是 `src/client/tests.rs` 与 `src/trade_session/tests.rs` 无回归

- [ ] **Step 2: Re-check examples after the doc/example updates**

Run:

```bash
cargo check --examples
```

Expected:

- 所有示例通过编译；`examples/trade.rs` 不再依赖 busy-poll readiness

- [ ] **Step 3: Run the strict clippy gate for public API changes**

Run:

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected:

- 无 lint 警告或错误

- [ ] **Step 4: Rebuild rustdoc**

Run:

```bash
cargo doc --no-deps
```

Expected:

- rustdoc 通过，crate-level docs 与新增 public methods 的注释都能正常生成

- [ ] **Step 5: Confirm the branch is clean after the planned commits**

Run:

```bash
git status --short
```

Expected:

- 无未提交改动；如果仍有改动，说明前面某个任务遗漏了文件或验证修复

## Spec Coverage Check

- `Client` market-init helper additive contract：Task 1
- `TradeSession` 一等 readiness gate：Task 2
- README / 示例 / rustdoc 对齐：Task 3
- 架构文档 / 迁移文档 / maintainer memory / skill refs 同步：Task 4
- release-gate verification：Task 5

## Deferred Follow-Up Plans

- typed trading public model migration
- replay cheap snapshot/view APIs
- root / `prelude` contraction
- deterministic `EndpointConfig::default()`
