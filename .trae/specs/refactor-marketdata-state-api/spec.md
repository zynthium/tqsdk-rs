# 高性能状态驱动行情订阅重构 Spec

## Why
当前 `tqsdk-rs` 的行情/序列订阅主要通过 Channel/Callback 与 `DataManager(serde_json::Value)` 进行路径监听与数据分发。在“全品种多合约订阅 + 单线程集中策略计算”的场景下，会产生显著的锁竞争、路径查找与分配开销，且策略端需要自行完成跨流状态同步，难以保持高性能与一致的心智模型。

本次重构目标是提供类似 Python TqSdk 的“状态驱动”使用体验，同时在 Rust 下实现更高吞吐与更低延迟，支撑 Live 与 Backtest 的高性能全品种策略。

## What Changes
- 新增高性能状态驱动行情数据层（Quote/Kline/Tick），以“强类型对象 + 版本号 + 精准唤醒/批量唤醒”为核心，对外提供订阅与读取能力。
- 重构现有 Quote/Series 订阅对外 API，使策略端不需要手写 Channel/Mutex/oneshot 来同步多个数据源。
- **BREAKING** 以新状态驱动接口替换 `QuoteSubscription`/`SeriesSubscription` 的 Channel/Callback 作为主要对外接口；旧接口可在迁移期保留为可选 feature 或内部适配层，但默认不再推荐。
- **BREAKING** `DataManager` 不再作为行情/序列的主对外访问路径；行情侧不再鼓励通过 `serde_json::Value` 路径访问核心数据。
- Live 与 Backtest 统一对外接口语义：策略端只感知“等待更新 → 读取快照/增量变更”。

## Impact
- Affected specs: 行情订阅、K线/Tick 序列订阅、背压/合并策略、示例代码与使用指南、（可选）Polars 转换入口。
- Affected code:
  - client: [facade.rs](file:///Users/joeslee/Projects/GitHub/tqsdk-rs/src/client/facade.rs)
  - websocket: `tqsdk-rs/src/websocket/*`（行情通道分发、背压、rtn_data 合并）
  - quote: `tqsdk-rs/src/quote/*`（订阅生命周期与 watch 模型）
  - series: `tqsdk-rs/src/series/*`（订阅与数据处理）
  - datamanager: `tqsdk-rs/src/datamanager/*`（行情路径监听的职责下沉/迁移）
  - types: `tqsdk-rs/src/types/*`（强类型结构体对齐与字段完善）

## ADDED Requirements

### Requirement: 高性能状态驱动 Quote API
系统 SHALL 提供强类型 Quote 订阅接口，返回可共享、可读取、可等待更新的 Quote 引用对象（下称 `QuoteRef`），并满足：
- 读取最新快照为 O(1)，不依赖 JSON 路径查找。
- 每个合约 Quote 更新时，能够触发该合约级别的等待唤醒（细粒度更新）。
- 提供版本号机制，使策略端可在不拷贝数据的情况下判断“自上次观测以来是否变化”。

#### Scenario: Success case（单合约）
- **WHEN** 用户订阅某合约 Quote 并等待更新
- **THEN** `quote_ref.wait_update().await` 在该合约发生更新时返回
- **AND** `quote_ref.is_changing()` 为 true
- **AND** `quote_ref.load()` 可读取到最新 `last_price` 等字段

#### Scenario: Success case（多合约）
- **WHEN** 用户订阅 N 个合约 Quote
- **THEN** 任一合约更新时，至少有一种方式让策略端以 O(changed) 的复杂度定位到发生变化的合约集合，而非强制遍历全部 N 个合约

### Requirement: 高性能状态驱动 Kline/Tick API
系统 SHALL 提供强类型 K线与 Tick 序列订阅接口（`KlineRef`/`TickRef`），并满足：
- 序列缓冲为固定上限（与现有 `view_width`/订阅长度一致），避免无界增长。
- 支持判断 “新 Bar 生成” 与 “最后一根 Bar 更新” 两类事件语义。
- Backtest 下支持高吞吐推进时间并持续产出一致的序列更新。

#### Scenario: Success case（新 K线）
- **WHEN** 用户订阅 K线并等待更新
- **THEN** 当新 K线生成时，策略端可区分 `NewBar` 与 `UpdateLastBar`（例如 `kline_ref.event()` 或等价机制）

### Requirement: 全局更新等待（可选）
系统 MAY 提供一个全局 `wait_update()`，用于“批量唤醒 + 增量集合返回”的模式，适用于全品种扫描策略：
- `wait_update()` 返回 `UpdateSet`（或等价结构），包含本次更新涉及到的 symbol/type 集合，且能被策略端高效迭代。
- `UpdateSet` 的创建与消费不得引入每次 tick 大量堆分配；应可复用缓冲或使用预分配结构。

#### Scenario: Success case（全品种扫描）
- **WHEN** 用户订阅大量合约并在循环中调用 `wait_update()`
- **THEN** 每次循环可仅处理本次变化的合约集合，并保持稳定 CPU 占用

### Requirement: 背压与数据合并策略
系统 SHALL 在慢策略/慢消费场景下保持稳定运行：
- 对“中间态”允许合并/覆盖（conflation），优先保证“最新状态”可用。
- 在队列/缓冲触顶时，应丢弃最旧更新并发出一次性告警/回调（复用现有背压能力，例如 [backpressure.rs](file:///Users/joeslee/Projects/GitHub/tqsdk-rs/src/websocket/backpressure.rs) 的 `rtn_data` 合并思想）。

## MODIFIED Requirements

### Requirement: 订阅接口对外语义
现有的订阅流程 “创建订阅 → 注册回调 → start()” SHALL 被替换为更直接的状态引用模式：
- 创建订阅时即可获得引用对象（Ref），Ref 的生命周期与订阅绑定。
- 策略端可选择：
  - 细粒度：对某些 Ref 调用 `wait_update().await`。
  - 批量：调用全局 `wait_update().await` 并消费 `UpdateSet`。

## REMOVED Requirements

### Requirement: 以 Channel/Callback 作为核心对外订阅模型
**Reason**: 对全品种复杂策略造成跨流同步负担与额外分配/锁开销，且难以提供一致性读取体验。
**Migration**: 以 `QuoteRef/KlineRef/TickRef + is_changing + wait_update` 替换旧的 Channel/Callback；若确需消息流，可在 Ref 上提供可选的 `stream()` 适配器作为附加能力（非核心路径）。

