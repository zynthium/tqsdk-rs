# 架构说明

本文档提供 `tqsdk-rs` 的模块视图与职责速查，方便在阅读代码、评估变更影响或设计新能力时快速建立全局认识。

如果你是要在仓库里执行修改，请优先阅读根目录的 [AGENTS.md](../AGENTS.md)；它包含更适合 agent 执行的验证、边界与提交流程要求。

> Canonical public contract:
> 当前公开主路径收口为 `Client`、`TradeSession`、`ReplaySession`、`TqRuntime`
> 四条入口。迁移背景与已删除 surface 见 `docs/migration-remove-legacy-compat.md`。

## 总体分层

```text
Client (facade + builder + market)
├── runtime/       统一任务运行时
│   ├── core       `TqRuntime`（公开运行时句柄；adapter 装配收口为 crate 内部）
│   ├── account    `AccountHandle`（按账户派生任务入口）
│   ├── market     runtime 行情适配（crate 内部）
│   ├── execution  runtime 执行适配（crate 内部）
│   ├── registry   task 唯一性 / 手工下单保护（crate 内部）
│   ├── tasks      `TargetPosTask` / `TargetPosScheduler` / child order runner
│   ├── modes      live/backtest 模式装配
│   └── types      运行时配置与错误
├── auth/          认证 (Authenticator trait → TqAuth)
│   ├── core       TqAuth 实现 (登录/token 获取)
│   ├── token      JWT 解析 + claims 校验
│   └── permissions 权限检查（含指数权限分支）
├── websocket/     WebSocket 连接层
│   ├── core       TqWebsocket 基类 (I/O actor 模式)
│   ├── quote      TqQuoteWebsocket (行情通道)
│   ├── trade      TqTradeWebsocket (交易通道)
│   ├── trading_status  TqTradingStatusWebsocket
│   ├── backpressure    消息背压 + rtn_data 合并
│   ├── reconnect       重连逻辑 + 数据完整性校验
│   └── message         通知构建 + 日志脱敏
├── datamanager/   DIFF 协议数据管理
│   ├── core       创建 + epoch 订阅
│   ├── merge      递归合并 (prototype 语义)
│   ├── query      路径查询 + 数据转换
│   └── watch      路径监听
├── cache/         本地缓存
│   └── kline      K线分段磁盘缓存（写锁、去重、区间读取、压缩）
├── quote/         Quote 订阅生命周期控制
│   └── lifecycle  订阅生命周期 (start/stop/add/remove)
├── series/        K线/Tick 窗口订阅 + 历史快照下载
│   ├── api        SeriesAPI (K线/Tick 请求 + one-shot 历史下载)
│   ├── subscription 生命周期 + snapshot 等待/读取
│   └── processing   数据处理
├── ins/           合约查询与基础数据
│   ├── query      GraphQL 查询 + 合约列表/期权筛选
│   ├── services   结算价/持仓排名/EDB/交易日历 (HTTP)
│   ├── trading_status 交易状态订阅（按 symbol 引用计数聚合）
│   ├── parse      查询结果解析
│   └── validation 参数校验 + 缓存匹配
├── trade_session/ 交易会话
│   ├── core       创建 + 登录 + watcher 生命周期
│   ├── ops        下单/撤单/查询持仓/账户/成交
│   └── watch      监听 DataManager epoch，再按 path epoch 生成 snapshot / reliable event
├── polars_ext/    Polars DataFrame 转换 (可选 feature)
│   ├── kline      KlineBuffer (K线 → DataFrame)
│   ├── tick       TickBuffer (Tick → DataFrame)
│   ├── tabular    EdbBuffer / RankingBuffer / SettlementBuffer
│   └── series     SeriesData → DataFrame 转换
├── types/         数据结构 (market/trading/series/query/helpers)
├── prelude        便捷 re-export (常用类型一次导入)
├── errors         TqError 枚举
├── logger         tracing 日志初始化
└── utils          HTTP 下载 / 时间转换 / 合约解析
```

## 模块职责速查

| 模块 | 入口类型 | 职责 |
|------|---------|------|
| `client` | `Client`, `ClientBuilder`, `ClientConfig`, `ClientOption` | 统一入口，管理生命周期 |
| `auth` | `Authenticator` trait, `TqAuth` | 登录、token 解析与 claims 校验、权限检查 |
| `websocket` | internal transport module | 底层连接、重连、消息分发；不作为推荐 public entry point |
| `datamanager` | `DataManager` | DIFF 合并、版本追踪、路径监听 |
| `runtime` | `TqRuntime`, `AccountHandle`, `TargetPosBuilder`, `TargetPosSchedulerBuilder` | 统一任务运行时与 Builder 任务入口；adapter/registry/planning primitives 属于内部装配 |
| `cache` | `DataSeriesCache` | 与 Python 官方兼容的 K线/Tick 历史快照缓存、范围扫描、文件合并与并发写保护 |
| `quote` | `QuoteSubscription` | 仅负责 Quote 生命周期控制；状态读取走 `Client::quote()` |
| `series` | `SeriesAPI`, `SeriesSubscription` | 窗口态 K线/Tick 订阅，使用 `wait_update()` / `load()` 读取快照 |
| `ins` | `InsAPI` | 合约查询、期权筛选、结算价、排名、EDB、交易日历、交易状态 |
| `trade_session` | `TradeSession` | 交易操作 (下单/撤单/查询) |
| `polars_ext` | `KlineBuffer`, `TickBuffer`, `EdbBuffer`, `RankingBuffer`, `SettlementBuffer` | DataFrame 转换 (可选) |
| `prelude` | — | 便捷 re-export |

## Canonical Contract

- live 主路径已收口到 `Client`：`Client` 是一次 live session owner，对齐 Python `TqApi` 的会话模型；行情状态读取、序列订阅和 query facade 都直接挂在 `Client`，`TqApi`、`SeriesAPI`、`InsAPI` 不再出现在 crate root / prelude / README 主路径。
- `ClientBuilder` / `ClientConfig` / `TqAuth` 是构造侧概念；不再引入 public `LiveClient` / `LiveSession` 双对象。当前内部 live 资源统一收敛在私有 `LiveContext`。
- Quote / Series 继续坚持状态驱动，不重新引入 Stream fan-out；当前已经是创建即生效。
- `TradeSession` 继续按“状态 vs 事件”分层：
  账户/持仓是 snapshot getter + `wait_update()`。
  订单/成交/通知/异步错误是可靠事件流。
- `ReplaySession` 与 `TqRuntime` 保持独立主路径，不为了 live API 对称性而重新缠回 `Client`。

## Replay Capability Boundary

- 当前 production replay 聚焦期货 / 商品期权回放，已覆盖显式 `ReplaySession` 入口、一步一推进的时间语义、隐式 quote driver、日切结算，以及 runtime / `TargetPosTask` 集成。
- replay 内核现在支持辅助时间线：`HistoricalSource::load_auxiliary_events()` 可按交易日提供辅助事件，`ReplaySession` 会在建立或切换交易日时应用这些 patch。当前已用于主连 `underlying_symbol` 日切。
- 当前仓库仍没有默认的“历史主连映射”上游抓取路径；`Client::create_backtest_session()` 所使用的 `SdkHistoricalSource` 目前不会自动拉取历史 continuous mapping，因此这项能力现阶段主要面向可注入 / 可测试的 historical source。
- 当前明确不覆盖：股票回放 / T+1 股票模拟、分红送股时间线注入、时间驱动的 `TqReplay` 等价物、GUI report / chart 渲染。
- 当前 replay 已提供纯后处理指标层：`ReplaySession::finish()` 产出原始 `BacktestResult`，`ReplayReport::from_result(&result)` 在其上计算 Python 对齐 headline metrics。

## 关键设计模式

- I/O actor：WebSocket 读写通过单所有者 actor 隔离，避免跨 `await` 持锁。
- DIFF 合并：`DataManager` 负责递归 merge、默认值补齐、路径监听与查询；merge 完成通知优先使用 `subscribe_epoch()`。
- `QuoteSubscription`、`SeriesSubscription` 已改为 auto-start，只保留 `close()` 作为提前释放资源接口。
- 状态读取与订阅控制分离：Quote 由 `QuoteSubscription` 管订阅生命周期，`QuoteRef` 负责读取最新状态。
- 窗口状态读取：`SeriesSubscription` 监听 DataManager epoch，并通过 coalesced `SeriesSnapshot` 暴露多合约对齐窗口状态。
- serial 收口：`Client::{get_kline_serial,get_tick_serial}` 走更新中的 bounded sequence 路径，`data_length` 会归一化到 `1..=10000`。
- 历史下载收口：`Client::{get_kline_data_series,get_tick_data_series}` 走 one-shot 下载路径，内部复用分页 `set_chart` 协议，并在入口检查 `tq_dl`。
- 背压控制：多个消费通道已改为有界缓冲，慢消费者场景下允许丢弃旧更新。
- 重连完整性：重连阶段通过临时缓冲校验数据，再合并回主状态。
- transport 收口：`TqWebsocket`、`TqQuoteWebsocket`、`TqTradeWebsocket` 等原始连接拼装件保持 crate 内部，外部统一从 `Client` / `TradeSession` / `ReplaySession` 进入。
- live owner：当前由 `Client` 私有 `LiveContext` 统一拥有 `DataManager`、`MarketDataState`、quote websocket、`SeriesAPI`、`InsAPI` 和 close signal；runtime live adapter 复用同一 context，不能自建第二套行情 websocket / state。
- mode transition：live context 重建属于 `Client` 私有实现细节，不再作为公开 API 暴露，也不承担 re-auth 或切账号职责。
- 任务所有权：`TaskRegistry` 保证同一 runtime/account/symbol 的目标持仓任务唯一，并阻止冲突的手工下单。
- 执行解耦：`TargetPosTask` / `TargetPosScheduler` 复用相同任务逻辑，只通过 `ExecutionAdapter` / `MarketAdapter` 切换 live 与 replay runtime 行为。
- 公开入口收口：运行时的公开表面聚焦在 `TqRuntime`、`AccountHandle` 和 Builder 任务类型；adapter、registry、child-order planning 等拼装件保持 crate 内部。
- 回测入口收敛：公开回测路径统一通过 `Client::create_backtest_session` 构造 `ReplaySession`，不再维持独立 `BacktestHandle` facade。
- 回放实现收口：`ReplayKernel`、quote 合成器、`SimBroker` 等回放拼装件属于内部实现细节；公开回测入口聚焦在 `ReplaySession` 与返回的 handles/result。
- 结果与报告分层：`SimBroker` / `ReplaySession` 只负责产生原始 `BacktestResult`；`ReplayReport` 保持为独立后处理层，避免把 analytics policy 倒灌回回放执行内核。
- replay 辅助时间线：`HistoricalSource` 可以提供按交易日排序的辅助事件，`ReplaySession` 在交易日建立 / 切换时原子应用这些 patch；这避免把主连切换或其他未来 replay 扩展重新塞回 quote snapshot plumbing。
- 订阅生命周期：`InsAPI` 的交易状态订阅按 symbol 做引用计数，receiver 释放后会自动回收订阅意图。
- 交易状态分层：`TradeSession` 以 DataManager epoch 驱动内部 watcher，再用 path epoch 区分账户/持仓快照与可靠订单/成交事件。
- breaking 目标：`TradeSession` 的通知与异步错误同样并入可靠事件流；账户/持仓相关 callback/channel 不再保留。
- 初始化鲁棒性：日志层与磁盘缓存初始化优先降级和告警，而不是库级 `panic`。
- 缓存治理：Series 磁盘缓存默认关闭；开启后写入 `~/.tqsdk/data_series_1`，并支持按总容量上限清理、按保留天数清理。

## 阅读建议

- 想理解外部 API：先看 `src/lib.rs`、`src/client.rs`、`src/client/`
- 想理解状态模型：看 `src/datamanager/`
- 想理解目标持仓与新任务运行时：看 `src/runtime/`
- 想理解历史 K 线 / Tick 磁盘复用：看 `src/cache/data_series.rs` 与 `src/series/api.rs`
- 想理解实时链路：看 `src/websocket/`、`src/quote/`、`src/series/`
- 想理解查询接口：看 `src/ins/`
- 想理解交易链路：看 `src/trade_session/`
- 想理解可选分析能力：看 `src/polars_ext/`
