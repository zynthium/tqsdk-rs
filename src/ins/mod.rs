//! 合约与基础数据查询接口
//!
//! 提供合约列表、期权筛选、结算价、持仓排名、EDB 指标、交易日历与交易状态等能力。

mod parse;
mod query;
mod services;
mod trading_status;
mod validation;

#[cfg(test)]
mod tests;

use self::validation::{match_query_cache, validate_query_variables};
use crate::auth::Authenticator;
use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant, MissedTickBehavior};
use uuid::Uuid;

/// 合约查询与基础数据接口
pub struct InsAPI {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    trading_status_ws: Option<Arc<TqTradingStatusWebsocket>>,
    auth: Arc<RwLock<dyn Authenticator>>,
    trading_status_symbols: Arc<RwLock<HashMap<String, usize>>>,
    stock: bool,
    holiday_url: String,
}

impl InsAPI {
    /// 创建合约与基础数据查询接口
    ///
    /// `stock` 用于区分期货与股票行情系统。
    pub(crate) fn new(
        dm: Arc<DataManager>,
        ws: Arc<TqQuoteWebsocket>,
        trading_status_ws: Option<Arc<TqTradingStatusWebsocket>>,
        auth: Arc<RwLock<dyn Authenticator>>,
        stock: bool,
        holiday_url: String,
    ) -> Self {
        Self {
            dm,
            ws,
            trading_status_ws,
            auth,
            trading_status_symbols: Arc::new(RwLock::new(HashMap::new())),
            stock,
            holiday_url,
        }
    }

    /// 关闭交易状态通道
    pub async fn close(&self) -> Result<()> {
        if let Some(ws) = &self.trading_status_ws {
            ws.close().await?;
        }
        Ok(())
    }

    async fn send_ins_query(
        &self,
        query: String,
        variables: Option<Value>,
        query_id: Option<String>,
        timeout_secs: u64,
    ) -> Result<Value> {
        let id = query_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        if let Some(vars) = variables.as_ref() {
            validate_query_variables(vars)?;
        }

        let query_value = json!(query);
        let variables_value = variables.unwrap_or_else(|| Value::Object(Map::new()));

        if let Some(Value::Object(symbols)) = self.dm.get_by_path(&["symbols"]) {
            for symbol in symbols.values() {
                if let Value::Object(symbol_obj) = symbol
                    && match_query_cache(symbol_obj, &query_value, &variables_value)
                {
                    return Ok(Value::Object(symbol_obj.clone()));
                }
            }
        }

        let mut req = Map::new();
        req.insert("aid".to_string(), json!("ins_query"));
        req.insert("query_id".to_string(), json!(id));
        req.insert("query".to_string(), query_value);
        if !matches!(variables_value, Value::Object(ref map) if map.is_empty()) {
            req.insert("variables".to_string(), variables_value);
        }

        self.ws.send(&Value::Object(req)).await?;
        self.ws.send(&json!({"aid": "peek_message"})).await?;

        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let watch_path = vec!["symbols".to_string(), id.clone()];
        let (watch_id, rx) = self.dm.watch_register(watch_path.clone());
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            if let Some(val) = self.dm.get_by_path(&["symbols", &id]) {
                let _ = self.dm.unwatch_by_id(&watch_path, watch_id);
                return Ok(val);
            }
            if start.elapsed() >= timeout {
                let _ = self.dm.unwatch_by_id(&watch_path, watch_id);
                return Err(TqError::Timeout);
            }
            tokio::select! {
                _ = rx.recv() => {}
                _ = interval.tick() => {
                    let _ = self.ws.send(&json!({"aid": "peek_message"})).await;
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn trading_status_ref_count_for_test(&self, symbol: &str) -> usize {
        let guard = self.trading_status_symbols.read().await;
        guard.get(symbol).copied().unwrap_or(0)
    }
}
