use super::Client;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::marketdata::MarketDataState;
use crate::replay::{HistoricalSource, InstrumentMetadata};
use crate::series::{SeriesAPI, SeriesCachePolicy};
use crate::types::Kline;
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket, WebSocketConfig};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

struct MarketBootstrap {
    md_url: String,
    headers: HeaderMap,
    enable_trading_status: bool,
}

#[derive(Clone)]
pub(crate) struct SdkHistoricalSource {
    state: Arc<SdkHistoricalSourceState>,
}

struct SdkHistoricalSourceState {
    auth: Arc<tokio::sync::RwLock<dyn crate::auth::Authenticator>>,
    config: crate::client::ClientConfig,
    endpoints: crate::client::EndpointConfig,
    apis: tokio::sync::Mutex<Option<HistoricalApis>>,
}

struct HistoricalApis {
    _dm: Arc<DataManager>,
    _quotes_ws: Arc<TqQuoteWebsocket>,
    ins: Arc<InsAPI>,
    series: Arc<SeriesAPI>,
}

impl SdkHistoricalSource {
    pub(crate) fn new(
        auth: Arc<tokio::sync::RwLock<dyn crate::auth::Authenticator>>,
        config: crate::client::ClientConfig,
        endpoints: crate::client::EndpointConfig,
    ) -> Self {
        Self {
            state: Arc::new(SdkHistoricalSourceState {
                auth,
                config,
                endpoints,
                apis: tokio::sync::Mutex::new(None),
            }),
        }
    }

    async fn ensure_apis(&self) -> Result<(Arc<InsAPI>, Arc<SeriesAPI>)> {
        let mut guard = self.state.apis.lock().await;
        if let Some(apis) = guard.as_ref() {
            return Ok((Arc::clone(&apis.ins), Arc::clone(&apis.series)));
        }

        let dm = Arc::new(DataManager::new(
            std::collections::HashMap::new(),
            DataManagerConfig {
                default_view_width: self.state.config.view_width,
                enable_auto_cleanup: true,
                ..DataManagerConfig::default()
            },
        ));

        let (md_url, headers) = {
            let auth = self.state.auth.read().await;
            let md_url = if let Some(md_url) = self.state.endpoints.md_url.as_deref() {
                md_url.to_string()
            } else {
                auth.get_md_url(self.state.config.stock, true).await?
            };
            (md_url, auth.base_header())
        };

        preload_symbol_info_into_dm(
            self.state.config.stock,
            &self.state.endpoints.ins_url,
            headers.clone(),
            &dm,
            None,
        )
        .await?;

        let ws_config = WebSocketConfig {
            headers,
            auto_peek: false,
            quote_subscribe_only_add: true,
            message_queue_capacity: self.state.config.message_queue_capacity,
            message_backlog_warn_step: self.state.config.message_backlog_warn_step,
            message_batch_max: self.state.config.message_batch_max,
            ..Default::default()
        };
        let quotes_ws = Arc::new(TqQuoteWebsocket::new(
            md_url,
            Arc::clone(&dm),
            Arc::new(MarketDataState::default()),
            ws_config,
        ));
        quotes_ws.init(false).await?;

        let series = Arc::new(SeriesAPI::new_with_cache_policy(
            Arc::clone(&dm),
            Arc::clone(&quotes_ws),
            Arc::clone(&self.state.auth),
            SeriesCachePolicy {
                enabled: self.state.config.series_disk_cache_enabled,
                max_bytes: self.state.config.series_disk_cache_max_bytes,
                retention_days: self.state.config.series_disk_cache_retention_days,
            },
        ));
        let ins = Arc::new(InsAPI::new(
            Arc::clone(&dm),
            Arc::clone(&quotes_ws),
            None,
            Arc::clone(&self.state.auth),
            self.state.config.stock,
            self.state.endpoints.holiday_url.clone(),
        ));

        *guard = Some(HistoricalApis {
            _dm: Arc::clone(&dm),
            _quotes_ws: Arc::clone(&quotes_ws),
            ins: Arc::clone(&ins),
            series: Arc::clone(&series),
        });
        Ok((ins, series))
    }
}

#[async_trait]
impl HistoricalSource for SdkHistoricalSource {
    async fn instrument_metadata(&self, symbol: &str) -> Result<InstrumentMetadata> {
        let (ins, _) = self.ensure_apis().await?;
        let mut rows = ins.query_symbol_info(&[symbol]).await?;
        let row = rows
            .pop()
            .ok_or_else(|| TqError::DataNotFound(format!("instrument metadata not found: {symbol}")))?;
        Ok(parse_instrument_metadata(symbol, &row))
    }

    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        let (_, series) = self.ensure_apis().await?;
        series
            .replay_kline_data_series(symbol, duration, start_dt, end_dt)
            .await
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<crate::types::Tick>> {
        let (_, series) = self.ensure_apis().await?;
        series.replay_tick_data_series(symbol, start_dt, end_dt).await
    }
}

impl Client {
    async fn preload_symbol_info(&self, headers: HeaderMap) -> Result<()> {
        preload_symbol_info_into_dm(
            self.config.stock,
            &self.endpoints.ins_url,
            headers,
            &self.live.dm,
            Some(&self.live.market_state),
        )
        .await
    }

    async fn load_market_bootstrap(&self, backtest: bool) -> Result<MarketBootstrap> {
        let auth = self.auth.read().await;
        let md_url = if let Some(md_url) = self.endpoints.md_url.as_deref() {
            md_url.to_string()
        } else {
            auth.get_md_url(self.config.stock, backtest).await?
        };
        let headers = auth.base_header();
        let enable_trading_status = auth.has_feature("tq_trading_status");
        drop(auth);

        Ok(MarketBootstrap {
            md_url,
            headers,
            enable_trading_status,
        })
    }

    fn build_market_ws_config(&self, headers: HeaderMap, backtest: bool) -> WebSocketConfig {
        WebSocketConfig {
            headers,
            auto_peek: !backtest,
            quote_subscribe_only_add: backtest,
            message_queue_capacity: self.config.message_queue_capacity,
            message_backlog_warn_step: self.config.message_backlog_warn_step,
            message_batch_max: self.config.message_batch_max,
            ..Default::default()
        }
    }

    async fn build_trading_status_ws(
        &self,
        enabled: bool,
        ws_config: &WebSocketConfig,
        backtest: bool,
    ) -> Result<Option<Arc<TqTradingStatusWebsocket>>> {
        if !enabled {
            return Ok(None);
        }

        let mut status_config = ws_config.clone();
        if backtest {
            status_config.auto_peek = true;
        }

        let ts_ws = Arc::new(TqTradingStatusWebsocket::new(
            "wss://trading-status.shinnytech.com/status".to_string(),
            Arc::clone(&self.live.dm),
            status_config,
        ));
        ts_ws.init(false).await?;
        Ok(Some(ts_ws))
    }

    async fn initialize_market_runtime(&mut self, backtest: bool) -> Result<Arc<TqQuoteWebsocket>> {
        let bootstrap = self.load_market_bootstrap(backtest).await?;
        self.preload_symbol_info(bootstrap.headers.clone()).await?;

        let ws_config = self.build_market_ws_config(bootstrap.headers, backtest);
        let quotes_ws = Arc::new(TqQuoteWebsocket::new(
            bootstrap.md_url,
            Arc::clone(&self.live.dm),
            Arc::clone(&self.live.market_state),
            ws_config.clone(),
        ));
        quotes_ws.init(false).await?;

        self.live.quotes_ws = Some(Arc::clone(&quotes_ws));
        self.live.series_api = Some(Arc::new(SeriesAPI::new_with_cache_policy(
            Arc::clone(&self.live.dm),
            Arc::clone(&quotes_ws),
            Arc::clone(&self.auth),
            SeriesCachePolicy {
                enabled: self.config.series_disk_cache_enabled,
                max_bytes: self.config.series_disk_cache_max_bytes,
                retention_days: self.config.series_disk_cache_retention_days,
            },
        )));

        let trading_status_ws = self
            .build_trading_status_ws(bootstrap.enable_trading_status, &ws_config, backtest)
            .await?;

        self.live.ins_api = Some(Arc::new(InsAPI::new(
            Arc::clone(&self.live.dm),
            Arc::clone(&quotes_ws),
            trading_status_ws,
            Arc::clone(&self.auth),
            self.config.stock,
            self.endpoints.holiday_url.clone(),
        )));
        self.live.set_active(true);

        Ok(quotes_ws)
    }

    pub(crate) async fn build_historical_source(&self) -> Result<Arc<dyn HistoricalSource>> {
        Ok(Arc::new(SdkHistoricalSource::new(
            Arc::clone(&self.auth),
            self.config.clone(),
            self.endpoints.clone(),
        )))
    }

    /// 初始化行情功能
    pub async fn init_market(&mut self) -> Result<()> {
        if self.live.market_state.is_closed() {
            return Err(TqError::InternalError(
                "cannot initialize market on a closed Client session; create a new Client instead".to_string(),
            ));
        }
        let _ = self.initialize_market_runtime(false).await?;
        Ok(())
    }

    pub async fn switch_to_live(&mut self) -> Result<()> {
        self.close_market().await?;
        self.replace_live_context();
        self.live.dm.merge_data(
            json!({
                "action": { "mode": "real" },
                "_tqsdk_backtest": null
            }),
            true,
            true,
        );
        self.init_market().await
    }

    pub(super) async fn close_market(&self) -> Result<()> {
        self.live.set_active(false);
        self.live.market_state.close();
        if let Some(ws) = &self.live.quotes_ws {
            ws.close().await?;
        }
        if let Some(ins) = &self.live.ins_api {
            ins.close().await?;
        }
        Ok(())
    }
}

async fn preload_symbol_info_into_dm(
    stock: bool,
    ins_url: &str,
    headers: HeaderMap,
    dm: &Arc<DataManager>,
    market_state: Option<&Arc<MarketDataState>>,
) -> Result<()> {
    if stock {
        return Ok(());
    }

    let url = ins_url.trim();
    if url.is_empty() {
        return Ok(());
    }

    let content = crate::utils::fetch_json_with_headers(url, headers).await?;
    let symbols = content
        .as_object()
        .ok_or_else(|| TqError::ParseError("合约信息格式错误，应为对象".to_string()))?;

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
    let quote_symbols = quotes.keys().cloned().collect::<Vec<_>>();
    payload.insert("quotes".to_string(), Value::Object(quotes));
    dm.merge_data(Value::Object(payload), true, true);
    if let Some(market_state) = market_state {
        for symbol in quote_symbols {
            if let Ok(mut quote) = dm.get_quote_data(&symbol) {
                quote.update_change();
                market_state.update_quote(symbol.into(), quote).await;
            }
        }
    }
    Ok(())
}

fn parse_instrument_metadata(symbol: &str, value: &Value) -> InstrumentMetadata {
    InstrumentMetadata {
        symbol: symbol.to_string(),
        exchange_id: value
            .get("exchange_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        instrument_id: value
            .get("instrument_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        class: value
            .get("class")
            .or_else(|| value.get("ins_class"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        underlying_symbol: value
            .get("underlying_symbol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        option_class: value
            .get("option_class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        strike_price: value.get("strike_price").and_then(Value::as_f64).unwrap_or_default(),
        trading_time: value.get("trading_time").cloned().unwrap_or(Value::Null),
        price_tick: value.get("price_tick").and_then(Value::as_f64).unwrap_or_default(),
        volume_multiple: numeric_i32_field(value, "volume_multiple"),
        margin: value.get("margin").and_then(Value::as_f64).unwrap_or_default(),
        commission: value.get("commission").and_then(Value::as_f64).unwrap_or_default(),
        open_min_market_order_volume: numeric_i32_field(value, "open_min_market_order_volume"),
        open_min_limit_order_volume: numeric_i32_field(value, "open_min_limit_order_volume"),
    }
}

fn numeric_i32_field(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(|field| {
            field.as_i64().or_else(|| {
                field
                    .as_f64()
                    .filter(|number| number.is_finite())
                    .map(|number| number as i64)
            })
        })
        .unwrap_or_default() as i32
}

fn build_preloaded_quote(source: &Map<String, Value>) -> Map<String, Value> {
    let mut quote = Map::new();

    quote.insert("ins_class".to_string(), source_string(source, "class"));
    quote.insert("instrument_id".to_string(), source_string(source, "instrument_id"));
    quote.insert("exchange_id".to_string(), source_string(source, "exchange_id"));
    quote.insert(
        "margin".to_string(),
        source.get("margin").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "commission".to_string(),
        source.get("commission").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "price_tick".to_string(),
        source.get("price_tick").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "price_decs".to_string(),
        source.get("price_decs").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "volume_multiple".to_string(),
        source.get("volume_multiple").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "max_limit_order_volume".to_string(),
        source
            .get("max_limit_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "max_market_order_volume".to_string(),
        source
            .get("max_market_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "min_limit_order_volume".to_string(),
        source
            .get("min_limit_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "min_market_order_volume".to_string(),
        source
            .get("min_market_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "open_max_limit_order_volume".to_string(),
        source
            .get("open_max_limit_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "open_max_market_order_volume".to_string(),
        source
            .get("open_max_market_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "open_min_limit_order_volume".to_string(),
        source
            .get("open_min_limit_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "open_min_market_order_volume".to_string(),
        source
            .get("open_min_market_order_volume")
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    quote.insert(
        "underlying_symbol".to_string(),
        source_string(source, "underlying_symbol"),
    );
    quote.insert(
        "strike_price".to_string(),
        source.get("strike_price").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "expired".to_string(),
        source.get("expired").cloned().unwrap_or(Value::Bool(false)),
    );
    quote.insert(
        "trading_time".to_string(),
        source.get("trading_time").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "expire_datetime".to_string(),
        source.get("expire_datetime").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "delivery_month".to_string(),
        source.get("delivery_month").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "delivery_year".to_string(),
        source.get("delivery_year").cloned().unwrap_or(Value::Null),
    );
    quote.insert("option_class".to_string(), source_string(source, "option_class"));
    quote.insert("product_id".to_string(), source_string(source, "product_id"));

    quote
}

fn source_string(source: &Map<String, Value>, key: &str) -> Value {
    Value::String(source.get(key).and_then(Value::as_str).unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_instrument_metadata;
    use serde_json::json;

    #[test]
    fn parse_instrument_metadata_accepts_float_encoded_integer_fields() {
        let metadata = parse_instrument_metadata(
            "SHFE.au2606",
            &json!({
                "instrument_id": "SHFE.au2606",
                "exchange_id": "SHFE",
                "class": "FUTURE",
                "volume_multiple": 1000.0,
                "open_min_market_order_volume": 1.0,
                "open_min_limit_order_volume": 2.0,
                "price_tick": 0.02
            }),
        );

        assert_eq!(metadata.volume_multiple, 1000);
        assert_eq!(metadata.open_min_market_order_volume, 1);
        assert_eq!(metadata.open_min_limit_order_volume, 2);
        assert_eq!(metadata.class, "FUTURE");
    }
}
