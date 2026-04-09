use chrono::{DateTime, Utc};

use crate::replay::{BarState, InstrumentMetadata, ReplayConfig};

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
