use crate::errors::{Result, TqError};
use crate::types::Tick;
use polars::prelude::*;

/// Tick 数据缓冲区
#[derive(Debug, Clone)]
pub struct TickBuffer {
    /// Tick ID
    pub ids: Vec<i64>,
    /// 时间戳（纳秒）
    pub datetimes: Vec<i64>,
    /// 最新价
    pub last_prices: Vec<f64>,
    /// 均价
    pub averages: Vec<f64>,
    /// 最高价
    pub highests: Vec<f64>,
    /// 最低价
    pub lowests: Vec<f64>,
    /// 卖一价
    pub ask_price1s: Vec<f64>,
    /// 卖一量
    pub ask_volume1s: Vec<i64>,
    /// 买一价
    pub bid_price1s: Vec<f64>,
    /// 买一量
    pub bid_volume1s: Vec<i64>,
    /// 成交量
    pub volumes: Vec<i64>,
    /// 成交额
    pub amounts: Vec<f64>,
    /// 持仓量
    pub open_interests: Vec<i64>,
}

impl TickBuffer {
    /// 创建空缓冲区
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            datetimes: Vec::new(),
            last_prices: Vec::new(),
            averages: Vec::new(),
            highests: Vec::new(),
            lowests: Vec::new(),
            ask_price1s: Vec::new(),
            ask_volume1s: Vec::new(),
            bid_price1s: Vec::new(),
            bid_volume1s: Vec::new(),
            volumes: Vec::new(),
            amounts: Vec::new(),
            open_interests: Vec::new(),
        }
    }

    /// 从 Tick 数组创建缓冲区
    pub fn from_ticks(ticks: &[Tick]) -> Self {
        let mut buffer = Self::new();
        for tick in ticks {
            buffer.push(tick);
        }
        buffer
    }

    /// 添加新 Tick
    pub fn push(&mut self, tick: &Tick) {
        self.ids.push(tick.id);
        self.datetimes.push(tick.datetime);
        self.last_prices.push(tick.last_price);
        self.averages.push(tick.average);
        self.highests.push(tick.highest);
        self.lowests.push(tick.lowest);
        self.ask_price1s.push(tick.ask_price1);
        self.ask_volume1s.push(tick.ask_volume1);
        self.bid_price1s.push(tick.bid_price1);
        self.bid_volume1s.push(tick.bid_volume1);
        self.volumes.push(tick.volume);
        self.amounts.push(tick.amount);
        self.open_interests.push(tick.open_interest);
    }

    /// 更新最后一个 Tick
    pub fn update_last(&mut self, tick: &Tick) {
        if self.ids.is_empty() {
            self.push(tick);
            return;
        }

        let last_idx = self.ids.len() - 1;

        self.last_prices[last_idx] = tick.last_price;
        self.averages[last_idx] = tick.average;
        self.highests[last_idx] = tick.highest;
        self.lowests[last_idx] = tick.lowest;
        self.ask_price1s[last_idx] = tick.ask_price1;
        self.ask_volume1s[last_idx] = tick.ask_volume1;
        self.bid_price1s[last_idx] = tick.bid_price1;
        self.bid_volume1s[last_idx] = tick.bid_volume1;
        self.volumes[last_idx] = tick.volume;
        self.amounts[last_idx] = tick.amount;
        self.open_interests[last_idx] = tick.open_interest;
    }

    /// 获取缓冲区长度
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// 检查缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// 清空缓冲区
    pub fn clear(&mut self) {
        self.ids.clear();
        self.datetimes.clear();
        self.last_prices.clear();
        self.averages.clear();
        self.highests.clear();
        self.lowests.clear();
        self.ask_price1s.clear();
        self.ask_volume1s.clear();
        self.bid_price1s.clear();
        self.bid_volume1s.clear();
        self.volumes.clear();
        self.amounts.clear();
        self.open_interests.clear();
    }

    /// 转换为 Polars DataFrame
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("Tick缓冲区为空".to_string()));
        }

        let df = DataFrame::new(
            self.ids.len(),
            vec![
                Column::new("id".into(), &self.ids),
                Column::new("datetime".into(), &self.datetimes),
                Column::new("last_price".into(), &self.last_prices),
                Column::new("average".into(), &self.averages),
                Column::new("highest".into(), &self.highests),
                Column::new("lowest".into(), &self.lowests),
                Column::new("ask_price1".into(), &self.ask_price1s),
                Column::new("ask_volume1".into(), &self.ask_volume1s),
                Column::new("bid_price1".into(), &self.bid_price1s),
                Column::new("bid_volume1".into(), &self.bid_volume1s),
                Column::new("volume".into(), &self.volumes),
                Column::new("amount".into(), &self.amounts),
                Column::new("open_interest".into(), &self.open_interests),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }

    /// 获取最后 n 行的 DataFrame
    pub fn tail(&self, n: usize) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("Tick缓冲区为空".to_string()));
        }

        let len = self.ids.len();
        let start = len.saturating_sub(n);

        let df = DataFrame::new(
            len - start,
            vec![
                Column::new("id".into(), &self.ids[start..]),
                Column::new("datetime".into(), &self.datetimes[start..]),
                Column::new("last_price".into(), &self.last_prices[start..]),
                Column::new("average".into(), &self.averages[start..]),
                Column::new("highest".into(), &self.highests[start..]),
                Column::new("lowest".into(), &self.lowests[start..]),
                Column::new("ask_price1".into(), &self.ask_price1s[start..]),
                Column::new("ask_volume1".into(), &self.ask_volume1s[start..]),
                Column::new("bid_price1".into(), &self.bid_price1s[start..]),
                Column::new("bid_volume1".into(), &self.bid_volume1s[start..]),
                Column::new("volume".into(), &self.volumes[start..]),
                Column::new("amount".into(), &self.amounts[start..]),
                Column::new("open_interest".into(), &self.open_interests[start..]),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }
}

impl Default for TickBuffer {
    fn default() -> Self {
        Self::new()
    }
}
