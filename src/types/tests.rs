use super::{InsertOrderRequest, Quote, Tick};

#[test]
fn test_quote_deserialize_with_nulls() {
    let json_data = r#"{
        "instrument_id":"DCE.m2512",
        "datetime":"2025-11-24 22:59:59.000001",
        "ask_price1":3005.0,
        "ask_volume1":1,
        "ask_price2":null,
        "ask_volume2":null,
        "bid_price1":2995.0,
        "bid_volume1":2,
        "bid_price2":null,
        "bid_volume2":null,
        "last_price":3000.0,
        "highest":3000.0,
        "lowest":2986.0,
        "open":2998.0,
        "close":"-",
        "average":2995.0,
        "volume":688,
        "amount":20609740.0,
        "open_interest":5278,
        "settlement":"-",
        "upper_limit":3181.0,
        "lower_limit":2821.0,
        "pre_open_interest":5729,
        "pre_settlement":3001.0,
        "pre_close":3001.0
    }"#;

    let result = serde_json::from_str::<Quote>(json_data);
    assert!(result.is_ok(), "Quote 解析失败: {:?}", result.err());

    let quote = result.unwrap();
    assert_eq!(quote.instrument_id, "DCE.m2512");
    assert_eq!(quote.last_price, 3000.0);
    assert_eq!(quote.ask_price1, 3005.0);
    assert!(quote.ask_price2.is_nan(), "null 应该被转换为 NaN");
    assert_eq!(quote.ask_volume2, 0, "null 应该被转换为 0");
    assert!(quote.close.is_nan(), "dash 应该被转换为 NaN");
    assert!(quote.settlement.is_nan(), "dash 应该被转换为 NaN");
}

#[test]
fn quote_parses_open_min_order_volume_fields() {
    let json_data = r#"{
        "instrument_id":"SHFE.au2602",
        "datetime":"2025-11-24 09:00:00.000000",
        "open_min_market_order_volume":3,
        "open_min_limit_order_volume":5,
        "open_max_market_order_volume":9,
        "open_max_limit_order_volume":11
    }"#;

    let quote = serde_json::from_str::<Quote>(json_data).expect("Quote 解析失败");

    assert_eq!(quote.open_min_market_order_volume, 3);
    assert_eq!(quote.open_min_limit_order_volume, 5);
    assert_eq!(quote.open_max_market_order_volume, 9);
    assert_eq!(quote.open_max_limit_order_volume, 11);
}

#[test]
fn quote_parses_open_limit_field() {
    let json_data = r#"{
        "instrument_id":"SHFE.au2602",
        "datetime":"2025-11-24 09:00:00.000000",
        "open_limit":123
    }"#;

    let quote = serde_json::from_str::<Quote>(json_data).expect("Quote 解析失败");
    let encoded = serde_json::to_value(&quote).expect("Quote 序列化失败");

    assert_eq!(encoded.get("open_limit").and_then(|value| value.as_i64()), Some(123));
}

#[test]
fn quote_open_limit_defaults_to_zero_when_missing() {
    let json_data = r#"{
        "instrument_id":"SHFE.au2602",
        "datetime":"2025-11-24 09:00:00.000000"
    }"#;

    let quote = serde_json::from_str::<Quote>(json_data).expect("Quote 解析失败");
    let encoded = serde_json::to_value(&quote).expect("Quote 序列化失败");

    assert_eq!(encoded.get("open_limit").and_then(|value| value.as_i64()), Some(0));
}

#[test]
fn tick_deserialize_with_sparse_depth_fields() {
    let json_data = r#"{
        "datetime": 1774702800000000000,
        "last_price": 3000.0,
        "average": 2995.0,
        "highest": 3001.0,
        "lowest": 2986.0,
        "ask_price1": 3000.2,
        "ask_volume1": 1,
        "ask_price2": null,
        "ask_volume2": null,
        "bid_price1": 2999.8,
        "bid_volume1": 2,
        "bid_price2": null,
        "bid_volume2": null,
        "volume": 688,
        "amount": 20609740.0,
        "open_interest": 5278
    }"#;

    let tick = serde_json::from_str::<Tick>(json_data).expect("Tick 解析失败");

    assert_eq!(tick.last_price, 3000.0);
    assert_eq!(tick.ask_volume1, 1);
    assert!(tick.ask_price2.is_nan(), "null 应该被转换为 NaN");
    assert_eq!(tick.ask_volume2, 0, "null 应该被转换为 0");
    assert!(tick.ask_price3.is_nan(), "缺失字段应该被转换为 NaN");
    assert_eq!(tick.ask_volume3, 0, "缺失字段应该被转换为 0");
    assert!(tick.bid_price2.is_nan(), "null 应该被转换为 NaN");
    assert_eq!(tick.bid_volume2, 0, "null 应该被转换为 0");
}

#[test]
fn test_insert_order_request_parses_symbol_when_fields_missing() {
    let req = InsertOrderRequest {
        symbol: "SHFE.au2602".to_string(),
        exchange_id: None,
        instrument_id: None,
        direction: "BUY".to_string(),
        offset: "OPEN".to_string(),
        price_type: "LIMIT".to_string(),
        limit_price: 500.0,
        volume: 1,
    };

    assert_eq!(req.get_exchange_id(), "SHFE");
    assert_eq!(req.get_instrument_id(), "au2602");
}

#[test]
fn test_insert_order_request_prefers_explicit_fields_over_symbol_parsing() {
    let req = InsertOrderRequest {
        symbol: "SHFE.au2602".to_string(),
        exchange_id: Some("DCE".to_string()),
        instrument_id: Some("m2505".to_string()),
        direction: "BUY".to_string(),
        offset: "OPEN".to_string(),
        price_type: "LIMIT".to_string(),
        limit_price: 500.0,
        volume: 1,
    };

    assert_eq!(req.get_exchange_id(), "DCE");
    assert_eq!(req.get_instrument_id(), "m2505");
}
