use super::helpers::{
    default_nan, deserialize_f64_or_nan, deserialize_f64_or_nan_or_dash, deserialize_i64_or_zero,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 行情报价数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    /// 合约代码
    pub instrument_id: String,
    /// 行情时间
    pub datetime: String,
    /// 最新价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub last_price: f64,

    /// 卖一价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price1: f64,
    /// 卖一量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub ask_volume1: i64,
    /// 卖二价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price2: f64,
    /// 卖二量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub ask_volume2: i64,
    /// 卖三价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price3: f64,
    /// 卖三量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub ask_volume3: i64,
    /// 卖四价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price4: f64,
    /// 卖四量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub ask_volume4: i64,
    /// 卖五价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price5: f64,
    /// 卖五量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub ask_volume5: i64,

    /// 买一价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price1: f64,
    /// 买一量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub bid_volume1: i64,
    /// 买二价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price2: f64,
    /// 买二量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub bid_volume2: i64,
    /// 买三价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price3: f64,
    /// 买三量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub bid_volume3: i64,
    /// 买四价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price4: f64,
    /// 买四量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub bid_volume4: i64,
    /// 买五价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price5: f64,
    /// 买五量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub bid_volume5: i64,

    /// 最高价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub highest: f64,
    /// 最低价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub lowest: f64,
    /// 开盘价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub open: f64,
    /// 收盘价
    #[serde(
        default = "default_nan",
        deserialize_with = "deserialize_f64_or_nan_or_dash"
    )]
    pub close: f64,
    /// 均价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub average: f64,
    /// 成交量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub volume: i64,
    /// 成交额
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub amount: f64,
    /// 持仓量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub open_interest: i64,

    /// 跌停价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub lower_limit: f64,
    /// 涨停价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub upper_limit: f64,

    /// 结算价
    #[serde(
        default = "default_nan",
        deserialize_with = "deserialize_f64_or_nan_or_dash"
    )]
    pub settlement: f64,
    /// 昨结算价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub pre_settlement: f64,

    /// 涨跌
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub change: f64,
    /// 涨跌幅
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub change_percent: f64,

    /// 行权价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub strike_price: f64,

    /// 昨持仓量
    #[serde(default, deserialize_with = "deserialize_i64_or_zero")]
    pub pre_open_interest: i64,
    /// 昨收盘价
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub pre_close: f64,
    /// 昨成交量
    #[serde(default)]
    pub pre_volume: i64,

    /// 每手保证金
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub margin: f64,
    /// 每手手续费
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub commission: f64,

    /// 合约类型
    #[serde(default)]
    pub class: String,
    /// 交易所代码
    #[serde(default)]
    pub exchange_id: String,
    /// 品种代码
    #[serde(default)]
    pub product_id: String,
    /// 品种简称
    #[serde(default)]
    pub product_short_name: String,
    /// 标的产品
    #[serde(default)]
    pub underlying_product: String,
    /// 标的合约
    #[serde(default)]
    pub underlying_symbol: String,
    /// 交割年份
    #[serde(default)]
    pub delivery_year: i32,
    /// 交割月份
    #[serde(default)]
    pub delivery_month: i32,
    /// 到期时间
    #[serde(default)]
    pub expire_datetime: i64,
    /// 合约乘数
    #[serde(default)]
    pub volume_multiple: i32,
    /// 最小变动价位
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub price_tick: f64,
    /// 价格小数位数
    #[serde(default)]
    pub price_decs: i32,
    /// 市价单最大下单量
    #[serde(default)]
    pub max_market_order_vol: i32,
    /// 市价单最小下单量
    #[serde(default)]
    pub min_market_order_vol: i32,
    /// 限价单最大下单量
    #[serde(default)]
    pub max_limit_order_vol: i32,
    /// 限价单最小下单量
    #[serde(default)]
    pub min_limit_order_vol: i32,
    /// 是否已下市
    #[serde(default)]
    pub expired: bool,
    /// 拼音
    #[serde(default)]
    pub py: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Default for Quote {
    fn default() -> Self {
        Self {
            instrument_id: String::new(),
            datetime: String::new(),
            last_price: f64::NAN,
            ask_price1: f64::NAN,
            ask_volume1: 0,
            ask_price2: f64::NAN,
            ask_volume2: 0,
            ask_price3: f64::NAN,
            ask_volume3: 0,
            ask_price4: f64::NAN,
            ask_volume4: 0,
            ask_price5: f64::NAN,
            ask_volume5: 0,
            bid_price1: f64::NAN,
            bid_volume1: 0,
            bid_price2: f64::NAN,
            bid_volume2: 0,
            bid_price3: f64::NAN,
            bid_volume3: 0,
            bid_price4: f64::NAN,
            bid_volume4: 0,
            bid_price5: f64::NAN,
            bid_volume5: 0,
            highest: f64::NAN,
            lowest: f64::NAN,
            open: f64::NAN,
            close: f64::NAN,
            average: f64::NAN,
            volume: 0,
            amount: f64::NAN,
            open_interest: 0,
            lower_limit: f64::NAN,
            upper_limit: f64::NAN,
            settlement: f64::NAN,
            pre_settlement: f64::NAN,
            change: f64::NAN,
            change_percent: f64::NAN,
            strike_price: f64::NAN,
            pre_open_interest: 0,
            pre_close: f64::NAN,
            pre_volume: 0,
            margin: f64::NAN,
            commission: f64::NAN,
            class: String::new(),
            exchange_id: String::new(),
            product_id: String::new(),
            product_short_name: String::new(),
            underlying_product: String::new(),
            underlying_symbol: String::new(),
            delivery_year: 0,
            delivery_month: 0,
            expire_datetime: 0,
            volume_multiple: 0,
            price_tick: f64::NAN,
            price_decs: 0,
            max_market_order_vol: 0,
            min_market_order_vol: 0,
            max_limit_order_vol: 0,
            min_limit_order_vol: 0,
            expired: false,
            py: String::new(),
            epoch: None,
        }
    }
}

impl Quote {
    /// 更新涨跌和涨跌幅
    pub fn update_change(&mut self) {
        if !self.last_price.is_nan() && !self.pre_settlement.is_nan() && self.pre_settlement != 0.0
        {
            self.change = self.last_price - self.pre_settlement;
            self.change_percent = self.change / self.pre_settlement * 100.0;
        }
    }
}

/// K线数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kline {
    #[serde(default)]
    pub id: i64,
    pub datetime: i64,
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub open_oi: i64,
    pub close_oi: i64,
    pub volume: i64,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Default for Kline {
    fn default() -> Self {
        Self {
            id: 0,
            datetime: 0,
            open: f64::NAN,
            close: f64::NAN,
            high: f64::NAN,
            low: f64::NAN,
            open_oi: 0,
            close_oi: 0,
            volume: 0,
            epoch: None,
        }
    }
}

/// Tick数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tick {
    #[serde(default)]
    pub id: i64,
    pub datetime: i64,
    pub last_price: f64,
    pub average: f64,
    pub highest: f64,
    pub lowest: f64,
    pub ask_price1: f64,
    pub ask_volume1: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price2: f64,
    pub ask_volume2: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price3: f64,
    pub ask_volume3: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price4: f64,
    pub ask_volume4: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub ask_price5: f64,
    pub ask_volume5: i64,
    pub bid_price1: f64,
    pub bid_volume1: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price2: f64,
    pub bid_volume2: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price3: f64,
    pub bid_volume3: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price4: f64,
    pub bid_volume4: i64,
    #[serde(default = "default_nan", deserialize_with = "deserialize_f64_or_nan")]
    pub bid_price5: f64,
    pub bid_volume5: i64,
    pub volume: i64,
    pub amount: f64,
    pub open_interest: i64,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Default for Tick {
    fn default() -> Self {
        Self {
            id: 0,
            datetime: 0,
            last_price: f64::NAN,
            average: f64::NAN,
            highest: f64::NAN,
            lowest: f64::NAN,
            ask_price1: f64::NAN,
            ask_volume1: 0,
            ask_price2: f64::NAN,
            ask_volume2: 0,
            ask_price3: f64::NAN,
            ask_volume3: 0,
            ask_price4: f64::NAN,
            ask_volume4: 0,
            ask_price5: f64::NAN,
            ask_volume5: 0,
            bid_price1: f64::NAN,
            bid_volume1: 0,
            bid_price2: f64::NAN,
            bid_volume2: 0,
            bid_price3: f64::NAN,
            bid_volume3: 0,
            bid_price4: f64::NAN,
            bid_volume4: 0,
            bid_price5: f64::NAN,
            bid_volume5: 0,
            volume: 0,
            amount: f64::NAN,
            open_interest: 0,
            epoch: None,
        }
    }
}

/// 图表状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chart {
    pub left_id: i64,
    pub right_id: i64,
    pub more_data: bool,
    pub ready: bool,
    #[serde(default)]
    pub state: HashMap<String, serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_epoch")]
    pub epoch: Option<i64>,
}

impl Default for Chart {
    fn default() -> Self {
        Self {
            left_id: -1,
            right_id: -1,
            more_data: true,
            ready: false,
            state: HashMap::new(),
            epoch: None,
        }
    }
}

/// Chart 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartInfo {
    #[serde(default)]
    pub chart_id: String,
    pub left_id: i64,
    pub right_id: i64,
    pub more_data: bool,
    pub ready: bool,
    #[serde(default)]
    pub view_width: usize,
}
