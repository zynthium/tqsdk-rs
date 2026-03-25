use super::QuoteSubscription;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::websocket::{TqQuoteWebsocket, WebSocketConfig};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::test]
async fn start_failure_rolls_back_quote_callback_registration() {
    let dm = Arc::new(DataManager::new(
        HashMap::new(),
        DataManagerConfig::default(),
    ));
    let ws = Arc::new(TqQuoteWebsocket::new(
        "wss://example.com".to_string(),
        Arc::clone(&dm),
        WebSocketConfig::default(),
    ));
    ws.force_send_failure_for_test();

    let sub = QuoteSubscription::new(dm, ws, vec!["SHFE.au2602".to_string()]);

    assert!(sub.start().await.is_err());
    assert!(!*sub.running.read().await);
    assert!(sub.data_cb_id.lock().unwrap().is_none());
}
