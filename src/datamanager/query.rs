use super::DataManager;
use crate::errors::{Result, TqError};
use crate::types::*;
use crate::utils::{nanos_to_datetime, value_to_i64};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::error;

const MAX_NUMERIC_VALUE_INDEX_CACHE_ENTRIES: usize = 64;

impl DataManager {
    fn with_data_read<R>(&self, f: impl FnOnce(&HashMap<String, Value>) -> R) -> R {
        let data = self.data.read().unwrap();
        f(&data)
    }

    fn cached_numeric_value_index(
        &self,
        cache_key: &str,
        epoch: i64,
        data_map: &Map<String, Value>,
    ) -> Arc<HashMap<i64, Value>> {
        if let Some(cached) = self.numeric_value_index_cache.read().unwrap().get(cache_key)
            && cached.epoch == epoch
        {
            return Arc::clone(&cached.index);
        }

        let index = Arc::new(build_owned_numeric_value_index(data_map));
        let mut cache = self.numeric_value_index_cache.write().unwrap();
        if let Some(cached) = cache.get(cache_key)
            && cached.epoch == epoch
        {
            return Arc::clone(&cached.index);
        }
        if cache.len() >= MAX_NUMERIC_VALUE_INDEX_CACHE_ENTRIES && !cache.contains_key(cache_key) {
            cache.clear();
        }
        cache.insert(
            cache_key.to_string(),
            super::CachedNumericValueIndex {
                epoch,
                index: Arc::clone(&index),
            },
        );
        index
    }

    fn snapshot_multi_kline_query(
        &self,
        data: &HashMap<String, Value>,
        symbols: &[String],
        main_symbol: &str,
        duration_str: &str,
        chart_id: &str,
    ) -> MultiKlineQuerySnapshot {
        let (left_id, right_id) = if let Some(Value::Object(chart_map)) = get_by_path_ref(data, &["charts", chart_id]) {
            (
                value_to_i64(chart_map.get("left_id").unwrap_or(&Value::Null)),
                value_to_i64(chart_map.get("right_id").unwrap_or(&Value::Null)),
            )
        } else {
            (-1, -1)
        };

        let mut metadata = HashMap::new();
        let mut aligned_data_indexes = HashMap::new();
        for symbol in symbols {
            if let Some(Value::Object(kline_map)) = get_by_path_ref(data, &["klines", symbol.as_str(), duration_str]) {
                metadata.insert(
                    symbol.clone(),
                    KlineMetadata {
                        symbol: symbol.clone(),
                        last_id: value_to_i64(kline_map.get("last_id").unwrap_or(&Value::Null)),
                        trading_day_start_id: value_to_i64(
                            kline_map.get("trading_day_start_id").unwrap_or(&Value::Null),
                        ),
                        trading_day_end_id: value_to_i64(kline_map.get("trading_day_end_id").unwrap_or(&Value::Null)),
                    },
                );
                if let Some(Value::Object(symbol_data_map)) = kline_map.get("data") {
                    let cache_key = numeric_value_index_cache_key(symbol, duration_str);
                    let epoch = value_to_i64(kline_map.get("_epoch").unwrap_or(&Value::Null));
                    aligned_data_indexes.insert(
                        symbol.clone(),
                        self.cached_numeric_value_index(cache_key.as_str(), epoch, symbol_data_map),
                    );
                }
            }
        }

        let mut bindings = HashMap::new();
        if let Some(Value::Object(main_kline_map)) = get_by_path_ref(data, &["klines", main_symbol, duration_str])
            && let Some(Value::Object(binding_map)) = main_kline_map.get("binding")
        {
            for (symbol, binding_info) in binding_map {
                if let Value::Object(binding_id_map) = binding_info {
                    let mut id_map = HashMap::with_capacity(binding_id_map.len());
                    for (main_id_str, other_id) in binding_id_map {
                        if let Ok(main_id) = main_id_str.parse::<i64>() {
                            id_map.insert(main_id, value_to_i64(other_id));
                        }
                    }
                    bindings.insert(symbol.clone(), id_map);
                }
            }
        }

        let main_data = aligned_data_indexes
            .remove(main_symbol)
            .unwrap_or_else(|| Arc::new(HashMap::new()));

        MultiKlineQuerySnapshot {
            left_id,
            right_id,
            metadata,
            main_data,
            aligned_data_indexes,
            bindings,
        }
    }

    pub(crate) fn with_path_ref<R>(&self, path: &[&str], f: impl FnOnce(Option<&Value>) -> R) -> R {
        if path.is_empty() {
            return f(None);
        }
        self.with_data_read(|data| f(get_by_path_ref(data, path)))
    }

    /// 根据路径获取数据
    pub fn get_by_path(&self, path: &[&str]) -> Option<Value> {
        self.with_path_ref(path, |value| value.cloned())
    }

    /// 获取指定路径的数据更新版本号
    pub fn get_path_epoch(&self, path: &[&str]) -> i64 {
        self.with_path_ref(path, |current_value| {
            if let Some(Value::Object(map)) = current_value
                && let Some(Value::Number(epoch_val)) = map.get("_epoch")
                && let Some(epoch) = epoch_val.as_i64()
            {
                return epoch;
            }

            0
        })
    }

    /// 判断指定路径的数据是否在最近一次更新中发生了变化
    pub fn is_changing(&self, path: &[&str]) -> bool {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        self.with_path_ref(path, |current_value| {
            if let Some(Value::Object(map)) = current_value
                && let Some(Value::Number(epoch_val)) = map.get("_epoch")
                && let Some(epoch) = epoch_val.as_i64()
            {
                return epoch == current_epoch;
            }

            false
        })
    }

    /// 设置默认值
    pub fn set_default(&self, path: &[&str], default_value: Value) -> Option<Value> {
        if path.is_empty() {
            return None;
        }

        let mut data = self.data.write().unwrap();
        if path.len() == 1 {
            let leaf = path[0];
            if !data.contains_key(leaf) {
                data.insert(leaf.to_string(), default_value.clone());
            }
            return data.get(leaf).cloned();
        }

        let mut current = data
            .entry(path[0].to_string())
            .or_insert_with(|| Value::Object(Map::new()));

        for &key in &path[1..path.len() - 1] {
            current = match current {
                Value::Object(map) => map.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new())),
                _ => return None,
            };
        }

        let leaf = path[path.len() - 1];
        let map = match current {
            Value::Object(map) => map,
            _ => return None,
        };
        if !map.contains_key(leaf) {
            map.insert(leaf.to_string(), default_value.clone());
        }
        map.get(leaf).cloned()
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
                trading_day_start_id: value_to_i64(data_map.get("trading_day_start_id").unwrap_or(&Value::Null)),
                trading_day_end_id: value_to_i64(data_map.get("trading_day_end_id").unwrap_or(&Value::Null)),
                data: Vec::new(),
                has_new_bar: false,
            };

            if let Some(Value::Object(kline_map)) = data_map.get("data") {
                let selected_ids =
                    collect_windowed_ids(kline_map, right_id, view_width, self.config.default_view_width);
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
                            error!("{}", TqError::ParseError(format!("K线数据格式错误 id={}: {}", id, e)));
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
        let snapshot = self.with_data_read(|data| {
            self.snapshot_multi_kline_query(data, symbols, main_symbol, duration_str.as_str(), chart_id)
        });

        let mut result = MultiKlineSeriesData {
            chart_id: chart_id.to_string(),
            duration,
            main_symbol: main_symbol.clone(),
            symbols: symbols.to_vec(),
            left_id: snapshot.left_id,
            right_id: snapshot.right_id,
            view_width,
            data: Vec::new(),
            has_new_bar: false,
            metadata: snapshot.metadata,
        };

        let mut main_ids: Vec<i64> = snapshot.main_data.keys().copied().collect();
        main_ids.sort_unstable();

        if result.right_id > 0 {
            main_ids.retain(|&id| id <= result.right_id);
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

            let Some(kline_data) = snapshot.main_data.get(&main_id) else {
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
                let Some(binding) = snapshot.bindings.get(symbol) else {
                    aligned = false;
                    break;
                };
                let Some(other_index) = snapshot.aligned_data_indexes.get(symbol) else {
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
                let Ok(mut kline) = self.convert_to_struct::<Kline>(other_kline_data) else {
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

        Ok(result)
    }

    /// 获取 Tick 数据
    pub fn get_ticks_data(&self, symbol: &str, view_width: usize, right_id: i64) -> Result<TickSeriesData> {
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
                let selected_ids = collect_windowed_ids(tick_map, right_id, view_width, self.config.default_view_width);
                let mut ticks: Vec<Tick> = Vec::with_capacity(selected_ids.len());
                for id in selected_ids {
                    let id_key = id.to_string();
                    let Some(tick_data) = tick_map.get(&id_key) else {
                        continue;
                    };
                    match self.convert_to_struct::<Tick>(tick_data) {
                        Ok(mut tick) => {
                            tick.id = id;
                            ticks.push(tick);
                        }
                        Err(e) => {
                            error!("{}", TqError::ParseError(format!("Tick数据格式错误: id={}, {}", id, e)));
                        }
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
    let target_width = if view_width > 0 { view_width } else { default_view_width };
    if ids.len() > target_width {
        let split_index = ids.len() - target_width;
        ids.drain(0..split_index);
    }
    ids
}

struct MultiKlineQuerySnapshot {
    left_id: i64,
    right_id: i64,
    metadata: HashMap<String, KlineMetadata>,
    main_data: Arc<HashMap<i64, Value>>,
    aligned_data_indexes: HashMap<String, Arc<HashMap<i64, Value>>>,
    bindings: HashMap<String, HashMap<i64, i64>>,
}

fn build_owned_numeric_value_index(data_map: &Map<String, Value>) -> HashMap<i64, Value> {
    let mut index = HashMap::with_capacity(data_map.len());
    for (id_str, value) in data_map {
        if let Ok(id) = id_str.parse::<i64>() {
            index.insert(id, value.clone());
        }
    }
    index
}

fn numeric_value_index_cache_key(symbol: &str, duration_str: &str) -> String {
    format!("klines:{symbol}:{duration_str}")
}
