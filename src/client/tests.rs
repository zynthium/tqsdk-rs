use super::*;
use crate::auth::Authenticator;
use crate::errors::{Result, TqError};
use crate::websocket::WebSocketConfig;
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::time::{Duration, sleep};

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

    async fn get_td_url(
        &self,
        _broker_id: &str,
        _account_id: &str,
    ) -> Result<crate::auth::BrokerInfo> {
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
    let auth: Arc<RwLock<dyn Authenticator>> = Arc::new(RwLock::new(TestAuth));
    let quotes_ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
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
    ));

    Client {
        _username: "tester".to_string(),
        _config: ClientConfig::default(),
        auth,
        dm,
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

    let fired = Arc::new(AtomicUsize::new(0));
    {
        let fired = Arc::clone(&fired);
        sub.on_quote(move |_quote| {
            fired.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    }

    client.dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:00.000000",
                    "last_price": 500.0
                }
            }
        }),
        true,
        true,
    );
    sleep(Duration::from_millis(30)).await;
    assert_eq!(fired.load(Ordering::SeqCst), 0);

    sub.start().await.unwrap();
    client.dm.merge_data(
        json!({
            "quotes": {
                "SHFE.au2602": {
                    "instrument_id": "SHFE.au2602",
                    "datetime": "2024-01-01 09:00:01.000000",
                    "last_price": 501.0
                }
            }
        }),
        true,
        true,
    );
    sleep(Duration::from_millis(30)).await;
    assert_eq!(fired.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn close_invalidates_market_interfaces() {
    let client = build_client_with_market();
    client.close().await.unwrap();

    assert!(client.series().is_err());
    assert!(client.ins().is_err());
    assert!(client.subscribe_quote(&["SHFE.au2602"]).await.is_err());
}
