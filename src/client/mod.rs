//! 客户端模块
//!
//! 统一的客户端入口

mod builder;
mod endpoints;
mod facade;
mod market;

#[cfg(test)]
mod tests;

use crate::auth::Authenticator;
use crate::datamanager::DataManager;
use crate::errors::Result;
use crate::ins::InsAPI;
use crate::marketdata::{KlineRef, MarketDataState, MarketDataUpdates, QuoteRef, TickRef, TqApi};
use crate::series::SeriesAPI;
use crate::trade_session::TradeSession;
use crate::websocket::TqQuoteWebsocket;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::RwLock as AsyncRwLock;

pub use endpoints::{EndpointConfig, TradeSessionOptions};

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
    pub message_queue_capacity: usize,
    pub message_backlog_warn_step: usize,
    pub message_batch_max: usize,
    pub series_disk_cache_enabled: bool,
    pub series_disk_cache_max_bytes: Option<u64>,
    pub series_disk_cache_retention_days: Option<u64>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            view_width: 10000,
            development: false,
            stock: true,
            message_queue_capacity: 2048,
            message_backlog_warn_step: 1024,
            message_batch_max: 32,
            series_disk_cache_enabled: false,
            series_disk_cache_max_bytes: None,
            series_disk_cache_retention_days: None,
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
    endpoints: EndpointConfig,
    auth: Option<Arc<AsyncRwLock<dyn Authenticator>>>,
    trade_session_configs: Vec<PendingTradeSessionConfig>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingTradeSessionConfig {
    pub broker: String,
    pub user_id: String,
    pub password: String,
    pub options: endpoints::TradeSessionOptions,
}

/// 客户端
pub struct Client {
    #[allow(dead_code)]
    username: String,
    config: ClientConfig,
    endpoints: EndpointConfig,
    auth: Arc<AsyncRwLock<dyn Authenticator>>,
    dm: Arc<DataManager>,
    market_state: Arc<MarketDataState>,
    live_api: TqApi,
    quotes_ws: Option<Arc<TqQuoteWebsocket>>,
    series_api: Option<Arc<SeriesAPI>>,
    ins_api: Option<Arc<InsAPI>>,
    market_active: AtomicBool,
    trade_sessions: Arc<std::sync::RwLock<HashMap<String, Arc<TradeSession>>>>,
}

impl Client {
    pub fn quote(&self, symbol: impl Into<crate::marketdata::SymbolId>) -> QuoteRef {
        self.live_api.quote(symbol)
    }

    pub fn kline_ref(&self, symbol: impl Into<crate::marketdata::SymbolId>, duration: Duration) -> KlineRef {
        self.live_api.kline(symbol, duration)
    }

    pub fn tick_ref(&self, symbol: impl Into<crate::marketdata::SymbolId>) -> TickRef {
        self.live_api.tick(symbol)
    }

    pub async fn wait_update(&self) -> Result<()> {
        self.live_api.wait_update().await
    }

    pub async fn wait_update_and_drain(&self) -> Result<MarketDataUpdates> {
        self.live_api.wait_update_and_drain().await
    }
}
