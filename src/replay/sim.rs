use std::collections::HashMap;

use chrono::NaiveDate;

use crate::errors::{Result, TqError};
use crate::replay::{BacktestResult, DailySettlementLog, ReplayQuote};
use crate::types::{
    Account, DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, OFFSET_OPEN, ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED,
    Order, PRICE_TYPE_ANY, PRICE_TYPE_LIMIT, Position, Trade,
};

#[derive(Debug, Default)]
pub struct SimBroker {
    accounts: HashMap<String, Account>,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    positions: HashMap<(String, String), Position>,
    settlements: Vec<DailySettlementLog>,
    recent_trades: Vec<Trade>,
    all_trades: Vec<Trade>,
    next_order_seq: i64,
    next_trade_seq: i64,
    last_settled_day: Option<NaiveDate>,
}

impl SimBroker {
    pub fn new(account_keys: Vec<String>, initial_balance: f64) -> Self {
        let accounts = account_keys
            .into_iter()
            .map(|account_key| (account_key.clone(), default_account(&account_key, initial_balance)))
            .collect();

        Self {
            accounts,
            ..Self::default()
        }
    }

    pub fn insert_order(&mut self, account_key: &str, req: &InsertOrderRequest) -> Result<String> {
        self.ensure_account(account_key)?;

        self.next_order_seq += 1;
        let order_id = format!("sim-order-{}", self.next_order_seq);
        let order = Order {
            seqno: self.next_order_seq,
            user_id: account_key.to_string(),
            order_id: order_id.clone(),
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
            status: ORDER_STATUS_ALIVE.to_string(),
            volume_left: req.volume,
            frozen_margin: 0.0,
            last_msg: String::new(),
            epoch: None,
        };

        self.orders.insert(order_id.clone(), order);
        Ok(order_id)
    }

    pub fn apply_quote_path(&mut self, symbol: &str, path: &[ReplayQuote]) -> Result<()> {
        for quote in path {
            if quote.symbol != symbol {
                return Err(TqError::InvalidParameter(format!(
                    "quote symbol {} does not match path symbol {}",
                    quote.symbol, symbol
                )));
            }

            self.mark_positions(symbol, quote);

            let alive_order_ids = self
                .orders
                .iter()
                .filter(|(_, order)| {
                    order.status == ORDER_STATUS_ALIVE && order.volume_left > 0 && matches_symbol(order, symbol)
                })
                .map(|(order_id, _)| order_id.clone())
                .collect::<Vec<_>>();

            for order_id in alive_order_ids {
                let Some(outcome) = self.match_order(&order_id, quote)? else {
                    continue;
                };

                match outcome {
                    MatchOutcome::Filled { fill_price } => {
                        self.finish_fill(&order_id, fill_price, quote.datetime_nanos)?
                    }
                    MatchOutcome::Cancel => self.finish_without_fill(&order_id)?,
                }
            }
        }

        Ok(())
    }

    pub fn order(&self, account_key: &str, order_id: &str) -> Result<Order> {
        let order = self
            .orders
            .get(order_id)
            .cloned()
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
        ensure_account_owns_order(account_key, &order)?;
        Ok(order)
    }

    pub fn trades_by_order(&self, account_key: &str, order_id: &str) -> Result<Vec<Trade>> {
        let order = self
            .orders
            .get(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
        ensure_account_owns_order(account_key, order)?;
        Ok(self.trades_by_order.get(order_id).cloned().unwrap_or_default())
    }

    pub fn cancel_order(&mut self, account_key: &str, order_id: &str) -> Result<()> {
        let order = self
            .orders
            .get_mut(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
        ensure_account_owns_order(account_key, order)?;
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg = "cancelled by replay runtime".to_string();
        Ok(())
    }

    pub fn position(&self, account_key: &str, symbol: &str) -> Result<Position> {
        self.ensure_account(account_key)?;
        Ok(self
            .positions
            .get(&(account_key.to_string(), symbol.to_string()))
            .cloned()
            .unwrap_or_else(|| default_position(account_key, symbol)))
    }

    pub fn settle_day(&mut self, trading_day: NaiveDate) -> Result<Option<DailySettlementLog>> {
        if self.last_settled_day == Some(trading_day) {
            return Ok(None);
        }

        let account = self
            .accounts
            .values()
            .next()
            .cloned()
            .ok_or_else(|| TqError::TradeError("no replay accounts configured".to_string()))?;
        let positions = self.positions.values().cloned().collect::<Vec<_>>();
        let trades = self.recent_trades.clone();

        for position in self.positions.values_mut() {
            rollover_position(position);
        }

        let settlement = DailySettlementLog {
            trading_day,
            account,
            positions,
            trades,
        };

        self.settlements.push(settlement.clone());
        self.recent_trades.clear();
        self.last_settled_day = Some(trading_day);

        Ok(Some(settlement))
    }

    pub fn finish(&self) -> Result<BacktestResult> {
        let mut final_accounts = self.accounts.values().cloned().collect::<Vec<_>>();
        final_accounts.sort_by(|left, right| left.user_id.cmp(&right.user_id));

        let mut final_positions = self.positions.values().cloned().collect::<Vec<_>>();
        final_positions.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.exchange_id.cmp(&right.exchange_id))
                .then_with(|| left.instrument_id.cmp(&right.instrument_id))
        });

        Ok(BacktestResult {
            settlements: self.settlements.clone(),
            final_accounts,
            final_positions,
            trades: self.all_trades.clone(),
        })
    }

    fn ensure_account(&self, account_key: &str) -> Result<()> {
        if self.accounts.contains_key(account_key) {
            Ok(())
        } else {
            Err(TqError::TradeError(format!("unknown replay account: {account_key}")))
        }
    }

    fn mark_positions(&mut self, symbol: &str, quote: &ReplayQuote) {
        for ((_, position_symbol), position) in &mut self.positions {
            if position_symbol == symbol {
                position.last_price = quote.last_price;
            }
        }
    }

    fn match_order(&self, order_id: &str, quote: &ReplayQuote) -> Result<Option<MatchOutcome>> {
        let order = self
            .orders
            .get(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;

        match order.price_type.as_str() {
            PRICE_TYPE_LIMIT => Ok(match limit_fill_price(order, quote) {
                Some(fill_price) => Some(MatchOutcome::Filled { fill_price }),
                None => None,
            }),
            PRICE_TYPE_ANY => Ok(match market_fill_price(order, quote) {
                Some(fill_price) => Some(MatchOutcome::Filled { fill_price }),
                None => Some(MatchOutcome::Cancel),
            }),
            other => Err(TqError::TradeError(format!("unsupported replay price_type: {other}"))),
        }
    }

    fn finish_fill(&mut self, order_id: &str, fill_price: f64, trade_date_time: i64) -> Result<()> {
        let order = self
            .orders
            .get_mut(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
        let filled = order.volume_left;

        order.volume_left = 0;
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg.clear();

        self.next_trade_seq += 1;
        let trade = Trade {
            seqno: self.next_trade_seq,
            user_id: order.user_id.clone(),
            trade_id: format!("sim-trade-{}", self.next_trade_seq),
            exchange_id: order.exchange_id.clone(),
            instrument_id: order.instrument_id.clone(),
            order_id: order.order_id.clone(),
            exchange_trade_id: String::new(),
            direction: order.direction.clone(),
            offset: order.offset.clone(),
            volume: filled,
            price: fill_price,
            trade_date_time,
            commission: 0.0,
            epoch: None,
        };

        let symbol = format!("{}.{}", order.exchange_id, order.instrument_id);
        let position = self
            .positions
            .entry((order.user_id.clone(), symbol.clone()))
            .or_insert_with(|| default_position(&order.user_id, &symbol));
        apply_fill_to_position(position, order, filled, fill_price);

        self.trades_by_order
            .entry(order_id.to_string())
            .or_default()
            .push(trade.clone());
        self.all_trades.push(trade.clone());
        self.recent_trades.push(trade);

        Ok(())
    }

    fn finish_without_fill(&mut self, order_id: &str) -> Result<()> {
        let order = self
            .orders
            .get_mut(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg = "no opponent quote".to_string();
        Ok(())
    }
}

enum MatchOutcome {
    Filled { fill_price: f64 },
    Cancel,
}

fn limit_fill_price(order: &Order, quote: &ReplayQuote) -> Option<f64> {
    match order.direction.as_str() {
        DIRECTION_BUY if is_usable_quote(quote.ask_price1) => {
            (quote.ask_price1 <= order.limit_price).then_some(order.limit_price)
        }
        DIRECTION_SELL if is_usable_quote(quote.bid_price1) => {
            (quote.bid_price1 >= order.limit_price).then_some(order.limit_price)
        }
        _ => None,
    }
}

fn market_fill_price(order: &Order, quote: &ReplayQuote) -> Option<f64> {
    match order.direction.as_str() {
        DIRECTION_BUY => usable_quote_value(quote.ask_price1),
        DIRECTION_SELL => usable_quote_value(quote.bid_price1),
        _ => None,
    }
}

fn usable_quote_value(price: f64) -> Option<f64> {
    is_usable_quote(price).then_some(price)
}

fn is_usable_quote(price: f64) -> bool {
    price.is_finite()
}

fn matches_symbol(order: &Order, symbol: &str) -> bool {
    let order_symbol = format!("{}.{}", order.exchange_id, order.instrument_id);
    order_symbol == symbol
}

fn ensure_account_owns_order(account_key: &str, order: &Order) -> Result<()> {
    if order.user_id == account_key {
        Ok(())
    } else {
        Err(TqError::TradeError(format!(
            "order {} does not belong to account {}",
            order.order_id, account_key
        )))
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

fn default_account(account_key: &str, initial_balance: f64) -> Account {
    Account {
        user_id: account_key.to_string(),
        currency: "CNY".to_string(),
        available: initial_balance,
        balance: initial_balance,
        close_profit: 0.0,
        commission: 0.0,
        ctp_available: initial_balance,
        ctp_balance: initial_balance,
        deposit: 0.0,
        float_profit: 0.0,
        frozen_commission: 0.0,
        frozen_margin: 0.0,
        frozen_premium: 0.0,
        margin: 0.0,
        market_value: 0.0,
        position_profit: 0.0,
        pre_balance: initial_balance,
        premium: 0.0,
        risk_ratio: 0.0,
        static_balance: initial_balance,
        withdraw: 0.0,
        epoch: None,
    }
}

fn apply_fill_to_position(position: &mut Position, order: &Order, filled: i64, fill_price: f64) {
    match (order.direction.as_str(), order.offset.as_str()) {
        (DIRECTION_BUY, OFFSET_OPEN) => {
            position.pos_long_today += filled;
            position.volume_long_today += filled;
            position.volume_long = position.volume_long_today + position.volume_long_his;
            position.open_price_long = weighted_price(
                position.open_price_long,
                position.volume_long - filled,
                fill_price,
                filled,
            );
            position.position_price_long = position.open_price_long;
            position.open_cost_long += fill_price * filled as f64;
            position.position_cost_long += fill_price * filled as f64;
        }
        (DIRECTION_SELL, OFFSET_OPEN) => {
            position.pos_short_today += filled;
            position.volume_short_today += filled;
            position.volume_short = position.volume_short_today + position.volume_short_his;
            position.open_price_short = weighted_price(
                position.open_price_short,
                position.volume_short - filled,
                fill_price,
                filled,
            );
            position.position_price_short = position.open_price_short;
            position.open_cost_short += fill_price * filled as f64;
            position.position_cost_short += fill_price * filled as f64;
        }
        _ => {}
    }
}

fn weighted_price(current_price: f64, current_volume: i64, fill_price: f64, fill_volume: i64) -> f64 {
    let total_volume = current_volume + fill_volume;
    if total_volume <= 0 {
        0.0
    } else {
        ((current_price * current_volume as f64) + (fill_price * fill_volume as f64)) / total_volume as f64
    }
}

fn rollover_position(position: &mut Position) {
    position.volume_long_his += position.volume_long_today;
    position.volume_long_yd = position.volume_long_his;
    position.pos_long_his += position.pos_long_today;
    position.volume_long_today = 0;
    position.pos_long_today = 0;
    position.volume_long = position.volume_long_his;

    position.volume_short_his += position.volume_short_today;
    position.volume_short_yd = position.volume_short_his;
    position.pos_short_his += position.pos_short_today;
    position.volume_short_today = 0;
    position.pos_short_today = 0;
    position.volume_short = position.volume_short_his;
}
