# tqsdk-rs Replay Backtest Design

## 背景

当前 `tqsdk-rs` 的回测能力本质上是“时间推进句柄 + 现有行情接口复用”：

- `BacktestHandle` 负责写入 `_tqsdk_backtest.current_dt` 并发送 `peek_message`
- `SeriesAPI::kline()/tick()` 仍然走实时 chart 订阅路径
- `runtime` 的 backtest 模式只替换了执行适配器，没有本地回放行情内核

这导致 Rust 当前回测语义与官方 `tqsdk-python` 不一致。Python 的 `TqBacktest` 不是简单依赖服务端 realtime chart，而是通过本地 serial 生成器将历史 K 线 / Tick 按时间回放，再据此生成 quote 并驱动模拟撮合。

因此，Rust 若要实现与 Python 对等的回测功能，不能继续在现有 realtime 系列 API 上打补丁，而应提供显式的 replay/backtest API。

## 目标

v1 目标只有一个：

**让 Rust 可以在显式 replay session 下，稳定复现 Python `TqBacktest + TqSim` 最常见策略路径的核心回测语义。**

这里的“核心语义”指：

- 回测数据由本地 session 显式推进，而不是依赖 realtime 订阅副作用
- 单合约 K 线 / Tick 序列能够随回测时间推进更新
- 多合约对齐 K 线在回测中可稳定使用
- quote 的生成规则与 Python 的回测规则一致
- `TargetPosTask` 等 runtime 任务可以在回测行情上工作
- 模拟撮合能使用 replay quote 路径得到合理且稳定的结果

设计优先级：

1. 策略结果语义对齐
2. Rust 原生 API 清晰与可维护性
3. 尽量复用现有组件
4. Python 接口形状兼容性

## 非目标

v1 明确不做以下内容：

- Python `wait_update()` 风格 API 兼容壳
- 将 replay 状态重新编码回 `DataManager` diff/merge 流程
- 对外暴露 `chart_id`、`left_id`、`right_id` 兼容语义
- DataFrame 级兼容
- 完整复制 Python 所有边角行为与内部优化
- 股票、ETF、完整期权账户体系的全量模拟兼容
- 复用当前 realtime `SeriesSubscription` 作为 backtest serial 核心实现

核心原则：

**如果某个 Python 复杂度只服务其 `wait_update/diff/DataFrame` 框架，而不影响策略逻辑、quote 语义或撮合结果，则 Rust 不实现。**

## Python 实现调研结论

### 必须对齐的部分

根据官方 `tqsdk-python` 本地实现，以下行为是回测语义的核心：

- 回测模式下 K 线只在“新 bar 创建”和“bar 收盘”两个时点更新
- quote 来源有优先级：`tick > 最小周期 kline > 自动补一分钟线`
- `wait_update()` 每次最多推进一个“行情时间”
- K 线回测时，模拟撮合仍需要消费 bar 内价格路径
- 模拟撮合基于 quote 序列顺序判断成交，而不是直接读取最终 OHLC

这些行为分别体现在：

- `tqsdk/backtest.py` 对回测语义的定义
- `_gen_serial()` 对 K 线 / Tick 的本地回放
- `_get_quotes_from_tick()` / `_get_quotes_from_kline_open()` / `_get_quotes_from_kline()`
- `tradeable/sim/trade_base.py` 对订单撮合的处理

### 可以不照搬的部分

Python 中以下复杂度主要来自其整体框架，不是 Rust v1 的必需项：

- `wait_update()` 同时承担任务调度、网络收包、diff merge、serial DataFrame 更新
- 双 chart + `8964` 窗口 + `_sended_to_api` + `gc_data()` 的数据回收机制
- `chart_id`、`binding`、`right_id` 等面向 diff/DataFrame 的内部脚手架

Rust 应保留结果语义，不复制这些中间形态。

## 总体方案

新增显式 replay/backtest 子系统，不把新能力继续塞进当前 `BacktestHandle`。

对外入口：

- `Client::create_backtest_session(config) -> ReplaySession`
- `ReplaySession::series().kline_serial(...)`
- `ReplaySession::series().tick_serial(...)`
- `ReplaySession::series().aligned_kline_serial(...)`
- `ReplaySession::quote(symbol)`
- `ReplaySession::runtime(accounts)`
- `ReplaySession::step()`

核心设计原则：

- 一个 `ReplaySession` 表示一个独立回测世界
- 时间推进只有一个入口：`step()`
- serial、quote、matcher、runtime 都挂在 session 上
- 不存在“realtime series 在 backtest 模式下自动切语义”的隐式行为

## 公开 API 设计

建议的最小外部接口：

```rust
let mut session = client.create_backtest_session(config).await?;

let quote = session.quote("SHFE.cu2606").await?;
let daily = session
    .series()
    .kline_serial("SHFE.cu2606", Duration::from_secs(86_400), 200)
    .await?;
let spread = session
    .series()
    .aligned_kline_serial(&["SHFE.rb2605", "SHFE.hc2605"], Duration::from_secs(60), 300)
    .await?;

let runtime = session.runtime(["TQSIM"]).await?;

while let Some(step) = session.step().await? {
    if daily.updated_in(&step) {
        // 读取序列与 quote，执行策略
    }
}
```

外部 API 特征：

- 使用显式 `step()` 推进
- serial/quote 是只读句柄，不自行推进时间
- 不暴露 `chart_id`、`left_id`、`right_id`
- `kline_serial` 的最新一根可以处于 opening/provisional 状态，但状态显式可读

## 时间推进模型

`ReplaySession::step()` 的语义定义为：

**推进到下一个“行情时间”，并原子完成该时间点上 serial、quote、matcher、runtime 的状态更新。**

对于用户而言：

- 一次 `step()` 之后读取到的 serial、quote、runtime 状态必须来自同一个回测时间点
- 不允许出现 serial 已推进而 matcher 还停留在上一跳的状态

对于内部而言：

- `step()` 可能处理单个 tick，或单个 K 线的 opening/closing 阶段
- 但对外返回的仍是单个 `ReplayStep`
- `ReplayStep` 只暴露“哪些句柄发生了变化”以及“当前 replay 时间”

这等价于保留 Python 的“每次最多推进一个行情时间”语义，但不暴露 Python 的 `wait_update()` 和 diff 细节。

## Serial 模型

### 单合约 K 线

`kline_serial(symbol, duration, width)` 返回固定窗口的只读序列句柄。

对外表现：

- 窗口长度固定
- 最新一根 bar 可能是 `Opening` 或 `Closed`
- `Opening` 阶段时 bar 的 `open/high/low/close` 都等于开盘价
- `Closed` 阶段时 bar 为完整 OHLCV

这保留了 Python “新 bar 出现一次、bar 收盘再更新一次”的可观察语义，但用显式状态代替隐式 diff。

### Tick

`tick_serial` 只维护固定窗口的最终 tick 数据。

Tick 没有 provisional 状态，单次出现即为完整数据。

### 多合约对齐 K 线

`aligned_kline_serial(symbols, duration, width)` 返回以主时间线为基准的固定窗口。

对外暴露为按时间对齐的一组行，每行包含：

- 主时间戳
- 主合约 bar
- 其他合约在该时间点上的对齐 bar 或 `None`

内部可使用 binding 索引，但不对外暴露 Python 的 binding 结构。

## Quote 生成规则

quote 来源优先级直接对齐 Python：

1. 若某合约订阅了 tick serial，则 quote 来自 tick
2. 否则若订阅了一个或多个 K 线 serial，则 quote 来自该合约的最小周期 K 线
3. 否则自动为该合约补一个一分钟 K 线来源，仅用于 quote 生成和模拟撮合

K 线驱动 quote 的规则：

- bar opening：以开盘价生成一跳 quote
- bar closing：对用户可见 quote 更新为 close
- matcher 内部需要消费 high/low/close 的价格路径，以完成与 Python 类似的撮合判断

重要约束：

- 内部 matcher 可以看到完整价格路径
- 外部用户看到的是稳定 quote 快照，而不是内部所有子步骤

## 撮合模型

新增本地 `Matcher`，不复用当前 `BacktestExecutionAdapter` 的“下单即成交”逻辑。

撮合规则 v1 对齐 Python `TqSim` 的核心部分：

- 市价单按对手价成交；若无对手价则撤单
- 限价单当价格达到或超过对手价时成交；成交价为报单价
- IOC 不能成交则直接撤单
- 仅支持全部成交，不做部分成交
- 未成交订单在后续 quote 更新时重新判断

`Matcher` 只依赖当前 quote 路径，不依赖 serial 的具体表示。

对于 K 线驱动回测：

- 用户策略每个 `step()` 只看到 stable state
- matcher 在当前 step 内部仍可按 high/low/close 路径依次判断成交

## Runtime 集成

为了让 `TargetPosTask` 可直接工作，需要提供 replay 版 runtime 适配器：

- `ReplayMarketAdapter`
- `ReplayExecutionAdapter`

其中：

- `ReplayMarketAdapter` 从 session 的 replay quote 读取行情
- `ReplayExecutionAdapter` 委托给 `Matcher`

这样 `runtime` 层不需要理解 replay 内部细节，只继续依赖现有 trait：

- `MarketAdapter`
- `ExecutionAdapter`

## 模块边界

建议新增精简的 replay 子系统：

- `src/replay/mod.rs`
- `src/replay/session.rs`
- `src/replay/loader.rs`
- `src/replay/serial.rs`
- `src/replay/quote.rs`
- `src/replay/matcher.rs`
- `src/replay/runtime.rs`

职责：

- `session.rs`
  - session 生命周期与 `step()` 推进
  - 管理已注册 serial / quote / runtime

- `loader.rs`
  - 复用现有历史接口和缓存拉取原始 tick/kline 数据

- `serial.rs`
  - 单合约 K 线
  - Tick
  - 多合约对齐 K 线

- `quote.rs`
  - quote 源优先级
  - 自动补一分钟线
  - quote 快照更新

- `matcher.rs`
  - 订单、成交、持仓、资金
  - 按 quote 路径撮合

- `runtime.rs`
  - replay 版 `MarketAdapter` / `ExecutionAdapter`

## 复用与迁移策略

以下能力应尽量复用现有实现：

- 历史数据拉取与缓存：复用 `series/api.rs` 和 `DataSeriesCache`
- runtime trait 边界：复用现有 `runtime` 设计
- 现有 `TargetPosTask` / `TargetPosScheduler` 逻辑

以下能力不应复用：

- realtime `SeriesSubscription`
- 现有 `BacktestHandle` 作为新 replay 核心
- 当前 `BacktestExecutionAdapter` 的即时成交语义

`BacktestHandle` 在 v1 中保留为 legacy 功能，不与新 replay API 混合。

## v1 范围

### 必做

- 单合约 `kline_serial`
- 单合约 `tick_serial`
- 多合约对齐 `aligned_kline_serial`
- 显式 `step()`
- quote 源优先级与自动补一分钟线
- K 线 opening/closing 两阶段可观察语义
- matcher 内部 high/low/close 路径
- replay runtime 支撑 `TargetPosTask`
- futures / 商品期权所需的基础撮合规则
- 回测结束判定

### 延后但预留

- 滑点/手续费可插拔模型
- `run_until(...)` 等高级推进接口
- 回测报告与统计
- 更丰富的订阅/stream/watcher 观察接口

### 明确不做

- Python `wait_update()` 兼容壳
- diff/DataFrame 兼容
- 对外公开 `chart_id/right_id/left_id`
- 股票 / ETF / 全量期权账户模拟兼容
- 所有 Python 边角行为

## 风险与约束

主要风险不在 replay API 本身，而在以下两点：

1. 历史数据装载与自动补一分钟线的交互
2. matcher 内部价格路径与 `TargetPosTask` 提交时机的契约

对应约束：

- session 内部所有 replay 状态更新必须串行完成
- `step()` 后的所有可见状态必须一致
- 新订单不能回头消费已经过去的价格路径

## 结论

Rust 的对等实现不应复制 Python 的整套 `wait_update/diff/DataFrame` 体系。

更合理的实现方式是：

- 提供显式 `ReplaySession`
- 以本地 serial 回放为核心
- 用 quote 源优先级和最小撮合规则复现 Python 的关键语义
- 通过 runtime adapter 复用现有 Rust 任务系统

这条路线在复杂度、语义对齐和 Rust 风格之间取得了最合理的平衡。
