use super::Quote;

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
