use super::{
    OrderEventStream, TradeEventHub, TradeEventRecvError, TradeEventStream, TradeLoginOptions, TradeOnlyEventStream,
    TradeSession, TradeSessionEventKind,
};
use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::types::Notification;
use crate::websocket::{TqTradeWebsocket, WebSocketConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use tracing::info;

const DEFAULT_CLIENT_APP_ID: &str = "SHINNY_TQ_1.0";

impl TradeSession {
    /// 创建交易会话
    #[expect(clippy::too_many_arguments, reason = "内部装配 TradeSession 时保持依赖显式展开")]
    pub(crate) fn new(
        broker: String,
        user_id: String,
        password: String,
        dm: Arc<DataManager>,
        ws_url: String,
        ws_config: WebSocketConfig,
        reliable_events_max_retained: usize,
        login_options: TradeLoginOptions,
    ) -> Self {
        let trade_events = Arc::new(std::sync::RwLock::new(Arc::new(TradeEventHub::new(
            reliable_events_max_retained,
        ))));
        let (snapshot_epoch_tx, _) = tokio::sync::watch::channel(Some(0i64));
        let snapshot_ready = Arc::new(AtomicBool::new(false));

        let ws = Arc::new(TqTradeWebsocket::new(ws_url, Arc::clone(&dm), ws_config));
        let trade_events_for_notify = Arc::clone(&trade_events);
        let snapshot_ready_for_notify = Arc::clone(&snapshot_ready);
        ws.on_notify(move |noti| {
            if Self::should_reset_snapshot_on_notification(&noti) {
                snapshot_ready_for_notify.store(false, Ordering::SeqCst);
            }
            let trade_events = trade_events_for_notify.read().unwrap().clone();
            let _ = trade_events.publish_notification(noti);
        });

        let trade_events_for_error = Arc::clone(&trade_events);
        ws.on_error(move |msg| {
            let trade_events = trade_events_for_error.read().unwrap().clone();
            let _ = trade_events.publish_transport_error(msg);
        });

        Self {
            broker,
            user_id,
            password,
            login_options,
            dm,
            ws,
            trade_events,
            reliable_events_max_retained,
            snapshot_epoch_tx,
            snapshot_seen_epoch: AtomicI64::new(0),
            snapshot_ready,
            logged_in: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            watch_task: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(super) fn current_trade_events(&self) -> Arc<TradeEventHub> {
        self.trade_events.read().unwrap().clone()
    }

    pub(super) fn reset_trade_events(&self) -> Arc<TradeEventHub> {
        let next_hub = Arc::new(TradeEventHub::new(self.reliable_events_max_retained));
        let previous_hub = {
            let mut guard = self.trade_events.write().unwrap();
            std::mem::replace(&mut *guard, Arc::clone(&next_hub))
        };
        previous_hub.close();
        next_hub
    }

    pub(super) fn reset_snapshot_epoch(&self) {
        self.snapshot_seen_epoch.store(0, Ordering::SeqCst);
        self.snapshot_ready.store(false, Ordering::SeqCst);
        let _ = self.snapshot_epoch_tx.send_replace(Some(0));
    }

    pub(super) fn close_snapshot_epoch(&self) {
        self.snapshot_ready.store(false, Ordering::SeqCst);
        let _ = self.snapshot_epoch_tx.send_replace(None);
    }

    pub(super) fn should_reset_snapshot_on_notification(notification: &Notification) -> bool {
        notification
            .code
            .parse::<i64>()
            .ok()
            .is_some_and(|code| matches!(code, 2019112901 | 2019112902 | 2019112910 | 2019112911))
    }

    pub(super) async fn send_login(&self) -> Result<()> {
        let mut login_req = serde_json::json!({
            "aid": "req_login",
            "bid": self.broker,
            "user_name": self.user_id,
            "password": self.password
        });
        if let Some(mac) = self
            .login_options
            .client_mac_address
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            login_req["client_mac_address"] = serde_json::json!(mac);
        }
        if let Some(system_info) = self
            .login_options
            .client_system_info
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            login_req["client_system_info"] = serde_json::json!(system_info);
            login_req["client_app_id"] = serde_json::json!(
                self.login_options
                    .client_app_id
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(DEFAULT_CLIENT_APP_ID)
            );
        }
        if let Some(front) = &self.login_options.front {
            login_req["broker_id"] = serde_json::json!(front.broker_id);
            login_req["front"] = serde_json::json!(front.url);
        }

        info!("发送交易登录请求: broker={}, user_id={}", self.broker, self.user_id);
        self.ws.send(&login_req).await?;
        Ok(())
    }

    pub(super) async fn send_confirm_settlement(&self) -> Result<()> {
        if !self.login_options.confirm_settlement {
            return Ok(());
        }
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

    /// 订阅可靠交易事件流（仅接收订阅之后产生的事件）
    pub fn subscribe_events(&self) -> TradeEventStream {
        self.current_trade_events().subscribe_tail()
    }

    /// 仅订阅订单状态变更事件
    pub fn subscribe_order_events(&self) -> OrderEventStream {
        self.current_trade_events().subscribe_orders()
    }

    /// 仅订阅成交创建事件
    pub fn subscribe_trade_events(&self) -> TradeOnlyEventStream {
        self.current_trade_events().subscribe_trades()
    }

    #[cfg(test)]
    pub(crate) fn login_options_for_test(&self) -> TradeLoginOptions {
        self.login_options.clone()
    }

    /// 等待交易快照在本会话下推进至少一次。
    pub async fn wait_update(&self) -> Result<()> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TqError::InternalError("交易会话未连接或已关闭".to_string()));
        }

        let seen_epoch = self.snapshot_seen_epoch.load(Ordering::SeqCst);
        let mut rx = self.snapshot_epoch_tx.subscribe();
        loop {
            match *rx.borrow_and_update() {
                Some(epoch) if epoch > seen_epoch => {
                    self.snapshot_seen_epoch.store(epoch, Ordering::SeqCst);
                    return Ok(());
                }
                None => return Err(TqError::InternalError("交易会话已关闭".to_string())),
                _ => {}
            }

            if rx.changed().await.is_err() {
                return Err(TqError::InternalError("交易快照订阅已关闭".to_string()));
            }
        }
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

    #[cfg(test)]
    pub(crate) fn reliable_events_max_retained_for_test(&self) -> usize {
        self.current_trade_events().max_retained_events_for_test()
    }

    #[cfg(test)]
    pub(crate) fn inject_trade_data_for_test(&self, value: serde_json::Value) {
        self.dm.merge_data(value, true, true);
    }

    #[cfg(test)]
    pub(crate) fn snapshot_ready_for_test(&self) -> bool {
        self.snapshot_ready.load(Ordering::SeqCst)
    }
}
