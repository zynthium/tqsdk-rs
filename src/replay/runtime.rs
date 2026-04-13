use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::replay::{InstrumentMetadata, ReplayQuote};
use crate::runtime::{ExecutionAdapter, MarketAdapter, RuntimeError, RuntimeResult};
use crate::types::{InsertOrderRequest, Order, Position, Quote, Trade, ORDER_STATUS_FINISHED};

use super::session::ReplayBootstrapper;
use super::sim::SimBroker;

#[derive(Debug)]
pub(crate) struct ReplayMarketState {
    metadata: RwLock<HashMap<String, InstrumentMetadata>>,
    quotes: RwLock<HashMap<String, ReplayQuote>>,
    updates_tx: broadcast::Sender<String>,
}

impl ReplayMarketState {
    pub(crate) fn new() -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            metadata: RwLock::new(HashMap::new()),
            quotes: RwLock::new(HashMap::new()),
            updates_tx,
        }
    }

    pub(crate) async fn register_symbol(&self, metadata: InstrumentMetadata) {
        self.metadata.write().await.insert(metadata.symbol.clone(), metadata);
    }

    pub(crate) async fn metadata_for(&self, symbol: &str) -> Option<InstrumentMetadata> {
        self.metadata.read().await.get(symbol).cloned()
    }

    pub(crate) async fn all_metadata(&self) -> Vec<InstrumentMetadata> {
        self.metadata.read().await.values().cloned().collect()
    }

    pub(crate) async fn update_quote(&self, quote: ReplayQuote) {
        let symbol = quote.symbol.clone();
        self.quotes.write().await.insert(symbol.clone(), quote);
        let _ = self.updates_tx.send(symbol);
    }

    pub(crate) async fn replay_quote(&self, symbol: &str) -> Option<ReplayQuote> {
        self.quotes.read().await.get(symbol).cloned()
    }

    async fn api_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        let metadata = self.metadata.read().await.get(symbol).cloned().ok_or_else(|| {
            RuntimeError::Tq(crate::TqError::DataNotFound(format!(
                "replay metadata not found: {symbol}"
            )))
        })?;
        let replay_quote = self
            .quotes
            .read()
            .await
            .get(symbol)
            .cloned()
            .unwrap_or_else(|| ReplayQuote {
                symbol: symbol.to_string(),
                ..ReplayQuote::default()
            });
        Ok(build_quote(&metadata, &replay_quote))
    }
}

impl Default for ReplayMarketState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub(crate) struct ReplayExecutionState {
    broker: Mutex<SimBroker>,
    updates_tx: broadcast::Sender<u64>,
    next_update_seq: AtomicU64,
}

impl ReplayExecutionState {
    pub(crate) fn new(broker: SimBroker) -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            broker: Mutex::new(broker),
            updates_tx,
            next_update_seq: AtomicU64::new(1),
        }
    }

    pub(crate) async fn notify_update(&self) {
        let seq = self.next_update_seq.fetch_add(1, Ordering::Relaxed);
        let _ = self.updates_tx.send(seq);
    }

    pub(crate) async fn finish(&self) -> crate::Result<crate::replay::BacktestResult> {
        self.broker.lock().await.finish()
    }

    pub(crate) async fn apply_quote_path(&self, symbol: &str, path: &[ReplayQuote]) -> crate::Result<()> {
        self.broker.lock().await.apply_quote_path(symbol, path)
    }

    pub(crate) async fn register_symbol(&self, metadata: InstrumentMetadata) {
        self.broker.lock().await.register_symbol(metadata);
    }

    pub(crate) async fn settle_day(
        &self,
        trading_day: chrono::NaiveDate,
    ) -> crate::Result<Option<crate::replay::DailySettlementLog>> {
        self.broker.lock().await.settle_day(trading_day)
    }

    async fn order_update_signature(
        &self,
        account_key: &str,
        order_id: &str,
    ) -> RuntimeResult<ReplayOrderUpdateSignature> {
        let broker = self.broker.lock().await;
        let order = broker.order(account_key, order_id)?;
        let traded_volume = broker
            .trades_by_order(account_key, order_id)?
            .iter()
            .map(|trade| trade.volume)
            .sum();

        Ok(ReplayOrderUpdateSignature {
            status: order.status,
            volume_left: order.volume_left,
            traded_volume,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayOrderUpdateSignature {
    status: String,
    volume_left: i64,
    traded_volume: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplayMarketAdapter {
    state: Arc<ReplayMarketState>,
}

impl ReplayMarketAdapter {
    pub(crate) fn new(state: Arc<ReplayMarketState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl MarketAdapter for ReplayMarketAdapter {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        self.state.api_quote(symbol).await
    }

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()> {
        let mut rx = self.state.updates_tx.subscribe();
        loop {
            let updated = rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "replay quote updates",
            })?;
            if updated == symbol {
                return Ok(());
            }
        }
    }

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>> {
        Ok(self
            .state
            .metadata_for(symbol)
            .await
            .map(|metadata| metadata.trading_time)
            .filter(|value| !value.is_null()))
    }
}

#[derive(Clone)]
pub(crate) struct ReplayExecutionAdapter {
    accounts: Vec<String>,
    state: Arc<ReplayExecutionState>,
    market: Arc<ReplayMarketState>,
    bootstrap: ReplayBootstrapper,
}

impl ReplayExecutionAdapter {
    pub(crate) fn new(
        accounts: Vec<String>,
        state: Arc<ReplayExecutionState>,
        market: Arc<ReplayMarketState>,
        bootstrap: ReplayBootstrapper,
    ) -> Self {
        Self {
            accounts,
            state,
            market,
            bootstrap,
        }
    }
}

#[async_trait]
impl ExecutionAdapter for ReplayExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        self.accounts.clone()
    }

    fn has_account(&self, account_key: &str) -> bool {
        self.accounts.iter().any(|candidate| candidate == account_key)
    }

    async fn insert_order(&self, account_key: &str, req: &InsertOrderRequest) -> RuntimeResult<String> {
        let metadata = self.bootstrap.ensure_quote_driver(&req.symbol).await?;
        self.state.register_symbol(metadata.clone()).await;
        if !metadata.underlying_symbol.is_empty()
            && let Some(underlying_metadata) = self.market.metadata_for(&metadata.underlying_symbol).await
        {
            self.state.register_symbol(underlying_metadata).await;
        }

        let current_quote = self.market.replay_quote(&req.symbol).await;
        let underlying_quote = if metadata.class.ends_with("OPTION") && !metadata.underlying_symbol.is_empty() {
            self.market.replay_quote(&metadata.underlying_symbol).await
        } else {
            None
        };

        let order_id = {
            let mut broker = self.state.broker.lock().await;
            if let Some(underlying_quote) = underlying_quote.as_ref() {
                broker.sync_quote_snapshot(&metadata.underlying_symbol, underlying_quote.clone())?;
            }
            if let Some(current_quote) = current_quote.as_ref() {
                broker.sync_quote_snapshot(req.symbol.as_str(), current_quote.clone())?;
            }
            broker.insert_order(account_key, req)?
        };
        self.state.notify_update().await;
        Ok(order_id)
    }

    async fn cancel_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        self.state.broker.lock().await.cancel_order(account_key, order_id)?;
        self.state.notify_update().await;
        Ok(())
    }

    async fn order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        Ok(self.state.broker.lock().await.order(account_key, order_id)?)
    }

    async fn trades_by_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Ok(self.state.broker.lock().await.trades_by_order(account_key, order_id)?)
    }

    async fn position(&self, account_key: &str, symbol: &str) -> RuntimeResult<Position> {
        Ok(self.state.broker.lock().await.position(account_key, symbol)?)
    }

    async fn wait_order_update(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let mut rx = self.state.updates_tx.subscribe();
        let observed = self.state.order_update_signature(account_key, order_id).await?;
        if observed.status == ORDER_STATUS_FINISHED {
            return Ok(());
        }

        loop {
            rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "replay order updates",
            })?;

            let current = self.state.order_update_signature(account_key, order_id).await?;
            if current != observed {
                return Ok(());
            }
        }
    }
}

fn build_quote(metadata: &InstrumentMetadata, replay_quote: &ReplayQuote) -> Quote {
    Quote {
        instrument_id: metadata.instrument_id.clone(),
        datetime: format_quote_datetime(replay_quote.datetime_nanos),
        last_price: replay_quote.last_price,
        ask_price1: replay_quote.ask_price1,
        ask_volume1: replay_quote.ask_volume1,
        bid_price1: replay_quote.bid_price1,
        bid_volume1: replay_quote.bid_volume1,
        highest: replay_quote.highest,
        lowest: replay_quote.lowest,
        open: replay_quote.last_price,
        close: replay_quote.last_price,
        average: replay_quote.average,
        volume: replay_quote.volume,
        amount: replay_quote.amount,
        open_interest: replay_quote.open_interest,
        margin: metadata.margin,
        commission: metadata.commission,
        class: metadata.class.clone(),
        exchange_id: metadata.exchange_id.clone(),
        underlying_symbol: metadata.underlying_symbol.clone(),
        strike_price: metadata.strike_price,
        volume_multiple: metadata.volume_multiple,
        price_tick: metadata.price_tick,
        open_min_market_order_volume: metadata.open_min_market_order_volume,
        open_min_limit_order_volume: metadata.open_min_limit_order_volume,
        ..Quote::default()
    }
}

fn format_quote_datetime(datetime_nanos: i64) -> String {
    DateTime::<Utc>::from_timestamp_nanos(datetime_nanos)
        .with_timezone(&FixedOffset::east_opt(8 * 3600).expect("CST offset must be valid"))
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string()
}
