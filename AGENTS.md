# AGENTS.md — tqsdk-rs

## 项目概述

天勤 DIFF 协议的 Rust SDK，连接 Shinnytech 量化交易平台。支持实时行情订阅（Quote/K线/Tick）、历史数据获取、实盘/模拟交易、回测。

## 技术栈

- Rust 2024 edition
- 异步运行时: tokio (full features)
- WebSocket: yawc (zlib deflate)
- HTTP: reqwest (gzip/brotli/stream)
- JSON: serde + serde_json
- JWT: jsonwebtoken (insecure decode)
- 日志: tracing + tracing-subscriber
- 错误处理: thiserror
- 可选: polars (数据分析)

## 架构

```
Client (facade + builder + market)
├── auth/          认证 (Authenticator trait → TqAuth)
│   ├── core       TqAuth 实现 (登录/token 刷新)
│   ├── token      JWT 解析 (insecure decode)
│   └── permissions 权限检查
├── websocket/     WebSocket 连接层
│   ├── core       TqWebsocket 基类 (I/O actor 模式)
│   ├── quote      TqQuoteWebsocket (行情通道)
│   ├── trade      TqTradeWebsocket (交易通道)
│   ├── trading_status  TqTradingStatusWebsocket
│   ├── backpressure    消息背压 + rtn_data 合并
│   ├── reconnect       重连逻辑 + 数据完整性校验
│   └── message         通知构建 + 日志脱敏
├── datamanager/   DIFF 协议数据管理
│   ├── core       创建 + 回调注册
│   ├── merge      递归合并 (prototype 语义)
│   ├── query      路径查询 + 数据转换
│   └── watch      路径监听 (async_channel)
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
│   ├── trading_status 交易状态订阅
│   ├── parse      查询结果解析
│   └── validation 参数校验 + 缓存匹配
├── trade_session/ 交易会话
│   ├── core       创建 + 登录 + 回调注册
│   ├── ops        下单/撤单/查询持仓/账户/成交
│   └── watch      数据变更监听 (DM → channel/callback)
├── backtest/      回测控制
│   ├── core       初始化 + 时间推进 + 事件分发
│   └── parsing    回测时间状态解析
├── polars_ext/    Polars DataFrame 转换 (可选 feature)
│   ├── kline      KlineBuffer (K线 → DataFrame)
│   ├── tick       TickBuffer (Tick → DataFrame)
│   ├── tabular    EdbBuffer / RankingBuffer / SettlementBuffer
│   └── series     SeriesData → DataFrame 转换
├── types/         数据结构 (market/trading/series/query/helpers)
├── prelude        便捷 re-export (常用类型一次导入)
├── errors         TqError 枚举 (thiserror)
├── logger         tracing 日志初始化
└── utils          HTTP 下载 / 时间转换 / 合约解析
```

## 关键设计模式

- **I/O Actor**: WebSocket 读写通过单所有者 actor 隔离，避免跨 await 持锁
- **背压队列**: 消息处理慢时 backlog 缓冲 + rtn_data 合并 + 有界丢弃
- **DIFF 合并**: 递归 merge 支持 prototype 语义 (`*`/`@`/`#` 分支)、reduce_diff、persist
- **延迟启动**: QuoteSubscription/SeriesSubscription 创建后需显式 `start()` 才开始接收
- **重连完整性**: 重连时用临时 DataManager 缓冲数据，校验完整后一次性合并到主 DM

## 构建与测试

```bash
cargo build                    # 构建
cargo test                     # 运行测试 (79 unit + 9 doctests)
cargo clippy                   # lint (0 warnings)
cargo build --features polars  # 含 polars 扩展
```

## 模块职责速查

| 模块 | 入口类型 | 职责 |
|------|---------|------|
| `client` | `Client`, `ClientBuilder` | 统一入口，管理生命周期 |
| `auth` | `Authenticator` trait, `TqAuth` | 登录、token 解析、权限检查 |
| `websocket` | `TqWebsocket` | 底层连接、重连、消息分发 |
| `datamanager` | `DataManager` | DIFF 合并、版本追踪、路径监听 |
| `quote` | `QuoteSubscription` | 行情订阅 (回调/channel) |
| `series` | `SeriesAPI`, `SeriesSubscription` | K线/Tick 订阅 |
| `ins` | `InsAPI` | 合约查询、期权筛选、交易日历 |
| `trade_session` | `TradeSession` | 交易操作 (下单/撤单/查询) |
| `backtest` | `BacktestHandle` | 回测时间推进 |
