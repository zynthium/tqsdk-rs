# AGENTS.md — tqsdk-rs

## 项目概述

`tqsdk-rs` 是天勤 DIFF 协议的 Rust SDK，连接 Shinnytech 量化交易平台，提供以下能力：

- 实时行情订阅：Quote、K 线、Tick、交易状态
- 历史与基础数据：合约查询、结算价、持仓排名、EDB、交易日历
- 交易与回测：实盘/模拟交易、回测时间推进
- 可选数据分析：`polars` DataFrame 转换

这是一个单 crate Rust 项目，不是 workspace。主入口在 `src/lib.rs`。

## 技术栈

- Rust 2024 edition
- `tokio` 异步运行时
- `yawc` WebSocket
- `reqwest` HTTP
- `serde` / `serde_json`
- `jsonwebtoken`
- `tracing` / `tracing-subscriber`
- `thiserror`
- 可选 feature：`polars`

## 目录与职责

```text
src/
├── auth/           认证、token、权限检查
├── websocket/      WebSocket 连接、背压、重连、消息分发
├── datamanager/    DIFF 合并、查询、监听
├── cache/          本地 K 线磁盘缓存（分段写入/压缩/区间读取）
├── quote/          行情订阅
├── series/         K 线/Tick 订阅与处理
├── ins/            合约与基础数据查询
├── trade_session/  交易会话与交易操作
├── replay/         回放内核、回测 session、仿真撮合
├── runtime/        目标持仓运行时与任务调度
├── polars_ext/     DataFrame 转换（feature = "polars"）
├── types/          各类数据结构
├── prelude.rs      常用 re-export
├── logger.rs       日志初始化
├── errors.rs       错误类型
└── utils.rs        通用工具

examples/
├── quote.rs
├── history.rs
├── trade.rs
├── backtest.rs
├── pivot_point.rs
├── datamanager.rs
├── option_levels.rs
├── custom_logger.rs
└── marketdata_state_stress.rs
```

## 核心架构提示

- `Client` / `ClientBuilder` 是统一入口，优先从这里开始理解调用路径。
- WebSocket 层使用 I/O actor 模式，避免跨 `await` 持锁。
- `ReplaySession` 是唯一推荐的公开回测入口；不要为新代码重新引入 `BacktestHandle` 风格的 facade。
- `DataManager` 是 DIFF 协议状态中心，很多上层行为都依赖其 merge/watch/query 语义。
- `DataManager` 的 merge 通知统一使用 `subscribe_epoch()`；旧的 `on_data` / `on_data_register` / `off_data` callback plumbing 已删除，不要重新引入。
- breaking target：live API 继续收口到 `Client` 单入口；`tqapi()` 已删除，`series()` / `ins()` 也不应再扩大使用面。
- `QuoteSubscription` 和 `SeriesSubscription` 已经是 auto-start；创建后立即生效，`close()` 只负责提前释放资源。
- `QuoteSubscription` 只负责订阅生命周期；读取 Quote 的长期目标是 `Client::quote(symbol)`，不要重新引入 `on_quote` / `quote_channel` 一类 fan-out API。
- `SeriesSubscription` 的 canonical 消费方式是 `wait_update()` / `snapshot()` / `load()`；不要重新引入 `on_update`、`on_new_bar`、`data_stream()` 一类 fan-out API。
- `TqRuntime` 的目标持仓 canonical 入口是 `runtime.account(\"...\").target_pos(...).build()` 和 scheduler builder；`compat::` facade 已删除，不要重新引入。
- `TradeSession` 对外分为两层：账户/持仓等仍以最新状态读取为主，`order` / `trade` 则使用可靠事件流 API。
- `TradeSession` 的通知与异步错误已经并入可靠事件流；账户/持仓 callback/channel 已不再是推荐路径，不要为新代码新增依赖。
- `TradeSession` 内部 watcher 已改为监听 DataManager epoch，再按 path epoch 拆分账户快照与可靠订单/成交事件。
- `subscribe_order_events()` / `subscribe_trade_events()` 是按类型过滤的可靠事件视图；它们不应再因为通知或异步错误事件共享 retention 窗口而误报 `Lagged`。
- 重连逻辑包含完整性校验，不要为了“简化”而破坏临时缓冲再合并的策略。
- 行情与非关键状态更新存在有界缓冲和丢弃策略；但 `TradeSession` 的 `order` / `trade` 公开接口不再走 best-effort channel。

## 环境准备

### 本地开发

- 构建：`cargo build`
- 运行测试：`cargo test`
- 严格 lint：`cargo clippy --all-targets --all-features -- -D warnings`
- 检查格式：`cargo fmt --check`
- 启用 Polars：`cargo build --features polars`

### 常用环境变量

这些变量只在需要联网、登录或运行示例时才需要：

| 变量 | 用途 |
|------|------|
| `TQ_AUTH_USER` | 天勤账号 |
| `TQ_AUTH_PASS` | 天勤密码 |
| `TQ_LOG_LEVEL` | 示例中的日志级别 |
| `TQ_QUOTE_AU` | `quote` 示例中的第一个行情合约 |
| `TQ_QUOTE_AG` | `quote` 示例中的第二个行情合约 |
| `TQ_QUOTE_M` | `quote` 示例中的第三个行情合约 |
| `TQ_TEST_SYMBOL` | live 查询示例/测试使用的合约 |
| `TQ_LEFT_KLINE_ID` | `history` 示例按左边界定位历史窗口；支持整数或 `auto`，未设置时自动聚焦最近窗口 |
| `TQ_HISTORY_VIEW_WIDTH` | `history` 示例历史窗口宽度，默认 `8000` |
| `TQ_HISTORY_FOCUS_POSITION` | `history` 示例按时间焦点定位时的窗口偏移，默认 `0` |
| `TQ_START_DT` | `backtest` 示例起始日期 |
| `TQ_END_DT` | `backtest` 示例结束日期 |
| `TQ_POSITION_SIZE` | `backtest` / `pivot_point` 示例使用的目标手数 |
| `SIMNOW_USER_0` | `trade` 示例的 SimNow 账号 |
| `SIMNOW_PASS_0` | `trade` 示例的 SimNow 密码 |
| `TQ_TRADE_EXAMPLE_DURATION_SECS` | `trade` 示例持续运行秒数，默认 `300` |
| `TQ_UNDERLYING` | `option_levels` 示例的标的合约 |

还有少量高级环境变量会覆盖默认服务地址，例如 `TQ_AUTH_URL`、`TQ_MD_URL`、`TQ_TD_URL`、`TQ_INS_URL`、`TQ_CHINESE_HOLIDAY_URL`。只有在排查网络、联调特殊服务或切换服务端点时才应修改。

## 验证策略

按改动范围选择验证，不要机械地只跑一条命令。

### 安全的本地验证

这些命令不应触达真实交易：

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo build --features polars`

### 建议的最小验证矩阵

- 只改文档：检查改动内容是否与仓库现状一致
- 只改纯逻辑模块：`cargo test`
- 改公开 API、trait、类型定义、模块导出：`cargo test` + `cargo clippy --all-targets --all-features -- -D warnings`
- 改 `polars_ext/`：额外运行 `cargo build --features polars`
- 改示例：至少运行 `cargo check --examples`
- 改 `websocket/`、`datamanager/`、`quote/`、`series/`、`trade_session/` 这类核心链路：优先补/改对应单元测试，再跑完整 `cargo test`

### 联网 / live 验证

以下内容可能联网，甚至接近真实交易环境：

- 被 `#[ignore]` 标记的 live 测试，尤其是 `src/ins/tests.rs` 中依赖 `TQ_AUTH_USER` / `TQ_AUTH_PASS` 的测试
- `cargo run --example history`
- `cargo run --example quote`
- `cargo run --example option_levels`
- `cargo run --example backtest`
- `cargo run --example trade`

执行原则：

- 未经明确授权，不要运行 live 测试或任何需要账号的示例。
- 未经明确授权，不要运行 `trade` 示例。
- 即使是模拟盘，也视为高风险外部操作。
- 如果只是验证编译，请优先使用 `cargo check --examples`，不要直接运行示例。

## 修改代码时的工作准则

- 优先保持现有架构边界，不要把高层策略逻辑塞回底层传输层。
- 修改公开 API 时，同时检查 `README.md`、`AGENTS.md`、示例代码、`prelude` re-export 是否需要同步。
- 如果这次改动删除或替换 public surface，同时更新 `docs/migration-remove-legacy-compat.md`。
- 如果这次改动涉及 live API，优先把入口收口到 `Client`，不要继续把新能力挂到 `TqApi` / `SeriesAPI` / `InsAPI` 上。
- 修改 `DataManager` merge/query/watch 语义时，要特别关注向后兼容性；这部分会影响多个上层模块。
- 修改 `DataManager` 通知路径时，优先沿用 `subscribe_epoch()` + `get_path_epoch()` 的状态驱动模式，不要为新逻辑新增 callback plumbing。
- 修改重连、背压、消息队列时，要明确说明是否改变了丢弃策略、顺序语义或完整性保证。
- `TradeSession` 的 `order` / `trade` 事件以可靠事件流为 canonical API；不要新增或恢复 `on_order`、`on_trade`、`order_channel`、`trade_channel` 这类 best-effort public surface。
- 不要为新代码新增或恢复 `Client::tqapi()` / `Client::series()` / `Client::ins()` 依赖、`QuoteSubscription::start()` / `SeriesSubscription::start()` 依赖、以及 `TradeSession` 的 snapshot callback/channel 依赖。
- 新增错误类型时，优先复用 `TqError`，保持错误边界集中。
- 示例代码是对外接口的一部分；如果 API 变了，示例必须同步更新。

## 测试与示例约定

- 单元测试主要分布在对应模块目录和 `src/*/tests.rs`。
- 部分 `ins` 相关测试是 live 测试，默认被 `#[ignore]` 跳过。
- `examples/custom_logger.rs` 会创建 `logs/` 目录；不要把这类产物加入版本控制。
- 示例默认以“可读、可演示”为目标，不要为局部技巧牺牲 API 清晰度。

## 安全边界

- 不要在代码、测试、示例、文档中硬编码真实账号、密码、token 或服务器地址凭据。
- 不要提交本地联调凭据或日志产物。
- 不要擅自修改默认服务端点、认证流程或权限检查逻辑，除非任务明确要求。
- 不要因为方便调试而绕过权限检查、重连完整性校验或背压边界。

## 提交前检查

除非任务明确仅修改文档，否则提交前至少确认：

- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`

有以下情况时追加检查：

- 涉及格式化或宏展开相关改动：`cargo fmt --check`
- 涉及 `polars_ext/`：`cargo build --features polars`
- 涉及示例或 README 中的命令：至少确认命令与当前代码一致

## 给 Agent 的建议阅读顺序

当任务不明确时，按这个顺序建立上下文：

1. `README.md`
2. `Cargo.toml`
3. `src/lib.rs`
4. 与任务直接相关的模块
5. 对应模块的测试
6. 相关示例

如果任务涉及变更 API 或架构，先检查 README、示例和测试是否会一起受影响，再开始修改。

## 附录：架构速查

详细架构说明已抽出到 [docs/architecture.md](docs/architecture.md)。

当你需要快速判断模块边界、调用层级、关键设计模式或某个改动的影响面时，直接阅读这份文档。
当它与上面的可执行规则冲突时，以上面的操作规范为准。
