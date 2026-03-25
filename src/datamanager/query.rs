use super::DataManager;
use crate::errors::{Result, TqError};
use crate::types::*;
use crate::utils::{nanos_to_datetime, value_to_i64};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::error;

impl DataManager {
    /// 根据路径获取数据
    pub fn get_by_path(&self, path: &[&str]) -> Option<Value> {
        if path.is_empty() {
            return None;
        }

        let data = self.data.read().unwrap();
        get_by_path_ref(&data, path).cloned()
    }

    /// 获取指定路径的数据更新版本号
    pub fn get_path_epoch(&self, path: &[&str]) -> i64 {
        let data = self.data.read().unwrap();
        if path.is_empty() {
            return 0;
        }

        let mut current_value = data.get(path[0]);
        for &key in &path[1..] {
            if let Some(Value::Object(map)) = current_value {
                current_value = map.get(key);
            } else {
                return 0;
            }
        }

        if let Some(Value::Object(map)) = current_value
            && let Some(Value::Number(epoch_val)) = map.get("_epoch")
            && let Some(epoch) = epoch_val.as_i64()
        {
            return epoch;
        }

        0
    }

    /// 判断指定路径的数据是否在最近一次更新中发生了变化
    pub fn is_changing(&self, path: &[&str]) -> bool {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        let data = self.data.read().unwrap();

        if path.is_empty() {
            return false;
        }

        let mut current_value = data.get(path[0]);
        for &key in &path[1..] {
            if let Some(Value::Object(map)) = current_value {
                current_value = map.get(key);
            } else {
                return false;
            }
        }

        if let Some(Value::Object(map)) = current_value
            && let Some(Value::Number(epoch_val)) = map.get("_epoch")
            && let Some(epoch) = epoch_val.as_i64()
        {
            return epoch == current_epoch;
        }

        false
    }

    /// 设置默认值
    pub fn set_default(&self, path: &[&str], default_value: Value) -> Option<Value> {
        let mut data = self.data.write().unwrap();
        let current = &mut *data;

        for (i, &key) in path.iter().enumerate() {
            if i == path.len() - 1 {
                if !current.contains_key(key) {
                    current.insert(key.to_string(), default_value.clone());
                }
                return current.get(key).cloned();
            }

            let entry = current
                .entry(key.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));

            if !entry.is_object() {
                return None;
            }
        }

        None
    }

    /// 转换为结构体
    pub fn convert_to_struct<T: DeserializeOwned>(&self, data: &Value) -> Result<T> {
        T::deserialize(data).map_err(|e| TqError::Json {
            context: "转换失败".to_string(),
            source: e,
        })
    }

    /// 获取 Quote 数据
    pub fn get_quote_data(&self, symbol: &str) -> Result<Quote> {
        let data = self
            .get_by_path(&["quotes", symbol])
            .ok_or_else(|| TqError::DataNotFound(format!("Quote 未找到: {}", symbol)))?;

        self.convert_to_struct(&data)
    }

    /// 获取 K线数据
    pub fn get_klines_data(
        &self,
        symbol: &str,
        duration: i64,
        view_width: usize,
        right_id: i64,
    ) -> Result<KlineSeriesData> {
        let duration_str = duration.to_string();
        let data = self
            .get_by_path(&["klines", symbol, &duration_str])
            .ok_or_else(|| TqError::DataNotFound(format!("K线未找到: {}/{}", symbol, duration)))?;

        if let Value::Object(data_map) = data {
            let mut kline_series = KlineSeriesData {
                symbol: symbol.to_string(),
                duration,
                chart_id: String::new(),
                chart: None,
                last_id: value_to_i64(data_map.get("last_id").unwrap_or(&Value::Null)),
                trading_day_start_id: value_to_i64(
                    data_map.get("trading_day_start_id").unwrap_or(&Value::Null),
                ),
                trading_day_end_id: value_to_i64(
                    data_map.get("trading_day_end_id").unwrap_or(&Value::Null),
                ),
                data: Vec::new(),
                has_new_bar: false,
            };

            if let Some(Value::Object(kline_map)) = data_map.get("data") {
                let selected_ids = collect_windowed_ids(
                    kline_map,
                    right_id,
                    view_width,
                    self.config.default_view_width,
                );
                let mut klines: Vec<Kline> = Vec::with_capacity(selected_ids.len());
                for id in selected_ids {
                    let id_key = id.to_string();
                    let Some(kline_data) = kline_map.get(&id_key) else {
                        continue;
                    };
                    match self.convert_to_struct::<Kline>(kline_data) {
                        Ok(mut kline) => {
                            kline.id = id;
                            klines.push(kline);
                        }
                        Err(e) => {
                            error!(
                                "{}",
                                TqError::ParseError(format!("K线数据格式错误 id={}: {}", id, e))
                            );
                        }
                    }
                }

                kline_series.data = klines;
            }

            Ok(kline_series)
        } else {
            Err(TqError::ParseError("K线数据格式错误".to_string()))
        }
    }

    /// 获取多合约对齐的 K线数据
    pub fn get_multi_klines_data(
        &self,
        symbols: &[String],
        duration: i64,
        chart_id: &str,
        view_width: usize,
    ) -> Result<MultiKlineSeriesData> {
        if symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbols 为空".to_string()));
        }

        let main_symbol = &symbols[0];
        let duration_str = duration.to_string();
        let data_guard = self.data.read().unwrap();

        let (left_id, right_id) = if let Some(Value::Object(chart_map)) =
            get_by_path_ref(&data_guard, &["charts", chart_id])
        {
            let left = value_to_i64(chart_map.get("left_id").unwrap_or(&Value::Null));
            let right = value_to_i64(chart_map.get("right_id").unwrap_or(&Value::Null));
            (left, right)
        } else {
            (-1, -1)
        };

        let mut result = MultiKlineSeriesData {
            chart_id: chart_id.to_string(),
            duration,
            main_symbol: main_symbol.clone(),
            symbols: symbols.to_vec(),
            left_id,
            right_id,
            view_width,
            data: Vec::new(),
            has_new_bar: false,
            metadata: HashMap::new(),
        };

        let mut aligned_data_indexes: HashMap<String, HashMap<i64, &Value>> = HashMap::new();
        for symbol in symbols {
            if let Some(Value::Object(kline_map)) = get_by_path_ref(
                &data_guard,
                &["klines", symbol.as_str(), duration_str.as_str()],
            ) {
                let metadata = KlineMetadata {
                    symbol: symbol.clone(),
                    last_id: value_to_i64(kline_map.get("last_id").unwrap_or(&Value::Null)),
                    trading_day_start_id: value_to_i64(
                        kline_map
                            .get("trading_day_start_id")
                            .unwrap_or(&Value::Null),
                    ),
                    trading_day_end_id: value_to_i64(
                        kline_map.get("trading_day_end_id").unwrap_or(&Value::Null),
                    ),
                };
                result.metadata.insert(symbol.clone(), metadata);
                if let Some(Value::Object(symbol_data_map)) = kline_map.get("data") {
                    aligned_data_indexes
                        .insert(symbol.clone(), build_numeric_value_index(symbol_data_map));
                }
            }
        }

        if let Some(Value::Object(main_kline_map)) = get_by_path_ref(
            &data_guard,
            &["klines", main_symbol.as_str(), duration_str.as_str()],
        ) {
            let mut bindings: HashMap<String, HashMap<i64, i64>> = HashMap::new();
            if let Some(Value::Object(binding_map)) = main_kline_map.get("binding") {
                for (symbol, binding_info) in binding_map {
                    if let Value::Object(binding_id_map) = binding_info {
                        let mut id_map: HashMap<i64, i64> = HashMap::new();
                        for (main_id_str, other_id) in binding_id_map {
                            if let Ok(main_id) = main_id_str.parse::<i64>() {
                                id_map.insert(main_id, value_to_i64(other_id));
                            }
                        }
                        bindings.insert(symbol.clone(), id_map);
                    }
                }
            }

            if let Some(Value::Object(main_data_map)) = main_kline_map.get("data") {
                let main_data_index = aligned_data_indexes
                    .remove(main_symbol)
                    .unwrap_or_else(|| build_numeric_value_index(main_data_map));
                let mut main_ids: Vec<i64> = main_data_index.keys().copied().collect();
                main_ids.sort();

                if right_id > 0 {
                    main_ids.retain(|&id| id <= right_id);
                }

                if view_width > 0 && main_ids.len() > view_width {
                    let split_index = main_ids.len() - view_width;
                    main_ids.drain(0..split_index);
                    if let Some(first) = main_ids.first() {
                        result.left_id = *first;
                    }
                    if let Some(last) = main_ids.last() {
                        result.right_id = *last;
                    }
                }

                for main_id in main_ids {
                    let mut set = AlignedKlineSet {
                        main_id,
                        timestamp: Utc::now(),
                        klines: HashMap::new(),
                    };

                    let Some(kline_data) = main_data_index.get(&main_id) else {
                        continue;
                    };
                    let Ok(mut kline) = self.convert_to_struct::<Kline>(kline_data) else {
                        continue;
                    };
                    kline.id = main_id;
                    set.timestamp = nanos_to_datetime(kline.datetime);
                    set.klines.insert(main_symbol.clone(), kline);

                    let mut aligned = true;
                    for symbol in symbols.iter().skip(1) {
                        let Some(binding) = bindings.get(symbol) else {
                            aligned = false;
                            break;
                        };
                        let Some(other_index) = aligned_data_indexes.get(symbol) else {
                            aligned = false;
                            break;
                        };
                        let Some(&mapped_id) = binding.get(&main_id) else {
                            aligned = false;
                            break;
                        };
                        let Some(other_kline_data) = other_index.get(&mapped_id) else {
                            aligned = false;
                            break;
                        };
                        let Ok(mut kline) = self.convert_to_struct::<Kline>(other_kline_data)
                        else {
                            aligned = false;
                            break;
                        };
                        kline.id = mapped_id;
                        set.klines.insert(symbol.clone(), kline);
                    }

                    if aligned && set.klines.len() == symbols.len() {
                        result.data.push(set);
                    }
                }
            }
        }

        Ok(result)
    }

    /// 获取 Tick 数据
    pub fn get_ticks_data(
        &self,
        symbol: &str,
        view_width: usize,
        right_id: i64,
    ) -> Result<TickSeriesData> {
        let data = self
            .get_by_path(&["ticks", symbol])
            .ok_or_else(|| TqError::DataNotFound(format!("Tick 未找到: {}", symbol)))?;

        if let Value::Object(data_map) = data {
            let mut tick_series = TickSeriesData {
                symbol: symbol.to_string(),
                chart_id: String::new(),
                chart: None,
                last_id: value_to_i64(data_map.get("last_id").unwrap_or(&Value::Null)),
                data: Vec::new(),
                has_new_bar: false,
            };

            if let Some(Value::Object(tick_map)) = data_map.get("data") {
                let selected_ids = collect_windowed_ids(
                    tick_map,
                    right_id,
                    view_width,
                    self.config.default_view_width,
                );
                let mut ticks: Vec<Tick> = Vec::with_capacity(selected_ids.len());
                for id in selected_ids {
                    let id_key = id.to_string();
                    let Some(tick_data) = tick_map.get(&id_key) else {
                        continue;
                    };
                    if let Ok(mut tick) = self.convert_to_struct::<Tick>(tick_data) {
                        tick.id = id;
                        ticks.push(tick);
                    }
                }
                tick_series.data = ticks;
            }

            Ok(tick_series)
        } else {
            Err(TqError::ParseError("Tick数据格式错误".to_string()))
        }
    }

    /// 获取账户数据
    pub fn get_account_data(&self, user_id: &str, currency: &str) -> Result<Account> {
        let data = self
            .get_by_path(&["trade", user_id, "accounts", currency])
            .unwrap_or_else(|| {
                json!({
                    "user_id": user_id,
                    "currency": currency,
                })
            });

        self.convert_to_struct(&data)
    }

    /// 获取持仓数据
    pub fn get_position_data(&self, user_id: &str, symbol: &str) -> Result<Position> {
        let data = self
            .get_by_path(&["trade", user_id, "positions", symbol])
            .unwrap_or_else(|| {
                json!({
                    "user_id": user_id,
                    "symbol": symbol,
                })
            });

        self.convert_to_struct(&data)
    }

    /// 获取委托单数据
    pub fn get_order_data(&self, user_id: &str, order_id: &str) -> Result<Order> {
        let data = self
            .get_by_path(&["trade", user_id, "orders", order_id])
            .unwrap_or_else(|| {
                json!({
                    "user_id": user_id,
                    "order_id": order_id,
                })
            });

        self.convert_to_struct(&data)
    }

    /// 获取成交数据
    pub fn get_trade_data(&self, user_id: &str, trade_id: &str) -> Result<Trade> {
        let data = self
            .get_by_path(&["trade", user_id, "trades", trade_id])
            .unwrap_or_else(|| {
                json!({
                    "user_id": user_id,
                    "trade_id": trade_id,
                })
            });

        self.convert_to_struct(&data)
    }
}

fn get_by_path_ref<'a>(data: &'a HashMap<String, Value>, path: &[&str]) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = data.get(path[0])?;
    for &key in &path[1..] {
        match current {
            Value::Object(map) => {
                current = map.get(key)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn collect_windowed_ids(
    data_map: &Map<String, Value>,
    right_id: i64,
    view_width: usize,
    default_view_width: usize,
) -> Vec<i64> {
    let mut ids: Vec<i64> = data_map
        .keys()
        .filter_map(|key| key.parse::<i64>().ok())
        .filter(|id| right_id <= 0 || *id <= right_id)
        .collect();
    ids.sort_unstable();
    let target_width = if view_width > 0 {
        view_width
    } else {
        default_view_width
    };
    if ids.len() > target_width {
        let split_index = ids.len() - target_width;
        ids.drain(0..split_index);
    }
    ids
}

fn build_numeric_value_index(data_map: &Map<String, Value>) -> HashMap<i64, &Value> {
    let mut index = HashMap::with_capacity(data_map.len());
    for (id_str, value) in data_map {
        if let Ok(id) = id_str.parse::<i64>() {
            index.insert(id, value);
        }
    }
    index
}
