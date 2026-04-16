# AGENTS.md — tqsdk-rs

## 作用域与优先级

- 作用域：本文件适用于当前仓库根目录及其所有子目录。
- 如果某个子目录后续出现更深层的 `AGENTS.md`，则该子树以更近的文件为准。
- 用户的显式指令优先于本文件。
- 把本文件当作仓库级执行说明；公开用法看 `README.md`，模块边界看 `docs/architecture.md`，具体行为以相关测试和示例为准。

## 项目速写

- `tqsdk-rs` 是 Shinnytech 天勤 DIFF 协议的 Rust SDK。
- 这是一个单 crate Rust 2024 项目，不是 workspace。
- 主入口在 `src/lib.rs`。
- 核心能力：实时 Quote / K 线 / Tick、合约与基础数据查询、交易会话、回放/回测、可选 `polars` DataFrame 转换。
- 关键技术：`tokio`、`yawc`、`reqwest`、`serde`、`jsonwebtoken`、`tracing`、`thiserror`。

## 目录速查

```text
src/
├── auth/           认证、token 解析、权限检查
├── websocket/      WebSocket 传输、背压、重连、消息分发
├── datamanager/    DIFF merge/query/watch 状态中心
├── cache/          本地 K 线缓存
├── quote/          Quote 订阅生命周期
├── series/         K 线 / Tick 订阅与快照
├── ins/            合约与基础数据查询
├── trade_session/  交易会话与订单/成交链路
├── replay/         回放内核与回测 session
├── runtime/        目标持仓运行时与 scheduler
├── polars_ext/     `polars` feature 下的 DataFrame 转换
├── types/          公共数据结构
├── prelude.rs      常用 re-export
├── logger.rs       日志初始化
├── errors.rs       `TqError`
└── utils.rs        通用工具
```

- 示例在 `examples/`。
- 压测工件在 `benches/`。

## Canonical API 与架构约束

- live 主路径从 `Client` / `ClientBuilder` 进入。
- `Client` 应视为一次 live session owner，对齐 Python `TqApi` 的会话模型；不要新增 public `LiveClient` / `LiveSession` 双对象。
- `ClientBuilder` / `ClientConfig` / `TqAuth` 是构造侧概念；不要把 `Client` 降级为可反复切换 active live context 的配置根。
- `ReplaySession` 是唯一推荐的公开回放/回测入口。
- `DataManager` 是 DIFF 协议状态中心；merge 通知继续以 `subscribe_epoch()` 为主。
- WebSocket 层继续保持 I/O actor 模式，避免跨 `await` 持锁。
- `QuoteSubscription` 和 `SeriesSubscription` 已是 auto-start；创建后立即生效，`close()` 只负责提前释放资源。
- Quote 的订阅生命周期归 `QuoteSubscription`；长期读取路径应收敛到 `Client::quote(symbol)`。
- `SeriesSubscription` 的 canonical 消费方式是 `wait_update()` / `snapshot()` / `load()`。
- `SeriesSubscription` 的 canonical 发起入口应使用 `Client::{get_kline_serial,get_tick_serial}`。
- `Client::{get_kline_data_series,get_tick_data_series}` 是显式时间范围下载接口，不要把它们讲成普通“历史窗口”。
- `TqRuntime` 的目标持仓入口应使用 `runtime.account("...").target_pos(...).build()` 或 scheduler builder。
- `TradeSession` 对外分成两层：
  - 账户/持仓：最新状态读取 + 等待更新
  - 订单/成交/通知/异步错误：可靠事件流
- `subscribe_order_events()` / `subscribe_trade_events()` 是按类型过滤的可靠事件视图；不要再退回共享 retention 窗口导致误报 `Lagged` 的旧行为。
- 重连逻辑必须保留“临时缓冲 + 完整性校验 + 再合并”的策略。
- 行情与非关键状态更新上的有界缓冲和丢弃策略是刻意设计；不要无意改回无界队列。

## 不要重新引入的 public surface

- `BacktestHandle` 风格 facade
- `Client::tqapi()`
- 扩大 `Client::series()` 或 `Client::ins()` 的使用面
- `QuoteSubscription::start()` / `SeriesSubscription::start()`
- `on_data` / `on_data_register` / `off_data` callback plumbing
- `on_quote`、`quote_channel` 或其他 Quote fan-out API
- `on_update`、`on_new_bar`、`data_stream()` 或其他 series fan-out API
- runtime `compat::` 目标持仓 facade
- `on_order`、`on_trade`、`order_channel`、`trade_channel` 或其他 best-effort trade 事件 API
- 账户/持仓 snapshot callback/channel 依赖

## 本地常用命令

这些命令默认不应触达真实交易：

- 构建：`cargo build`
- 测试：`cargo test`
- 严格 lint：`cargo clippy --all-targets --all-features -- -D warnings`
- 格式检查：`cargo fmt --check`
- 示例编译检查：`cargo check --examples`
- Polars 构建：`cargo build --features polars`

## 环境变量提示

只有在任务确实需要联网、登录或运行示例时再设置这些变量。

- 认证：`TQ_AUTH_USER`、`TQ_AUTH_PASS`
- 常见示例开关：`TQ_LOG_LEVEL`、`TQ_TEST_SYMBOL`、`TQ_HISTORY_BAR_SECONDS`、`TQ_HISTORY_LOOKBACK_MINUTES`、`TQ_START_DT`、`TQ_END_DT`、`TQ_POSITION_SIZE`、`TQ_UNDERLYING`
- `trade` 示例相关：`SIMNOW_USER_0`、`SIMNOW_PASS_0`、`TQ_TRADE_EXAMPLE_DURATION_SECS`
- 高级端点覆盖：`TQ_AUTH_URL`、`TQ_MD_URL`、`TQ_TD_URL`、`TQ_INS_URL`、`TQ_CHINESE_HOLIDAY_URL`

除非任务明确要求，不要改默认服务端点、认证流程或权限检查逻辑。

## 验证矩阵

按改动范围选验证，不要机械地只跑一条最大命令：

- 只改文档：确认文本与当前仓库状态一致
- 只改纯逻辑模块：`cargo test`
- 改公开 API、trait、导出类型、模块导出：`cargo test` 和 `cargo clippy --all-targets --all-features -- -D warnings`
- 改 `polars_ext/`：追加 `cargo build --features polars`
- 改示例：至少运行 `cargo check --examples`
- 改 `websocket/`、`datamanager/`、`quote/`、`series/`、`trade_session/` 等核心链路：优先补/改对应测试，再跑完整 `cargo test`
- 改 README canonical 用法、架构说明或 agent 规则：额外确认 `skills/tqsdk-rs/`、`AGENTS.md`、`CLAUDE.md` 一致

## 联网 / live 安全边界

下列动作可能触达外部服务，甚至接近交易环境：

- `#[ignore]` 的 live 测试，尤其是 `src/ins/tests.rs` 中依赖 `TQ_AUTH_USER` / `TQ_AUTH_PASS` 的测试
- `cargo run --example history`
- `cargo run --example data_series`
- `cargo run --example quote`
- `cargo run --example option_levels`
- `cargo run --example backtest`
- `cargo run --example trade`

执行原则：

- 未经明确授权，不要运行 live 测试或任何需要账号的示例。
- 未经明确授权，绝不要运行 `cargo run --example trade`。
- 即使是模拟盘，也按高风险外部操作处理。
- 如果只是验证示例是否可编译，优先用 `cargo check --examples`。

## 修改代码时的工作准则

- 保持现有架构边界，不要把高层策略逻辑塞回底层传输层。
- 修改公开 API 时，同时检查 `README.md`、`examples/`、`prelude` re-export、`AGENTS.md`、`CLAUDE.md`。
- `skills/tqsdk-rs/` 是当前 public shape 的知识包，不是历史归档；当架构、公开 API、README canonical 路径、示例、迁移建议或 agent 规则变化时，要同步更新。
- 如果本次改动删除或替换了 public surface，同时更新 `docs/migration-remove-legacy-compat.md`。
- live API 继续收口到 `Client`，不要把新能力重新挂回 `TqApi` / `SeriesAPI` / `InsAPI`。
- runtime live adapter 必须复用 `Client` 的同一私有 live context，不要再自建第二套行情 websocket / `MarketDataState`。
- 切换账号应关闭旧 `Client` 并创建新 `Client`；不要把 `set_auth()+init_market()` 写成运行时切账号 canonical 路径。
- 权限检查优先 `Client::{auth_id,has_feature,check_md_grants}`；不要把 auth guard 暴露回主路径。
- 修改 `DataManager` merge/query/watch 语义时，要显式考虑向后兼容性。
- 修改 `DataManager` 通知路径时，继续优先 `subscribe_epoch()` + `get_path_epoch()`，不要为新逻辑重新引入 callback plumbing。
- 修改重连、背压或消息队列时，要说明是否改变了丢弃策略、顺序语义或完整性保证。
- 新增错误边界时，优先扩展 `TqError`，不要把公开错误类型打散。
- 示例代码属于公开接口的一部分；API 变了，示例必须同步。
- `benches/` 是内部性能验证工件，不是公开 canonical 示例；不要把 advanced/internal 压测脚本搬回 `examples/`。

## Agent 文档同步

- `skills/tqsdk-rs/` 应持续反映 `README.md`、`src/lib.rs`、`examples/`、`docs/architecture.md`、`AGENTS.md`、`CLAUDE.md` 的最新规则。
- 如果某次改动会让未来智能体继续生成旧 API、旧 callback/channel 写法、旧回测入口或过时验证策略，说明文档同步还没完成。
- 保持 `AGENTS.md` 作为较完整的仓库执行说明，保持 `CLAUDE.md` 作为更短、可导入的 Anthropic 项目记忆，两者在 canonical API 和安全边界上必须一致。

## 测试与示例约定

- 单元测试主要分布在对应模块目录和 `src/*/tests.rs`。
- 部分 `ins` 相关测试是 live 测试，默认被 `#[ignore]` 跳过。
- `examples/custom_logger.rs` 会创建 `logs/` 目录；不要提交产物。
- 示例应以“可读、可演示”为目标，不要为了局部技巧牺牲 API 清晰度。

## 安全要求

- 不要在代码、测试、示例、文档中硬编码真实账号、密码、token 或私有端点凭据。
- 不要提交本地联调凭据或日志产物。
- 不要为了方便调试而绕过权限检查、重连完整性校验或背压边界。

## 提交前检查

除非任务明确仅修改文档，否则至少确认：

- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`

按改动情况追加：

- `cargo fmt --check`
- `cargo build --features polars`
- README / 示例中的命令与当前代码一致
- `skills/tqsdk-rs/`、`AGENTS.md`、`CLAUDE.md` 已随 API / 架构 / agent 规则变化同步

## 建议阅读顺序

当任务不明确时，按这个顺序建立上下文：

1. `README.md`
2. `Cargo.toml`
3. `src/lib.rs`
4. 与任务直接相关的模块
5. 对应模块的测试
6. 相关示例

如果任务涉及 API 或架构变更，开始修改前先检查 README、示例和测试是否都要同步。

## 附录：架构参考

更完整的模块视图见 `docs/architecture.md`。
如果它与本文件的可执行规则冲突，以本文件为准。
