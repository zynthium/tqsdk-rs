use super::QuoteSubscription;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::marketdata::MarketDataState;
use crate::websocket::{TqQuoteWebsocket, WebSocketConfig};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn symbol_set(symbols: &[&str]) -> HashSet<String> {
    symbols.iter().map(|symbol| (*symbol).to_string()).collect()
}

#[tokio::test]
async fn start_failure_rolls_back_quote_subscription_state() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();

    let sub = QuoteSubscription::new(Arc::clone(&ws), vec!["SHFE.au2602".to_string()]);

    assert!(sub.start().await.is_err());
    assert!(!*sub.running.read().await);
    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());
}

#[tokio::test]
async fn quote_subscription_start_and_close_sync_server_lifecycle() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let sub = QuoteSubscription::new(Arc::clone(&ws), vec!["SHFE.au2602".to_string()]);

    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());
    sub.start().await.unwrap();
    assert_eq!(
        ws.aggregated_quote_subscriptions_for_test(),
        symbol_set(&["SHFE.au2602"])
    );

    sub.close().await.unwrap();
    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());
}

#[tokio::test]
async fn quote_subscription_add_remove_symbols_resync_server_state() {
    let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        Arc::new(MarketDataState::default()),
        WebSocketConfig::default(),
    ));
    let sub = QuoteSubscription::new(Arc::clone(&ws), vec!["SHFE.au2602".to_string()]);

    sub.start().await.unwrap();
    sub.add_symbols(&["SHFE.ag2602", "DCE.m2601"]).await.unwrap();
    assert_eq!(
        ws.aggregated_quote_subscriptions_for_test(),
        symbol_set(&["SHFE.au2602", "SHFE.ag2602", "DCE.m2601"])
    );

    sub.remove_symbols(&["SHFE.au2602"]).await.unwrap();
    assert_eq!(
        ws.aggregated_quote_subscriptions_for_test(),
        symbol_set(&["SHFE.ag2602", "DCE.m2601"])
    );

    sub.close().await.unwrap();
    assert!(ws.aggregated_quote_subscriptions_for_test().is_empty());
}
