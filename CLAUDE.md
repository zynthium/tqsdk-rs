# CLAUDE.md — tqsdk-rs

参考 @AGENTS.md 获取完整的仓库执行规则。
参考 @README.md 获取公开用法、示例和环境变量。
参考 @docs/architecture.md 获取模块边界和设计模式。

## 高信号提醒

- 这是单 crate Rust 2024 项目，入口先看 `src/lib.rs`。
- live 主路径优先 `Client` / `ClientBuilder`。
- 回放/回测主路径优先 `ReplaySession`。
- `DataManager` 是 DIFF 状态中心，通知逻辑优先 `subscribe_epoch()` 驱动。
- `QuoteSubscription` / `SeriesSubscription` 已是 auto-start，`close()` 只负责提前释放资源。
- `TradeSession` 的快照读取保持状态驱动；订单/成交/通知/异步错误保持可靠事件流。

## 不要重新引入

- `BacktestHandle` 风格 facade
- `Client::tqapi()`
- 扩大 `Client::series()` / `Client::ins()` 的依赖面
- `QuoteSubscription::start()` / `SeriesSubscription::start()`
- 已被移除的 quote / series / trade callback 或 channel fan-out API
- runtime `compat::` 目标持仓 facade

## 常用命令

- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo fmt --check`
- `cargo check --examples`
- `cargo build --features polars`

## 验证与安全

- 只改文档：确认文档与当前仓库状态一致。
- 改公开 API 或导出类型：运行 `cargo test` 和 `cargo clippy --all-targets --all-features -- -D warnings`。
- 改示例：至少运行 `cargo check --examples`。
- 未经明确授权，不要运行被忽略的 live 测试或任何需要账号的示例。
- 未经明确授权，绝不要运行 `cargo run --example trade`。

## 同步要求

- canonical API 或 agent 规则变化时，同步检查 `README.md`、`examples/`、`prelude` re-export、`skills/tqsdk-rs/`、`AGENTS.md` 和本文件。
