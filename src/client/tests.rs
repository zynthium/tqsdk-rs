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

#[derive(Default)]
struct TestAuth;

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

fn build_client_with_market() -> Client {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        crate::datamanager::DataManagerConfig::default(),
    ));
    let market_state = Arc::new(MarketDataState::default());
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
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
        quotes_ws: Some(quotes_ws),
        series_api: Some(series_api),
        ins_api: Some(ins_api),
        market_active: AtomicBool::new(true),
        trade_sessions: Arc::new(RwLock::new(HashMap::new())),
    }
}

#[tokio::test]
async fn subscribe_quote_requires_explicit_start() {
    let client = build_client_with_market();
    let sub = client.subscribe_quote(&["SHFE.au2602"]).await.unwrap();
    let ws = client.quotes_ws.as_ref().unwrap();

    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());

    sub.start().await.unwrap();
    assert_eq!(
        ws.aggregated_quote_subscriptions_for_test(),
        HashSet::from(["SHFE.au2602".to_string()])
    );
}

#[tokio::test]
async fn close_invalidates_market_interfaces() {
    let client = build_client_with_market();
    client.close().await.unwrap();

    assert!(client.series().is_err());
    assert!(client.ins().is_err());
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
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
            },
        )
        .await;

    assert!(session.is_ok());
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
            },
        )
        .await
        .unwrap();

    assert_eq!(session.reliable_events_max_retained_for_test(), 32);
}
