use std::collections::HashMap;

use crate::types::{Kline, Tick};

use super::types::{InstrumentMetadata, ReplayQuote};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSelection {
    Tick,
    Kline { duration_nanos: i64, implicit: bool },
}

#[derive(Debug, Clone)]
pub struct QuoteUpdate {
    #[allow(dead_code)]
    pub visible: ReplayQuote,
    pub path: Vec<ReplayQuote>,
    pub source_selected: bool,
}

#[derive(Debug, Default)]
pub struct QuoteSynthesizer {
    symbols: HashMap<String, SymbolQuoteState>,
}

#[derive(Debug, Clone)]
struct SymbolQuoteState {
    metadata: InstrumentMetadata,
    selected: QuoteSelection,
    visible: ReplayQuote,
}

impl QuoteSynthesizer {
    pub fn register_symbol(&mut self, symbol: &str, metadata: InstrumentMetadata, selection: QuoteSelection) {
        match self.symbols.get_mut(symbol) {
            Some(state) => {
                state.metadata = merge_metadata(&state.metadata, &metadata);
                if selection_priority(selection) > selection_priority(state.selected) {
                    state.selected = selection;
                }
            }
            None => {
                self.symbols.insert(
                    symbol.to_string(),
                    SymbolQuoteState {
                        metadata: merge_metadata(
                            &InstrumentMetadata {
                                symbol: symbol.to_string(),
                                ..InstrumentMetadata::default()
                            },
                            &metadata,
                        ),
                        selected: selection,
                        visible: ReplayQuote {
                            symbol: symbol.to_string(),
                            ..ReplayQuote::default()
                        },
                    },
                );
            }
        }
    }

    #[cfg(test)]
    pub fn selection_for(&self, symbol: &str) -> Option<QuoteSelection> {
        self.symbols.get(symbol).map(|state| state.selected)
    }

    pub fn visible_quote(&self, symbol: &str) -> Option<&ReplayQuote> {
        self.symbols.get(symbol).map(|state| &state.visible)
    }

    pub fn apply_tick(&mut self, symbol: &str, tick: &Tick) -> QuoteUpdate {
        self.observe_source(
            symbol,
            InstrumentMetadata {
                symbol: symbol.to_string(),
                ..InstrumentMetadata::default()
            },
            QuoteSelection::Tick,
        );
        let state = self.symbols.get_mut(symbol).expect("symbol state must exist");
        let visible = quote_from_tick(symbol, tick);
        let source_selected = matches!(state.selected, QuoteSelection::Tick);

        if source_selected {
            state.visible = visible.clone();
        }

        QuoteUpdate {
            visible: if source_selected {
                visible.clone()
            } else {
                state.visible.clone()
            },
            path: vec![visible],
            source_selected,
        }
    }

    pub fn apply_bar_open(
        &mut self,
        symbol: &str,
        duration_nanos: i64,
        kline: &Kline,
        event_timestamp_nanos: i64,
    ) -> QuoteUpdate {
        let selection = QuoteSelection::Kline {
            duration_nanos,
            implicit: false,
        };
        self.observe_source(
            symbol,
            InstrumentMetadata {
                symbol: symbol.to_string(),
                ..InstrumentMetadata::default()
            },
            selection,
        );
        let state = self.symbols.get_mut(symbol).expect("symbol state must exist");
        let selected = state.selected == selection;
        let opening = quote_from_kline_step(
            symbol,
            &state.metadata,
            kline,
            event_timestamp_nanos,
            kline.open,
            kline.open_oi,
        );

        if selected {
            state.visible = opening.clone();
        }

        QuoteUpdate {
            visible: if selected {
                opening.clone()
            } else {
                state.visible.clone()
            },
            path: if selected { vec![opening] } else { Vec::new() },
            source_selected: selected,
        }
    }

    pub fn apply_bar_close(
        &mut self,
        symbol: &str,
        duration_nanos: i64,
        kline: &Kline,
        event_timestamp_nanos: i64,
    ) -> QuoteUpdate {
        let selection = QuoteSelection::Kline {
            duration_nanos,
            implicit: false,
        };
        self.observe_source(
            symbol,
            InstrumentMetadata {
                symbol: symbol.to_string(),
                ..InstrumentMetadata::default()
            },
            selection,
        );
        let state = self.symbols.get_mut(symbol).expect("symbol state must exist");
        let selected = state.selected == selection;
        let path = vec![
            quote_from_kline_step(
                symbol,
                &state.metadata,
                kline,
                event_timestamp_nanos,
                kline.open,
                kline.open_oi,
            ),
            quote_from_kline_step(
                symbol,
                &state.metadata,
                kline,
                event_timestamp_nanos,
                kline.high,
                kline.close_oi,
            ),
            quote_from_kline_step(
                symbol,
                &state.metadata,
                kline,
                event_timestamp_nanos,
                kline.low,
                kline.close_oi,
            ),
            quote_from_kline_step(
                symbol,
                &state.metadata,
                kline,
                event_timestamp_nanos,
                kline.close,
                kline.close_oi,
            ),
        ];
        let visible = path.last().cloned().expect("path must not be empty");

        if selected {
            state.visible = visible.clone();
        }

        QuoteUpdate {
            visible: if selected { visible } else { state.visible.clone() },
            path: if selected { path } else { Vec::new() },
            source_selected: selected,
        }
    }

    fn observe_source(&mut self, symbol: &str, metadata: InstrumentMetadata, selection: QuoteSelection) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.metadata = merge_metadata(&state.metadata, &metadata);
            if selection_priority(selection) > selection_priority(state.selected) {
                state.selected = selection;
            }
            return;
        }

        self.register_symbol(symbol, metadata, selection);
    }
}

fn selection_priority(selection: QuoteSelection) -> (u8, i64) {
    match selection {
        QuoteSelection::Tick => (2, 0),
        QuoteSelection::Kline {
            duration_nanos,
            implicit: false,
        } => (1, -duration_nanos),
        QuoteSelection::Kline { .. } => (0, 0),
    }
}

fn merge_metadata(current: &InstrumentMetadata, incoming: &InstrumentMetadata) -> InstrumentMetadata {
    InstrumentMetadata {
        symbol: pick_string(&incoming.symbol, &current.symbol),
        exchange_id: pick_string(&incoming.exchange_id, &current.exchange_id),
        instrument_id: pick_string(&incoming.instrument_id, &current.instrument_id),
        class: pick_string(&incoming.class, &current.class),
        underlying_symbol: pick_string(&incoming.underlying_symbol, &current.underlying_symbol),
        option_class: pick_string(&incoming.option_class, &current.option_class),
        strike_price: pick_f64(incoming.strike_price, current.strike_price),
        trading_time: pick_value(&incoming.trading_time, &current.trading_time),
        price_tick: pick_f64(incoming.price_tick, current.price_tick),
        volume_multiple: pick_i32(incoming.volume_multiple, current.volume_multiple),
        margin: pick_f64(incoming.margin, current.margin),
        commission: pick_f64(incoming.commission, current.commission),
        open_min_market_order_volume: pick_i32(
            incoming.open_min_market_order_volume,
            current.open_min_market_order_volume,
        ),
        open_min_limit_order_volume: pick_i32(
            incoming.open_min_limit_order_volume,
            current.open_min_limit_order_volume,
        ),
    }
}

fn quote_from_tick(symbol: &str, tick: &Tick) -> ReplayQuote {
    ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos: tick.datetime,
        last_price: tick.last_price,
        ask_price1: tick.ask_price1,
        ask_volume1: tick.ask_volume1,
        bid_price1: tick.bid_price1,
        bid_volume1: tick.bid_volume1,
        highest: tick.highest,
        lowest: tick.lowest,
        average: tick.average,
        volume: tick.volume,
        amount: tick.amount,
        open_interest: tick.open_interest,
        open_limit: 0,
    }
}

fn quote_from_kline_step(
    symbol: &str,
    metadata: &InstrumentMetadata,
    kline: &Kline,
    event_timestamp_nanos: i64,
    last_price: f64,
    open_interest: i64,
) -> ReplayQuote {
    let price_tick = normalized_price_tick(metadata.price_tick);
    ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos: event_timestamp_nanos,
        last_price,
        ask_price1: last_price + price_tick,
        ask_volume1: 1,
        bid_price1: last_price - price_tick,
        bid_volume1: 1,
        highest: kline.high,
        lowest: kline.low,
        average: last_price,
        volume: kline.volume,
        amount: 0.0,
        open_interest,
        open_limit: 0,
    }
}

fn normalized_price_tick(price_tick: f64) -> f64 {
    if price_tick.is_finite() && price_tick > 0.0 {
        price_tick
    } else {
        0.0
    }
}

fn pick_string(incoming: &str, current: &str) -> String {
    if incoming.is_empty() {
        current.to_string()
    } else {
        incoming.to_string()
    }
}

fn pick_f64(incoming: f64, current: f64) -> f64 {
    if incoming == 0.0 { current } else { incoming }
}

fn pick_i32(incoming: i32, current: i32) -> i32 {
    if incoming == 0 { current } else { incoming }
}

fn pick_value(incoming: &serde_json::Value, current: &serde_json::Value) -> serde_json::Value {
    if incoming.is_null() {
        current.clone()
    } else {
        incoming.clone()
    }
}
