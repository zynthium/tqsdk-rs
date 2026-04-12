use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde_json::Value;

use crate::errors::{Result, TqError};
use crate::types::{
    Account, DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, OFFSET_CLOSE, OFFSET_CLOSETODAY, OFFSET_OPEN,
    ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, PRICE_TYPE_ANY, PRICE_TYPE_LIMIT, Position, Trade,
};

use super::types::{BacktestResult, DailySettlementLog, InstrumentMetadata, ReplayQuote};

#[derive(Debug, Default)]
pub struct SimBroker {
    accounts: HashMap<String, Account>,
    orders: HashMap<String, Order>,
    trades_by_order: HashMap<String, Vec<Trade>>,
    positions: HashMap<(String, String), Position>,
    metadata: HashMap<String, InstrumentMetadata>,
    current_quotes: HashMap<String, ReplayQuote>,
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

    pub fn register_symbol(&mut self, metadata: InstrumentMetadata) {
        self.metadata.insert(metadata.symbol.clone(), metadata);
    }

    pub fn insert_order(&mut self, account_key: &str, req: &InsertOrderRequest) -> Result<String> {
        self.ensure_account(account_key)?;

        self.next_order_seq += 1;
        let order_id = format!("sim-order-{}", self.next_order_seq);
        let symbol = req.symbol.clone();
        let metadata = self
            .metadata
            .get(&symbol)
            .cloned()
            .ok_or_else(|| TqError::TradeError(format!("replay symbol metadata not found: {symbol}")))?;
        let last_price = current_quote_last_price(&self.current_quotes, &symbol);

        self.ensure_position_exists(account_key, &symbol, last_price);

        let mut order = Order {
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
            insert_date_time: self
                .current_quotes
                .get(&symbol)
                .map(|quote| quote.datetime_nanos)
                .unwrap_or(0),
            exchange_order_id: order_id.clone(),
            status: ORDER_STATUS_ALIVE.to_string(),
            volume_left: req.volume,
            frozen_margin: 0.0,
            last_msg: "报单成功".to_string(),
            epoch: None,
        };

        validate_order_support(&mut order, &metadata);
        if order.status == ORDER_STATUS_ALIVE
            && let Some(current_quote) = self.current_quotes.get(&symbol)
            && !metadata.trading_time.is_null()
            && !is_in_trading_time(&metadata.trading_time, current_quote.datetime_nanos)?
        {
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.last_msg = "下单失败, 不在可交易时间段内".to_string();
        }
        if order.status == ORDER_STATUS_ALIVE {
            if order.offset == OFFSET_OPEN {
                let available = self
                    .accounts
                    .get(account_key)
                    .ok_or_else(|| TqError::TradeError(format!("unknown replay account: {account_key}")))?
                    .available;
                freeze_open_order(&mut order, &metadata, available);
            } else {
                let position = self.position_mut(account_key, &symbol, last_price);
                freeze_close_order(position, &mut order);
            }
        }

        self.orders.insert(order_id.clone(), order);
        self.recompute_account(account_key)?;
        Ok(order_id)
    }

    pub fn apply_quote_path(&mut self, symbol: &str, path: &[ReplayQuote]) -> Result<()> {
        let metadata = self
            .metadata
            .get(symbol)
            .cloned()
            .ok_or_else(|| TqError::TradeError(format!("replay symbol metadata not found: {symbol}")))?;

        for quote in path {
            if quote.symbol != symbol {
                return Err(TqError::InvalidParameter(format!(
                    "quote symbol {} does not match path symbol {}",
                    quote.symbol, symbol
                )));
            }

            self.current_quotes.insert(symbol.to_string(), quote.clone());

            let alive_order_ids = self
                .orders
                .iter()
                .filter(|(_, order)| {
                    order.status == ORDER_STATUS_ALIVE && order.volume_left > 0 && matches_symbol(order, symbol)
                })
                .map(|(order_id, _)| order_id.clone())
                .collect::<Vec<_>>();

            for order_id in alive_order_ids {
                let Some(outcome) = self.match_order(&order_id, &metadata, quote)? else {
                    continue;
                };

                match outcome {
                    MatchOutcome::Filled { fill_price } => self.finish_fill(&order_id, &metadata, quote, fill_price)?,
                    MatchOutcome::Cancel => self.finish_without_fill(&order_id, "市价指令剩余撤销")?,
                }
            }

            self.recompute_symbol_state(symbol)?;
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
        let snapshot = {
            let order = self
                .orders
                .get_mut(order_id)
                .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
            ensure_account_owns_order(account_key, order)?;
            if order.status != ORDER_STATUS_ALIVE {
                return Ok(());
            }
            let snapshot = order.clone();
            order.frozen_margin = 0.0;
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.last_msg = "已撤单".to_string();
            snapshot
        };

        self.release_order_resources(&snapshot)?;
        self.recompute_symbol_state(&order_symbol(&snapshot))?;
        Ok(())
    }

    pub fn position(&self, account_key: &str, symbol: &str) -> Result<Position> {
        self.ensure_account(account_key)?;
        Ok(self
            .positions
            .get(&(account_key.to_string(), symbol.to_string()))
            .cloned()
            .unwrap_or_else(|| {
                default_position(
                    account_key,
                    symbol,
                    current_quote_last_price(&self.current_quotes, symbol),
                )
            }))
    }

    pub fn settle_day(&mut self, trading_day: NaiveDate) -> Result<Option<DailySettlementLog>> {
        if self.last_settled_day == Some(trading_day) {
            return Ok(None);
        }

        self.recompute_all()?;

        let settlement = DailySettlementLog {
            trading_day,
            account: self.primary_account_snapshot()?,
            positions: self.sorted_positions_snapshot(),
            trades: self.recent_trades.clone(),
        };

        let alive_order_ids = self
            .orders
            .iter()
            .filter(|(_, order)| order.status == ORDER_STATUS_ALIVE && order.volume_left > 0)
            .map(|(order_id, _)| order_id.clone())
            .collect::<Vec<_>>();
        for order_id in alive_order_ids {
            let snapshot = {
                let order = self
                    .orders
                    .get_mut(&order_id)
                    .ok_or_else(|| TqError::OrderNotFound(order_id.clone()))?;
                let snapshot = order.clone();
                order.frozen_margin = 0.0;
                order.status = ORDER_STATUS_FINISHED.to_string();
                order.last_msg = "交易日结束，自动撤销当日有效的委托单（GFD）".to_string();
                snapshot
            };
            self.release_order_resources(&snapshot)?;
        }

        for account in self.accounts.values_mut() {
            account.pre_balance = account.balance - account.market_value;
            account.static_balance = account.pre_balance;
            account.close_profit = 0.0;
            account.commission = 0.0;
            account.premium = 0.0;
            account.frozen_margin = 0.0;
            account.frozen_premium = 0.0;
        }

        for ((_, symbol), position) in &mut self.positions {
            let metadata = self
                .metadata
                .get(symbol)
                .ok_or_else(|| TqError::TradeError(format!("replay symbol metadata not found: {symbol}")))?;
            let volume_multiple = metadata.volume_multiple as f64;

            position.volume_long_frozen_today = 0;
            position.volume_long_frozen_his = 0;
            position.volume_short_frozen_today = 0;
            position.volume_short_frozen_his = 0;
            position.volume_long_today = 0;
            position.volume_long_his = position.volume_long;
            position.volume_short_today = 0;
            position.volume_short_his = position.volume_short;
            position.position_cost_long = position.last_price * position.volume_long as f64 * volume_multiple;
            position.position_cost_short = position.last_price * position.volume_short as f64 * volume_multiple;
            refresh_position_volume_fields(position);
        }

        self.recompute_all()?;
        self.settlements.push(settlement.clone());
        self.recent_trades.clear();
        self.last_settled_day = Some(trading_day);

        Ok(Some(settlement))
    }

    pub fn finish(&self) -> Result<BacktestResult> {
        Ok(BacktestResult {
            settlements: self.settlements.clone(),
            final_accounts: self.sorted_accounts_snapshot(),
            final_positions: self.sorted_positions_snapshot(),
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

    fn ensure_position_exists(&mut self, account_key: &str, symbol: &str, last_price: f64) {
        self.positions
            .entry((account_key.to_string(), symbol.to_string()))
            .or_insert_with(|| default_position(account_key, symbol, last_price));
    }

    fn position_mut(&mut self, account_key: &str, symbol: &str, last_price: f64) -> &mut Position {
        self.positions
            .entry((account_key.to_string(), symbol.to_string()))
            .or_insert_with(|| default_position(account_key, symbol, last_price))
    }

    fn match_order(
        &self,
        order_id: &str,
        metadata: &InstrumentMetadata,
        quote: &ReplayQuote,
    ) -> Result<Option<MatchOutcome>> {
        let order = self
            .orders
            .get(order_id)
            .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;

        match order.price_type.as_str() {
            PRICE_TYPE_LIMIT => {
                Ok(limit_fill_price(order, metadata, quote).map(|fill_price| MatchOutcome::Filled { fill_price }))
            }
            PRICE_TYPE_ANY => Ok(match market_fill_price(order, metadata, quote) {
                Some(fill_price) => Some(MatchOutcome::Filled { fill_price }),
                None => Some(MatchOutcome::Cancel),
            }),
            other => Err(TqError::TradeError(format!("unsupported replay price_type: {other}"))),
        }
    }

    fn finish_fill(
        &mut self,
        order_id: &str,
        metadata: &InstrumentMetadata,
        quote: &ReplayQuote,
        fill_price: f64,
    ) -> Result<()> {
        let order = {
            let order = self
                .orders
                .get_mut(order_id)
                .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
            let snapshot = order.clone();
            order.frozen_margin = 0.0;
            order.volume_left = 0;
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.last_msg = "全部成交".to_string();
            snapshot
        };

        let symbol = order_symbol(&order);
        let last_price = current_quote_last_price(&self.current_quotes, &symbol);
        self.ensure_position_exists(&order.user_id, &symbol, last_price);

        let volume_multiple = metadata.volume_multiple as f64;
        let commission = order.volume_left as f64 * metadata.commission.max(0.0);
        let close_profit = {
            let position = self.position_mut(&order.user_id, &symbol, last_price);
            if order.offset == OFFSET_OPEN {
                apply_open_fill_raw(position, &order, fill_price, volume_multiple);
                0.0
            } else {
                let close_profit = close_profit(position, &order, fill_price, volume_multiple);
                release_close_order_resources(position, &order);
                apply_close_fill_raw(position, &order, volume_multiple);
                close_profit
            }
        };

        let account = self
            .accounts
            .get_mut(&order.user_id)
            .ok_or_else(|| TqError::TradeError(format!("unknown replay account: {}", order.user_id)))?;
        account.close_profit += close_profit;
        account.commission += commission;

        self.next_trade_seq += 1;
        let trade = Trade {
            seqno: self.next_trade_seq,
            user_id: order.user_id.clone(),
            trade_id: format!("{}|{}", order.order_id, order.volume_left),
            exchange_id: order.exchange_id.clone(),
            instrument_id: order.instrument_id.clone(),
            order_id: order.order_id.clone(),
            exchange_trade_id: format!("{}|{}", order.order_id, order.volume_left),
            direction: order.direction.clone(),
            offset: order.offset.clone(),
            volume: order.volume_left,
            price: fill_price,
            trade_date_time: quote.datetime_nanos,
            commission,
            epoch: None,
        };

        self.trades_by_order
            .entry(order_id.to_string())
            .or_default()
            .push(trade.clone());
        self.all_trades.push(trade.clone());
        self.recent_trades.push(trade);

        Ok(())
    }

    fn finish_without_fill(&mut self, order_id: &str, last_msg: &str) -> Result<()> {
        let snapshot = {
            let order = self
                .orders
                .get_mut(order_id)
                .ok_or_else(|| TqError::OrderNotFound(order_id.to_string()))?;
            let snapshot = order.clone();
            order.frozen_margin = 0.0;
            order.status = ORDER_STATUS_FINISHED.to_string();
            order.last_msg = last_msg.to_string();
            snapshot
        };

        self.release_order_resources(&snapshot)?;
        Ok(())
    }

    fn release_order_resources(&mut self, order: &Order) -> Result<()> {
        if order.offset == OFFSET_OPEN {
            return Ok(());
        }

        let symbol = order_symbol(order);
        let last_price = current_quote_last_price(&self.current_quotes, &symbol);
        let position = self.position_mut(&order.user_id, &symbol, last_price);
        release_close_order_resources(position, order);
        Ok(())
    }

    fn recompute_symbol_state(&mut self, symbol: &str) -> Result<()> {
        let mut account_keys = HashSet::new();
        let position_keys = self
            .positions
            .keys()
            .filter(|(_, position_symbol)| position_symbol == symbol)
            .cloned()
            .collect::<Vec<_>>();

        for (account_key, position_symbol) in &position_keys {
            account_keys.insert(account_key.clone());
            self.recompute_position_metrics(account_key, position_symbol)?;
        }

        for order in self.orders.values() {
            if matches_symbol(order, symbol) {
                account_keys.insert(order.user_id.clone());
            }
        }

        for account_key in account_keys {
            self.recompute_account(&account_key)?;
        }

        Ok(())
    }

    fn recompute_position_metrics(&mut self, account_key: &str, symbol: &str) -> Result<()> {
        let metadata = self
            .metadata
            .get(symbol)
            .cloned()
            .ok_or_else(|| TqError::TradeError(format!("replay symbol metadata not found: {symbol}")))?;
        let key = (account_key.to_string(), symbol.to_string());
        let position = self
            .positions
            .get_mut(&key)
            .ok_or_else(|| TqError::TradeError(format!("missing replay position for {account_key}:{symbol}")))?;
        let last_price = self
            .current_quotes
            .get(symbol)
            .map(|quote| quote.last_price)
            .unwrap_or(position.last_price);
        recompute_position_metrics(position, &metadata, last_price);
        Ok(())
    }

    fn recompute_account(&mut self, account_key: &str) -> Result<()> {
        let position_totals = self
            .positions
            .iter()
            .filter(|((user_id, _), _)| user_id == account_key)
            .map(|(_, position)| position)
            .fold(AccountPositionTotals::default(), |mut totals, position| {
                totals.float_profit += position.float_profit;
                totals.position_profit += position.position_profit;
                totals.margin += position.margin;
                totals.market_value += position.market_value;
                totals
            });
        let frozen_margin = self
            .orders
            .values()
            .filter(|order| {
                order.user_id == account_key && order.status == ORDER_STATUS_ALIVE && order.offset == OFFSET_OPEN
            })
            .map(|order| order.frozen_margin)
            .sum::<f64>();

        let account = self
            .accounts
            .get_mut(account_key)
            .ok_or_else(|| TqError::TradeError(format!("unknown replay account: {account_key}")))?;
        account.float_profit = position_totals.float_profit;
        account.position_profit = position_totals.position_profit;
        account.margin = position_totals.margin;
        account.market_value = position_totals.market_value;
        account.frozen_margin = frozen_margin;
        account.frozen_premium = 0.0;
        account.balance = account.static_balance + account.close_profit - account.commission
            + account.premium
            + account.position_profit
            + account.market_value;
        account.available = account.static_balance + account.close_profit - account.commission
            + account.premium
            + account.position_profit
            - account.margin
            - account.frozen_margin
            - account.frozen_premium;
        account.risk_ratio = if account.balance.abs() > f64::EPSILON {
            account.margin / account.balance
        } else {
            0.0
        };
        account.ctp_balance = account.balance;
        account.ctp_available = account.available;
        Ok(())
    }

    fn recompute_all(&mut self) -> Result<()> {
        let position_keys = self.positions.keys().cloned().collect::<Vec<_>>();
        for (account_key, symbol) in &position_keys {
            self.recompute_position_metrics(account_key, symbol)?;
        }

        let account_keys = self.accounts.keys().cloned().collect::<Vec<_>>();
        for account_key in &account_keys {
            self.recompute_account(account_key)?;
        }
        Ok(())
    }

    fn primary_account_snapshot(&self) -> Result<Account> {
        self.sorted_accounts_snapshot()
            .into_iter()
            .next()
            .ok_or_else(|| TqError::TradeError("no replay accounts configured".to_string()))
    }

    fn sorted_accounts_snapshot(&self) -> Vec<Account> {
        let mut final_accounts = self.accounts.values().cloned().collect::<Vec<_>>();
        final_accounts.sort_by(|left, right| left.user_id.cmp(&right.user_id));
        final_accounts
    }

    fn sorted_positions_snapshot(&self) -> Vec<Position> {
        let mut final_positions = self.positions.values().cloned().collect::<Vec<_>>();
        final_positions.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.exchange_id.cmp(&right.exchange_id))
                .then_with(|| left.instrument_id.cmp(&right.instrument_id))
        });
        final_positions
    }
}

#[derive(Default)]
struct AccountPositionTotals {
    float_profit: f64,
    position_profit: f64,
    margin: f64,
    market_value: f64,
}

enum MatchOutcome {
    Filled { fill_price: f64 },
    Cancel,
}

fn validate_order_support(order: &mut Order, metadata: &InstrumentMetadata) {
    let supported = metadata.volume_multiple > 0
        && metadata.margin.is_finite()
        && metadata.commission.is_finite()
        && !metadata.class.ends_with("OPTION");
    if !supported {
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg = "不支持的合约类型，ReplaySession 目前只支持期货回放交易".to_string();
    }
}

fn freeze_open_order(order: &mut Order, metadata: &InstrumentMetadata, available: f64) {
    let frozen_margin = order.volume_orign as f64 * metadata.margin.max(0.0);
    if frozen_margin > available {
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg = "开仓资金不足".to_string();
        return;
    }

    order.frozen_margin = frozen_margin;
}

fn freeze_close_order(position: &mut Position, order: &mut Order) {
    match order.exchange_id.as_str() {
        "SHFE" | "INE" => match (order.direction.as_str(), order.offset.as_str()) {
            (DIRECTION_BUY, OFFSET_CLOSETODAY) => {
                if position.volume_short_today - position.volume_short_frozen_today < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平今仓手数不足".to_string();
                    return;
                }
                position.volume_short_frozen_today += order.volume_orign;
            }
            (DIRECTION_BUY, OFFSET_CLOSE) => {
                if position.volume_short_his - position.volume_short_frozen_his < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平昨仓手数不足".to_string();
                    return;
                }
                position.volume_short_frozen_his += order.volume_orign;
            }
            (DIRECTION_SELL, OFFSET_CLOSETODAY) => {
                if position.volume_long_today - position.volume_long_frozen_today < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平今仓手数不足".to_string();
                    return;
                }
                position.volume_long_frozen_today += order.volume_orign;
            }
            (DIRECTION_SELL, OFFSET_CLOSE) => {
                if position.volume_long_his - position.volume_long_frozen_his < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平昨仓手数不足".to_string();
                    return;
                }
                position.volume_long_frozen_his += order.volume_orign;
            }
            _ => {}
        },
        _ => match order.direction.as_str() {
            DIRECTION_BUY => {
                if position.volume_short - position.volume_short_frozen < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平仓手数不足".to_string();
                    return;
                }
                let available_his = position.volume_short_his - position.volume_short_frozen_his;
                let freeze_his = available_his.min(order.volume_orign);
                position.volume_short_frozen_his += freeze_his;
                position.volume_short_frozen_today += order.volume_orign - freeze_his;
            }
            DIRECTION_SELL => {
                if position.volume_long - position.volume_long_frozen < order.volume_orign {
                    order.status = ORDER_STATUS_FINISHED.to_string();
                    order.last_msg = "平仓手数不足".to_string();
                    return;
                }
                let available_his = position.volume_long_his - position.volume_long_frozen_his;
                let freeze_his = available_his.min(order.volume_orign);
                position.volume_long_frozen_his += freeze_his;
                position.volume_long_frozen_today += order.volume_orign - freeze_his;
            }
            _ => {}
        },
    }

    refresh_position_volume_fields(position);
}

fn release_close_order_resources(position: &mut Position, order: &Order) {
    match order.exchange_id.as_str() {
        "SHFE" | "INE" => match (order.direction.as_str(), order.offset.as_str()) {
            (DIRECTION_BUY, OFFSET_CLOSETODAY) => position.volume_short_frozen_today -= order.volume_orign,
            (DIRECTION_BUY, OFFSET_CLOSE) => position.volume_short_frozen_his -= order.volume_orign,
            (DIRECTION_SELL, OFFSET_CLOSETODAY) => position.volume_long_frozen_today -= order.volume_orign,
            (DIRECTION_SELL, OFFSET_CLOSE) => position.volume_long_frozen_his -= order.volume_orign,
            _ => {}
        },
        _ => match order.direction.as_str() {
            DIRECTION_BUY => {
                if position.volume_short_frozen_today >= order.volume_orign {
                    position.volume_short_frozen_today -= order.volume_orign;
                } else {
                    let remainder = order.volume_orign - position.volume_short_frozen_today;
                    position.volume_short_frozen_today = 0;
                    position.volume_short_frozen_his -= remainder;
                }
            }
            DIRECTION_SELL => {
                if position.volume_long_frozen_today >= order.volume_orign {
                    position.volume_long_frozen_today -= order.volume_orign;
                } else {
                    let remainder = order.volume_orign - position.volume_long_frozen_today;
                    position.volume_long_frozen_today = 0;
                    position.volume_long_frozen_his -= remainder;
                }
            }
            _ => {}
        },
    }

    refresh_position_volume_fields(position);
}

fn apply_open_fill_raw(position: &mut Position, order: &Order, fill_price: f64, volume_multiple: f64) {
    match order.direction.as_str() {
        DIRECTION_BUY => {
            position.volume_long_today += order.volume_orign;
            position.open_cost_long += fill_price * order.volume_orign as f64 * volume_multiple;
            position.position_cost_long += fill_price * order.volume_orign as f64 * volume_multiple;
        }
        DIRECTION_SELL => {
            position.volume_short_today += order.volume_orign;
            position.open_cost_short += fill_price * order.volume_orign as f64 * volume_multiple;
            position.position_cost_short += fill_price * order.volume_orign as f64 * volume_multiple;
        }
        _ => {}
    }

    refresh_position_volume_fields(position);
}

fn apply_close_fill_raw(position: &mut Position, order: &Order, volume_multiple: f64) {
    let filled = order.volume_orign;

    match order.exchange_id.as_str() {
        "SHFE" | "INE" => match (order.direction.as_str(), order.offset.as_str()) {
            (DIRECTION_SELL, OFFSET_CLOSETODAY) => position.volume_long_today -= filled,
            (DIRECTION_SELL, OFFSET_CLOSE) => position.volume_long_his -= filled,
            (DIRECTION_BUY, OFFSET_CLOSETODAY) => position.volume_short_today -= filled,
            (DIRECTION_BUY, OFFSET_CLOSE) => position.volume_short_his -= filled,
            _ => {}
        },
        _ => match order.direction.as_str() {
            DIRECTION_SELL => {
                let close_his = position.volume_long_his.min(filled);
                position.volume_long_his -= close_his;
                position.volume_long_today -= filled - close_his;
            }
            DIRECTION_BUY => {
                let close_his = position.volume_short_his.min(filled);
                position.volume_short_his -= close_his;
                position.volume_short_today -= filled - close_his;
            }
            _ => {}
        },
    }

    match order.direction.as_str() {
        DIRECTION_SELL => {
            position.open_cost_long -= position.open_price_long * filled as f64 * volume_multiple;
            position.position_cost_long -= position.position_price_long * filled as f64 * volume_multiple;
        }
        DIRECTION_BUY => {
            position.open_cost_short -= position.open_price_short * filled as f64 * volume_multiple;
            position.position_cost_short -= position.position_price_short * filled as f64 * volume_multiple;
        }
        _ => {}
    }

    refresh_position_volume_fields(position);
}

fn recompute_position_metrics(position: &mut Position, metadata: &InstrumentMetadata, last_price: f64) {
    let volume_multiple = metadata.volume_multiple as f64;
    let margin_per_lot = metadata.margin.max(0.0);

    position.last_price = last_price;
    refresh_position_volume_fields(position);

    if position.volume_long > 0 {
        position.open_price_long = position.open_cost_long / position.volume_long as f64 / volume_multiple;
        position.position_price_long = position.position_cost_long / position.volume_long as f64 / volume_multiple;
        position.float_profit_long =
            (last_price - position.open_price_long) * position.volume_long as f64 * volume_multiple;
        position.position_profit_long =
            (last_price - position.position_price_long) * position.volume_long as f64 * volume_multiple;
        position.margin_long = position.volume_long as f64 * margin_per_lot;
    } else {
        position.open_price_long = 0.0;
        position.position_price_long = 0.0;
        position.open_cost_long = 0.0;
        position.position_cost_long = 0.0;
        position.float_profit_long = 0.0;
        position.position_profit_long = 0.0;
        position.margin_long = 0.0;
    }

    if position.volume_short > 0 {
        position.open_price_short = position.open_cost_short / position.volume_short as f64 / volume_multiple;
        position.position_price_short = position.position_cost_short / position.volume_short as f64 / volume_multiple;
        position.float_profit_short =
            (position.open_price_short - last_price) * position.volume_short as f64 * volume_multiple;
        position.position_profit_short =
            (position.position_price_short - last_price) * position.volume_short as f64 * volume_multiple;
        position.margin_short = position.volume_short as f64 * margin_per_lot;
    } else {
        position.open_price_short = 0.0;
        position.position_price_short = 0.0;
        position.open_cost_short = 0.0;
        position.position_cost_short = 0.0;
        position.float_profit_short = 0.0;
        position.position_profit_short = 0.0;
        position.margin_short = 0.0;
    }

    position.float_profit = position.float_profit_long + position.float_profit_short;
    position.position_profit = position.position_profit_long + position.position_profit_short;
    position.margin = position.margin_long + position.margin_short;
    position.market_value_long = 0.0;
    position.market_value_short = 0.0;
    position.market_value = 0.0;
}

fn close_profit(position: &Position, order: &Order, fill_price: f64, volume_multiple: f64) -> f64 {
    let filled = order.volume_orign as f64;
    match order.direction.as_str() {
        DIRECTION_SELL => (fill_price - position.position_price_long) * filled * volume_multiple,
        DIRECTION_BUY => (position.position_price_short - fill_price) * filled * volume_multiple,
        _ => 0.0,
    }
}

fn refresh_position_volume_fields(position: &mut Position) {
    position.pos_long_today = position.volume_long_today;
    position.pos_long_his = position.volume_long_his;
    position.pos_short_today = position.volume_short_today;
    position.pos_short_his = position.volume_short_his;
    position.volume_long = position.volume_long_today + position.volume_long_his;
    position.volume_short = position.volume_short_today + position.volume_short_his;
    position.volume_long_frozen = position.volume_long_frozen_today + position.volume_long_frozen_his;
    position.volume_short_frozen = position.volume_short_frozen_today + position.volume_short_frozen_his;
    position.volume_long_yd = position.volume_long_his;
    position.volume_short_yd = position.volume_short_his;
}

fn limit_fill_price(order: &Order, metadata: &InstrumentMetadata, quote: &ReplayQuote) -> Option<f64> {
    let (ask_price, bid_price) = quote_price_range(metadata, quote);
    match order.direction.as_str() {
        DIRECTION_BUY => ask_price
            .filter(|ask| *ask <= order.limit_price)
            .map(|_| order.limit_price),
        DIRECTION_SELL => bid_price
            .filter(|bid| *bid >= order.limit_price)
            .map(|_| order.limit_price),
        _ => None,
    }
}

fn market_fill_price(order: &Order, metadata: &InstrumentMetadata, quote: &ReplayQuote) -> Option<f64> {
    let (ask_price, bid_price) = quote_price_range(metadata, quote);
    match order.direction.as_str() {
        DIRECTION_BUY => ask_price,
        DIRECTION_SELL => bid_price,
        _ => None,
    }
}

fn quote_price_range(metadata: &InstrumentMetadata, quote: &ReplayQuote) -> (Option<f64>, Option<f64>) {
    let mut ask_price = usable_quote_value(quote.ask_price1);
    let mut bid_price = usable_quote_value(quote.bid_price1);

    if metadata.class.ends_with("INDEX") {
        let price_tick = if metadata.price_tick.is_finite() && metadata.price_tick > 0.0 {
            metadata.price_tick
        } else {
            0.0
        };
        if ask_price.is_none() && quote.last_price.is_finite() {
            ask_price = Some(quote.last_price + price_tick);
        }
        if bid_price.is_none() && quote.last_price.is_finite() {
            bid_price = Some(quote.last_price - price_tick);
        }
    }

    (ask_price, bid_price)
}

fn usable_quote_value(price: f64) -> Option<f64> {
    is_usable_quote(price).then_some(price)
}

fn is_usable_quote(price: f64) -> bool {
    price.is_finite()
}

fn matches_symbol(order: &Order, symbol: &str) -> bool {
    order_symbol(order) == symbol
}

fn order_symbol(order: &Order) -> String {
    format!("{}.{}", order.exchange_id, order.instrument_id)
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

fn current_quote_last_price(quotes: &HashMap<String, ReplayQuote>, symbol: &str) -> f64 {
    quotes.get(symbol).map(|quote| quote.last_price).unwrap_or(0.0)
}

const DAY_NANOS: i64 = 86_400_000_000_000;
const EIGHTEEN_HOURS_NANOS: i64 = 64_800_000_000_000;
const BEGIN_MARK_NANOS: i64 = 631_123_200_000_000_000;

fn is_in_trading_time(trading_time: &Value, current_timestamp: i64) -> Result<bool> {
    for (start, end) in trading_ranges_for_timestamp(current_timestamp, trading_time)? {
        if start <= current_timestamp && current_timestamp < end {
            return Ok(true);
        }
    }
    Ok(false)
}

fn trading_ranges_for_timestamp(current_timestamp: i64, trading_time: &Value) -> Result<Vec<(i64, i64)>> {
    let current_trading_day = trading_day_from_timestamp(current_timestamp);
    let last_trading_day = trading_day_from_timestamp(trading_day_start_time(current_trading_day) - 1);

    let mut ranges = parse_period_ranges(last_trading_day, trading_time.get("night"))?;
    ranges.extend(parse_period_ranges(current_trading_day, trading_time.get("day"))?);
    Ok(ranges)
}

fn trading_day_start_time(trading_day: i64) -> i64 {
    let mut start_time = trading_day - 21_600_000_000_000;
    let week_day = (start_time - BEGIN_MARK_NANOS) / DAY_NANOS % 7;
    if week_day >= 5 {
        start_time -= DAY_NANOS * (week_day - 4);
    }
    start_time
}

fn trading_day_from_timestamp(timestamp: i64) -> i64 {
    let mut days = (timestamp - BEGIN_MARK_NANOS) / DAY_NANOS;
    if (timestamp - BEGIN_MARK_NANOS) % DAY_NANOS >= EIGHTEEN_HOURS_NANOS {
        days += 1;
    }
    let week_day = days % 7;
    if week_day >= 5 {
        days += 7 - week_day;
    }
    BEGIN_MARK_NANOS + days * DAY_NANOS
}

fn parse_period_ranges(base_day_timestamp: i64, raw_periods: Option<&Value>) -> Result<Vec<(i64, i64)>> {
    let Some(periods) = raw_periods.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut ranges = Vec::with_capacity(periods.len());
    for period in periods {
        let pair = period
            .as_array()
            .filter(|pair| pair.len() == 2)
            .ok_or_else(|| TqError::TradeError("invalid replay trading_time period".to_string()))?;
        let start = pair[0]
            .as_str()
            .ok_or_else(|| TqError::TradeError("invalid replay trading_time period start".to_string()))?;
        let end = pair[1]
            .as_str()
            .ok_or_else(|| TqError::TradeError("invalid replay trading_time period end".to_string()))?;
        ranges.push((
            base_day_timestamp + parse_hms_nanos(start)?,
            base_day_timestamp + parse_hms_nanos(end)?,
        ));
    }
    Ok(ranges)
}

fn parse_hms_nanos(raw: &str) -> Result<i64> {
    let mut parts = raw.split(':');
    let hour = parts
        .next()
        .ok_or_else(|| TqError::TradeError("invalid replay trading_time hh:mm:ss".to_string()))?
        .parse::<i64>()
        .map_err(|_| TqError::TradeError("invalid replay trading_time hour".to_string()))?;
    let minute = parts
        .next()
        .ok_or_else(|| TqError::TradeError("invalid replay trading_time hh:mm:ss".to_string()))?
        .parse::<i64>()
        .map_err(|_| TqError::TradeError("invalid replay trading_time minute".to_string()))?;
    let second = parts
        .next()
        .ok_or_else(|| TqError::TradeError("invalid replay trading_time hh:mm:ss".to_string()))?
        .parse::<i64>()
        .map_err(|_| TqError::TradeError("invalid replay trading_time second".to_string()))?;
    if parts.next().is_some() {
        return Err(TqError::TradeError("invalid replay trading_time hh:mm:ss".to_string()));
    }
    Ok((hour * 3600 + minute * 60 + second) * 1_000_000_000)
}

fn default_position(account_key: &str, symbol: &str, last_price: f64) -> Position {
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
        last_price,
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
