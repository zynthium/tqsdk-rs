//! 客户端模块
//!
//! 统一的客户端入口

use crate::auth::{Authenticator, TqAuth};
use crate::backtest::{BacktestConfig, BacktestHandle};
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::quote::QuoteSubscription;
use crate::series::SeriesAPI;
use crate::trade_session::TradeSession;
use crate::utils::fetch_json_with_headers;
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket, WebSocketConfig};
use crate::types::{EdbIndexData, SymbolRanking, SymbolSettlement, TradingCalendarDay, TradingStatus};
use async_channel::Receiver;
use chrono::NaiveDate;
use reqwest::header::HeaderMap;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub const TQ_INS_URL_DEFAULT: &str = "https://openmd.shinnytech.com/t/md/symbols/latest.json";

fn default_ins_url() -> String {
    std::env::var("TQ_INS_URL").unwrap_or_else(|_| TQ_INS_URL_DEFAULT.to_string())
}

/// 客户端配置
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// 日志级别
    pub log_level: String,
    /// 默认视图宽度
    pub view_width: usize,
    /// 开发模式
    pub development: bool,
    pub stock: bool,
    pub ins_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            log_level: "info".to_string(),
            view_width: 10000,
            development: false,
            stock: true,
            ins_url: default_ins_url(),
        }
    }
}

/// 客户端选项
pub type ClientOption = Box<dyn Fn(&mut ClientConfig)>;

/// 客户端构建器
pub struct ClientBuilder {
    username: String,
    password: String,
    config: ClientConfig,
    auth: Option<Arc<RwLock<dyn Authenticator>>>,
}

impl ClientBuilder {
    /// 创建新的客户端构建器
    ///
    /// # 参数
    ///
    /// * `username` - 用户名
    /// * `password` - 密码
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::client::ClientBuilder;
    /// # async fn example() -> Result<()> {
    /// let client = ClientBuilder::new("username", "password")
    ///     .log_level("debug")
    ///     .view_width(5000)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        ClientBuilder {
            username: username.into(),
            password: password.into(),
            config: ClientConfig::default(),
            auth: None,
        }
    }

    /// 设置日志级别
    pub fn log_level(mut self, level: impl Into<String>) -> Self {
        self.config.log_level = level.into();
        self
    }

    /// 设置默认视图宽度
    pub fn view_width(mut self, width: usize) -> Self {
        self.config.view_width = width;
        self
    }

    /// 设置开发模式
    pub fn development(mut self, dev: bool) -> Self {
        self.config.development = dev;
        self
    }

    pub fn ins_url(mut self, url: impl Into<String>) -> Self {
        self.config.ins_url = url.into();
        self
    }

    /// 设置完整配置
    pub fn config(mut self, config: ClientConfig) -> Self {
        self.config = config;
        self
    }

    /// 使用自定义认证器（高级用法）
    ///
    /// 如果不设置，将使用默认的 TqAuth
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::auth::TqAuth;
    /// # async fn example() -> Result<()> {
    /// let mut auth = TqAuth::new("username".to_string(), "password".to_string());
    /// auth.login().await?;
    ///
    /// let client = Client::builder("username", "password")
    ///     .auth(auth)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn auth<A: Authenticator + 'static>(mut self, auth: A) -> Self {
        self.auth = Some(Arc::new(RwLock::new(auth)));
        self
    }

    /// 构建客户端
    ///
    /// # 错误
    ///
    /// 如果认证失败，返回错误
    pub async fn build(self) -> Result<Client> {
        // 初始化日志
        crate::logger::init_logger(&self.config.log_level, true);

        // 创建或使用提供的认证器
        let auth: Arc<RwLock<dyn Authenticator>> = if let Some(custom_auth) = self.auth {
            custom_auth
        } else {
            let mut auth = TqAuth::new(self.username.clone(), self.password.clone());
            auth.login().await?;
            Arc::new(RwLock::new(auth))
        };

        // 创建数据管理器
        let dm_config = DataManagerConfig {
            default_view_width: self.config.view_width,
            enable_auto_cleanup: true,
            ..DataManagerConfig::default()
        };
        let initial_data = HashMap::new();
        let dm = Arc::new(DataManager::new(initial_data, dm_config));

        Ok(Client {
            _username: self.username,
            _config: self.config,
            auth,
            dm,
            quotes_ws: None,
            series_api: None,
            ins_api: None,
            trade_sessions: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

/// 客户端
pub struct Client {
    _username: String,
    _config: ClientConfig,
    auth: Arc<RwLock<dyn Authenticator>>,
    dm: Arc<DataManager>,
    quotes_ws: Option<Arc<TqQuoteWebsocket>>,
    series_api: Option<Arc<SeriesAPI>>,
    ins_api: Option<Arc<InsAPI>>,
    trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>,
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

        let content = fetch_json_with_headers(url, headers).await?;
        let symbols = content
            .as_object()
            .ok_or_else(|| TqError::ParseError("合约信息格式错误，应为对象".to_string()))?;

        let mut quotes = Map::new();
        for (symbol, item) in symbols {
            let Some(source) = item.as_object() else {
                continue;
            };
            let mut quote = Map::new();

            quote.insert(
                "ins_class".to_string(),
                Value::String(
                    source
                        .get("class")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
            );
            quote.insert(
                "instrument_id".to_string(),
                Value::String(
                    source
                        .get("instrument_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
            );
            quote.insert(
                "exchange_id".to_string(),
                Value::String(
                    source
                        .get("exchange_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
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
                Value::String(
                    source
                        .get("underlying_symbol")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
            );
            quote.insert(
                "strike_price".to_string(),
                source.get("strike_price").cloned().unwrap_or(Value::Null),
            );
            quote.insert(
                "expired".to_string(),
                source.get("expired").cloned().unwrap_or(json!(false)),
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
            quote.insert(
                "option_class".to_string(),
                Value::String(
                    source
                        .get("option_class")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
            );
            quote.insert(
                "product_id".to_string(),
                Value::String(
                    source
                        .get("product_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
            );

            quotes.insert(symbol.clone(), Value::Object(quote));
        }

        if let Some(mut quote) = quotes.remove("CSI.000300") {
            if let Some(obj) = quote.as_object_mut() {
                obj.insert("exchange_id".to_string(), Value::String("SSE".to_string()));
            }
            quotes.insert("SSE.000300".to_string(), quote);
        }

        for (symbol, quote_value) in quotes.iter_mut() {
            if symbol.starts_with("CFFEX.IO") {
                if let Some(obj) = quote_value.as_object_mut() {
                    if obj.get("ins_class").and_then(Value::as_str) == Some("OPTION") {
                        obj.insert(
                            "underlying_symbol".to_string(),
                            Value::String("SSE.000300".to_string()),
                        );
                    }
                }
            }
        }

        let mut payload = Map::new();
        payload.insert("quotes".to_string(), Value::Object(quotes));
        self.dm.merge_data(Value::Object(payload), true, true);
        Ok(())
    }

    /// 创建新的客户端（使用默认配置）
    ///
    /// 这是一个便捷方法，等同于：
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::client::ClientBuilder;
    /// # async fn example() -> Result<()> {
    /// let client = ClientBuilder::new("username", "password")
    ///     .config(ClientConfig::default())
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// 如需更多配置选项，请使用 `ClientBuilder`
    pub async fn new(username: &str, password: &str, config: ClientConfig) -> Result<Self> {
        ClientBuilder::new(username, password)
            .config(config)
            .build()
            .await
    }

    /// 创建客户端构建器
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # async fn example() -> Result<()> {
    /// let client = Client::builder("username", "password")
    ///     .log_level("debug")
    ///     .view_width(5000)
    ///     .development(true)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder(username: impl Into<String>, password: impl Into<String>) -> ClientBuilder {
        ClientBuilder::new(username, password)
    }

    pub fn ins_url(&self) -> &str {
        &self._config.ins_url
    }

    /// 初始化行情功能
    pub async fn init_market(&mut self) -> Result<()> {
        let auth = self.auth.read().await;
        let md_url = auth.get_md_url(self._config.stock, false).await?;
        let headers = auth.base_header();
        let enable_trading_status = auth.has_feature("tq_trading_status");
        drop(auth);

        self.preload_symbol_info(headers.clone()).await?;

        let ws_config = WebSocketConfig {
            headers,
            ..Default::default()
        };

        let quotes_ws = Arc::new(TqQuoteWebsocket::new(
            md_url,
            Arc::clone(&self.dm),
            ws_config.clone(),
        ));

        quotes_ws.init(false).await?;

        self.quotes_ws = Some(Arc::clone(&quotes_ws));

        // 创建 SeriesAPI（传入 auth）
        let series_api = Arc::new(SeriesAPI::new(
            Arc::clone(&self.dm),
            quotes_ws,
            Arc::clone(&self.auth),
        ));
        self.series_api = Some(series_api);
        let trading_status_ws = if enable_trading_status {
            let ts_ws = Arc::new(TqTradingStatusWebsocket::new(
                "wss://trading-status.shinnytech.com/status".to_string(),
                Arc::clone(&self.dm),
                ws_config,
            ));
            ts_ws.init(false).await?;
            Some(ts_ws)
        } else {
            None
        };

        self.ins_api = Some(Arc::new(InsAPI::new(
            Arc::clone(&self.dm),
            self.quotes_ws.as_ref().unwrap().clone(),
            trading_status_ws,
            Arc::clone(&self.auth),
            self._config.stock,
        )));

        Ok(())
    }

    pub async fn init_market_backtest(&mut self, config: BacktestConfig) -> Result<BacktestHandle> {
        let auth = self.auth.read().await;
        let md_url = auth.get_md_url(self._config.stock, true).await?;
        let headers = auth.base_header();
        let enable_trading_status = auth.has_feature("tq_trading_status");
        drop(auth);

        self.preload_symbol_info(headers.clone()).await?;

        let ws_config = WebSocketConfig {
            headers,
            auto_peek: false,
            quote_subscribe_only_add: true,
            ..Default::default()
        };

        let quotes_ws = Arc::new(TqQuoteWebsocket::new(
            md_url,
            Arc::clone(&self.dm),
            ws_config.clone(),
        ));

        quotes_ws.init(false).await?;

        self.quotes_ws = Some(Arc::clone(&quotes_ws));

        let series_api = Arc::new(SeriesAPI::new(
            Arc::clone(&self.dm),
            Arc::clone(&quotes_ws),
            Arc::clone(&self.auth),
        ));
        self.series_api = Some(series_api);

        let trading_status_ws = if enable_trading_status {
            let mut status_config = ws_config.clone();
            status_config.auto_peek = true; // Status server likely needs auto-peek

            let ts_ws = Arc::new(TqTradingStatusWebsocket::new(
                "wss://trading-status.shinnytech.com/status".to_string(),
                Arc::clone(&self.dm),
                status_config,
            ));
            ts_ws.init(false).await?;
            Some(ts_ws)
        } else {
            None
        };

        self.ins_api = Some(Arc::new(InsAPI::new(
            Arc::clone(&self.dm),
            self.quotes_ws.as_ref().unwrap().clone(),
            trading_status_ws,
            Arc::clone(&self.auth),
            self._config.stock,
        )));

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

    /// 设置认证器
    ///
    /// 允许在运行时更换认证器，例如切换账号或更新 token
    ///
    /// # 注意
    ///
    /// - 更换认证器后，需要重新调用 `init_market()` 来使用新的认证信息
    /// - 已创建的 `SeriesAPI` 和 `TradeSession` 仍使用旧的认证器
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::auth::TqAuth;
    /// # async fn example() -> Result<()> {
    /// let mut client = Client::new("user1", "pass1", ClientConfig::default()).await?;
    ///
    /// // 切换到另一个账号
    /// let mut new_auth = TqAuth::new("user2".to_string(), "pass2".to_string());
    /// new_auth.login().await?;
    /// client.set_auth(new_auth).await;
    ///
    /// // 重新初始化行情功能
    /// client.init_market().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_auth<A: Authenticator + 'static>(&mut self, auth: A) {
        self.auth = Arc::new(RwLock::new(auth));
    }

    /// 获取认证器的只读引用
    ///
    /// 用于检查当前的认证状态或权限
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # async fn example(client: &Client) -> Result<()> {
    /// let auth = client.get_auth().await;
    /// if auth.has_feature("futr") {
    ///     println!("有期货权限");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_auth(&self) -> tokio::sync::RwLockReadGuard<'_, dyn Authenticator> {
        self.auth.read().await
    }

    pub async fn get_auth_id(&self) -> String {
        let auth = self.auth.read().await;
        auth.get_auth_id().to_string()
    }

    /// 获取 Series API
    pub fn series(&self) -> Result<Arc<SeriesAPI>> {
        self.series_api
            .clone()
            .ok_or_else(|| TqError::InternalError("Series API 未初始化".to_string()))
    }

    /// 获取合约查询 API
    pub fn ins(&self) -> Result<Arc<InsAPI>> {
        self.ins_api
            .clone()
            .ok_or_else(|| TqError::InternalError("合约查询 API 未初始化".to_string()))
    }

    pub async fn query_graphql(&self, query: &str, variables: Option<serde_json::Value>) -> Result<serde_json::Value> {
        self.ins()?.query_graphql(query, variables).await
    }

    pub async fn query_quotes(
        &self,
        ins_class: Option<&str>,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        expired: Option<bool>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        self.ins()?
            .query_quotes(ins_class, exchange_id, product_id, expired, has_night)
            .await
    }

    pub async fn query_cont_quotes(
        &self,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        self.ins()?
            .query_cont_quotes(exchange_id, product_id, has_night)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn query_options(
        &self,
        underlying_symbol: &str,
        option_class: Option<&str>,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        strike_price: Option<f64>,
        expired: Option<bool>,
        has_a: Option<bool>,
    ) -> Result<Vec<String>> {
        self.ins()?
            .query_options(
                underlying_symbol,
                option_class,
                exercise_year,
                exercise_month,
                strike_price,
                expired,
                has_a,
            )
            .await
    }

    pub async fn query_atm_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        price_level: &[i32],
        option_class: &str,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        has_a: Option<bool>,
    ) -> Result<Vec<Option<String>>> {
        self.ins()?
            .query_atm_options(
                underlying_symbol,
                underlying_price,
                price_level,
                option_class,
                exercise_year,
                exercise_month,
                has_a,
            )
            .await
    }

    pub async fn query_all_level_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        option_class: &str,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        has_a: Option<bool>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        self.ins()?
            .query_all_level_options(
                underlying_symbol,
                underlying_price,
                option_class,
                exercise_year,
                exercise_month,
                has_a,
            )
            .await
    }

    pub async fn query_all_level_finance_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        option_class: &str,
        nearbys: &[i32],
        has_a: Option<bool>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        self.ins()?
            .query_all_level_finance_options(
                underlying_symbol,
                underlying_price,
                option_class,
                nearbys,
                has_a,
            )
            .await
    }

    pub async fn query_symbol_info(&self, symbols: &[&str]) -> Result<Vec<serde_json::Value>> {
        self.ins()?.query_symbol_info(symbols).await
    }

    pub async fn query_symbol_settlement(
        &self,
        symbols: &[&str],
        days: i32,
        start_dt: Option<NaiveDate>,
    ) -> Result<Vec<SymbolSettlement>> {
        self.ins()?
            .query_symbol_settlement(symbols, days, start_dt)
            .await
    }

    pub async fn query_symbol_ranking(
        &self,
        symbol: &str,
        ranking_type: &str,
        days: i32,
        start_dt: Option<NaiveDate>,
        broker: Option<&str>,
    ) -> Result<Vec<SymbolRanking>> {
        self.ins()?
            .query_symbol_ranking(symbol, ranking_type, days, start_dt, broker)
            .await
    }

    pub async fn query_edb_data(
        &self,
        ids: &[i32],
        n: i32,
        align: Option<&str>,
        fill: Option<&str>,
    ) -> Result<Vec<EdbIndexData>> {
        self.ins()?.query_edb_data(ids, n, align, fill).await
    }

    pub async fn get_trading_calendar(
        &self,
        start_dt: NaiveDate,
        end_dt: NaiveDate,
    ) -> Result<Vec<TradingCalendarDay>> {
        self.ins()?.get_trading_calendar(start_dt, end_dt).await
    }

    pub async fn get_trading_status(&self, symbol: &str) -> Result<Receiver<TradingStatus>> {
        self.ins()?.get_trading_status(symbol).await
    }

    /// 订阅 Quote
    pub async fn subscribe_quote(&self, symbols: &[&str]) -> Result<Arc<QuoteSubscription>> {
        if self.quotes_ws.is_none() {
            return Err(TqError::InternalError(
                "行情 WebSocket 未初始化".to_string(),
            ));
        }
        {
            let auth = self.auth.read().await;
            auth.has_md_grants(symbols)?
        }
        let symbol_list: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        let qs = Arc::new(QuoteSubscription::new(
            Arc::clone(&self.dm),
            self.quotes_ws.as_ref().unwrap().clone(),
            symbol_list,
        ));

        // 启动订阅
        qs.start().await?;

        Ok(qs)
    }

    /// 创建交易会话（不自动连接）
    ///
    /// # 重要提示
    ///
    /// 由于 TradeSession 使用 broadcast 队列，建议按以下顺序使用：
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # async fn example() -> Result<()> {
    /// # let client = Client::new("user", "pass", ClientConfig::default()).await?;
    /// // 1. 创建会话（不连接）
    /// let session = client.create_trade_session("simnow", "user_id", "password").await?;
    ///
    /// // 2. 先注册回调或订阅 channel（避免丢失消息）
    /// session.on_account(|account| {
    ///     println!("账户: {}", account.balance);
    /// }).await;
    ///
    /// // 3. 最后连接
    /// session.connect().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # 参数
    ///
    /// * `broker` - 期货公司代码（如 "simnow"）
    /// * `user_id` - 用户账号
    /// * `password` - 密码
    ///
    /// # 返回
    ///
    /// 返回 `TradeSession` 实例，需要手动调用 `connect()` 连接
    pub async fn create_trade_session(
        &self,
        broker: &str,
        user_id: &str,
        password: &str,
    ) -> Result<Arc<TradeSession>> {
        // 获取交易服务器地址
        let auth = self.auth.read().await;
        let broker_info = auth.get_td_url(broker, user_id).await?;

        let ws_config = WebSocketConfig {
            headers: auth.base_header(),
            ..Default::default()
        };
        drop(auth);

        // 创建交易会话（不自动连接）
        let session = Arc::new(TradeSession::new(
            broker.to_string(),
            user_id.to_string(),
            password.to_string(),
            Arc::clone(&self.dm),
            broker_info.url,
            ws_config,
        ));

        // 保存会话
        let key = format!("{}:{}", broker, user_id);
        let mut sessions = self.trade_sessions.write().await;
        sessions.insert(key, Arc::clone(&session));

        Ok(session)
    }

    /// 注册交易会话
    pub async fn register_trade_session(&self, key: &str, session: Arc<TradeSession>) {
        let mut sessions = self.trade_sessions.write().await;
        sessions.insert(key.to_string(), session);
    }

    /// 获取交易会话
    pub async fn get_trade_session(&self, key: &str) -> Option<Arc<TradeSession>> {
        let sessions = self.trade_sessions.read().await;
        sessions.get(key).cloned()
    }

    /// 关闭客户端
    pub async fn close(&self) -> Result<()> {
        self.close_market().await?;

        let sessions = self.trade_sessions.read().await;
        for (_key, trader) in sessions.iter() {
            trader.close().await?;
        }

        Ok(())
    }

    async fn close_market(&self) -> Result<()> {
        if let Some(ws) = &self.quotes_ws {
            ws.close().await?;
        }
        if let Some(ins) = &self.ins_api {
            ins.close().await?;
        }
        Ok(())
    }
}
