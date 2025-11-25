//! # TQSDK-RS3
//!
//! 天勤 DIFF 协议的 Rust 语言封装
//!
//! 这是一个用于连接天勤量化交易平台的 Rust SDK，支持：
//! - 实时行情订阅（Quote, K线, Tick）
//! - 历史数据获取
//! - 实盘/模拟交易
//! - DIFF 协议数据管理
//!
//! ## 快速开始
//!
//! ```no_run
//! use tqsdk_rs::{Client, ClientConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 创建客户端
//!     let client = Client::new("username", "password", ClientConfig::default()).await?;
//!     
//!     // 初始化行情
//!     client.init_market().await?;
//!     
//!     // 订阅行情
//!     let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
//!     
//!     Ok(())
//! }
//! ```

// 错误类型
pub mod errors;

// 日志系统
pub mod logger;

// 工具函数
pub mod utils;

// 数据结构
pub mod types;

// 数据管理器
pub mod datamanager;

// 认证模块
pub mod auth;

// WebSocket 连接
pub mod websocket;

// Quote 订阅
pub mod quote;

// Series API
pub mod series;

// 交易会话
pub mod trade_session;

// 客户端
pub mod client;

// 重新导出常用类型
pub use auth::Authenticator;
pub use client::{Client, ClientConfig, ClientOption};
pub use datamanager::{DataManager, DataManagerConfig};
pub use errors::{Result, TqError};
pub use logger::init_logger;
pub use quote::QuoteSubscription;
pub use series::{SeriesAPI, SeriesSubscription};
pub use trade_session::TradeSession;
pub use types::*; // SeriesData 和 UpdateInfo 已在此导出
