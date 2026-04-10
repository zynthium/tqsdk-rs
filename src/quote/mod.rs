//! Quote 订阅模块
//!
//! 实现行情订阅功能

mod lifecycle;

#[cfg(test)]
mod tests;

use crate::websocket::TqQuoteWebsocket;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Quote 订阅
pub struct QuoteSubscription {
    id: String,
    ws: Arc<TqQuoteWebsocket>,
    symbols: Arc<RwLock<HashSet<String>>>,
    running: Arc<RwLock<bool>>,
}
