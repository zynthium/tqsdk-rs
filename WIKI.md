# tqsdk-rs 代码 Wiki

`tqsdk-rs` 是天勤量化交易平台 DIFF 协议的 Rust 语言封装 SDK。它提供获取实时行情、K 线与 Tick 数据、合约基础数据查询、以及实盘与模拟回测交易等核心量化能力。

本文档是对该项目仓库的全面梳理，旨在帮助开发者快速理解项目架构、模块职责、核心逻辑和运行方式。

---

## 1. 项目整体架构

`tqsdk-rs` 是一个单 crate 的 Rust 项目，基于 `tokio` 异步运行时构建，采用强类型设计和基于 Actor 模式的 I/O 处理。整体架构分为几个核心层：

*   **客户端层 (Client Layer)**：统一的对外接口门面 (`Client` / `ClientBuilder`)，负责配置解析、组件初始化和生命周期管理。
*   **运行时与交易层 (Runtime & Trade Layer)**：包括 `TqRuntime` 和 `TradeSession`。封装了目标持仓调度、账户、订单与成交管理，并通过 Adapter 模式分离了实盘 (Live) 和回测 (Backtest) 的执行逻辑。
*   **行情与序列层 (Quote & Series Layer)**：处理实时 Quote 订阅、K 线 (Kline) 和 Tick 序列的请求、合并、以及本地磁盘缓存。采用高性能状态驱动模式，通过 `wait_update` 和 `load` 读取数据。
*   **数据管理层 (DataManager Layer)**：协议核心层。天勤 DIFF 协议本质上是增量字典的合并协议。`DataManager` 负责 JSON 对象的递归合并、路径查询、变化监听 (Watch) 以及全局 Epoch 版本追踪。
*   **通信与认证层 (Transport & Auth Layer)**：
    *   **WebSocket**：采用 I/O Actor 模式处理底层 WebSocket 消息发送与接收，实现了断线重连、完整性校验、背压控制（使用有界缓冲通道避免内存溢出）。
    *   **Auth**：处理用户登录、JWT 解析、Token claims 校验及权限检查。

---

## 2. 主要模块职责

项目的源代码主要分布在 `src/` 目录下，各模块的职责明确：

| 目录/模块 | 主要职责 |
| :--- | :--- |
| `client/` | **统一门面**。提供 `Client` 和 `ClientBuilder`，是使用该 SDK 的起点。管理行情、交易和配置。 |
| `datamanager/` | **DIFF 数据核心**。维护全局合并字典，处理增量更新，支持按路径 (`path`) 监听和查询，并计算版本号 (`epoch`)。 |
| `websocket/` | **通信底层**。管理 WebSocket 连接（行情、交易通道），处理发送/接收队列背压，实现自动重连与心跳保活。 |
| `quote/` | **实时行情**。管理 `QuoteSubscription` 订阅的生命周期，向用户推送实时的切片行情更新。 |
| `series/` | **时间序列**。管理 K 线和 Tick 的订阅 (`SeriesAPI`, `SeriesSubscription`)，处理历史数据与实时数据的衔接。 |
| `trade_session/` | **交易会话**。提供 `TradeSession` 用于实盘或模拟盘的下单、撤单、以及账户/持仓/委托/成交状态的实时同步。 |
| `runtime/` | **交易运行时**。提供 `TqRuntime`，支持 Python 风格的 `TargetPosTask`（目标持仓任务），统一回测和实盘的执行上下文。 |
| `replay/` & `backtest/` | **回测引擎**。提供 `ReplaySession` 和 `BacktestHandle`，负责时间轴推进、历史行情回放、以及本地撮合逻辑。 |
| `ins/` | **信息查询**。处理基础数据查询（GraphQL），如合约列表、期权筛选、结算价、持仓排名等 HTTP 请求。 |
| `auth/` | **鉴权模块**。处理账号密码登录、获取 Token、验证 JWT 以及各种数据/交易权限的检查。 |
| `cache/` | **本地缓存**。管理与 Python SDK 兼容的本地历史 K 线/Tick 磁盘缓存，支持去重、分段写入和压缩。 |
| `polars_ext/` | **数据分析 (可选)**。当启用 `polars` feature 时，提供将 K 线、Tick 转换为 DataFrame 的扩展能力。 |

---

## 3. 关键类与函数说明

### 3.1 客户端构建与管理
*   **`ClientBuilder`**: 客户端的构造器，支持链式调用配置账号、密码、日志级别、队列容量、覆盖服务 URL 等。
    *   `build()`: 异步创建并返回 `Client` 实例。
*   **`Client`**: SDK 主入口。
    *   `init_market()`: 初始化行情连接。
    *   `subscribe_quote(symbols)`: 订阅指定的实时行情。
    *   `series()`: 获取 `SeriesAPI` 用于订阅 K 线或 Tick。
    *   `create_trade_session(...)`: 创建并返回交易会话实例。
    *   `create_backtest_session(...)`: 创建历史回测会话。

### 3.2 行情与序列订阅
*   **`TqApi`**: 状态驱动的数据访问入口，通过 `client.tqapi()` 获取。
    *   `quote(symbol)`: 获取 `QuoteRef` 引用。
    *   `kline(symbol, duration)`: 获取 `KlineRef` 引用。
    *   `wait_update_and_drain()`: 批量等待数据更新并返回变化集合。
*   **`QuoteSubscription`**: 行情订阅生命周期句柄，主要用于向服务器声明订阅范围。
    *   `start()`: 启动订阅并连接后台推送。
*   **`SeriesSubscription`**: K 线 / Tick 序列订阅句柄。
    *   `start()`: 启动序列订阅。

### 3.3 数据管理核心
*   **`DataManager`**: 增量数据合并中心。
    *   `get_by_path(path)`: 按路径获取底层原始 JSON 数据。
    *   `is_changing(path)`: 判断指定路径的数据在最新一个 Epoch 中是否发生变化。
    *   `get_epoch()`: 获取当前全局版本号，用于做增量追踪。

### 3.4 交易与回测
*   **`TradeSession`**: 实盘交易核心。
    *   `insert_order(request)`: 提交 `InsertOrderRequest` 进行下单。
    *   `cancel_order(order_id)`: 撤单。
    *   `get_account()` / `get_position(symbol)`: 主动查询当前账户和持仓。
*   **`TargetPosTask`**: 兼容 Python SDK 的目标持仓控制任务。
    *   `set_target_volume(volume)`: 声明目标持仓，内部自动判断需要开平仓的方向并下单。
*   **`ReplaySession`**: 新版推荐的回测会话引擎。
    *   `step()`: 驱动时间轴前进一步，返回更新的数据状态，供策略判断。

---

## 4. 依赖关系

项目在 `Cargo.toml` 中定义了以下核心依赖，整体构建在现代 Rust 异步生态之上：

| 依赖库 | 作用与用途 |
| :--- | :--- |
| `tokio` | 核心异步运行时，提供任务调度、Channel、时间控制等。 |
| `yawc` | WebSocket 客户端，必须支持 `zlib` 特性以处理天勤压缩的行情数据。 |
| `reqwest` | HTTP / GraphQL 客户端，用于登录鉴权和基础查询（启用了 gzip, br 等）。 |
| `serde` & `serde_json` | 高性能的序列化/反序列化，处理天勤庞大的 DIFF 协议报文。 |
| `jsonwebtoken` | JWT Token 解析与验证。 |
| `tracing` | 结构化日志系统，配合 `tracing-subscriber` 打印调试信息。 |
| `async-channel` | 跨任务的有界/无界异步消息通道通信。 |
| `chrono` | 日期与时间处理。 |
| `polars` | **(可选 Feature)** 高性能 DataFrame 库，用于量化数据清洗分析。 |

---

## 5. 项目运行方式

### 5.1 环境准备
进行开发或运行前，需要准备好 Rust 2024 edition 环境，并配置必要的天勤平台环境变量（推荐配置在 `.env` 或运行时注入）：

*   `TQ_AUTH_USER`: 天勤账号（必填）
*   `TQ_AUTH_PASS`: 天勤密码（必填）
*   `SIMNOW_USER_0` / `SIMNOW_PASS_0`: 运行 `trade` 示例时的模拟盘账号密码。
*   `TQ_LOG_LEVEL`: 日志级别 (例如 `info`, `debug`)。

### 5.2 基础开发命令
*   **构建项目**: `cargo build`
*   **运行单元测试**: `cargo test`
*   **严格 Lint 检查**: `cargo clippy --all-targets --all-features -- -D warnings`
*   **开启 Polars 支持的构建**: `cargo build --features polars`

### 5.3 运行官方示例
SDK 的 `examples/` 目录下提供了多种使用场景的最佳实践，是学习和验证功能的最快方式。

```bash
# 1. 行情订阅示例 (Quote, K线, Tick)
cargo run --example quote

# 2. 历史数据与接口联调
cargo run --example history

# 3. 模拟盘/实盘交易示例 (高风险，需配置 SIMNOW 账号)
cargo run --example trade

# 4. 回测引擎示例 (历史回放、TargetPos 持仓控制)
cargo run --example backtest

# 5. 日线枢轴点反转策略示例
cargo run --example pivot_point

# 6. 期权查询示例 (平值/实值/虚值)
cargo run --example option_levels
```

### 5.4 注意事项与最佳实践
1. **延迟启动**：使用行情 (`Quote`) 和序列 (`Series`) 订阅时，务必在获取到 `QuoteRef` / `KlineRef` 之后调用 `.start()`，以触发后台的数据推送。
2. **背压与丢弃**：底层的更新通道使用了**有界缓冲队列**。如果在日志中看到“通道已满，丢弃一次更新”，说明消费者处理过慢。可以适当调大 `ClientBuilder` 的 `message_queue_capacity`，或避免在回调中执行长阻塞任务。
3. **合约格式**：API 要求的合约格式一律为带交易所前缀的全称，如 `"SHFE.au2602"`。
4. **安全验证**：切勿在代码库中硬编码真实账号密码，避免直接针对实盘环境运行未知测试。
