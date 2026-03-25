//! 交易会话实现
//!
//! 实现实盘交易功能

mod core;
mod ops;
mod watch;

#[cfg(test)]
mod tests;

use crate::datamanager::DataManager;
use crate::types::{Account, Notification, Order, Position, PositionUpdate, Trade};
use crate::websocket::TqTradeWebsocket;
use async_channel::{Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;

type AccountCallback = Arc<RwLock<Option<Arc<dyn Fn(Account) + Send + Sync>>>>;
type PositionCallback = Arc<RwLock<Option<Arc<dyn Fn(String, Position) + Send + Sync>>>>;
type OrderCallback = Arc<RwLock<Option<Arc<dyn Fn(Order) + Send + Sync>>>>;
type TradeCallback = Arc<RwLock<Option<Arc<dyn Fn(Trade) + Send + Sync>>>>;
type NotificationCallback = Arc<RwLock<Option<Arc<dyn Fn(Notification) + Send + Sync>>>>;
type ErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(String) + Send + Sync>>>>;

/// 交易会话
#[allow(unused)]
pub struct TradeSession {
    broker: String,
    user_id: String,
    password: String,
    dm: Arc<DataManager>,
    ws: Arc<TqTradeWebsocket>,

    account_tx: Sender<Account>,
    account_rx: Receiver<Account>,
    position_tx: Sender<PositionUpdate>,
    position_rx: Receiver<PositionUpdate>,
    order_tx: Sender<Order>,
    order_rx: Receiver<Order>,
    trade_tx: Sender<Trade>,
    trade_rx: Receiver<Trade>,
    notification_tx: Sender<Notification>,
    notification_rx: Receiver<Notification>,

    on_account: AccountCallback,
    on_position: PositionCallback,
    on_order: OrderCallback,
    on_trade: TradeCallback,
    on_notification: NotificationCallback,
    on_error: ErrorCallback,

    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}
