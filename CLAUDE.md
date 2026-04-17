# CLAUDE.md — tqsdk-rs

参考 @AGENTS.md 获取完整的仓库执行规则。
参考 @README.md 获取公开用法、示例和环境变量。
参考 @docs/architecture.md 获取模块边界和设计模式。

## 高信号提醒

- 这是单 crate Rust 2024 项目，入口先看 `src/lib.rs`。
- live 主路径优先 `Client` / `ClientBuilder`。
- `Client` 是一次 live session owner，对齐 Python `TqApi`；不要新增 public `LiveClient` / `LiveSession` 双对象。
- 切换账号应关闭旧 `Client` 并创建新 `Client`，不要把 `set_auth()+init_market()` 当作 canonical 路径。
- 权限检查优先 `Client::{auth_id,has_feature,check_md_grants}`，不要暴露 auth guard 作为主路径。
- 回放/回测主路径优先 `ReplaySession`。
- `DataManager` 是 DIFF 状态中心，通知逻辑优先 `subscribe_epoch()` 驱动。
- `QuoteSubscription` / `SeriesSubscription` 已是 auto-start，`close()` 只负责提前释放资源。
- market ref 新代码优先使用 `try_load()` / `snapshot()` / `is_ready()`；`load()` 仅作为兼容 convenience API。
- `DataManager` watcher 推荐使用 `watch_handle()`；不要再把 `unwatch(path)` 当成精确释放单个 watcher 的 canonical 路径。
- live bounded 序列入口优先 `Client::{get_kline_serial,get_tick_serial}`。
- `Client::{get_kline_data_series,get_tick_data_series}` 是显式时间范围下载接口，不要和 serial 混用。
- `ReplaySession::runtime(accounts)` 初始化后账户集合固定；需要换账户集合时重新创建 `ReplaySession`。
- `Client::builder(...).build()` / `ClientBuilder::build()` 后若要用 live 行情 / serial / query facade，仍需显式 `init_market()`。
- `TradeSession` 的快照读取保持状态驱动；订单/成交/通知/异步错误保持可靠事件流。
- `TradeSessionEvent` 与 `MarketDataUpdates` 属于 root / prelude 的 canonical contract，可直接命名与导入。
- `ClientBuilder::build()` 不会隐式初始化 tracing；如需 SDK 日志，请显式调用 `init_logger()` 或组合 `create_logger_layer()`。

## 不要重新引入

- `BacktestHandle` 风格 facade
- `Client::tqapi()`
- 扩大 `Client::series()` / `Client::ins()` 的依赖面
- 重新把 `SeriesAPI` / `InsAPI` 暴露为对外扩展点
- `QuoteSubscription::start()` / `SeriesSubscription::start()`
- 已被移除的 quote / series / trade callback 或 channel fan-out API
- runtime `compat::` 目标持仓 facade
- runtime 自建第二套行情 websocket / `MarketDataState`
- `Client::set_auth()` / `Client::get_auth()` / `switch_to_live()` 公开入口

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
