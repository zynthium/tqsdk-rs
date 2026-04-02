use super::{ErrorCallback, NotificationCallback, TradeSession};
use crate::datamanager::DataManager;
use crate::errors::Result;
use crate::types::{Account, Notification, Order, Position, PositionUpdate, Trade};
use crate::websocket::{TqTradeWebsocket, WebSocketConfig};
use async_channel::{Receiver, TrySendError, bounded};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
use tracing::{info, warn};

impl TradeSession {
    /// 创建交易会话
    pub fn new(
        broker: String,
        user_id: String,
        password: String,
        dm: Arc<DataManager>,
        ws_url: String,
        ws_config: WebSocketConfig,
    ) -> Self {
        let trade_channel_capacity = ws_config.message_queue_capacity.max(1);

        let (account_tx, account_rx) = bounded(trade_channel_capacity);
        let (position_tx, position_rx) = bounded(trade_channel_capacity);
        let (order_tx, order_rx) = bounded(trade_channel_capacity);
        let (trade_tx, trade_rx) = bounded(trade_channel_capacity);
        let (notification_tx, notification_rx) = bounded(trade_channel_capacity);

        let ws = Arc::new(TqTradeWebsocket::new(ws_url, Arc::clone(&dm), ws_config));

        let on_notification: NotificationCallback = Arc::new(RwLock::new(None));
        let on_error: ErrorCallback = Arc::new(RwLock::new(None));

        let noti_tx = notification_tx.clone();
        let on_notification_for_ws = Arc::clone(&on_notification);
        ws.on_notify(move |noti| {
            let noti_tx = noti_tx.clone();
            let on_notification_for_ws = Arc::clone(&on_notification_for_ws);
            tokio::spawn(async move {
                match noti_tx.try_send(noti.clone()) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        warn!("TradeSession 通知队列已满，丢弃一条通知");
                    }
                    Err(TrySendError::Closed(_)) => {}
                }

                let callback = on_notification_for_ws.read().await.clone();
                if let Some(callback) = callback {
                    callback(noti);
                }
            });
        });

        let on_error_for_ws = Arc::clone(&on_error);
        ws.on_error(move |msg| {
            let on_error_for_ws = Arc::clone(&on_error_for_ws);
            tokio::spawn(async move {
                let callback = on_error_for_ws.read().await.clone();
                if let Some(callback) = callback {
                    callback(msg);
                }
            });
        });

        Self {
            broker,
            user_id,
            password,
            dm,
            ws,
            account_tx,
            account_rx,
            position_tx,
            position_rx,
            order_tx,
            order_rx,
            trade_tx,
            trade_rx,
            notification_tx,
            notification_rx,
            on_account: Arc::new(RwLock::new(None)),
            on_position: Arc::new(RwLock::new(None)),
            on_order: Arc::new(RwLock::new(None)),
            on_trade: Arc::new(RwLock::new(None)),
            on_notification,
            on_error,
            logged_in: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            data_cb_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(super) async fn send_login(&self) -> Result<()> {
        let login_req = serde_json::json!({
            "aid": "req_login",
            "bid": self.broker,
            "user_name": self.user_id,
            "password": self.password
        });

        info!("发送交易登录请求: broker={}, user_id={}", self.broker, self.user_id);
        self.ws.send(&login_req).await?;
        Ok(())
    }

    pub(super) async fn send_confirm_settlement(&self) -> Result<()> {
        let confirm_req = serde_json::json!({
            "aid": "confirm_settlement"
        });
        self.ws.send(&confirm_req).await?;
        Ok(())
    }

    pub(super) fn detach_data_callback(&self) {
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
    }

    /// 注册账户更新回调
    pub async fn on_account<F>(&self, handler: F)
    where
        F: Fn(Account) + Send + Sync + 'static,
    {
        let mut guard = self.on_account.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册持仓更新回调
    pub async fn on_position<F>(&self, handler: F)
    where
        F: Fn(String, Position) + Send + Sync + 'static,
    {
        let mut guard = self.on_position.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册委托单更新回调
    pub async fn on_order<F>(&self, handler: F)
    where
        F: Fn(Order) + Send + Sync + 'static,
    {
        let mut guard = self.on_order.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册成交记录回调
    pub async fn on_trade<F>(&self, handler: F)
    where
        F: Fn(Trade) + Send + Sync + 'static,
    {
        let mut guard = self.on_trade.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册通知回调
    pub async fn on_notification<F>(&self, handler: F)
    where
        F: Fn(Notification) + Send + Sync + 'static,
    {
        let mut guard = self.on_notification.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册错误回调
    pub async fn on_error<F>(&self, handler: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut guard = self.on_error.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 获取账户更新 Channel（克隆接收端）
    pub fn account_channel(&self) -> Receiver<Account> {
        self.account_rx.clone()
    }

    /// 获取持仓更新 Channel（克隆接收端）
    pub fn position_channel(&self) -> Receiver<PositionUpdate> {
        self.position_rx.clone()
    }

    /// 获取委托单更新 Channel（克隆接收端）
    pub fn order_channel(&self) -> Receiver<Order> {
        self.order_rx.clone()
    }

    /// 获取成交记录 Channel（克隆接收端）
    pub fn trade_channel(&self) -> Receiver<Trade> {
        self.trade_rx.clone()
    }

    /// 获取通知 Channel（克隆接收端）
    pub fn notification_channel(&self) -> Receiver<Notification> {
        self.notification_rx.clone()
    }
}
