//! Quote 订阅模块
//!
//! 实现行情订阅功能

mod lifecycle;
mod watch;

#[cfg(test)]
mod tests;

use crate::datamanager::DataManager;
use crate::types::Quote;
use crate::websocket::TqQuoteWebsocket;
use async_channel::{Receiver, Sender};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

type QuoteCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<Quote>) + Send + Sync>>>>;
type QuoteErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<String>) + Send + Sync>>>>;

/// Quote 订阅
pub struct QuoteSubscription {
    id: String,
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    symbols: Arc<RwLock<HashSet<String>>>,
    quote_tx: Sender<Quote>,
    quote_rx: Receiver<Quote>,
    on_quote: QuoteCallback,
    on_error: QuoteErrorCallback,
    running: Arc<RwLock<bool>>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}
