use std::collections::HashSet;

use crate::runtime::{AccountHandle, OrderDirection, RuntimeError, RuntimeResult, TargetPosConfig};
use crate::types::{Position, Quote};

use super::{OffsetAction, PlannedBatch, PlannedOffset, PlannedOrder};

#[derive(Clone)]
pub struct TargetPosBuilder {
    account: AccountHandle,
    symbol: String,
    config: TargetPosConfig,
}

#[derive(Clone)]
pub struct TargetPosHandle {
    account: AccountHandle,
    symbol: String,
    config: TargetPosConfig,
    task_id: crate::runtime::TaskId,
    offset_priority: Vec<Vec<OffsetAction>>,
}

impl TargetPosBuilder {
    pub(crate) fn new(account: AccountHandle, symbol: impl Into<String>) -> Self {
        Self {
            account,
            symbol: symbol.into(),
            config: TargetPosConfig::default(),
        }
    }

    pub fn config(mut self, config: TargetPosConfig) -> Self {
        self.config = config;
        self
    }

    pub fn build(self) -> RuntimeResult<TargetPosHandle> {
        let offset_priority = parse_offset_priority(self.config.offset_priority.as_str())?;
        let registry = self.account.runtime().registry();
        let registered = registry.register_target_task(
            self.account.runtime_id(),
            self.account.account_key(),
            &self.symbol,
            &self.config,
        )?;

        Ok(TargetPosHandle {
            account: self.account,
            symbol: self.symbol,
            config: self.config,
            task_id: registered.task_id,
            offset_priority,
        })
    }
}

impl TargetPosHandle {
    pub fn account(&self) -> &AccountHandle {
        &self.account
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn config(&self) -> &TargetPosConfig {
        &self.config
    }

    pub fn task_id(&self) -> crate::runtime::TaskId {
        self.task_id
    }

    pub fn offset_priority(&self) -> &[Vec<OffsetAction>] {
        &self.offset_priority
    }

    pub fn compute_plan(
        &self,
        quote: &Quote,
        position: &Position,
        target_volume: i64,
    ) -> RuntimeResult<Vec<PlannedBatch>> {
        compute_plan(quote, position, target_volume, &self.offset_priority)
    }
}

pub fn parse_offset_priority(raw: &str) -> RuntimeResult<Vec<Vec<OffsetAction>>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut seen = HashSet::new();

    for ch in raw.chars() {
        if ch == ',' {
            if current.is_empty() {
                return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
            }
            groups.push(std::mem::take(&mut current));
            continue;
        }

        let action = match ch {
            '今' => OffsetAction::Today,
            '昨' => OffsetAction::Yesterday,
            '开' => OffsetAction::Open,
            _ => {
                return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
            }
        };

        if !seen.insert(action) {
            return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
        }
        current.push(action);
    }

    if current.is_empty() {
        return Err(RuntimeError::InvalidOffsetPriority { raw: raw.to_string() });
    }
    groups.push(current);

    Ok(groups)
}

pub fn validate_quote_constraints(quote: &Quote) -> RuntimeResult<()> {
    if quote.open_min_market_order_volume > 1 || quote.open_min_limit_order_volume > 1 {
        return Err(RuntimeError::UnsupportedOpenOrderVolume {
            symbol: quote.instrument_id.clone(),
            open_min_market_order_volume: quote.open_min_market_order_volume,
            open_min_limit_order_volume: quote.open_min_limit_order_volume,
        });
    }

    Ok(())
}

pub fn compute_plan(
    quote: &Quote,
    position: &Position,
    target_volume: i64,
    offset_priority: &[Vec<OffsetAction>],
) -> RuntimeResult<Vec<PlannedBatch>> {
    validate_quote_constraints(quote)?;

    let mut remaining = target_volume - net_position(position);
    if remaining == 0 {
        return Ok(Vec::new());
    }

    let mut batches = Vec::new();
    for group in offset_priority {
        if remaining == 0 {
            break;
        }

        let mut pending_frozen = 0_i64;
        let mut orders = Vec::new();

        for action in group {
            if remaining == 0 {
                break;
            }

            let Some((order, frozen_delta)) =
                planned_order_for_action(*action, remaining, pending_frozen, quote, position)
            else {
                continue;
            };

            pending_frozen += frozen_delta;
            remaining -= match order.direction {
                OrderDirection::Buy => order.volume,
                OrderDirection::Sell => -order.volume,
            };
            orders.push(order);
        }

        if !orders.is_empty() {
            batches.push(PlannedBatch { orders });
        }
    }

    Ok(batches)
}

fn planned_order_for_action(
    action: OffsetAction,
    delta_volume: i64,
    pending_frozen: i64,
    quote: &Quote,
    position: &Position,
) -> Option<(PlannedOrder, i64)> {
    let direction = if delta_volume > 0 {
        OrderDirection::Buy
    } else {
        OrderDirection::Sell
    };
    let requested = delta_volume.abs();
    let exchange = exchange_id(quote);
    let is_shfe_like = matches!(exchange.as_str(), "SHFE" | "INE");

    let (offset, available) = match action {
        OffsetAction::Open => (PlannedOffset::Open, requested),
        OffsetAction::Today => {
            if is_shfe_like {
                let available = match direction {
                    OrderDirection::Buy => short_today(position) - short_frozen_today(position),
                    OrderDirection::Sell => long_today(position) - long_frozen_today(position),
                };
                (PlannedOffset::CloseToday, available)
            } else {
                let frozen = pending_frozen + total_close_frozen(position, direction);
                let today_available = match direction {
                    OrderDirection::Buy => short_today(position),
                    OrderDirection::Sell => long_today(position),
                };
                (PlannedOffset::Close, today_available - frozen)
            }
        }
        OffsetAction::Yesterday => {
            if is_shfe_like {
                let available = match direction {
                    OrderDirection::Buy => short_his(position) - short_frozen_his(position),
                    OrderDirection::Sell => long_his(position) - long_frozen_his(position),
                };
                (PlannedOffset::Close, available)
            } else {
                let frozen = pending_frozen + total_close_frozen(position, direction);
                let today_left = match direction {
                    OrderDirection::Buy => short_today(position) - frozen,
                    OrderDirection::Sell => long_today(position) - frozen,
                };
                if today_left > 0 {
                    (PlannedOffset::Close, 0)
                } else {
                    let total_available = match direction {
                        OrderDirection::Buy => short_total(position),
                        OrderDirection::Sell => long_total(position),
                    };
                    (PlannedOffset::Close, total_available - frozen)
                }
            }
        }
    };

    let volume = requested.min(available.max(0));
    if volume == 0 {
        return None;
    }

    let frozen_delta = if matches!(offset, PlannedOffset::Open) {
        0
    } else {
        volume
    };

    Some((
        PlannedOrder {
            direction,
            offset,
            volume,
        },
        frozen_delta,
    ))
}

fn exchange_id(quote: &Quote) -> String {
    if !quote.exchange_id.is_empty() {
        return quote.exchange_id.clone();
    }

    quote
        .instrument_id
        .split_once('.')
        .map(|(exchange, _)| exchange.to_string())
        .unwrap_or_default()
}

fn net_position(position: &Position) -> i64 {
    long_total(position) - short_total(position)
}

fn long_today(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_today
    } else {
        position.volume_long_today
    }
}

fn long_his(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_his
    } else {
        position.volume_long_his
    }
}

fn long_total(position: &Position) -> i64 {
    if position.pos_long_today != 0 || position.pos_long_his != 0 {
        position.pos_long_today + position.pos_long_his
    } else if position.volume_long != 0 {
        position.volume_long
    } else {
        position.volume_long_today + position.volume_long_his
    }
}

fn short_today(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_today
    } else {
        position.volume_short_today
    }
}

fn short_his(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_his
    } else {
        position.volume_short_his
    }
}

fn short_total(position: &Position) -> i64 {
    if position.pos_short_today != 0 || position.pos_short_his != 0 {
        position.pos_short_today + position.pos_short_his
    } else if position.volume_short != 0 {
        position.volume_short
    } else {
        position.volume_short_today + position.volume_short_his
    }
}

fn long_frozen_today(position: &Position) -> i64 {
    position.volume_long_frozen_today
}

fn long_frozen_his(position: &Position) -> i64 {
    position.volume_long_frozen_his
}

fn short_frozen_today(position: &Position) -> i64 {
    position.volume_short_frozen_today
}

fn short_frozen_his(position: &Position) -> i64 {
    position.volume_short_frozen_his
}

fn total_close_frozen(position: &Position, direction: OrderDirection) -> i64 {
    match direction {
        OrderDirection::Buy => {
            if position.volume_short_frozen != 0 {
                position.volume_short_frozen
            } else {
                position.volume_short_frozen_today + position.volume_short_frozen_his
            }
        }
        OrderDirection::Sell => {
            if position.volume_long_frozen != 0 {
                position.volume_long_frozen
            } else {
                position.volume_long_frozen_today + position.volume_long_frozen_his
            }
        }
    }
}
