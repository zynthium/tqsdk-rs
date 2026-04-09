use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::trade_session::TradeSession;
use crate::types::{InsertOrderRequest, Order, Position, Trade};

use super::{RuntimeError, RuntimeResult};

#[async_trait]
pub trait ExecutionAdapter: Send + Sync {
    fn known_accounts(&self) -> Vec<String>;

    async fn insert_order(&self, _account_key: &str, _req: &InsertOrderRequest) -> RuntimeResult<String> {
        Err(RuntimeError::Unsupported("insert_order"))
    }

    async fn cancel_order(&self, _account_key: &str, _order_id: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported("cancel_order"))
    }

    async fn order(&self, _account_key: &str, _order_id: &str) -> RuntimeResult<Order> {
        Err(RuntimeError::Unsupported("order"))
    }

    async fn trades_by_order(&self, _account_key: &str, _order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Err(RuntimeError::Unsupported("trades_by_order"))
    }

    async fn position(&self, _account_key: &str, _symbol: &str) -> RuntimeResult<Position> {
        Err(RuntimeError::Unsupported("position"))
    }

    async fn wait_order_update(&self, _account_key: &str, _order_id: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported("wait_order_update"))
    }
}

#[derive(Default)]
pub struct LiveExecutionAdapter {
    trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>,
}

impl LiveExecutionAdapter {
    pub fn new(trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>) -> Self {
        Self { trade_sessions }
    }

    async fn session(&self, account_key: &str) -> RuntimeResult<Arc<TradeSession>> {
        let sessions = self.trade_sessions.read().await;
        sessions
            .get(account_key)
            .cloned()
            .ok_or_else(|| RuntimeError::AccountNotFound {
                account_key: account_key.to_string(),
            })
    }
}

#[async_trait]
impl ExecutionAdapter for LiveExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        match self.trade_sessions.try_read() {
            Ok(sessions) => sessions.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    async fn insert_order(&self, account_key: &str, req: &InsertOrderRequest) -> RuntimeResult<String> {
        let session = self.session(account_key).await?;
        Ok(session.insert_order(req).await?)
    }

    async fn cancel_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let session = self.session(account_key).await?;
        Ok(session.cancel_order(order_id).await?)
    }

    async fn order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        let session = self.session(account_key).await?;
        let orders = session.get_orders().await?;
        orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| RuntimeError::OrderNotFound {
                account_key: account_key.to_string(),
                order_id: order_id.to_string(),
            })
    }

    async fn trades_by_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        let session = self.session(account_key).await?;
        let trades = session.get_trades().await?;
        Ok(trades
            .into_values()
            .filter(|trade| trade.order_id == order_id)
            .collect())
    }

    async fn position(&self, account_key: &str, symbol: &str) -> RuntimeResult<Position> {
        let session = self.session(account_key).await?;
        Ok(session.get_position(symbol).await?)
    }

    async fn wait_order_update(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let session = self.session(account_key).await?;
        let order_rx = session.order_channel();
        let trade_rx = session.trade_channel();

        loop {
            tokio::select! {
                order = order_rx.recv() => {
                    let order = order.map_err(|_| RuntimeError::AdapterChannelClosed {
                        resource: "trade session order channel",
                    })?;
                    if order.order_id == order_id {
                        return Ok(());
                    }
                }
                trade = trade_rx.recv() => {
                    let trade = trade.map_err(|_| RuntimeError::AdapterChannelClosed {
                        resource: "trade session trade channel",
                    })?;
                    if trade.order_id == order_id {
                        return Ok(());
                    }
                }
            }
        }
    }
}
