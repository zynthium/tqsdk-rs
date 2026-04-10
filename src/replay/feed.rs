use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::errors::Result;
use crate::replay::InstrumentMetadata;
use crate::types::{Kline, Tick};

#[async_trait(?Send)]
pub(crate) trait HistoricalSource: Send + Sync {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata>;

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>>;
}

#[derive(Debug, Clone)]
pub enum FeedEvent {
    #[allow(dead_code)]
    Tick { symbol: String, tick: Tick },
    BarOpen {
        symbol: String,
        duration_nanos: i64,
        kline: Kline,
    },
    BarClose {
        symbol: String,
        duration_nanos: i64,
        kline: Kline,
    },
}

#[derive(Debug, Clone)]
pub struct FeedCursor {
    pending: VecDeque<FeedEvent>,
}

impl FeedCursor {
    pub fn from_kline_rows(symbol: &str, duration_nanos: i64, rows: Vec<Kline>) -> Self {
        let mut pending = VecDeque::new();

        for row in rows {
            pending.push_back(FeedEvent::BarOpen {
                symbol: symbol.to_string(),
                duration_nanos,
                kline: Kline {
                    high: row.open,
                    low: row.open,
                    close: row.open,
                    ..row.clone()
                },
            });
            pending.push_back(FeedEvent::BarClose {
                symbol: symbol.to_string(),
                duration_nanos,
                kline: row,
            });
        }

        Self { pending }
    }

    #[allow(dead_code)]
    pub fn from_tick_rows(symbol: &str, rows: Vec<Tick>) -> Self {
        let pending = rows
            .into_iter()
            .map(|tick| FeedEvent::Tick {
                symbol: symbol.to_string(),
                tick,
            })
            .collect();
        Self { pending }
    }

    pub fn peek_timestamp(&self) -> Option<i64> {
        match self.pending.front() {
            Some(FeedEvent::Tick { tick, .. }) => Some(tick.datetime),
            Some(FeedEvent::BarOpen { kline, .. }) => Some(kline.datetime),
            Some(FeedEvent::BarClose {
                kline, duration_nanos, ..
            }) => Some(kline.datetime + duration_nanos - 1),
            None => None,
        }
    }

    pub fn next_event(&mut self) -> Option<FeedEvent> {
        self.pending.pop_front()
    }
}
