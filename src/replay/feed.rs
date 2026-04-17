use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};

use crate::errors::Result;
use crate::replay::InstrumentMetadata;
use crate::types::{Kline, Tick};

use super::providers::ContinuousMapping;

#[derive(Debug, Clone)]
pub enum ReplayAuxiliaryEvent {
    ContinuousMapping(ContinuousMapping),
}

impl From<ContinuousMapping> for ReplayAuxiliaryEvent {
    fn from(mapping: ContinuousMapping) -> Self {
        Self::ContinuousMapping(mapping)
    }
}

impl ReplayAuxiliaryEvent {
    pub fn trading_day(&self) -> NaiveDate {
        match self {
            Self::ContinuousMapping(mapping) => mapping.trading_day,
        }
    }
}

#[async_trait]
pub(crate) trait HistoricalSource: Send + Sync {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata>;

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>>;

    async fn load_ticks(&self, symbol: &str, start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Result<Vec<Tick>>;

    async fn load_auxiliary_events(
        &self,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> Result<Vec<ReplayAuxiliaryEvent>> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone)]
pub enum FeedEvent {
    #[allow(dead_code)]
    Tick { symbol: String, tick: Tick },
    BarOpen {
        symbol: String,
        duration_nanos: i64,
        event_timestamp_nanos: i64,
        kline: Kline,
    },
    BarClose {
        symbol: String,
        duration_nanos: i64,
        event_timestamp_nanos: i64,
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
            let (open_timestamp_nanos, close_timestamp_nanos) =
                kline_event_timestamps_nanos(row.datetime, duration_nanos);
            pending.push_back(FeedEvent::BarOpen {
                symbol: symbol.to_string(),
                duration_nanos,
                event_timestamp_nanos: open_timestamp_nanos,
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
                event_timestamp_nanos: close_timestamp_nanos,
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
            Some(FeedEvent::BarOpen {
                event_timestamp_nanos, ..
            }) => Some(*event_timestamp_nanos),
            Some(FeedEvent::BarClose {
                event_timestamp_nanos, ..
            }) => Some(*event_timestamp_nanos),
            None => None,
        }
    }

    pub fn next_event(&mut self) -> Option<FeedEvent> {
        self.pending.pop_front()
    }
}

const DAY_NANOS: i64 = 86_400_000_000_000;
const SIX_HOURS_NANOS: i64 = 21_600_000_000_000;
const BEGIN_MARK_NANOS: i64 = 631_123_200_000_000_000;

fn kline_event_timestamps_nanos(kline_datetime_nanos: i64, duration_nanos: i64) -> (i64, i64) {
    if duration_nanos < DAY_NANOS {
        return (
            kline_datetime_nanos,
            kline_datetime_nanos + duration_nanos.saturating_sub(1),
        );
    }

    (
        trading_day_start_time_nanos(kline_datetime_nanos),
        trading_day_start_time_nanos(kline_datetime_nanos + duration_nanos) - 1_000,
    )
}

fn trading_day_start_time_nanos(timestamp_nanos: i64) -> i64 {
    let mut start_time = timestamp_nanos - SIX_HOURS_NANOS;
    let week_day = (start_time - BEGIN_MARK_NANOS) / DAY_NANOS % 7;
    if week_day >= 5 {
        start_time -= DAY_NANOS * (week_day - 4);
    }
    start_time
}
