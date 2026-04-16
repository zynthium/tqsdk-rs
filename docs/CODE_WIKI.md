# tqsdk-rs 核心代码维基 (Code Wiki)

本文档是 `tqsdk-rs` 项目的结构化代码维基，旨在帮助开发者快速理解项目整体架构、模块划分、核心数据结构以及如何运行和调试项目。

---

## 1. 项目整体架构

`tqsdk-rs` 是天勤（Shinnytech）量化交易平台 DIFF 协议的 Rust 异步实现版本。该项目采用 **单 Crate** 结构，基于 `tokio` 异步生态，整体架构呈现 **状态驱动** 与 **事件流** 结合的特征。

### 架构核心特征：
- **状态驱动优先**：对行情、K线、Tick 等高频数据，采用 DIFF 协议进行增量数据合并，更新维护在本地状态树中。外部消费者通过引用（如 `QuoteRef`）读取最新快照，有效避免传统队列分发带来的内存积压（慢消费者问题）。
- **异步与 I/O Actor 模式**：底层 WebSocket 传输层独立为 Actor 任务，负责网络 I/O、协议解包与重连校验；主逻辑无阻塞地读取合并后的状态。
- **DIFF 协议引擎**：核心为 `DataManager`，支持 JSON 对象的递归合并、路径监听（watch/unwatch）以及 Epoch（版本号）追踪。
- **交易状态与事件分层**：实盘交易 (`TradeSession`) 中，账户、持仓等结果采用“状态快照”机制读取，而订单状态变化、成交回报、错误通知等采用“可靠事件流”机制推送。
- **统一运行时 (`TqRuntime`)**：无论实盘还是回测 (`ReplaySession`)，目标持仓任务 (`TargetPosTask`) 和调度器均由底层的 `TqRuntime` 统一抽象。

---

## 2. 主要模块职责

项目源代码主要集中在 `src/` 目录下，各模块分工明确：

| 模块目录 | 核心职责 |
| --- | --- |
| **`client/`** | 提供统一对外的 `Client` 和 `ClientBuilder`。负责生命周期管理、网络入口整合。 |
| **`datamanager/`** | 核心 DIFF 协议合并引擎。处理服务器下发的增量 JSON 数据，维护本地状态树，提供路径查询和基于 Epoch 的更新通知。 |
| **`websocket/`** | 底层通信层（基于 `yawc`）。管理行情 (`TqQuoteWebsocket`) 和交易 (`TqTradeWebsocket`) 通道，处理断线重连、数据缓冲与背压控制。 |
| **`auth/`** | 认证与权限校验模块。负责向天勤服务器获取 JWT Token，解析 Token Claims，并校验用户是否拥有相应的交易或行情权限。 |
| **`quote/`** & **`series/`** | 行情订阅（Quote）与序列订阅（K线/Tick）。管理向服务器发起的订阅生命周期，并将数据暴漏为快照引用。 |
| **`trade_session/`** | 交易会话模块。负责账户登录、下单、撤单操作，维护持仓/账户状态，并吐出可靠的订单与成交事件流。 |
| **`runtime/`** | 策略执行引擎。包含 `TqRuntime`、`AccountHandle`，支持通过 Builder 模式创建目标持仓任务 (`TargetPosTask`) 或时间切片调度器 (`TargetPosScheduler`)。 |
| **`replay/`** | 回放与回测引擎。包含 `ReplaySession`，模拟时间推演、K线/Tick数据合成，以及与 `TqRuntime` 结合的本地仿真撮合。 |
| **`ins/`** | 静态与基础数据查询。支持通过 HTTP/GraphQL 查询合约列表、期权数据、结算价、交易日历等。 |
| **`cache/`** | 本地历史数据磁盘缓存。可选开启，缓存 K线/Tick 数据到本地（与 Python SDK 兼容），支持并发写保护与自动清理。 |
| **`polars_ext/`** | DataFrame 转换（可选 Feature）。将 K线、Tick、财务等数据直接转换为 `polars` DataFrame 供量化分析使用。 |

---

## 3. 关键类与函数说明

### 3.1 客户端与连接
- **`ClientBuilder`**: 客户端构造器。用于配置用户名、密码、日志级别、队列容量及自定义服务端点。
- **`Client`**: 整个 SDK 的入口 Facade。
  - `init_market()`: 初始化行情连接。
  - `wait_update_and_drain()`: 等待全局任意数据更新，并获取发生变更的资源集合。
  - `spawn_data_downloader()`: 发起后台异步数据下载任务（如下载 CSV）。

### 3.2 行情与序列数据
- **`QuoteSubscription` / `SeriesSubscription`**: 订阅生命周期句柄，对象创建即代表向服务端发起订阅，调用 `close()` 可提前释放。
- **`QuoteRef` / `KlineRef` / `TickRef`**: 轻量级状态引用。
  - `wait_update()`: 异步等待该标的数据的下一次更新（基于 Epoch）。
  - `load()`: 异步读取当前的快照数据（如 `Quote`, `SeriesData`）。

### 3.3 交易与账户 (`TradeSession`)
- **`TradeSession`**: 实盘/模拟盘交易会话。
  - `connect()`: 建立交易 WebSocket 连接并登录。
  - `insert_order(req: &InsertOrderRequest)`: 发起报单（限价单、市价单等）。
  - `cancel_order(order_id: &str)`: 撤销订单。
  - `get_account()` / `get_position()`: 获取最新的账户资金和持仓快照。
  - `subscribe_events()`: 获取可靠的交易事件流（包含 `OrderUpdated`, `TradeCreated`, `NotificationReceived` 等）。

### 3.4 回测与执行 (`ReplaySession` & `TqRuntime`)
- **`ReplaySession`**: 回测会话。
  - `step()`: 驱动回测时间向前推进一个最小粒度。
- **`TqRuntime` & `AccountHandle`**:
  - `target_pos(symbol)`: 创建基于单一合约的目标持仓规划任务（自动计算开平、买卖方向）。
  - `target_pos_scheduler(symbol)`: 创建时间切片调仓任务（例如 TWAP/VWAP 执行逻辑）。

### 3.5 核心机制 (`DataManager`)
- **`DataManager`**:
  - `merge_data()`: 递归合并 DIFF 补丁。
  - `get_by_path(path: &[&str])`: 提取状态树中特定路径的数据。
  - `subscribe_epoch()`: 订阅全局版本号变化，用于触发顶层事件循环。

---

## 4. 核心依赖关系 (Dependencies)

项目在 `Cargo.toml` 中配置了丰富的依赖，关键依赖如下：

- **异步与并发**:
  - `tokio`: 基础异步运行时（包含 rt, macros, sync, time 等）。
  - `futures`, `async-channel`, `async-stream`: 辅助异步流与通道。
- **网络通信**:
  - `yawc`: 高性能 WebSocket 客户端（天勤 DIFF 协议要求支持 zlib 压缩，因此启用了 `zlib` feature）。
  - `reqwest`: HTTP/GraphQL 客户端，用于 REST API 查询。
- **序列化与协议**:
  - `serde`, `serde_json`: JSON 数据的序列化与反序列化（DIFF 协议核心）。
- **安全与认证**:
  - `jsonwebtoken`: JWT 的签发验证，用于天勤服务器鉴权。
- **日志**:
  - `tracing`, `tracing-subscriber`: 结构化日志记录。
- **数据分析 (Optional)**:
  - `polars`: （在 `features = ["polars"]` 时启用）用于序列数据的内存表格化及快速运算。

---

## 5. 项目运行与调试方式

### 5.1 环境准备
在运行项目或测试前，需配置天勤的账户环境变量。部分示例还会涉及到模拟交易（SimNow）。

```bash
export TQ_AUTH_USER="你的天勤账号"
export TQ_AUTH_PASS="你的天勤密码"
# 可选：调整日志级别
export TQ_LOG_LEVEL="debug" # 或 info, warn
```

### 5.2 基础命令
- **编译项目**: `cargo build`
- **运行单元测试**: `cargo test` (注：依赖 live 账号的测试带有 `#[ignore]` 标签，默认跳过)
- **代码规范检查**: `cargo clippy --all-targets --all-features -- -D warnings`
- **代码格式化**: `cargo fmt --check`
- **启用 Polars 编译**: `cargo build --features polars`

### 5.3 运行官方示例 (Examples)
项目在 `examples/` 目录下提供了丰富的用例，涵盖了核心功能的标准用法。

```bash
# 1. 运行行情订阅示例
cargo run --example quote

# 2. 运行历史 K 线数据序列示例
cargo run --example history

# 3. 运行实盘/模拟盘交易示例 (需配置 SIMNOW_USER_0 等)
cargo run --example trade

# 4. 运行回测框架示例
cargo run --example backtest

# 5. 运行量化策略示例 (例如：双均线策略)
cargo run --example doublema
```

> **安全警告**:
> `cargo run --example trade` 会直接向交易环境发送报单，请务必确认你使用的是 SimNow 模拟账号或明确了解风险。

### 5.4 本地联调注意事项
- **日志降级**: 本项目不提倡在库内部使用 `panic!`，核心网络或解析失败会通过 `tracing` 打印告警日志并尝试自动恢复（或返回 `TqError`）。联调时请务必开启 `TQ_LOG_LEVEL=info` 或 `debug`。
- **慢消费者警告**: 如果控制台出现“通道已满，丢弃一次更新”的警告，说明策略循环处理过慢。SDK 设计为“有界缓冲+丢弃旧状态”策略以防止 OOM。
- **接口同步**: 修改核心逻辑时，请务必检查是否破坏了 `README.md` 和 `AGENTS.md` 中规定的 Canonical 架构。
