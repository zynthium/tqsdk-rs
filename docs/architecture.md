# 架构说明

本文档提供 `tqsdk-rs` 的模块视图与职责速查，方便在阅读代码、评估变更影响或设计新能力时快速建立全局认识。

如果你是要在仓库里执行修改，请优先阅读根目录的 [AGENTS.md](../AGENTS.md)；它包含更适合 agent 执行的验证、边界与提交流程要求。

## 总体分层

```text
Client (facade + builder + market)
├── runtime/       统一任务运行时
│   ├── core       `TqRuntime`（账户句柄、模式、adapter 装配）
│   ├── account    `AccountHandle`（按账户派生任务入口）
│   ├── market     `MarketAdapter`（Quote / trading_time 读取）
│   ├── execution  `ExecutionAdapter`（live/backtest 下单执行）
│   ├── registry   `TaskRegistry`（symbol 所有权、手工下单保护、order owner）
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
│   ├── core       创建 + epoch 订阅 + legacy callback 注册
│   ├── merge      递归合并 (prototype 语义)
│   ├── query      路径查询 + 数据转换
│   └── watch      路径监听
├── cache/         本地缓存
│   └── kline      K线分段磁盘缓存（写锁、去重、区间读取、压缩）
├── quote/         Quote 订阅 (回调 + channel 双模式)
│   ├── lifecycle  订阅生命周期 (start/stop)
│   └── watch      数据变更监听
├── series/        K线/Tick 订阅 (单合约 + 多合约对齐)
│   ├── api        SeriesAPI (K线/Tick 请求)
│   ├── subscription 订阅管理
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
├── backtest/      回测控制
│   ├── core       初始化 + 时间推进 + 事件分发 + 回调清理
│   └── parsing    回测时间状态解析
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
| `websocket` | `TqWebsocket` | 底层连接、重连、消息分发 |
| `datamanager` | `DataManager` | DIFF 合并、版本追踪、路径监听 |
| `runtime` | `TqRuntime`, `AccountHandle`, `TargetPosHandle`, `TargetPosSchedulerHandle` | 统一任务运行时、目标持仓调度、任务所有权与执行 adapter |
| `cache` | `DataSeriesCache` | 与 Python 官方兼容的 K线/Tick 历史快照缓存、范围扫描、文件合并与并发写保护 |
| `quote` | `QuoteSubscription` | 行情订阅 (回调/channel) |
| `series` | `SeriesAPI`, `SeriesSubscription` | K线/Tick 订阅 |
| `ins` | `InsAPI` | 合约查询、期权筛选、结算价、排名、EDB、交易日历、交易状态 |
| `trade_session` | `TradeSession` | 交易操作 (下单/撤单/查询) |
| `backtest` | `BacktestHandle`, `BacktestConfig`, `BacktestEvent`, `BacktestTime` | 回测时间推进与事件 |
| `polars_ext` | `KlineBuffer`, `TickBuffer`, `EdbBuffer`, `RankingBuffer`, `SettlementBuffer` | DataFrame 转换 (可选) |
| `prelude` | — | 便捷 re-export |

## 关键设计模式

- I/O actor：WebSocket 读写通过单所有者 actor 隔离，避免跨 `await` 持锁。
- DIFF 合并：`DataManager` 负责递归 merge、默认值补齐、路径监听与查询；merge 完成通知优先使用 `subscribe_epoch()`。
- 延迟启动：`QuoteSubscription`、`SeriesSubscription` 创建后需显式 `start()`。
- 背压控制：多个消费通道已改为有界缓冲，慢消费者场景下允许丢弃旧更新。
- 重连完整性：重连阶段通过临时缓冲校验数据，再合并回主状态。
- 任务所有权：`TaskRegistry` 保证同一 runtime/account/symbol 的目标持仓任务唯一，并阻止冲突的手工下单。
- 执行解耦：`TargetPosTask` / `TargetPosScheduler` 复用相同任务逻辑，只通过 `ExecutionAdapter` / `MarketAdapter` 切换 live 与 backtest 行为。
- 订阅生命周期：`InsAPI` 的交易状态订阅按 symbol 做引用计数，receiver 释放后会自动回收订阅意图。
- 交易状态分层：`TradeSession` 以 DataManager epoch 驱动内部 watcher，再用 path epoch 区分账户/持仓快照与可靠订单/成交事件。
- 回测生命周期：`BacktestHandle` 释放时会注销内部 DataManager 回调，避免长会话回调累积。
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
