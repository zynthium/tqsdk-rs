//! WebSocket 连接封装
//!
//! 基于 yawc 库实现 WebSocket 连接，支持：
//! - deflate 压缩
//! - 自动重连
//! - 消息队列
//! - Debug 日志

use serde_json::Value;
use std::sync::{Arc, RwLock};

mod backpressure;
mod core;
mod message;
mod quote;
mod reconnect;
mod trade;
mod trading_status;

type MessageCallback = Arc<RwLock<Option<Arc<dyn Fn(Value) + Send + Sync>>>>;
type OpenCallback = Arc<RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>;
type CloseCallback = Arc<RwLock<Option<Arc<dyn Fn() + Send + Sync>>>>;
type ErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(String) + Send + Sync>>>>;
type NotifyCallback = Arc<RwLock<Option<Arc<dyn Fn(crate::types::Notification) + Send + Sync>>>>;

pub use core::{TqWebsocket, WebSocketConfig, WebSocketStatus};
pub use quote::TqQuoteWebsocket;
pub use trade::TqTradeWebsocket;
pub use trading_status::TqTradingStatusWebsocket;

pub(crate) use backpressure::{BackpressureState, derive_message_backlog_max};
pub(crate) use message::{
    build_connection_notify, extract_notify_code, has_reconnect_notify, sanitize_log_pack_value,
};
pub(crate) use reconnect::{
    extract_trade_positions, is_md_reconnect_complete, is_ops_maintenance_window_cst,
    is_trade_reconnect_complete, next_shared_reconnect_delay,
};
