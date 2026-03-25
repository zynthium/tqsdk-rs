use super::Client;
use crate::backtest::{BacktestConfig, BacktestHandle};
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::series::SeriesAPI;
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket, WebSocketConfig};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::Ordering;

struct MarketBootstrap {
    md_url: String,
    headers: HeaderMap,
    enable_trading_status: bool,
}

impl Client {
    async fn preload_symbol_info(&self, headers: HeaderMap) -> Result<()> {
        if self._config.stock {
            return Ok(());
        }

        let url = self._config.ins_url.trim();
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
                obj.insert(
                    "underlying_symbol".to_string(),
                    Value::String("SSE.000300".to_string()),
                );
            }
        }

        let mut payload = Map::new();
        payload.insert("quotes".to_string(), Value::Object(quotes));
        self.dm.merge_data(Value::Object(payload), true, true);
        Ok(())
    }

    async fn load_market_bootstrap(&self, backtest: bool) -> Result<MarketBootstrap> {
        let auth = self.auth.read().await;
        let md_url = auth.get_md_url(self._config.stock, backtest).await?;
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
            message_queue_capacity: self._config.message_queue_capacity,
            message_backlog_warn_step: self._config.message_backlog_warn_step,
            message_batch_max: self._config.message_batch_max,
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
            Arc::clone(&self.dm),
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
            Arc::clone(&self.dm),
            ws_config.clone(),
        ));
        quotes_ws.init(false).await?;

        self.quotes_ws = Some(Arc::clone(&quotes_ws));
        self.series_api = Some(Arc::new(SeriesAPI::new(
            Arc::clone(&self.dm),
            Arc::clone(&quotes_ws),
            Arc::clone(&self.auth),
        )));

        let trading_status_ws = self
            .build_trading_status_ws(bootstrap.enable_trading_status, &ws_config, backtest)
            .await?;

        self.ins_api = Some(Arc::new(InsAPI::new(
            Arc::clone(&self.dm),
            Arc::clone(&quotes_ws),
            trading_status_ws,
            Arc::clone(&self.auth),
            self._config.stock,
        )));
        self.market_active.store(true, Ordering::SeqCst);

        Ok(quotes_ws)
    }

    /// 初始化行情功能
    pub async fn init_market(&mut self) -> Result<()> {
        let _ = self.initialize_market_runtime(false).await?;
        Ok(())
    }

    pub async fn init_market_backtest(&mut self, config: BacktestConfig) -> Result<BacktestHandle> {
        let quotes_ws = self.initialize_market_runtime(true).await?;
        Ok(BacktestHandle::new(Arc::clone(&self.dm), quotes_ws, config))
    }

    pub async fn switch_to_live(&mut self) -> Result<()> {
        self.close_market().await?;
        self.dm.merge_data(
            json!({
                "action": { "mode": "real" },
                "_tqsdk_backtest": null
            }),
            true,
            true,
        );
        self.init_market().await
    }

    pub async fn switch_to_backtest(&mut self, config: BacktestConfig) -> Result<BacktestHandle> {
        self.close_market().await?;
        self.init_market_backtest(config).await
    }
    pub(super) async fn close_market(&self) -> Result<()> {
        self.market_active.store(false, Ordering::SeqCst);
        if let Some(ws) = &self.quotes_ws {
            ws.close().await?;
        }
        if let Some(ins) = &self.ins_api {
            ins.close().await?;
        }
        Ok(())
    }
}

fn build_preloaded_quote(source: &Map<String, Value>) -> Map<String, Value> {
    let mut quote = Map::new();

    quote.insert("ins_class".to_string(), source_string(source, "class"));
    quote.insert(
        "instrument_id".to_string(),
        source_string(source, "instrument_id"),
    );
    quote.insert(
        "exchange_id".to_string(),
        source_string(source, "exchange_id"),
    );
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
        source
            .get("volume_multiple")
            .cloned()
            .unwrap_or(Value::Null),
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
        source
            .get("expire_datetime")
            .cloned()
            .unwrap_or(Value::Null),
    );
    quote.insert(
        "delivery_month".to_string(),
        source.get("delivery_month").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "delivery_year".to_string(),
        source.get("delivery_year").cloned().unwrap_or(Value::Null),
    );
    quote.insert(
        "option_class".to_string(),
        source_string(source, "option_class"),
    );
    quote.insert(
        "product_id".to_string(),
        source_string(source, "product_id"),
    );

    quote
}

fn source_string(source: &Map<String, Value>, key: &str) -> Value {
    Value::String(
        source
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}
