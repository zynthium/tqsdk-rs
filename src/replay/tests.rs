use chrono::{DateTime, NaiveDate, Utc};

use crate::replay::{
    BarState, ContinuousContractProvider, ContinuousMapping, FeedCursor, FeedEvent, InstrumentMetadata, ReplayConfig,
    ReplayKernel, SeriesStore,
};
use crate::types::{Kline, Tick};

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
    let handle = kernel.register_kline("SHFE.rb2605", 60_000_000_000, 16);

    let step = kernel.step().await.unwrap().unwrap();
    assert!(step.updated_handles.contains(handle.id()));

    let rows = handle.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, BarState::Opening);
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
