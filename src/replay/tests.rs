use chrono::{DateTime, NaiveDate, Utc};

use crate::replay::{
    BarState, ContinuousContractProvider, ContinuousMapping, FeedCursor, FeedEvent, InstrumentMetadata, ReplayConfig,
};
use crate::types::Kline;

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
