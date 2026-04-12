//! 交易会话实现
//!
//! 实现实盘交易功能

mod core;
mod events;
mod ops;
mod watch;

#[cfg(test)]
mod tests;

use crate::datamanager::DataManager;
use crate::websocket::TqTradeWebsocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};
use tokio::task::JoinHandle;

pub(crate) use events::TradeEventHub;
pub use events::{
    OrderEventStream, TradeEventRecvError, TradeEventStream, TradeOnlyEventStream, TradeSessionEvent,
    TradeSessionEventKind,
};

/// 交易会话
#[allow(unused)]
pub struct TradeSession {
    broker: String,
    user_id: String,
    password: String,
    dm: Arc<DataManager>,
    ws: Arc<TqTradeWebsocket>,
    trade_events: Arc<std::sync::RwLock<Arc<TradeEventHub>>>,
    reliable_events_max_retained: usize,
    snapshot_epoch_tx: tokio::sync::watch::Sender<Option<i64>>,
    snapshot_seen_epoch: AtomicI64,
    snapshot_ready: Arc<AtomicBool>,

    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    watch_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}
