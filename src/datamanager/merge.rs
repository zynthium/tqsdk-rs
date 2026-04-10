use super::{DataManager, MergeOptions, MergeSemanticsConfig, PrototypeBranch};
use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::debug;

// ---------------------------------------------------------------------------
// MergeTarget trait — 统一 HashMap<String, Value> 和 Map<String, Value> 的操作
// ---------------------------------------------------------------------------

trait MergeTarget {
    fn mt_get(&self, key: &str) -> Option<&Value>;
    fn mt_contains_key(&self, key: &str) -> bool;
    fn mt_insert(&mut self, key: String, value: Value) -> Option<Value>;
    fn mt_remove(&mut self, key: &str) -> Option<Value>;
    fn mt_entry_or_insert(&mut self, key: String, default: Value) -> &mut Value;
}

impl MergeTarget for HashMap<String, Value> {
    fn mt_get(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
    fn mt_contains_key(&self, key: &str) -> bool {
        self.contains_key(key)
    }
    fn mt_insert(&mut self, key: String, value: Value) -> Option<Value> {
        self.insert(key, value)
    }
    fn mt_remove(&mut self, key: &str) -> Option<Value> {
        self.remove(key)
    }
    fn mt_entry_or_insert(&mut self, key: String, default: Value) -> &mut Value {
        self.entry(key).or_insert(default)
    }
}

impl MergeTarget for Map<String, Value> {
    fn mt_get(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
    fn mt_contains_key(&self, key: &str) -> bool {
        self.contains_key(key)
    }
    fn mt_insert(&mut self, key: String, value: Value) -> Option<Value> {
        self.insert(key, value)
    }
    fn mt_remove(&mut self, key: &str) -> Option<Value> {
        self.remove(key)
    }
    fn mt_entry_or_insert(&mut self, key: String, default: Value) -> &mut Value {
        self.entry(key).or_insert(default)
    }
}

impl DataManager {
    /// 合并数据（DIFF 协议核心）
    ///
    /// # 参数
    ///
    /// * `source` - 源数据（可以是单个对象或数组）
    /// * `epoch_increase` - 是否增加版本号
    /// * `delete_null` - 是否删除 null 对象
    pub fn merge_data(&self, source: Value, epoch_increase: bool, delete_null: bool) {
        self.merge_data_with_semantics(source, epoch_increase, delete_null, self.config.merge_semantics.clone());
    }

    pub(crate) fn merge_data_with_semantics(
        &self,
        source: Value,
        epoch_increase: bool,
        delete_null: bool,
        semantics: MergeSemanticsConfig,
    ) {
        let options = MergeOptions::new(delete_null, semantics);
        let should_notify_watchers = if epoch_increase {
            let current_epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let source_arr = match source {
                Value::Array(arr) => arr,
                Value::Object(_) => vec![source],
                _ => {
                    debug!("merge_data: 无效的源数据类型");
                    return;
                }
            };

            let mut data = self.data.write().unwrap();
            for item in &source_arr {
                if let Value::Object(obj) = item
                    && !obj.is_empty()
                {
                    self.merge_into(
                        &mut *data,
                        obj,
                        current_epoch,
                        options.delete_null,
                        Some(&options.prototype),
                        &options,
                        options.persist,
                    );
                }
            }
            self.apply_python_data_semantics(&mut data, current_epoch);
            drop(data);

            // Publish the latest completed epoch after merge so watch receivers always
            // observe a completed state snapshot.
            let completed_epoch = self.epoch.load(Ordering::SeqCst);
            self.epoch_tx.send_replace(completed_epoch);

            true
        } else {
            let current_epoch = self.epoch.load(Ordering::SeqCst);
            let source_arr = match source {
                Value::Array(arr) => arr,
                Value::Object(_) => vec![source],
                _ => return,
            };

            let mut data = self.data.write().unwrap();
            for item in &source_arr {
                if let Value::Object(obj) = item
                    && !obj.is_empty()
                {
                    self.merge_into(
                        &mut *data,
                        obj,
                        current_epoch,
                        options.delete_null,
                        Some(&options.prototype),
                        &options,
                        options.persist,
                    );
                }
            }
            self.apply_python_data_semantics(&mut data, current_epoch);

            false
        };

        if should_notify_watchers {
            self.notify_watchers();
        }
    }

    /// 统一的递归合并实现，适用于 HashMap 和 Map
    #[expect(
        clippy::too_many_arguments,
        reason = "递归合并需要显式传递上下文，避免频繁构造中间对象"
    )]
    fn merge_into<T: MergeTarget>(
        &self,
        target: &mut T,
        source: &Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        for (property, value) in source {
            let (property_prototype, proto_branch) = resolve_child_prototype(prototype, property);
            let transformed_value = transform_value_by_prototype(value, property_prototype, proto_branch);
            let child_persist = persist_ctx || matches!(proto_branch, PrototypeBranch::Hash);

            if value.is_null() {
                if options.reduce_diff && (persist_ctx || has_hash_prototype(prototype)) {
                    continue;
                }
                if delete_null && target.mt_remove(property).is_some() {
                    changed = true;
                }
                continue;
            }

            match transformed_value {
                Value::String(ref s) if s == "NaN" || s == "-" => {
                    if options.reduce_diff && target.mt_get(property).is_some_and(|v| v.is_null()) {
                        continue;
                    }
                    target.mt_insert(property.clone(), Value::Null);
                    changed = true;
                }
                Value::Object(ref obj) => {
                    if property == "quotes" {
                        if self.merge_quotes_into(
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
                        let existed = target.mt_contains_key(property);
                        if !existed {
                            target.mt_insert(
                                property.clone(),
                                default_object_by_branch(property_prototype, proto_branch),
                            );
                        }
                        let target_obj = target.mt_entry_or_insert(property.clone(), Value::Object(Map::new()));
                        if !target_obj.is_object() {
                            *target_obj = Value::Object(Map::new());
                        }
                        if let Value::Object(target_map) = target_obj {
                            let child_changed = self.merge_into(
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
                                target.mt_remove(property);
                            }
                        }
                    }
                }
                _ => {
                    if options.reduce_diff && target.mt_get(property) == Some(&transformed_value) {
                        continue;
                    }
                    target.mt_insert(property.clone(), transformed_value);
                    changed = true;
                }
            }
        }

        if changed || !options.reduce_diff {
            target.mt_insert("_epoch".to_string(), Value::Number(epoch.into()));
        }
        changed
    }

    /// 统一的行情合并实现
    #[expect(clippy::too_many_arguments, reason = "行情合并路径需要携带完整上下文参数")]
    fn merge_quotes_into<T: MergeTarget>(
        &self,
        target: &mut T,
        quotes: &Map<String, Value>,
        epoch: i64,
        delete_null: bool,
        prototype: Option<&Value>,
        options: &MergeOptions,
        persist_ctx: bool,
    ) -> bool {
        let mut changed = false;
        let quotes_obj = target.mt_entry_or_insert("quotes".to_string(), Value::Object(Map::new()));

        if let Value::Object(quotes_map) = quotes_obj {
            for (symbol, quote_data) in quotes {
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
                        quotes_map.insert(symbol.clone(), default_object_by_branch(symbol_prototype, proto_branch));
                    }
                    let target_quote = quotes_map
                        .entry(symbol.clone())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if !target_quote.is_object() {
                        *target_quote = Value::Object(Map::new());
                    }

                    if let Value::Object(target_quote_map) = target_quote {
                        let child_changed = self.merge_into(
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
            let exchange_order_id = order.get("exchange_order_id").and_then(|v| v.as_str()).unwrap_or("");
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
            let entry = order_trade_stats.entry(trade_order_id.to_string()).or_insert((0, 0.0));
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
            if let Some((sum_volume, sum_amount)) = order_trade_stats.get(order_id)
                && let Some(number) = serde_json::Number::from_f64(*sum_amount / *sum_volume as f64)
            {
                order.insert("trade_price".to_string(), Value::Number(number));
                order.insert("_epoch".to_string(), Value::Number(epoch.into()));
            }
        }
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

fn resolve_child_prototype<'a>(prototype: Option<&'a Value>, key: &str) -> (Option<&'a Value>, PrototypeBranch) {
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

fn transform_value_by_prototype(value: &Value, prototype: Option<&Value>, branch: PrototypeBranch) -> Value {
    match (value, prototype, branch) {
        (Value::String(_), Some(proto), PrototypeBranch::Direct) if !proto.is_string() => proto.clone(),
        _ => value.clone(),
    }
}

fn default_object_by_branch(prototype: Option<&Value>, branch: PrototypeBranch) -> Value {
    if matches!(branch, PrototypeBranch::At | PrototypeBranch::Hash) && prototype.is_some_and(Value::is_object) {
        return prototype.cloned().unwrap_or_else(|| Value::Object(Map::new()));
    }
    Value::Object(Map::new())
}
