use super::{Client, ClientBuilder, ClientConfig, TradeSessionOptions};
use crate::auth::Authenticator;
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::quote::QuoteSubscription;
use crate::replay::{ReplayConfig, ReplaySession};
use crate::runtime::{LiveExecutionAdapter, LiveMarketAdapter, RuntimeMode, TqRuntime};
use crate::series::SeriesAPI;
use crate::trade_session::TradeSession;
use crate::types::{EdbIndexData, Kline, SymbolRanking, SymbolSettlement, TradingCalendarDay, TradingStatus};
use crate::websocket::WebSocketConfig;
use async_channel::Receiver;
use chrono::{DateTime, NaiveDate, Utc};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;

impl Client {
    /// 创建新的客户端（使用默认配置）
    ///
    /// 这是一个便捷方法，等同于：
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::client::ClientBuilder;
    /// # async fn example() -> Result<()> {
    /// let client = ClientBuilder::new("username", "password")
    ///     .config(ClientConfig::default())
    ///     .endpoints(EndpointConfig::from_env())
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// 如需更多配置选项，请使用 `ClientBuilder`
    pub async fn new(username: &str, password: &str, config: ClientConfig) -> Result<Self> {
        ClientBuilder::new(username, password).config(config).build().await
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
    ///     .endpoints(EndpointConfig::from_env())
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder(username: impl Into<String>, password: impl Into<String>) -> ClientBuilder {
        ClientBuilder::new(username, password)
    }

    pub fn ins_url(&self) -> &str {
        &self.endpoints.ins_url
    }

    /// 设置认证器
    pub async fn set_auth<A: Authenticator + 'static>(&mut self, auth: A) {
        self.auth = Arc::new(RwLock::new(auth));
    }

    /// 获取认证器的只读引用
    pub async fn get_auth(&self) -> tokio::sync::RwLockReadGuard<'_, dyn Authenticator> {
        self.auth.read().await
    }

    pub async fn get_auth_id(&self) -> String {
        let auth = self.auth.read().await;
        auth.get_auth_id().to_string()
    }

    /// 获取 Series API
    pub fn series(&self) -> Result<Arc<SeriesAPI>> {
        if !self.market_active.load(Ordering::SeqCst) {
            return Err(TqError::InternalError("Series API 未初始化或已关闭".to_string()));
        }
        self.series_api
            .clone()
            .ok_or_else(|| TqError::InternalError("Series API 未初始化".to_string()))
    }

    /// 获取合约查询 API
    pub fn ins(&self) -> Result<Arc<InsAPI>> {
        if !self.market_active.load(Ordering::SeqCst) {
            return Err(TqError::InternalError("合约查询 API 未初始化或已关闭".to_string()));
        }
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
        self.ins()?.query_cont_quotes(exchange_id, product_id, has_night).await
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

    #[expect(clippy::too_many_arguments, reason = "对外 API 保持与 InsAPI/Python SDK 参数一致")]
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
            .query_all_level_finance_options(underlying_symbol, underlying_price, option_class, nearbys, has_a)
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
        self.ins()?.query_symbol_settlement(symbols, days, start_dt).await
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

    /// 一次性获取按时间窗口的历史 K 线快照（不随行情更新），语义为 `[start_dt, end_dt)`。
    pub async fn get_kline_data_series(
        &self,
        symbol: &str,
        duration: StdDuration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        self.series()?
            .kline_data_series(symbol, duration, start_dt, end_dt)
            .await
    }

    /// 一次性获取按时间窗口的历史 Tick 快照（不随行情更新），语义为 `[start_dt, end_dt)`。
    pub async fn get_tick_data_series(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<crate::types::Tick>> {
        self.series()?.tick_data_series(symbol, start_dt, end_dt).await
    }

    /// 订阅 Quote。
    pub async fn subscribe_quote(&self, symbols: &[&str]) -> Result<Arc<QuoteSubscription>> {
        if !self.market_active.load(Ordering::SeqCst) || self.quotes_ws.is_none() {
            return Err(TqError::InternalError("行情 WebSocket 未初始化或已关闭".to_string()));
        }
        {
            let auth = self.auth.read().await;
            auth.has_md_grants(symbols)?
        }
        let symbol_list: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        Ok(Arc::new(QuoteSubscription::new(
            Arc::clone(&self.dm),
            self.quotes_ws.as_ref().unwrap().clone(),
            symbol_list,
        )))
    }

    /// 创建交易会话（不自动连接）
    pub async fn create_trade_session(&self, broker: &str, user_id: &str, password: &str) -> Result<Arc<TradeSession>> {
        self.create_trade_session_with_options(broker, user_id, password, TradeSessionOptions::default())
            .await
    }

    /// 创建交易会话（支持覆盖交易地址）
    ///
    /// 优先级：`TradeSessionOptions.td_url_override` > `ClientBuilder::td_url` /
    /// `EndpointConfig.td_url` > `TQ_TD_URL` > 鉴权返回的默认交易地址。
    pub async fn create_trade_session_with_options(
        &self,
        broker: &str,
        user_id: &str,
        password: &str,
        options: TradeSessionOptions,
    ) -> Result<Arc<TradeSession>> {
        let auth = self.auth.read().await;
        let td_url = if let Some(url) = options
            .td_url_override
            .filter(|url| !url.trim().is_empty())
            .or_else(|| self.endpoints.td_url.clone())
        {
            url
        } else {
            auth.get_td_url(broker, user_id).await?.url
        };

        let ws_config = WebSocketConfig {
            headers: auth.base_header(),
            message_queue_capacity: self.config.message_queue_capacity,
            message_backlog_warn_step: self.config.message_backlog_warn_step,
            message_batch_max: self.config.message_batch_max,
            ..Default::default()
        };
        drop(auth);

        let session = Arc::new(TradeSession::new(
            broker.to_string(),
            user_id.to_string(),
            password.to_string(),
            Arc::clone(&self.dm),
            td_url,
            ws_config,
        ));

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

    pub fn into_runtime(self) -> Arc<TqRuntime> {
        let market = Arc::new(LiveMarketAdapter::new(Arc::clone(&self.dm)));
        let execution = Arc::new(LiveExecutionAdapter::new(Arc::clone(&self.trade_sessions)));
        Arc::new(TqRuntime::new(RuntimeMode::Live, market, execution))
    }

    pub async fn create_backtest_session(&mut self, config: ReplayConfig) -> Result<ReplaySession> {
        let source = self.build_historical_source().await?;
        ReplaySession::from_source(config, source).await
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
}
