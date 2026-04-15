use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::trade_session::{OrderEventStream, TradeEventStream, TradeOnlyEventStream, TradeSession};
use crate::types::{InsertOrderRequest, Order, Position, Quote, Trade};

use super::{RuntimeError, RuntimeResult};

#[async_trait]
pub trait ExecutionAdapter: Send + Sync {
    fn known_accounts(&self) -> Vec<String>;

    fn has_account(&self, account_key: &str) -> bool {
        self.known_accounts().iter().any(|candidate| candidate == account_key)
    }

    async fn insert_order(&self, _account_key: &str, _req: &InsertOrderRequest) -> RuntimeResult<String> {
        Err(RuntimeError::Unsupported("insert_order"))
    }

    async fn insert_order_with_quote_hint(
        &self,
        account_key: &str,
        req: &InsertOrderRequest,
        _quote: &Quote,
    ) -> RuntimeResult<String> {
        self.insert_order(account_key, req).await
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

    async fn trades_for_symbol(&self, _account_key: &str, _symbol: &str) -> RuntimeResult<Vec<Trade>> {
        Err(RuntimeError::Unsupported("trades_for_symbol"))
    }

    async fn position(&self, _account_key: &str, _symbol: &str) -> RuntimeResult<Position> {
        Err(RuntimeError::Unsupported("position"))
    }

    async fn wait_order_update(&self, _account_key: &str, _order_id: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported("wait_order_update"))
    }

    fn subscribe_events(&self, _account_key: &str) -> RuntimeResult<TradeEventStream> {
        Err(RuntimeError::Unsupported("subscribe_events"))
    }

    fn subscribe_order_events(&self, _account_key: &str) -> RuntimeResult<OrderEventStream> {
        Err(RuntimeError::Unsupported("subscribe_order_events"))
    }

    fn subscribe_trade_events(&self, _account_key: &str) -> RuntimeResult<TradeOnlyEventStream> {
        Err(RuntimeError::Unsupported("subscribe_trade_events"))
    }
}

#[derive(Default)]
pub struct LiveExecutionAdapter {
    trade_sessions: Arc<std::sync::RwLock<HashMap<String, Arc<TradeSession>>>>,
}

impl LiveExecutionAdapter {
    pub fn new(trade_sessions: Arc<std::sync::RwLock<HashMap<String, Arc<TradeSession>>>>) -> Self {
        Self { trade_sessions }
    }

    fn session_sync(&self, account_key: &str) -> RuntimeResult<Arc<TradeSession>> {
        let sessions = self.trade_sessions.read().unwrap();
        sessions
            .get(account_key)
            .cloned()
            .ok_or_else(|| RuntimeError::AccountNotFound {
                account_key: account_key.to_string(),
            })
    }

    async fn session(&self, account_key: &str) -> RuntimeResult<Arc<TradeSession>> {
        self.session_sync(account_key)
    }
}

#[async_trait]
impl ExecutionAdapter for LiveExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        self.trade_sessions.read().unwrap().keys().cloned().collect()
    }

    fn has_account(&self, account_key: &str) -> bool {
        self.trade_sessions.read().unwrap().contains_key(account_key)
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

    async fn trades_for_symbol(&self, account_key: &str, symbol: &str) -> RuntimeResult<Vec<Trade>> {
        let session = self.session(account_key).await?;
        let trades = session.get_trades().await?;
        let (exchange_id, instrument_id) = symbol
            .split_once('.')
            .ok_or(RuntimeError::Unsupported("invalid runtime symbol"))?;

        Ok(trades
            .into_values()
            .filter(|trade| trade.exchange_id == exchange_id && trade.instrument_id == instrument_id)
            .collect())
    }

    async fn position(&self, account_key: &str, symbol: &str) -> RuntimeResult<Position> {
        let session = self.session(account_key).await?;
        Ok(session.get_position(symbol).await?)
    }

    async fn wait_order_update(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let session = self.session(account_key).await?;
        session
            .wait_order_update_reliable(order_id)
            .await
            .map_err(RuntimeError::Tq)
    }

    fn subscribe_events(&self, account_key: &str) -> RuntimeResult<TradeEventStream> {
        Ok(self.session_sync(account_key)?.subscribe_events())
    }

    fn subscribe_order_events(&self, account_key: &str) -> RuntimeResult<OrderEventStream> {
        Ok(self.session_sync(account_key)?.subscribe_order_events())
    }

    fn subscribe_trade_events(&self, account_key: &str) -> RuntimeResult<TradeOnlyEventStream> {
        Ok(self.session_sync(account_key)?.subscribe_trade_events())
    }
}
