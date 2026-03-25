use super::helpers::{default_currency, deserialize_f64_default};
use serde::{Deserialize, Serialize};

/// 账户资金信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub user_id: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub available: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub close_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub commission: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub ctp_available: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub ctp_balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub deposit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub float_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub frozen_commission: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub frozen_margin: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub frozen_premium: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub margin: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub market_value: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub pre_balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub premium: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub risk_ratio: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub static_balance: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub withdraw: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Account {
    pub fn curr_margin(&self) -> f64 {
        self.margin
    }

    pub fn _epoch(&self) -> Option<i64> {
        self.epoch
    }
}

/// 持仓信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(default)]
    pub volume_long_today: i64,
    #[serde(default)]
    pub volume_long_his: i64,
    #[serde(default)]
    pub volume_long: i64,
    #[serde(default)]
    pub volume_long_frozen_today: i64,
    #[serde(default)]
    pub volume_long_frozen_his: i64,
    #[serde(default)]
    pub volume_long_frozen: i64,
    #[serde(default)]
    pub volume_short_today: i64,
    #[serde(default)]
    pub volume_short_his: i64,
    #[serde(default)]
    pub volume_short: i64,
    #[serde(default)]
    pub volume_short_frozen_today: i64,
    #[serde(default)]
    pub volume_short_frozen_his: i64,
    #[serde(default)]
    pub volume_short_frozen: i64,
    #[serde(default)]
    pub volume_long_yd: i64,
    #[serde(default)]
    pub volume_short_yd: i64,
    #[serde(default)]
    pub pos_long_his: i64,
    #[serde(default)]
    pub pos_long_today: i64,
    #[serde(default)]
    pub pos_short_his: i64,
    #[serde(default)]
    pub pos_short_today: i64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub open_price_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub open_price_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub open_cost_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub open_cost_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_price_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_price_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_cost_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_cost_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub last_price: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub float_profit_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub float_profit_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub float_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_profit_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_profit_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub position_profit: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub margin_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub margin_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub margin: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub market_value_long: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub market_value_short: f64,
    #[serde(default, deserialize_with = "deserialize_f64_default")]
    pub market_value: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

/// 委托单信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    #[serde(default)]
    pub seqno: i64,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub offset: String,
    #[serde(default)]
    pub volume_orign: i64,
    #[serde(default)]
    pub price_type: String,
    #[serde(default)]
    pub limit_price: f64,
    #[serde(default)]
    pub time_condition: String,
    #[serde(default)]
    pub volume_condition: String,
    #[serde(default)]
    pub insert_date_time: i64,
    #[serde(default)]
    pub exchange_order_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub volume_left: i64,
    #[serde(default)]
    pub frozen_margin: f64,
    #[serde(default)]
    pub last_msg: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Order {
    pub fn volume(&self) -> i64 {
        self.volume_orign
    }

    pub fn price(&self) -> f64 {
        self.limit_price
    }
}

/// 成交记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    #[serde(default)]
    pub seqno: i64,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub trade_id: String,
    #[serde(default)]
    pub exchange_id: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub exchange_trade_id: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub offset: String,
    #[serde(default)]
    pub volume: i64,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub trade_date_time: i64,
    #[serde(default)]
    pub commission: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

/// 通知事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyEvent {
    pub code: String,
    pub level: String,
    pub r#type: String,
    pub content: String,
    pub bid: String,
    pub user_id: String,
}

/// 通知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub code: String,
    pub level: String,
    pub r#type: String,
    pub content: String,
    pub bid: String,
    pub user_id: String,
}

/// 持仓更新
#[derive(Debug, Clone)]
pub struct PositionUpdate {
    pub symbol: String,
    pub position: Position,
}

/// 下单请求
#[derive(Debug, Clone)]
pub struct InsertOrderRequest {
    pub symbol: String,
    pub exchange_id: Option<String>,
    pub instrument_id: Option<String>,
    pub direction: String,
    pub offset: String,
    pub price_type: String,
    pub limit_price: f64,
    pub volume: i64,
}

impl InsertOrderRequest {
    pub fn get_exchange_id(&self) -> String {
        if let Some(ref exchange) = self.exchange_id {
            return exchange.clone();
        }
        if let Some(dot_pos) = self.symbol.find('.') {
            self.symbol[..dot_pos].to_string()
        } else {
            String::new()
        }
    }

    pub fn get_instrument_id(&self) -> String {
        if let Some(ref instrument) = self.instrument_id {
            return instrument.clone();
        }
        if let Some(dot_pos) = self.symbol.find('.') {
            self.symbol[dot_pos + 1..].to_string()
        } else {
            self.symbol.clone()
        }
    }
}

/// 方向 - 买入
pub const DIRECTION_BUY: &str = "BUY";
/// 方向 - 卖出
pub const DIRECTION_SELL: &str = "SELL";
/// 开平 - 开仓
pub const OFFSET_OPEN: &str = "OPEN";
/// 开平 - 平仓
pub const OFFSET_CLOSE: &str = "CLOSE";
/// 开平 - 平今
pub const OFFSET_CLOSETODAY: &str = "CLOSETODAY";
/// 价格类型 - 限价单
pub const PRICE_TYPE_LIMIT: &str = "LIMIT";
/// 价格类型 - 市价单
pub const PRICE_TYPE_ANY: &str = "ANY";
/// 订单状态 - 活动（未成交或部分成交）
pub const ORDER_STATUS_ALIVE: &str = "ALIVE";
/// 订单状态 - 已完成（全部成交或已撤销）
pub const ORDER_STATUS_FINISHED: &str = "FINISHED";
