use super::{
    BackpressureState, TqWebsocket, WebSocketConfig, derive_message_backlog_max,
    has_reconnect_notify, is_md_reconnect_complete,
};
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, trace};

#[derive(Clone)]
struct QuoteRuntime {
    dm: Arc<DataManager>,
    subscribe_quote: Arc<std::sync::RwLock<Option<Value>>>,
    quote_subscriptions: Arc<std::sync::RwLock<HashMap<String, HashSet<String>>>>,
    charts: Arc<std::sync::RwLock<HashMap<String, Value>>>,
    pending_ins_query: Arc<std::sync::RwLock<HashMap<String, Value>>>,
    login_ready: Arc<AtomicBool>,
    reconnect_pending: Arc<AtomicBool>,
    reconnect_diffs: Arc<std::sync::RwLock<Vec<Value>>>,
    reconnect_dm: Arc<std::sync::RwLock<Option<Arc<DataManager>>>>,
}

/// 行情 WebSocket
pub struct TqQuoteWebsocket {
    base: Arc<TqWebsocket>,
    quote_subscribe_only_add: bool,
    runtime: QuoteRuntime,
}

impl TqQuoteWebsocket {
    /// 创建行情 WebSocket
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let quote_subscribe_only_add = config.quote_subscribe_only_add;
        let message_backlog_max = derive_message_backlog_max(
            config.message_queue_capacity,
            config.message_backlog_warn_step,
        );
        let base = Arc::new(TqWebsocket::new(url, config.clone()));
        let runtime = QuoteRuntime {
            dm,
            subscribe_quote: Arc::new(std::sync::RwLock::new(None)),
            quote_subscriptions: Arc::new(std::sync::RwLock::new(HashMap::new())),
            charts: Arc::new(std::sync::RwLock::new(HashMap::new())),
            pending_ins_query: Arc::new(std::sync::RwLock::new(HashMap::new())),
            login_ready: Arc::new(AtomicBool::new(false)),
            reconnect_pending: Arc::new(AtomicBool::new(false)),
            reconnect_diffs: Arc::new(std::sync::RwLock::new(Vec::new())),
            reconnect_dm: Arc::new(std::sync::RwLock::new(None)),
        };

        let (msg_tx, mut msg_rx) =
            tokio::sync::mpsc::channel::<Value>(config.message_queue_capacity);
        let backpressure = BackpressureState::new(
            msg_tx,
            message_backlog_max,
            config.message_backlog_warn_step,
            config.message_batch_max,
            "行情",
        );

        {
            let base_clone = Arc::clone(&base);
            let runtime_clone = runtime.clone();
            tokio::spawn(async move {
                while let Some(data) = msg_rx.recv().await {
                    if let Some(aid) = data.get("aid").and_then(|aid| aid.as_str()) {
                        match aid {
                            "rtn_data" => {
                                if let Some(payload) = data.get("data") {
                                    if let Some(array) = payload.as_array() {
                                        for item in array {
                                            if let Some(symbols) = item.get("symbols")
                                                && let Some(obj) = symbols.as_object()
                                            {
                                                let mut pending_guard = runtime_clone
                                                    .pending_ins_query
                                                    .write()
                                                    .unwrap();
                                                for (query_id, value) in obj {
                                                    if !value.is_null() {
                                                        pending_guard.remove(query_id);
                                                    }
                                                }
                                            }
                                        }

                                        let reconnect_index =
                                            array.iter().position(has_reconnect_notify);
                                        if let Some(index) = reconnect_index {
                                            runtime_clone
                                                .reconnect_pending
                                                .store(true, Ordering::SeqCst);
                                            let mut diffs =
                                                runtime_clone.reconnect_diffs.write().unwrap();
                                            diffs.clear();
                                            diffs.extend(array[index..].iter().cloned());
                                            let dm_temp = Arc::new(DataManager::new(
                                                HashMap::new(),
                                                DataManagerConfig::default(),
                                            ));
                                            dm_temp.merge_data(
                                                Value::Array(diffs.clone()),
                                                true,
                                                true,
                                            );
                                            *runtime_clone.reconnect_dm.write().unwrap() =
                                                Some(Arc::clone(&dm_temp));
                                            let sub = runtime_clone
                                                .subscribe_quote
                                                .read()
                                                .unwrap()
                                                .clone();
                                            let charts =
                                                runtime_clone.charts.read().unwrap().clone();
                                            let base_for_send = Arc::clone(&base_clone);
                                            tokio::spawn(async move {
                                                if let Some(sub) = sub {
                                                    debug!(pack = ?sub, "resend request");
                                                    let _ = base_for_send.send(&sub).await;
                                                }
                                                for chart in charts.values() {
                                                    if let Some(view_width) = chart
                                                        .get("view_width")
                                                        .and_then(|view_width| view_width.as_f64())
                                                        && view_width > 0.0
                                                    {
                                                        debug!(pack = ?chart, "resend request");
                                                        let _ = base_for_send.send(chart).await;
                                                    }
                                                }
                                                let _ = base_for_send.send_peek_message().await;
                                            });
                                        } else if runtime_clone
                                            .reconnect_pending
                                            .load(Ordering::SeqCst)
                                        {
                                            let mut diffs =
                                                runtime_clone.reconnect_diffs.write().unwrap();
                                            diffs.extend(array.iter().cloned());
                                            if let Some(dm_temp) = runtime_clone
                                                .reconnect_dm
                                                .read()
                                                .unwrap()
                                                .as_ref()
                                                .cloned()
                                            {
                                                dm_temp.merge_data(
                                                    Value::Array(array.clone()),
                                                    true,
                                                    true,
                                                );
                                            }
                                        }

                                        if runtime_clone.reconnect_pending.load(Ordering::SeqCst) {
                                            let dm_temp = runtime_clone
                                                .reconnect_dm
                                                .read()
                                                .unwrap()
                                                .as_ref()
                                                .cloned();
                                            let charts_snapshot =
                                                runtime_clone.charts.read().unwrap().clone();
                                            let subscribe_snapshot = runtime_clone
                                                .subscribe_quote
                                                .read()
                                                .unwrap()
                                                .clone();
                                            if let Some(dm_temp) = dm_temp {
                                                if is_md_reconnect_complete(
                                                    &dm_temp,
                                                    &charts_snapshot,
                                                    &subscribe_snapshot,
                                                ) {
                                                    let mut diffs = runtime_clone
                                                        .reconnect_diffs
                                                        .write()
                                                        .unwrap();
                                                    let pending = diffs.clone();
                                                    diffs.clear();
                                                    runtime_clone
                                                        .reconnect_pending
                                                        .store(false, Ordering::SeqCst);
                                                    *runtime_clone.reconnect_dm.write().unwrap() =
                                                        None;
                                                    runtime_clone.dm.merge_data(
                                                        Value::Array(pending),
                                                        true,
                                                        true,
                                                    );
                                                    debug!("data completed");
                                                } else {
                                                    debug!(pack = ?json!({"aid": "peek_message"}), "wait for data completed");
                                                    let base_for_peek = Arc::clone(&base_clone);
                                                    tokio::spawn(async move {
                                                        let _ =
                                                            base_for_peek.send_peek_message().await;
                                                    });
                                                }
                                            }
                                            continue;
                                        }
                                    }
                                    runtime_clone.dm.merge_data(payload.clone(), true, true);
                                }
                            }
                            "rsp_login" => {
                                runtime_clone.login_ready.store(true, Ordering::SeqCst);
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
                runtime_clone
                    .reconnect_pending
                    .store(false, Ordering::SeqCst);
                runtime_clone.reconnect_diffs.write().unwrap().clear();
                *runtime_clone.reconnect_dm.write().unwrap() = None;
                let has_quote_interest = runtime_clone
                    .subscribe_quote
                    .read()
                    .unwrap()
                    .as_ref()
                    .and_then(|value| value.get("ins_list").and_then(|ins_list| ins_list.as_str()))
                    .map(|ins_list| !ins_list.trim().is_empty())
                    .unwrap_or(false);
                let has_chart_interest =
                    runtime_clone.charts.read().unwrap().values().any(|chart| {
                        let width_ok = chart
                            .get("view_width")
                            .and_then(|view_width| view_width.as_f64())
                            .map(|view_width| view_width > 0.0)
                            .unwrap_or(false);
                        let symbols_ok = chart
                            .get("ins_list")
                            .and_then(|ins_list| ins_list.as_str())
                            .map(|ins_list| !ins_list.trim().is_empty())
                            .unwrap_or(false);
                        width_ok && symbols_ok
                    });
                let has_pending_query = !runtime_clone.pending_ins_query.read().unwrap().is_empty();
                let should_reconnect =
                    has_quote_interest || has_chart_interest || has_pending_query;
                if !should_reconnect {
                    info!("无活跃订阅意图，跳过自动重连");
                    return;
                }
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
                debug!("WebSocket 连接建立，重新发送订阅和图表请求");
                let base = Arc::clone(&base_clone);
                let sub = runtime_clone.subscribe_quote.read().unwrap().clone();
                let charts = runtime_clone.charts.read().unwrap().clone();
                let pending_queries = runtime_clone.pending_ins_query.read().unwrap().clone();
                tokio::spawn(async move {
                    if let Some(sub) = sub {
                        debug!("重新发送订阅: {:?}", sub);
                        let _ = base.send(&sub).await;
                    }
                    for (id, chart) in charts {
                        debug!("重新发送图表: {} -> {:?}", id, chart);
                        let _ = base.send(&chart).await;
                    }
                    for (id, query) in pending_queries {
                        debug!("重新发送合约查询: {} -> {:?}", id, query);
                        let _ = base.send(&query).await;
                    }
                    let _ = base.send_peek_message().await;
                });
            });
        }

        Self {
            base,
            quote_subscribe_only_add,
            runtime,
        }
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    pub async fn update_quote_subscription(
        &self,
        subscription_id: &str,
        symbols: HashSet<String>,
    ) -> Result<()> {
        {
            let mut guard = self.runtime.quote_subscriptions.write().unwrap();
            guard.insert(subscription_id.to_string(), symbols);
        }
        self.sync_quote_subscriptions().await
    }

    pub async fn remove_quote_subscription(&self, subscription_id: &str) -> Result<()> {
        {
            let mut guard = self.runtime.quote_subscriptions.write().unwrap();
            guard.remove(subscription_id);
        }
        self.sync_quote_subscriptions().await
    }

    async fn sync_quote_subscriptions(&self) -> Result<()> {
        let mut all_symbols: Vec<String> = {
            let guard = self.runtime.quote_subscriptions.read().unwrap();
            guard
                .values()
                .flat_map(|symbols| symbols.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        all_symbols.sort();
        let req = json!({
            "aid": "subscribe_quote",
            "ins_list": all_symbols.join(",")
        });
        self.send(&req).await
    }

    /// 发送消息（重写以记录订阅和图表请求）
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let mut value = serde_json::to_value(obj).map_err(|source| TqError::Json {
            context: "序列化 websocket 消息失败".to_string(),
            source,
        })?;

        if let Some(aid) = value.get("aid").and_then(|aid| aid.as_str()) {
            match aid {
                "subscribe_quote" => {
                    if self.quote_subscribe_only_add {
                        let old_ins_list = self
                            .runtime
                            .subscribe_quote
                            .read()
                            .unwrap()
                            .as_ref()
                            .and_then(|value| value.get("ins_list"))
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let new_ins_list = value
                            .get("ins_list")
                            .and_then(|ins_list| ins_list.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut merged = old_ins_list
                            .split(',')
                            .chain(new_ins_list.split(','))
                            .map(str::trim)
                            .filter(|symbol| !symbol.is_empty())
                            .map(|symbol| symbol.to_string())
                            .collect::<HashSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>();
                        merged.sort();
                        value["ins_list"] = Value::String(merged.join(","));
                    }

                    let should_send = {
                        let mut subscribe_guard = self.runtime.subscribe_quote.write().unwrap();
                        let mut should = false;

                        if let Some(old_sub) = subscribe_guard.as_ref() {
                            let old_list = old_sub.get("ins_list");
                            let new_list = value.get("ins_list");
                            if old_list != new_list {
                                debug!("订阅列表变化，更新订阅");
                                *subscribe_guard = Some(value.clone());
                                should = true;
                            } else {
                                debug!("订阅列表未变化，跳过");
                            }
                        } else {
                            debug!("首次订阅");
                            *subscribe_guard = Some(value.clone());
                            should = true;
                        }
                        should
                    };

                    if should_send {
                        let res = self.base.send(&value).await;
                        if res.is_ok() {
                            let _ = self.base.send_peek_message().await;
                        }
                        return res;
                    }
                    let _ = self.base.send_peek_message().await;
                    return Ok(());
                }
                "set_chart" => {
                    if let Some(chart_id) =
                        value.get("chart_id").and_then(|chart_id| chart_id.as_str())
                    {
                        {
                            let mut charts_guard = self.runtime.charts.write().unwrap();
                            let empty_ins_list = value
                                .get("ins_list")
                                .and_then(|ins_list| ins_list.as_str())
                                .map(|ins_list| ins_list.trim().is_empty())
                                .unwrap_or(false);
                            let width_is_zero = value
                                .get("view_width")
                                .and_then(|view_width| view_width.as_f64())
                                .map(|view_width| view_width == 0.0)
                                .unwrap_or(false);
                            if empty_ins_list || width_is_zero {
                                trace!("删除图表: {}", chart_id);
                                charts_guard.remove(chart_id);
                            } else {
                                trace!("保存图表请求: {}", chart_id);
                                charts_guard.insert(chart_id.to_string(), value.clone());
                            }
                        }

                        let res = self.base.send(&value).await;
                        if res.is_ok() {
                            let _ = self.base.send_peek_message().await;
                        }
                        return res;
                    }
                }
                "ins_query" => {
                    if let Some(query_id) =
                        value.get("query_id").and_then(|query_id| query_id.as_str())
                    {
                        self.runtime
                            .pending_ins_query
                            .write()
                            .unwrap()
                            .insert(query_id.to_string(), value.clone());
                    }
                    let res = self.base.send(&value).await;
                    if res.is_ok() {
                        let _ = self.base.send_peek_message().await;
                    }
                    return res;
                }
                _ => {}
            }
        }

        self.base.send(&value).await
    }

    /// 检查是否就绪
    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    pub fn is_logged_in(&self) -> bool {
        self.runtime.login_ready.load(Ordering::SeqCst)
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}

#[cfg(test)]
impl TqQuoteWebsocket {
    pub(crate) fn force_send_failure_for_test(&self) {
        self.base.force_send_failure_for_test();
    }
}
