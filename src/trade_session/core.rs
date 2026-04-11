use super::{
    ErrorCallback, NotificationCallback, OrderEventStream, TradeEventHub, TradeEventRecvError, TradeEventStream,
    TradeOnlyEventStream, TradeSession, TradeSessionEventKind,
};
use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::types::{Account, Notification, Position};
use crate::websocket::{TqTradeWebsocket, WebSocketConfig};
use async_channel::{Receiver, TrySendError, bounded};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
use tracing::{info, warn};

impl TradeSession {
    /// 创建交易会话
    pub(crate) fn new(
        broker: String,
        user_id: String,
        password: String,
        dm: Arc<DataManager>,
        ws_url: String,
        ws_config: WebSocketConfig,
        reliable_events_max_retained: usize,
    ) -> Self {
        let trade_channel_capacity = ws_config.message_queue_capacity.max(1);
        let trade_events = Arc::new(TradeEventHub::new(reliable_events_max_retained));

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
            trade_events,
            notification_tx,
            notification_rx,
            on_account: Arc::new(RwLock::new(None)),
            on_position: Arc::new(RwLock::new(None)),
            on_notification,
            on_error,
            logged_in: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            watch_task: Arc::new(std::sync::Mutex::new(None)),
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

    pub(super) fn stop_watch_task(&self) {
        if let Some(handle) = self.watch_task.lock().unwrap().take() {
            handle.abort();
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

    /// 订阅可靠交易事件流（仅接收订阅之后产生的事件）
    pub fn subscribe_events(&self) -> TradeEventStream {
        self.trade_events.subscribe_tail()
    }

    /// 仅订阅订单状态变更事件
    pub fn subscribe_order_events(&self) -> OrderEventStream {
        self.trade_events.subscribe_orders()
    }

    /// 仅订阅成交创建事件
    pub fn subscribe_trade_events(&self) -> TradeOnlyEventStream {
        self.trade_events.subscribe_trades()
    }

    /// 等待指定订单出现后续可靠更新
    pub async fn wait_order_update_reliable(&self, order_id: &str) -> Result<()> {
        let mut stream = self.subscribe_events();
        loop {
            match stream.recv().await {
                Ok(event) => match event.kind {
                    TradeSessionEventKind::OrderUpdated {
                        order_id: updated_order_id,
                        ..
                    } if updated_order_id == order_id => return Ok(()),
                    TradeSessionEventKind::TradeCreated { trade, .. } if trade.order_id == order_id => {
                        return Ok(());
                    }
                    _ => {}
                },
                Err(TradeEventRecvError::Closed) => {
                    return Err(TqError::InternalError("trade event stream closed".to_string()));
                }
                Err(TradeEventRecvError::Lagged { missed_before_seq }) => {
                    return Err(TqError::InternalError(format!(
                        "trade event stream lagged before seq {missed_before_seq}"
                    )));
                }
            }
        }
    }

    /// 获取通知 Channel（克隆接收端）
    pub fn notification_channel(&self) -> Receiver<Notification> {
        self.notification_rx.clone()
    }

    #[cfg(test)]
    pub(crate) fn reliable_events_max_retained_for_test(&self) -> usize {
        self.trade_events.max_retained_events_for_test()
    }
}
