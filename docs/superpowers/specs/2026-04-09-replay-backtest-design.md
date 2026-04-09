# tqsdk-rs Replay Backtest Design

## 结论先行

基于对官方 `tqsdk-python` 回测实现的重新拆解，Rust 版回测最应该对齐的，不是 `wait_update()`、diff merge、chart_id 或 DataFrame 更新机制，而是下面这条核心链路：

**历史数据源 -> 本地行情时间推进器 -> quote 合成器 -> 本地撮合器 -> 账户/持仓/序列快照**

Python 的 `TqBacktest` 本质上是一个插在 `TqSim` 和上游行情服务之间的“虚拟行情服务器”；`TqSim` 则是本地撮合器。Rust 版最合理的对等实现，应保留这个“组件替换 + 本地回放”的架构思想，但把 Python 的 diff/queue/DataFrame 脚手架换成强类型的 session/kernel API。

本设计**不考虑当前仓库中已经存在的 Rust 回测实现**，按全新的 Rust 回测子系统来设计。

## 设计目标

v1 的目标只有一个：

**让 Rust 可以通过显式 replay session，稳定复现 Python `TqBacktest + TqSim` 最常见策略路径的核心回测语义。**

这里的“核心回测语义”指：

- 一个本地 session 拥有唯一的回测时间
- 单次推进最多进入一个“行情时间”
- 单合约 K 线 / Tick 序列随时间推进更新
- 多合约对齐 K 线可以稳定使用
- quote 来源遵守 Python 的优先级规则
- K 线回测时，撮合器仍能看到 bar 内价格路径
- 本地模拟账户可以完成下单、撮合、持仓更新、日切结算
- `TargetPosTask` 等 runtime 任务可以直接跑在 replay 行情上

优先级排序：

1. 策略结果与撮合结果语义对齐
2. Rust API 清晰、强类型、可维护
3. 内部实现简单且可测
4. 与 Python API 形状近似

## Python 实现中真正重要的部分

根据官方 Python 回测实现，以下行为是 Rust 必须吸收的：

### 1. 管道式替换架构

Python 在回测模式下不是“给 realtime API 加一个开关”，而是直接把 `TqBacktest` 插入数据管道，让它充当假的行情上游。这个架构思路是对的，Rust 也应该采用“插入本地回放内核”的思路，而不是继续复用 realtime series 订阅。

### 2. 一个回测时间点只推进一次

Python 保证 `wait_update()` 每次最多推进一个行情时间点。这保证了策略在同一时间点上的判断、下单、撤单具有确定性。Rust 虽然不需要实现 `wait_update()`，但必须保留“一次 step 只推进一个市场时间点”的契约。

### 3. K 线两阶段更新

Python 回测里每根 K 线不是只出现一次，而是：

- bar 刚开始时先出现一个 opening 状态
- bar 结束时再变成最终 OHLCV

Rust 必须保留这个观察语义，否则大量基于“当前 bar 是否刚生成”的策略无法平滑迁移。

### 4. quote 来源优先级

Python 回测的 quote 来源有明确优先级：

1. 订阅了 tick，用 tick
2. 没有 tick，但订阅了 K 线，用最小周期 K 线
3. 都没有时，自动补一个 1 分钟 K 线，只用于 quote 和撮合

这个规则直接决定策略看到的行情和模拟成交结果，Rust 必须对齐。

### 5. K 线撮合需要 bar 内价格路径

Python 的关键点不是“用户看到了 high/low diff”，而是 **TqSim 需要按顺序消费 high/low/close 对应的盘口变化**，否则限价单会漏成交。Rust 不需要复制 Python 的多 diff 包形式，但必须保留“matcher 能消费 bar 内价格路径”的内部语义。

### 6. 本地模拟账户与结算

Python 回测里的交易语义不是附属功能，而是核心能力：

- 订单进入本地撮合器
- 撮合器根据 quote 路径判断成交
- 日切时执行结算、撤 GFD 单、滚动今昨仓
- 最终产出 trade log / 账户快照

Rust v1 也应把“本地模拟账户 + 结算”作为一等能力，而不是只做一个极简 order fill stub。

## Python 实现中不该照搬的部分

以下复杂度主要服务 Python 自身框架，不应原样迁移到 Rust：

- `wait_update()` 同时承担任务调度、网络收包、diff merge、DataFrame 更新
- `chart_id_a/chart_id_b` 双窗口和 `10000` 根滑动 chart 的传输层技巧
- `_sended_to_api` / `_gc_data()` 这种面向 diff 协议的回收逻辑
- `chart_id` / `left_id` / `right_id` / `binding` 的对外暴露
- Web GUI 报表绘制链路

Rust 可以保留这些机制背后的语义目标，但不保留它们的实现形态。

## 总体架构

Rust 版回测采用**强类型管道内核**：

```text
Historical Source
    ↓
Replay Kernel
    ├── Serial Store
    ├── Quote Synthesizer
    ├── Sim Broker
    └── Runtime Adapters
```

各层职责：

- `Historical Source`
  - 拉取 K 线 / Tick / 合约静态信息
  - 为连续合约提供主连映射
  - 未来可扩展股票分红送股数据源

- `Replay Kernel`
  - 拥有唯一回测时钟
  - 合并所有订阅序列的下一个事件时间
  - 驱动 serial、quote、broker 在同一时间点上更新

- `Serial Store`
  - 保存外部可读的 K 线 / Tick / 对齐 K 线窗口

- `Quote Synthesizer`
  - 根据 tick / 最小周期 kline / 自动 1 分钟线生成 quote
  - 在 K 线回测时提供 bar 内价格路径给 broker

- `Sim Broker`
  - 管理订单、成交、持仓、资金、结算

- `Runtime Adapters`
  - 让 replay 行情直接接入 `TargetPosTask` / `TargetPosScheduler`

## 对外 API

对外接口保持显式、最小、可预测：

```rust
let mut bt = client.create_backtest_session(config).await?;

let quote = bt.quote("SHFE.cu2606").await?;
let daily = bt.series().kline("SHFE.cu2606", Duration::from_secs(86_400), 200).await?;
let spread = bt
    .series()
    .aligned_kline(&["SHFE.rb2605", "SHFE.hc2605"], Duration::from_secs(60), 300)
    .await?;

let runtime = bt.runtime(["TQSIM"]).await?;

while let Some(step) = bt.step().await? {
    if daily.updated_in(&step) {
        // 读取序列、quote、账户状态并执行策略
    }
}

let result = bt.finish().await?;
```

公开 API 的设计原则：

- 时间推进只有一个入口：`step()`
- serial / quote / account 都是只读句柄
- 不暴露 `chart_id` / `left_id` / `right_id`
- 不做 Python `wait_update()` 风格兼容
- 回测结束通过显式 `finish()` 或 `step() -> None` 表达，不用异常作为主控制流

## 时间推进模型

`step()` 的定义：

**推进到下一个市场时间点，并在返回前完成该时间点上的 serial、quote、broker、runtime 全部状态更新。**

这意味着：

- 一次 `step()` 返回后，策略看到的是单一、一致的世界状态
- 不会出现 serial 已前进但账户状态还没更新的半提交状态

### 外部时间粒度

外部 API 只看到“市场时间点”，而不是 Python diff 包级别的子步骤。

对于 K 线驱动场景，`step()` 只会在以下两类外部可观察时间点返回：

- `BarOpenTime`
  - 当前 bar 首次出现
  - 用户可观察到 `BarState::Opening`
  - 用户可观察到以开盘价合成的 quote

- `BarCloseTime`
  - 当前 bar 收盘
  - 用户可观察到 `BarState::Closed`
  - 用户可观察到以收盘价稳定下来的 quote

也就是说，Rust 必须保留 Python 的“K 线在开盘和收盘各更新一次”的策略可观察语义；只是不会把 high/low 的内部撮合路径也暴露成额外的公共更新时间点。

### 内部微步骤

内部仍允许在单个 `step()` 中执行多个撮合微步骤，尤其是 K 线驱动时：

- `BarOpen`
- `BarHigh`
- `BarLow`
- `BarClose`

这些微步骤只对 quote 合成器和 broker 可见，不直接暴露给用户。用户在该 `step()` 结束后只看到最终稳定快照。

这种设计保留了 Python 的撮合能力，但避免把 Python 的多 diff 包细节泄漏到 Rust 公共接口。

## Serial 设计

### 单合约 K 线

`series().kline(symbol, duration, width)` 返回固定窗口序列。

每根 bar 具备显式状态：

- `Opening`
- `Closed`

语义：

- `Opening` 时，`high=low=close=open`
- `Closed` 时，为完整 OHLCV

这等价于 Python 的“K 线刚创建更新一次、收盘再更新一次”，但不使用 diff 形式表达。

### Tick

`series().tick(symbol, width)` 返回固定窗口 tick 序列。

Tick 不存在 provisional 状态，出现即为完整数据。

### 多合约对齐 K 线

`series().aligned_kline(symbols, duration, width)` 返回以主时间线对齐的固定窗口。

对外每一行包含：

- 主时间戳
- 主合约 bar
- 副合约 bar 或 `None`

内部允许使用 binding 映射或 join 索引，但不对外暴露 Python 的 binding 结构。

## Quote 生成规则

Rust 直接对齐 Python 的来源优先级：

1. 有 tick 订阅时，quote 来源为 tick
2. 没有 tick，但有 K 线时，quote 来源为该合约最小周期 K 线
3. 都没有时，自动补一个 1 分钟 K 线 feed，仅用于 quote 和撮合

### K 线驱动 quote

K 线驱动 quote 时：

- bar opening 用开盘价生成 quote
- bar close 后，用户可见 quote 更新为 close 对应的盘口

### 内部路径

若该 K 线是当前最小周期来源，则 broker 还应顺序消费：

- `open`
- `high`
- `low`
- `close`

对应的盘口变化，以保证 Python 同等级别的撮合能力。

### 字段范围

v1 不追求 Python 所有 quote 字段的全量镜像，只保证：

- 撮合所需字段完整
- `TargetPosTask` 所需字段完整
- 常见策略会读取的核心字段完整

## 防未来数据边界

Python 会在回测时清洗服务器返回的 quote 字段，避免把未来行情直接透传给下游。Rust 版不应把这个逻辑做成“收到后再删字段”的补丁，而应在模型边界上彻底拆开：

- `InstrumentMetadata`
  - 只包含静态合约信息
  - 例如乘数、最小变动价位、手续费、保证金、期权静态属性

- `ReplayQuote`
  - 只包含当前 replay 时间点可见的动态行情字段
  - 例如买一卖一、最新价、持仓量、成交量等

- `AuxiliaryTimelineData`
  - 只包含需要按交易日推进注入的历史辅助字段
  - 例如连续合约映射、未来的 corporate actions

强约束：

- 历史数据源返回的静态合约信息不得携带未来行情字段
- session 对外暴露的动态行情字段只能来自 quote synthesizer
- 连续合约映射和未来 corporate actions 必须按交易日推进注入，不能在 session 初始化时一次性暴露整个区间的结果

这相当于把 Python 的 `_update_valid_quotes` 语义提升为 Rust 的类型系统边界。

## 模拟账户与撮合器

Rust 版 `SimBroker` 是一等核心模块，不再使用“下单即成交”的最简 backtest stub。

### 职责

- 接收订单命令
- 根据当前 quote 路径撮合
- 更新订单/成交/持仓/账户
- 处理日切结算
- 保存每日 trade log

### 撮合规则

v1 对齐 Python futures / 商品期权的核心规则：

- 市价单按对手价成交；没有对手价则撤单
- 限价单当价格达到或超过对手价时成交；成交价为报单价
- IOC 不能立即成交则撤单
- 不支持部分成交
- 未成交订单在后续 quote 事件继续判断

### 调度方式

Python 的 `BaseSim` 采用“每合约一个 quote_handler 协程”的方式，这是它的异步管道适配方案。

Rust 不需要复制这一结构。更合理的设计是：

- 一个 session 内只有一个 `SimBroker`
- broker 内部按 `account + symbol` 分类管理状态
- quote 事件和 order 命令在 broker 内串行处理

这样能获得：

- 更强的确定性
- 更低的调度复杂度
- 更容易测试的状态机

## 日切结算与结果产出

Python 回测在交易日切换时会结算并记录账户截面。Rust 也应该保留这个能力。

v1 建议产出两类结果：

- `DailySettlementLog`
  - 每日账户快照
  - 每日持仓快照
  - 每日成交记录

- `BacktestResult`
  - 全量 trade log
  - 最终账户状态
  - 最终持仓状态

### 报表指标

Python 的 `TqReport` 和 Web GUI 图表不应成为 v1 的阻塞项。

Rust v1 建议：

- 先保证 raw log 和 settlement 数据完整
- 报表指标单独设计为后续阶段
- 不做 GUI 依赖

## 连续合约与辅助数据

你给出的 Python 分析里有两个重要辅助模块：

- `TqBacktestContinuous`
- `TqBacktestDividend`

这提示 Rust 版设计应当预留辅助数据 provider，而不是把一切写死到 kernel 里。

### v1 纳入：连续合约映射

连续合约（`KQ.m@...`）是 futures 回测的常见路径，且不引入复杂账户逻辑，因此建议 v1 纳入：

- `ContinuousContractProvider`
  - 按交易日解析 `underlying_symbol`
  - 在 session 推进跨日时更新对应元数据

### v1 不纳入：股票分红送股

分红送股主要服务 stock 回测，而 stock/ETF 不在当前 v1 范围内，因此：

- 设计预留 `CorporateActionProvider`
- v1 不实现具体股票分红逻辑

## 内存管理

Python 需要 `_gc_data()`，是因为它长期维护 diff 协议数据树，并持续向下游发送 / 删除历史 K 线。

Rust 不需要复制这个模型。

Rust 的简化方案：

- 每个 serial 只保留固定窗口
- loader 只为当前窗口和适量预取保留数据
- 内部使用 ring buffer / VecDeque / chunk cache 控制上界

这样可以用**固定窗口 + 受控预取**替代 Python 的显式 diff GC。

## Runtime 集成

为了让现有 `TargetPosTask` 能直接工作，需要 replay 版 runtime 适配器：

- `ReplayMarketAdapter`
- `ReplayExecutionAdapter`

职责：

- `ReplayMarketAdapter` 从 replay quote 快照读取行情
- `ReplayExecutionAdapter` 将下单、撤单、查询委托/持仓委托给 `SimBroker`

重要契约：

- 策略代码在 `step()` 返回后发出的新单，不会回头消费已经处理过的本 step 内部价格路径
- 新单最早从下一次 `step()` 开始参与撮合

这个契约比 Python 的 `wait_update()` 更显式，也更适合 Rust。

## 模块规划

建议新回测子系统独立放在 `src/replay/`：

- `src/replay/mod.rs`
- `src/replay/session.rs`
- `src/replay/kernel.rs`
- `src/replay/feed.rs`
- `src/replay/series.rs`
- `src/replay/quote.rs`
- `src/replay/sim.rs`
- `src/replay/runtime.rs`
- `src/replay/providers.rs`

职责：

- `session.rs`
  - public facade
  - session 生命周期
  - `step()` / `finish()`

- `kernel.rs`
  - 回测时钟
  - 多 feed 时间归并
  - 单步推进协调

- `feed.rs`
  - 历史 K 线 / Tick 装载
  - 预取和窗口推进

- `series.rs`
  - 单合约 K 线
  - Tick
  - 多合约对齐 K 线

- `quote.rs`
  - quote 源优先级
  - 自动 1 分钟线
  - bar 内价格路径生成

- `sim.rs`
  - 订单、成交、账户、结算

- `runtime.rs`
  - replay 版 market / execution adapter

- `providers.rs`
  - 连续合约 provider
  - 未来 corporate action provider

## 与当前 Rust 代码的关系

本设计不把当前已有 Rust 回测实现视为约束。

保留原则：

- 可复用已有历史数据拉取与缓存能力
- 可复用 runtime trait 边界

不作为约束：

- 当前 `BacktestHandle`
- 当前 realtime `SeriesSubscription`
- 当前简化版 `BacktestExecutionAdapter`

如果需要兼容旧接口，应在新回测系统稳定后再单独设计 legacy facade，而不是反过来让新设计受旧实现牵制。

## v1 范围

### 必做

- 全新 `ReplaySession`
- 单合约 `kline`
- 单合约 `tick`
- 多合约对齐 `aligned_kline`
- `step()` 一次推进一个市场时间点
- quote 来源优先级
- 自动补 1 分钟线
- K 线 opening/closed 语义
- broker 内部高低收路径撮合
- futures / 商品期权的基础模拟撮合
- 日切结算
- 连续合约映射 provider
- replay runtime 接 `TargetPosTask`
- 最终 trade log / settlement result

### 延后但预留

- 简单绩效指标汇总
- `run_until(...)` / `step_until(...)`
- stream/watch 风格观察接口
- replay mode（类似 Python `TqReplay`）
- corporate actions / 股票分红送股
- stock / ETF 模拟账户

### 明确不做

- Python `wait_update()` 兼容壳
- diff/DataFrame/`chart_id` 兼容
- Python Web GUI 报表链路
- 股票 / ETF / 全量期权账户体系
- 所有 Python 边角行为

## 风险与关键约束

主要风险点：

1. 多 feed 时间归并是否能保持确定性
2. 自动补 1 分钟线与显式订阅 feed 的优先级是否一致
3. bar 内部价格路径如何影响撮合，但不污染公共 API
4. 连续合约日切映射与结算时点是否一致

必须写死的系统约束：

- 一个 session 只有一个权威回测时间
- 一次 `step()` 完成一次完整提交
- 用户看不到半更新状态
- 新订单不能回头吃掉本 step 已处理过的内部路径

## 最终结论

新的 Rust 回测设计应当：

- 吸收 Python 的**虚拟行情服务器 + 本地撮合器**架构思想
- 对齐 Python 的**时间推进、quote 来源、K 线两阶段、bar 内撮合路径、日切结算**语义
- 放弃 Python 的**wait_update/diff/chart/DataFrame/GC** 脚手架

也就是说，最合适的 Rust 方案不是“兼容 Python 外壳”，而是：

**用一个强类型的 `ReplaySession + ReplayKernel + SimBroker`，复现 Python 回测的本质。**
