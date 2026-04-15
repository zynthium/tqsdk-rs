use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::replay::{InstrumentMetadata, ReplayQuote};
use crate::runtime::{ExecutionAdapter, MarketAdapter, RuntimeError, RuntimeResult};
use crate::trade_session::{OrderEventStream, TradeEventHub, TradeEventStream, TradeOnlyEventStream};
use crate::types::{InsertOrderRequest, ORDER_STATUS_FINISHED, Order, Position, Quote, Trade};

use super::session::ReplayBootstrapper;
use super::sim::SimBroker;

#[derive(Debug)]
pub(crate) struct ReplayMarketState {
    metadata: RwLock<HashMap<String, InstrumentMetadata>>,
    quotes: RwLock<HashMap<String, ReplayQuote>>,
    active_quotes: RwLock<HashSet<String>>,
    updates_tx: broadcast::Sender<String>,
}

impl ReplayMarketState {
    pub(crate) fn new() -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            metadata: RwLock::new(HashMap::new()),
            quotes: RwLock::new(HashMap::new()),
            active_quotes: RwLock::new(HashSet::new()),
            updates_tx,
        }
    }

    pub(crate) async fn register_symbol(&self, metadata: InstrumentMetadata) {
        self.metadata
            .write()
            .expect("replay market metadata lock poisoned")
            .insert(metadata.symbol.clone(), metadata);
    }

    pub(crate) async fn metadata_for(&self, symbol: &str) -> Option<InstrumentMetadata> {
        self.metadata
            .read()
            .expect("replay market metadata lock poisoned")
            .get(symbol)
            .cloned()
    }

    pub(crate) async fn all_metadata(&self) -> Vec<InstrumentMetadata> {
        self.metadata
            .read()
            .expect("replay market metadata lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub(crate) async fn update_quote(&self, quote: ReplayQuote) {
        let symbol = quote.symbol.clone();
        self.quotes
            .write()
            .expect("replay market quotes lock poisoned")
            .insert(symbol.clone(), quote);
        self.active_quotes
            .write()
            .expect("replay market active quote lock poisoned")
            .insert(symbol.clone());
        let _ = self.updates_tx.send(symbol);
    }

    pub(crate) async fn seed_quote(&self, quote: ReplayQuote) {
        let symbol = quote.symbol.clone();
        self.quotes
            .write()
            .expect("replay market quotes lock poisoned")
            .insert(symbol.clone(), quote);
        self.active_quotes
            .write()
            .expect("replay market active quote lock poisoned")
            .remove(&symbol);
    }

    pub(crate) async fn replay_quote(&self, symbol: &str) -> Option<ReplayQuote> {
        self.quotes
            .read()
            .expect("replay market quotes lock poisoned")
            .get(symbol)
            .cloned()
    }

    pub(crate) async fn active_replay_quote(&self, symbol: &str) -> Option<ReplayQuote> {
        if !self
            .active_quotes
            .read()
            .expect("replay market active quote lock poisoned")
            .contains(symbol)
        {
            return None;
        }
        self.replay_quote(symbol).await
    }

    async fn api_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        let metadata = self
            .metadata
            .read()
            .expect("replay market metadata lock poisoned")
            .get(symbol)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::Tq(crate::TqError::DataNotFound(format!(
                    "replay metadata not found: {symbol}"
                )))
            })?;
        let replay_quote = self
            .quotes
            .read()
            .expect("replay market quotes lock poisoned")
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
    trade_events: HashMap<String, Arc<TradeEventHub>>,
}

impl ReplayExecutionState {
    pub(crate) fn new(account_keys: &[String], broker: SimBroker) -> Self {
        let (updates_tx, _) = broadcast::channel(64);
        Self {
            broker: Mutex::new(broker),
            updates_tx,
            next_update_seq: AtomicU64::new(1),
            trade_events: account_keys
                .iter()
                .cloned()
                .map(|account_key| (account_key, Arc::new(TradeEventHub::new(8_192))))
                .collect(),
        }
    }

    pub(crate) async fn notify_update(&self) {
        let seq = self.next_update_seq.fetch_add(1, Ordering::Relaxed);
        let _ = self.updates_tx.send(seq);
    }

    pub(crate) fn current_update_seq(&self) -> u64 {
        self.next_update_seq.load(Ordering::Relaxed)
    }

    pub(crate) async fn finish(&self) -> crate::Result<crate::replay::BacktestResult> {
        self.broker.lock().expect("replay broker lock poisoned").finish()
    }

    pub(crate) async fn apply_quote_path(&self, symbol: &str, path: &[ReplayQuote]) -> crate::Result<()> {
        let (changed_orders, new_trades) = {
            let mut broker = self.broker.lock().expect("replay broker lock poisoned");
            let before_orders = broker.order_snapshots_for_symbol(symbol);
            let before_trade_ids = broker
                .trades_for_symbol(symbol)
                .into_iter()
                .map(|trade| trade.trade_id)
                .collect::<HashSet<_>>();
            broker.apply_quote_path(symbol, path)?;
            let after_orders = broker.order_snapshots_for_symbol(symbol);
            let after_trades = broker.trades_for_symbol(symbol);
            let changed_orders = changed_order_snapshots(&before_orders, &after_orders);
            let new_trades = after_trades
                .into_iter()
                .filter(|trade| !before_trade_ids.contains(&trade.trade_id))
                .collect::<Vec<_>>();
            (changed_orders, new_trades)
        };

        for (order_id, order) in changed_orders {
            let account_key = order.user_id.clone();
            self.publish_order(&account_key, order_id, order)
                .map_err(|err| crate::TqError::InternalError(err.to_string()))?;
        }
        for trade in new_trades {
            let account_key = trade.user_id.clone();
            let trade_id = trade.trade_id.clone();
            self.publish_trade(&account_key, trade_id, trade)
                .map_err(|err| crate::TqError::InternalError(err.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn register_symbol(&self, metadata: InstrumentMetadata) {
        self.broker
            .lock()
            .expect("replay broker lock poisoned")
            .register_symbol(metadata);
    }

    pub(crate) async fn settle_day(
        &self,
        trading_day: chrono::NaiveDate,
    ) -> crate::Result<Option<crate::replay::DailySettlementLog>> {
        self.broker
            .lock()
            .expect("replay broker lock poisoned")
            .settle_day(trading_day)
    }

    async fn order_update_signature(
        &self,
        account_key: &str,
        order_id: &str,
    ) -> RuntimeResult<ReplayOrderUpdateSignature> {
        let broker = self.broker.lock().expect("replay broker lock poisoned");
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

    fn trade_event_hub(&self, account_key: &str) -> RuntimeResult<Arc<TradeEventHub>> {
        self.trade_events
            .get(account_key)
            .cloned()
            .ok_or_else(|| RuntimeError::AccountNotFound {
                account_key: account_key.to_string(),
            })
    }

    pub(crate) fn subscribe_events(&self, account_key: &str) -> RuntimeResult<TradeEventStream> {
        Ok(self.trade_event_hub(account_key)?.subscribe_tail())
    }

    pub(crate) fn subscribe_order_events(&self, account_key: &str) -> RuntimeResult<OrderEventStream> {
        Ok(self.trade_event_hub(account_key)?.subscribe_orders())
    }

    pub(crate) fn subscribe_trade_events(&self, account_key: &str) -> RuntimeResult<TradeOnlyEventStream> {
        Ok(self.trade_event_hub(account_key)?.subscribe_trades())
    }

    fn publish_order(&self, account_key: &str, order_id: String, order: Order) -> RuntimeResult<()> {
        let hub = self.trade_event_hub(account_key)?;
        let _ = hub.publish_order(order_id, order);
        Ok(())
    }

    fn publish_trade(&self, account_key: &str, trade_id: String, trade: Trade) -> RuntimeResult<()> {
        let hub = self.trade_event_hub(account_key)?;
        let _ = hub.publish_trade(trade_id, trade);
        Ok(())
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

    async fn insert_order_internal(
        &self,
        account_key: &str,
        req: &InsertOrderRequest,
        quote_hint: Option<&Quote>,
    ) -> RuntimeResult<String> {
        let metadata = self.bootstrap.ensure_quote_driver(&req.symbol).await?;
        self.state.register_symbol(metadata.clone()).await;
        if !metadata.underlying_symbol.is_empty()
            && let Some(underlying_metadata) = self.market.metadata_for(&metadata.underlying_symbol).await
        {
            self.state.register_symbol(underlying_metadata).await;
        }

        let snapshot_quote = match quote_hint {
            Some(quote) => Some(replay_quote_from_api_quote(&req.symbol, quote)?),
            None => self.market.replay_quote(&req.symbol).await,
        };
        let match_quote = match quote_hint {
            Some(_) => snapshot_quote.clone(),
            None => self.market.active_replay_quote(&req.symbol).await,
        };
        let underlying_quote = if metadata.class.ends_with("OPTION") && !metadata.underlying_symbol.is_empty() {
            self.market.replay_quote(&metadata.underlying_symbol).await
        } else {
            None
        };

        let (order_id, submitted_order, matched_orders, new_trades) = {
            let mut broker = self.state.broker.lock().expect("replay broker lock poisoned");
            if let Some(underlying_quote) = underlying_quote.as_ref() {
                broker.sync_quote_snapshot(&metadata.underlying_symbol, underlying_quote.clone())?;
            }
            if let Some(snapshot_quote) = snapshot_quote.as_ref() {
                broker.sync_quote_snapshot(req.symbol.as_str(), snapshot_quote.clone())?;
            }
            let order_id = broker.insert_order(account_key, req)?;
            let submitted_order = broker.order(account_key, &order_id)?;

            let (matched_orders, new_trades) = if let Some(match_quote) = match_quote.as_ref() {
                let before_orders = broker.order_snapshots_for_symbol(&req.symbol);
                let before_trade_ids = broker
                    .trades_for_symbol(&req.symbol)
                    .into_iter()
                    .map(|trade| trade.trade_id)
                    .collect::<HashSet<_>>();
                broker.apply_quote_path(req.symbol.as_str(), std::slice::from_ref(match_quote))?;
                let after_orders = broker.order_snapshots_for_symbol(&req.symbol);
                let after_trades = broker.trades_for_symbol(&req.symbol);
                let matched_orders = changed_order_snapshots(&before_orders, &after_orders);
                let new_trades = after_trades
                    .into_iter()
                    .filter(|trade| !before_trade_ids.contains(&trade.trade_id))
                    .collect::<Vec<_>>();
                (matched_orders, new_trades)
            } else {
                (Vec::new(), Vec::new())
            };

            (order_id, submitted_order, matched_orders, new_trades)
        };

        self.state
            .publish_order(account_key, order_id.clone(), submitted_order)?;
        for (matched_order_id, matched_order) in matched_orders {
            self.state.publish_order(account_key, matched_order_id, matched_order)?;
        }
        for trade in new_trades {
            let trade_id = trade.trade_id.clone();
            self.state.publish_trade(account_key, trade_id, trade)?;
        }
        self.state.notify_update().await;
        Ok(order_id)
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
        self.insert_order_internal(account_key, req, None).await
    }

    async fn insert_order_with_quote_hint(
        &self,
        account_key: &str,
        req: &InsertOrderRequest,
        quote: &Quote,
    ) -> RuntimeResult<String> {
        self.insert_order_internal(account_key, req, Some(quote)).await
    }

    async fn cancel_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<()> {
        let order = {
            let mut broker = self.state.broker.lock().expect("replay broker lock poisoned");
            broker.cancel_order(account_key, order_id)?;
            broker.order(account_key, order_id)?
        };
        self.state.publish_order(account_key, order_id.to_string(), order)?;
        self.state.notify_update().await;
        Ok(())
    }

    async fn order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Order> {
        Ok(self
            .state
            .broker
            .lock()
            .expect("replay broker lock poisoned")
            .order(account_key, order_id)?)
    }

    async fn trades_by_order(&self, account_key: &str, order_id: &str) -> RuntimeResult<Vec<Trade>> {
        Ok(self
            .state
            .broker
            .lock()
            .expect("replay broker lock poisoned")
            .trades_by_order(account_key, order_id)?)
    }

    async fn position(&self, account_key: &str, symbol: &str) -> RuntimeResult<Position> {
        Ok(self
            .state
            .broker
            .lock()
            .expect("replay broker lock poisoned")
            .position(account_key, symbol)?)
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

    fn subscribe_events(&self, account_key: &str) -> RuntimeResult<TradeEventStream> {
        self.state.subscribe_events(account_key)
    }

    fn subscribe_order_events(&self, account_key: &str) -> RuntimeResult<OrderEventStream> {
        self.state.subscribe_order_events(account_key)
    }

    fn subscribe_trade_events(&self, account_key: &str) -> RuntimeResult<TradeOnlyEventStream> {
        self.state.subscribe_trade_events(account_key)
    }
}

fn changed_order_snapshots(before: &[(String, Order)], after: &[(String, Order)]) -> Vec<(String, Order)> {
    let before_map = before.iter().cloned().collect::<HashMap<_, _>>();
    after
        .iter()
        .filter(|(order_id, order)| before_map.get(order_id) != Some(order))
        .cloned()
        .collect()
}

fn replay_quote_from_api_quote(symbol: &str, quote: &Quote) -> RuntimeResult<ReplayQuote> {
    let parsed = NaiveDateTime::parse_from_str(&quote.datetime, "%Y-%m-%d %H:%M:%S%.f").map_err(|err| {
        RuntimeError::Tq(crate::TqError::InternalError(format!(
            "invalid replay quote datetime {}: {err}",
            quote.datetime
        )))
    })?;
    let cst = FixedOffset::east_opt(8 * 3600).ok_or_else(|| {
        RuntimeError::Tq(crate::TqError::InternalError(
            "invalid replay quote CST offset".to_string(),
        ))
    })?;
    let datetime_nanos = cst
        .from_local_datetime(&parsed)
        .single()
        .ok_or_else(|| {
            RuntimeError::Tq(crate::TqError::InternalError(format!(
                "invalid replay quote local datetime: {}",
                quote.datetime
            )))
        })?
        .timestamp_nanos_opt()
        .ok_or_else(|| {
            RuntimeError::Tq(crate::TqError::InternalError(format!(
                "invalid replay quote timestamp: {}",
                quote.datetime
            )))
        })?;

    Ok(ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos,
        last_price: quote.last_price,
        ask_price1: quote.ask_price1,
        ask_volume1: quote.ask_volume1,
        bid_price1: quote.bid_price1,
        bid_volume1: quote.bid_volume1,
        highest: quote.highest,
        lowest: quote.lowest,
        average: quote.average,
        volume: quote.volume,
        amount: quote.amount,
        open_interest: quote.open_interest,
        open_limit: quote.open_limit,
    })
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
        open_limit: replay_quote.open_limit,
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

#[cfg(test)]
mod tests {
    use super::{build_quote, replay_quote_from_api_quote};
    use crate::replay::InstrumentMetadata;
    use crate::types::Quote;

    #[test]
    fn replay_quote_roundtrip_preserves_open_limit() {
        let api_quote = serde_json::from_str::<Quote>(
            r#"{
                "instrument_id":"rb2605",
                "datetime":"2026-04-15 09:30:00.000000",
                "last_price":3210.0,
                "ask_price1":3211.0,
                "ask_volume1":4,
                "bid_price1":3209.0,
                "bid_volume1":3,
                "open_limit":18
            }"#,
        )
        .expect("Quote 解析失败");

        let replay_quote = replay_quote_from_api_quote("SHFE.rb2605", &api_quote).expect("ReplayQuote 构造失败");
        let rebuilt = build_quote(
            &InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                ..InstrumentMetadata::default()
            },
            &replay_quote,
        );
        let encoded = serde_json::to_value(&rebuilt).expect("Quote 序列化失败");

        assert_eq!(encoded.get("open_limit").and_then(|value| value.as_i64()), Some(18));
    }
}
