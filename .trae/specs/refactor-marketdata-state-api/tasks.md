# Tasks
- [x] Task 0: 在新分支/Worktree 中执行重构（工作流要求）
  - [x] 使用独立分支承载全部 BREAKING 变更与迁移（避免污染主分支）
  - [x] 分支命名建议：`refactor/marketdata-state-api`（或按团队规范）
  - [x] 需要并行开发时，优先使用 git worktree（保证与主工作区隔离）

- [x] Task 1: 明确新对外 API 形态与迁移策略（BREAKING）
  - [x] 定义 `MarketData`/`TqApi` 新入口（从 `Client` 获取或替换 `Client`）
  - [x] 定义 `QuoteRef`/`KlineRef`/`TickRef` 的最小方法集：`load()`、`is_changing()`、`wait_update()`、版本号读取
  - [x] 定义全局批量模式（可选）：`wait_update()` -> `UpdateSet` + `drain_changes()` 语义
  - [x] 输出迁移指南要点（旧 Channel/Callback → 新 Ref 模式）

- [x] Task 2: 引入高性能数据容器与符号索引
  - [x] 设计 `SymbolId`（字符串 → 索引）与订阅期的预分配策略（避免频繁 HashMap 查找）
  - [x] 为 Quote/Kline/Tick 选择并发原语：默认采用无锁读（如 `ArcSwap` 或等价方案）+ 原子版本号
  - [x] 定义序列缓冲结构（固定上限、push/update 语义、NewBar/UpdateLastBar 事件判定）

- [x] Task 3: 重构 Live 行情链路写入路径（WebSocket → Typed Store）
  - [x] 在行情消息解码后，将更新路由到 `QuoteStore`/`SeriesStore`（避免落入 `serde_json::Value` 树作为主存）
  - [x] 复用/对齐现有背压策略：合并 `rtn_data`、积压告警、丢弃最旧更新
  - [x] 对每个 symbol 的更新触发精准唤醒（Notify），并更新版本号/变更集合

- [x] Task 4: 重构 Backtest 数据推进路径（Backtest → Typed Store）
  - [x] 将回测时间推进产生的数据直接写入 Typed Store
  - [x] 保证 Live/Backtest 对外事件语义一致（同样的 Ref API，同样的 NewBar/UpdateLastBar 判定）

- [x] Task 5: 替换对外订阅接口与示例
  - [x] 替换/新增等价于 `examples/quote.rs` 的新示例（多合约订阅 + 只处理变化集合）
  - [x] 替换/新增等价于 `examples/history.rs` 的新示例（历史序列加载 + chart_ready/范围变化语义）
  - [x] 将旧的 Channel/Callback 示例迁移到 “兼容层示例” 或移除

- [x] Task 6: 性能与正确性验证
  - [x] 增加基准测试（criterion 或等价）对比：旧路径监听/回调 vs 新 Ref 模式（至少包含 Quote 更新与 K线更新）
  - [x] 增加压力测试：订阅 200/500/1000 symbol 的空策略循环 CPU 占用（以 `UpdateSet` 仅处理变化集为主）
  - [x] 增加并发正确性测试：版本号单调递增、`is_changing` 语义、不会读取到半更新状态

# Task Dependencies
- Task 3 depends on Task 2
- Task 4 depends on Task 2
- Task 5 depends on Task 1, Task 3, Task 4
- Task 6 depends on Task 3, Task 4, Task 5
