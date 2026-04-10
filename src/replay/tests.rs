use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::TargetPosTask;
use crate::types::{DIRECTION_BUY, InsertOrderRequest, Kline, OFFSET_OPEN, PRICE_TYPE_ANY, PRICE_TYPE_LIMIT, Tick};
use async_trait::async_trait;

use super::feed::{FeedCursor, FeedEvent, HistoricalSource};
use super::kernel::ReplayKernel;
use super::providers::{ContinuousContractProvider, ContinuousMapping};
use super::quote::{QuoteSelection, QuoteSynthesizer};
use super::series::SeriesStore;
use super::sim::SimBroker;
use super::{BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayQuote, ReplaySession};

struct FakeHistoricalSource {
    meta: HashMap<String, InstrumentMetadata>,
    klines: HashMap<(String, i64), Vec<Kline>>,
}

#[async_trait(?Send)]
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
        _symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Tick>> {
        Ok(Vec::new())
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
    let _klines = session
        .series()
        .kline("SHFE.rb2605", Duration::from_secs(60), 32)
        .await
        .unwrap();

    let runtime = session.runtime(["TQSIM"]).await.unwrap();
    let account = runtime.account("TQSIM").expect("configured account should exist");
    let task: TargetPosTask = account.target_pos("SHFE.rb2605").build().unwrap();

    task.set_target_volume(1).unwrap();

    while session.step().await.unwrap().is_some() {}

    let position = runtime.execution().position("TQSIM", "SHFE.rb2605").await.unwrap();
    assert_eq!(position.volume_long, 1);
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

#[test]
fn sim_broker_fills_limit_buy_when_price_path_crosses() {
    let mut broker = SimBroker::new(vec!["TQSIM".to_string()], 10_000_000.0);
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
