# TQSDK-RS3

天勤 DIFF 协议的 Rust 语言封装

**⚠️ 注意：本项目目前处于初始开发阶段，基础框架已搭建完成，但部分功能尚未完全实现。**

## 项目状态

### 已完成
- ✅ 项目结构和依赖配置
- ✅ 错误类型定义 (thiserror)
- ✅ 日志系统 (tracing)
- ✅ 工具函数 (流式 fetch_json)
- ✅ 数据结构定义 (90+ 字段)
- ✅ DataManager 核心实现 (DIFF 协议)
- ✅ 认证模块 (JWT)
- ✅ WebSocket 基础框架 (yawc)
- ✅ Quote, Series, Trader 等核心接口定义
- ✅ Client 统一入口

### 待完善
- ⚠️ WebSocket 实际连接逻辑 (需要根据 yawc 0.2.7 API 调整)
- ⚠️ Quote 订阅的完整实现
- ⚠️ Series API 的完整实现
- ⚠️ TradeSession 交易功能
- ⚠️ VirtualTrader 虚拟交易
- ⚠️ 示例程序的完善

## 特性

- **类型安全**：使用 Rust 强类型系统，消除运行时错误
- **并发安全**：基于 Arc + RwLock，确保线程安全
- **异步优化**：使用 tokio 异步运行时，高效处理 I/O
- **DIFF 协议**：完整实现天勤 DIFF 协议数据合并
- **流式接口**：支持 async_stream 流式数据处理
- **详细日志**：使用 tracing 提供详细的调试信息

## 技术栈

- **WebSocket**: yawc 0.2.7 (支持 deflate 压缩)
- **HTTP**: reqwest (支持 gzip, brotli, stream)
- **JSON**: serde + serde_json
- **JWT**: jsonwebtoken
- **日志**: tracing + tracing-subscriber
- **错误**: thiserror
- **异步**: tokio + async-trait
- **时间**: chrono

## 安装

在 `Cargo.toml` 中添加：

```toml
[dependencies]
tqsdk-rs3 = "0.1.0"
```

## 快速开始

```rust
use tqsdk_rs::{Client, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 创建客户端
    let mut client = Client::new("username", "password", ClientConfig::default()).await?;
    
    // 初始化行情
    client.init_market().await?;
    
    // 订阅行情
    let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
    
    // TODO: 处理行情数据
    
    Ok(())
}
```

## 示例程序

项目提供三个示例程序：

```bash
# 行情订阅示例
cargo run --example quote

# 历史数据示例
cargo run --example history

# 交易示例
cargo run --example trade
```

**注意**：运行示例前需要设置环境变量：

```bash
export SHINNYTECH_ID="your_username"
export SHINNYTECH_PW="your_password"
```

## 项目结构

```
tqsdk-rs3/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs              # 库入口
│   ├── client.rs           # 客户端
│   ├── auth.rs             # 认证模块
│   ├── websocket.rs        # WebSocket 封装
│   ├── datamanager.rs      # 数据管理器
│   ├── datastructure.rs    # 数据结构
│   ├── quote.rs            # Quote 订阅
│   ├── series.rs           # Series API
│   ├── trader.rs           # Trader trait
│   ├── trade_session.rs    # 交易会话
│   ├── virtual_trader.rs   # 虚拟交易
│   ├── utils.rs            # 工具函数
│   ├── logger.rs           # 日志系统
│   └── errors.rs           # 错误类型
└── examples/
    ├── quote.rs            # 行情示例
    ├── history.rs          # 历史数据示例
    └── trade.rs            # 交易示例
```

## 开发计划

### 第一阶段：基础功能完善
- [ ] 完善 WebSocket 连接逻辑 (基于 yawc)
- [ ] 实现 Quote 订阅的 Channel 和回调
- [ ] 实现 Series API 的 K线/Tick 订阅

### 第二阶段：交易功能
- [ ] 实现 TradeSession 登录和交易操作
- [ ] 实现 VirtualTrader 虚拟交易逻辑
- [ ] 完善错误处理和重连机制

### 第三阶段：优化和测试
- [ ] 内存优化和并发安全测试
- [ ] 编写单元测试和集成测试
- [ ] 性能基准测试
- [ ] 完善文档和示例

## 注意事项

1. **WebSocket 实现**：当前 WebSocket 模块提供了基础框架，实际使用时需要根据 yawc 0.2.7 的 API 进行调整。查看 `src/websocket.rs` 中的 TODO 注释。

2. **TODO 标记**：代码中标记了多个 `TODO` 和 `todo!()` 宏的地方，这些是需要完善的功能点。

3. **示例程序**：当前示例程序只演示了基础流程，实际功能尚未完全实现。

## 贡献

欢迎提交 Issue 和 Pull Request！

## 许可证

Apache License 2.0

## 致谢

本项目是 [tqsdk-go](https://github.com/pseudocodes/tqsdk-go) 的 Rust 移植版本。

感谢 [天勤量化](https://www.shinnytech.com/) 提供优秀的 DIFF 协议和服务支持。

## 免责声明

本项目明确拒绝对产品做任何明示或暗示的担保。使用本项目进行交易和投资的一切风险由使用者自行承担。

