use super::*;
use crate::auth::Authenticator;
use crate::errors::{Result, TqError};
use crate::marketdata::MarketDataState;
use crate::websocket::WebSocketConfig;
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Default)]
struct TestAuth;

#[derive(Default)]
struct SmTestAuth;

#[async_trait]
impl Authenticator for TestAuth {
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
        Ok("wss://example.com".to_string())
    }

    fn has_feature(&self, _feature: &str) -> bool {
        false
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

#[async_trait]
impl Authenticator for SmTestAuth {
    fn base_header(&self) -> HeaderMap {
        HeaderMap::new()
    }

    async fn login(&mut self) -> Result<()> {
        Ok(())
    }

    async fn get_td_url(&self, _broker_id: &str, _account_id: &str) -> Result<crate::auth::BrokerInfo> {
        Ok(crate::auth::BrokerInfo {
            category: vec!["TQ".to_string()],
            url: "http://example.org/trade".to_string(),
            broker_type: Some("FUTURE".to_string()),
            smtype: Some("smtcp".to_string()),
            smconfig: Some("cfg".to_string()),
            condition_type: None,
            condition_config: None,
        })
    }

    async fn get_md_url(&self, _stock: bool, _backtest: bool) -> Result<String> {
        Ok("wss://example.com".to_string())
    }

    fn has_feature(&self, _feature: &str) -> bool {
        false
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

fn build_client_with_market() -> Client {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        crate::datamanager::DataManagerConfig::default(),
    ));
    let market_state = Arc::new(MarketDataState::default());
    let auth: Arc<tokio::sync::RwLock<dyn Authenticator>> = Arc::new(tokio::sync::RwLock::new(TestAuth));
    let live_api = crate::marketdata::TqApi::new(Arc::clone(&market_state));
    let quotes_ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::clone(&market_state),
        WebSocketConfig::default(),
    ));
    let series_api = Arc::new(SeriesAPI::new(
        Arc::clone(&dm),
        Arc::clone(&quotes_ws),
        Arc::clone(&auth),
    ));
    let ins_api = Arc::new(InsAPI::new(
        Arc::clone(&dm),
        Arc::clone(&quotes_ws),
        None,
        Arc::clone(&auth),
        true,
        "https://files.shinnytech.com/shinny_chinese_holiday.json".to_string(),
    ));

    Client {
        username: "tester".to_string(),
        config: ClientConfig::default(),
        endpoints: EndpointConfig::default(),
        auth,
        dm,
        market_state,
        live_api,
        quotes_ws: Some(quotes_ws),
        series_api: Some(series_api),
        ins_api: Some(ins_api),
        market_active: AtomicBool::new(true),
        trade_sessions: Arc::new(std::sync::RwLock::new(HashMap::new())),
    }
}

fn build_inactive_client() -> Client {
    build_inactive_client_with_auth(TestAuth)
}

fn build_inactive_client_with_auth<A: Authenticator + 'static>(auth_impl: A) -> Client {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        crate::datamanager::DataManagerConfig::default(),
    ));
    let market_state = Arc::new(MarketDataState::default());
    let auth: Arc<tokio::sync::RwLock<dyn Authenticator>> = Arc::new(tokio::sync::RwLock::new(auth_impl));
    let live_api = crate::marketdata::TqApi::new(Arc::clone(&market_state));

    Client {
        username: "tester".to_string(),
        config: ClientConfig::default(),
        endpoints: EndpointConfig::default(),
        auth,
        dm,
        market_state,
        live_api,
        quotes_ws: None,
        series_api: None,
        ins_api: None,
        market_active: AtomicBool::new(false),
        trade_sessions: Arc::new(std::sync::RwLock::new(HashMap::new())),
    }
}

#[tokio::test]
async fn client_exposes_market_state_refs_directly() {
    let client = build_client_with_market();
    let symbol = "SHFE.au2602";

    let quote_ref = client.quote(symbol);
    let kline_ref = client.kline_ref(symbol, Duration::from_secs(60));
    let tick_ref = client.tick_ref(symbol);

    let quote = crate::types::Quote {
        instrument_id: "au2602".to_string(),
        last_price: 520.5,
        ..Default::default()
    };

    let kline = crate::types::Kline {
        id: 7,
        datetime: 1_711_111_111,
        open: 510.0,
        close: 521.0,
        high: 523.0,
        low: 509.0,
        open_oi: 10,
        close_oi: 12,
        volume: 99,
        epoch: None,
    };

    let tick = crate::types::Tick {
        id: 11,
        datetime: 1_711_111_222,
        last_price: 520.5,
        average: 519.0,
        highest: 523.0,
        lowest: 509.0,
        ask_price1: 521.0,
        ask_volume1: 3,
        bid_price1: 520.0,
        bid_volume1: 2,
        volume: 101,
        amount: 0.0,
        open_interest: 88,
        ..Default::default()
    };

    client.market_state.update_quote(symbol.into(), quote).await;
    client
        .market_state
        .update_kline(crate::marketdata::KlineKey::new(symbol, Duration::from_secs(60)), kline)
        .await;
    client.market_state.update_tick(symbol.into(), tick).await;

    let updates = client.wait_update_and_drain().await.unwrap();
    assert_eq!(updates.quotes.len(), 1);
    assert_eq!(updates.klines.len(), 1);
    assert_eq!(updates.ticks.len(), 1);

    let quote = quote_ref.load().await;
    let kline = kline_ref.load().await;
    let tick = tick_ref.load().await;

    assert_eq!(quote_ref.symbol(), symbol);
    assert_eq!(kline_ref.symbol(), symbol);
    assert_eq!(tick_ref.symbol(), symbol);
    assert_eq!(quote.last_price, 520.5);
    assert_eq!(kline.close, 521.0);
    assert_eq!(tick.last_price, 520.5);
}

#[tokio::test]
async fn client_exposes_serial_subscriptions_directly() {
    let client = build_client_with_market();

    let kline = client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await;
    let tick = client.get_tick_serial("SHFE.au2602", 64).await;

    assert!(kline.is_ok());
    assert!(tick.is_ok());
}

#[tokio::test]
async fn subscribe_quote_is_active_immediately() {
    let client = build_client_with_market();
    let ws = client.quotes_ws.as_ref().unwrap();

    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());

    let sub = client.subscribe_quote(&["SHFE.au2602"]).await.unwrap();
    assert_eq!(
        ws.aggregated_quote_subscriptions_for_test(),
        HashSet::from(["SHFE.au2602".to_string()])
    );

    sub.close().await.unwrap();
}

#[tokio::test]
async fn subscribe_quote_creation_failure_rolls_back_server_state() {
    let client = build_client_with_market();
    let ws = client.quotes_ws.as_ref().unwrap();
    ws.force_send_failure_for_test();

    let sub = client.subscribe_quote(&["SHFE.au2602"]).await;

    assert!(sub.is_err());
    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());
}

#[tokio::test]
async fn close_invalidates_market_interfaces() {
    let client = build_client_with_market();
    client.close().await.unwrap();

    assert!(client.query_quotes(None, None, None, None, None).await.is_err());
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
    assert!(client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await
        .is_err());
    assert!(client.get_tick_serial("SHFE.au2602", 64).await.is_err());
}

#[tokio::test]
async fn create_trade_session_allows_td_url_override() {
    let client = build_client_with_market();

    let session = client
        .create_trade_session_with_options(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
        )
        .await;

    assert!(session.is_ok());
}

#[tokio::test]
async fn create_trade_session_uses_sm_td_url_when_requested() {
    let client = build_inactive_client_with_auth(SmTestAuth);

    let session = client
        .create_trade_session_with_options(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: None,
                reliable_events_max_retained: 32,
                use_sm: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        session.ws_url_for_test(),
        "smtcp://example.org/cfg/dXNlcg==/cGFzc3dvcmQ=/trade"
    );
}

#[tokio::test]
async fn create_trade_session_plumbs_reliable_event_retention() {
    let client = build_client_with_market();

    let session = client
        .create_trade_session_with_options(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
        )
        .await
        .unwrap();

    assert_eq!(session.reliable_events_max_retained_for_test(), 32);
}

#[tokio::test]
async fn create_trade_session_with_login_options_plumbs_login_configuration() {
    let client = build_client_with_market();

    let session = client
        .create_trade_session_with_options_and_login(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
            crate::trade_session::TradeLoginOptions {
                front: Some(crate::trade_session::TradeFrontConfig {
                    broker_id: "9999".to_string(),
                    url: "tcp://127.0.0.1:12345".to_string(),
                }),
                client_system_info: Some("SYSINFO".to_string()),
                ..crate::trade_session::TradeLoginOptions::default()
            },
        )
        .await
        .unwrap();

    let login = session.login_options_for_test();
    assert_eq!(login.front.as_ref().unwrap().broker_id, "9999");
    assert_eq!(login.front.as_ref().unwrap().url, "tcp://127.0.0.1:12345");
    assert_eq!(login.client_system_info.as_deref(), Some("SYSINFO"));
}

#[tokio::test]
async fn runtime_picks_up_trade_sessions_registered_after_creation() {
    let client = build_inactive_client();
    let trade_sessions = Arc::clone(&client.trade_sessions);
    let runtime = client.into_runtime();

    assert!(runtime.account("simnow:user").is_err());

    let session = Arc::new(crate::trade_session::TradeSession::new(
        "simnow".to_string(),
        "user".to_string(),
        "password".to_string(),
        Arc::new(DataManager::new(
            HashMap::new(),
            crate::datamanager::DataManagerConfig::default(),
        )),
        "wss://example.com/trade".to_string(),
        WebSocketConfig::default(),
        32,
        crate::trade_session::TradeLoginOptions::default(),
    ));
    trade_sessions
        .write()
        .unwrap()
        .insert("simnow:user".to_string(), session);

    let account = runtime
        .account("simnow:user")
        .expect("runtime should see newly registered live trade sessions");
    assert_eq!(account.account_key(), "simnow:user");
}

#[tokio::test]
async fn builder_can_preconfigure_trade_sessions_for_runtime() {
    let runtime = Client::builder("tester", "secret")
        .auth(TestAuth)
        .trade_session_with_options(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
        )
        .build_runtime()
        .await
        .expect("builder should create runtime with configured trade sessions");

    let account = runtime
        .account("simnow:user")
        .expect("configured trade session should be exposed to runtime");
    assert_eq!(account.account_key(), "simnow:user");
}

#[tokio::test]
async fn builder_can_preconfigure_trade_sessions_with_login_options() {
    let client = Client::builder("tester", "secret")
        .auth(TestAuth)
        .trade_session_with_options_and_login(
            "simnow",
            "user",
            "password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
            crate::trade_session::TradeLoginOptions {
                front: Some(crate::trade_session::TradeFrontConfig {
                    broker_id: "9999".to_string(),
                    url: "tcp://127.0.0.1:12345".to_string(),
                }),
                client_system_info: Some("SYSINFO".to_string()),
                ..crate::trade_session::TradeLoginOptions::default()
            },
        )
        .build()
        .await
        .expect("builder should create client with configured login options");

    let session = client.get_trade_session("simnow:user").await.unwrap();
    let login = session.login_options_for_test();
    assert_eq!(login.front.as_ref().unwrap().broker_id, "9999");
    assert_eq!(login.client_system_info.as_deref(), Some("SYSINFO"));
}

#[tokio::test]
async fn trade_sessions_created_by_client_are_dm_isolated() {
    let client = build_inactive_client();
    let left = client
        .create_trade_session_with_options(
            "simnow",
            "shared-user",
            "left-password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade-left".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
        )
        .await
        .unwrap();
    let right = client
        .create_trade_session_with_options(
            "other",
            "shared-user",
            "right-password",
            TradeSessionOptions {
                td_url_override: Some("wss://example.com/trade-right".to_string()),
                reliable_events_max_retained: 32,
                use_sm: false,
            },
        )
        .await
        .unwrap();

    left.inject_trade_data_for_test(serde_json::json!({
        "trade": {
            "shared-user": {
                "accounts": {
                    "CNY": {
                        "user_id": "shared-user",
                        "currency": "CNY",
                        "balance": 1000.0,
                        "available": 900.0
                    }
                }
            }
        }
    }));
    right.inject_trade_data_for_test(serde_json::json!({
        "trade": {
            "shared-user": {
                "accounts": {
                    "CNY": {
                        "user_id": "shared-user",
                        "currency": "CNY",
                        "balance": 2000.0,
                        "available": 1800.0
                    }
                }
            }
        }
    }));

    let left_account = left.get_account().await.unwrap();
    let right_account = right.get_account().await.unwrap();

    assert_eq!(left_account.balance, 1000.0);
    assert_eq!(right_account.balance, 2000.0);
}

#[tokio::test]
async fn create_backtest_session_does_not_activate_client_market_state() {
    let client = Client::builder("tester", "secret")
        .auth(TestAuth)
        .build()
        .await
        .expect("test client should build without network");

    let start = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
    let end = chrono::DateTime::<chrono::Utc>::from_timestamp(60, 0).unwrap();
    let _session = client
        .create_backtest_session(crate::ReplayConfig::new(start, end).unwrap())
        .await
        .expect("creating replay session should not initialize live market state");

    assert!(!client.market_active.load(std::sync::atomic::Ordering::SeqCst));
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
    assert!(client
        .get_kline_serial("SHFE.au2602", Duration::from_secs(60), 64)
        .await
        .is_err());
}
