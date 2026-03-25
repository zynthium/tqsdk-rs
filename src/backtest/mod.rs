//! 回测控制与时间推进接口

mod core;
mod parsing;

use crate::datamanager::DataManager;
use crate::websocket::TqQuoteWebsocket;
use async_channel::Receiver;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
/// 回测起止时间配置
pub struct BacktestConfig {
    /// 回测开始时间（UTC）
    pub start_dt: DateTime<Utc>,
    /// 回测结束时间（UTC）
    pub end_dt: DateTime<Utc>,
}

impl BacktestConfig {
    /// 创建回测配置
    pub fn new(start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Self {
        BacktestConfig { start_dt, end_dt }
    }
}

#[derive(Debug, Clone)]
/// 回测时间状态（纳秒时间戳）
pub struct BacktestTime {
    /// 回测开始时间戳（纳秒）
    pub start_dt: i64,
    /// 回测结束时间戳（纳秒）
    pub end_dt: i64,
    /// 当前回测时间戳（纳秒）
    pub current_dt: i64,
}

#[derive(Debug, Clone)]
/// 回测推进事件
pub enum BacktestEvent {
    /// 回测推进到下一个时间点
    Tick { current_dt: DateTime<Utc> },
    /// 回测已完成
    Finished { current_dt: DateTime<Utc> },
}

/// 回测控制句柄
pub struct BacktestHandle {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    rx: Receiver<()>,
    start_dt: i64,
    end_dt: i64,
    current_dt: Arc<RwLock<i64>>,
}
