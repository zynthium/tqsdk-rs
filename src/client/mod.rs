//! 客户端模块
//!
//! 统一的客户端入口

mod builder;
mod facade;
mod market;

#[cfg(test)]
mod tests;

use crate::auth::Authenticator;
use crate::datamanager::DataManager;
use crate::ins::InsAPI;
use crate::series::SeriesAPI;
use crate::trade_session::TradeSession;
use crate::websocket::TqQuoteWebsocket;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;

pub(crate) const TQ_INS_URL_DEFAULT: &str = "https://openmd.shinnytech.com/t/md/symbols/latest.json";

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
    pub message_queue_capacity: usize,
    pub message_backlog_warn_step: usize,
    pub message_batch_max: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            view_width: 10000,
            development: false,
            stock: true,
            ins_url: default_ins_url(),
            message_queue_capacity: 2048,
            message_backlog_warn_step: 1024,
            message_batch_max: 32,
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

/// 客户端
pub struct Client {
    #[allow(dead_code)]
    username: String,
    config: ClientConfig,
    auth: Arc<RwLock<dyn Authenticator>>,
    dm: Arc<DataManager>,
    quotes_ws: Option<Arc<TqQuoteWebsocket>>,
    series_api: Option<Arc<SeriesAPI>>,
    ins_api: Option<Arc<InsAPI>>,
    market_active: AtomicBool,
    trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>,
}
