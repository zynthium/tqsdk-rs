use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::errors::Result;
use crate::replay::{
    AlignedKlineHandle, AlignedKlineRow, BarState, FeedCursor, FeedEvent, ReplayHandleId, ReplayStep, SeriesStore,
};

const DEFAULT_SERIES_WIDTH: usize = 1_024;

#[derive(Debug)]
pub struct ReplayKernel {
    feeds: Vec<(String, FeedCursor)>,
    series_store: SeriesStore,
    aligned_klines: Vec<AlignedKlineHandle>,
    next_handle_id: usize,
}

impl ReplayKernel {
    pub fn for_test(feeds: Vec<(String, FeedCursor)>) -> Self {
        Self {
            feeds,
            series_store: SeriesStore::default(),
            aligned_klines: Vec::new(),
            next_handle_id: 0,
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

        for (_, cursor) in &mut self.feeds {
            loop {
                if cursor.peek_timestamp() != Some(next_timestamp) {
                    break;
                }

                let Some(event) = cursor.next_event() else {
                    break;
                };
                Self::apply_event(&mut self.series_store, event, &mut updated_symbols);
            }
        }

        let updated_handles = self.materialize_aligned_rows(next_timestamp, &updated_symbols).await;

        Ok(Some(ReplayStep {
            current_dt: DateTime::<Utc>::from_timestamp_nanos(next_timestamp),
            updated_handles,
            updated_quotes: Vec::new(),
            settled_trading_day: None,
        }))
    }

    fn apply_event(series_store: &mut SeriesStore, event: FeedEvent, updated_symbols: &mut Vec<String>) {
        match event {
            FeedEvent::Tick { .. } => {}
            FeedEvent::BarOpen {
                symbol,
                duration_nanos,
                kline,
            } => {
                series_store.push_kline(&symbol, duration_nanos, kline, BarState::Opening, DEFAULT_SERIES_WIDTH);
                updated_symbols.push(symbol);
            }
            FeedEvent::BarClose {
                symbol,
                duration_nanos,
                kline,
            } => {
                series_store.push_kline(&symbol, duration_nanos, kline, BarState::Closed, DEFAULT_SERIES_WIDTH);
                updated_symbols.push(symbol);
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
