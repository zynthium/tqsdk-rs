# tqsdk-rs 公开接口收口方案

日期：2026-04-17
状态：已评审草案，待用户审阅
范围：`src/lib.rs`、`src/prelude.rs`、公开模块边界、README canonical 路径、相关示例与迁移文档

## 1. 背景

当前 `tqsdk-rs` 的主路径设计已经基本收口到 `Client`、`TradeSession`、`ReplaySession`、`TqRuntime` 四条主线，但公开接口仍存在以下问题：

- crate root / `prelude` 的导出面与 README 主路径不完全一致。
- 部分 `pub fn` / `pub async fn` 的签名仍泄漏内部 `marketdata` 类型，导致 public contract 不闭合。
- `SeriesAPI`、`InsAPI` 以 public type 暴露，但下游无法稳定获取实例，形成“假 public surface”。
- `ClientBuilder::build()` 带有全局日志初始化副作用，不符合库类型 API 的最小越权原则。
- 一些高级能力虽然对外可见，但既不是 canonical 路径，也没有被明确声明为稳定 advanced path。

本方案目标不是继续堆叠兼容层，而是一次性明确：

- 什么是 crate 的 canonical public contract。
- 什么是有限支持的 module-level advanced surface。
- 什么必须退回内部实现。

## 2. 设计目标

### 2.1 核心目标

1. 让 crate root、`prelude`、README、示例四者的主路径完全一致。
2. 让所有公开方法的参数与返回值都来自稳定可命名的 public 类型。
3. 消除“公开了名字但拿不到实例”的表面开放接口。
4. 隐藏 transport / state / assembly 内部细节，避免未来 semver 负担继续扩大。
5. 去除构造期全局副作用，让日志初始化成为显式 opt-in 行为。

### 2.2 非目标

- 不重写 live / replay / runtime 架构。
- 不把所有能力都重新塞回单一超大 facade。
- 不重新引入旧版 callback/channel 风格 public API。
- 不在本设计中新增与当前 public contract 无直接关系的新功能。

## 3. 公开接口分层

本 crate 的公开面收口为三层。

### 3.1 Root Canonical

crate root 与 `prelude` 只暴露 README 主路径推荐直接使用的能力。

保留的典型类型：

- `Client`
- `ClientBuilder`
- `ClientConfig`
- `EndpointConfig`
- `TradeSessionOptions`
- `QuoteSubscription`
- `SeriesSubscription`
- `TradeSession`
- `TradeSessionEvent`
- `TradeSessionEventKind`
- `TradeEventRecvError`
- `ReplaySession`
- `ReplayConfig`
- `TqRuntime`
- `AccountHandle`
- `TargetPosTask`
- `TargetPosScheduler`
- `TargetPosScheduleStep`
- `TargetPosConfig`
- `OrderDirection`
- `PriceMode`
- `OffsetPriority`
- `VolumeSplitPolicy`
- `Result`
- `TqError`
- canonical 主路径会直接消费的 market / trade / replay / series 数据结构

原则：

- README 示例中直接出现的类型，必须能从 crate root 或 `prelude` 稳定命名。
- `prelude` 只服务“高频策略用法”，不是第二个不受控 root。
- root canonical 只导出稳定承诺面，不为“可能将来有用”保留冗余类型。

### 3.2 Module Advanced

仅保留确有独立价值、且下游可真实使用的高级模块能力。

建议保留：

- `datamanager`
- `download`
- `types`
- `auth`
- `runtime` 中明确打算长期支持的 typed config / error
- `replay` 中的回测句柄与结果类型

原则：

- 类型公开，必须有稳定获取路径。
- 模块公开，必须有明确使用语义。
- 如果某模块只是内部装配细节的残留出口，应收回内部。

### 3.3 Internal Hidden

以下能力不再属于 public contract：

- `marketdata`
- `websocket`
- client / runtime / replay 的内部 adapter
- replay kernel / sim broker / providers / synthesizer
- 仅用于内部拼装的 state carrier、registry、engine

原则：

- 内部模块不能通过 public 方法签名泄漏到下游。
- 不能依赖 `cfg(clippy)` 这类手段在检查态与发布态切换可见性。

## 4. 模块级取舍

### 4.1 `client`

`Client` 继续作为 live 单入口。

保留：

- live 行情初始化
- quote / series / query facade
- trade session factory
- replay session factory
- runtime 转换入口

约束：

- `Client::quote()`、`kline_ref()`、`tick_ref()` 若继续保留，则签名中的参数和返回值必须全部位于稳定 public 类型层。
- `wait_update_and_drain()` 若继续暴露，其返回类型也必须是 crate root 可稳定命名的 public struct。

设计要求：

- 不再让 `Client` 签名依赖隐藏 `marketdata` 模块中的专有类型。
- 不新增新的 live session facade 同义对象。

### 4.2 `trade_session`

`TradeSession` 继续保持“两层模型”：

- 账户/持仓：最新状态读取 + `wait_update()`
- 订单/成交/通知/异步错误：可靠事件流

必须补齐：

- crate root / `prelude` re-export `TradeSessionEvent`

原因：

- `TradeEventStream::recv()` 的真实返回值就是 `TradeSessionEvent`
- 如果 root canonical 推荐事件流用法，就必须保证事件对象本体也属于 canonical contract

### 4.3 `series`

`SeriesSubscription` 保持 canonical。

`SeriesAPI` 的处理策略：

- 默认方案：收回内部，不再作为 public type 暴露。
- 仅当项目明确承诺 advanced accessor 时，才保留为公开类型，并增加稳定入口。

推荐选择：收回内部。

原因：

- 当前 README 将主路径收口到 `Client::{get_kline_serial,get_tick_serial,...}`
- 下游拿不到稳定 `SeriesAPI` 实例
- 保留 public type 只会扩大未来 semver 维护面

### 4.4 `ins`

`InsAPI` 采取与 `SeriesAPI` 相同策略：

- 默认收回内部
- 查询能力继续通过 `Client` facade 暴露

推荐选择：收回内部。

原因：

- 当前 canonical 语义已经是“查询走 `Client`”
- `InsAPI` 当前属于“可见但不可达”的假 public surface

### 4.5 `replay`

保留以下公开能力：

- `ReplaySession`
- `ReplayQuoteHandle`
- `ReplayKlineHandle`
- `ReplayTickHandle`
- `AlignedKlineHandle`
- `ReplayStep`
- `BacktestResult`
- `BarState`
- `ReplayHandleId`
- `ReplayQuote`

收回以下内部件：

- replay kernel
- sim broker
- quote synthesizer
- provider / bootstrap 内部装配细节

### 4.6 `runtime`

保留公开：

- `TqRuntime`
- `AccountHandle`
- `TargetPosTask`
- `TargetPosScheduler`
- `TargetPosScheduleStep`
- `TargetPosConfig`
- `OrderDirection`
- `PriceMode`
- `OffsetPriority`
- `VolumeSplitPolicy`
- `RuntimeError`
- `RuntimeResult`

继续内部化：

- `ExecutionAdapter`
- `MarketAdapter`
- `ExecutionEngine`
- `TaskRegistry`
- 仅为内部执行器协作服务的底层组件

## 5. 导出规则

### 5.1 `src/lib.rs`

职责限定为：

1. 声明真正需要 public 的模块
2. re-export canonical root surface
3. 提供与 README 一致的 crate-level 使用示例

禁止：

- 用 `cfg(clippy)` 修改发布态 public surface
- 保留已不准备支持的历史 façade 出口
- 通过 re-export 暴露内部 transport/state 细节

### 5.2 `src/prelude.rs`

职责限定为：

- 高频主路径类型的简洁导入集合

禁止：

- advanced surface 全量搬运
- 把 `prelude` 变成“所有东西都能 import 到”的兜底层

建议保留：

- `Client`
- `TradeSession`
- `ReplaySession`
- `TqRuntime`
- `QuoteSubscription`
- `SeriesSubscription`
- `TradeSessionEvent`
- `TradeSessionEventKind`
- `TradeEventRecvError`
- `TargetPos*`
- 常用错误与结果类型

## 6. 日志策略调整

当前问题：

- `ClientBuilder::build()` 无条件初始化全局 `tracing` subscriber。

目标行为：

- 库默认不抢占宿主应用的全局日志配置。
- 用户若需要库级日志，显式调用 `init_logger()` 或组合 `create_logger_layer()`。

方案：

- 将 `ClientBuilder::build()` 中的全局日志初始化移除。
- 保留 `init_logger()` 和 `create_logger_layer()` 作为显式 opt-in 工具。
- README 和示例同步说明：“如需 SDK 日志，请手动初始化”。

兼容性说明：

- 这会改变“第一次 build 自动打开日志”的隐含行为。
- 该变化应作为 breaking behavior change 进入迁移说明。

## 7. 签名闭合规则

为防止再次出现 hidden type leak，建立以下规则：

1. 任意 `pub fn` / `pub async fn` 的参数类型必须可从稳定 public surface 命名。
2. 任意 `pub fn` / `pub async fn` 的返回类型必须可从稳定 public surface 命名。
3. 不能依赖 lint 特判来维持“看起来 public”。
4. 如果某类型只是某个方法的技术性实现细节，不应出现在 public 签名里。

建议将此规则视为仓库级 API 审查标准。

## 8. 迁移策略

### 8.1 发布策略

建议按一次明确 breaking slice 处理。

如果项目仍处于 `0.x`：

- 建议提升到下一次明确 breaking 的 minor 版本
- 例如 `0.1.x -> 0.2.0`

### 8.2 两阶段落地

第一阶段：contract 修补

- 去掉 `cfg(clippy)` 可见性分叉
- 修复 hidden type leak
- 补齐 root / `prelude` re-export
- 移除构造期日志副作用

第二阶段：surface 收口

- 删除或内部化 `SeriesAPI`
- 删除或内部化 `InsAPI`
- 审查 `runtime` / `replay` 的多余 `pub use`
- 同步 README、示例、迁移文档

优点：

- 将“修错”与“删面”拆开验证
- 降低一次性改动面带来的回归风险

### 8.3 过渡策略

若需要平滑迁移，可在一个过渡版本中：

- 保留极少数旧入口
- 显式 `#[deprecated]`
- 从 README canonical 路径中移除
- 在 `docs/migration-remove-legacy-compat.md` 提供替代写法

不建议：

- 继续保留“实际不推荐、也不稳定”的假 public surface

## 9. 文档与示例同步

本方案落地时必须同步：

- `README.md`
- `src/lib.rs` 顶层文档示例
- `src/prelude.rs`
- `examples/`
- `AGENTS.md`
- `CLAUDE.md`
- `docs/migration-remove-legacy-compat.md`（如涉及删除/替换 public surface）
- `skills/tqsdk-rs/`（如当前仓库有与 public shape 绑定的知识包）

同步原则：

- README 怎么教，root/prelude 就必须怎么用。
- 示例怎么写，root canonical 就必须如何导出。
- agent 规则怎么描述，代码 surface 就必须如何成立。

## 10. 验证矩阵

本方案落地后至少验证：

- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo check --examples`

针对 API contract 追加人工检查：

- README 中所有 canonical 示例是否只依赖 canonical surface
- `prelude::*` 是否足以支撑 README 中的“推荐写法”
- 所有 public 方法签名是否引用稳定 public 类型
- 是否仍存在“public type 但拿不到实例”的模块类型

## 11. 预期结果

方案完成后，crate 对外将形成一个更清晰的 contract：

- live 主路径统一收口到 `Client`
- 交易主路径统一收口到 `TradeSession`
- 回放/回测统一收口到 `ReplaySession`
- 目标持仓运行时统一收口到 `TqRuntime`
- root / `prelude` 只承载 canonical API
- advanced module 有明确边界
- internal implementation 不再进入 semver 承诺面

这会让后续版本迭代更容易做到：

- README 与代码一致
- 示例与 public contract 一致
- agent 与文档不会继续生成旧 surface
- 下游用户更容易理解哪些类型值得依赖，哪些不应触碰

## 12. 实施摘要

建议执行顺序如下：

1. 定义最终 canonical 清单
2. 重写 `lib.rs` / `prelude.rs` 导出面
3. 修复所有 hidden type leak
4. 内部化 `SeriesAPI` / `InsAPI`
5. 清理 `runtime` / `replay` 冗余 public surface
6. 移除构造期日志副作用
7. 同步 README / 示例 / 迁移文档 / agent 文档
8. 运行验证矩阵

本设计文档只定义收口方向、边界与迁移策略；具体代码改动顺序与任务拆解在后续 implementation plan 中展开。
