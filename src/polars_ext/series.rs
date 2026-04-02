use super::{KlineBuffer, TickBuffer};
use crate::errors::{Result, TqError};
use crate::types::{KlineSeriesData, MultiKlineSeriesData, SeriesData, TickSeriesData};
use polars::prelude::*;

impl SeriesData {
    /// 转换为 Polars DataFrame
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.is_tick {
            self.tick_to_dataframe()
        } else if self.is_multi {
            self.multi_kline_to_dataframe()
        } else {
            self.single_kline_to_dataframe()
        }
    }

    /// 单合约 K线转 DataFrame
    fn single_kline_to_dataframe(&self) -> Result<DataFrame> {
        let kline_data = self
            .single
            .as_ref()
            .ok_or_else(|| TqError::Other("单合约数据不存在".to_string()))?;

        kline_data.to_dataframe()
    }

    /// Tick 数据转 DataFrame
    fn tick_to_dataframe(&self) -> Result<DataFrame> {
        let tick_data = self
            .tick_data
            .as_ref()
            .ok_or_else(|| TqError::Other("Tick数据不存在".to_string()))?;

        tick_data.to_dataframe()
    }

    /// 多合约 K线转 DataFrame（长表格式）
    fn multi_kline_to_dataframe(&self) -> Result<DataFrame> {
        let multi_data = self
            .multi
            .as_ref()
            .ok_or_else(|| TqError::Other("多合约数据不存在".to_string()))?;

        multi_data.to_dataframe()
    }

    /// 多合约 K线转 DataFrame（宽表格式）
    /// 每个合约的 OHLCV 数据作为独立的列
    pub fn to_wide_dataframe(&self) -> Result<DataFrame> {
        if !self.is_multi {
            return Err(TqError::Other("只有多合约数据支持宽表格式".to_string()));
        }

        let multi_data = self
            .multi
            .as_ref()
            .ok_or_else(|| TqError::Other("多合约数据不存在".to_string()))?;

        multi_data.to_wide_dataframe()
    }
}

impl KlineSeriesData {
    /// 转换为 Polars DataFrame
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        let buffer = KlineBuffer::from_klines(&self.data);
        buffer.to_dataframe()
    }

    /// 转换为 KlineBuffer
    pub fn to_buffer(&self) -> KlineBuffer {
        KlineBuffer::from_klines(&self.data)
    }
}

impl TickSeriesData {
    /// 转换为 Polars DataFrame
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        let buffer = TickBuffer::from_ticks(&self.data);
        buffer.to_dataframe()
    }

    /// 转换为 TickBuffer
    pub fn to_buffer(&self) -> TickBuffer {
        TickBuffer::from_ticks(&self.data)
    }
}

impl MultiKlineSeriesData {
    /// 转换为 DataFrame（长表格式）
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        if self.data.is_empty() {
            return Err(TqError::Other("多合约K线数据为空".to_string()));
        }

        let mut main_ids = Vec::new();
        let mut timestamps = Vec::new();
        let mut symbols = Vec::new();
        let mut opens = Vec::new();
        let mut highs = Vec::new();
        let mut lows = Vec::new();
        let mut closes = Vec::new();
        let mut volumes = Vec::new();
        let mut open_ois = Vec::new();
        let mut close_ois = Vec::new();

        for aligned_set in &self.data {
            for (symbol, kline) in &aligned_set.klines {
                main_ids.push(aligned_set.main_id);
                timestamps.push(aligned_set.timestamp.timestamp_nanos_opt().unwrap_or(0));
                symbols.push(symbol.clone());
                opens.push(kline.open);
                highs.push(kline.high);
                lows.push(kline.low);
                closes.push(kline.close);
                volumes.push(kline.volume);
                open_ois.push(kline.open_oi);
                close_ois.push(kline.close_oi);
            }
        }

        let df = DataFrame::new(
            main_ids.len(),
            vec![
                Column::new("main_id".into(), main_ids),
                Column::new("timestamp".into(), timestamps),
                Column::new("symbol".into(), symbols),
                Column::new("open".into(), opens),
                Column::new("high".into(), highs),
                Column::new("low".into(), lows),
                Column::new("close".into(), closes),
                Column::new("volume".into(), volumes),
                Column::new("open_oi".into(), open_ois),
                Column::new("close_oi".into(), close_ois),
            ],
        )
        .map_err(|e| TqError::Other(format!("创建 DataFrame 失败: {}", e)))?;

        Ok(df)
    }

    /// 转换为 DataFrame（宽表格式）
    pub fn to_wide_dataframe(&self) -> Result<DataFrame> {
        if self.data.is_empty() {
            return Err(TqError::Other("多合约K线数据为空".to_string()));
        }

        let main_ids: Vec<i64> = self.data.iter().map(|s| s.main_id).collect();
        let timestamps: Vec<i64> = self
            .data
            .iter()
            .map(|s| s.timestamp.timestamp_nanos_opt().unwrap_or(0))
            .collect();

        let mut columns = vec![
            Column::new("main_id".into(), main_ids),
            Column::new("timestamp".into(), timestamps),
        ];

        for symbol in &self.symbols {
            let mut opens = Vec::new();
            let mut highs = Vec::new();
            let mut lows = Vec::new();
            let mut closes = Vec::new();
            let mut volumes = Vec::new();

            for aligned_set in &self.data {
                if let Some(kline) = aligned_set.klines.get(symbol) {
                    opens.push(kline.open);
                    highs.push(kline.high);
                    lows.push(kline.low);
                    closes.push(kline.close);
                    volumes.push(kline.volume);
                } else {
                    opens.push(f64::NAN);
                    highs.push(f64::NAN);
                    lows.push(f64::NAN);
                    closes.push(f64::NAN);
                    volumes.push(0);
                }
            }

            columns.push(Column::new(format!("{}_open", symbol).into(), opens));
            columns.push(Column::new(format!("{}_high", symbol).into(), highs));
            columns.push(Column::new(format!("{}_low", symbol).into(), lows));
            columns.push(Column::new(format!("{}_close", symbol).into(), closes));
            columns.push(Column::new(format!("{}_volume", symbol).into(), volumes));
        }

        let df = DataFrame::new(self.data.len(), columns)
            .map_err(|e| TqError::Other(format!("创建宽表 DataFrame 失败: {}", e)))?;

        Ok(df)
    }
}
