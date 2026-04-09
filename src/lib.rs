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
//! use tqsdk_rs::{Client, ClientConfig, EndpointConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut client = Client::builder("username", "password")
//!         .config(ClientConfig::default())
//!         .endpoints(EndpointConfig::from_env())
//!         .build()
//!         .await?;
//!
//!     client.init_market().await?;
//!     let quote_sub = client.subscribe_quote(&["SHFE.au2602"]).await?;
//!     quote_sub.start().await?;
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

pub mod prelude;
pub mod replay;

// 数据管理器
pub mod datamanager;

// 认证模块
pub mod auth;

// WebSocket 连接
pub mod cache;
#[path = "websocket/mod.rs"]
pub mod websocket;

// Quote 订阅
pub mod quote;

// Series API
pub mod series;

pub mod compat;
pub mod runtime;

// 合约查询
pub mod ins;

// 交易会话
pub mod trade_session;

// 客户端
pub mod client;

pub mod backtest;

// Polars 扩展（可选功能）
#[cfg(feature = "polars")]
pub mod polars_ext;

// 重新导出常用类型
pub use auth::Authenticator;
pub use backtest::{BacktestConfig, BacktestEvent, BacktestHandle, BacktestTime};
pub use client::{Client, ClientBuilder, ClientConfig, ClientOption, EndpointConfig, TradeSessionOptions};
pub use compat::{TargetPosScheduler, TargetPosSchedulerOptions, TargetPosTask, TargetPosTaskOptions};
pub use datamanager::{DataManager, DataManagerConfig, MergeSemanticsConfig};
pub use errors::{Result, TqError};
pub use ins::InsAPI;
pub use logger::{create_logger_layer, init_logger};
pub use quote::QuoteSubscription;
pub use replay::{
    BacktestResult, BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayHandleId, ReplayQuote,
    ReplayStep,
};
pub use runtime::{
    AccountHandle, BacktestExecutionAdapter, BacktestRuntimeMode, OffsetPriority, OrderDirection, PriceMode,
    PriceResolver, RuntimeError, RuntimeMode, RuntimeResult, TargetPosBuilder, TargetPosConfig,
    TargetPosExecutionReport, TargetPosHandle, TargetPosScheduleStep, TargetPosSchedulerBuilder,
    TargetPosSchedulerConfig, TargetPosSchedulerHandle, TqRuntime, VolumeSplitPolicy,
};
pub use series::{SeriesAPI, SeriesCachePolicy, SeriesSubscription};
pub use trade_session::TradeSession;
pub use types::*; // SeriesData 和 UpdateInfo 已在此导出
pub use websocket::TqWebsocket;

// Polars 扩展
#[cfg(feature = "polars")]
pub use polars_ext::{KlineBuffer, TickBuffer};
