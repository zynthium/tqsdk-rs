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
        trade_sessions: Arc::new(RwLock::new(HashMap::new())),
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
async fn client_exposes_series_subscriptions_directly() {
    let client = build_client_with_market();

    let kline = client.kline("SHFE.au2602", Duration::from_secs(60), 64).await;
    let tick = client.tick("SHFE.au2602", 64).await;

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

    assert!(client.ins().is_err());
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
    assert!(client.kline("SHFE.au2602", Duration::from_secs(60), 64).await.is_err());
    assert!(client.tick("SHFE.au2602", 64).await.is_err());
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
