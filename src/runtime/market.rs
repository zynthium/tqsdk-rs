use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

use crate::auth::Authenticator;
use crate::client::{ClientConfig, EndpointConfig};
use crate::datamanager::DataManager;
use crate::marketdata::MarketDataState;
use crate::types::Quote;
use crate::websocket::{TqQuoteWebsocket, WebSocketConfig};

use super::{RuntimeError, RuntimeResult};

/// Runtime market adapters are no longer derived from `DataManager`.
/// Implementations must provide the async market access methods directly.
#[async_trait]
pub trait MarketAdapter: Send + Sync {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote>;

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()>;

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>>;
}

pub struct LiveMarketAdapter {
    state: Arc<LiveMarketState>,
}

struct LiveMarketState {
    auth: Arc<tokio::sync::RwLock<dyn Authenticator>>,
    config: ClientConfig,
    endpoints: EndpointConfig,
    dm: Arc<DataManager>,
    quotes_ws: tokio::sync::Mutex<Option<Arc<TqQuoteWebsocket>>>,
    init_lock: tokio::sync::Mutex<()>,
    subscribed_symbols: tokio::sync::Mutex<HashSet<String>>,
    subscription_id: String,
}

impl LiveMarketAdapter {
    pub fn new(
        dm: Arc<DataManager>,
        auth: Arc<tokio::sync::RwLock<dyn Authenticator>>,
        config: ClientConfig,
        endpoints: EndpointConfig,
    ) -> Self {
        Self {
            state: Arc::new(LiveMarketState {
                auth,
                config,
                endpoints,
                dm,
                quotes_ws: tokio::sync::Mutex::new(None),
                init_lock: tokio::sync::Mutex::new(()),
                subscribed_symbols: tokio::sync::Mutex::new(HashSet::new()),
                subscription_id: format!("runtime-{}", uuid::Uuid::new_v4()),
            }),
        }
    }
}

impl LiveMarketState {
    async fn ensure_runtime_ready(&self) -> RuntimeResult<Arc<TqQuoteWebsocket>> {
        if let Some(ws) = self.quotes_ws.lock().await.as_ref().cloned() {
            return Ok(ws);
        }

        let _guard = self.init_lock.lock().await;
        if let Some(ws) = self.quotes_ws.lock().await.as_ref().cloned() {
            return Ok(ws);
        }

        let (md_url, headers) = {
            let auth = self.auth.read().await;
            let md_url = if let Some(md_url) = self.endpoints.md_url.as_deref() {
                md_url.to_string()
            } else {
                auth.get_md_url(self.config.stock, false).await?
            };
            (md_url, auth.base_header())
        };

        self.preload_symbol_info(headers.clone()).await?;

        let ws_config = WebSocketConfig {
            headers,
            auto_peek: true,
            quote_subscribe_only_add: false,
            message_queue_capacity: self.config.message_queue_capacity,
            message_backlog_warn_step: self.config.message_backlog_warn_step,
            message_batch_max: self.config.message_batch_max,
            ..Default::default()
        };
        let ws = Arc::new(TqQuoteWebsocket::new(
            md_url,
            Arc::clone(&self.dm),
            Arc::new(MarketDataState::default()),
            ws_config,
        ));
        ws.init(false).await?;
        *self.quotes_ws.lock().await = Some(Arc::clone(&ws));
        Ok(ws)
    }

    async fn ensure_symbol_subscription(&self, symbol: &str) -> RuntimeResult<()> {
        let ws = self.ensure_runtime_ready().await?;
        let mut symbols = self.subscribed_symbols.lock().await;
        if !symbols.insert(symbol.to_string()) {
            return Ok(());
        }
        ws.update_quote_subscription(&self.subscription_id, symbols.clone())
            .await?;
        Ok(())
    }

    async fn preload_symbol_info(&self, headers: HeaderMap) -> RuntimeResult<()> {
        if self.config.stock {
            return Ok(());
        }

        let url = self.endpoints.ins_url.trim();
        if url.is_empty() {
            return Ok(());
        }

        let content = crate::utils::fetch_json_with_headers(url, headers).await?;
        let symbols = content
            .as_object()
            .ok_or_else(|| RuntimeError::Tq(crate::TqError::ParseError("合约信息格式错误，应为对象".to_string())))?;

        let mut quotes = Map::new();
        for (symbol, item) in symbols {
            let Some(source) = item.as_object() else {
                continue;
            };
            quotes.insert(symbol.clone(), Value::Object(build_preloaded_quote(source)));
        }

        if let Some(mut quote) = quotes.remove("CSI.000300") {
            if let Some(obj) = quote.as_object_mut() {
                obj.insert("exchange_id".to_string(), Value::String("SSE".to_string()));
            }
            quotes.insert("SSE.000300".to_string(), quote);
        }

        for (symbol, quote_value) in &mut quotes {
            if symbol.starts_with("CFFEX.IO")
                && let Some(obj) = quote_value.as_object_mut()
                && obj.get("ins_class").and_then(Value::as_str) == Some("OPTION")
            {
                obj.insert("underlying_symbol".to_string(), Value::String("SSE.000300".to_string()));
            }
        }

        let mut payload = Map::new();
        payload.insert("quotes".to_string(), Value::Object(quotes));
        self.dm.merge_data(Value::Object(payload), true, true);
        Ok(())
    }

    async fn wait_until_quote_ready(&self, symbol: &str) -> RuntimeResult<Quote> {
        loop {
            if let Ok(quote) = self.dm.get_quote_data(symbol)
                && !quote.datetime.is_empty()
            {
                return Ok(quote);
            }

            let rx = self.dm.watch(vec!["quotes".to_string(), symbol.to_string()]);
            rx.recv().await.map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "market quote bootstrap",
            })?;
        }
    }
}

#[async_trait]
impl MarketAdapter for LiveMarketAdapter {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        self.state.ensure_symbol_subscription(symbol).await?;
        self.state.wait_until_quote_ready(symbol).await
    }

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()> {
        self.state.ensure_symbol_subscription(symbol).await?;
        let rx = self.state.dm.watch(vec!["quotes".to_string(), symbol.to_string()]);
        rx.recv()
            .await
            .map(|_| ())
            .map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "market quote updates",
            })
    }

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>> {
        self.state.ensure_symbol_subscription(symbol).await?;
        Ok(self.state.dm.get_by_path(&["quotes", symbol, "trading_time"]))
    }
}

fn build_preloaded_quote(source: &Map<String, Value>) -> Map<String, Value> {
    let mut quote = source.clone();
    quote.insert("datetime".to_string(), Value::String(String::new()));
    quote.insert("last_price".to_string(), Value::from(f64::NAN));
    quote.insert("ask_price1".to_string(), Value::from(f64::NAN));
    quote.insert("ask_volume1".to_string(), Value::from(0));
    quote.insert("bid_price1".to_string(), Value::from(f64::NAN));
    quote.insert("bid_volume1".to_string(), Value::from(0));
    quote.insert("highest".to_string(), Value::from(f64::NAN));
    quote.insert("lowest".to_string(), Value::from(f64::NAN));
    quote.insert("open".to_string(), Value::from(f64::NAN));
    quote.insert("close".to_string(), Value::from(f64::NAN));
    quote.insert("average".to_string(), Value::from(f64::NAN));
    quote.insert("volume".to_string(), Value::from(0));
    quote.insert("amount".to_string(), Value::from(0.0));
    quote.insert("open_interest".to_string(), Value::from(0));
    quote
}
