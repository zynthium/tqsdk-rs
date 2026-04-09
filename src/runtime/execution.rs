use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, broadcast};

use crate::trade_session::TradeSession;
use crate::types::{
    DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, OFFSET_OPEN, ORDER_STATUS_FINISHED, Order, Position, Trade,
};

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

#[derive(Debug)]
struct BacktestExecutionState {
    next_order_seq: usize,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    positions: HashMap<(String, String), Position>,
}

#[derive(Debug, Clone)]
pub struct BacktestExecutionAdapter {
    accounts: Vec<String>,
    state: Arc<Mutex<BacktestExecutionState>>,
    updates_tx: broadcast::Sender<String>,
}

impl BacktestExecutionAdapter {
    pub fn new(accounts: Vec<String>) -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            accounts,
            state: Arc::new(Mutex::new(BacktestExecutionState {
                next_order_seq: 0,
                orders: HashMap::new(),
                trades_by_order: HashMap::new(),
                positions: HashMap::new(),
            })),
            updates_tx,
        }
    }
}

#[async_trait]
impl ExecutionAdapter for BacktestExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }

    async fn insert_order(&self, account_key: &str, req: &InsertOrderRequest) -> RuntimeResult<String> {
        let mut state = self.state.lock().await;
        state.next_order_seq += 1;
        let order_id = format!("bt-order-{}", state.next_order_seq);
        let trade_id = format!("bt-trade-{}", state.next_order_seq);

        let order = backtest_order(account_key, &order_id, req);
        let trade = backtest_trade(account_key, &trade_id, &order_id, req);
        let symbol = req.symbol.clone();
        let position = state
            .positions
            .entry((account_key.to_string(), symbol.clone()))
            .or_insert_with(|| default_position(account_key, &symbol));
        apply_backtest_fill(position, &order, req.volume);

        state.orders.insert(order_id.clone(), order);
        state.trades_by_order.insert(order_id.clone(), vec![trade]);
        drop(state);

        let _ = self.updates_tx.send(order_id.clone());
        Ok(order_id)
    }

    async fn cancel_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let mut state = self.state.lock().await;
        let order = state
            .orders
            .get_mut(order_id)
            .ok_or_else(|| RuntimeError::OrderNotFound {
                account_key: account_key.to_string(),
                order_id: order_id.to_string(),
            })?;
        order.status = ORDER_STATUS_FINISHED.to_string();
        drop(state);

        let _ = self.updates_tx.send(order_id.to_string());
        Ok(())
    }

    async fn order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        self.state
            .lock()
            .await
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| RuntimeError::OrderNotFound {
                account_key: account_key.to_string(),
                order_id: order_id.to_string(),
            })
    }

    async fn trades_by_order(&self, _account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Ok(self
            .state
            .lock()
            .await
            .trades_by_order
            .get(order_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn position(&self, account_key: &str, symbol: &str) -> RuntimeResult<Position> {
        Ok(self
            .state
            .lock()
            .await
            .positions
            .get(&(account_key.to_string(), symbol.to_string()))
            .cloned()
            .unwrap_or_else(|| default_position(account_key, symbol)))
    }

    async fn wait_order_update(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        if let Some(order) = self.state.lock().await.orders.get(order_id).cloned()
            && order.status == ORDER_STATUS_FINISHED
        {
            return Ok(());
        }

        let mut rx = self.updates_tx.subscribe();
        loop {
            let updated = rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "backtest execution updates",
            })?;
            if updated == order_id {
                let _ = account_key;
                return Ok(());
            }
        }
    }
}

fn backtest_order(account_key: &str, order_id: &str, req: &InsertOrderRequest) -> Order {
    Order {
        seqno: 0,
        user_id: account_key.to_string(),
        order_id: order_id.to_string(),
        exchange_id: req.get_exchange_id(),
        instrument_id: req.get_instrument_id(),
        direction: req.direction.clone(),
        offset: req.offset.clone(),
        volume_orign: req.volume,
        price_type: req.price_type.clone(),
        limit_price: req.limit_price,
        time_condition: "GFD".to_string(),
        volume_condition: "ANY".to_string(),
        insert_date_time: 0,
        exchange_order_id: String::new(),
        status: ORDER_STATUS_FINISHED.to_string(),
        volume_left: 0,
        frozen_margin: 0.0,
        last_msg: String::new(),
        epoch: None,
    }
}

fn backtest_trade(account_key: &str, trade_id: &str, order_id: &str, req: &InsertOrderRequest) -> Trade {
    Trade {
        seqno: 0,
        user_id: account_key.to_string(),
        trade_id: trade_id.to_string(),
        exchange_id: req.get_exchange_id(),
        instrument_id: req.get_instrument_id(),
        order_id: order_id.to_string(),
        exchange_trade_id: String::new(),
        direction: req.direction.clone(),
        offset: req.offset.clone(),
        volume: req.volume,
        price: req.limit_price,
        trade_date_time: 0,
        commission: 0.0,
        epoch: None,
    }
}

fn default_position(account_key: &str, symbol: &str) -> Position {
    let (exchange_id, instrument_id) = match symbol.split_once('.') {
        Some((exchange_id, instrument_id)) => (exchange_id.to_string(), instrument_id.to_string()),
        None => (String::new(), symbol.to_string()),
    };

    Position {
        user_id: account_key.to_string(),
        exchange_id,
        instrument_id,
        volume_long_today: 0,
        volume_long_his: 0,
        volume_long: 0,
        volume_long_frozen_today: 0,
        volume_long_frozen_his: 0,
        volume_long_frozen: 0,
        volume_short_today: 0,
        volume_short_his: 0,
        volume_short: 0,
        volume_short_frozen_today: 0,
        volume_short_frozen_his: 0,
        volume_short_frozen: 0,
        volume_long_yd: 0,
        volume_short_yd: 0,
        pos_long_his: 0,
        pos_long_today: 0,
        pos_short_his: 0,
        pos_short_today: 0,
        open_price_long: 0.0,
        open_price_short: 0.0,
        open_cost_long: 0.0,
        open_cost_short: 0.0,
        position_price_long: 0.0,
        position_price_short: 0.0,
        position_cost_long: 0.0,
        position_cost_short: 0.0,
        last_price: 0.0,
        float_profit_long: 0.0,
        float_profit_short: 0.0,
        float_profit: 0.0,
        position_profit_long: 0.0,
        position_profit_short: 0.0,
        position_profit: 0.0,
        margin_long: 0.0,
        margin_short: 0.0,
        margin: 0.0,
        market_value_long: 0.0,
        market_value_short: 0.0,
        market_value: 0.0,
        epoch: None,
    }
}

fn apply_backtest_fill(position: &mut Position, order: &Order, filled: i64) {
    match (order.direction.as_str(), order.offset.as_str()) {
        (DIRECTION_BUY, OFFSET_OPEN) => {
            position.pos_long_today += filled;
            position.volume_long_today += filled;
            position.volume_long = position.volume_long_today + position.volume_long_his;
        }
        (DIRECTION_SELL, OFFSET_OPEN) => {
            position.pos_short_today += filled;
            position.volume_short_today += filled;
            position.volume_short = position.volume_short_today + position.volume_short_his;
        }
        (DIRECTION_BUY, _) => {
            let close_today = filled.min(position.pos_short_today);
            position.pos_short_today -= close_today;
            position.volume_short_today -= close_today;
            let close_his = filled - close_today;
            position.pos_short_his -= close_his;
            position.volume_short_his -= close_his;
            position.volume_short = position.volume_short_today + position.volume_short_his;
        }
        (DIRECTION_SELL, _) => {
            let close_today = filled.min(position.pos_long_today);
            position.pos_long_today -= close_today;
            position.volume_long_today -= close_today;
            let close_his = filled - close_today;
            position.pos_long_his -= close_his;
            position.volume_long_his -= close_his;
            position.volume_long = position.volume_long_today + position.volume_long_his;
        }
        _ => {}
    }
}
