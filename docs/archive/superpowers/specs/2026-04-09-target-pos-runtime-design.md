# Rust 版 TargetPosTask / Unified Runtime 设计

> Historical design note:
> This document captures an early runtime architecture exploration from April 2026.
> It may mention compatibility facades and adapter types that have since been removed or collapsed.
> For the current runtime API, prefer `README.md`, `docs/architecture.md`, and `docs/migration-remove-legacy-compat.md`.

## 状态

- 设计阶段：已确认
- 目标仓库：`tqsdk-rs`
- 设计日期：2026-04-09

## 背景

`tqsdk-python` 中的 `TargetPosTask` 不是边缘 helper，而是官方推荐的核心交易抽象。它直接出现在 README、quickstart、教程、示例和高级调度器中，承担“把目标净持仓变成持续执行的下单/撤单/追价/收尾流程”的职责。

当前 `tqsdk-rs` 已具备以下基础能力：

- 行情订阅与 `DataManager` 状态合并
- 交易会话、下单、撤单、账户/持仓/订单/成交查询
- 交易相关 DIFF 状态监听
- 市场回测时间推进

但这些能力仍然停留在“原语层”。Rust SDK 目前缺少一个统一的策略执行运行时，也缺少一个与 Python `TargetPosTask` 等价的长期运行任务抽象。

## 问题陈述

如果继续基于现有 `Client + TradeSession + BacktestHandle + QuoteSubscription` 的分裂模型增量打补丁，会遇到以下问题：

- `TargetPosTask` 只能做成脆弱的跨对象拼装，而不是统一执行模型
- `TargetPosScheduler`、未来 TWAP/VWAP、风控规则缺少可复用底座
- live/sim/backtest 执行语义会越走越分叉
- Python 策略迁移会持续遇到“Rust 没有等价心智模型”的阻力

因此，本设计不只解决 `TargetPosTask`，而是引入统一 runtime，把目标仓位执行、未来高级任务与交易回测都纳入同一条主路径。

## 目标

- 为 Rust SDK 提供未来主路径的统一运行时 `TqRuntime`
- 提供完整的 `TargetPosTask` 设计，支持 live / sim / future-backtest 三种执行模式
- 提供 Rust idiomatic API 与 Python 兼容 API 两层外观
- 提前为 `TargetPosScheduler`、TWAP/VWAP、风控规则预留统一任务执行框架
- 保留现有 `Client` / `TradeSession` 可用，但逐步将其降级为 facade

## 非目标

- 本设计不要求一次性实现交易回测撮合细节
- 本设计不要求立刻废弃 `Client`、`TradeSession` 或现有行情订阅 API
- 本设计不试图在第一阶段解决所有算法任务（TWAP/VWAP 等）
- 本设计不改动 DIFF merge 语义与重连完整性边界

## 设计原则

1. 统一 runtime 优先，不在旧抽象上继续堆特例
2. 算法主体与执行模式解耦，模式差异下沉到 adapter
3. 任务唯一性与订单归属必须显式建模，不能靠 symbol 推断
4. 必须建立在现有 `yawc` + WebSocket actor + `DataManager` DIFF 合并底座上，不为该功能单独引入新的传输/状态栈
5. 兼容 Python 心智模型，但核心实现以 Rust 异步任务模型为准
6. 先建立稳定执行底座，再扩展 scheduler/backtest/twap/vwap

## 顶层架构

### 新主路径

```text
ClientBuilder / TqRuntimeBuilder
    -> TqRuntime
        -> MarketAdapter
        -> ExecutionAdapter
        -> AccountHandle
        -> TaskRegistry
        -> ExecutionEngine
            -> TargetPosTask
            -> TargetPosScheduler
            -> Future Algo Tasks
```

### 现有路径的未来定位

- `Client`：认证、端点配置、兼容入口
- `TradeSession`：逐步降级为 `AccountHandle` 风格 facade
- `BacktestHandle`：逐步收敛进 runtime mode，而不再只是市场时间推进句柄

## 核心对象

### `TqRuntime`

职责：

- 统一持有 `DataManager`
- 统一管理行情源、执行源、任务注册表与生命周期
- 提供账户句柄、任务创建、统一关闭与模式切换能力

建议能力：

- `TqRuntime::builder(...)`
- `TqRuntime::account(account_key)`
- `TqRuntime::mode()`
- `TqRuntime::close()`

### `MarketAdapter`

职责：

- 统一 live / replay / backtest 的最新行情、交易时段与市场时间
- 向上提供最新 quote、时间变化通知、交易时段判断

建议 trait：

```rust
pub trait MarketAdapter: Send + Sync {
    fn dm(&self) -> Arc<DataManager>;
    async fn latest_quote(&self, symbol: &str) -> Result<Quote>;
    async fn wait_quote_update(&self, symbol: &str) -> Result<()>;
    async fn market_time(&self, symbol: &str) -> Result<MarketTime>;
    async fn is_in_trading_time(&self, symbol: &str) -> Result<bool>;
}
```

### `ExecutionAdapter`

职责：

- 统一 live、sim、future-backtest 三种执行模式
- 向上暴露一致的下单、撤单、订单/成交查询、持仓快照与事件等待能力

建议 trait：

```rust
pub trait ExecutionAdapter: Send + Sync {
    async fn insert_order(&self, req: &InsertOrderRequest) -> Result<String>;
    async fn cancel_order(&self, order_id: &str) -> Result<()>;
    async fn position(&self, account_key: &str, symbol: &str) -> Result<Position>;
    async fn active_orders(&self, account_key: &str, symbol: &str) -> Result<Vec<Order>>;
    async fn order(&self, account_key: &str, order_id: &str) -> Result<Order>;
    async fn trades_by_order(&self, account_key: &str, order_id: &str) -> Result<Vec<Trade>>;
    async fn wait_account_update(&self, account_key: &str) -> Result<()>;
}
```

### `AccountHandle`

职责：

- 面向单账户暴露查询、订阅与算法任务入口
- 成为未来 `TradeSession` 的兼容委托目标

建议能力：

- `get_account`
- `get_position`
- `get_orders`
- `get_trades`
- `insert_order`
- `cancel_order`
- `target_pos(symbol)`
- `target_pos_scheduler(symbol)`

### `TaskRegistry`

职责：

- 维护任务唯一性
- 建立 `order_id -> task_id` 的订单归属索引
- 管理同 `account + symbol` 的算法任务冲突

关键 key：

- `runtime_id + account_key + symbol`

## 双层 API 设计

### Rust idiomatic 层

```rust
let runtime = TqRuntime::builder(username, password)
    .live()
    .account(sim_account)
    .build()
    .await?;

let account = runtime.account("simnow:123456")?;
let task = account
    .target_pos("SHFE.rb2601")
    .config(TargetPosConfig::default())
    .build()
    .await?;

task.set_target_volume(5)?;
task.wait_target_reached().await?;
task.cancel().await?;
task.wait_finished().await?;
```

建议类型：

```rust
pub enum PriceMode {
    Active,
    Passive,
    Custom(Arc<dyn Fn(OrderDirection, &Quote) -> Result<f64> + Send + Sync>),
}

pub enum OffsetPriority {
    TodayYesterdayThenOpenWait,
    TodayYesterdayThenOpen,
    YesterdayThenOpen,
    OpenOnly,
}

pub struct VolumeSplitPolicy {
    pub min_volume: i64,
    pub max_volume: i64,
}

pub struct TargetPosConfig {
    pub price_mode: PriceMode,
    pub offset_priority: OffsetPriority,
    pub split_policy: Option<VolumeSplitPolicy>,
}
```

### Python 兼容层

兼容层不是第二套实现，只是围绕 `TargetPosHandle` 提供近 Python 语义的 facade。

```rust
let task = TargetPosTask::new(
    runtime.clone(),
    "simnow:123456",
    "SHFE.rb2601",
    TargetPosTaskOptions::default(),
).await?;

task.set_target_volume(5)?;
task.cancel().await?;
task.wait_finished().await?;
```

兼容语义：

- 同一 `runtime + account + symbol` 只允许一个 `TargetPosTask`
- 同配置重复创建返回同一逻辑任务
- 不同配置重复创建报错
- `set_target_volume()` 只更新目标，不阻塞等待
- `cancel()` 负责进入收尾并撤销本任务未完成挂单
- 同时提供 `is_finished()` 与 `wait_finished()`

## `TargetPosTask` 执行语义

### 状态机

状态建议如下：

- `Idle`
- `Planning`
- `Submitting`
- `Working`
- `Repricing`
- `Paused`
- `TargetReached`
- `Cancelling`
- `Finished`
- `Failed`

关键规则：

- `set_target_volume(v)` 在 `Idle`、`Planning`、`Submitting`、`Working`、`Repricing`、`Paused`、`TargetReached` 中都合法
- 最新目标覆盖旧目标，不保证中间目标被逐个达到
- `cancel()` 进入 `Cancelling`，撤掉本任务归属的活动单后转 `Finished`
- `wait_target_reached()` 只等待一次“当前目标被达到”
- `wait_finished()` 只表示任务彻底结束

### 目标驱动而非“每个 update 全撤全重算”

Rust 版必须对齐官方真实控制流，而不是采用“每次数据更新都先撤全部活动单、再重算、再重下”的简化模型。

官方语义更接近：

- `set_target_volume()` 只更新任务目标
- `TargetPosTask` 在收到一个新目标后，按 `offset_priority` 生成一组子任务
- 每个子任务负责一段明确的执行目标，例如 `BUY + CLOSE`、`SELL + OPEN`
- 真正的追价、撤单重报发生在子任务内部，而不是 `TargetPosTask` 主循环每个 update 都进行一次全量撤单

因此 Rust 版建议：

- `TargetPosTask` 负责目标整合、计划生成、子任务编排与取消收尾
- `InsertOrderUntilAllTradedTask` 等价物负责具体订单的追价、撤单和剩余量继续执行
- 当新目标覆盖旧目标时，中止旧计划并切换到新计划，而不是在每个市场事件上重建全量计划

### 调仓计划生成

沿用 Python 语义：

- 以净持仓为目标
- 根据 `offset_priority` 计算 `CLOSE` / `CLOSETODAY` / `OPEN`
- SHFE / INE 与其他交易所的平今/平昨规则分开处理
- 需要考虑当前活动单已经冻结的仓位

Rust 版不应依赖“Position 内嵌 orders”这种 Python 对象模型，而应直接通过：

- 当前持仓快照
- 任务归属的活动单集合
- `DataManager` 派生字段（如 `pos`、`is_dead`）

来生成计划。

需要特别注意：

- 官方 Python 在规划层主要依赖“该持仓相关的活动单集合 + `order.volume_left` + `pending_frozen`”
- 不能把 `volume_*_frozen_today/his` 直接当成 `TargetPosTask` 规划层的唯一真实来源
- Rust 版应从执行适配器或任务注册表拿到“本任务相关活动单集合”，再推导剩余可平量

### 价格模式

- `Active`
  - 买：优先卖一，再回退买一、最新价、昨收
  - 卖：优先买一，再回退卖一、最新价、昨收
- `Passive`
  - 买：优先买一，再回退卖一、最新价、昨收
  - 卖：优先卖一，再回退买一、最新价、昨收
- `Custom`
  - 由用户函数计算价格
  - 返回 `NaN` 或非法价格时直接失败

### 大单拆分

当配置 `split_policy` 时：

- 剩余量 `< max_volume`：直接整笔下单
- 剩余量 `>= max_volume`：在 `[min_volume, max_volume]` 之间随机/策略化拆分
- 仅当 `min_volume` 与 `max_volume` 同时设置且合法时启用

### 最小开仓手数限制

官方实现会显式拒绝“最小市价开仓手数”或“最小限价开仓手数”大于 1 的合约，因为部分成交后剩余手数可能无法重新报单。

Rust 版必须保留该限制检查，并在任务启动阶段直接返回明确错误，而不是进入执行期后再随机失败。

### 非交易时段

行为与 Python 对齐：

- 不在可交易时段时不盲目下单
- 进入 `Paused`
- 等待下次有效行情/市场时间推进后重新进入 `Planning`

## 冲突与安全语义

### 任务唯一性

同一个：

- `runtime`
- `account`
- `symbol`

在任意时刻只允许一个仓位控制类任务：

- `TargetPosTask`
- `TargetPosScheduler`
- 未来 `TwapTask`
- 未来 `VwapTask`

冲突规则：

- 同 key、同配置：返回同一逻辑任务
- 同 key、不同配置：报错
- 不同 symbol：允许并行
- 多账户：按 `account_key + symbol` 隔离

### 手工下单冲突

当某个 `account + symbol` 已被仓位控制类任务占用时：

- 默认拒绝手工 `insert_order()`
- 如需要强制绕过，必须显式指定 `ManualOrderMode::Force`
- 发生强制绕过后，相关任务应进入 `Failed` 或 `Cancelling`

### 订单归属

必须显式维护：

- `order_id -> task_id`

原因：

- 不能只按 symbol 归因订单
- 取消时必须只撤本任务订单
- 计算剩余量、冻结量、成交量时必须精确到任务

建议做法：

- 订单号前缀带 `runtime/task` 信息
- 注册表里保留强索引
- 所有归因逻辑基于索引而非模糊扫描

### 订单完成语义

Rust 版不能把“订单状态变成 `FINISHED`”简单视作“该子任务已完全结束”。

需要与官方对齐：

- 订单 `FINISHED` 仅表示委托生命周期结束
- 子任务结束前还需要确认成交记录已经完整可见，至少要能可靠归集该订单已成交手数
- 关闭 / 取消路径需要等待子任务收尾，并设置合理超时，避免连接关闭后子任务永久悬挂

## live / sim / backtest 一致性

### 算法主体不区分模式

`TargetPosTask` 主体只依赖统一抽象：

- `position_snapshot`
- `active_orders`
- `recent_trades`
- `latest_quote`
- `market_time`
- `is_in_trading_time`

差异全部下沉到 `ExecutionAdapter` / `MarketAdapter`。

### 适配器模式

- `LiveExecutionAdapter`
  - 对接真实交易网关
- `SimExecutionAdapter`
  - 对接模拟交易网关
- `BacktestExecutionAdapter`
  - 对接未来交易回测撮合

### 对现有回测能力的要求

当前 `tqsdk-rs` 的 backtest 只覆盖市场时间推进，不覆盖统一交易执行。因此：

- `TargetPosTask` v1 只承诺 live / sim
- 设计层面必须预留 `BacktestExecutionAdapter`
- 交易回测完成后，`TargetPosTask` 不需要重写算法主体，只需切换 adapter

## `TargetPosScheduler`

`TargetPosScheduler` 直接建立在 `TargetPosTask` 上，不单独实现一套下单状态机。

职责：

- 接受 `time_table`
- 按顺序创建或复用 `TargetPosTask`
- 在每个 step 的 deadline 前执行指定价格模式
- 最后一项等待目标仓位达成
- 聚合所有成交，输出执行报告

这保证：

- scheduler 与 target pos 逻辑一致
- future TWAP/VWAP 可以继续复用该执行层

## 向后兼容策略

### `Client`

- 保留现有 `ClientBuilder::build()`
- 新增 `ClientBuilder::build_runtime()` 或 `Client::into_runtime()`
- 新文档主路径转向 runtime

### `TradeSession`

- 第一阶段保持原样
- 第二阶段开始将查询/下单/撤单逐步委托给 `AccountHandle`
- 最终定位为兼容 facade，而不是未来主入口

### `BacktestHandle`

- 第一阶段继续保留现状
- 后续逐步将其能力合并到 runtime mode

## 文件与模块拆分

建议新增：

```text
src/runtime/
├── mod.rs
├── core.rs
├── account.rs
├── registry.rs
├── engine.rs
├── market.rs
├── execution.rs
├── errors.rs
├── types.rs
├── modes/
│   ├── live.rs
│   └── backtest.rs
└── tasks/
    ├── common.rs
    ├── target_pos.rs
    └── scheduler.rs

src/compat/
├── mod.rs
├── target_pos_task.rs
└── target_pos_scheduler.rs
```

现有模块的未来接缝：

- `src/client/`：新增 runtime 构建与托管逻辑
- `src/trade_session/`：逐步改为 facade
- `src/backtest/`：逐步改为 runtime mode 的组成部分

## 测试策略

### 第一阶段

- `TaskRegistry` 唯一性测试
- 订单归属索引测试
- `TargetPosConfig` 参数校验测试
- live/sim adapter 的查询和委托桥接测试

### 第二阶段

- `TargetPosTask` 状态机测试
- `offset_priority` 计划生成测试
- repricing 触发撤单重报测试
- cancel 收尾测试
- 手工下单冲突测试
- 最小开仓手数限制测试
- 子任务“订单已 FINISHED 但成交归集未完整”收尾测试

### 第三阶段

- `split_policy` 拆单测试
- `TargetPosScheduler` deadline 行为测试
- 成交归集与执行报告测试

### 第四阶段

- `BacktestExecutionAdapter` 撮合一致性测试
- runtime 模式切换测试
- Python 示例迁移测试

## 分阶段实施顺序

### 阶段 1：Runtime skeleton

- 建立 `TqRuntime`
- 接通 `MarketAdapter` / `ExecutionAdapter`
- 建立 `AccountHandle`
- 建立 `TaskRegistry`
- 不实现 `TargetPosTask`

### 阶段 2：`TargetPosTask` v1

- 支持 `Active` / `Passive`
- 支持 `offset_priority`
- 支持 `cancel` / `wait_finished` / `wait_target_reached`
- 支持冲突拒绝
- 仅覆盖 live / sim

### 阶段 3：`TargetPosTask` v2 + `TargetPosScheduler`

- 支持 `split_policy`
- 支持 scheduler
- 支持成交归集与执行报告
- 对外补文档与 examples

### 阶段 4：交易回测执行

- 落地 `BacktestExecutionAdapter`
- 打通 backtest 下的 `TargetPosTask`
- 为 TWAP / VWAP / 风控任务提供执行底座

## 主要风险

### 风险 1：旧 API 与新 runtime 双轨长期并存

缓解：

- 明确 runtime 是未来主路径
- 兼容层只做 facade，不做第二套实现

### 风险 2：订单归属不清导致误撤单

缓解：

- 强制 `order_id -> task_id` 索引
- 不依赖 symbol 模糊推断

### 风险 3：回测能力滞后，导致“设计很完整、实现不完整”

缓解：

- 在设计上预留 backtest adapter
- 在实施上分阶段交付，不承诺第一阶段即全模式可用

### 风险 4：把 runtime 做成新的“大泥球”

缓解：

- runtime 只做编排，不做所有业务逻辑
- 具体算法下沉到 `tasks/`
- 模式差异下沉到 `modes/`
- 归属与冲突单独下沉到 `registry.rs`

## 最终决策摘要

- 采用统一 runtime 作为未来主路径
- 采用 Rust idiomatic + Python 兼容 facade 双层 API
- `TargetPosTask`、`TargetPosScheduler`、未来算法任务共享同一执行底座
- live/sim/backtest 差异下沉到 adapter，而不是复制算法
- 分四阶段落地，优先建立 runtime skeleton 和 `TargetPosTask` v1
