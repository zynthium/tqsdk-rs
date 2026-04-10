use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::errors::Result;
use crate::replay::{
    AlignedKlineHandle, AlignedKlineRow, BarState, FeedCursor, FeedEvent, ReplayHandleId, ReplayKlineHandle,
    ReplayStep, QuoteSynthesizer, SeriesStore,
};

const DEFAULT_SERIES_WIDTH: usize = 1_024;

#[derive(Debug)]
pub struct ReplayKernel {
    feeds: Vec<(String, FeedCursor)>,
    series_store: SeriesStore,
    quote_synthesizer: QuoteSynthesizer,
    klines: Vec<ReplayKlineHandle>,
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
            klines: Vec::new(),
            aligned_klines: Vec::new(),
            next_handle_id: 0,
            tick_window_width: DEFAULT_SERIES_WIDTH,
        }
    }

    pub fn series_store(&self) -> &SeriesStore {
        &self.series_store
    }

    pub fn register_aligned_kline(
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

    pub fn register_kline(&mut self, symbol: &str, duration_nanos: i64, width: usize) -> ReplayKlineHandle {
        let id = ReplayHandleId(format!("kline_{}", self.next_handle_id));
        self.next_handle_id += 1;

        let handle = ReplayKlineHandle::new(id, symbol.to_string(), duration_nanos, width);
        self.klines.push(handle.clone());
        handle
    }

    pub fn set_tick_window_width(&mut self, width: usize) {
        self.tick_window_width = width;
    }

    pub async fn step(&mut self) -> Result<Option<ReplayStep>> {
        let Some(next_timestamp) = self
            .feeds
            .iter()
            .filter_map(|(_, cursor)| cursor.peek_timestamp())
            .min()
        else {
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
                self.series_store.push_tick(&symbol, tick, self.tick_window_width);
                if update.source_selected && !updated_quotes.contains(&symbol) {
                    updated_quotes.push(symbol);
                }
            }
            FeedEvent::BarOpen {
                symbol,
                duration_nanos,
                kline,
            } => {
                self.series_store
                    .push_kline(&symbol, duration_nanos, kline.clone(), BarState::Opening, DEFAULT_SERIES_WIDTH);
                let update = self.quote_synthesizer.apply_bar_open(&symbol, duration_nanos, &kline);
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
                kline,
            } => {
                self.series_store
                    .close_kline(&symbol, duration_nanos, kline.clone(), DEFAULT_SERIES_WIDTH);
                let update = self.quote_synthesizer.apply_bar_close(&symbol, duration_nanos, &kline);
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
