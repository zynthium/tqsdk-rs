use super::{Client, ClientBuilder, ClientConfig, TradeSessionOptions};
use crate::auth::BrokerInfo;
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::download::{DataDownloadOptions, DataDownloadRequest, DataDownloadWriter, DataDownloader};
use crate::errors::{Result, TqError};
use crate::ins::InsAPI;
use crate::quote::QuoteSubscription;
use crate::replay::{ReplayConfig, ReplaySession};
use crate::runtime::{LiveExecutionAdapter, LiveMarketAdapter, RuntimeMode, TqRuntime};
use crate::series::{KlineSymbols, SeriesAPI, SeriesSubscription};
use crate::trade_session::{TradeLoginOptions, TradeSession};
use crate::types::{EdbIndexData, Kline, SymbolRanking, SymbolSettlement, TradingCalendarDay, TradingStatus};
use crate::websocket::WebSocketConfig;
use async_channel::Receiver;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn urlsafe_base64_encode(input: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8) | (bytes[index + 2] as u32);
        output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        output.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }
    match bytes.len() - index {
        1 => {
            let chunk = (bytes[index] as u32) << 16;
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            output.push('=');
            output.push('=');
        }
        2 => {
            let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
            output.push('=');
        }
        _ => {}
    }
    output
}

fn rewrite_sm_td_url(url: &str, sm_type: &str, sm_config: &str, account_id: &str, password: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let split_idx = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..split_idx];
    let suffix = &rest[split_idx..];
    format!(
        "{}://{}/{}/{}/{}{}",
        sm_type,
        authority,
        sm_config,
        urlsafe_base64_encode(account_id),
        urlsafe_base64_encode(password),
        suffix
    )
}

fn resolve_trade_td_url(
    broker_info: &BrokerInfo,
    user_id: &str,
    password: &str,
    options: &TradeSessionOptions,
) -> String {
    if !options.use_sm {
        return broker_info.url.clone();
    }
    let Some(sm_type) = broker_info.smtype.as_deref().filter(|value| !value.is_empty()) else {
        return broker_info.url.clone();
    };
    let Some(sm_config) = broker_info.smconfig.as_deref().filter(|value| !value.is_empty()) else {
        return broker_info.url.clone();
    };
    rewrite_sm_td_url(&broker_info.url, sm_type, sm_config, user_id, password)
}

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

    pub async fn has_feature(&self, feature: &str) -> bool {
        self.auth.read().await.has_feature(feature)
    }

    pub async fn check_md_grants(&self, symbols: &[&str]) -> Result<()> {
        self.auth.read().await.has_md_grants(symbols)
    }

    pub async fn auth_id(&self) -> String {
        let auth = self.auth.read().await;
        auth.get_auth_id().to_string()
    }

    fn series_api(&self) -> Result<Arc<SeriesAPI>> {
        if self.live.market_state.is_closed() {
            return Err(TqError::client_closed("Series API"));
        }
        if !self.live.is_active() {
            return Err(TqError::market_not_initialized("Series API"));
        }
        self.live
            .series_api
            .clone()
            .ok_or_else(|| TqError::market_not_initialized("Series API"))
    }

    fn ins_api(&self) -> Result<Arc<InsAPI>> {
        if self.live.market_state.is_closed() {
            return Err(TqError::client_closed("合约查询 API"));
        }
        if !self.live.is_active() {
            return Err(TqError::market_not_initialized("合约查询 API"));
        }
        self.live
            .ins_api
            .clone()
            .ok_or_else(|| TqError::market_not_initialized("合约查询 API"))
    }

    pub async fn query_graphql(&self, query: &str, variables: Option<serde_json::Value>) -> Result<serde_json::Value> {
        self.ins_api()?.query_graphql(query, variables).await
    }

    pub async fn query_quotes(
        &self,
        ins_class: Option<&str>,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        expired: Option<bool>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        self.ins_api()?
            .query_quotes(ins_class, exchange_id, product_id, expired, has_night)
            .await
    }

    pub async fn query_cont_quotes(
        &self,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        self.ins_api()?
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
        self.ins_api()?
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
        self.ins_api()?
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
        self.ins_api()?
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
        self.ins_api()?
            .query_all_level_finance_options(underlying_symbol, underlying_price, option_class, nearbys, has_a)
            .await
    }

    pub async fn query_symbol_info(&self, symbols: &[&str]) -> Result<Vec<serde_json::Value>> {
        self.ins_api()?.query_symbol_info(symbols).await
    }

    pub async fn query_symbol_settlement(
        &self,
        symbols: &[&str],
        days: i32,
        start_dt: Option<NaiveDate>,
    ) -> Result<Vec<SymbolSettlement>> {
        self.ins_api()?.query_symbol_settlement(symbols, days, start_dt).await
    }

    pub async fn query_symbol_ranking(
        &self,
        symbol: &str,
        ranking_type: &str,
        days: i32,
        start_dt: Option<NaiveDate>,
        broker: Option<&str>,
    ) -> Result<Vec<SymbolRanking>> {
        self.ins_api()?
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
        self.ins_api()?.query_edb_data(ids, n, align, fill).await
    }

    pub async fn get_trading_calendar(
        &self,
        start_dt: NaiveDate,
        end_dt: NaiveDate,
    ) -> Result<Vec<TradingCalendarDay>> {
        self.ins_api()?.get_trading_calendar(start_dt, end_dt).await
    }

    pub async fn get_trading_status(&self, symbol: &str) -> Result<Receiver<TradingStatus>> {
        self.ins_api()?.get_trading_status(symbol).await
    }

    /// 订阅 K 线 serial（单合约/多合约对齐）。
    pub async fn get_kline_serial<T>(
        &self,
        symbols: T,
        duration: StdDuration,
        data_length: usize,
    ) -> Result<Arc<SeriesSubscription>>
    where
        T: Into<KlineSymbols>,
    {
        self.series_api()?
            .get_kline_serial(symbols, duration, data_length)
            .await
    }

    /// 订阅 Tick serial。
    pub async fn get_tick_serial(&self, symbol: &str, data_length: usize) -> Result<Arc<SeriesSubscription>> {
        self.series_api()?.get_tick_serial(symbol, data_length).await
    }

    /// 一次性获取按时间窗口的历史 K 线快照（不随行情更新），语义为 `[start_dt, end_dt)`。
    pub async fn get_kline_data_series(
        &self,
        symbol: &str,
        duration: StdDuration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        self.series_api()?
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
        self.series_api()?.tick_data_series(symbol, start_dt, end_dt).await
    }

    /// 启动后台历史数据下载任务，并将结果写入 CSV。
    pub fn spawn_data_downloader(&self, request: DataDownloadRequest) -> Result<DataDownloader> {
        DataDownloader::spawn(self.series_api()?, request)
    }

    /// 启动后台历史数据下载任务，并应用下载选项。
    pub fn spawn_data_downloader_with_options(
        &self,
        request: DataDownloadRequest,
        options: DataDownloadOptions,
    ) -> Result<DataDownloader> {
        DataDownloader::spawn_with_options(self.series_api()?, request, options)
    }

    /// 启动后台历史数据下载任务，并将结果流式写入自定义 writer。
    pub fn spawn_data_downloader_to_writer(
        &self,
        request: DataDownloadRequest,
        writer: DataDownloadWriter,
        options: DataDownloadOptions,
    ) -> Result<DataDownloader> {
        DataDownloader::spawn_to_writer(self.series_api()?, request, writer, options)
    }

    /// 订阅 Quote。
    pub async fn subscribe_quote(&self, symbols: &[&str]) -> Result<Arc<QuoteSubscription>> {
        if self.live.market_state.is_closed() {
            return Err(TqError::client_closed("quote 订阅"));
        }
        if !self.live.is_active() || self.live.quotes_ws.is_none() {
            return Err(TqError::market_not_initialized("quote 订阅"));
        }
        {
            let auth = self.auth.read().await;
            auth.has_md_grants(symbols)?
        }
        let symbol_list: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        Ok(Arc::new(
            QuoteSubscription::new(self.live.quotes_ws.as_ref().unwrap().clone(), symbol_list).await?,
        ))
    }

    /// 创建交易会话（不自动连接）
    pub async fn create_trade_session(&self, broker: &str, user_id: &str, password: &str) -> Result<Arc<TradeSession>> {
        self.create_trade_session_with_options_and_login(
            broker,
            user_id,
            password,
            TradeSessionOptions::default(),
            TradeLoginOptions::default(),
        )
        .await
    }

    pub async fn create_trade_session_with_login_options(
        &self,
        broker: &str,
        user_id: &str,
        password: &str,
        login_options: TradeLoginOptions,
    ) -> Result<Arc<TradeSession>> {
        self.create_trade_session_with_options_and_login(
            broker,
            user_id,
            password,
            TradeSessionOptions::default(),
            login_options,
        )
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
        self.create_trade_session_with_options_and_login(
            broker,
            user_id,
            password,
            options,
            TradeLoginOptions::default(),
        )
        .await
    }

    /// 创建交易会话（支持覆盖交易地址并扩展登录报文）
    pub async fn create_trade_session_with_options_and_login(
        &self,
        broker: &str,
        user_id: &str,
        password: &str,
        options: TradeSessionOptions,
        login_options: TradeLoginOptions,
    ) -> Result<Arc<TradeSession>> {
        let auth = self.auth.read().await;
        let td_url = if let Some(url) = options
            .td_url_override
            .as_ref()
            .filter(|url| !url.trim().is_empty())
            .cloned()
            .or_else(|| self.endpoints.td_url.clone())
        {
            url
        } else {
            let broker_info = auth.get_td_url(broker, user_id).await?;
            resolve_trade_td_url(&broker_info, user_id, password, &options)
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
            Arc::new(DataManager::new(
                HashMap::new(),
                DataManagerConfig {
                    default_view_width: self.config.view_width,
                    enable_auto_cleanup: true,
                    ..DataManagerConfig::default()
                },
            )),
            td_url,
            ws_config,
            options.reliable_events_max_retained,
            login_options,
        ));

        let key = format!("{}:{}", broker, user_id);
        let mut sessions = self.trade_sessions.write().unwrap();
        sessions.insert(key, Arc::clone(&session));

        Ok(session)
    }

    pub fn into_runtime(self) -> Arc<TqRuntime> {
        let market = Arc::new(LiveMarketAdapter::new(
            Arc::clone(&self.live.dm),
            Arc::clone(&self.live.market_state),
            self.live.quotes_ws.clone(),
            Arc::clone(&self.auth),
            self.config.clone(),
            self.endpoints.clone(),
        ));
        let execution = Arc::new(LiveExecutionAdapter::new(Arc::clone(&self.trade_sessions)));
        Arc::new(TqRuntime::new(RuntimeMode::Live, market, execution))
    }

    pub async fn create_backtest_session(&self, config: ReplayConfig) -> Result<ReplaySession> {
        let source = self.build_historical_source().await?;
        ReplaySession::from_source(config, source).await
    }

    /// 关闭客户端
    pub async fn close(&self) -> Result<()> {
        self.close_market().await?;

        let sessions = self
            .trade_sessions
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for trader in sessions {
            trader.close().await?;
        }

        Ok(())
    }
}
