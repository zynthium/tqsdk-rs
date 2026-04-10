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
use crate::types::{Account, Notification, Position, PositionUpdate};
use crate::websocket::TqTradeWebsocket;
use async_channel::{Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

type AccountCallback = Arc<RwLock<Option<Arc<dyn Fn(Account) + Send + Sync>>>>;
type PositionCallback = Arc<RwLock<Option<Arc<dyn Fn(String, Position) + Send + Sync>>>>;
type NotificationCallback = Arc<RwLock<Option<Arc<dyn Fn(Notification) + Send + Sync>>>>;
type ErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(String) + Send + Sync>>>>;

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
    trade_events: Arc<TradeEventHub>,

    account_tx: Sender<Account>,
    account_rx: Receiver<Account>,
    position_tx: Sender<PositionUpdate>,
    position_rx: Receiver<PositionUpdate>,
    notification_tx: Sender<Notification>,
    notification_rx: Receiver<Notification>,

    on_account: AccountCallback,
    on_position: PositionCallback,
    on_notification: NotificationCallback,
    on_error: ErrorCallback,

    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    watch_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}
