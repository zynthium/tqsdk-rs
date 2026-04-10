use super::parsing::parse_backtest_time;
use super::{BacktestConfig, BacktestEvent, BacktestHandle};
use crate::errors::{Result, TqError};
use crate::utils::{datetime_to_nanos, nanos_to_datetime};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

impl BacktestHandle {
    /// 初始化回测并注册数据推进监听
    pub fn new(
        dm: Arc<crate::datamanager::DataManager>,
        ws: Arc<crate::websocket::TqQuoteWebsocket>,
        config: BacktestConfig,
    ) -> Self {
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
        let data_cb_id = dm.on_data_register(move || {
            let _ = tx.try_send(());
        });

        BacktestHandle {
            dm,
            ws,
            rx,
            start_dt,
            end_dt,
            current_dt: Arc::new(tokio::sync::RwLock::new(start_dt)),
            data_cb_id: Some(data_cb_id),
        }
    }

    /// 推进回测并返回下一事件
    ///
    /// 若内部未收到数据更新，会触发 peek_message 并等待回测推进。
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

        let value = self
            .dm
            .get_by_path(&["_tqsdk_backtest"])
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

    /// 主动触发一次回测推进
    pub async fn peek(&self) -> Result<()> {
        self.ws.send(&json!({ "aid": "peek_message" })).await?;
        Ok(())
    }

    /// 获取当前回测时间
    pub async fn current_dt(&self) -> DateTime<Utc> {
        let current = self.current_dt.read().await;
        nanos_to_datetime(*current)
    }

    /// 获取回测开始时间
    pub fn start_dt(&self) -> DateTime<Utc> {
        nanos_to_datetime(self.start_dt)
    }

    /// 获取回测结束时间
    pub fn end_dt(&self) -> DateTime<Utc> {
        nanos_to_datetime(self.end_dt)
    }

    /// 获取内部数据管理器
    pub fn dm(&self) -> Arc<crate::datamanager::DataManager> {
        Arc::clone(&self.dm)
    }
}

impl Drop for BacktestHandle {
    fn drop(&mut self) {
        if let Some(id) = self.data_cb_id.take() {
            let _ = self.dm.off_data(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamanager::{DataManager, DataManagerConfig};
    use crate::marketdata::MarketDataState;
    use crate::websocket::WebSocketConfig;
    use chrono::Duration as ChronoDuration;
    use std::collections::HashMap;

    #[tokio::test]
    async fn backtest_handle_drop_should_unregister_data_callback() {
        let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
        let ws = Arc::new(crate::websocket::TqQuoteWebsocket::new(
            "wss://example.com".to_string(),
            Arc::clone(&dm),
            Arc::new(MarketDataState::default()),
            WebSocketConfig::default(),
        ));

        assert_eq!(dm.callback_count_for_test(), 0);
        {
            let now = Utc::now();
            let _handle = BacktestHandle::new(
                Arc::clone(&dm),
                ws,
                BacktestConfig::new(now, now + ChronoDuration::days(1)),
            );
            assert_eq!(dm.callback_count_for_test(), 1);
        }
        assert_eq!(dm.callback_count_for_test(), 0);
    }
}
