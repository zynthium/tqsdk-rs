use std::sync::Arc;

use crate::runtime::{
    ExecutionAdapter, MarketAdapter, OrderDirection, PriceMode, RuntimeError, RuntimeResult, VolumeSplitPolicy,
};
use crate::types::{
    DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, OFFSET_CLOSE, OFFSET_CLOSETODAY, OFFSET_OPEN,
    ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, PRICE_TYPE_LIMIT, Quote, Trade,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetAction {
    Today,
    Yesterday,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlannedOffset {
    Open,
    Close,
    CloseToday,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOrder {
    pub direction: OrderDirection,
    pub offset: PlannedOffset,
    pub volume: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedBatch {
    pub orders: Vec<PlannedOrder>,
}

impl PlannedOffset {
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Open => OFFSET_OPEN,
            Self::Close => OFFSET_CLOSE,
            Self::CloseToday => OFFSET_CLOSETODAY,
        }
    }
}

impl OrderDirection {
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Buy => DIRECTION_BUY,
            Self::Sell => DIRECTION_SELL,
        }
    }
}

pub struct ChildOrderRunner {
    market: Arc<dyn MarketAdapter>,
    execution: Arc<dyn ExecutionAdapter>,
    account_key: String,
    symbol: String,
    direction: OrderDirection,
    offset: PlannedOffset,
    volume: i64,
    split_policy: Option<VolumeSplitPolicy>,
    price_mode: PriceMode,
}

impl ChildOrderRunner {
    #[expect(
        clippy::too_many_arguments,
        reason = "task runner wiring keeps execution context explicit"
    )]
    pub fn new(
        market: Arc<dyn MarketAdapter>,
        execution: Arc<dyn ExecutionAdapter>,
        account_key: impl Into<String>,
        symbol: impl Into<String>,
        direction: OrderDirection,
        offset: PlannedOffset,
        volume: i64,
        split_policy: Option<VolumeSplitPolicy>,
        price_mode: PriceMode,
    ) -> Self {
        Self {
            market,
            execution,
            account_key: account_key.into(),
            symbol: symbol.into(),
            direction,
            offset,
            volume,
            split_policy,
            price_mode,
        }
    }

    pub async fn run_until_all_traded(&self) -> RuntimeResult<()> {
        let mut remaining = self.volume;

        while remaining > 0 {
            let quote = self.market.latest_quote(&self.symbol).await?;
            let order_price = resolve_order_price(&self.symbol, self.direction, &self.price_mode, &quote)?;
            let order_volume = split_order_volume(remaining, self.split_policy);
            let req = InsertOrderRequest {
                symbol: self.symbol.clone(),
                exchange_id: None,
                instrument_id: None,
                direction: self.direction.as_api_str().to_string(),
                offset: self.offset.as_api_str().to_string(),
                price_type: PRICE_TYPE_LIMIT.to_string(),
                limit_price: order_price,
                volume: order_volume,
            };
            let order_id = self.execution.insert_order(&self.account_key, &req).await?;
            let mut repriced_or_cancelled = false;

            loop {
                let order = self.execution.order(&self.account_key, &order_id).await?;
                let trades = self.execution.trades_by_order(&self.account_key, &order_id).await?;
                let filled = filled_volume(&order);

                if order.status == ORDER_STATUS_FINISHED && traded_volume(&trades) >= filled {
                    if order.volume_left > 0 && !repriced_or_cancelled {
                        return Err(RuntimeError::OrderCompletionInvariant { order_id });
                    }
                    remaining -= filled;
                    break;
                }

                tokio::select! {
                    update = self.execution.wait_order_update(&self.account_key, &order_id) => {
                        update?;
                    }
                    quote_update = self.market.wait_quote_update(&self.symbol), if !repriced_or_cancelled => {
                        quote_update?;
                        let latest_quote = self.market.latest_quote(&self.symbol).await?;
                        let latest_price = resolve_order_price(&self.symbol, self.direction, &self.price_mode, &latest_quote)?;
                        if is_price_worse(self.direction, order_price, latest_price) {
                            let current = self.execution.order(&self.account_key, &order_id).await?;
                            if current.status == ORDER_STATUS_ALIVE {
                                self.execution.cancel_order(&self.account_key, &order_id).await?;
                                repriced_or_cancelled = true;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

pub fn resolve_order_price(
    symbol: &str,
    direction: OrderDirection,
    price_mode: &PriceMode,
    quote: &Quote,
) -> RuntimeResult<f64> {
    let price = match price_mode {
        PriceMode::Active => {
            let mut price_list = [quote.ask_price1, quote.bid_price1];
            if matches!(direction, OrderDirection::Sell) {
                price_list.reverse();
            }
            fallback_price(price_list, quote)
        }
        PriceMode::Passive => {
            let mut price_list = [quote.ask_price1, quote.bid_price1];
            if matches!(direction, OrderDirection::Sell) {
                price_list.reverse();
            }
            price_list.reverse();
            fallback_price(price_list, quote)
        }
        PriceMode::Custom(resolver) => resolver(direction, quote)?,
    };

    if price.is_nan() {
        return Err(RuntimeError::InvalidOrderPrice {
            symbol: symbol.to_string(),
            direction: direction.as_api_str().to_string(),
        });
    }

    Ok(price)
}

fn fallback_price(price_list: [f64; 2], quote: &Quote) -> f64 {
    let mut price = price_list[0];
    if price.is_nan() {
        price = price_list[1];
    }
    if price.is_nan() {
        price = quote.last_price;
    }
    if price.is_nan() {
        price = quote.pre_close;
    }
    price
}

fn split_order_volume(remaining: i64, split_policy: Option<VolumeSplitPolicy>) -> i64 {
    match split_policy {
        Some(policy) if remaining >= policy.max_volume => policy.max_volume,
        _ => remaining,
    }
}

fn filled_volume(order: &Order) -> i64 {
    order.volume_orign - order.volume_left
}

fn traded_volume(trades: &[Trade]) -> i64 {
    trades.iter().map(|trade| trade.volume).sum()
}

fn is_price_worse(direction: OrderDirection, old_price: f64, new_price: f64) -> bool {
    match direction {
        OrderDirection::Buy => new_price > old_price,
        OrderDirection::Sell => new_price < old_price,
    }
}
