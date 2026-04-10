use super::parse::{
    BisectPriority, OptionNode, bisect_value_index, filter_option_nodes, parse_query_cont_quotes_result,
    parse_query_options_result, parse_query_quotes_result, parse_query_symbol_info_result,
    sort_options_and_get_atm_index,
};
use super::validation::{validate_finance_nearbys, validate_finance_underlying, validate_price_level};
use crate::auth::Authenticator;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::marketdata::MarketDataState;
use crate::websocket::WebSocketConfig;
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket};
use crate::{Client, ClientConfig};
use async_trait::async_trait;
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use reqwest::header::HeaderMap;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Duration;

#[test]
fn parse_quotes_filters_exchange() {
    let res = json!({
        "result": {
            "multi_symbol_info": [
                { "instrument_id": "SHFE.cu2405" },
                { "instrument_id": "DCE.m2405" }
            ]
        }
    });
    let all = parse_query_quotes_result(&res, None);
    assert_eq!(all, vec!["SHFE.cu2405", "DCE.m2405"]);
    let shfe = parse_query_quotes_result(&res, Some("SHFE"));
    assert_eq!(shfe, vec!["SHFE.cu2405"]);
}

#[test]
fn parse_cont_quotes_filters_exchange_product() {
    let res = json!({
        "result": {
            "multi_symbol_info": [
                {
                    "underlying": {
                        "edges": [
                            { "node": { "instrument_id": "SHFE.cu2405", "exchange_id": "SHFE", "product_id": "cu" } },
                            { "node": { "instrument_id": "DCE.m2405", "exchange_id": "DCE", "product_id": "m" } }
                        ]
                    }
                }
            ]
        }
    });
    let only_shfe = parse_query_cont_quotes_result(&res, Some("SHFE"), None);
    assert_eq!(only_shfe, vec!["SHFE.cu2405"]);
    let only_m = parse_query_cont_quotes_result(&res, None, Some("m"));
    assert_eq!(only_m, vec!["DCE.m2405"]);
}

#[test]
fn parse_options_filters_conditions() {
    let ts = Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap().timestamp() * 1_000_000_000;
    let res = json!({
        "result": {
            "multi_symbol_info": [
                {
                    "derivatives": {
                        "edges": [
                            {
                                "node": {
                                    "instrument_id": "SHFE.cu2405C3000",
                                    "english_name": "AAA",
                                    "call_or_put": "CALL",
                                    "strike_price": 3000.0,
                                    "expired": false,
                                    "last_exercise_datetime": ts
                                }
                            },
                            {
                                "node": {
                                    "instrument_id": "SHFE.cu2405P3100",
                                    "english_name": "BBB",
                                    "call_or_put": "PUT",
                                    "strike_price": 3100.0,
                                    "expired": true,
                                    "last_exercise_datetime": ts
                                }
                            }
                        ]
                    }
                }
            ]
        }
    });

    let calls = parse_query_options_result(&res, Some("CALL"), None, None, None, None, None);
    assert_eq!(calls, vec!["SHFE.cu2405C3000"]);

    let year_month = parse_query_options_result(&res, None, Some(2024), Some(12), None, None, None);
    assert_eq!(year_month.len(), 2);

    let strike = parse_query_options_result(&res, None, None, None, Some(3100.0), None, None);
    assert_eq!(strike, vec!["SHFE.cu2405P3100"]);

    let has_a = parse_query_options_result(&res, None, None, None, None, None, Some(true));
    assert_eq!(has_a, vec!["SHFE.cu2405C3000"]);
}

fn make_option(
    instrument_id: &str,
    strike_price: f64,
    call_or_put: &str,
    last_exercise_datetime: i64,
    english_name: &str,
    expired: bool,
) -> OptionNode {
    let dt = chrono::DateTime::<Utc>::from_timestamp(
        last_exercise_datetime / 1_000_000_000,
        (last_exercise_datetime % 1_000_000_000) as u32,
    )
    .unwrap_or_else(|| chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    OptionNode {
        instrument_id: instrument_id.to_string(),
        english_name: english_name.to_string(),
        call_or_put: call_or_put.to_string(),
        strike_price,
        expired,
        last_exercise_datetime,
        exercise_year: dt.year(),
        exercise_month: dt.month() as i32,
    }
}

#[test]
fn bisect_value_index_tie_priority() {
    let a = vec![100.0, 110.0];
    assert_eq!(a[bisect_value_index(&a, 105.0, BisectPriority::Right)], 110.0);
    assert_eq!(a[bisect_value_index(&a, 105.0, BisectPriority::Left)], 100.0);
}

#[test]
fn atm_equal_distance_rule_call_put() -> Result<()> {
    let ts = Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap().timestamp() * 1_000_000_000;

    let mut calls = vec![
        make_option("C90", 90.0, "CALL", ts, "", false),
        make_option("C110", 110.0, "CALL", ts, "", false),
    ];
    let idx = sort_options_and_get_atm_index(&mut calls, 100.0, "CALL")?;
    assert_eq!(calls[idx].instrument_id, "C110");

    let mut puts = vec![
        make_option("P90", 90.0, "PUT", ts, "", false),
        make_option("P110", 110.0, "PUT", ts, "", false),
    ];
    let idx = sort_options_and_get_atm_index(&mut puts, 100.0, "PUT")?;
    assert_eq!(puts[idx].instrument_id, "P90");
    Ok(())
}

#[test]
fn price_level_validation_rejects_out_of_range() {
    let err = validate_price_level(&[-101]).unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[test]
fn finance_underlying_validation() {
    let err = validate_finance_underlying("SHFE.au2405").unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[test]
fn finance_nearbys_validation_matches_python_ranges() {
    validate_finance_nearbys("SSE.000300", &[0, 5]).unwrap();
    let err = validate_finance_nearbys("SSE.000300", &[6]).unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));

    validate_finance_nearbys("SSE.510300", &[0, 3]).unwrap();
    let err = validate_finance_nearbys("SSE.510300", &[4]).unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[test]
fn filter_nearbys_keeps_only_selected_expiries() {
    let ts1 = Utc.with_ymd_and_hms(2024, 11, 1, 0, 0, 0).unwrap().timestamp() * 1_000_000_000;
    let ts2 = Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap().timestamp() * 1_000_000_000;

    let opts = vec![
        make_option("A1", 100.0, "CALL", ts1, "", false),
        make_option("A2", 110.0, "CALL", ts1, "", false),
        make_option("B1", 100.0, "CALL", ts2, "", false),
        make_option("B2", 110.0, "CALL", ts2, "", false),
    ];
    let filtered = filter_option_nodes(opts, Some("CALL"), None, None, None, Some(&[1]));
    let ids: HashSet<String> = filtered.into_iter().map(|o| o.instrument_id).collect();
    assert!(ids.contains("B1"));
    assert!(ids.contains("B2"));
    assert!(!ids.contains("A1"));
    assert!(!ids.contains("A2"));
}

#[test]
fn parse_symbol_info_maps_fields() {
    let expire_ts = 1_731_801_600_i64 * 1_000_000_000;
    let exercise_ts = 1_714_492_800_i64 * 1_000_000_000;
    let res = json!({
        "result": {
            "multi_symbol_info": [
                {
                    "instrument_id": "SHFE.cu2405",
                    "class": "FUTURE",
                    "exchange_id": "SHFE",
                    "product_id": "cu",
                    "instrument_name": "沪铜",
                    "price_tick": 10.0,
                    "volume_multiple": 5,
                    "delivery_year": 2024,
                    "delivery_month": 5,
                    "expire_datetime": expire_ts,
                    "max_limit_order_volume": 100,
                    "max_market_order_volume": 50,
                    "min_limit_order_volume": 2,
                    "min_market_order_volume": 1
                },
                {
                    "instrument_id": "SHFE.cu2405C3000",
                    "class": "OPTION",
                    "exchange_id": "SHFE",
                    "call_or_put": "CALL",
                    "strike_price": 3000.0,
                    "open_min_market_order_volume": 3,
                    "open_min_limit_order_volume": 5,
                    "open_max_market_order_volume": 9,
                    "open_max_limit_order_volume": 11,
                    "expired": false,
                    "last_exercise_datetime": exercise_ts,
                    "expire_datetime": expire_ts,
                    "underlying": {
                        "edges": [
                            {
                                "node": {
                                    "instrument_id": "SHFE.cu2405",
                                    "class": "FUTURE",
                                    "exchange_id": "SHFE",
                                    "product_id": "cu",
                                    "delivery_year": 2024,
                                    "delivery_month": 5,
                                    "volume_multiple": 5
                                }
                            }
                        ]
                    }
                },
                {
                    "instrument_id": "SHFE.cu2405&cu2406",
                    "class": "COMBINE",
                    "leg1": { "instrument_id": "SHFE.cu2405" }
                }
            ]
        }
    });

    let list = parse_query_symbol_info_result(
        &res,
        &["SHFE.cu2405C3000".to_string(), "SHFE.cu2405&cu2406".to_string()],
        1_731_715_200,
    );
    let option = list[0].as_object().unwrap();
    assert_eq!(option.get("ins_class").unwrap(), "OPTION");
    assert_eq!(option.get("underlying_symbol").unwrap(), "SHFE.cu2405");
    assert_eq!(option.get("delivery_year").unwrap(), 2024);
    assert_eq!(option.get("delivery_month").unwrap(), 5);
    assert_eq!(option.get("exercise_year").unwrap(), 2024);
    assert_eq!(option.get("exercise_month").unwrap(), 5);
    assert_eq!(option.get("open_min_market_order_volume").unwrap(), 3);
    assert_eq!(option.get("open_min_limit_order_volume").unwrap(), 5);
    assert_eq!(option.get("open_max_market_order_volume").unwrap(), 9);
    assert_eq!(option.get("open_max_limit_order_volume").unwrap(), 11);
    assert_eq!(option.get("min_limit_order_volume").unwrap(), &serde_json::Value::Null);
    assert_eq!(option.get("min_market_order_volume").unwrap(), &serde_json::Value::Null);

    let combine = list[1].as_object().unwrap();
    assert_eq!(combine.get("ins_class").unwrap(), "COMBINE");
    assert_eq!(combine.get("volume_multiple").unwrap(), 5);
}

#[tokio::test]
#[ignore]
async fn graphql_live_smoke() -> Result<()> {
    let user = match std::env::var("TQ_AUTH_USER") {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let pass = match std::env::var("TQ_AUTH_PASS") {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };

    let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
    client.init_market().await?;

    let quote_sub = match run_with_timeout(client.subscribe_quote(&["SHFE.au2602"]), 30).await {
        Ok(sub) => sub,
        Err(err) => {
            println!("graphql_live_smoke subscribe_quote error: {}", err);
            client.close().await?;
            return Ok(());
        }
    };

    let quotes = match run_with_timeout(
        client.query_quotes(Some("FUTURE"), Some("SHFE"), Some("cu"), Some(false), None),
        30,
    )
    .await
    {
        Ok(list) => list,
        Err(err) => {
            println!("graphql_live_smoke query_quotes error: {}", err);
            quote_sub.close().await?;
            client.close().await?;
            return Ok(());
        }
    };
    let underlying = quotes.first().cloned().unwrap_or_else(|| "SHFE.cu2405".to_string());

    let _ = match run_with_timeout(client.query_cont_quotes(Some("SHFE"), Some("cu"), None), 30).await {
        Ok(list) => list,
        Err(err) => {
            println!("graphql_live_smoke query_cont_quotes error: {}", err);
            quote_sub.close().await?;
            client.close().await?;
            return Ok(());
        }
    };
    let _ = match run_with_timeout(
        client.query_options(&underlying, None, None, None, None, None, None),
        30,
    )
    .await
    {
        Ok(list) => list,
        Err(err) => {
            println!("graphql_live_smoke query_options error: {}", err);
            quote_sub.close().await?;
            client.close().await?;
            return Ok(());
        }
    };

    let query = r#"query($class_:[Class]){ multi_symbol_info(class:$class_){ ... on basic { instrument_id } } }"#;
    let variables = json!({ "class_": ["FUTURE"] });
    let raw = match run_with_timeout(client.query_graphql(query, Some(variables)), 30).await {
        Ok(value) => value,
        Err(err) => {
            println!("graphql_live_smoke query_graphql error: {}", err);
            quote_sub.close().await?;
            client.close().await?;
            return Ok(());
        }
    };
    if raw.get("result").is_none() {
        println!("graphql_live_smoke result missing");
    }
    quote_sub.close().await?;
    client.close().await?;

    Ok(())
}

struct DummyAuth {
    features: HashSet<String>,
}

#[async_trait]
impl Authenticator for DummyAuth {
    fn base_header(&self) -> HeaderMap {
        HeaderMap::new()
    }

    async fn login(&mut self) -> Result<()> {
        Ok(())
    }

    async fn get_td_url(&self, _broker_id: &str, _account_id: &str) -> Result<crate::auth::BrokerInfo> {
        Err(TqError::NotLoggedIn)
    }

    async fn get_md_url(&self, _stock: bool, _backtest: bool) -> Result<String> {
        Err(TqError::NotLoggedIn)
    }

    fn has_feature(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    fn has_md_grants(&self, _symbols: &[&str]) -> Result<()> {
        Ok(())
    }

    fn has_td_grants(&self, _symbol: &str) -> Result<()> {
        Ok(())
    }

    fn get_auth_id(&self) -> &str {
        ""
    }

    fn get_access_token(&self) -> &str {
        ""
    }
}

fn build_ins_api(features: &[&str]) -> InsAPI {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let mut feature_set = HashSet::new();
    for item in features {
        feature_set.insert(item.to_string());
    }
    let auth = Arc::new(RwLock::new(DummyAuth { features: feature_set }));
    InsAPI::new(
        dm,
        ws,
        None,
        auth,
        true,
        "https://files.shinnytech.com/shinny_chinese_holiday.json".to_string(),
    )
}

fn build_ins_api_with_trading_status(features: &[&str]) -> (Arc<DataManager>, InsAPI) {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let ts_ws = Arc::new(TqTradingStatusWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    let mut feature_set = HashSet::new();
    for item in features {
        feature_set.insert(item.to_string());
    }
    let auth = Arc::new(RwLock::new(DummyAuth { features: feature_set }));
    let api = InsAPI::new(
        dm.clone(),
        ws,
        Some(ts_ws),
        auth,
        true,
        "https://files.shinnytech.com/shinny_chinese_holiday.json".to_string(),
    );
    (dm, api)
}

#[tokio::test]
async fn settlement_rejects_invalid_args() {
    let api = build_ins_api(&[]);
    let err = api.query_symbol_settlement(&[], 1, None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api
        .query_symbol_settlement(&["SHFE.cu2405"], 0, None)
        .await
        .unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn ranking_rejects_invalid_args() {
    let api = build_ins_api(&[]);
    let err = api.query_symbol_ranking("", "VOLUME", 1, None, None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api
        .query_symbol_ranking("SHFE.cu2405", "INVALID", 1, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api
        .query_symbol_ranking("SHFE.cu2405", "VOLUME", 1, None, Some(""))
        .await
        .unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn edb_rejects_invalid_args() {
    let api = build_ins_api(&[]);
    let err = api.query_edb_data(&[], 1, None, None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api.query_edb_data(&[1], 0, None, None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api.query_edb_data(&[1], 1, Some("week"), None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let err = api.query_edb_data(&[1], 1, None, Some("pad")).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
    let ids: Vec<i32> = (1..=101).collect();
    let err = api.query_edb_data(&ids, 1, None, None).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn trading_calendar_rejects_invalid_range() {
    let api = build_ins_api(&[]);
    let start = NaiveDate::from_ymd_opt(2024, 12, 2).unwrap();
    let end = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
    let err = api.get_trading_calendar(start, end).await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn trading_status_rejects_without_permission() {
    let api = build_ins_api(&[]);
    let err = api.get_trading_status("SHFE.cu2405").await.unwrap_err();
    assert!(matches!(err, TqError::PermissionDenied(_)));
    let err = api.get_trading_status("").await.unwrap_err();
    assert!(matches!(err, TqError::InvalidParameter(_)));
}

#[tokio::test]
async fn trading_status_allows_multiple_subscribers_for_same_symbol() {
    let (dm, api) = build_ins_api_with_trading_status(&["tq_trading_status"]);
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.cu2405": {
                    "class": "FUTURE"
                }
            }
        }),
        true,
        true,
    );

    let rx1 = tokio::time::timeout(Duration::from_secs(1), api.get_trading_status("SHFE.cu2405"))
        .await
        .unwrap()
        .unwrap();
    let rx2 = tokio::time::timeout(Duration::from_secs(1), api.get_trading_status("SHFE.cu2405"))
        .await
        .unwrap()
        .unwrap();

    dm.merge_data(
        json!({
            "trading_status": {
                "SHFE.cu2405": {
                    "symbol": "SHFE.cu2405",
                    "trade_status": "CONTINOUS"
                }
            }
        }),
        true,
        true,
    );

    let status1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv())
        .await
        .unwrap()
        .unwrap();
    let status2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(status1.symbol, "SHFE.cu2405");
    assert_eq!(status2.symbol, "SHFE.cu2405");
}

#[tokio::test]
async fn trading_status_receiver_drop_should_release_symbol_ref_count() {
    let (dm, api) = build_ins_api_with_trading_status(&["tq_trading_status"]);
    dm.merge_data(
        json!({
            "quotes": {
                "SHFE.cu2405": {
                    "class": "FUTURE"
                }
            }
        }),
        true,
        true,
    );

    let rx1 = api.get_trading_status("SHFE.cu2405").await.unwrap();
    let rx2 = api.get_trading_status("SHFE.cu2405").await.unwrap();

    assert_eq!(api.trading_status_ref_count_for_test("SHFE.cu2405").await, 2);

    drop(rx1);
    drop(rx2);

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if api.trading_status_ref_count_for_test("SHFE.cu2405").await == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("receiver drop should eventually release symbol ref-count");
}

#[tokio::test]
#[ignore]
async fn live_high_priority_interfaces() -> Result<()> {
    let auth_user = std::env::var("TQ_AUTH_USER").ok();
    let auth_pass = std::env::var("TQ_AUTH_PASS").ok();
    let symbol = std::env::var("TQ_TEST_SYMBOL").unwrap_or_else(|_| "SHFE.cu2405".to_string());

    let api = build_ins_api(&[]);
    match api.query_symbol_settlement(&[&symbol], 1, None).await {
        Ok(settlement) => println!("query_symbol_settlement: {:?}", settlement),
        Err(err) => println!("query_symbol_settlement error: {}", err),
    }

    match api.query_symbol_ranking(&symbol, "VOLUME", 1, None, None).await {
        Ok(ranking) => println!("query_symbol_ranking: {:?}", ranking),
        Err(err) => println!("query_symbol_ranking error: {}", err),
    }

    match api.query_edb_data(&[1, 2], 5, Some("day"), Some("ffill")).await {
        Ok(edb) => println!("query_edb_data: {:?}", edb),
        Err(err) => println!("query_edb_data error: {}", err),
    }

    let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();
    match api.get_trading_calendar(start, end).await {
        Ok(calendar) => println!("get_trading_calendar: {:?}", calendar),
        Err(err) => println!("get_trading_calendar error: {}", err),
    }

    if let (Some(user), Some(pass)) = (auth_user, auth_pass) {
        let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
        client.init_market().await?;
        match client.get_trading_status(&symbol).await {
            Ok(rx) => {
                let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
                match recv {
                    Ok(Ok(status)) => println!("get_trading_status: {:?}", status),
                    Ok(Err(err)) => println!("get_trading_status recv error: {}", err),
                    Err(_) => println!("get_trading_status timeout"),
                }
            }
            Err(err) => println!("get_trading_status error: {}", err),
        }
        client.close().await?;
    } else {
        println!("get_trading_status skipped: missing TQ_AUTH_USER/TQ_AUTH_PASS");
    }

    Ok(())
}

#[tokio::test]
#[ignore]
async fn live_query_cont_and_symbol_info() -> Result<()> {
    let user = match std::env::var("TQ_AUTH_USER") {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let pass = match std::env::var("TQ_AUTH_PASS") {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };

    let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
    client.init_market().await?;

    match run_with_timeout(client.query_cont_quotes(Some("GFEX"), Some("lc"), None), 30).await {
        Ok(list) => println!("query_cont_quotes: {:?}", list),
        Err(err) => println!("query_cont_quotes error: {}", err),
    }

    match run_with_timeout(client.query_symbol_info(&["GFEX.lc2605"]), 30).await {
        Ok(info) => println!("query_symbol_info: {:?}", info),
        Err(err) => println!("query_symbol_info error: {}", err),
    }

    client.close().await?;
    Ok(())
}

async fn run_with_timeout<T, F>(future: F, secs: u64) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match tokio::time::timeout(Duration::from_secs(secs), future).await {
        Ok(result) => result,
        Err(_) => Err(TqError::Timeout),
    }
}
