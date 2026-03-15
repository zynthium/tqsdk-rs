//! 数据管理器
//!
//! 实现 DIFF 协议的数据合并和管理，包括：
//! - DIFF 数据递归合并
//! - 路径访问和版本追踪
//! - Watch/UnWatch 路径监听
//! - 数据类型转换

use crate::errors::{Result, TqError};
use crate::types::*;
use crate::utils::{nanos_to_datetime, value_to_i64};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use async_channel::{Sender, Receiver, unbounded};
use tracing::{debug, error};

type DataCallbacks = Arc<RwLock<Vec<Arc<dyn Fn() + Send + Sync>>>>;

#[derive(Debug, Clone, Default)]
pub struct MergeSemanticsConfig {
    pub persist: bool,
    pub reduce_diff: bool,
    pub prototype: Option<Value>,
}

/// 数据管理器配置
#[derive(Debug, Clone)]
pub struct DataManagerConfig {
    /// 默认视图宽度
    pub default_view_width: usize,
    /// 启用自动清理
    pub enable_auto_cleanup: bool,
    pub merge_semantics: MergeSemanticsConfig,
}

impl Default for DataManagerConfig {
    fn default() -> Self {
        DataManagerConfig {
            default_view_width: 10000,
            enable_auto_cleanup: true,
            merge_semantics: MergeSemanticsConfig::default(),
        }
    }
}

/// 路径监听器
struct PathWatcher {
    path: Vec<String>,
    tx: Sender<Value>,
}

/// 数据管理器
///
/// 管理所有 DIFF 协议数据，支持：
/// - 递归合并
/// - 版本追踪
/// - 路径监听
pub struct DataManager {
    /// 数据存储
    data: Arc<RwLock<HashMap<String, Value>>>,
    /// 版本号（使用原子变量以提高性能）
    epoch: AtomicI64,
    /// 配置
    config: DataManagerConfig,
    /// 路径监听器
    watchers: Arc<RwLock<HashMap<String, PathWatcher>>>,
    /// 数据更新回调（使用 Arc 以支持异步触发）
    on_data_callbacks: DataCallbacks,
}

#[derive(Clone)]
struct MergeOptions {
    delete_null: bool,
    persist: bool,
    reduce_diff: bool,
    prototype: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrototypeBranch {
    Direct,
    Star,
    At,
    Hash,
    None,
}

impl MergeOptions {
    fn new(delete_null: bool, semantics: MergeSemanticsConfig) -> Self {
        let prototype = semantics
            .prototype
            .unwrap_or_else(|| Value::Object(Map::new()));
        MergeOptions {
            delete_null,
            persist: semantics.persist,
            reduce_diff: semantics.reduce_diff,
            prototype,
        }
    }
}

impl DataManager {
    /// 创建新的数据管理器
    pub fn new(initial_data: HashMap<String, Value>, config: DataManagerConfig) -> Self {
        DataManager {
            data: Arc::new(RwLock::new(initial_data)),
            epoch: AtomicI64::new(0),
            config,
            watchers: Arc::new(RwLock::new(HashMap::new())),
            on_data_callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 注册数据更新回调
    pub fn on_data<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut callbacks = self.on_data_callbacks.write().unwrap();
        callbacks.push(Arc::new(callback));
    }

    /// 合并数据（DIFF 协议核心）
    ///
    /// # 参数
    ///
    /// * `source` - 源数据（可以是单个对象或数组）
    /// * `epoch_increase` - 是否增加版本号
    /// * `delete_null` - 是否删除 null 对象
    pub fn merge_data(&self, source: Value, epoch_increase: bool, delete_null: bool) {
        self.merge_data_with_semantics(
            source,
            epoch_increase,
            delete_null,
            self.config.merge_semantics.clone(),
        );
    }

    pub fn merge_data_with_semantics(
        &self,
        source: Value,
        epoch_increase: bool,
        delete_null: bool,
        semantics: MergeSemanticsConfig,
    ) {
        let options = MergeOptions::new(delete_null, semantics);
        let should_notify_watchers = if epoch_increase {
            // 增加版本号（使用原子操作）
            let current_epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;

            // 转换为数组
            let source_arr = match source {
                Value::Array(arr) => arr,
                Value::Object(_) => vec![source],
                _ => {
                    debug!("merge_data: 无效的源数据类型");
                    return;
                }
            };

            // 合并数据
            let mut data = self.data.write().unwrap();
            for item in source_arr.iter() {
                if let Value::Object(obj) = item {
                    if !obj.is_empty() {
                        self.merge_object(
                            &mut data,
                            obj,
                            current_epoch,
                            options.delete_null,
                            Some(&options.prototype),
                            &options,
                            options.persist,
                        );
                    }
                }
            }
            self.apply_python_data_semantics(&mut data, current_epoch);
            drop(data);

            // 异步触发回调（不阻塞数据合并）
            let callbacks = self.on_data_callbacks.read().unwrap();
            for callback in callbacks.iter() {
                let cb = Arc::clone(callback);
                tokio::spawn(async move {
                    cb();
                });
            }
            drop(callbacks);

            true
        } else {
            // 不增加版本号，只合并（使用原子操作读取）
            let current_epoch = self.epoch.load(Ordering::SeqCst);

            let source_arr = match source {
                Value::Array(arr) => arr,
                Value::Object(_) => vec![source],
                _ => return,
            };

            let mut data = self.data.write().unwrap();
            for item in source_arr.iter() {
                if let Value::Object(obj) = item {
                    if !obj.is_empty() {
                        self.merge_object(
                            &mut data,
                            obj,
                            current_epoch,
                            options.delete_null,
                            Some(&options.prototype),
                            &options,
                            options.persist,
                        );
                    }
                }
            }
            self.apply_python_data_semantics(&mut data, current_epoch);

            false
        };

        // 通知 watchers
        if should_notify_watchers {
            self.notify_watchers();
        }
    }

    fn merge_object(
        &self,
        target: &mut HashMap<String, Value>,
        source: &serde_json::Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        for (property, value) in source.iter() {
            let (property_prototype, proto_branch) = resolve_child_prototype(prototype, property);
            let transformed_value =
                transform_value_by_prototype(value, property_prototype, proto_branch);
            let child_persist = persist_ctx || matches!(proto_branch, PrototypeBranch::Hash);

            if value.is_null() {
                if options.reduce_diff && (persist_ctx || has_hash_prototype(prototype)) {
                    continue;
                }
                if delete_null && target.remove(property).is_some() {
                    changed = true;
                }
                continue;
            }

            match transformed_value {
                Value::String(ref s) if s == "NaN" || s == "-" => {
                    if options.reduce_diff && target.get(property).is_some_and(|v| v.is_null()) {
                        continue;
                    }
                    target.insert(property.clone(), Value::Null);
                    changed = true;
                }
                Value::Object(ref obj) => {
                    if property == "quotes" {
                        if self.merge_quotes(
                            target,
                            obj,
                            epoch,
                            delete_null,
                            property_prototype,
                            options,
                            child_persist,
                        ) {
                            changed = true;
                        }
                    } else {
                        let existed = target.contains_key(property);
                        if !existed {
                            target.insert(
                                property.clone(),
                                default_object_by_branch(property_prototype, proto_branch),
                            );
                        }
                        let target_obj = target
                            .entry(property.clone())
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if !target_obj.is_object() {
                            *target_obj = Value::Object(serde_json::Map::new());
                        }
                        if let Value::Object(target_map) = target_obj {
                            let child_changed = self.merge_object_map(
                                target_map,
                                obj,
                                epoch,
                                delete_null,
                                property_prototype,
                                options,
                                child_persist,
                            );
                            if child_changed {
                                changed = true;
                            } else if !existed && options.reduce_diff {
                                target.remove(property);
                            }
                        }
                    }
                }
                _ => {
                    if options.reduce_diff && target.get(property) == Some(&transformed_value) {
                        continue;
                    }
                    target.insert(property.clone(), transformed_value);
                    changed = true;
                }
            }
        }

        if changed || !options.reduce_diff {
            target.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
        changed
    }

    fn merge_object_map(
        &self,
        target: &mut Map<String, Value>,
        source: &Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        for (property, value) in source.iter() {
            let (property_prototype, proto_branch) = resolve_child_prototype(prototype, property);
            let transformed_value =
                transform_value_by_prototype(value, property_prototype, proto_branch);
            let child_persist = persist_ctx || matches!(proto_branch, PrototypeBranch::Hash);

            if value.is_null() {
                if options.reduce_diff && (persist_ctx || has_hash_prototype(prototype)) {
                    continue;
                }
                if delete_null && target.remove(property).is_some() {
                    changed = true;
                }
                continue;
            }

            match transformed_value {
                Value::String(ref s) if s == "NaN" || s == "-" => {
                    if options.reduce_diff && target.get(property).is_some_and(|v| v.is_null()) {
                        continue;
                    }
                    target.insert(property.clone(), Value::Null);
                    changed = true;
                }
                Value::Object(ref obj) => {
                    if property == "quotes" {
                        if self.merge_quotes_map(
                            target,
                            obj,
                            epoch,
                            delete_null,
                            property_prototype,
                            options,
                            child_persist,
                        ) {
                            changed = true;
                        }
                    } else {
                        let existed = target.contains_key(property);
                        if !existed {
                            target.insert(
                                property.clone(),
                                default_object_by_branch(property_prototype, proto_branch),
                            );
                        }
                        let target_obj = target
                            .entry(property.clone())
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if !target_obj.is_object() {
                            *target_obj = Value::Object(serde_json::Map::new());
                        }
                        if let Value::Object(target_map) = target_obj {
                            let child_changed = self.merge_object_map(
                                target_map,
                                obj,
                                epoch,
                                delete_null,
                                property_prototype,
                                options,
                                child_persist,
                            );
                            if child_changed {
                                changed = true;
                            } else if !existed && options.reduce_diff {
                                target.remove(property);
                            }
                        }
                    }
                }
                _ => {
                    if options.reduce_diff && target.get(property) == Some(&transformed_value) {
                        continue;
                    }
                    target.insert(property.clone(), transformed_value);
                    changed = true;
                }
            }
        }

        if changed || !options.reduce_diff {
            target.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
        changed
    }

    fn merge_quotes(
        &self,
        target: &mut HashMap<String, Value>,
        quotes: &serde_json::Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        let quotes_obj = target
            .entry("quotes".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));

        if let Value::Object(quotes_map) = quotes_obj {
            for (symbol, quote_data) in quotes.iter() {
                let (symbol_prototype, proto_branch) = resolve_child_prototype(prototype, symbol);
                let child_persist = persist_ctx || matches!(proto_branch, PrototypeBranch::Hash);
                if quote_data.is_null() {
                    if options.reduce_diff && (persist_ctx || has_hash_prototype(prototype)) {
                        continue;
                    }
                    if delete_null && quotes_map.remove(symbol).is_some() {
                        changed = true;
                    }
                    continue;
                }

                if let Value::Object(quote_obj) = quote_data {
                    let existed = quotes_map.contains_key(symbol);
                    if !existed {
                        quotes_map.insert(
                            symbol.clone(),
                            default_object_by_branch(symbol_prototype, proto_branch),
                        );
                    }
                    let target_quote = quotes_map
                        .entry(symbol.clone())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if !target_quote.is_object() {
                        *target_quote = Value::Object(serde_json::Map::new());
                    }

                    if let Value::Object(target_quote_map) = target_quote {
                        let child_changed = self.merge_object_map(
                            target_quote_map,
                            quote_obj,
                            epoch,
                            delete_null,
                            symbol_prototype,
                            options,
                            child_persist,
                        );
                        if child_changed {
                            changed = true;
                        } else if !existed && options.reduce_diff {
                            quotes_map.remove(symbol);
                        }
                    }
                }
            }
        }
        changed
    }

    fn merge_quotes_map(
        &self,
        target: &mut Map<String, Value>,
        quotes: &Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        let quotes_obj = target
            .entry("quotes".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));

        if let Value::Object(quotes_map) = quotes_obj {
            for (symbol, quote_data) in quotes.iter() {
                let (symbol_prototype, proto_branch) = resolve_child_prototype(prototype, symbol);
                let child_persist = persist_ctx || matches!(proto_branch, PrototypeBranch::Hash);
                if quote_data.is_null() {
                    if options.reduce_diff && (persist_ctx || has_hash_prototype(prototype)) {
                        continue;
                    }
                    if delete_null && quotes_map.remove(symbol).is_some() {
                        changed = true;
                    }
                    continue;
                }

                if let Value::Object(quote_obj) = quote_data {
                    let existed = quotes_map.contains_key(symbol);
                    if !existed {
                        quotes_map.insert(
                            symbol.clone(),
                            default_object_by_branch(symbol_prototype, proto_branch),
                        );
                    }
                    let target_quote = quotes_map
                        .entry(symbol.clone())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if !target_quote.is_object() {
                        *target_quote = Value::Object(serde_json::Map::new());
                    }

                    if let Value::Object(target_quote_map) = target_quote {
                        let child_changed = self.merge_object_map(
                            target_quote_map,
                            quote_obj,
                            epoch,
                            delete_null,
                            symbol_prototype,
                            options,
                            child_persist,
                        );
                        if child_changed {
                            changed = true;
                        } else if !existed && options.reduce_diff {
                            quotes_map.remove(symbol);
                        }
                    }
                }
            }
        }
        changed
    }

    fn apply_python_data_semantics(&self, root: &mut HashMap<String, Value>, epoch: i64) {
        self.apply_quote_expire_rest_days(root, epoch);
        self.apply_trade_derived_fields(root, epoch);
    }

    fn apply_quote_expire_rest_days(&self, root: &mut HashMap<String, Value>, epoch: i64) {
        let now_ts = Utc::now().timestamp();
        let Some(Value::Object(quotes)) = root.get_mut("quotes") else {
            return;
        };
        for quote_val in quotes.values_mut() {
            let Some(quote) = quote_val.as_object_mut() else {
                continue;
            };
            let Some(expire_raw) = quote.get("expire_datetime").and_then(to_i64_any) else {
                continue;
            };
            let expire_secs = normalize_ts_to_secs(expire_raw);
            let days = (expire_secs / 86_400) - (now_ts / 86_400);
            quote.insert("expire_rest_days".to_string(), Value::Number(days.into()));
            quote.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
    }

    fn apply_trade_derived_fields(&self, root: &mut HashMap<String, Value>, epoch: i64) {
        let Some(Value::Object(trade_map)) = root.get_mut("trade") else {
            return;
        };
        for user_val in trade_map.values_mut() {
            let Some(user_map) = user_val.as_object_mut() else {
                continue;
            };
            self.apply_positions_derived(user_map, epoch);
            self.apply_orders_derived(user_map, epoch);
            self.apply_orders_trade_price(user_map, epoch);
            user_map.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
    }

    fn apply_positions_derived(&self, user_map: &mut Map<String, Value>, epoch: i64) {
        let Some(Value::Object(positions)) = user_map.get_mut("positions") else {
            return;
        };
        for pos_val in positions.values_mut() {
            let Some(pos) = pos_val.as_object_mut() else {
                continue;
            };
            let long_his = pos.get("pos_long_his").and_then(to_i64_any).unwrap_or(0);
            let long_today = pos.get("pos_long_today").and_then(to_i64_any).unwrap_or(0);
            let short_his = pos.get("pos_short_his").and_then(to_i64_any).unwrap_or(0);
            let short_today = pos.get("pos_short_today").and_then(to_i64_any).unwrap_or(0);
            let pos_long = long_his + long_today;
            let pos_short = short_his + short_today;
            pos.insert("pos_long".to_string(), Value::Number(pos_long.into()));
            pos.insert("pos_short".to_string(), Value::Number(pos_short.into()));
            pos.insert("pos".to_string(), Value::Number((pos_long - pos_short).into()));
            pos.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
    }

    fn apply_orders_derived(&self, user_map: &mut Map<String, Value>, epoch: i64) {
        let Some(Value::Object(orders)) = user_map.get_mut("orders") else {
            return;
        };
        for order_val in orders.values_mut() {
            let Some(order) = order_val.as_object_mut() else {
                continue;
            };
            let status = order.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let exchange_order_id = order
                .get("exchange_order_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_dead = status == "FINISHED";
            let is_online = !exchange_order_id.is_empty() && status == "ALIVE";
            let is_error = exchange_order_id.is_empty() && status == "FINISHED";
            order.insert("is_dead".to_string(), Value::Bool(is_dead));
            order.insert("is_online".to_string(), Value::Bool(is_online));
            order.insert("is_error".to_string(), Value::Bool(is_error));
            order.insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
    }

    fn apply_orders_trade_price(&self, user_map: &mut Map<String, Value>, epoch: i64) {
        let Some(Value::Object(trades_map)) = user_map.get("trades") else {
            return;
        };
        let mut order_trade_stats: HashMap<String, (i64, f64)> = HashMap::with_capacity(trades_map.len());
        for trade_val in trades_map.values() {
            let Some(trade) = trade_val.as_object() else {
                continue;
            };
            let trade_order_id = trade.get("order_id").and_then(|v| v.as_str()).unwrap_or("");
            if trade_order_id.is_empty() {
                continue;
            }
            let volume = trade.get("volume").and_then(to_i64_any).unwrap_or(0);
            if volume <= 0 {
                continue;
            }
            let price = trade.get("price").and_then(to_f64_any).unwrap_or(0.0);
            let entry = order_trade_stats
                .entry(trade_order_id.to_string())
                .or_insert((0, 0.0));
            entry.0 += volume;
            entry.1 += price * volume as f64;
        }
        let Some(Value::Object(orders)) = user_map.get_mut("orders") else {
            return;
        };

        for (order_id, order_val) in orders.iter_mut() {
            let Some(order) = order_val.as_object_mut() else {
                continue;
            };
            if let Some((sum_volume, sum_amount)) = order_trade_stats.get(order_id) {
                if let Some(number) = serde_json::Number::from_f64(*sum_amount / *sum_volume as f64) {
                    order.insert("trade_price".to_string(), Value::Number(number));
                    order.insert("_epoch".to_string(), Value::Number(epoch.into()));
                }
            }
        }
    }

    /// 根据路径获取数据
    pub fn get_by_path(&self, path: &[&str]) -> Option<Value> {
        if path.is_empty() {
            return None;
        }

        let data = self.data.read().unwrap();
        get_by_path_ref(&data, path).cloned()
    }

    /// 判断指定路径的数据是否在最近一次更新中发生了变化
    pub fn is_changing(&self, path: &[&str]) -> bool {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        let data = self.data.read().unwrap();

        if path.is_empty() {
            return false;
        }

        // 第一层直接从 HashMap 获取
        let mut current_value = data.get(path[0]);

        // 遍历路径
        for &key in path[1..].iter() {
            if let Some(Value::Object(map)) = current_value {
                current_value = map.get(key);
            } else {
                return false;
            }
        }

        // 检查 _epoch
        if let Some(Value::Object(map)) = current_value {
            if let Some(Value::Number(epoch_val)) = map.get("_epoch") {
                if let Some(epoch) = epoch_val.as_i64() {
                    return epoch == current_epoch;
                }
            }
        }

        false
    }

    /// 获取当前版本号
    pub fn get_epoch(&self) -> i64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// 设置默认值
    pub fn set_default(&self, path: &[&str], default_value: Value) -> Option<Value> {
        let mut data = self.data.write().unwrap();
        let current = &mut *data;

        for (i, &key) in path.iter().enumerate() {
            if i == path.len() - 1 {
                // 最后一个 key
                if !current.contains_key(key) {
                    current.insert(key.to_string(), default_value.clone());
                }
                return current.get(key).cloned();
            }

            // 中间节点
            let entry = current
                .entry(key.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));

            // 无法继续（类型不匹配）
            if !entry.is_object() {
                return None;
            }
        }

        None
    }

    /// 监听指定路径的数据变化
    ///
    /// 返回一个 receiver，数据变化时会推送到这个 channel
    pub fn watch(&self, path: Vec<String>) -> Receiver<Value> {
        let path_key = path.join(".");
        let (tx, rx) = unbounded();

        let watcher = PathWatcher {
            path: path.clone(),
            tx,
        };

        let mut watchers = self.watchers.write().unwrap();
        watchers.insert(path_key, watcher);

        rx
    }

    /// 取消路径监听
    pub fn unwatch(&self, path: &[String]) -> Result<()> {
        let path_key = path.join(".");
        let mut watchers = self.watchers.write().unwrap();

        if watchers.remove(&path_key).is_none() {
            return Err(TqError::DataNotFound(format!("路径未监听: {}", path_key)));
        }

        Ok(())
    }

    /// 通知所有 watchers
    fn notify_watchers(&self) {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        let watchers = self.watchers.read().unwrap();
        let data = self.data.read().unwrap();
        for watcher in watchers.values() {
            if is_path_epoch_changed(&data, &watcher.path, current_epoch) {
                if let Some(data) = get_by_path_ref_strings(&data, &watcher.path).cloned() {
                    let tx = watcher.tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(data).await;
                    });
                }
            }
        }
    }

    /// 转换为结构体
    pub fn convert_to_struct<T: DeserializeOwned>(&self, data: &Value) -> Result<T> {
        T::deserialize(data)
            .map_err(|e| TqError::ParseError(format!("转换失败: {}", e)))
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

            // 转换 data map 为数组
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
        let data_guard = self.data.read().unwrap();

        // 获取 Chart 信息
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

        // 获取每个合约的元数据
        for symbol in symbols.iter() {
            if let Some(Value::Object(kline_map)) = get_by_path_ref(
                &data_guard,
                &["klines", symbol.as_str(), duration_str.as_str()],
            )
            {
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
            }
        }

        // 获取主合约的 K线数据
        if let Some(Value::Object(main_kline_map)) = get_by_path_ref(
            &data_guard,
            &["klines", main_symbol.as_str(), duration_str.as_str()],
        )
        {
            // 获取 binding 信息
            let mut bindings: HashMap<String, HashMap<i64, i64>> = HashMap::new();
            if let Some(Value::Object(binding_map)) = main_kline_map.get("binding") {
                for (symbol, binding_info) in binding_map.iter() {
                    if let Value::Object(binding_id_map) = binding_info {
                        let mut id_map: HashMap<i64, i64> = HashMap::new();
                        for (main_id_str, other_id) in binding_id_map.iter() {
                            if let Ok(main_id) = main_id_str.parse::<i64>() {
                                id_map.insert(main_id, value_to_i64(other_id));
                            }
                        }
                        bindings.insert(symbol.clone(), id_map);
                    }
                }
            }

            // 获取主合约的 K线 map
            if let Some(Value::Object(main_data_map)) = main_kline_map.get("data") {
                let mut main_ids: Vec<i64> = main_data_map
                    .keys()
                    .filter_map(|k| k.parse::<i64>().ok())
                    .collect();
                main_ids.sort();

                // 过滤超出 right_id 的数据
                if right_id > 0 {
                    main_ids.retain(|&id| id <= right_id);
                }

                // 应用 ViewWidth 限制
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

                // 对齐所有合约的 K线
                for main_id in main_ids {
                    let main_id_str = main_id.to_string();
                    let mut set = AlignedKlineSet {
                        main_id,
                        timestamp: Utc::now(),
                        klines: HashMap::new(),
                    };

                    // 添加主合约 K线
                    if let Some(kline_data) = main_data_map.get(&main_id_str) {
                        if let Ok(mut kline) = self.convert_to_struct::<Kline>(kline_data) {
                            kline.id = main_id;
                            set.timestamp = nanos_to_datetime(kline.datetime);
                            set.klines.insert(main_symbol.clone(), kline);
                        }
                    }

                    // 添加其他合约的对齐 K线
                    for symbol in symbols.iter().skip(1) {
                        if let Some(binding) = bindings.get(symbol) {
                            if let Some(&mapped_id) = binding.get(&main_id) {
                                let mapped_id_str = mapped_id.to_string();
                                if let Some(other_kline_data) = get_by_path_ref(
                                    &data_guard,
                                    &[
                                        "klines",
                                        symbol.as_str(),
                                        duration_str.as_str(),
                                        "data",
                                        mapped_id_str.as_str(),
                                    ],
                                ) {
                                    if let Ok(mut kline) = self.convert_to_struct::<Kline>(other_kline_data) {
                                        kline.id = mapped_id;
                                        set.klines.insert(symbol.clone(), kline);
                                    }
                                }
                            }
                        }
                    }

                    result.data.push(set);
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

            // 转换 data map 为数组
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
            .ok_or_else(|| {
                TqError::DataNotFound(format!("账户未找到: {}/{}", user_id, currency))
            })?;

        self.convert_to_struct(&data)
    }

    /// 获取持仓数据
    pub fn get_position_data(&self, user_id: &str, symbol: &str) -> Result<Position> {
        let data = self
            .get_by_path(&["trade", user_id, "positions", symbol])
            .ok_or_else(|| TqError::DataNotFound(format!("持仓未找到: {}/{}", user_id, symbol)))?;

        self.convert_to_struct(&data)
    }

    /// 获取委托单数据
    pub fn get_order_data(&self, user_id: &str, order_id: &str) -> Result<Order> {
        let data = self
            .get_by_path(&["trade", user_id, "orders", order_id])
            .ok_or_else(|| {
                TqError::DataNotFound(format!("委托单未找到: {}/{}", user_id, order_id))
            })?;

        self.convert_to_struct(&data)
    }

    /// 获取成交数据
    pub fn get_trade_data(&self, user_id: &str, trade_id: &str) -> Result<Trade> {
        let data = self
            .get_by_path(&["trade", user_id, "trades", trade_id])
            .ok_or_else(|| {
                TqError::DataNotFound(format!("成交未找到: {}/{}", user_id, trade_id))
            })?;

        self.convert_to_struct(&data)
    }
}

fn to_i64_any(value: &Value) -> Option<i64> {
    match value {
        Value::Number(num) => num
            .as_i64()
            .or_else(|| num.as_u64().map(|v| v as i64))
            .or_else(|| num.as_f64().map(|v| v as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn get_by_path_ref<'a>(data: &'a HashMap<String, Value>, path: &[&str]) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = data.get(path[0])?;
    for &key in path[1..].iter() {
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

fn get_by_path_ref_strings<'a>(
    data: &'a HashMap<String, Value>,
    path: &[String],
) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = data.get(&path[0])?;
    for key in &path[1..] {
        match current {
            Value::Object(map) => {
                current = map.get(key)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn is_path_epoch_changed(data: &HashMap<String, Value>, path: &[String], current_epoch: i64) -> bool {
    if let Some(Value::Object(map)) = get_by_path_ref_strings(data, path) {
        if let Some(Value::Number(epoch_val)) = map.get("_epoch") {
            if let Some(epoch) = epoch_val.as_i64() {
                return epoch == current_epoch;
            }
        }
    }
    false
}

fn to_f64_any(value: &Value) -> Option<f64> {
    match value {
        Value::Number(num) => num.as_f64().or_else(|| num.as_i64().map(|v| v as f64)),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn normalize_ts_to_secs(ts: i64) -> i64 {
    if ts.abs() >= 1_000_000_000_000_000_000 {
        ts / 1_000_000_000
    } else if ts.abs() >= 1_000_000_000_000 {
        ts / 1_000
    } else {
        ts
    }
}

fn resolve_child_prototype<'a>(
    prototype: Option<&'a Value>,
    key: &str,
) -> (Option<&'a Value>, PrototypeBranch) {
    let Some(Value::Object(proto_map)) = prototype else {
        return (None, PrototypeBranch::None);
    };
    if let Some(p) = proto_map.get(key) {
        return (Some(p), PrototypeBranch::Direct);
    }
    if let Some(p) = proto_map.get("*") {
        return (Some(p), PrototypeBranch::Star);
    }
    if let Some(p) = proto_map.get("@") {
        return (Some(p), PrototypeBranch::At);
    }
    if let Some(p) = proto_map.get("#") {
        return (Some(p), PrototypeBranch::Hash);
    }
    (None, PrototypeBranch::None)
}

fn has_hash_prototype(prototype: Option<&Value>) -> bool {
    matches!(prototype, Some(Value::Object(map)) if map.contains_key("#"))
}

fn transform_value_by_prototype(
    value: &Value,
    prototype: Option<&Value>,
    branch: PrototypeBranch,
) -> Value {
    match (value, prototype, branch) {
        (Value::String(_), Some(proto), PrototypeBranch::Direct) if !proto.is_string() => {
            proto.clone()
        }
        _ => value.clone(),
    }
}

fn default_object_by_branch(prototype: Option<&Value>, branch: PrototypeBranch) -> Value {
    if matches!(branch, PrototypeBranch::At | PrototypeBranch::Hash)
        && prototype.is_some_and(Value::is_object)
    {
        return prototype.cloned().unwrap_or_else(|| Value::Object(Map::new()));
    }
    Value::Object(Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_merge_data() {
        let initial_data = HashMap::new();
        let config = DataManagerConfig::default();
        let dm = DataManager::new(initial_data, config);

        let source = json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0,
                    "volume": 1000
                }
            }
        });

        dm.merge_data(source, true, false);

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]);
        assert!(quote.is_some());

        if let Some(Value::Object(quote_map)) = quote {
            assert_eq!(quote_map.get("last_price"), Some(&json!(500.0)));
            assert_eq!(quote_map.get("volume"), Some(&json!(1000)));
        }
    }

    #[test]
    fn test_is_changing() {
        let initial_data = HashMap::new();
        let config = DataManagerConfig::default();
        let dm = DataManager::new(initial_data, config);

        let source = json!({
            "quotes": {
                "SHFE.au2602": {
                    "last_price": 500.0
                }
            }
        });

        dm.merge_data(source, true, false);

        assert!(dm.is_changing(&["quotes", "SHFE.au2602"]));
        assert!(!dm.is_changing(&["quotes", "SHFE.ag2512"]));
    }

    #[test]
    fn test_merge_persist_mode() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.persist = true;
        config.merge_semantics.reduce_diff = true;
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": null
                    }
                }
            }),
            true,
            true,
        );

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
        assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
    }

    #[test]
    fn test_merge_prototype_mode() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.prototype = Some(json!({
            "quotes": {
                "*": {
                    "price_tick": 0.2
                }
            }
        }));
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "price_tick": "invalid"
                    }
                }
            }),
            true,
            true,
        );

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
        assert_eq!(quote.get("price_tick"), Some(&json!(0.2)));
    }

    #[test]
    fn test_merge_reduce_diff_mode() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.reduce_diff = true;
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        assert!(!dm.is_changing(&["quotes", "SHFE.au2602"]));
    }

    #[test]
    fn test_merge_prototype_at_branch_default_path() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.prototype = Some(json!({
            "quotes": {
                "@": {
                    "ins_class": "FUTURE",
                    "price_tick": 0.2
                }
            }
        }));
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
        assert_eq!(quote.get("ins_class"), Some(&json!("FUTURE")));
        assert_eq!(quote.get("price_tick"), Some(&json!(0.2)));
        assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
    }

    #[test]
    fn test_merge_prototype_hash_branch_persist_with_reduce_diff() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.reduce_diff = true;
        config.merge_semantics.prototype = Some(json!({
            "quotes": {
                "#": {
                    "price_tick": 0.2
                }
            }
        }));
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": null
                    }
                }
            }),
            true,
            true,
        );

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
        assert_eq!(quote.get("last_price"), Some(&json!(500.0)));
    }

    #[test]
    fn test_merge_prototype_hash_branch_delete_without_reduce_diff() {
        let initial_data = HashMap::new();
        let mut config = DataManagerConfig::default();
        config.merge_semantics.reduce_diff = false;
        config.merge_semantics.prototype = Some(json!({
            "quotes": {
                "#": {
                    "price_tick": 0.2
                }
            }
        }));
        let dm = DataManager::new(initial_data, config);

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": 500.0
                    }
                }
            }),
            true,
            true,
        );

        dm.merge_data(
            json!({
                "quotes": {
                    "SHFE.au2602": {
                        "last_price": null
                    }
                }
            }),
            true,
            true,
        );

        let quote = dm.get_by_path(&["quotes", "SHFE.au2602"]).unwrap();
        assert!(quote.get("last_price").is_none());
    }
}
