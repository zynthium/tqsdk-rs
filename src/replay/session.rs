use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use tokio::sync::Mutex;

use crate::errors::{Result, TqError};
use crate::replay::{
    BacktestResult, HistoricalSource, InstrumentMetadata, ReplayConfig, ReplayKlineHandle, ReplayQuote, ReplayStep,
};
use crate::runtime::{RuntimeMode, TqRuntime};

use super::feed::FeedCursor;
use super::kernel::ReplayKernel;
use super::runtime::{ReplayExecutionAdapter, ReplayExecutionState, ReplayMarketAdapter, ReplayMarketState};
use super::sim::SimBroker;

const IMPLICIT_QUOTE_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ReplayQuoteHandle {
    symbol: String,
    market: Arc<ReplayMarketState>,
}

impl ReplayQuoteHandle {
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub async fn snapshot(&self) -> Option<ReplayQuote> {
        self.market.replay_quote(&self.symbol).await
    }
}

pub struct ReplaySession {
    config: ReplayConfig,
    source: Arc<dyn HistoricalSource>,
    kernel: Arc<Mutex<ReplayKernel>>,
    market: Arc<ReplayMarketState>,
    registered_feeds: HashSet<(String, i64)>,
    execution: Option<Arc<ReplayExecutionState>>,
    runtime: Option<Arc<TqRuntime>>,
    active_trading_day: Option<NaiveDate>,
    active_trading_day_end_nanos: Option<i64>,
}

impl ReplaySession {
    pub(crate) async fn from_source(config: ReplayConfig, source: Arc<dyn HistoricalSource>) -> Result<Self> {
        Ok(Self {
            config,
            source,
            kernel: Arc::new(Mutex::new(ReplayKernel::default())),
            market: Arc::new(ReplayMarketState::default()),
            registered_feeds: HashSet::new(),
            execution: None,
            runtime: None,
            active_trading_day: None,
            active_trading_day_end_nanos: None,
        })
    }

    pub async fn quote(&mut self, symbol: &str) -> Result<ReplayQuoteHandle> {
        let metadata = self.ensure_symbol(symbol).await?;
        self.ensure_option_underlying_quote_feed(&metadata).await?;
        self.ensure_kline_feed(
            symbol,
            IMPLICIT_QUOTE_DURATION,
            duration_nanos(IMPLICIT_QUOTE_DURATION)?,
            &metadata,
        )
        .await?;
        Ok(ReplayQuoteHandle {
            symbol: symbol.to_string(),
            market: Arc::clone(&self.market),
        })
    }

    pub async fn kline(&mut self, symbol: &str, duration: Duration, width: usize) -> Result<ReplayKlineHandle> {
        let metadata = self.ensure_symbol(symbol).await?;
        self.ensure_option_underlying_quote_feed(&metadata).await?;
        let duration_nanos = duration_nanos(duration)?;
        self.ensure_kline_feed(symbol, duration, duration_nanos, &metadata)
            .await?;

        let handle = {
            let mut kernel = self.kernel.lock().await;
            kernel.register_kline(symbol, duration_nanos, width, metadata.clone())
        };

        Ok(handle)
    }

    pub async fn runtime<I, S>(&mut self, accounts: I) -> Result<Arc<TqRuntime>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if let Some(runtime) = &self.runtime {
            return Ok(Arc::clone(runtime));
        }

        let account_keys = accounts
            .into_iter()
            .map(|account| account.as_ref().to_string())
            .collect::<Vec<_>>();
        let execution = Arc::new(ReplayExecutionState::new(SimBroker::new(
            account_keys.clone(),
            self.config.initial_balance,
        )));
        for metadata in self.market.all_metadata().await {
            execution.register_symbol(metadata).await;
        }
        let market = Arc::new(ReplayMarketAdapter::new(Arc::clone(&self.market)));
        let adapter = Arc::new(ReplayExecutionAdapter::new(
            account_keys,
            Arc::clone(&execution),
            Arc::clone(&self.market),
        ));
        let runtime = Arc::new(TqRuntime::new(RuntimeMode::Backtest, market, adapter));

        self.execution = Some(execution);
        self.runtime = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    pub async fn step(&mut self) -> Result<Option<ReplayStep>> {
        let Some(mut step) = self.kernel.lock().await.step().await? else {
            return Ok(None);
        };
        let mut settled_trading_day = None;

        for symbol in &step.updated_quotes {
            let (visible, path) = {
                let kernel = self.kernel.lock().await;
                let visible = kernel.visible_quote(symbol).cloned().ok_or_else(|| {
                    TqError::InternalError(format!("replay kernel missing visible quote for {symbol}"))
                })?;
                let path = kernel.quote_path(symbol).unwrap_or(&[]).to_vec();
                (visible, path)
            };

            if let Some(trading_day) = self.advance_trading_day_clock(visible.datetime_nanos).await? {
                settled_trading_day = Some(trading_day);
            }
            self.market.update_quote(visible).await;
            if let Some(execution) = &self.execution {
                execution.apply_quote_path(symbol, &path).await?;
                execution.notify_update().await;
            }
        }

        tokio::task::yield_now().await;
        step.settled_trading_day = settled_trading_day;
        Ok(Some(step))
    }

    pub async fn finish(&mut self) -> Result<BacktestResult> {
        if let (Some(execution), Some(trading_day)) = (&self.execution, self.active_trading_day) {
            execution.settle_day(trading_day).await?;
        }
        match &self.execution {
            Some(execution) => execution.finish().await,
            None => SimBroker::default().finish(),
        }
    }

    async fn ensure_symbol(&self, symbol: &str) -> Result<InstrumentMetadata> {
        if let Some(existing) = self.market.metadata_for(symbol).await {
            return Ok(existing);
        }

        let metadata = self.source.instrument_metadata(symbol).await?;
        self.market.register_symbol(metadata.clone()).await;
        self.kernel.lock().await.register_quote(symbol, metadata.clone());
        if let Some(execution) = &self.execution {
            execution.register_symbol(metadata.clone()).await;
        }
        Ok(metadata)
    }

    async fn ensure_option_underlying_quote_feed(&mut self, metadata: &InstrumentMetadata) -> Result<()> {
        if !metadata.class.ends_with("OPTION") || metadata.underlying_symbol.is_empty() {
            return Ok(());
        }

        let underlying_metadata = self.ensure_symbol(&metadata.underlying_symbol).await?;
        self.ensure_kline_feed(
            &metadata.underlying_symbol,
            IMPLICIT_QUOTE_DURATION,
            duration_nanos(IMPLICIT_QUOTE_DURATION)?,
            &underlying_metadata,
        )
        .await
    }

    async fn ensure_kline_feed(
        &mut self,
        symbol: &str,
        duration: Duration,
        duration_nanos: i64,
        metadata: &InstrumentMetadata,
    ) -> Result<()> {
        let key = (symbol.to_string(), duration_nanos);
        if self.registered_feeds.contains(&key) {
            return Ok(());
        }

        let rows = self
            .source
            .load_klines(symbol, duration, self.config.start_dt, self.config.end_dt)
            .await?;
        self.kernel.lock().await.push_feed(
            symbol.to_string(),
            FeedCursor::from_kline_rows(symbol, duration_nanos, rows.clone()),
        );
        self.registered_feeds.insert(key);

        if self.market.replay_quote(symbol).await.is_none()
            && let Some(first) = rows.first()
        {
            self.market
                .update_quote(preview_bar_open_quote(symbol, metadata, first))
                .await;
        }

        Ok(())
    }

    async fn advance_trading_day_clock(&mut self, quote_datetime_nanos: i64) -> Result<Option<NaiveDate>> {
        let trading_day_nanos = trading_day_from_timestamp_nanos(quote_datetime_nanos);
        let trading_day = trading_day_date(trading_day_nanos);
        let trading_day_end_nanos = trading_day_end_time_nanos(trading_day_nanos);

        let Some(active_trading_day) = self.active_trading_day else {
            self.active_trading_day = Some(trading_day);
            self.active_trading_day_end_nanos = Some(trading_day_end_nanos);
            return Ok(None);
        };

        if quote_datetime_nanos <= self.active_trading_day_end_nanos.unwrap_or(i64::MAX) {
            return Ok(None);
        }

        if let Some(execution) = &self.execution {
            execution.settle_day(active_trading_day).await?;
        }
        self.active_trading_day = Some(trading_day);
        self.active_trading_day_end_nanos = Some(trading_day_end_nanos);

        Ok(self.execution.as_ref().map(|_| active_trading_day))
    }
}

fn preview_bar_open_quote(symbol: &str, metadata: &InstrumentMetadata, bar: &crate::types::Kline) -> ReplayQuote {
    let price_tick = if metadata.price_tick.is_finite() && metadata.price_tick > 0.0 {
        metadata.price_tick
    } else {
        0.0
    };

    ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos: bar.datetime,
        last_price: bar.open,
        ask_price1: bar.open + price_tick,
        ask_volume1: 1,
        bid_price1: bar.open - price_tick,
        bid_volume1: 1,
        highest: bar.high,
        lowest: bar.low,
        average: bar.open,
        volume: bar.volume,
        amount: 0.0,
        open_interest: bar.open_oi,
    }
}

fn duration_nanos(duration: Duration) -> Result<i64> {
    i64::try_from(duration.as_nanos())
        .map_err(|_| TqError::InvalidParameter(format!("duration is too large: {:?}", duration)))
}

const CST_OFFSET_SECONDS: i32 = 8 * 3600;
const DAY_NANOS: i64 = 86_400_000_000_000;
const EIGHTEEN_HOURS_NANOS: i64 = 64_800_000_000_000;
const TRADING_DAY_END_NANOS: i64 = 64_799_999_999_999;
const BEGIN_MARK_NANOS: i64 = 631_123_200_000_000_000;

fn trading_day_from_timestamp_nanos(timestamp_nanos: i64) -> i64 {
    let mut days = (timestamp_nanos - BEGIN_MARK_NANOS) / DAY_NANOS;
    if (timestamp_nanos - BEGIN_MARK_NANOS) % DAY_NANOS >= EIGHTEEN_HOURS_NANOS {
        days += 1;
    }
    let week_day = days % 7;
    if week_day >= 5 {
        days += 7 - week_day;
    }
    BEGIN_MARK_NANOS + days * DAY_NANOS
}

fn trading_day_end_time_nanos(trading_day_nanos: i64) -> i64 {
    trading_day_nanos + TRADING_DAY_END_NANOS
}

fn trading_day_date(trading_day_nanos: i64) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_nanos(trading_day_nanos)
        .with_timezone(&FixedOffset::east_opt(CST_OFFSET_SECONDS).expect("CST offset must be valid"))
        .date_naive()
}
