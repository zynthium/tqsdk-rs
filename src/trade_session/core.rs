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

fn configured_value(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_mac_candidate(candidate: &str) -> Option<String> {
    let parts = candidate
        .trim()
        .split([':', '-'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 6
        || parts
            .iter()
            .any(|part| part.len() != 2 || !part.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return None;
    }
    let normalized = parts
        .into_iter()
        .map(str::to_ascii_uppercase)
        .collect::<Vec<_>>()
        .join("-");
    if normalized == "00-00-00-00-00-00" {
        return None;
    }
    Some(normalized)
}

#[cfg(target_os = "linux")]
fn detect_client_mac_address() -> Option<String> {
    let entries = std::fs::read_dir("/sys/class/net").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy() == "lo" {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path().join("address")) else {
            continue;
        };
        if let Some(mac) = normalize_mac_candidate(&raw) {
            return Some(mac);
        }
    }
    None
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn detect_client_mac_address() -> Option<String> {
    let output = std::process::Command::new("ifconfig").arg("-a").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(raw) = line.trim().strip_prefix("ether ")
            && let Some(mac) = normalize_mac_candidate(raw)
        {
            return Some(mac);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn detect_client_mac_address() -> Option<String> {
    let output = std::process::Command::new("getmac").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for token in stdout.split_whitespace() {
        if let Some(mac) = normalize_mac_candidate(token) {
            return Some(mac);
        }
    }
    None
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "windows"
)))]
fn detect_client_mac_address() -> Option<String> {
    None
}

fn detect_client_system_info() -> Option<String> {
    // 官方 Python 在 macOS 等不支持的平台也会静默跳过穿透式监管信息采集。
    None
}

fn resolve_client_mac_address(login_options: &TradeLoginOptions) -> Option<String> {
    configured_value(&login_options.client_mac_address).or_else(detect_client_mac_address)
}

fn resolve_client_system_info(login_options: &TradeLoginOptions) -> Option<String> {
    configured_value(&login_options.client_system_info)
        .or_else(|| {
            std::env::var("TQ_CLIENT_SYSTEM_INFO")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(detect_client_system_info)
}

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
        if let Some(mac) = resolve_client_mac_address(&self.login_options) {
            login_req["client_mac_address"] = serde_json::json!(mac);
        }
        if let Some(system_info) = resolve_client_system_info(&self.login_options) {
            login_req["client_system_info"] = serde_json::json!(system_info);
            login_req["client_app_id"] = serde_json::json!(
                configured_value(&self.login_options.client_app_id)
                    .as_deref()
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

    #[cfg(test)]
    pub(crate) fn ws_url_for_test(&self) -> String {
        self.ws.url_for_test()
    }

    /// 等待交易快照在本会话下推进至少一次。
    pub async fn wait_update(&self) -> Result<()> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TqError::TradeSessionNotConnected);
        }

        let seen_epoch = self.snapshot_seen_epoch.load(Ordering::SeqCst);
        let mut rx = self.snapshot_epoch_tx.subscribe();
        loop {
            match *rx.borrow_and_update() {
                Some(epoch) if epoch > seen_epoch => {
                    self.snapshot_seen_epoch.store(epoch, Ordering::SeqCst);
                    return Ok(());
                }
                None => return Err(TqError::TradeSessionNotConnected),
                _ => {}
            }

            if rx.changed().await.is_err() {
                return Err(TqError::TradeSessionNotConnected);
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
