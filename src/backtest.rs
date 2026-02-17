use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::utils::{datetime_to_nanos, nanos_to_datetime};
use crate::websocket::TqQuoteWebsocket;
use async_channel::Receiver;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct BacktestConfig {
    pub start_dt: DateTime<Utc>,
    pub end_dt: DateTime<Utc>,
}

impl BacktestConfig {
    pub fn new(start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Self {
        BacktestConfig { start_dt, end_dt }
    }
}

#[derive(Debug, Clone)]
pub struct BacktestTime {
    pub start_dt: i64,
    pub end_dt: i64,
    pub current_dt: i64,
}

#[derive(Debug, Clone)]
pub enum BacktestEvent {
    Tick { current_dt: DateTime<Utc> },
    Finished { current_dt: DateTime<Utc> },
}

pub struct BacktestHandle {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    rx: Receiver<()>,
    start_dt: i64,
    end_dt: i64,
    current_dt: Arc<RwLock<i64>>,
}

impl BacktestHandle {
    pub fn new(dm: Arc<DataManager>, ws: Arc<TqQuoteWebsocket>, config: BacktestConfig) -> Self {
        let start_dt = datetime_to_nanos(&config.start_dt);
        let end_dt = datetime_to_nanos(&config.end_dt);
        dm.merge_data(
            json!({
                "action": { "mode": "backtest" },
                "_tqsdk_backtest": {
                    "start_dt": start_dt,
                    "end_dt": end_dt,
                    "current_dt": start_dt
                }
            }),
            true,
            true,
        );

        let (tx, rx) = async_channel::bounded(1);
        dm.on_data(move || {
            let _ = tx.try_send(());
        });

        BacktestHandle {
            dm,
            ws,
            rx,
            start_dt,
            end_dt,
            current_dt: Arc::new(RwLock::new(start_dt)),
        }
    }

    pub async fn next(&self) -> Result<BacktestEvent> {
        if self.rx.try_recv().is_err() {
            loop {
                self.ws.send(&json!({ "aid": "peek_message" })).await?;

                match tokio::time::timeout(Duration::from_secs(30), self.rx.recv()).await {
                    Ok(Ok(_)) => {
                        break;
                    }
                    Ok(Err(e)) => {
                        return Err(TqError::InternalError(format!("回测接收失败: {}", e)));
                    }
                    Err(_) => {
                        warn!("Backtest timeout waiting for response, retry peek_message");
                    }
                }
            }
        }

        // info!("Received backtest update");

        let value = self.dm.get_by_path(&["_tqsdk_backtest"])
            .ok_or_else(|| TqError::DataNotFound("_tqsdk_backtest object not found".to_string()))?;

        if let Some(time) = parse_backtest_time(&value) {
            let mut current_guard = self.current_dt.write().await;
            *current_guard = time.current_dt;
            drop(current_guard);
            let dt = nanos_to_datetime(time.current_dt);
            if time.current_dt >= self.end_dt {
                return Ok(BacktestEvent::Finished { current_dt: dt });
            }
            return Ok(BacktestEvent::Tick { current_dt: dt });
        }

        Err(TqError::InternalError("Failed to parse backtest time".to_string()))
    }

    pub async fn peek(&self) -> Result<()> {
        self.ws.send(&json!({ "aid": "peek_message" })).await?;
        Ok(())
    }

    pub async fn current_dt(&self) -> DateTime<Utc> {
        let current = self.current_dt.read().await;
        nanos_to_datetime(*current)
    }

    pub fn start_dt(&self) -> DateTime<Utc> {
        nanos_to_datetime(self.start_dt)
    }

    pub fn end_dt(&self) -> DateTime<Utc> {
        nanos_to_datetime(self.end_dt)
    }

    pub fn dm(&self) -> Arc<DataManager> {
        Arc::clone(&self.dm)
    }
}

fn parse_backtest_time(value: &Value) -> Option<BacktestTime> {
    if let Value::Object(map) = value {
        let start_dt = value_to_i64(map.get("start_dt")?)?;
        let end_dt = value_to_i64(map.get("end_dt")?)?;
        let current_dt = value_to_i64(map.get("current_dt")?)?;
        return Some(BacktestTime {
            start_dt,
            end_dt,
            current_dt,
        });
    }
    None
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(num) => num
            .as_i64()
            .or_else(|| num.as_u64().map(|v| v as i64))
            .or_else(|| num.as_f64().map(|v| v as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}
