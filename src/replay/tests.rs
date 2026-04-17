use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::errors::TqError;
use crate::runtime::TargetPosTask;
use crate::types::{
    DIRECTION_BUY, DIRECTION_SELL, InsertOrderRequest, Kline, OFFSET_CLOSE, OFFSET_OPEN, PRICE_TYPE_ANY,
    PRICE_TYPE_LIMIT, Tick,
};
use async_trait::async_trait;
use tokio::time::{sleep, timeout};

use super::feed::{FeedCursor, FeedEvent, HistoricalSource, ReplayAuxiliaryEvent};
use super::kernel::ReplayKernel;
use super::providers::{ContinuousContractProvider, ContinuousMapping};
use super::quote::{QuoteSelection, QuoteSynthesizer};
use super::series::SeriesStore;
use super::sim::SimBroker;
use super::{BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayQuote, ReplaySession};

struct FakeHistoricalSource {
    meta: HashMap<String, InstrumentMetadata>,
    klines: HashMap<(String, i64), Vec<Kline>>,
    ticks: HashMap<String, Vec<Tick>>,
}

#[async_trait]
impl HistoricalSource for FakeHistoricalSource {
    async fn instrument_metadata(&self, symbol: &str) -> crate::Result<InstrumentMetadata> {
        Ok(self.meta.get(symbol).cloned().unwrap())
    }

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Kline>> {
        Ok(self
            .klines
            .get(&(symbol.to_string(), duration.as_nanos() as i64))
            .cloned()
            .unwrap_or_default())
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Tick>> {
        Ok(self.ticks.get(symbol).cloned().unwrap_or_default())
    }
}

struct FakeHistoricalSourceWithAux {
    inner: FakeHistoricalSource,
    auxiliary_events: Vec<ReplayAuxiliaryEvent>,
}

#[async_trait]
impl HistoricalSource for FakeHistoricalSourceWithAux {
    async fn instrument_metadata(&self, symbol: &str) -> crate::Result<InstrumentMetadata> {
        self.inner.instrument_metadata(symbol).await
    }

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Kline>> {
        self.inner.load_klines(symbol, duration, start_dt, end_dt).await
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Tick>> {
        self.inner.load_ticks(symbol, start_dt, end_dt).await
    }

    async fn load_auxiliary_events(
        &self,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<ReplayAuxiliaryEvent>> {
        Ok(self.auxiliary_events.clone())
    }
}

#[tokio::test]
async fn replay_session_runtime_can_drive_target_pos_task() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            "SHFE.rb2605".to_string(),
            InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                price_tick: 1.0,
                volume_multiple: 10,
                margin: 1000.0,
                commission: 2.0,
                ..InstrumentMetadata::default()
            },
        )]),
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![
                Kline {
                    id: 1,
                    datetime: 1_000,
                    open: 10.0,
                    high: 12.0,
                    low: 9.0,
                    close: 11.0,
                    open_oi: 10,
                    close_oi: 11,
                    volume: 5,
                    epoch: None,
                },
                Kline {
                    id: 2,
                    datetime: 60_000_001_000,
                    open: 11.0,
                    high: 13.0,
                    low: 10.0,
                    close: 12.0,
                    open_oi: 11,
                    close_oi: 12,
                    volume: 6,
                    epoch: None,
                },
            ],
        )]),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let _quote = session.quote("SHFE.rb2605").await.unwrap();
    let _klines = session.kline("SHFE.rb2605", Duration::from_secs(60), 32).await.unwrap();

    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").expect("configured account should exist");
    let task: TargetPosTask = account.target_pos("SHFE.rb2605").build().unwrap();

    task.set_target_volume(1).unwrap();

    while session.step().await.unwrap().is_some() {}

    let result = session.finish().await.unwrap();
    assert_eq!(result.final_positions.len(), 1);
    assert_eq!(result.final_positions[0].user_id, "TQSIM");
    assert_eq!(result.final_positions[0].exchange_id, "SHFE");
    assert_eq!(result.final_positions[0].instrument_id, "rb2605");
    assert_eq!(result.final_positions[0].volume_long, 1);
}

#[tokio::test]
async fn replay_session_runtime_accepts_same_account_set_in_different_order() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::new(),
        klines: HashMap::new(),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let first = session.runtime([" TQSIM ", "OTHER", "TQSIM"]).await.unwrap();
    let second = session.runtime(["OTHER", "TQSIM"]).await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn replay_session_runtime_preserves_initial_account_lookup_keys_after_normalized_reuse() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::new(),
        klines: HashMap::new(),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let first = session.runtime([" TQSIM "]).await.unwrap();
    assert!(first.account(" TQSIM ").is_ok());

    let second = session.runtime(["TQSIM"]).await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(second.account(" TQSIM ").is_ok());
}

#[tokio::test]
async fn replay_session_runtime_rejects_mismatched_account_sets() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::new(),
        klines: HashMap::new(),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    session.runtime(["TQSIM"]).await.unwrap();
    let err = match session.runtime(["TQSIM", "OTHER"]).await {
        Ok(_) => panic!("expected mismatched account set to be rejected"),
        Err(err) => err,
    };
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn replay_session_does_not_fill_target_pos_after_final_market_update_without_next_step() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            "SHFE.rb2605".to_string(),
            InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                price_tick: 1.0,
                volume_multiple: 10,
                margin: 1000.0,
                commission: 2.0,
                ..InstrumentMetadata::default()
            },
        )]),
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: 1_000,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 10,
                close_oi: 11,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let _quote = session.quote("SHFE.rb2605").await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").expect("configured account should exist");
    let task: TargetPosTask = account.target_pos("SHFE.rb2605").build().unwrap();

    while session.step().await.unwrap().is_some() {}

    task.set_target_volume(1).unwrap();
    timeout(Duration::from_millis(200), task.wait_target_reached())
        .await
        .expect_err("target should not be reachable without another replay step");
    task.cancel().await.unwrap();
    task.wait_finished().await.unwrap();

    let result = session.finish().await.unwrap();
    assert!(result.trades.is_empty());
    assert!(
        result
            .final_positions
            .iter()
            .all(|position| position.volume_long == 0 && position.volume_short == 0)
    );
}

#[tokio::test]
async fn replay_session_does_not_close_target_pos_after_final_market_update_without_next_step() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            "SHFE.rb2605".to_string(),
            InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                price_tick: 1.0,
                volume_multiple: 10,
                margin: 1000.0,
                commission: 2.0,
                ..InstrumentMetadata::default()
            },
        )]),
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![
                Kline {
                    id: 1,
                    datetime: 1_000,
                    open: 10.0,
                    high: 12.0,
                    low: 9.0,
                    close: 11.0,
                    open_oi: 10,
                    close_oi: 11,
                    volume: 5,
                    epoch: None,
                },
                Kline {
                    id: 2,
                    datetime: 60_000_001_000,
                    open: 11.0,
                    high: 13.0,
                    low: 10.0,
                    close: 12.0,
                    open_oi: 11,
                    close_oi: 12,
                    volume: 6,
                    epoch: None,
                },
            ],
        )]),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let _quote = session.quote("SHFE.rb2605").await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").expect("configured account should exist");
    let task: TargetPosTask = account.target_pos("SHFE.rb2605").build().unwrap();

    task.set_target_volume(1).unwrap();
    while session.step().await.unwrap().is_some() {}
    timeout(Duration::from_secs(1), task.wait_target_reached())
        .await
        .expect("open target should be reachable after replay steps")
        .expect("open target task should succeed");
    task.set_target_volume(0).unwrap();
    timeout(Duration::from_millis(200), task.wait_target_reached())
        .await
        .expect_err("close target should not be reachable without another replay step");
    task.cancel().await.unwrap();
    task.wait_finished().await.unwrap();

    let result = session.finish().await.unwrap();
    assert_eq!(result.trades.len(), 1);
    assert_eq!(result.final_positions.len(), 1);
    assert_eq!(result.final_positions[0].volume_long, 1);
}

#[tokio::test]
async fn replay_session_quote_uses_implicit_minute_feed() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            "SHFE.rb2605".to_string(),
            InstrumentMetadata {
                symbol: "SHFE.rb2605".to_string(),
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2605".to_string(),
                class: "FUTURE".to_string(),
                price_tick: 1.0,
                ..InstrumentMetadata::default()
            },
        )]),
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: 1_000,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 10,
                close_oi: 11,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            DateTime::<Utc>::from_timestamp(3600, 0).unwrap(),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let quote = session.quote("SHFE.rb2605").await.unwrap();

    let first_step = session
        .step()
        .await
        .unwrap()
        .expect("implicit minute feed should advance");
    assert_eq!(first_step.updated_quotes, vec!["SHFE.rb2605".to_string()]);

    let snapshot = quote.snapshot().await.unwrap();
    assert_eq!(snapshot.last_price, 10.0);
    assert_eq!(snapshot.ask_price1, 11.0);
}

#[tokio::test]
async fn replay_runtime_market_adapter_exposes_cst_datetime_and_trading_time() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(
            symbol.to_string(),
            future_metadata_with_trading_time(
                symbol,
                serde_json::json!({
                    "day": [["09:00:00", "15:00:00"]],
                    "night": []
                }),
            ),
        )]),
        klines: HashMap::from([(
            (symbol.to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 10,
                close_oi: 11,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let mut session = ReplaySession::from_source(
        ReplayConfig::new(
            chrono::FixedOffset::east_opt(8 * 3600)
                .unwrap()
                .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
                .single()
                .unwrap()
                .with_timezone(&Utc),
            chrono::FixedOffset::east_opt(8 * 3600)
                .unwrap()
                .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
                .single()
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap(),
        source,
    )
    .await
    .unwrap();

    let _quote = session.quote(symbol).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    session.step().await.unwrap().unwrap();

    let market = runtime.account("TQSIM").unwrap().runtime().market();
    let quote = market.latest_quote(symbol).await.unwrap();
    let trading_time = market.trading_time(symbol).await.unwrap().unwrap();

    assert_eq!(quote.datetime, "2026-04-09 09:01:00.000000");
    assert_eq!(
        trading_time,
        serde_json::json!({
            "day": [["09:00:00", "15:00:00"]],
            "night": []
        })
    );
}

#[tokio::test]
async fn replay_session_option_quote_bootstraps_underlying_for_runtime_orders() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([
            (underlying.to_string(), future_metadata(underlying)),
            (option.to_string(), option_metadata(option, underlying, "CALL", 100.0)),
        ]),
        klines: HashMap::from([
            (
                (underlying.to_string(), 60_000_000_000),
                vec![Kline {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    open: 100.0,
                    high: 100.0,
                    low: 100.0,
                    close: 100.0,
                    open_oi: 10,
                    close_oi: 10,
                    volume: 1,
                    epoch: None,
                }],
            ),
            (
                (option.to_string(), 60_000_000_000),
                vec![Kline {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    open: 2.0,
                    high: 2.0,
                    low: 2.0,
                    close: 2.0,
                    open_oi: 10,
                    close_oi: 10,
                    volume: 1,
                    epoch: None,
                }],
            ),
        ]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _quote = session.quote(option).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();

    let order_id = account
        .insert_order(&limit_order(option, DIRECTION_SELL, OFFSET_OPEN, 1.9, 1))
        .await
        .unwrap();
    let pending = runtime.execution().order("TQSIM", &order_id).await.unwrap();
    assert_eq!(pending.status, "ALIVE");
    assert_eq!(pending.volume_left, 1);

    let _ = session.step().await.unwrap().unwrap();
    let order = runtime.execution().order("TQSIM", &order_id).await.unwrap();
    let position = runtime.execution().position("TQSIM", option).await.unwrap();

    assert_eq!(order.status, "FINISHED");
    assert_eq!(order.volume_left, 0);
    assert_eq!(position.volume_short, 1);
}

#[tokio::test]
async fn replay_session_tick_handle_streams_rows_and_drives_quote() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::new(),
        ticks: HashMap::from([(
            symbol.to_string(),
            vec![
                Tick {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    last_price: 10.0,
                    average: 10.0,
                    highest: 10.0,
                    lowest: 10.0,
                    ask_price1: 10.1,
                    ask_volume1: 1,
                    bid_price1: 9.9,
                    bid_volume1: 1,
                    volume: 1,
                    amount: 10.0,
                    open_interest: 100,
                    ..Tick::default()
                },
                Tick {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 1),
                    last_price: 11.0,
                    average: 11.0,
                    highest: 11.0,
                    lowest: 10.0,
                    ask_price1: 11.1,
                    ask_volume1: 2,
                    bid_price1: 10.9,
                    bid_volume1: 2,
                    volume: 2,
                    amount: 22.0,
                    open_interest: 101,
                    ..Tick::default()
                },
            ],
        )]),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let quote = session.quote(symbol).await.unwrap();
    let ticks = session.tick(symbol, 8).await.unwrap();

    let first = session.step().await.unwrap().unwrap();
    assert_eq!(first.updated_quotes, vec![symbol.to_string()]);
    let first_rows = ticks.rows().await;
    assert_eq!(first_rows.len(), 1);
    assert_eq!(first_rows[0].tick.id, 1);
    assert_eq!(quote.snapshot().await.unwrap().last_price, 10.0);

    let second = session.step().await.unwrap().unwrap();
    assert_eq!(second.updated_quotes, vec![symbol.to_string()]);
    let second_rows = ticks.rows().await;
    assert_eq!(second_rows.len(), 2);
    assert_eq!(second_rows[1].tick.id, 2);
    assert_eq!(quote.snapshot().await.unwrap().last_price, 11.0);
}

#[tokio::test]
async fn replay_session_public_aligned_kline_matches_main_timeline() {
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([
            ("SHFE.rb2605".to_string(), future_metadata("SHFE.rb2605")),
            ("SHFE.hc2605".to_string(), future_metadata("SHFE.hc2605")),
        ]),
        klines: HashMap::from([
            (
                ("SHFE.rb2605".to_string(), 60_000_000_000),
                vec![Kline {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    open: 10.0,
                    high: 11.0,
                    low: 9.0,
                    close: 10.5,
                    open_oi: 100,
                    close_oi: 101,
                    volume: 5,
                    epoch: None,
                }],
            ),
            (
                ("SHFE.hc2605".to_string(), 60_000_000_000),
                vec![Kline {
                    id: 7,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    open: 20.0,
                    high: 21.0,
                    low: 19.0,
                    close: 20.5,
                    open_oi: 200,
                    close_oi: 201,
                    volume: 6,
                    epoch: None,
                }],
            ),
        ]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let aligned = session
        .aligned_kline(&["SHFE.rb2605", "SHFE.hc2605"], Duration::from_secs(60), 8)
        .await
        .unwrap();

    let _ = session.step().await.unwrap().unwrap();
    let rows = aligned.rows().await;
    let last = rows.last().unwrap();

    assert_eq!(last.datetime_nanos, shanghai_nanos(2026, 4, 9, 9, 1, 0));
    assert!(last.bars["SHFE.rb2605"].is_some());
    assert!(last.bars["SHFE.hc2605"].is_some());
}

#[tokio::test]
async fn replay_runtime_insert_order_bootstraps_symbol_without_explicit_quote_subscription() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::from([(
            (symbol.to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 10,
                close_oi: 11,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();

    let order_id = account.insert_order(&limit_buy(symbol, 11.0, 1)).await.unwrap();
    let pending = runtime.execution().order("TQSIM", &order_id).await.unwrap();
    assert_eq!(pending.status, "ALIVE");
    assert_eq!(pending.volume_left, 1);

    let _ = session.step().await.unwrap().unwrap();
    let filled = runtime.execution().order("TQSIM", &order_id).await.unwrap();
    assert_eq!(filled.status, "FINISHED");
    assert_eq!(filled.volume_left, 0);
}

#[tokio::test]
async fn replay_runtime_wait_order_update_ignores_quote_only_updates() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::from([(
            (symbol.to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 10.0,
                low: 10.0,
                close: 10.0,
                open_oi: 10,
                close_oi: 10,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();

    let order_id = account.insert_order(&limit_buy(symbol, 9.0, 1)).await.unwrap();
    let pending = runtime.execution().order("TQSIM", &order_id).await.unwrap();
    assert_eq!(pending.status, "ALIVE");
    assert_eq!(pending.volume_left, 1);

    let execution = Arc::clone(&runtime.execution());
    let wait_order_id = order_id.clone();
    let wait_task = tokio::spawn(async move { execution.wait_order_update("TQSIM", &wait_order_id).await });

    tokio::task::yield_now().await;
    let _ = session.step().await.unwrap().unwrap();
    sleep(Duration::from_millis(50)).await;
    assert!(
        !wait_task.is_finished(),
        "quote-only replay updates should not wake a waiter for an unchanged order"
    );

    wait_task.abort();
}

#[tokio::test]
async fn replay_account_trade_event_streams_publish_order_and_trade_updates() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::from([(
            (symbol.to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 10,
                close_oi: 11,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();
    let mut order_events = account.subscribe_order_events().unwrap();
    let mut trade_events = account.subscribe_trade_events().unwrap();

    let order_id = account.insert_order(&limit_buy(symbol, 11.0, 1)).await.unwrap();
    let submitted = timeout(Duration::from_secs(1), order_events.recv())
        .await
        .unwrap()
        .unwrap();
    match submitted.kind {
        crate::TradeSessionEventKind::OrderUpdated {
            order_id: got_order_id,
            order,
        } => {
            assert_eq!(got_order_id, order_id);
            assert_eq!(order.status, "ALIVE");
            assert_eq!(order.last_msg, "报单成功");
            assert_eq!(order.volume_left, 1);
        }
        other => panic!("unexpected order event after insert: {other:?}"),
    }

    let _ = session.step().await.unwrap().unwrap();

    let finished = timeout(Duration::from_secs(1), order_events.recv())
        .await
        .unwrap()
        .unwrap();
    match finished.kind {
        crate::TradeSessionEventKind::OrderUpdated {
            order_id: got_order_id,
            order,
        } => {
            assert_eq!(got_order_id, order_id);
            assert_eq!(order.status, "FINISHED");
            assert_eq!(order.last_msg, "全部成交");
            assert_eq!(order.volume_left, 0);
        }
        other => panic!("unexpected order event after fill: {other:?}"),
    }

    let trade_event = timeout(Duration::from_secs(1), trade_events.recv())
        .await
        .unwrap()
        .unwrap();
    match trade_event.kind {
        crate::TradeSessionEventKind::TradeCreated { trade_id, trade } => {
            assert_eq!(trade.order_id, order_id);
            assert_eq!(trade.trade_id, trade_id);
            assert_eq!(trade.direction, DIRECTION_BUY);
            assert_eq!(trade.offset, OFFSET_OPEN);
            assert_eq!(trade.volume, 1);
            assert_eq!(trade.price, 11.0);
            assert_eq!(trade.trade_date_time, shanghai_nanos(2026, 4, 9, 9, 1, 0));
        }
        other => panic!("unexpected trade event after fill: {other:?}"),
    }
}

#[tokio::test]
async fn replay_insert_order_matches_immediately_against_current_quote_snapshot() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::new(),
        ticks: HashMap::from([(
            symbol.to_string(),
            vec![Tick {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                last_price: 10.0,
                average: 10.0,
                highest: 10.0,
                lowest: 10.0,
                ask_price1: 10.5,
                ask_volume1: 10,
                bid_price1: 9.5,
                bid_volume1: 10,
                volume: 10,
                amount: 1_000.0,
                open_interest: 100,
                ..Tick::default()
            }],
        )]),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _ticks = session.tick(symbol, 8).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();
    let mut order_events = account.subscribe_order_events().unwrap();
    let mut trade_events = account.subscribe_trade_events().unwrap();

    let first_step = session.step().await.unwrap().unwrap();
    assert_eq!(
        first_step.current_dt,
        DateTime::<Utc>::from_timestamp_nanos(shanghai_nanos(2026, 4, 9, 9, 1, 0))
    );

    let order_id = account.insert_order(&limit_buy(symbol, 10.5, 1)).await.unwrap();

    let submitted = timeout(Duration::from_secs(1), order_events.recv())
        .await
        .unwrap()
        .unwrap();
    match submitted.kind {
        crate::TradeSessionEventKind::OrderUpdated {
            order_id: got_order_id,
            order,
        } => {
            assert_eq!(got_order_id, order_id);
            assert_eq!(order.status, "ALIVE");
            assert_eq!(order.last_msg, "报单成功");
            assert_eq!(order.volume_left, 1);
        }
        other => panic!("unexpected order event after insert: {other:?}"),
    }

    let finished = timeout(Duration::from_millis(200), order_events.recv())
        .await
        .expect("crossing order should finish immediately against current quote snapshot")
        .unwrap();
    match finished.kind {
        crate::TradeSessionEventKind::OrderUpdated {
            order_id: got_order_id,
            order,
        } => {
            assert_eq!(got_order_id, order_id);
            assert_eq!(order.status, "FINISHED");
            assert_eq!(order.last_msg, "全部成交");
            assert_eq!(order.volume_left, 0);
        }
        other => panic!("unexpected order event after immediate fill: {other:?}"),
    }

    let trade_event = timeout(Duration::from_millis(200), trade_events.recv())
        .await
        .expect("crossing order should publish trade immediately against current quote snapshot")
        .unwrap();
    match trade_event.kind {
        crate::TradeSessionEventKind::TradeCreated { trade_id, trade } => {
            assert_eq!(trade.order_id, order_id);
            assert_eq!(trade.trade_id, trade_id);
            assert_eq!(trade.direction, DIRECTION_BUY);
            assert_eq!(trade.offset, OFFSET_OPEN);
            assert_eq!(trade.volume, 1);
            assert_eq!(trade.price, 10.5);
            assert_eq!(trade.trade_date_time, shanghai_nanos(2026, 4, 9, 9, 1, 0));
        }
        other => panic!("unexpected trade event after immediate fill: {other:?}"),
    }

    let result = session.finish().await.unwrap();
    assert_eq!(result.trades.len(), 1);
    assert_eq!(result.trades[0].order_id, order_id);
}

#[tokio::test]
async fn replay_step_flushes_pending_target_pos_commands_before_advancing_market() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::new(),
        ticks: HashMap::from([(
            symbol.to_string(),
            vec![
                Tick {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    last_price: 10.0,
                    average: 10.0,
                    highest: 10.0,
                    lowest: 10.0,
                    ask_price1: 10.5,
                    ask_volume1: 10,
                    bid_price1: 9.5,
                    bid_volume1: 10,
                    volume: 10,
                    amount: 1_000.0,
                    open_interest: 100,
                    ..Tick::default()
                },
                Tick {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 2, 0),
                    last_price: 12.0,
                    average: 12.0,
                    highest: 12.0,
                    lowest: 12.0,
                    ask_price1: 12.5,
                    ask_volume1: 10,
                    bid_price1: 11.5,
                    bid_volume1: 10,
                    volume: 20,
                    amount: 2_400.0,
                    open_interest: 110,
                    ..Tick::default()
                },
            ],
        )]),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _ticks = session.tick(symbol, 8).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();
    let task: TargetPosTask = account.target_pos(symbol).build().unwrap();
    let mut trade_events = account.subscribe_trade_events().unwrap();

    let first_step = session.step().await.unwrap().unwrap();
    assert_eq!(
        first_step.current_dt,
        DateTime::<Utc>::from_timestamp_nanos(shanghai_nanos(2026, 4, 9, 9, 1, 0))
    );

    task.set_target_volume(1).unwrap();

    let second_step = session.step().await.unwrap().unwrap();
    assert_eq!(
        second_step.current_dt,
        DateTime::<Utc>::from_timestamp_nanos(shanghai_nanos(2026, 4, 9, 9, 2, 0))
    );

    let trade_event = timeout(Duration::from_secs(1), trade_events.recv())
        .await
        .unwrap()
        .unwrap();
    match trade_event.kind {
        crate::TradeSessionEventKind::TradeCreated { trade, .. } => {
            assert_eq!(trade.price, 10.5);
            assert_eq!(trade.trade_date_time, shanghai_nanos(2026, 4, 9, 9, 1, 0));
        }
        other => panic!("unexpected trade event after target_pos command: {other:?}"),
    }

    task.cancel().await.unwrap();
    task.wait_finished().await.unwrap();
    let result = session.finish().await.unwrap();
    assert_eq!(result.trades.len(), 1);
    assert_eq!(result.trades[0].trade_date_time, shanghai_nanos(2026, 4, 9, 9, 1, 0));
}

#[tokio::test]
async fn replay_step_flushes_target_pos_against_opening_bar_quote_before_close_event() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::from([(
            (symbol.to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                open_oi: 100,
                close_oi: 110,
                volume: 10,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 9, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 15, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _quote = session.quote(symbol).await.unwrap();
    let _bars = session.kline(symbol, Duration::from_secs(60), 8).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();
    let task: TargetPosTask = account.target_pos(symbol).build().unwrap();
    let mut trade_events = account.subscribe_trade_events().unwrap();

    let first_step = session.step().await.unwrap().unwrap();
    assert_eq!(
        first_step.current_dt,
        DateTime::<Utc>::from_timestamp_nanos(shanghai_nanos(2026, 4, 9, 9, 1, 0))
    );

    task.set_target_volume(1).unwrap();

    let second_step = session.step().await.unwrap().unwrap();
    assert_eq!(
        second_step.current_dt,
        DateTime::<Utc>::from_timestamp_nanos(shanghai_nanos(2026, 4, 9, 9, 1, 59) + 999_999_999)
    );

    let trade_event = timeout(Duration::from_secs(1), trade_events.recv())
        .await
        .unwrap()
        .unwrap();
    match trade_event.kind {
        crate::TradeSessionEventKind::TradeCreated { trade, .. } => {
            assert_eq!(trade.price, 11.0);
            assert_eq!(trade.trade_date_time, shanghai_nanos(2026, 4, 9, 9, 1, 0));
        }
        other => panic!("unexpected trade event after kline-driven target_pos command: {other:?}"),
    }

    task.cancel().await.unwrap();
    task.wait_finished().await.unwrap();
}

#[tokio::test]
async fn replay_target_pos_replans_shfe_close_today_after_settlement_retry() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::new(),
        ticks: HashMap::from([(
            symbol.to_string(),
            vec![
                Tick {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 14, 58, 0),
                    last_price: 100.0,
                    average: 100.0,
                    highest: 100.0,
                    lowest: 100.0,
                    ask_price1: 101.0,
                    ask_volume1: 1,
                    bid_price1: 99.0,
                    bid_volume1: 1,
                    volume: 1,
                    amount: 100.0,
                    open_interest: 100,
                    ..Tick::default()
                },
                Tick {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 9, 14, 59, 0),
                    last_price: 100.0,
                    average: 100.0,
                    highest: 100.0,
                    lowest: 100.0,
                    ask_price1: 101.0,
                    ask_volume1: 1,
                    bid_price1: 99.0,
                    bid_volume1: 1,
                    volume: 2,
                    amount: 200.0,
                    open_interest: 101,
                    ..Tick::default()
                },
                Tick {
                    id: 3,
                    datetime: shanghai_nanos(2026, 4, 10, 9, 0, 0),
                    last_price: 100.0,
                    average: 100.0,
                    highest: 100.0,
                    lowest: 100.0,
                    ask_price1: 101.0,
                    ask_volume1: 1,
                    bid_price1: 99.0,
                    bid_volume1: 1,
                    volume: 3,
                    amount: 300.0,
                    open_interest: 102,
                    ..Tick::default()
                },
                Tick {
                    id: 4,
                    datetime: shanghai_nanos(2026, 4, 10, 9, 1, 0),
                    last_price: 100.0,
                    average: 100.0,
                    highest: 100.0,
                    lowest: 100.0,
                    ask_price1: 101.0,
                    ask_volume1: 1,
                    bid_price1: 99.0,
                    bid_volume1: 1,
                    volume: 4,
                    amount: 400.0,
                    open_interest: 103,
                    ..Tick::default()
                },
                Tick {
                    id: 5,
                    datetime: shanghai_nanos(2026, 4, 10, 9, 2, 0),
                    last_price: 100.0,
                    average: 100.0,
                    highest: 100.0,
                    lowest: 100.0,
                    ask_price1: 101.0,
                    ask_volume1: 1,
                    bid_price1: 99.0,
                    bid_volume1: 1,
                    volume: 5,
                    amount: 500.0,
                    open_interest: 104,
                    ..Tick::default()
                },
            ],
        )]),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 14, 57, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 10, 9, 5, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _quote = session.quote(symbol).await.unwrap();
    let _ticks = session.tick(symbol, 8).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").unwrap();

    let open_order = account.insert_order(&limit_buy(symbol, 1_000.0, 1)).await.unwrap();
    let _ = session.step().await.unwrap().unwrap();
    let _ = session.step().await.unwrap().unwrap();

    let open_state = runtime.execution().order("TQSIM", &open_order).await.unwrap();
    assert_eq!(open_state.status, "FINISHED");
    let long_position = runtime.execution().position("TQSIM", symbol).await.unwrap();
    assert_eq!(long_position.volume_long_today, 1);
    assert_eq!(long_position.volume_long_his, 0);

    let task: TargetPosTask = account.target_pos(symbol).build().unwrap();
    task.set_target_volume(0).unwrap();
    tokio::task::yield_now().await;

    let _ = session.step().await.unwrap().unwrap();
    let after_settlement = runtime.execution().position("TQSIM", symbol).await.unwrap();
    assert_eq!(after_settlement.volume_long_today, 0);
    assert_eq!(after_settlement.volume_long_his, 1);

    let _ = session.step().await.unwrap().unwrap();
    let _ = session.step().await.unwrap().unwrap();

    timeout(Duration::from_secs(1), task.wait_target_reached())
        .await
        .expect("task should resolve after the next trading-day quotes")
        .expect("task should replan a carried SHFE close-today order instead of failing");

    let flat_position = runtime.execution().position("TQSIM", symbol).await.unwrap();
    assert_eq!(flat_position.volume_long, 0);
    assert_eq!(flat_position.volume_short, 0);
}

#[test]
fn replay_config_rejects_inverted_range() {
    let start_dt = DateTime::<Utc>::from_timestamp(1_700_000_100, 0).unwrap();
    let end_dt = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

    let err = ReplayConfig::new(start_dt, end_dt).unwrap_err();
    assert!(err.to_string().contains("end_dt must be after start_dt"));
}

#[test]
fn instrument_metadata_serialization_excludes_dynamic_quote_fields() {
    let value = serde_json::to_value(InstrumentMetadata::default()).unwrap();

    assert!(value.get("last_price").is_none());
    assert!(value.get("ask_price1").is_none());
}

#[test]
fn bar_state_helper_methods_match_semantics() {
    assert!(BarState::Opening.is_opening());
    assert!(BarState::Closed.is_closed());
}

#[test]
fn feed_cursor_emits_open_then_close_for_one_kline() {
    let bars = vec![Kline {
        id: 7,
        datetime: 1_000,
        open: 10.0,
        high: 12.0,
        low: 9.0,
        close: 11.0,
        open_oi: 100,
        close_oi: 105,
        volume: 6,
        epoch: None,
    }];

    let mut cursor = FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars);

    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarOpen { .. }));
    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarClose { .. }));
    assert!(cursor.next_event().is_none());
}

#[test]
fn daily_feed_cursor_uses_python_trading_day_boundaries() {
    let bars = vec![Kline {
        id: 1,
        datetime: shanghai_nanos(2026, 4, 8, 0, 0, 0),
        open: 10.0,
        high: 12.0,
        low: 9.0,
        close: 11.0,
        open_oi: 100,
        close_oi: 105,
        volume: 6,
        epoch: None,
    }];

    let mut cursor = FeedCursor::from_kline_rows("SHFE.rb2605", 86_400_000_000_000, bars);

    assert_eq!(cursor.peek_timestamp(), Some(shanghai_nanos(2026, 4, 7, 18, 0, 0)));
    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarOpen { .. }));
    assert_eq!(
        cursor.peek_timestamp(),
        Some(shanghai_nanos(2026, 4, 8, 18, 0, 0) - 1_000)
    );
    assert!(matches!(cursor.next_event().unwrap(), FeedEvent::BarClose { .. }));
    assert!(cursor.next_event().is_none());
}

#[test]
fn continuous_provider_only_reveals_requested_day() {
    let provider = ContinuousContractProvider::from_rows(vec![
        ContinuousMapping {
            trading_day: NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
            symbol: "KQ.m@SHFE.rb".to_string(),
            underlying_symbol: "SHFE.rb2605".to_string(),
        },
        ContinuousMapping {
            trading_day: NaiveDate::from_ymd_opt(2026, 4, 9).unwrap(),
            symbol: "KQ.m@SHFE.rb".to_string(),
            underlying_symbol: "SHFE.rb2610".to_string(),
        },
    ]);

    let current = provider.mapping_for(NaiveDate::from_ymd_opt(2026, 4, 8).unwrap());
    assert_eq!(current["KQ.m@SHFE.rb"], "SHFE.rb2605");
}

#[tokio::test]
async fn replay_session_continuous_contract_switches_underlying_symbol() {
    let symbol = "KQ.m@SHFE.rb";
    let first_day = NaiveDate::from_ymd_opt(2026, 4, 8).unwrap();
    let second_day = NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
    let provider = ContinuousContractProvider::from_rows(vec![
        ContinuousMapping {
            trading_day: first_day,
            symbol: symbol.to_string(),
            underlying_symbol: "SHFE.rb2605".to_string(),
        },
        ContinuousMapping {
            trading_day: second_day,
            symbol: symbol.to_string(),
            underlying_symbol: "SHFE.rb2610".to_string(),
        },
    ]);
    let first_day_mapping = provider.mapping_for(first_day);
    let second_day_mapping = provider.mapping_for(second_day);
    let expected_first_underlying = first_day_mapping.get(symbol).unwrap().clone();
    let expected_second_underlying = second_day_mapping.get(symbol).unwrap().clone();

    let source = Arc::new(FakeHistoricalSourceWithAux {
        inner: FakeHistoricalSource {
            meta: HashMap::from([(
                symbol.to_string(),
                InstrumentMetadata {
                    underlying_symbol: expected_first_underlying.clone(),
                    price_tick: 1.0,
                    ..future_metadata(symbol)
                },
            )]),
            klines: HashMap::from([(
                (symbol.to_string(), 86_400_000_000_000),
                vec![
                    Kline {
                        id: 1,
                        datetime: shanghai_nanos(2026, 4, 8, 0, 0, 0),
                        open: 10.0,
                        high: 12.0,
                        low: 9.0,
                        close: 11.0,
                        open_oi: 100,
                        close_oi: 110,
                        volume: 5,
                        epoch: None,
                    },
                    Kline {
                        id: 2,
                        datetime: shanghai_nanos(2026, 4, 9, 0, 0, 0),
                        open: 11.0,
                        high: 13.0,
                        low: 10.0,
                        close: 12.0,
                        open_oi: 110,
                        close_oi: 120,
                        volume: 6,
                        epoch: None,
                    },
                ],
            )]),
            ticks: HashMap::new(),
        },
        auxiliary_events: vec![
            ReplayAuxiliaryEvent::ContinuousMapping(ContinuousMapping {
                trading_day: first_day,
                symbol: symbol.to_string(),
                underlying_symbol: expected_first_underlying.clone(),
            }),
            ReplayAuxiliaryEvent::ContinuousMapping(ContinuousMapping {
                trading_day: second_day,
                symbol: symbol.to_string(),
                underlying_symbol: expected_second_underlying.clone(),
            }),
        ],
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 8, 0, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 9, 23, 59, 59)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _daily = session.kline(symbol, Duration::from_secs(86_400), 8).await.unwrap();
    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let market = runtime.account("TQSIM").unwrap().runtime().market();

    let mut observed_underlyings = Vec::new();
    while let Some(step) = session.step().await.unwrap() {
        let quote = market.latest_quote(symbol).await.unwrap();
        observed_underlyings.push((step.current_dt, quote.underlying_symbol));
    }

    assert_eq!(
        observed_underlyings.first().map(|(_, underlying)| underlying.as_str()),
        Some(expected_first_underlying.as_str())
    );
    assert_eq!(
        observed_underlyings.last().map(|(_, underlying)| underlying.as_str()),
        Some(expected_second_underlying.as_str())
    );
}

#[test]
fn series_store_keeps_fixed_width_kline_window() {
    let mut store = SeriesStore::default();

    for id in 1..=3 {
        store.push_kline(
            "SHFE.rb2605",
            60_000_000_000,
            Kline {
                id,
                datetime: id * 1_000,
                open: 10.0,
                high: 10.0,
                low: 10.0,
                close: 10.0,
                open_oi: 100,
                close_oi: 100,
                volume: 1,
                epoch: None,
            },
            BarState::Closed,
            2,
        );
    }

    let rows = store.kline_rows("SHFE.rb2605", 60_000_000_000);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kline.id, 2);
    assert_eq!(rows[1].kline.id, 3);
}

#[test]
fn quote_synthesizer_prefers_tick_over_kline() {
    let mut synth = QuoteSynthesizer::default();
    let meta = InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        price_tick: 1.0,
        ..InstrumentMetadata::default()
    };

    synth.register_symbol(
        "SHFE.rb2605",
        meta,
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );
    synth.register_symbol("SHFE.rb2605", InstrumentMetadata::default(), QuoteSelection::Tick);

    let update = synth.apply_tick(
        "SHFE.rb2605",
        &Tick {
            datetime: 2_000,
            last_price: 12.0,
            ask_price1: 13.0,
            ask_volume1: 1,
            bid_price1: 11.0,
            bid_volume1: 1,
            ..Tick::default()
        },
    );

    assert_eq!(update.visible.last_price, 12.0);
}

#[test]
fn quote_synthesizer_builds_ohlc_price_path_for_smallest_kline() {
    let mut synth = QuoteSynthesizer::default();
    synth.register_symbol(
        "SHFE.rb2605",
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            price_tick: 1.0,
            ..InstrumentMetadata::default()
        },
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );

    let update = synth.apply_bar_close(
        "SHFE.rb2605",
        60_000_000_000,
        &Kline {
            datetime: 1_000,
            open: 10.0,
            high: 14.0,
            low: 9.0,
            close: 12.0,
            open_oi: 100,
            close_oi: 110,
            volume: 7,
            id: 1,
            epoch: None,
        },
        60_000_000_999,
    );

    let prices: Vec<f64> = update.path.iter().map(|step| step.last_price).collect();
    assert_eq!(prices, vec![10.0, 14.0, 9.0, 12.0]);
}

#[test]
fn quote_synthesizer_marks_implicit_minute_feed_as_lowest_priority() {
    let mut synth = QuoteSynthesizer::default();

    synth.register_symbol(
        "SHFE.rb2605",
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            price_tick: 1.0,
            ..InstrumentMetadata::default()
        },
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: true,
        },
    );

    assert_eq!(
        synth.selection_for("SHFE.rb2605"),
        Some(QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: true,
        })
    );
}

#[test]
fn quote_synthesizer_prefers_smaller_explicit_kline_over_larger_one() {
    let mut synth = QuoteSynthesizer::default();
    let meta = InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        price_tick: 1.0,
        ..InstrumentMetadata::default()
    };

    synth.register_symbol(
        "SHFE.rb2605",
        meta.clone(),
        QuoteSelection::Kline {
            duration_nanos: 300_000_000_000,
            implicit: false,
        },
    );
    synth.register_symbol(
        "SHFE.rb2605",
        meta,
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );

    assert_eq!(
        synth.selection_for("SHFE.rb2605"),
        Some(QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        })
    );
}

#[test]
fn quote_synthesizer_promotes_tick_when_tick_event_arrives_after_kline_registration() {
    let mut synth = QuoteSynthesizer::default();
    synth.register_symbol(
        "SHFE.rb2605",
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            price_tick: 1.0,
            ..InstrumentMetadata::default()
        },
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );

    let update = synth.apply_tick(
        "SHFE.rb2605",
        &Tick {
            datetime: 2_000,
            last_price: 12.0,
            ask_price1: 13.0,
            ask_volume1: 1,
            bid_price1: 11.0,
            bid_volume1: 1,
            ..Tick::default()
        },
    );

    assert!(update.source_selected);
    assert_eq!(update.visible.last_price, 12.0);
    assert_eq!(synth.selection_for("SHFE.rb2605"), Some(QuoteSelection::Tick));
}

#[test]
fn quote_synthesizer_promotes_smaller_kline_when_bar_event_arrives() {
    let mut synth = QuoteSynthesizer::default();
    let meta = InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        price_tick: 1.0,
        ..InstrumentMetadata::default()
    };

    synth.register_symbol(
        "SHFE.rb2605",
        meta.clone(),
        QuoteSelection::Kline {
            duration_nanos: 300_000_000_000,
            implicit: false,
        },
    );

    let update = synth.apply_bar_close(
        "SHFE.rb2605",
        60_000_000_000,
        &Kline {
            datetime: 1_000,
            open: 10.0,
            high: 14.0,
            low: 9.0,
            close: 12.0,
            open_oi: 100,
            close_oi: 110,
            volume: 7,
            id: 1,
            epoch: None,
        },
        60_000_000_999,
    );

    assert!(update.source_selected);
    assert_eq!(
        synth.selection_for("SHFE.rb2605"),
        Some(QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        })
    );
}

#[test]
fn quote_synthesizer_keeps_non_selected_bar_close_distinguishable_from_driver_path() {
    let mut synth = QuoteSynthesizer::default();
    let meta = InstrumentMetadata {
        symbol: "SHFE.rb2605".to_string(),
        price_tick: 1.0,
        ..InstrumentMetadata::default()
    };

    synth.register_symbol(
        "SHFE.rb2605",
        meta.clone(),
        QuoteSelection::Kline {
            duration_nanos: 60_000_000_000,
            implicit: false,
        },
    );

    let _ = synth.apply_bar_close(
        "SHFE.rb2605",
        60_000_000_000,
        &Kline {
            datetime: 1_000,
            open: 10.0,
            high: 14.0,
            low: 9.0,
            close: 12.0,
            open_oi: 100,
            close_oi: 110,
            volume: 7,
            id: 1,
            epoch: None,
        },
        60_000_000_999,
    );

    let update = synth.apply_bar_close(
        "SHFE.rb2605",
        300_000_000_000,
        &Kline {
            datetime: 1_000,
            open: 20.0,
            high: 24.0,
            low: 19.0,
            close: 22.0,
            open_oi: 200,
            close_oi: 210,
            volume: 9,
            id: 2,
            epoch: None,
        },
        300_000_000_999,
    );

    assert!(!update.source_selected);
    assert_eq!(update.visible.last_price, 12.0);
    assert!(update.path.is_empty());
}

#[tokio::test]
async fn replay_kernel_commits_bar_open_and_close_as_two_steps() {
    let bars = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 13.0,
        low: 9.0,
        close: 12.0,
        open_oi: 100,
        close_oi: 110,
        volume: 8,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![(
        "kline:SHFE.rb2605:60000000000".to_string(),
        FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars),
    )]);

    let first = kernel.step().await.unwrap().unwrap();
    assert_eq!(first.current_dt, DateTime::<Utc>::from_timestamp_nanos(1_000));

    let opening = kernel
        .series_store()
        .kline_rows("SHFE.rb2605", 60_000_000_000)
        .last()
        .unwrap()
        .clone();
    assert_eq!(opening.state, BarState::Opening);

    let second = kernel.step().await.unwrap().unwrap();
    assert_eq!(second.current_dt, DateTime::<Utc>::from_timestamp_nanos(60_000_000_999));

    let rows = kernel.series_store().kline_rows("SHFE.rb2605", 60_000_000_000);
    assert_eq!(rows.len(), 1);
    let closed = rows.last().unwrap().clone();
    assert_eq!(closed.state, BarState::Closed);
}

#[tokio::test]
async fn replay_kernel_marks_single_symbol_kline_handle_as_updated() {
    let bars = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 13.0,
        low: 9.0,
        close: 12.0,
        open_oi: 100,
        close_oi: 110,
        volume: 8,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![(
        "kline:SHFE.rb2605:60000000000".to_string(),
        FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars),
    )]);
    let handle = kernel.register_kline(
        "SHFE.rb2605",
        60_000_000_000,
        16,
        InstrumentMetadata {
            symbol: "SHFE.rb2605".to_string(),
            ..InstrumentMetadata::default()
        },
    );

    let step = kernel.step().await.unwrap().unwrap();
    assert!(step.updated_handles.contains(handle.id()));

    let rows = handle.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, BarState::Opening);
}

#[tokio::test]
async fn replay_kernel_marks_quote_driving_bar_events_as_updated_quotes() {
    let bars = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 13.0,
        low: 9.0,
        close: 12.0,
        open_oi: 100,
        close_oi: 110,
        volume: 8,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![(
        "kline:SHFE.rb2605:60000000000".to_string(),
        FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, bars),
    )]);

    let step = kernel.step().await.unwrap().unwrap();
    assert_eq!(step.updated_quotes, vec!["SHFE.rb2605".to_string()]);
}

#[tokio::test]
async fn replay_kernel_retains_tick_rows_in_fixed_width_window() {
    let ticks = vec![
        Tick {
            id: 1,
            datetime: 1_000,
            last_price: 10.0,
            average: 10.0,
            highest: 10.0,
            lowest: 10.0,
            ask_price1: 10.1,
            ask_volume1: 1,
            bid_price1: 9.9,
            bid_volume1: 1,
            volume: 1,
            amount: 10.0,
            open_interest: 100,
            ..Tick::default()
        },
        Tick {
            id: 2,
            datetime: 2_000,
            last_price: 11.0,
            average: 11.0,
            highest: 11.0,
            lowest: 11.0,
            ask_price1: 11.1,
            ask_volume1: 1,
            bid_price1: 10.9,
            bid_volume1: 1,
            volume: 2,
            amount: 22.0,
            open_interest: 101,
            ..Tick::default()
        },
        Tick {
            id: 3,
            datetime: 3_000,
            last_price: 12.0,
            average: 12.0,
            highest: 12.0,
            lowest: 12.0,
            ask_price1: 12.1,
            ask_volume1: 1,
            bid_price1: 11.9,
            bid_volume1: 1,
            volume: 3,
            amount: 36.0,
            open_interest: 102,
            ..Tick::default()
        },
    ];

    let mut kernel = ReplayKernel::for_test(vec![(
        "tick:SHFE.rb2605".to_string(),
        FeedCursor::from_tick_rows("SHFE.rb2605", ticks),
    )]);
    kernel.set_tick_window_width(2);

    while kernel.step().await.unwrap().is_some() {}

    let rows = kernel.series_store().tick_rows("SHFE.rb2605");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].tick.id, 2);
    assert_eq!(rows[1].tick.id, 3);
}

#[tokio::test]
async fn replay_kernel_aligns_secondary_symbol_to_main_bar_time() {
    let main = vec![Kline {
        id: 1,
        datetime: 1_000,
        open: 10.0,
        high: 11.0,
        low: 9.0,
        close: 10.5,
        open_oi: 100,
        close_oi: 101,
        volume: 5,
        epoch: None,
    }];
    let secondary = vec![Kline {
        id: 8,
        datetime: 1_000,
        open: 20.0,
        high: 21.0,
        low: 19.0,
        close: 20.5,
        open_oi: 200,
        close_oi: 201,
        volume: 6,
        epoch: None,
    }];

    let mut kernel = ReplayKernel::for_test(vec![
        (
            "kline:SHFE.rb2605:60000000000".to_string(),
            FeedCursor::from_kline_rows("SHFE.rb2605", 60_000_000_000, main),
        ),
        (
            "kline:SHFE.hc2605:60000000000".to_string(),
            FeedCursor::from_kline_rows("SHFE.hc2605", 60_000_000_000, secondary),
        ),
    ]);
    let aligned = kernel.register_aligned_kline(&["SHFE.rb2605", "SHFE.hc2605"], 60_000_000_000, 32);

    let _ = kernel.step().await.unwrap().unwrap();
    let rows = aligned.rows().await;
    let last = rows.last().unwrap();

    assert_eq!(last.datetime_nanos, 1_000);
    assert!(last.bars["SHFE.rb2605"].is_some());
    assert!(last.bars["SHFE.hc2605"].is_some());
}

fn limit_buy(symbol: &str, price: f64, volume: i64) -> InsertOrderRequest {
    InsertOrderRequest {
        symbol: symbol.to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_LIMIT.to_string(),
        limit_price: price,
        volume,
    }
}

fn limit_order(symbol: &str, direction: &str, offset: &str, price: f64, volume: i64) -> InsertOrderRequest {
    InsertOrderRequest {
        symbol: symbol.to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: direction.to_string(),
        offset: offset.to_string(),
        price_type: PRICE_TYPE_LIMIT.to_string(),
        limit_price: price,
        volume,
    }
}

fn market_buy(symbol: &str, volume: i64) -> InsertOrderRequest {
    InsertOrderRequest {
        symbol: symbol.to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: DIRECTION_BUY.to_string(),
        offset: OFFSET_OPEN.to_string(),
        price_type: PRICE_TYPE_ANY.to_string(),
        limit_price: 0.0,
        volume,
    }
}

fn future_metadata(symbol: &str) -> InstrumentMetadata {
    let (exchange_id, instrument_id) = symbol.split_once('.').unwrap();
    InstrumentMetadata {
        symbol: symbol.to_string(),
        exchange_id: exchange_id.to_string(),
        instrument_id: instrument_id.to_string(),
        class: "FUTURE".to_string(),
        price_tick: 1.0,
        volume_multiple: 10,
        margin: 1000.0,
        commission: 2.0,
        ..InstrumentMetadata::default()
    }
}

fn future_metadata_with_trading_time(symbol: &str, trading_time: serde_json::Value) -> InstrumentMetadata {
    InstrumentMetadata {
        trading_time,
        ..future_metadata(symbol)
    }
}

fn option_metadata(symbol: &str, underlying_symbol: &str, option_class: &str, strike_price: f64) -> InstrumentMetadata {
    let (exchange_id, instrument_id) = symbol.split_once('.').unwrap();
    InstrumentMetadata {
        symbol: symbol.to_string(),
        exchange_id: exchange_id.to_string(),
        instrument_id: instrument_id.to_string(),
        class: "OPTION".to_string(),
        underlying_symbol: underlying_symbol.to_string(),
        option_class: option_class.to_string(),
        strike_price,
        trading_time: serde_json::json!({
            "day": [["09:00:00", "15:00:00"]],
            "night": []
        }),
        price_tick: 0.1,
        volume_multiple: 100,
        commission: 10.0,
        open_min_market_order_volume: 1,
        open_min_limit_order_volume: 1,
        ..InstrumentMetadata::default()
    }
}

fn future_quote(symbol: &str, datetime_nanos: i64, last_price: f64, ask_price1: f64, bid_price1: f64) -> ReplayQuote {
    ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos,
        last_price,
        ask_price1,
        ask_volume1: 1,
        bid_price1,
        bid_volume1: 1,
        ..ReplayQuote::default()
    }
}

fn option_quote(symbol: &str, datetime_nanos: i64, last_price: f64, ask_price1: f64, bid_price1: f64) -> ReplayQuote {
    ReplayQuote {
        symbol: symbol.to_string(),
        datetime_nanos,
        last_price,
        ask_price1,
        ask_volume1: 10,
        bid_price1,
        bid_volume1: 10,
        highest: last_price,
        lowest: last_price,
        average: last_price,
        volume: 10,
        amount: last_price * 1000.0,
        open_interest: 100,
        open_limit: 0,
    }
}

fn shanghai_nanos(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
        .with_timezone(&Utc)
        .timestamp_nanos_opt()
        .unwrap()
}

fn assert_close(actual: f64, expected: f64) {
    let diff = (actual - expected).abs();
    assert!(diff < 1e-6, "expected {expected}, got {actual}, abs diff {diff}");
}

#[test]
fn sim_broker_fills_limit_buy_when_price_path_crosses() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata("SHFE.rb2605"));
    let order_id = broker
        .insert_order("TQSIM", &limit_buy("SHFE.rb2605", 11.0, 2))
        .unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[
                ReplayQuote {
                    symbol: "SHFE.rb2605".to_string(),
                    datetime_nanos: 1_000,
                    last_price: 10.0,
                    ask_price1: 11.0,
                    ask_volume1: 1,
                    bid_price1: 9.0,
                    bid_volume1: 1,
                    ..ReplayQuote::default()
                },
                ReplayQuote {
                    symbol: "SHFE.rb2605".to_string(),
                    datetime_nanos: 2_000,
                    last_price: 12.0,
                    ask_price1: 13.0,
                    ask_volume1: 1,
                    bid_price1: 11.0,
                    bid_volume1: 1,
                    ..ReplayQuote::default()
                },
            ],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(order.volume_left, 0);
}

#[test]
fn sim_broker_cancels_market_order_without_opponent_quote() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata("SHFE.rb2605"));
    let order_id = broker.insert_order("TQSIM", &market_buy("SHFE.rb2605", 1)).unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[ReplayQuote {
                symbol: "SHFE.rb2605".to_string(),
                datetime_nanos: 1_000,
                last_price: 10.0,
                ask_price1: f64::NAN,
                ask_volume1: 0,
                bid_price1: 9.0,
                bid_volume1: 1,
                ..ReplayQuote::default()
            }],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(order.volume_left, 1);
    assert_eq!(order.status, "FINISHED");
}

#[test]
fn sim_broker_records_daily_settlement() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata("SHFE.rb2605"));
    let settlement = broker
        .settle_day(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
        .unwrap()
        .unwrap();

    let DailySettlementLog { trading_day, .. } = settlement;
    assert_eq!(trading_day, NaiveDate::from_ymd_opt(2026, 4, 9).unwrap());
}

#[test]
fn sim_broker_limit_fill_uses_order_price_and_updates_position_once() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata("SHFE.rb2605"));
    let order_id = broker
        .insert_order("TQSIM", &limit_buy("SHFE.rb2605", 11.0, 2))
        .unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[
                ReplayQuote {
                    symbol: "SHFE.rb2605".to_string(),
                    datetime_nanos: 1_000,
                    last_price: 10.0,
                    ask_price1: 10.5,
                    ask_volume1: 3,
                    bid_price1: 9.5,
                    bid_volume1: 1,
                    ..ReplayQuote::default()
                },
                ReplayQuote {
                    symbol: "SHFE.rb2605".to_string(),
                    datetime_nanos: 2_000,
                    last_price: 9.0,
                    ask_price1: 9.5,
                    ask_volume1: 3,
                    bid_price1: 8.5,
                    bid_volume1: 1,
                    ..ReplayQuote::default()
                },
            ],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(order.status, "FINISHED");
    assert_eq!(order.volume_left, 0);

    let trades = broker.trades_by_order("TQSIM", &order_id).unwrap();
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, 11.0);
    assert_eq!(trades[0].volume, 2);

    let position = broker.position("TQSIM", "SHFE.rb2605").unwrap();
    assert_eq!(position.volume_long_today, 2);
    assert_eq!(position.volume_long, 2);

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[ReplayQuote {
                symbol: "SHFE.rb2605".to_string(),
                datetime_nanos: 3_000,
                last_price: 8.0,
                ask_price1: 8.5,
                ask_volume1: 5,
                bid_price1: 7.5,
                bid_volume1: 1,
                ..ReplayQuote::default()
            }],
        )
        .unwrap();

    let trades = broker.trades_by_order("TQSIM", &order_id).unwrap();
    assert_eq!(trades.len(), 1);
}

#[test]
fn sim_broker_finish_returns_accumulated_settlements_and_final_state() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata("SHFE.rb2605"));
    let order_id = broker
        .insert_order("TQSIM", &limit_buy("SHFE.rb2605", 11.0, 2))
        .unwrap();

    broker
        .apply_quote_path(
            "SHFE.rb2605",
            &[ReplayQuote {
                symbol: "SHFE.rb2605".to_string(),
                datetime_nanos: 1_000,
                last_price: 10.0,
                ask_price1: 10.5,
                ask_volume1: 3,
                bid_price1: 9.5,
                bid_volume1: 1,
                ..ReplayQuote::default()
            }],
        )
        .unwrap();

    let first_day = NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
    let second_day = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();

    let first = broker.settle_day(first_day).unwrap().unwrap();
    assert_eq!(first.trading_day, first_day);

    let second = broker.settle_day(second_day).unwrap().unwrap();
    assert_eq!(second.trading_day, second_day);
    assert!(second.trades.is_empty());

    let result = broker.finish().unwrap();
    assert_eq!(result.settlements.len(), 2);
    assert_eq!(result.settlements[0].trading_day, first_day);
    assert_eq!(result.settlements[1].trading_day, second_day);
    assert_eq!(result.final_accounts.len(), 1);
    assert_eq!(result.final_accounts[0].user_id, "TQSIM");
    assert_eq!(result.final_positions.len(), 1);
    assert_eq!(result.final_positions[0].user_id, "TQSIM");
    assert_eq!(result.final_positions[0].volume_long, 2);
    assert_eq!(result.trades.len(), 1);
    assert_eq!(result.trades[0].order_id, order_id);
}

#[test]
fn sim_broker_marks_futures_account_to_market_and_charges_commission() {
    let symbol = "SHFE.rb2605";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(symbol));

    broker
        .apply_quote_path(symbol, &[future_quote(symbol, 1_000, 10.0, 10.5, 9.5)])
        .unwrap();
    let order_id = broker.insert_order("TQSIM", &limit_buy(symbol, 11.0, 2)).unwrap();
    broker
        .apply_quote_path(
            symbol,
            &[
                future_quote(symbol, 1_000, 10.0, 10.5, 9.5),
                future_quote(symbol, 2_000, 12.0, 12.5, 11.5),
            ],
        )
        .unwrap();

    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];
    let position = broker.position("TQSIM", symbol).unwrap();
    let order = broker.order("TQSIM", &order_id).unwrap();

    assert_eq!(order.status, "FINISHED");
    assert_eq!(order.volume_left, 0);
    assert_eq!(result.trades.len(), 1);
    assert_close(result.trades[0].commission, 4.0);
    assert_close(account.commission, 4.0);
    assert_close(account.margin, 2_000.0);
    assert_close(account.float_profit, 20.0);
    assert_close(account.position_profit, 20.0);
    assert_close(account.balance, 10_000_016.0);
    assert_close(account.available, 9_998_016.0);
    assert_close(account.risk_ratio, 2_000.0 / 10_000_016.0);
    assert_eq!(position.volume_long_today, 2);
    assert_eq!(position.volume_long_his, 0);
    assert_eq!(position.volume_long, 2);
    assert_close(position.open_price_long, 11.0);
    assert_close(position.position_price_long, 11.0);
    assert_close(position.margin, 2_000.0);
    assert_close(position.float_profit, 20.0);
    assert_close(position.position_profit, 20.0);
    assert_close(position.last_price, 12.0);
}

#[test]
fn sim_broker_marks_long_option_with_premium_and_market_value() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9)],
        )
        .unwrap();

    let order_id = broker.insert_order("TQSIM", &limit_buy(option, 2.0, 1)).unwrap();
    broker
        .apply_quote_path(
            option,
            &[
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9),
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 2, 0), 2.5, 2.6, 2.5),
            ],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    let position = broker.position("TQSIM", option).unwrap();
    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];

    assert_eq!(order.status, "FINISHED");
    assert_eq!(order.volume_left, 0);
    assert_eq!(result.trades.len(), 1);
    assert_close(result.trades[0].commission, 10.0);
    assert_close(account.commission, 10.0);
    assert_close(account.premium, -200.0);
    assert_close(account.margin, 0.0);
    assert_close(account.market_value, 250.0);
    assert_close(account.float_profit, 50.0);
    assert_close(account.position_profit, 0.0);
    assert_close(account.balance, 10_000_040.0);
    assert_close(account.available, 9_999_790.0);
    assert_eq!(position.volume_long, 1);
    assert_eq!(position.volume_short, 0);
    assert_close(position.market_value_long, 250.0);
    assert_close(position.market_value, 250.0);
    assert_close(position.float_profit, 50.0);
    assert_close(position.position_profit, 0.0);
    assert_close(position.margin, 0.0);
}

#[test]
fn sim_broker_reprices_short_option_margin_when_underlying_moves() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.1, 2.0)],
        )
        .unwrap();

    let order_id = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_SELL, OFFSET_OPEN, 2.0, 1))
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.1, 2.0)],
        )
        .unwrap();

    let filled = broker.order("TQSIM", &order_id).unwrap();
    assert_eq!(filled.status, "FINISHED");

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 2, 0),
                105.0,
                105.5,
                104.5,
            )],
        )
        .unwrap();

    let position = broker.position("TQSIM", option).unwrap();
    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];

    assert_close(account.commission, 10.0);
    assert_close(account.premium, 200.0);
    assert_close(account.market_value, -200.0);
    assert_close(account.position_profit, 0.0);
    assert_close(account.margin, 1_460.0);
    assert_close(account.balance, 9_999_990.0);
    assert_close(account.available, 9_998_730.0);
    assert_eq!(position.volume_short, 1);
    assert_close(position.market_value_short, -200.0);
    assert_close(position.market_value, -200.0);
    assert_close(position.margin_short, 1_460.0);
    assert_close(position.margin, 1_460.0);
    assert_close(position.float_profit, 0.0);
    assert_close(position.position_profit, 0.0);
}

#[test]
fn sim_broker_option_close_realizes_pnl_through_premium_not_close_profit() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9)],
        )
        .unwrap();

    let open_order = broker.insert_order("TQSIM", &limit_buy(option, 2.0, 1)).unwrap();
    broker
        .apply_quote_path(
            option,
            &[
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9),
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 2, 0), 2.5, 2.6, 2.5),
            ],
        )
        .unwrap();
    assert_eq!(broker.order("TQSIM", &open_order).unwrap().status, "FINISHED");

    let close_order = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_SELL, OFFSET_CLOSE, 2.5, 1))
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 2, 0), 2.5, 2.6, 2.5)],
        )
        .unwrap();

    let close = broker.order("TQSIM", &close_order).unwrap();
    let position = broker.position("TQSIM", option).unwrap();
    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];

    assert_eq!(close.status, "FINISHED");
    assert_eq!(result.trades.len(), 2);
    assert_close(account.close_profit, 0.0);
    assert_close(account.premium, 50.0);
    assert_close(account.market_value, 0.0);
    assert_close(account.float_profit, 0.0);
    assert_close(account.position_profit, 0.0);
    assert_close(account.commission, 20.0);
    assert_close(account.balance, 10_000_030.0);
    assert_close(account.available, 10_000_030.0);
    assert_eq!(position.volume_long, 0);
    assert_close(position.market_value, 0.0);
    assert_close(position.float_profit, 0.0);
    assert_close(position.position_profit, 0.0);
}

#[test]
fn sim_broker_short_put_margin_matches_python_formula() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605P100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "PUT", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                95.0,
                95.5,
                94.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 3.0, 3.1, 3.0)],
        )
        .unwrap();

    let order_id = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_SELL, OFFSET_OPEN, 3.0, 1))
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 3.0, 3.1, 3.0)],
        )
        .unwrap();

    let order = broker.order("TQSIM", &order_id).unwrap();
    let position = broker.position("TQSIM", option).unwrap();
    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];

    assert_eq!(order.status, "FINISHED");
    assert_close(account.margin, 1_440.0);
    assert_close(account.market_value, -300.0);
    assert_close(account.premium, 300.0);
    assert_close(account.balance, 9_999_990.0);
    assert_close(account.available, 9_998_850.0);
    assert_close(position.margin_short, 1_440.0);
    assert_close(position.market_value_short, -300.0);
}

#[test]
fn sim_broker_option_settlement_rolls_market_value_into_pre_balance() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9)],
        )
        .unwrap();

    let open_order = broker.insert_order("TQSIM", &limit_buy(option, 2.0, 1)).unwrap();
    broker
        .apply_quote_path(
            option,
            &[
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.0, 1.9),
                option_quote(option, shanghai_nanos(2026, 4, 9, 9, 2, 0), 2.5, 2.6, 2.5),
            ],
        )
        .unwrap();
    assert_eq!(broker.order("TQSIM", &open_order).unwrap().status, "FINISHED");

    let settlement = broker
        .settle_day(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
        .unwrap()
        .unwrap();
    assert_close(settlement.account.balance, 10_000_040.0);
    assert_close(settlement.account.market_value, 250.0);
    assert_close(settlement.account.pre_balance, 10_000_000.0);
    assert_close(settlement.account.premium, -200.0);

    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];
    let position = broker.position("TQSIM", option).unwrap();

    assert_close(account.pre_balance, 9_999_790.0);
    assert_close(account.static_balance, 9_999_790.0);
    assert_close(account.balance, 10_000_040.0);
    assert_close(account.available, 9_999_790.0);
    assert_close(account.market_value, 250.0);
    assert_close(account.premium, 0.0);
    assert_close(account.commission, 0.0);
    assert_close(position.position_price_long, 2.5);
    assert_close(position.position_cost_long, 250.0);
    assert_close(position.market_value_long, 250.0);
    assert_close(position.position_profit, 0.0);
}

#[test]
fn sim_broker_short_option_settlement_rolls_negative_market_value_into_pre_balance() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.1, 2.0)],
        )
        .unwrap();

    let open_order = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_SELL, OFFSET_OPEN, 2.0, 1))
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.1, 2.0)],
        )
        .unwrap();
    assert_eq!(broker.order("TQSIM", &open_order).unwrap().status, "FINISHED");

    let settlement = broker
        .settle_day(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
        .unwrap()
        .unwrap();
    assert_close(settlement.account.balance, 9_999_990.0);
    assert_close(settlement.account.market_value, -200.0);
    assert_close(settlement.account.pre_balance, 10_000_000.0);
    assert_close(settlement.account.premium, 200.0);

    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];
    let position = broker.position("TQSIM", option).unwrap();

    assert_close(account.pre_balance, 10_000_190.0);
    assert_close(account.static_balance, 10_000_190.0);
    assert_close(account.balance, 9_999_990.0);
    assert_close(account.available, 9_998_790.0);
    assert_close(account.market_value, -200.0);
    assert_close(account.margin, 1_400.0);
    assert_close(account.premium, 0.0);
    assert_close(account.commission, 0.0);
    assert_close(position.position_price_short, 2.0);
    assert_close(position.position_cost_short, 200.0);
    assert_close(position.market_value_short, -200.0);
    assert_close(position.margin_short, 1_400.0);
    assert_close(position.position_profit, 0.0);
}

#[test]
fn sim_broker_freezes_and_releases_long_option_premium_on_open_order() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.5, 1.9)],
        )
        .unwrap();

    let order_id = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_BUY, OFFSET_OPEN, 2.0, 1))
        .unwrap();
    let order = broker.order("TQSIM", &order_id).unwrap();
    let account = &broker.finish().unwrap().final_accounts[0];

    assert_eq!(order.status, "ALIVE");
    assert_close(order.frozen_margin, 0.0);
    assert_close(order.frozen_premium, 200.0);
    assert_close(account.frozen_margin, 0.0);
    assert_close(account.frozen_premium, 200.0);
    assert_close(account.available, 9_999_800.0);

    broker.cancel_order("TQSIM", &order_id).unwrap();
    let cancelled = broker.order("TQSIM", &order_id).unwrap();
    let account = &broker.finish().unwrap().final_accounts[0];
    assert_eq!(cancelled.status, "FINISHED");
    assert_close(cancelled.frozen_premium, 0.0);
    assert_close(account.frozen_premium, 0.0);
    assert_close(account.available, 10_000_000.0);
}

#[test]
fn sim_broker_freezes_and_releases_short_option_margin_on_open_order() {
    let underlying = "DCE.m2605";
    let option = "DCE.m2605C100";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(underlying));
    broker.register_symbol(option_metadata(option, underlying, "CALL", 100.0));

    broker
        .apply_quote_path(
            underlying,
            &[future_quote(
                underlying,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                100.0,
                100.5,
                99.5,
            )],
        )
        .unwrap();
    broker
        .apply_quote_path(
            option,
            &[option_quote(option, shanghai_nanos(2026, 4, 9, 9, 1, 0), 2.0, 2.5, 1.9)],
        )
        .unwrap();

    let order_id = broker
        .insert_order("TQSIM", &limit_order(option, DIRECTION_SELL, OFFSET_OPEN, 2.0, 1))
        .unwrap();
    let order = broker.order("TQSIM", &order_id).unwrap();
    let account = &broker.finish().unwrap().final_accounts[0];

    assert_eq!(order.status, "ALIVE");
    assert_close(order.frozen_margin, 1_400.0);
    assert_close(order.frozen_premium, 0.0);
    assert_close(account.frozen_margin, 1_400.0);
    assert_close(account.frozen_premium, 0.0);
    assert_close(account.available, 9_998_600.0);

    broker.cancel_order("TQSIM", &order_id).unwrap();
    let cancelled = broker.order("TQSIM", &order_id).unwrap();
    let account = &broker.finish().unwrap().final_accounts[0];
    assert_eq!(cancelled.status, "FINISHED");
    assert_close(cancelled.frozen_margin, 0.0);
    assert_close(account.frozen_margin, 0.0);
    assert_close(account.available, 10_000_000.0);
}

#[test]
fn sim_broker_rejects_order_outside_trading_time() {
    let symbol = "SHFE.rb2605";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata_with_trading_time(
        symbol,
        serde_json::json!({
            "day": [["09:00:00", "15:00:00"]],
            "night": []
        }),
    ));

    broker
        .apply_quote_path(
            symbol,
            &[future_quote(
                symbol,
                shanghai_nanos(2026, 4, 9, 8, 59, 0),
                10.0,
                10.5,
                9.5,
            )],
        )
        .unwrap();
    let order_id = broker.insert_order("TQSIM", &limit_buy(symbol, 11.0, 2)).unwrap();
    let order = broker.order("TQSIM", &order_id).unwrap();

    assert_eq!(order.status, "FINISHED");
    assert_eq!(order.last_msg, "下单失败, 不在可交易时间段内");
}

#[test]
fn sim_broker_accepts_order_inside_trading_time() {
    let symbol = "SHFE.rb2605";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata_with_trading_time(
        symbol,
        serde_json::json!({
            "day": [["09:00:00", "15:00:00"]],
            "night": []
        }),
    ));

    broker
        .apply_quote_path(
            symbol,
            &[future_quote(
                symbol,
                shanghai_nanos(2026, 4, 9, 9, 1, 0),
                10.0,
                10.5,
                9.5,
            )],
        )
        .unwrap();
    let order_id = broker.insert_order("TQSIM", &limit_buy(symbol, 11.0, 2)).unwrap();
    let order = broker.order("TQSIM", &order_id).unwrap();

    assert_eq!(order.status, "ALIVE");
    assert_eq!(order.last_msg, "报单成功");
}

#[test]
fn sim_broker_settlement_rolls_positions_and_resets_daily_account_fields() {
    let symbol = "SHFE.rb2605";
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
    broker.register_symbol(future_metadata(symbol));

    broker
        .apply_quote_path(symbol, &[future_quote(symbol, 1_000, 10.0, 10.5, 9.5)])
        .unwrap();
    let _order_id = broker.insert_order("TQSIM", &limit_buy(symbol, 11.0, 2)).unwrap();
    broker
        .apply_quote_path(
            symbol,
            &[
                future_quote(symbol, 1_000, 10.0, 10.5, 9.5),
                future_quote(symbol, 2_000, 12.0, 12.5, 11.5),
            ],
        )
        .unwrap();

    let settlement = broker
        .settle_day(NaiveDate::from_ymd_opt(2026, 4, 9).unwrap())
        .unwrap()
        .unwrap();
    assert_close(settlement.account.balance, 10_000_016.0);
    assert_close(settlement.account.available, 9_998_016.0);
    assert_close(settlement.account.position_profit, 20.0);

    let result = broker.finish().unwrap();
    let account = &result.final_accounts[0];
    let position = broker.position("TQSIM", symbol).unwrap();

    assert_close(account.pre_balance, 10_000_016.0);
    assert_close(account.static_balance, 10_000_016.0);
    assert_close(account.balance, 10_000_016.0);
    assert_close(account.available, 9_998_016.0);
    assert_close(account.margin, 2_000.0);
    assert_close(account.commission, 0.0);
    assert_close(account.close_profit, 0.0);
    assert_close(account.position_profit, 0.0);
    assert_eq!(position.volume_long_today, 0);
    assert_eq!(position.volume_long_his, 2);
    assert_eq!(position.volume_long, 2);
    assert_close(position.position_price_long, 12.0);
    assert_close(position.position_cost_long, 240.0);
    assert_close(position.position_profit, 0.0);
}

#[tokio::test]
async fn replay_session_auto_settles_trading_days_and_reports_step_boundary() {
    let symbol = "SHFE.rb2605";
    let source = Arc::new(FakeHistoricalSource {
        meta: HashMap::from([(symbol.to_string(), future_metadata(symbol))]),
        klines: HashMap::from([(
            (symbol.to_string(), 86_400_000_000_000),
            vec![
                Kline {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 1, 0, 0, 0),
                    open: 10.0,
                    high: 12.0,
                    low: 9.0,
                    close: 11.0,
                    open_oi: 100,
                    close_oi: 110,
                    volume: 5,
                    epoch: None,
                },
                Kline {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 2, 0, 0, 0),
                    open: 11.0,
                    high: 13.0,
                    low: 10.0,
                    close: 12.0,
                    open_oi: 110,
                    close_oi: 120,
                    volume: 6,
                    epoch: None,
                },
            ],
        )]),
        ticks: HashMap::new(),
    });

    let start_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 4, 2, 23, 59, 59)
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut session = ReplaySession::from_source(ReplayConfig::new(start_dt, end_dt).unwrap(), source)
        .await
        .unwrap();
    let _quote = session.quote(symbol).await.unwrap();
    let _daily = session.kline(symbol, Duration::from_secs(86_400), 8).await.unwrap();
    let _runtime = session.runtime(["TQSIM"]).await.unwrap();

    let mut settled_days = Vec::new();
    while let Some(step) = session.step().await.unwrap() {
        if let Some(day) = step.settled_trading_day {
            settled_days.push(day);
        }
    }

    let result = session.finish().await.unwrap();
    assert_eq!(settled_days, vec![NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()]);
    assert_eq!(result.settlements.len(), 2);
    assert_eq!(
        result.settlements[0].trading_day,
        NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
    );
    assert_eq!(
        result.settlements[1].trading_day,
        NaiveDate::from_ymd_opt(2026, 4, 2).unwrap()
    );
}
