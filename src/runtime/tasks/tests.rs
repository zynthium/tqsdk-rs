use crate::runtime::{OffsetAction, PlannedOffset, compute_plan, parse_offset_priority, validate_quote_constraints};
use crate::types::{Position, Quote};
use serde_json::json;

#[test]
fn offset_priority_parser_accepts_python_compatible_forms() {
    assert_eq!(
        parse_offset_priority("今昨,开").expect("default form should parse"),
        vec![
            vec![OffsetAction::Today, OffsetAction::Yesterday],
            vec![OffsetAction::Open],
        ]
    );
    assert_eq!(
        parse_offset_priority("今昨开").expect("simultaneous form should parse"),
        vec![vec![OffsetAction::Today, OffsetAction::Yesterday, OffsetAction::Open,]]
    );
    assert_eq!(
        parse_offset_priority("昨开").expect("yesterday-then-open should parse"),
        vec![vec![OffsetAction::Yesterday, OffsetAction::Open]]
    );
    assert_eq!(
        parse_offset_priority("开").expect("open only should parse"),
        vec![vec![OffsetAction::Open]]
    );
}

#[test]
fn planning_rejects_contracts_with_open_min_volume_gt_one() {
    let quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        open_min_market_order_volume: 2,
        ..Quote::default()
    };

    let err = validate_quote_constraints(&quote).expect_err("open min volume > 1 should be rejected");
    assert!(format!("{err}").contains("最小市价开仓手数"));
}

#[test]
fn planning_respects_shfe_close_today_vs_close_yesterday() {
    let priorities = parse_offset_priority("今昨开").expect("priority should parse");
    let position: Position = serde_json::from_value(json!({
        "exchange_id": "SHFE",
        "instrument_id": "rb2601",
        "pos_long_today": 2,
        "pos_long_his": 3,
        "volume_long_today": 2,
        "volume_long_his": 3,
        "volume_long": 5
    }))
    .expect("position should deserialize");

    let shfe_quote = Quote {
        instrument_id: "SHFE.rb2601".to_string(),
        exchange_id: "SHFE".to_string(),
        ..Quote::default()
    };
    let shfe_plan = compute_plan(&shfe_quote, &position, -1, &priorities).expect("shfe plan should succeed");
    assert_eq!(shfe_plan.len(), 1);
    assert_eq!(shfe_plan[0].orders.len(), 3);
    assert_eq!(shfe_plan[0].orders[0].offset, PlannedOffset::CloseToday);
    assert_eq!(shfe_plan[0].orders[0].volume, 2);
    assert_eq!(shfe_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(shfe_plan[0].orders[1].volume, 3);
    assert_eq!(shfe_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(shfe_plan[0].orders[2].volume, 1);

    let cffex_quote = Quote {
        instrument_id: "CFFEX.IF2601".to_string(),
        exchange_id: "CFFEX".to_string(),
        ..Quote::default()
    };
    let cffex_plan = compute_plan(&cffex_quote, &position, -1, &priorities).expect("cffex plan should succeed");
    assert_eq!(cffex_plan.len(), 1);
    assert_eq!(cffex_plan[0].orders.len(), 3);
    assert_eq!(cffex_plan[0].orders[0].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[0].volume, 2);
    assert_eq!(cffex_plan[0].orders[1].offset, PlannedOffset::Close);
    assert_eq!(cffex_plan[0].orders[1].volume, 3);
    assert_eq!(cffex_plan[0].orders[2].offset, PlannedOffset::Open);
    assert_eq!(cffex_plan[0].orders[2].volume, 1);
}
