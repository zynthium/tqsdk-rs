use super::{TqWebsocket, WebSocketConfig, has_reconnect_notify};
use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
struct TradingStatusRuntime {
    dm: Arc<DataManager>,
    subscribe_trading_status: Arc<std::sync::RwLock<Option<Value>>>,
    option_underlyings: Arc<std::sync::RwLock<HashMap<String, String>>>,
}

pub struct TqTradingStatusWebsocket {
    base: Arc<TqWebsocket>,
    runtime: TradingStatusRuntime,
}

impl TqTradingStatusWebsocket {
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let base = Arc::new(TqWebsocket::new(url, config));
        let runtime = TradingStatusRuntime {
            dm,
            subscribe_trading_status: Arc::new(std::sync::RwLock::new(None)),
            option_underlyings: Arc::new(std::sync::RwLock::new(HashMap::new())),
        };

        {
            let runtime_clone = runtime.clone();
            let base_clone = Arc::clone(&base);
            base.on_message(move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|aid| aid.as_str())
                    && aid == "rtn_data"
                    && let Some(payload) = data.get("data").and_then(|payload| payload.as_array())
                {
                    if payload.iter().any(has_reconnect_notify) {
                        let sub = runtime_clone.subscribe_trading_status.read().unwrap().clone();
                        let base_for_send = Arc::clone(&base_clone);
                        tokio::spawn(async move {
                            if let Some(sub) = sub {
                                let _ = base_for_send.send(&sub).await;
                            }
                            let _ = base_for_send.send_peek_message().await;
                        });
                    }
                    let mut diffs = payload.clone();
                    let mut received = HashMap::<String, String>::new();
                    for diff in diffs.iter_mut() {
                        if let Some(map) = diff.get_mut("trading_status").and_then(|value| value.as_object_mut()) {
                            for (symbol, trading_status_value) in map.iter_mut() {
                                if let Some(trading_status_map) = trading_status_value.as_object_mut() {
                                    if !trading_status_map.contains_key("symbol") {
                                        trading_status_map.insert("symbol".to_string(), Value::String(symbol.clone()));
                                    }
                                    if let Some(status_value) = trading_status_map.get_mut("trade_status")
                                        && let Some(status) = status_value.as_str()
                                    {
                                        let normalized = if status == "AUCTIONORDERING" || status == "CONTINOUS" {
                                            status.to_string()
                                        } else {
                                            "NOTRADING".to_string()
                                        };
                                        *status_value = Value::String(normalized.clone());
                                        received.insert(symbol.clone(), normalized);
                                    }
                                }
                            }
                        }
                    }

                    let option_map = runtime_clone.option_underlyings.read().unwrap();
                    for (option, underlying) in option_map.iter() {
                        if let Some(status) = received.get(underlying) {
                            diffs.push(json!({
                                "trading_status": {
                                    option: {
                                        "symbol": option,
                                        "trade_status": status
                                    }
                                }
                            }));
                        }
                    }
                    runtime_clone.dm.merge_data(Value::Array(diffs), true, true);
                }
            });
        }

        {
            let base_clone = Arc::clone(&base);
            base.on_close(move || {
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
                let sub = runtime_clone.subscribe_trading_status.read().unwrap().clone();
                tokio::spawn(async move {
                    if let Some(sub) = sub {
                        let _ = base.send(&sub).await;
                    }
                    let _ = base.send_peek_message().await;
                });
            });
        }

        Self { base, runtime }
    }

    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let value = serde_json::to_value(obj).map_err(|source| TqError::Json {
            context: "序列化 websocket 消息失败".to_string(),
            source,
        })?;

        if let Some(aid) = value.get("aid").and_then(|aid| aid.as_str())
            && aid == "subscribe_trading_status"
        {
            let mut should_send = false;
            {
                let mut subscribe_guard = self.runtime.subscribe_trading_status.write().unwrap();
                if let Some(old_sub) = subscribe_guard.as_ref() {
                    let old_list = old_sub.get("ins_list");
                    let new_list = value.get("ins_list");
                    if old_list != new_list {
                        *subscribe_guard = Some(value.clone());
                        should_send = true;
                    }
                } else {
                    *subscribe_guard = Some(value.clone());
                    should_send = true;
                }
            }
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

        self.base.send(&value).await
    }

    pub fn update_option_underlyings(&self, mapping: HashMap<String, String>) {
        let mut guard = self.runtime.option_underlyings.write().unwrap();
        for (option, underlying) in mapping {
            guard.insert(option, underlying);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    pub(crate) fn message_queue_capacity(&self) -> usize {
        self.base.message_queue_capacity()
    }

    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}
