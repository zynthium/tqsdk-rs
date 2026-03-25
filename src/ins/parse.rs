use crate::errors::{Result, TqError};
use serde_json::{Map, Value, json};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use chrono::{Datelike, FixedOffset, TimeZone, Utc};

pub(crate) fn parse_query_quotes_result(res: &Value, target_ex: Option<&str>) -> Vec<String> {
    let mut list = Vec::new();
    if let Some(result_obj) = res.get("result")
        && let Some(arr) = result_obj
            .get("multi_symbol_info")
            .and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(ins) = item.get("instrument_id").and_then(|v| v.as_str()) {
                if let Some(ex) = target_ex {
                    if ins.contains(ex) {
                        list.push(ins.to_string());
                    }
                } else {
                    list.push(ins.to_string());
                }
            }
        }
    }
    list
}

pub(crate) fn parse_query_cont_quotes_result(
    res: &Value,
    exchange_id: Option<&str>,
    product_id: Option<&str>,
) -> Vec<String> {
    let mut list = Vec::new();
    if let Some(result_obj) = res.get("result")
        && let Some(arr) = result_obj
            .get("multi_symbol_info")
            .and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(underlying) = item.get("underlying")
                && let Some(edges) = underlying.get("edges").and_then(|v| v.as_array())
            {
                for edge in edges {
                    if let Some(node) = edge.get("node") {
                        let ex_ok = match exchange_id {
                            Some(ex) => node
                                .get("exchange_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s == ex)
                                .unwrap_or(false),
                            None => true,
                        };
                        let prod_ok = match product_id {
                            Some(pid) => node
                                .get("product_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s == pid)
                                .unwrap_or(false),
                            None => true,
                        };
                        if ex_ok
                            && prod_ok
                            && let Some(ins) = node.get("instrument_id").and_then(|v| v.as_str())
                        {
                            list.push(ins.to_string());
                        }
                    }
                }
            }
        }
    }
    list
}

pub(crate) fn parse_query_options_result(
    res: &Value,
    option_class: Option<&str>,
    exercise_year: Option<i32>,
    exercise_month: Option<i32>,
    strike_price: Option<f64>,
    expired: Option<bool>,
    has_a: Option<bool>,
) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(result_obj) = res.get("result")
        && let Some(arr) = result_obj
            .get("multi_symbol_info")
            .and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(der) = item.get("derivatives")
                && let Some(edges) = der.get("edges").and_then(|v| v.as_array())
            {
                for edge in edges {
                    if let Some(node) = edge.get("node") {
                        let mut ok = true;
                        if let Some(cls) = option_class {
                            ok = ok
                                && node
                                    .get("call_or_put")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s == cls)
                                    .unwrap_or(false);
                        }
                        if let Some(y) = exercise_year {
                            if let Some(ts) =
                                node.get("last_exercise_datetime").and_then(|v| v.as_i64())
                            {
                                let dt = chrono::DateTime::<Utc>::from_timestamp(
                                    ts / 1_000_000_000,
                                    (ts % 1_000_000_000) as u32,
                                )
                                .unwrap_or_else(|| {
                                    chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap()
                                });
                                ok = ok && dt.year() == y;
                            } else {
                                ok = false;
                            }
                        }
                        if let Some(m) = exercise_month {
                            if let Some(ts) =
                                node.get("last_exercise_datetime").and_then(|v| v.as_i64())
                            {
                                let dt = chrono::DateTime::<Utc>::from_timestamp(
                                    ts / 1_000_000_000,
                                    (ts % 1_000_000_000) as u32,
                                )
                                .unwrap_or_else(|| {
                                    chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap()
                                });
                                ok = ok && dt.month() as i32 == m;
                            } else {
                                ok = false;
                            }
                        }
                        if let Some(sp) = strike_price {
                            ok = ok
                                && node
                                    .get("strike_price")
                                    .and_then(|v| v.as_f64())
                                    .map(|x| (x - sp).abs() < f64::EPSILON)
                                    .unwrap_or(false);
                        }
                        if let Some(e) = expired {
                            ok = ok
                                && node
                                    .get("expired")
                                    .and_then(|v| v.as_bool())
                                    .map(|b| b == e)
                                    .unwrap_or(false);
                        }
                        if let Some(a) = has_a {
                            let cnt = node
                                .get("english_name")
                                .and_then(|v| v.as_str())
                                .map(|s| s.matches('A').count())
                                .unwrap_or(0);
                            ok = ok && ((a && cnt > 0) || (!a && cnt == 0));
                        }
                        if ok && let Some(ins) = node.get("instrument_id").and_then(|v| v.as_str())
                        {
                            options.push(ins.to_string());
                        }
                    }
                }
            }
        }
    }
    options
}

#[derive(Debug, Clone)]
pub(crate) struct OptionNode {
    pub(crate) instrument_id: String,
    pub(crate) english_name: String,
    pub(crate) call_or_put: String,
    pub(crate) strike_price: f64,
    pub(crate) expired: bool,
    pub(crate) last_exercise_datetime: i64,
    pub(crate) exercise_year: i32,
    pub(crate) exercise_month: i32,
}

pub(crate) fn parse_option_nodes(res: &Value) -> Vec<OptionNode> {
    let mut options = Vec::new();
    if let Some(result_obj) = res.get("result")
        && let Some(arr) = result_obj
            .get("multi_symbol_info")
            .and_then(|v| v.as_array())
    {
        for item in arr {
            if let Some(der) = item.get("derivatives")
                && let Some(edges) = der.get("edges").and_then(|v| v.as_array())
            {
                for edge in edges {
                    let Some(node) = edge.get("node").and_then(|v| v.as_object()) else {
                        continue;
                    };
                    let Some(instrument_id) = node.get("instrument_id").and_then(|v| v.as_str())
                    else {
                        continue;
                    };
                    let Some(call_or_put) = node.get("call_or_put").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Some(strike_price) = node.get("strike_price").and_then(|v| v.as_f64())
                    else {
                        continue;
                    };
                    let expired = node
                        .get("expired")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let Some(ts) = node.get("last_exercise_datetime").and_then(|v| v.as_i64())
                    else {
                        continue;
                    };
                    let english_name = node
                        .get("english_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let dt = chrono::DateTime::<Utc>::from_timestamp(
                        ts / 1_000_000_000,
                        (ts % 1_000_000_000) as u32,
                    )
                    .unwrap_or_else(|| chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap());
                    options.push(OptionNode {
                        instrument_id: instrument_id.to_string(),
                        english_name,
                        call_or_put: call_or_put.to_string(),
                        strike_price,
                        expired,
                        last_exercise_datetime: ts,
                        exercise_year: dt.year(),
                        exercise_month: dt.month() as i32,
                    });
                }
            }
        }
    }
    options
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BisectPriority {
    Left,
    Right,
}

pub(crate) fn bisect_value_index(a: &[f64], x: f64, priority: BisectPriority) -> usize {
    let insert_index = a.partition_point(|v| *v <= x);
    if 0 < insert_index && insert_index < a.len() {
        let left_dis = x - a[insert_index - 1];
        let right_dis = a[insert_index] - x;
        if left_dis == right_dis {
            match priority {
                BisectPriority::Left => insert_index - 1,
                BisectPriority::Right => insert_index,
            }
        } else if left_dis < right_dis {
            insert_index - 1
        } else {
            insert_index
        }
    } else if insert_index == 0 {
        0
    } else {
        a.len().saturating_sub(1)
    }
}

pub(crate) fn filter_option_nodes(
    options: Vec<OptionNode>,
    option_class: Option<&str>,
    exercise_year: Option<i32>,
    exercise_month: Option<i32>,
    has_a: Option<bool>,
    nearbys: Option<&[i32]>,
) -> Vec<OptionNode> {
    let mut filtered: Vec<OptionNode> = options
        .into_iter()
        .filter(|o| {
            let mut ok = true;
            if let Some(cls) = option_class {
                ok = ok && o.call_or_put == cls;
            }
            if let Some(a) = has_a {
                let cnt = o.english_name.matches('A').count();
                ok = ok && ((a && cnt > 0) || (!a && cnt == 0));
            }
            if let Some(y) = exercise_year {
                ok = ok && o.exercise_year == y;
            }
            if let Some(m) = exercise_month {
                ok = ok && o.exercise_month == m;
            }
            ok
        })
        .collect();

    if let Some(nearbys) = nearbys {
        filtered.retain(|o| !o.expired);
        let mut uniq: Vec<i64> = filtered.iter().map(|o| o.last_exercise_datetime).collect();
        uniq.sort();
        uniq.dedup();
        let selected: HashSet<i64> = nearbys
            .iter()
            .filter_map(|idx| uniq.get(*idx as usize).copied())
            .collect();
        filtered.retain(|o| selected.contains(&o.last_exercise_datetime));
    }

    filtered
}

pub(crate) fn sort_options_and_get_atm_index(
    options: &mut [OptionNode],
    underlying_price: f64,
    option_class: &str,
) -> Result<usize> {
    if options.is_empty() {
        return Err(TqError::InvalidParameter("options 为空".to_string()));
    }
    options.sort_by(|a, b| a.last_exercise_datetime.cmp(&b.last_exercise_datetime));
    options.sort_by(|a, b| {
        a.strike_price
            .partial_cmp(&b.strike_price)
            .unwrap_or(Ordering::Equal)
    });

    let strikes: Vec<f64> = options.iter().map(|o| o.strike_price).collect();
    let priority = if option_class == "CALL" {
        BisectPriority::Right
    } else {
        BisectPriority::Left
    };
    let mid_idx = bisect_value_index(&strikes, underlying_price, priority);
    let mid_strike = strikes[mid_idx];
    let mid_instrument = options
        .iter()
        .find(|o| o.strike_price == mid_strike)
        .map(|o| o.instrument_id.clone())
        .ok_or_else(|| TqError::InternalError("未找到平值期权".to_string()))?;

    if option_class == "PUT" {
        options.sort_by(|a, b| {
            b.strike_price
                .partial_cmp(&a.strike_price)
                .unwrap_or(Ordering::Equal)
        });
    }

    options
        .iter()
        .position(|o| o.instrument_id == mid_instrument)
        .ok_or_else(|| TqError::InternalError("未找到平值期权下标".to_string()))
}

fn timestamp_nano_to_seconds(ts: &Value) -> Option<i64> {
    if let Some(v) = ts.as_i64() {
        return Some(v / 1_000_000_000);
    }
    if let Some(v) = ts.as_f64() {
        return Some((v / 1_000_000_000.0).round() as i64);
    }
    None
}

fn timestamp_nano_to_cst_datetime(ts: i64) -> Option<chrono::DateTime<FixedOffset>> {
    let offset = FixedOffset::east_opt(8 * 3600)?;
    let secs = ts / 1_000_000_000;
    let nanos = (ts % 1_000_000_000) as u32;
    offset.timestamp_opt(secs, nanos).single()
}

fn get_trading_time_part(trading_time: &Value, key: &str) -> Option<Value> {
    trading_time
        .get(key)
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.is_empty() {
                None
            } else {
                Some(Value::Array(arr.clone()))
            }
        })
}

fn update_symbol_info_map(info: &mut Map<String, Value>, symbol: &Map<String, Value>) {
    if let Some(Value::String(class)) = symbol.get("class") {
        info.insert("ins_class".to_string(), Value::String(class.clone()));
    }
    if let Some(Value::String(instrument_id)) = symbol.get("instrument_id") {
        info.insert(
            "instrument_id".to_string(),
            Value::String(instrument_id.clone()),
        );
    }
    if let Some(Value::String(instrument_name)) = symbol.get("instrument_name") {
        info.insert(
            "instrument_name".to_string(),
            Value::String(instrument_name.clone()),
        );
    }
    if let Some(Value::String(exchange_id)) = symbol.get("exchange_id") {
        info.insert(
            "exchange_id".to_string(),
            Value::String(exchange_id.clone()),
        );
    }
    if let Some(Value::String(product_id)) = symbol.get("product_id") {
        info.insert("product_id".to_string(), Value::String(product_id.clone()));
    }
    if let Some(v) = symbol.get("price_tick") {
        info.insert("price_tick".to_string(), v.clone());
    }
    if let Some(v) = symbol
        .get("index_multiple")
        .or_else(|| symbol.get("volume_multiple"))
    {
        info.insert("volume_multiple".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("max_limit_order_volume") {
        info.insert("max_limit_order_volume".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("max_market_order_volume") {
        info.insert("max_market_order_volume".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("strike_price") {
        info.insert("strike_price".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("expired") {
        info.insert("expired".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("upper_limit") {
        info.insert("upper_limit".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("lower_limit") {
        info.insert("lower_limit".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("pre_open_interest") {
        info.insert("pre_open_interest".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("pre_close") {
        info.insert("pre_close".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("settlement_price") {
        info.insert("pre_settlement".to_string(), v.clone());
    }
    if let Some(ts) = symbol
        .get("expire_datetime")
        .and_then(timestamp_nano_to_seconds)
    {
        info.insert("expire_datetime".to_string(), json!(ts));
    }
    if let Some(ts_nano) = symbol
        .get("last_exercise_datetime")
        .and_then(|v| v.as_i64())
    {
        let ts = ts_nano / 1_000_000_000;
        info.insert("last_exercise_datetime".to_string(), json!(ts));
        if let Some(dt) = timestamp_nano_to_cst_datetime(ts_nano) {
            info.insert("exercise_year".to_string(), json!(dt.year()));
            info.insert("exercise_month".to_string(), json!(dt.month() as i32));
        }
    }
    if let Some(Value::String(call_or_put)) = symbol.get("call_or_put") {
        info.insert(
            "option_class".to_string(),
            Value::String(call_or_put.clone()),
        );
    }
    if let Some(v) = symbol.get("delivery_year") {
        info.insert("delivery_year".to_string(), v.clone());
    }
    if let Some(v) = symbol.get("delivery_month") {
        info.insert("delivery_month".to_string(), v.clone());
    }
    if let Some(trading_time) = symbol.get("trading_time") {
        if let Some(day) = get_trading_time_part(trading_time, "day") {
            info.insert("trading_time_day".to_string(), day);
        }
        if let Some(night) = get_trading_time_part(trading_time, "night") {
            info.insert("trading_time_night".to_string(), night);
        }
    }
}

fn normalize_symbol_info(info: &mut Map<String, Value>) {
    let keys = [
        "ins_class",
        "instrument_id",
        "instrument_name",
        "price_tick",
        "volume_multiple",
        "max_limit_order_volume",
        "max_market_order_volume",
        "underlying_symbol",
        "strike_price",
        "exchange_id",
        "product_id",
        "expired",
        "expire_datetime",
        "expire_rest_days",
        "delivery_year",
        "delivery_month",
        "last_exercise_datetime",
        "exercise_year",
        "exercise_month",
        "option_class",
        "upper_limit",
        "lower_limit",
        "pre_settlement",
        "pre_open_interest",
        "pre_close",
        "trading_time_day",
        "trading_time_night",
    ];
    for key in keys {
        if !info.contains_key(key) {
            info.insert(key.to_string(), Value::Null);
        }
    }
}

pub(crate) fn parse_query_symbol_info_result(
    res: &Value,
    symbol_list: &[String],
    current_ts: i64,
) -> Vec<Value> {
    let mut quotes: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut combine_leg1: HashMap<String, String> = HashMap::new();

    if let Some(result_obj) = res.get("result")
        && let Some(arr) = result_obj
            .get("multi_symbol_info")
            .and_then(|v| v.as_array())
    {
        for item in arr {
            let symbol = match item.as_object() {
                Some(obj) => obj,
                None => continue,
            };
            let instrument_id = match symbol.get("instrument_id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let mut underlying_nodes: Vec<(String, Map<String, Value>)> = Vec::new();
            let mut entry = quotes.remove(&instrument_id).unwrap_or_default();
            update_symbol_info_map(&mut entry, symbol);

            if let Some(leg1) = symbol
                .get("leg1")
                .and_then(|v| v.get("instrument_id"))
                .and_then(|v| v.as_str())
            {
                combine_leg1.insert(instrument_id.clone(), leg1.to_string());
            }

            if let Some(underlying) = symbol.get("underlying")
                && let Some(edges) = underlying.get("edges").and_then(|v| v.as_array())
            {
                for edge in edges {
                    if let Some(node) = edge.get("node").and_then(|v| v.as_object())
                        && let Some(under_id) = node.get("instrument_id").and_then(|v| v.as_str())
                    {
                        entry.insert(
                            "underlying_symbol".to_string(),
                            Value::String(under_id.to_string()),
                        );
                        underlying_nodes.push((under_id.to_string(), node.clone()));
                    }
                }
            }

            quotes.insert(instrument_id.clone(), entry);

            for (under_id, node) in underlying_nodes {
                let mut underlying_entry = quotes.remove(&under_id).unwrap_or_default();
                update_symbol_info_map(&mut underlying_entry, &node);
                quotes.insert(under_id, underlying_entry);
            }
        }
    }

    let mut leg1_volume_map: HashMap<String, Value> = HashMap::new();
    for (symbol, info) in &quotes {
        if let Some(v) = info.get("volume_multiple") {
            leg1_volume_map.insert(symbol.clone(), v.clone());
        }
    }

    for (symbol, leg1_symbol) in combine_leg1 {
        let leg1_volume = leg1_volume_map.get(&leg1_symbol).cloned();
        if let Some(combine) = quotes.get_mut(&symbol) {
            let volume_missing = combine
                .get("volume_multiple")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                == 0;
            if volume_missing && let Some(v) = leg1_volume {
                combine.insert("volume_multiple".to_string(), v);
            }
        }
    }

    let mut underlying_delivery: HashMap<String, (Value, Value)> = HashMap::new();
    for (symbol, info) in &quotes {
        if let (Some(year), Some(month)) = (info.get("delivery_year"), info.get("delivery_month")) {
            underlying_delivery.insert(symbol.clone(), (year.clone(), month.clone()));
        }
    }

    for info in quotes.values_mut() {
        let ins_class = info
            .get("ins_class")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let exchange_id = info
            .get("exchange_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let underlying_symbol = info
            .get("underlying_symbol")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let last_exercise_ts = info.get("last_exercise_datetime").and_then(|v| v.as_i64());
        let expire_ts = info.get("expire_datetime").and_then(|v| v.as_i64());

        if let Some(ins_class) = ins_class
            && ins_class == "OPTION"
            && let Some(exchange_id) = exchange_id
        {
            if matches!(exchange_id.as_str(), "DCE" | "CZCE" | "SHFE" | "GFEX")
                && let Some(underlying_symbol) = &underlying_symbol
                && let Some((year, month)) = underlying_delivery.get(underlying_symbol)
            {
                info.insert("delivery_year".to_string(), year.clone());
                info.insert("delivery_month".to_string(), month.clone());
            }
            if exchange_id == "CFFEX"
                && let Some(ts_i64) = last_exercise_ts
                && let Some(dt) = timestamp_nano_to_cst_datetime(ts_i64 * 1_000_000_000)
            {
                info.insert("delivery_year".to_string(), json!(dt.year()));
                info.insert("delivery_month".to_string(), json!(dt.month() as i32));
            }
        }

        if let Some(expire_ts) = expire_ts {
            let offset = FixedOffset::east_opt(8 * 3600).unwrap();
            if let (Some(expire_dt), Some(current_dt)) = (
                offset.timestamp_opt(expire_ts, 0).single(),
                offset.timestamp_opt(current_ts, 0).single(),
            ) {
                let expire_date = expire_dt.date_naive();
                let current_date = current_dt.date_naive();
                let days = (expire_date - current_date).num_days();
                info.insert("expire_rest_days".to_string(), json!(days));
            }
        }
        normalize_symbol_info(info);
    }

    let mut list = Vec::new();
    for symbol in symbol_list {
        if let Some(info) = quotes.get(symbol) {
            list.push(Value::Object(info.clone()));
        } else {
            let mut info = Map::new();
            info.insert("instrument_id".to_string(), Value::String(symbol.clone()));
            normalize_symbol_info(&mut info);
            list.push(Value::Object(info));
        }
    }
    list
}
