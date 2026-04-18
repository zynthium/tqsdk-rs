# tqsdk-rs 0.2 Public API Redesign RFC / Roadmap

日期：2026-04-18  
状态：维护者决策草案  
受众：仓库维护者、长期贡献者、负责 breaking release 的 agent / engineer  
范围：`src/lib.rs`、`src/prelude.rs`、`client` / `trade_session` / `replay` / `runtime` 四条主路径、相关 `types`、README / 迁移文档 / 示例 / rustdoc

> 这是一份仓库内部决策文档，不是面向下游用户的迁移指南。
> 它回答两个问题：
> 1. `tqsdk-rs` 的优秀 public contract 应该长成什么样。
> 2. 从当前状态走到 `0.2.0` 需要哪些 non-breaking 预埋和 breaking 切片。

## 1. 背景

`tqsdk-rs` 的主架构方向已经基本正确：

- live 主入口收口到 `Client`
- 交易状态与交易事件由 `TradeSession` 分层承载
- 回放 / 回测主入口收口到 `ReplaySession`
- 目标持仓与调度运行时收口到 `TqRuntime`

但从“优秀量化交易 SDK”的标准看，当前 public API 仍有几类系统性问题：

1. 最危险的交易入口仍然是 stringly-typed 协议层模型，而不是领域类型。
2. live market 与 trade readiness 语义不够原子，调用方需要自己拼状态机。
3. root / `prelude` / README / rustdoc 的 contract 叙事仍有漂移。
4. 部分类型已经被迁移文档判定为应收口，但源码和 rustdoc 仍把它们当 public surface 暴露。
5. 若干热路径 API 的 canonical 用法本身就带有不必要的 clone / alloc。
6. 构造与默认值语义中仍存在环境变量驱动的隐式行为。

`0.2.0` 的目标不是继续堆兼容 shim，而是完成一次有原则的 public contract 整理，让下游看到的 API、仓库文档承诺的 API、以及维护者愿意长期背负 semver 责任的 API 变成同一套东西。

## 2. 设计目标

### 2.1 核心目标

1. 保持四条主路径不变：`Client`、`TradeSession`、`ReplaySession`、`TqRuntime`。
2. 让高风险操作优先通过强类型 public model 表达，而不是字符串常量。
3. 让“可读”、“可等更新”、“可发单”、“已完成初始化”这些状态都具备明确 API 语义。
4. 把 root / `prelude` 压回到最小 canonical contract，不再作为“所有东西都能 import”的兜底层。
5. 让 `README.md`、`docs/architecture.md`、`docs/migration-remove-legacy-compat.md`、rustdoc 首页展示同一套主路径。
6. 为高频 live / replay 消费路径提供更低开销的读取方式。

### 2.2 非目标

- 不新增第五个主入口对象。
- 不重新引入 callback plumbing、fan-out quote stream、best-effort trade channel。
- 不在本次 redesign 中扩大回测能力边界，例如股票 replay、分红送股时间线、时间驱动 `TqReplay` 等价物。
- 不把内部 transport / kernel / adapter 重新抬回 public surface。

## 3. 设计原则

### 3.1 高风险接口必须强类型

对量化 SDK 而言，下单、撤单、目标持仓、订单价格语义、TIF / volume condition 不是普通 DTO；它们是错误成本最高的 public contract。  
`0.2.0` 必须优先让这些入口摆脱字符串常量模型。

### 3.2 session ownership 必须清晰

`Client` 是 live session owner，`ReplaySession` 是 replay session owner。  
任何 runtime、trade session、market ref 都只能围绕这两个 owner 派生，不能出现第二套等价 owner 心智模型。

### 3.3 初始化与 readiness 必须是显式状态

“句柄已经拿到了，但还不能等更新”或“connect 已完成，但还不能安全发单”这类半就绪状态可以存在，但不能靠示例里的轮询和口头约定来解释。  
如果状态存在，就必须有一等 public API 把它表达出来。

### 3.4 root / prelude 只承诺 canonical contract

crate root 和 `prelude` 不承担“方便内部人随手 import 一切”的职责。  
任何会增加 docs.rs 噪音、却不属于主路径心智模型的类型，都应该退回模块命名空间。

### 3.5 文档不是装饰，而是 release gate

只要迁移文档、README、AGENTS、CLAUDE、rustdoc 首页里有一处仍在鼓励旧 API、旧语义或旧心智模型，就说明 redesign 还没完成。

### 3.6 热路径默认用法要避免不必要开销

回放与行情消费是高频循环。  
如果 canonical 示例在每个 step / update 中都 clone 整个窗口，那这个 API 即使“可用”，也不算优秀。

## 4. 当前 public surface 的主要问题

## 4.1 Stringly-typed trading model

当前 `InsertOrderRequest` 把 `direction`、`offset`、`price_type` 暴露为 `String`，并依赖常量拼接协议值。  
这会让最危险的 public API 继续保留“无效状态可表达”的问题。

## 4.2 TradeSession readiness 不闭合

当前 `TradeSession::connect()` 并不承诺“已经可以安全读快照和发单”；示例仍需轮询 `is_ready()`。  
这说明“连接成功”与“交易就绪”不是同一个 public state，但 SDK 还没有把这个区别优雅建模。

## 4.3 Live market refs 仍保留 legacy silent-default 语义

`QuoteRef` / `KlineRef` / `TickRef` 的 `load()` 在未就绪时返回默认值，而 `SeriesSubscription::load()` 在未就绪时返回错误。  
同名 API 的语义不一致，且最危险的一侧是 silent default。

## 4.4 Root / prelude 与迁移文档冲突

迁移文档已经把若干类型标记为应退回模块命名空间，但 `src/lib.rs` / `src/prelude.rs` 仍把它们直接暴露出来。  
这会导致“文档说已经收口，代码和 rustdoc 却说还在支持”的 contract 漂移。

## 4.5 Config default 仍有隐式环境依赖

`EndpointConfig::default()` 当前等价于 `from_env()`。  
这让“默认配置”不再是纯默认值，而是当前进程环境的投影，削弱了可预期性和测试确定性。

## 4.6 Replay 读路径缺乏 cheap snapshot/view

`ReplayKlineHandle::rows()`、`ReplayTickHandle::rows()`、`AlignedKlineHandle::rows()` 都会 clone 当前窗口。  
回测示例本身也在热路径中这样用，说明目前没有更好的 canonical 读法。

## 4.7 Factory / builder 组合爆炸

`trade_session` / `trade_session_with_options` / `trade_session_with_login_options` / `trade_session_with_options_and_login` 在 `ClientBuilder` 与 `Client` 两边各有一套。  
这类 API 会不断膨胀，并让 discoverability 下降。

## 5. `0.2.0` 目标 public contract

## 5.1 Canonical entrypoints

`0.2.0` 继续明确只推荐四条入口：

- `Client`
- `TradeSession`
- `ReplaySession`
- `TqRuntime`

其余公开模块只作为命名空间存在，不作为“平行入口”存在。

## 5.2 `Client`

`Client` 的职责固定为：

- live 认证与 session owner 生命周期
- live market 初始化
- quote / kline / tick ref 获取
- live query facade
- live serial / historical download facade
- trade session factory
- replay session factory
- 派生 live runtime

明确不负责：

- 切账号
- 暴露内部 auth guard / live context
- 暴露 raw websocket / market state 容器
- 变成 `TradeSession` 或 `ReplaySession` 的同义外壳

## 5.3 `TradeSession`

`TradeSession` 的职责固定为：

- 显式连接与关闭
- 交易快照就绪状态
- 账户 / 持仓 / 委托 / 成交最新状态读取
- 订单 / 成交 / 通知 / 异步错误可靠事件流

`0.2.0` 应让调用方不再需要 busy-poll `is_ready()` 才能安全使用会话。

## 5.4 `ReplaySession`

`ReplaySession` 的职责固定为：

- 注册 replay quote / kline / tick / aligned kline 句柄
- 推进 replay 时间
- 派生 replay runtime
- 结束回放并生成原始结果

它不负责暴露 kernel、broker、provider、bootstrap 细节。

## 5.5 `TqRuntime`

`TqRuntime` 的职责固定为：

- 以账户为入口暴露目标持仓任务与调度器
- 提供受约束的手工下单入口
- 暴露与 runtime 账户绑定的 trade event 视图

它不负责暴露 adapter、registry、engine 或内部 planning primitives。

## 5.6 Root / prelude

`0.2.0` 后 root / `prelude` 只保留：

- 四条主路径类型
- 这些主路径方法签名必需的高频 DTO
- README canonical 示例直接出现的少量辅助类型

明确不再保留：

- `pub use types::*`
- 不属于 canonical contract 的 replay detail types
- 不属于 canonical contract 的 runtime builder detail types
- 仅作为 advanced namespace 更合理的 helper types

## 6. Breaking decisions

### BD-1：交易 public model 改为 typed domain model

`0.2.0` 要求把下单入口升级为 typed public model。  
推荐方向：

- `direction` 使用 enum
- `offset` 使用 enum
- `price_type + limit_price` 合并为 typed price spec
- `time_condition` / `volume_condition` 使用 enum 或 typed option

协议层字符串仍然存在，但只允许留在内部 packet builder 中。

### BD-2：TradeSession 提供一等 readiness contract

`0.1.x` 先补 `wait_ready()`；  
`0.2.0` 之后，`TradeSession::connect()` 的 public 语义应定义为“连接并进入可安全消费快照 / 发单的 ready state”，不再把握手中间态暴露给主路径用户。

`is_ready()` 可以保留为瞬时检查，但不再要求下游示例靠它轮询才能走通主路径。

### BD-3：移除 market ref 的 silent-default canonical 读法

`0.2.0` 后，`snapshot()` / `try_load()` / `is_ready()` 是唯一推荐的读法。  
`load()` 若继续保留，只能作为明确标记的 legacy convenience API，并且不能继续在 README / 示例 / rustdoc 主路径中出现。

推荐更强的做法是直接移除 `load()`，只留下显式错误或 `Option` 语义。

### BD-4：`EndpointConfig::default()` 改为纯默认值

`Default` 必须 deterministic。  
读取环境变量只能通过显式 `from_env()` 完成。

这意味着：

- `ClientBuilder::new()` 默认使用固定常量端点
- README / 示例若想支持环境覆盖，应显式调用 `EndpointConfig::from_env()`

### BD-5：收紧 root / prelude re-export

`0.2.0` 必须停止通过 wildcard 或宽泛 re-export 把整个 `types`、replay detail、runtime detail 抬到 root / `prelude`。

维护要求：

- `src/lib.rs` 只手写导出 canonical root surface
- `src/prelude.rs` 只手写导出高频主路径组合
- rustdoc 首页列出的类型必须可被维护者长期背书

### BD-6：回放句柄补 cheap snapshot/view API

`0.2.0` 至少为 replay 句柄提供一种低开销读取方式，例如：

- cheap snapshot (`Arc<[T]>` / `Arc<Vec<T>>`)
- `latest()` / `last_two()`
- 只读 view helper

`rows()` 可以保留为 convenience clone API，但不能继续作为 canonical 示例热路径。

### BD-7：合并 trade session factory 组合爆炸

`0.2.0` 把 `TradeSession` 构造入口收成单一 builder/config 模式。  
目标不是再引入新 owner，而是把 options / login 扩展都收进单一配置对象或 builder。

## 7. `0.1.x` additive 预埋

以下工作应在 `0.2.0` 前分批以 non-breaking 方式落地：

1. 为 live market 增加显式初始化检查 helper。
2. 为 `TradeSession` 增加 `wait_ready()` 并更新 README / 示例。
3. 为 live runtime 增加“已连接 trade sessions”的显式构造入口。
4. 并行引入 typed trading builder / typed order model，作为 `0.2.0` 的迁移垫片。
5. 在 replay handle 上新增 cheap snapshot/view API。
6. 把迁移文档、README、AGENTS、CLAUDE 与现状重新对齐，先消除错误叙事。

这些 additive 工作的目的不是长期保留双轨，而是降低 `0.2.0` 的切换成本。

## 8. `0.2.0` breaking roadmap

## Phase A：Contract freeze

目标：在发版前锁定最终 public surface 清单。

输出：

- root export 白名单
- `prelude` export 白名单
- `TradeSession` readiness 语义最终文本
- typed trading model 最终 public shape

release gate：

- `README.md`
- `docs/architecture.md`
- `docs/migration-remove-legacy-compat.md`
- `AGENTS.md`
- `CLAUDE.md`
- rustdoc 首页

六处必须一致。

## Phase B：Core break slice

目标：完成最关键的 contract break。

范围：

- trading typed model
- `TradeSession` readiness contract
- root / `prelude` contraction
- deterministic endpoint default
- replay cheap snapshot API 进入示例主路径

要求：

- 不引入长期 deprecated shim
- 示例与文档在同一 PR 或同一 release branch 内同步完成

## Phase C：Surface cleanup

目标：删除已经有替代路径的 legacy public surface。

候选：

- legacy `load()`
- 常量驱动的 order request 写法
- 宽泛 root / `prelude` re-export
- 假 public / 假 advanced helper type

原则：

- 只删除已经在 `0.1.x` 或 release notes 中提供明确迁移写法的内容
- 不在这个阶段再混入新功能

## 9. 文档同步规则

每次触碰 public contract，都必须同时检查：

- `README.md`
- `docs/architecture.md`
- `docs/migration-remove-legacy-compat.md`
- `AGENTS.md`
- `CLAUDE.md`
- `examples/`
- `skills/tqsdk-rs/`（如果本次改动影响 agent 生成代码的 canonical 路径）

只改代码不改这些文档，视为 redesign 未完成。

## 10. Verification matrix

### 10.1 Contract verification

- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo check --examples`
- `cargo doc --no-deps`

### 10.2 Surface verification

需要额外检查：

- rustdoc 首页是否仍在展示已判定应收口的类型
- `src/lib.rs` 与 `src/prelude.rs` 的导出是否仍与 migration doc 冲突
- README 示例是否仍在使用被降级或 legacy 的 API

### 10.3 Behaviour verification

对于关键 redesign slice，必须补针对 public contract 的测试：

- `TradeSession` ready / wait_ready / connect contract
- typed order model 到 wire packet 的映射
- replay cheap snapshot API 的零额外语义回归
- endpoint default 不再读取环境变量

## 11. 维护者决策摘要

`0.2.0` 的核心不是“删掉一些 export”，而是完成三件更重要的事：

1. 让最危险的交易接口变成强类型。
2. 让 live trade / live market 的 readiness contract 可直接使用，而不是靠示例暗示。
3. 让 root / `prelude` / README / rustdoc 只描述同一套 canonical API。

如果这三件事没有同时完成，即使表面上完成了若干 export contraction，也不算一次成功的 public API redesign。

## 12. 建议执行顺序

推荐按下面顺序推进：

1. 文档与 rustdoc contract 对齐
2. additive `wait_ready()` / market init helper / connected runtime path
3. additive typed trading path
4. replay cheap snapshot API
5. `0.2.0` root / `prelude` contraction 与 legacy 删除

原因：

- 先修叙事，再修行为，再做 break，返工最少。
- typed trading model 是价值最高、风险也最高的 redesign，应该在 break 前让调用方先看到稳定替代路径。
- replay 读路径优化虽然不一定最先 break，但它直接影响“示例是否鼓励坏用法”，应在 `0.2.0` 前完成。
