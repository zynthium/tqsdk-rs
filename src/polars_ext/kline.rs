use crate::errors::{Result, TqError};
use crate::types::Kline;
use polars::prelude::*;

/// K线数据缓冲区
///
/// 维护可变的列向量，支持高效的追加和更新操作
/// 可按需转换为 Polars DataFrame 进行分析
#[derive(Debug, Clone)]
pub struct KlineBuffer {
    /// K线ID
    pub ids: Vec<i64>,
    /// 时间戳（纳秒）
    pub datetimes: Vec<i64>,
    /// 开盘价
    pub opens: Vec<f64>,
    /// 最高价
    pub highs: Vec<f64>,
    /// 最低价
    pub lows: Vec<f64>,
    /// 收盘价
    pub closes: Vec<f64>,
    /// 成交量
    pub volumes: Vec<i64>,
    /// 起始持仓量
    pub open_ois: Vec<i64>,
    /// 结束持仓量
    pub close_ois: Vec<i64>,
}

impl KlineBuffer {
    /// 创建空缓冲区
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            datetimes: Vec::new(),
            opens: Vec::new(),
            highs: Vec::new(),
            lows: Vec::new(),
            closes: Vec::new(),
            volumes: Vec::new(),
            open_ois: Vec::new(),
            close_ois: Vec::new(),
        }
    }

    /// 从 K线数组创建缓冲区
    pub fn from_klines(klines: &[Kline]) -> Self {
        let mut buffer = Self::new();
        for kline in klines {
            buffer.push(kline);
        }
        buffer
    }

    /// 添加新 K线
    pub fn push(&mut self, kline: &Kline) {
        self.ids.push(kline.id);
        self.datetimes.push(kline.datetime);
        self.opens.push(kline.open);
        self.highs.push(kline.high);
        self.lows.push(kline.low);
        self.closes.push(kline.close);
        self.volumes.push(kline.volume);
        self.open_ois.push(kline.open_oi);
        self.close_ois.push(kline.close_oi);
    }

    /// 更新最后一根 K线
    ///
    /// 如果缓冲区为空，则添加新 K线
    pub fn update_last(&mut self, kline: &Kline) {
        if self.ids.is_empty() {
            self.push(kline);
            return;
        }

        let last_idx = self.ids.len() - 1;

        self.highs[last_idx] = kline.high;
        self.lows[last_idx] = kline.low;
        self.closes[last_idx] = kline.close;
        self.volumes[last_idx] = kline.volume;
        self.close_ois[last_idx] = kline.close_oi;
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
        self.opens.clear();
        self.highs.clear();
        self.lows.clear();
        self.closes.clear();
        self.volumes.clear();
        self.open_ois.clear();
        self.close_ois.clear();
    }

    /// 转换为 Polars DataFrame
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("K线缓冲区为空".to_string()));
        }

        let df = DataFrame::new(vec![
            Column::new("id".into(), &self.ids),
            Column::new("datetime".into(), &self.datetimes),
            Column::new("open".into(), &self.opens),
            Column::new("high".into(), &self.highs),
            Column::new("low".into(), &self.lows),
            Column::new("close".into(), &self.closes),
            Column::new("volume".into(), &self.volumes),
            Column::new("open_oi".into(), &self.open_ois),
            Column::new("close_oi".into(), &self.close_ois),
        ])
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }

    /// 获取最后 n 行的 DataFrame
    pub fn tail(&self, n: usize) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("K线缓冲区为空".to_string()));
        }

        let len = self.ids.len();
        let start = len.saturating_sub(n);

        let df = DataFrame::new(vec![
            Column::new("id".into(), &self.ids[start..]),
            Column::new("datetime".into(), &self.datetimes[start..]),
            Column::new("open".into(), &self.opens[start..]),
            Column::new("high".into(), &self.highs[start..]),
            Column::new("low".into(), &self.lows[start..]),
            Column::new("close".into(), &self.closes[start..]),
            Column::new("volume".into(), &self.volumes[start..]),
            Column::new("open_oi".into(), &self.open_ois[start..]),
            Column::new("close_oi".into(), &self.close_ois[start..]),
        ])
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }

    /// 获取前 n 行的 DataFrame
    pub fn head(&self, n: usize) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("K线缓冲区为空".to_string()));
        }

        let len = self.ids.len();
        let end = if n >= len { len } else { n };

        let df = DataFrame::new(vec![
            Column::new("id".into(), &self.ids[..end]),
            Column::new("datetime".into(), &self.datetimes[..end]),
            Column::new("open".into(), &self.opens[..end]),
            Column::new("high".into(), &self.highs[..end]),
            Column::new("low".into(), &self.lows[..end]),
            Column::new("close".into(), &self.closes[..end]),
            Column::new("volume".into(), &self.volumes[..end]),
            Column::new("open_oi".into(), &self.open_ois[..end]),
            Column::new("close_oi".into(), &self.close_ois[..end]),
        ])
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }

    /// 获取指定范围的 DataFrame
    pub fn slice(&self, start: usize, length: usize) -> Result<DataFrame> {
        if self.is_empty() {
            return Err(TqError::Other("K线缓冲区为空".to_string()));
        }

        let len = self.ids.len();
        if start >= len {
            return Err(TqError::Other(format!(
                "起始位置 {} 超出范围 {}",
                start, len
            )));
        }

        let end = std::cmp::min(start + length, len);

        let df = DataFrame::new(vec![
            Column::new("id".into(), &self.ids[start..end]),
            Column::new("datetime".into(), &self.datetimes[start..end]),
            Column::new("open".into(), &self.opens[start..end]),
            Column::new("high".into(), &self.highs[start..end]),
            Column::new("low".into(), &self.lows[start..end]),
            Column::new("close".into(), &self.closes[start..end]),
            Column::new("volume".into(), &self.volumes[start..end]),
            Column::new("open_oi".into(), &self.open_ois[start..end]),
            Column::new("close_oi".into(), &self.close_ois[start..end]),
        ])
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }
}

impl Default for KlineBuffer {
    fn default() -> Self {
        Self::new()
    }
}
