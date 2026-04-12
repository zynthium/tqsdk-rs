use super::{
    BackpressureState, NotifyCallback, TqWebsocket, WebSocketConfig, derive_message_backlog_max, extract_notify_code,
    extract_trade_positions, has_reconnect_notify, is_trade_reconnect_complete,
};
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::debug;

#[derive(Clone)]
struct TradeRuntime {
    dm: Arc<DataManager>,
    req_login: Arc<std::sync::RwLock<Option<Value>>>,
    confirm_settlement: Arc<std::sync::RwLock<Option<Value>>>,
    on_notify: NotifyCallback,
    reconnect_pending: Arc<AtomicBool>,
    reconnect_diffs: Arc<std::sync::RwLock<Vec<Value>>>,
    reconnect_dm: Arc<std::sync::RwLock<Option<Arc<DataManager>>>>,
    reconnect_prev_positions: Arc<std::sync::RwLock<HashMap<String, HashSet<String>>>>,
}

/// 交易 WebSocket
pub struct TqTradeWebsocket {
    base: Arc<TqWebsocket>,
    runtime: TradeRuntime,
}

impl TqTradeWebsocket {
    /// 创建交易 WebSocket
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let message_backlog_max =
            derive_message_backlog_max(config.message_queue_capacity, config.message_backlog_warn_step);
        let base = Arc::new(TqWebsocket::new(url, config.clone()));
        let runtime = TradeRuntime {
            dm,
            req_login: Arc::new(std::sync::RwLock::new(None)),
            confirm_settlement: Arc::new(std::sync::RwLock::new(None)),
            on_notify: Arc::new(std::sync::RwLock::new(None)),
            reconnect_pending: Arc::new(AtomicBool::new(false)),
            reconnect_diffs: Arc::new(std::sync::RwLock::new(Vec::new())),
            reconnect_dm: Arc::new(std::sync::RwLock::new(None)),
            reconnect_prev_positions: Arc::new(std::sync::RwLock::new(HashMap::new())),
        };

        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<Value>(config.message_queue_capacity);
        let backpressure = BackpressureState::new(
            msg_tx,
            message_backlog_max,
            config.message_backlog_warn_step,
            config.message_batch_max,
            "交易",
        );
        {
            let base_for_overflow = Arc::clone(&base);
            backpressure.set_overflow_handler(move || {
                base_for_overflow.force_reconnect_due_to_backpressure("交易");
            });
        }

        {
            let runtime_clone = runtime.clone();
            let base_clone = Arc::clone(&base);
            tokio::spawn(async move {
                while let Some(data) = msg_rx.recv().await {
                    if let Some(aid) = data.get("aid").and_then(|aid| aid.as_str()) {
                        match aid {
                            "rtn_data" => {
                                if let Some(payload) = data.get("data") {
                                    if let Some(array) = payload.as_array() {
                                        let reconnect_index = array.iter().position(has_reconnect_notify);
                                        let (notifies, cleaned_data) =
                                            TqTradeWebsocket::separate_notifies(array.clone());
                                        debug!("notifies: {:?}", notifies);

                                        let callback = runtime_clone.on_notify.read().unwrap().clone();
                                        if let Some(callback) = callback {
                                            for notify in notifies {
                                                callback(notify);
                                            }
                                        }

                                        if let Some(index) = reconnect_index {
                                            runtime_clone.reconnect_pending.store(true, Ordering::SeqCst);
                                            *runtime_clone.reconnect_prev_positions.write().unwrap() =
                                                extract_trade_positions(&runtime_clone.dm);
                                            let mut diffs = runtime_clone.reconnect_diffs.write().unwrap();
                                            diffs.clear();
                                            diffs.extend(cleaned_data[index..].iter().cloned());
                                            let dm_temp = Arc::new(DataManager::new(
                                                HashMap::new(),
                                                DataManagerConfig::default(),
                                            ));
                                            dm_temp.merge_data(Value::Array(diffs.clone()), true, true);
                                            *runtime_clone.reconnect_dm.write().unwrap() = Some(Arc::clone(&dm_temp));
                                            let base_for_send = Arc::clone(&base_clone);
                                            let req_login = runtime_clone.req_login.read().unwrap().clone();
                                            let confirm_settlement =
                                                runtime_clone.confirm_settlement.read().unwrap().clone();
                                            tokio::spawn(async move {
                                                if let Some(login) = req_login {
                                                    let log_pack = super::sanitize_log_pack_value(&login);
                                                    debug!(pack = %log_pack, "resend request");
                                                    let _ = base_for_send.send(&login).await;
                                                }
                                                if let Some(confirm) = confirm_settlement {
                                                    let log_pack = super::sanitize_log_pack_value(&confirm);
                                                    debug!(pack = %log_pack, "resend request");
                                                    let _ = base_for_send.send(&confirm).await;
                                                }
                                                let _ = base_for_send.send_peek_message().await;
                                            });
                                        } else if runtime_clone.reconnect_pending.load(Ordering::SeqCst) {
                                            let mut diffs = runtime_clone.reconnect_diffs.write().unwrap();
                                            diffs.extend(cleaned_data.iter().cloned());
                                            if let Some(dm_temp) =
                                                runtime_clone.reconnect_dm.read().unwrap().as_ref().cloned()
                                            {
                                                dm_temp.merge_data(Value::Array(cleaned_data.clone()), true, true);
                                            }
                                        }

                                        if runtime_clone.reconnect_pending.load(Ordering::SeqCst) {
                                            let dm_temp = runtime_clone.reconnect_dm.read().unwrap().as_ref().cloned();
                                            let prev_positions =
                                                runtime_clone.reconnect_prev_positions.read().unwrap().clone();
                                            if let Some(dm_temp) = dm_temp {
                                                if let Some(removal_diffs) =
                                                    is_trade_reconnect_complete(&dm_temp, &prev_positions)
                                                {
                                                    let mut diffs = runtime_clone.reconnect_diffs.write().unwrap();
                                                    let mut pending = diffs.clone();
                                                    diffs.clear();
                                                    runtime_clone.reconnect_pending.store(false, Ordering::SeqCst);
                                                    *runtime_clone.reconnect_dm.write().unwrap() = None;
                                                    if !removal_diffs.is_empty() {
                                                        pending.extend(removal_diffs);
                                                    }
                                                    runtime_clone.dm.merge_data(Value::Array(pending), true, true);
                                                    debug!("data completed");
                                                } else {
                                                    debug!(pack = ?json!({"aid": "peek_message"}), "wait for data completed");
                                                    let base_for_peek = Arc::clone(&base_clone);
                                                    tokio::spawn(async move {
                                                        let _ = base_for_peek.send_peek_message().await;
                                                    });
                                                }
                                            }
                                            continue;
                                        }

                                        runtime_clone.dm.merge_data(Value::Array(cleaned_data), true, true);
                                    } else {
                                        runtime_clone.dm.merge_data(payload.clone(), true, true);
                                    }
                                }
                            }
                            "rtn_brokers" => {
                                debug!("收到期货公司列表");
                            }
                            "qry_settlement_info" => {
                                if let (Some(settlement_info), Some(user_name), Some(trading_day)) = (
                                    data.get("settlement_info").and_then(|value| value.as_str()),
                                    data.get("user_name").and_then(|value| value.as_str()),
                                    data.get("trading_day").and_then(|value| value.as_str()),
                                ) {
                                    debug!("收到结算单: user={}, trading_day={}", user_name, trading_day);
                                    let settlement = TqTradeWebsocket::parse_settlement_content(settlement_info);
                                    let settlement_data = json!({
                                        "trade": {
                                            user_name: {
                                                "his_settlements": {
                                                    trading_day: settlement
                                                }
                                            }
                                        }
                                    });
                                    runtime_clone.dm.merge_data(settlement_data, true, true);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        }

        {
            let backpressure = backpressure.clone();
            base.on_message(move |data: Value| {
                backpressure.enqueue(data);
            });
        }

        {
            let base_clone = Arc::clone(&base);
            let runtime_clone = runtime.clone();
            base.on_close(move || {
                runtime_clone.reconnect_pending.store(false, Ordering::SeqCst);
                runtime_clone.reconnect_diffs.write().unwrap().clear();
                *runtime_clone.reconnect_dm.write().unwrap() = None;
                runtime_clone.reconnect_prev_positions.write().unwrap().clear();
                let base_for_reconnect = Arc::clone(&base_clone);
                tokio::spawn(async move {
                    base_for_reconnect.reconnect().await;
                });
            });
        }

        {
            let base_clone = Arc::clone(&base);
            let runtime_clone = runtime.clone();
            base.on_open(move || {
                let base = Arc::clone(&base_clone);
                let login = runtime_clone.req_login.read().unwrap().clone();
                let confirm = runtime_clone.confirm_settlement.read().unwrap().clone();
                tokio::spawn(async move {
                    if let Some(login) = login {
                        let _ = base.send(&login).await;
                    }
                    if let Some(confirm) = confirm {
                        let _ = base.send(&confirm).await;
                    }
                    let _ = base.send_peek_message().await;
                });
            });
        }

        Self { base, runtime }
    }

    /// 分离通知
    ///
    /// 从 rtn_data 的 data 数组中提取通知，并返回清理后的数据
    fn separate_notifies(data: Vec<Value>) -> (Vec<crate::types::Notification>, Vec<Value>) {
        let mut notifies = Vec::new();
        let mut cleaned_data = Vec::new();

        for mut item in data {
            if let Some(obj) = item.as_object_mut()
                && let Some(notify_data) = obj.remove("notify")
                && let Some(notify_map) = notify_data.as_object()
            {
                for (_key, notify_value) in notify_map {
                    if let Some(notify) = notify_value.as_object() {
                        notifies.push(crate::types::Notification {
                            code: notify
                                .get("code")
                                .and_then(extract_notify_code)
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                            level: notify
                                .get("level")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                            r#type: notify
                                .get("type")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                            content: notify
                                .get("content")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                            bid: notify
                                .get("bid")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                            user_id: notify
                                .get("user_id")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string(),
                        });
                    }
                }
            }

            cleaned_data.push(item);
        }

        (notifies, cleaned_data)
    }

    fn parse_settlement_content(content: &str) -> Value {
        json!({
            "content": content,
            "parsed": false
        })
    }

    /// 注册通知回调
    pub fn on_notify<F>(&self, callback: F)
    where
        F: Fn(crate::types::Notification) + Send + Sync + 'static,
    {
        *self.runtime.on_notify.write().unwrap() = Some(Arc::new(callback));
    }

    pub fn on_error<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.base.on_error(callback);
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    /// 发送消息（重写以记录登录请求）
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let json_str = serde_json::to_string(obj).map_err(|source| TqError::Json {
            context: "序列化 websocket 消息失败".to_string(),
            source,
        })?;
        let value: Value = serde_json::from_str(&json_str).map_err(|source| TqError::Json {
            context: "解析 websocket 消息 JSON 失败".to_string(),
            source,
        })?;

        if let Some(aid) = value.get("aid").and_then(|aid| aid.as_str()) {
            if aid == "req_login" {
                let log_pack = super::sanitize_log_pack_value(&value);
                debug!(pack = %log_pack, "记录登录请求");
                *self.runtime.req_login.write().unwrap() = Some(value.clone());
            } else if aid == "confirm_settlement" {
                *self.runtime.confirm_settlement.write().unwrap() = Some(value.clone());
            }
        }

        self.base.send(&value).await
    }

    pub async fn send_critical<T: Serialize>(&self, obj: &T) -> Result<()> {
        let value = serde_json::to_value(obj).map_err(|source| TqError::Json {
            context: "序列化 websocket 消息失败".to_string(),
            source,
        })?;
        self.base.send_or_fail(&value).await
    }

    /// 检查是否就绪
    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}

#[cfg(test)]
impl TqTradeWebsocket {
    pub(crate) fn emit_notify_for_test(&self, notification: crate::types::Notification) {
        if let Some(callback) = self.runtime.on_notify.read().unwrap().clone() {
            callback(notification);
        }
    }

    pub(crate) fn emit_error_for_test(&self, message: String) {
        self.base.emit_error_for_test(message);
    }

    #[allow(dead_code)]
    pub(crate) fn force_send_failure_for_test(&self) {
        self.base.force_send_failure_for_test();
    }

    pub(crate) fn force_missing_io_actor_for_test(&self) {
        self.base.force_missing_io_actor_for_test();
    }

    pub(crate) async fn pending_queue_len_for_test(&self) -> usize {
        self.base.pending_queue_len_for_test().await
    }

    pub(crate) fn url_for_test(&self) -> String {
        self.base.url_for_test()
    }

    pub(crate) fn req_login_for_test(&self) -> Option<Value> {
        self.runtime.req_login.read().unwrap().clone()
    }

    pub(crate) fn confirm_settlement_for_test(&self) -> Option<Value> {
        self.runtime.confirm_settlement.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing::Level;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    struct SharedWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(Arc::clone(&self.0))
        }
    }

    impl Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn req_login_logs_are_redacted() {
        let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
        let ws = TqTradeWebsocket::new("wss://example.com".to_string(), dm, WebSocketConfig::default());
        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        ws.send(&json!({
            "aid": "req_login",
            "bid": "simnow",
            "user_name": "demo",
            "password": "super-secret"
        }))
        .await
        .unwrap();

        let logs = String::from_utf8(writer.0.lock().unwrap().clone()).unwrap();
        assert!(!logs.contains("super-secret"), "日志不应包含明文密码: {logs}");
        assert!(!logs.contains("\"password\""), "日志不应包含 password 字段: {logs}");
    }
}
