use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::errors::Result;
use crate::replay::{BarState, InstrumentMetadata, ReplayHandleId, ReplayQuote, ReplayStep};

use super::feed::{FeedCursor, FeedEvent};
use super::quote::{QuoteSelection, QuoteSynthesizer};
use super::series::{AlignedKlineHandle, AlignedKlineRow, ReplayKlineHandle, ReplayTickHandle, SeriesStore};

const DEFAULT_SERIES_WIDTH: usize = 1_024;

#[derive(Debug)]
pub struct ReplayKernel {
    feeds: Vec<(String, FeedCursor)>,
    series_store: SeriesStore,
    quote_synthesizer: QuoteSynthesizer,
    quote_paths: HashMap<String, Vec<ReplayQuote>>,
    klines: Vec<ReplayKlineHandle>,
    tick_handles: Vec<ReplayTickHandle>,
    aligned_klines: Vec<AlignedKlineHandle>,
    next_handle_id: usize,
    tick_window_width: usize,
}

impl ReplayKernel {
    pub fn for_test(feeds: Vec<(String, FeedCursor)>) -> Self {
        Self {
            feeds,
            series_store: SeriesStore::default(),
            quote_synthesizer: QuoteSynthesizer::default(),
            quote_paths: HashMap::new(),
            klines: Vec::new(),
            tick_handles: Vec::new(),
            aligned_klines: Vec::new(),
            next_handle_id: 0,
            tick_window_width: DEFAULT_SERIES_WIDTH,
        }
    }

    #[cfg(test)]
    pub fn series_store(&self) -> &SeriesStore {
        &self.series_store
    }

    pub(crate) fn register_aligned_kline(
        &mut self,
        symbols: &[&str],
        duration_nanos: i64,
        width: usize,
    ) -> AlignedKlineHandle {
        let id = ReplayHandleId(format!("aligned_kline_{}", self.next_handle_id));
        self.next_handle_id += 1;

        let handle = AlignedKlineHandle::new(
            id,
            symbols.iter().map(|symbol| (*symbol).to_string()).collect(),
            duration_nanos,
            width,
        );
        self.aligned_klines.push(handle.clone());
        handle
    }

    pub(crate) fn register_tick(
        &mut self,
        symbol: &str,
        width: usize,
        metadata: InstrumentMetadata,
    ) -> ReplayTickHandle {
        let id = ReplayHandleId(format!("tick_{}", self.next_handle_id));
        self.next_handle_id += 1;

        self.quote_synthesizer
            .register_symbol(symbol, metadata, QuoteSelection::Tick);
        let handle = ReplayTickHandle::new(id, symbol.to_string(), width);
        self.tick_handles.push(handle.clone());
        handle
    }

    pub fn register_quote(&mut self, symbol: &str, metadata: InstrumentMetadata) {
        self.quote_synthesizer.register_symbol(
            symbol,
            metadata,
            QuoteSelection::Kline {
                duration_nanos: 60_000_000_000,
                implicit: true,
            },
        );
    }

    pub fn register_kline(
        &mut self,
        symbol: &str,
        duration_nanos: i64,
        width: usize,
        metadata: InstrumentMetadata,
    ) -> ReplayKlineHandle {
        let id = ReplayHandleId(format!("kline_{}", self.next_handle_id));
        self.next_handle_id += 1;

        self.quote_synthesizer.register_symbol(
            symbol,
            metadata,
            QuoteSelection::Kline {
                duration_nanos,
                implicit: false,
            },
        );
        let handle = ReplayKlineHandle::new(id, symbol.to_string(), duration_nanos, width);
        self.klines.push(handle.clone());
        handle
    }

    pub fn push_feed(&mut self, symbol: String, cursor: FeedCursor) {
        self.feeds.push((symbol, cursor));
    }

    #[cfg(test)]
    pub fn set_tick_window_width(&mut self, width: usize) {
        self.tick_window_width = width;
    }

    pub fn visible_quote(&self, symbol: &str) -> Option<&ReplayQuote> {
        self.quote_synthesizer.visible_quote(symbol)
    }

    pub fn quote_path(&self, symbol: &str) -> Option<&[ReplayQuote]> {
        self.quote_paths.get(symbol).map(Vec::as_slice)
    }

    pub fn peek_next_timestamp(&self) -> Option<i64> {
        self.feeds
            .iter()
            .filter_map(|(_, cursor)| cursor.peek_timestamp())
            .min()
    }

    pub async fn step(&mut self) -> Result<Option<ReplayStep>> {
        let Some(next_timestamp) = self.peek_next_timestamp() else {
            return Ok(None);
        };

        let mut updated_symbols = Vec::new();
        let mut updated_handles = Vec::new();
        let mut updated_quotes = Vec::new();
        let mut events = Vec::new();

        for (_, cursor) in &mut self.feeds {
            loop {
                if cursor.peek_timestamp() != Some(next_timestamp) {
                    break;
                }

                let Some(event) = cursor.next_event() else {
                    break;
                };
                events.push(event);
            }
        }

        for event in events {
            self.apply_event(event, &mut updated_symbols, &mut updated_handles, &mut updated_quotes)
                .await;
        }

        let aligned_handles = self.materialize_aligned_rows(next_timestamp, &updated_symbols).await;
        for handle_id in aligned_handles {
            if !updated_handles.contains(&handle_id) {
                updated_handles.push(handle_id);
            }
        }

        Ok(Some(ReplayStep {
            current_dt: DateTime::<Utc>::from_timestamp_nanos(next_timestamp),
            updated_handles,
            updated_quotes,
            settled_trading_day: None,
        }))
    }

    async fn apply_event(
        &mut self,
        event: FeedEvent,
        updated_symbols: &mut Vec<String>,
        updated_handles: &mut Vec<ReplayHandleId>,
        updated_quotes: &mut Vec<String>,
    ) {
        match event {
            FeedEvent::Tick { symbol, tick } => {
                let update = self.quote_synthesizer.apply_tick(&symbol, &tick);
                self.quote_paths.insert(symbol.clone(), update.path.clone());
                self.series_store
                    .push_tick(&symbol, tick.clone(), self.tick_window_width);
                self.update_tick_handles(&symbol, tick, updated_handles).await;
                if update.source_selected && !updated_quotes.contains(&symbol) {
                    updated_quotes.push(symbol);
                }
            }
            FeedEvent::BarOpen {
                symbol,
                duration_nanos,
                event_timestamp_nanos,
                kline,
            } => {
                self.series_store.push_kline(
                    &symbol,
                    duration_nanos,
                    kline.clone(),
                    BarState::Opening,
                    DEFAULT_SERIES_WIDTH,
                );
                let update =
                    self.quote_synthesizer
                        .apply_bar_open(&symbol, duration_nanos, &kline, event_timestamp_nanos);
                self.quote_paths.insert(symbol.clone(), update.path.clone());
                if update.source_selected && !updated_quotes.contains(&symbol) {
                    updated_quotes.push(symbol.clone());
                }
                self.update_kline_handles(&symbol, duration_nanos, kline, BarState::Opening, updated_handles)
                    .await;
                updated_symbols.push(symbol);
            }
            FeedEvent::BarClose {
                symbol,
                duration_nanos,
                event_timestamp_nanos,
                kline,
            } => {
                self.series_store
                    .close_kline(&symbol, duration_nanos, kline.clone(), DEFAULT_SERIES_WIDTH);
                let update =
                    self.quote_synthesizer
                        .apply_bar_close(&symbol, duration_nanos, &kline, event_timestamp_nanos);
                self.quote_paths.insert(symbol.clone(), update.path.clone());
                if update.source_selected && !updated_quotes.contains(&symbol) {
                    updated_quotes.push(symbol.clone());
                }
                self.update_kline_handles(&symbol, duration_nanos, kline, BarState::Closed, updated_handles)
                    .await;
                updated_symbols.push(symbol);
            }
        }
    }

    async fn update_kline_handles(
        &self,
        symbol: &str,
        duration_nanos: i64,
        kline: crate::types::Kline,
        state: BarState,
        updated_handles: &mut Vec<ReplayHandleId>,
    ) {
        for handle in &self.klines {
            if !handle.matches(symbol, duration_nanos) {
                continue;
            }

            handle.apply_kline(kline.clone(), state).await;
            if !updated_handles.contains(handle.id()) {
                updated_handles.push(handle.id().clone());
            }
        }
    }

    async fn update_tick_handles(
        &self,
        symbol: &str,
        tick: crate::types::Tick,
        updated_handles: &mut Vec<ReplayHandleId>,
    ) {
        for handle in &self.tick_handles {
            if !handle.matches(symbol) {
                continue;
            }

            handle.apply_tick(tick.clone()).await;
            if !updated_handles.contains(handle.id()) {
                updated_handles.push(handle.id().clone());
            }
        }
    }

    async fn materialize_aligned_rows(&self, timestamp_nanos: i64, updated_symbols: &[String]) -> Vec<ReplayHandleId> {
        let mut updated_handles = Vec::new();

        for handle in &self.aligned_klines {
            if !handle
                .symbols()
                .iter()
                .any(|symbol| updated_symbols.iter().any(|updated| updated == symbol))
            {
                continue;
            }

            let bars = handle
                .symbols()
                .iter()
                .map(|symbol| {
                    (
                        symbol.clone(),
                        self.series_store.latest_kline(symbol, handle.duration_nanos()).cloned(),
                    )
                })
                .collect::<HashMap<_, _>>();

            handle
                .push_row(AlignedKlineRow {
                    datetime_nanos: timestamp_nanos,
                    bars,
                })
                .await;
            updated_handles.push(handle.id().clone());
        }

        updated_handles
    }
}

impl Default for ReplayKernel {
    fn default() -> Self {
        Self::for_test(Vec::new())
    }
}
