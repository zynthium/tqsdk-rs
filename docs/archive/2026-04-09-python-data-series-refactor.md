# Python-Compatible DataSeries Refactor Archive

日期：2026-04-09

## 目标

将 Rust SDK 的历史快照缓存机制重构为与官方 Python SDK `DataSeries` 机制对齐，并支持直接共用缓存文件。

本次重构按以下约束执行：

- 不考虑向后兼容旧 Rust 缓存
- 首版完整覆盖 K 线和 Tick 的时间区间快照缓存
- 移除按 ID 的一次性快照 API
- 仅保留与 Python 官方能力对齐的时间区间快照缓存路径

## 实际落地结果

### 1. 缓存实现已切换到 Python-compatible DataSeries

- 新增 [`src/cache/data_series.rs`](../../src/cache/data_series.rs)
- 缓存目录固定为 `~/.tqsdk/data_series_1`
- 文件命名采用 `{symbol}.{dur_nano}.{start_id}.{end_id}`
- 临时文件采用 `{symbol}.{dur_nano}.temp`
- 锁文件采用 `.{symbol}.{dur_nano}.lock`
- 文件内容改为本机字节序、本机对齐的定长二进制记录
- K 线列顺序、Tick 列顺序、Tick 5 档/1 档规则均按 Python 版实现
- 实现了：
  - `rangeset_id` 扫描
  - `rangeset_dt` 推导
  - 末尾失效裁剪
  - 按差集下载缺失区间
  - 原子写入正式缓存文件
  - 相邻文件合并
  - 按 `[start_dt, end_dt)` 语义读取时间窗口

### 2. 公开 API 已按计划调整

- 保留并重写：
  - `SeriesAPI::kline_data_series(...)`
  - `Client::get_kline_data_series(...)`
- 新增：
  - `SeriesAPI::tick_data_series(...)`
  - `Client::get_tick_data_series(...)`
- 移除：
  - `SeriesAPI::kline_data_series_by_id(...)`
  - `Client::get_kline_data_series_by_id(...)`

### 3. 旧缓存实现已清理

- 删除 [`src/cache/kline.rs`](../../src/cache/kline.rs)
- `src/cache/mod.rs` 不再导出旧缓存模块
- 删除 `bincode` 依赖
- 删除 `TqError::Bincode`
- 运行路径中不再使用 `.tqsdk_cache`

### 4. 文档已同步

- 更新 [`README.md`](../../README.md)
- 更新 [`docs/architecture.md`](../architecture.md)
- 文档已改为说明：
  - 时间区间 K 线/Tick 快照接口
  - Python-compatible `~/.tqsdk/data_series_1`
  - `series_disk_cache_*` 配置项现在作用于新的 DataSeries 缓存

## 计划核对结果

本次实现与重构计划逐项核对后，结论如下：

- Python-compatible 缓存目录、命名、锁文件：已完成
- K 线/Tick 定长二进制布局：已完成
- `rangeset_id` / `rangeset_dt`：已完成
- 尾段失效策略：已完成
- 差集下载、合并相邻文件：已完成
- K 线时间区间快照：已完成
- Tick 时间区间快照：已完成
- `by_id` 一次性快照 API 移除：已完成
- 旧缓存实现清理：已完成
- README / 架构文档同步：已完成

未发现与本次计划直接冲突的遗留旧实现入口。

## 验证记录

最终在当前树上重新执行并通过：

- `cargo fmt --check`
- `cargo test`
- `cargo check --examples`
- `cargo clippy --all-targets --all-features -- -D warnings`

其中 `cargo test` 最终结果为：

- 单元测试与集成测试：`123 passed`
- ignored：`3`
- doctest：`8 passed`, `1 ignored`

## 调试残留处理

在实现过程中，曾留下 3 个孤儿测试进程：

- `cached_data_without_network --nocapture`
- `tick_data_series_returns_cached_data_without_network --nocapture`
- `kline_data_series_by_datetime_returns_cached_data_without_network --nocapture`

这些进程已于本次归档前显式终止，并复查确认不再运行。

## 保留边界

以下内容是保留能力或明确非目标，不视为本次重构缺口：

- `kline_history(..., left_kline_id)` 订阅接口仍保留
  - 这是订阅接口，不是已移除的按 ID 一次性快照接口
- `SeriesOptions.left_kline_id` 仍保留
  - 它仍用于订阅型 `set_chart` 请求
- 当主目录不可用时，缓存根路径仍允许降级到临时目录
  - 这是库初始化的鲁棒性策略，不影响默认 Python-compatible 路径

## 结论

本次重构已按计划完成，并已完成代码、文档、验证和调试残留清理。
