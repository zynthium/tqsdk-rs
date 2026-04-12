//! 数据结构定义
//!
//! 定义所有 TQSDK 使用的数据结构，包括：
//! - Quote: 行情报价
//! - Kline: K线数据
//! - Tick: Tick数据
//! - Account: 账户信息
//! - Position: 持仓信息
//! - Order: 委托单
//! - Trade: 成交记录
//! - Chart: 图表状态
//! - SeriesData: 序列数据

mod helpers;
mod market;
mod query;
mod rangeset;
mod series;
mod trading;

#[cfg(test)]
mod tests;

pub use market::{Chart, ChartInfo, Kline, Quote, Tick};
pub use query::{EdbIndexData, SymbolRanking, SymbolSettlement, TradingCalendarDay, TradingStatus};
pub use rangeset::{Range, RangeSet, rangeset_difference, rangeset_intersection, rangeset_merge, rangeset_union};
pub use series::{
    AlignedKlineSet, KlineMetadata, KlineSeriesData, MultiKlineSeriesData, SeriesData, SeriesOptions, SeriesSnapshot,
    TickSeriesData, UpdateInfo,
};
pub use trading::{
    Account, DIRECTION_BUY, DIRECTION_SELL, InsertOrderOptions, InsertOrderRequest, Notification, NotifyEvent,
    OFFSET_CLOSE, OFFSET_CLOSETODAY, OFFSET_OPEN, ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, PRICE_TYPE_ANY,
    PRICE_TYPE_BEST, PRICE_TYPE_FIVELEVEL, PRICE_TYPE_LIMIT, Position, PositionUpdate, TIME_CONDITION_GFD,
    TIME_CONDITION_IOC, Trade, VOLUME_CONDITION_ALL, VOLUME_CONDITION_ANY,
};
