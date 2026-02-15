use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::auth::Authenticator;
use crate::types::{
    EdbIndexData, SymbolRanking, SymbolSettlement, TradingCalendarDay, TradingStatus,
};
use crate::utils::{fetch_json_with_headers, split_symbol, value_to_f64};
use crate::websocket::{TqQuoteWebsocket, TqTradingStatusWebsocket};
use async_channel::{unbounded, Receiver};
use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone, Utc};
use chrono::Duration as ChronoDuration;
use reqwest::Client;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tokio::sync::RwLock;
use uuid::Uuid;

pub struct InsAPI {
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    trading_status_ws: Option<Arc<TqTradingStatusWebsocket>>,
    auth: Arc<RwLock<dyn Authenticator>>,
    trading_status_symbols: Arc<RwLock<HashSet<String>>>,
    stock: bool,
}

fn validate_query_variables(variables: &Value) -> Result<()> {
    let obj = variables
        .as_object()
        .ok_or_else(|| TqError::InvalidParameter("variables 必须是对象".to_string()))?;
    for value in obj.values() {
        match value {
            Value::String(s) => {
                if s.is_empty() {
                    return Err(TqError::InvalidParameter(
                        "variables 不支持空字符串".to_string(),
                    ));
                }
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return Err(TqError::InvalidParameter(
                        "variables 不支持空列表".to_string(),
                    ));
                }
                for item in items {
                    if let Value::String(s) = item {
                        if s.is_empty() {
                            return Err(TqError::InvalidParameter(
                                "variables 不支持列表中包含空字符串".to_string(),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn match_query_cache(symbol: &Map<String, Value>, query: &Value, variables: &Value) -> bool {
    match (symbol.get("query"), symbol.get("variables")) {
        (Some(q), Some(v)) => q == query && v == variables,
        _ => false,
    }
}

fn parse_query_quotes_result(res: &Value, target_ex: Option<&str>) -> Vec<String> {
    let mut list = Vec::new();
    if let Some(result_obj) = res.get("result") {
        if let Some(arr) = result_obj.get("multi_symbol_info").and_then(|v| v.as_array()) {
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
    }
    list
}

fn parse_query_cont_quotes_result(
    res: &Value,
    exchange_id: Option<&str>,
    product_id: Option<&str>,
) -> Vec<String> {
    let mut list = Vec::new();
    if let Some(result_obj) = res.get("result") {
        if let Some(arr) = result_obj.get("multi_symbol_info").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(underlying) = item.get("underlying") {
                    if let Some(edges) = underlying.get("edges").and_then(|v| v.as_array()) {
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
                                if ex_ok && prod_ok {
                                    if let Some(ins) =
                                        node.get("instrument_id").and_then(|v| v.as_str())
                                    {
                                        list.push(ins.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    list
}

fn parse_query_options_result(
    res: &Value,
    option_class: Option<&str>,
    exercise_year: Option<i32>,
    exercise_month: Option<i32>,
    strike_price: Option<f64>,
    expired: Option<bool>,
    has_a: Option<bool>,
) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(result_obj) = res.get("result") {
        if let Some(arr) = result_obj.get("multi_symbol_info").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(der) = item.get("derivatives") {
                    if let Some(edges) = der.get("edges").and_then(|v| v.as_array()) {
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
                                if ok {
                                    if let Some(ins) =
                                        node.get("instrument_id").and_then(|v| v.as_str())
                                    {
                                        options.push(ins.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    options
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
        info.insert("exchange_id".to_string(), Value::String(exchange_id.clone()));
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
    if let Some(ts) = symbol.get("expire_datetime").and_then(timestamp_nano_to_seconds) {
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
        info.insert("option_class".to_string(), Value::String(call_or_put.clone()));
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

fn parse_query_symbol_info_result(
    res: &Value,
    symbol_list: &[String],
    current_ts: i64,
) -> Vec<Value> {
    let mut quotes: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut combine_leg1: HashMap<String, String> = HashMap::new();

    if let Some(result_obj) = res.get("result") {
        if let Some(arr) = result_obj.get("multi_symbol_info").and_then(|v| v.as_array()) {
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

                if let Some(underlying) = symbol.get("underlying") {
                    if let Some(edges) = underlying.get("edges").and_then(|v| v.as_array()) {
                        for edge in edges {
                            if let Some(node) = edge.get("node").and_then(|v| v.as_object()) {
                                if let Some(under_id) =
                                    node.get("instrument_id").and_then(|v| v.as_str())
                                {
                                    entry.insert(
                                        "underlying_symbol".to_string(),
                                        Value::String(under_id.to_string()),
                                    );
                                    underlying_nodes.push((under_id.to_string(), node.clone()));
                                }
                            }
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
            if volume_missing {
                if let Some(v) = leg1_volume {
                    combine.insert("volume_multiple".to_string(), v);
                }
            }
        }
    }

    let mut underlying_delivery: HashMap<String, (Value, Value)> = HashMap::new();
    for (symbol, info) in &quotes {
        if let (Some(year), Some(month)) = (info.get("delivery_year"), info.get("delivery_month"))
        {
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
        let last_exercise_ts = info
            .get("last_exercise_datetime")
            .and_then(|v| v.as_i64());
        let expire_ts = info.get("expire_datetime").and_then(|v| v.as_i64());

        if let Some(ins_class) = ins_class {
            if ins_class == "OPTION" {
                if let Some(exchange_id) = exchange_id {
                    if matches!(exchange_id.as_str(), "DCE" | "CZCE" | "SHFE" | "GFEX") {
                        if let Some(underlying_symbol) = &underlying_symbol {
                            if let Some((year, month)) =
                                underlying_delivery.get(underlying_symbol)
                            {
                                info.insert("delivery_year".to_string(), year.clone());
                                info.insert("delivery_month".to_string(), month.clone());
                            }
                        }
                    }
                    if exchange_id == "CFFEX" {
                        if let Some(ts_i64) = last_exercise_ts {
                            if let Some(dt) =
                                timestamp_nano_to_cst_datetime(ts_i64 * 1_000_000_000)
                            {
                                info.insert("delivery_year".to_string(), json!(dt.year()));
                                info.insert("delivery_month".to_string(), json!(dt.month() as i32));
                            }
                        }
                    }
                }
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

impl InsAPI {
    pub fn new(
        dm: Arc<DataManager>,
        ws: Arc<TqQuoteWebsocket>,
        trading_status_ws: Option<Arc<TqTradingStatusWebsocket>>,
        auth: Arc<RwLock<dyn Authenticator>>,
        stock: bool,
    ) -> Self {
        InsAPI {
            dm,
            ws,
            trading_status_ws,
            auth,
            trading_status_symbols: Arc::new(RwLock::new(HashSet::new())),
            stock,
        }
    }

    async fn send_ins_query(
        &self,
        query: String,
        variables: Option<Value>,
        query_id: Option<String>,
        timeout_secs: u64,
    ) -> Result<Value> {
        let id = query_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        if let Some(vars) = variables.as_ref() {
            validate_query_variables(vars)?;
        }

        let query_value = json!(query);
        let variables_value = variables.clone().unwrap_or_else(|| Value::Object(Map::new()));

        if let Some(Value::Object(symbols)) = self.dm.get_by_path(&["symbols"]) {
            for symbol in symbols.values() {
                if let Value::Object(symbol_obj) = symbol {
                    if match_query_cache(symbol_obj, &query_value, &variables_value) {
                        return Ok(Value::Object(symbol_obj.clone()));
                    }
                }
            }
        }

        let mut req = Map::new();
        req.insert("aid".to_string(), json!("ins_query"));
        req.insert("query_id".to_string(), json!(id));
        req.insert("query".to_string(), query_value);
        if !matches!(variables_value, Value::Object(ref m) if m.is_empty()) {
            req.insert("variables".to_string(), variables_value);
        }

        self.ws.send(&Value::Object(req)).await?;
        self.ws.send(&json!({"aid": "peek_message"})).await?;

        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let rx = self
            .dm
            .watch(vec!["symbols".to_string(), id.clone()]);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            if let Some(val) = self.dm.get_by_path(&["symbols", &id]) {
                return Ok(val);
            }
            if start.elapsed() >= timeout {
                return Err(TqError::Timeout);
            }
            tokio::select! {
                _ = rx.recv() => {}
                _ = interval.tick() => {
                    let _ = self.ws.send(&json!({"aid": "peek_message"})).await;
                }
            }
        }
    }

    pub async fn query_graphql(
        &self,
        query: &str,
        variables: Option<Value>,
    ) -> Result<Value> {
        let vars = variables.unwrap_or_else(|| Value::Object(Map::new()));
        self.send_ins_query(query.to_string(), Some(vars), None, 60)
            .await
    }

    pub async fn query_quotes(
        &self,
        ins_class: Option<&str>,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        expired: Option<bool>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        let mut vars = Map::new();
        if let Some(cls) = ins_class {
            if cls.is_empty() {
                return Err(TqError::InvalidParameter("ins_class 不能为空".to_string()));
            }
            vars.insert("class_".to_string(), json!([cls]));
        }
        if let Some(ex) = exchange_id {
            if ex.is_empty() {
                return Err(TqError::InvalidParameter("exchange_id 不能为空".to_string()));
            }
            let is_future_ex = matches!(ex, "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX");
            let need_pass_ex = match ins_class {
                Some(c) => !matches!(c, "INDEX" | "CONT") || !is_future_ex,
                None => true,
            };
            if need_pass_ex {
                vars.insert("exchange_id".to_string(), json!([ex]));
            }
        }
        if let Some(pid) = product_id {
            if pid.is_empty() {
                return Err(TqError::InvalidParameter("product_id 不能为空".to_string()));
            }
            vars.insert("product_id".to_string(), json!([pid]));
        }
        if let Some(e) = expired {
            vars.insert("expired".to_string(), json!(e));
        }
        if let Some(n) = has_night {
            vars.insert("has_night".to_string(), json!(n));
        }

        let query = r#"query($class_:[Class], $exchange_id:[String], $product_id:[String], $expired:Boolean, $has_night:Boolean){
  multi_symbol_info(class: $class_, exchange_id: $exchange_id, product_id: $product_id, expired: $expired, has_night: $has_night) {
    ... on basic { instrument_id }
  }
}"#;

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(vars)), None, 60)
            .await?;

        let mut target_ex: Option<String> = None;
        if let Some(ex) = exchange_id {
            if matches!(ins_class, Some("INDEX") | Some("CONT")) && matches!(ex, "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX") {
                target_ex = Some(ex.to_string());
            }
        }
        Ok(parse_query_quotes_result(&res, target_ex.as_deref()))
    }

    pub async fn query_cont_quotes(
        &self,
        exchange_id: Option<&str>,
        product_id: Option<&str>,
        has_night: Option<bool>,
    ) -> Result<Vec<String>> {
        if !self.stock {
            return Err(TqError::InvalidParameter(
                "期货行情系统(_stock = False)不支持当前接口调用".to_string(),
            ));
        }
        let (query, vars) = if let Some(night) = has_night {
            let query = r#"query($class_:[Class], $has_night:Boolean){
  multi_symbol_info(class: $class_, has_night: $has_night) {
    ... on derivative {
      underlying {
        edges {
          node {
            ... on basic { instrument_id exchange_id }
            ... on future { product_id }
          }
        }
      }
    }
  }
}"#;
            let mut vars = Map::new();
            vars.insert("class_".to_string(), json!(["CONT"]));
            vars.insert("has_night".to_string(), json!(night));
            (query, vars)
        } else {
            let query = r#"query($class_:[Class]){
  multi_symbol_info(class: $class_) {
    ... on derivative {
      underlying {
        edges {
          node {
            ... on basic { instrument_id exchange_id }
            ... on future { product_id }
          }
        }
      }
    }
  }
}"#;
            let mut vars = Map::new();
            vars.insert("class_".to_string(), json!(["CONT"]));
            (query, vars)
        };

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(vars)), None, 60)
            .await?;

        Ok(parse_query_cont_quotes_result(&res, exchange_id, product_id))
    }

    pub async fn query_options(
        &self,
        underlying_symbol: &str,
        option_class: Option<&str>,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        strike_price: Option<f64>,
        expired: Option<bool>,
        has_a: Option<bool>,
    ) -> Result<Vec<String>> {
        if underlying_symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "underlying_symbol 不能为空".to_string(),
            ));
        }

        let query = r#"query($instrument_id:[String], $derivative_class:[Class]){
  multi_symbol_info(instrument_id: $instrument_id) {
    ... on basic {
      instrument_id
      derivatives(class: $derivative_class) {
        edges {
          node {
            ... on basic {
              class_
              instrument_id
              exchange_id
              english_name
            }
            ... on option {
              expired
              expire_datetime
              last_exercise_datetime
              strike_price
              call_or_put
            }
          }
        }
      }
    }
  }
}"#;

        let mut all_vars = Map::new();
        all_vars.insert("instrument_id".to_string(), Value::Array(vec![json!(underlying_symbol)]));
        all_vars.insert("derivative_class".to_string(), json!(["OPTION"]));

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(all_vars)), None, 60)
            .await?;

        Ok(parse_query_options_result(
            &res,
            option_class,
            exercise_year,
            exercise_month,
            strike_price,
            expired,
            has_a,
        ))
    }

    pub async fn query_symbol_info(&self, symbols: &[&str]) -> Result<Vec<Value>> {
        if symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空列表".to_string()));
        }
        if symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter(
                "symbol 参数中不能有空字符串".to_string(),
            ));
        }
        let symbol_list: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        let symbol_json = serde_json::to_string(&symbol_list)
            .map_err(|e| TqError::InternalError(format!("symbol 序列化失败: {}", e)))?;
        let mut query = r#"query{ multi_symbol_info(instrument_id: __SYMBOLS__){
    ... on basic {
      instrument_id
      exchange_id
      instrument_name
      english_name
      class
      price_tick
      price_decs
      trading_day
      trading_time {
        day
        night
      }
    }
    ... on tradeable {
      pre_close
      volume_multiple
      quote_multiple
      upper_limit
      lower_limit
    }
    ... on index {
      index_multiple
    }
    ... on future {
      pre_open_interest
      expired
      product_id
      product_short_name
      delivery_year
      delivery_month
      expire_datetime
      settlement_price
      max_market_order_volume
      max_limit_order_volume
      min_market_order_volume
      min_limit_order_volume
      open_max_market_order_volume
      open_max_limit_order_volume
      open_min_market_order_volume
      open_min_limit_order_volume
    }
    ... on option {
      pre_open_interest
      expired
      product_short_name
      expire_datetime
      last_exercise_datetime
      settlement_price
      max_market_order_volume
      max_limit_order_volume
      min_market_order_volume
      min_limit_order_volume
      open_max_market_order_volume
      open_max_limit_order_volume
      open_min_market_order_volume
      open_min_limit_order_volume
      strike_price
      call_or_put
      exercise_type
    }
    ... on combine {
      expired
      product_id
      expire_datetime
      max_market_order_volume
      max_limit_order_volume
      min_market_order_volume
      min_limit_order_volume
      open_max_market_order_volume
      open_max_limit_order_volume
      open_min_market_order_volume
      open_min_limit_order_volume
      leg1 { ... on basic { instrument_id } }
      leg2 { ... on basic { instrument_id } }
    }
    ... on derivative {
      underlying {
        edges {
          node {
            ... on basic {
              instrument_id
              exchange_id
              instrument_name
              english_name
              class
              price_tick
              price_decs
              trading_day
              trading_time { day night }
            }
            ... on tradeable {
              pre_close
              volume_multiple
              quote_multiple
              upper_limit
              lower_limit
            }
            ... on index {
              index_multiple
            }
            ... on future {
              pre_open_interest
              expired
              product_id
              product_short_name
              delivery_year
              delivery_month
              expire_datetime
              settlement_price
              max_market_order_volume
              max_limit_order_volume
              min_market_order_volume
              min_limit_order_volume
              open_max_market_order_volume
              open_max_limit_order_volume
              open_min_market_order_volume
              open_min_limit_order_volume
            }
          }
        }
      }
    }
  }
}"#
            .to_string();
        query = query.replace("__SYMBOLS__", &symbol_json);
        let res = self.send_ins_query(query, None, None, 60).await?;
        Ok(parse_query_symbol_info_result(
            &res,
            &symbol_list,
            Utc::now().timestamp(),
        ))
    }

    pub async fn query_symbol_settlement(
        &self,
        symbols: &[&str],
        days: i32,
        start_dt: Option<NaiveDate>,
    ) -> Result<Vec<SymbolSettlement>> {
        if symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空列表".to_string()));
        }
        if symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter(
                "symbol 参数中不能有空字符串".to_string(),
            ));
        }
        if days < 1 {
            return Err(TqError::InvalidParameter(format!("days 参数 {} 错误。", days)));
        }
        let mut query_days = days;
        let mut url = url::Url::parse("https://md-settlement-system-fc-api.shinnytech.com/mss")
            .map_err(|e| TqError::InternalError(format!("URL 解析失败: {}", e)))?;
        let symbols_str = symbols.join(",");
        let mut params = vec![("symbols", symbols_str), ("days", query_days.to_string())];
        if let Some(dt) = start_dt {
            params.push(("start_date", dt.format("%Y%m%d").to_string()));
        } else {
            query_days += 1;
            params[1].1 = query_days.to_string();
        }
        url.query_pairs_mut().extend_pairs(params);

        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(url.as_str(), headers).await?;
        let obj = content.as_object().ok_or_else(|| {
            TqError::ParseError("结算价数据格式错误，应为对象".to_string())
        })?;

        let mut dates: Vec<String> = obj.keys().cloned().collect();
        dates.sort_by(|a, b| b.cmp(a));
        let mut rows: Vec<SymbolSettlement> = Vec::new();
        for date in dates.into_iter().take(days as usize) {
            if let Some(symbols_map) = obj.get(&date).and_then(|v| v.as_object()) {
                for (symbol, settlement_val) in symbols_map.iter() {
                    if settlement_val.is_null() {
                        continue;
                    }
                    let settlement = value_to_f64(settlement_val);
                    rows.push(SymbolSettlement {
                        datetime: date.clone(),
                        symbol: symbol.clone(),
                        settlement,
                    });
                }
            }
        }

        rows.sort_by(|a, b| {
            let dt_cmp = a.datetime.cmp(&b.datetime);
            if dt_cmp == std::cmp::Ordering::Equal {
                a.symbol.cmp(&b.symbol)
            } else {
                dt_cmp
            }
        });
        Ok(rows)
    }

    pub async fn query_symbol_ranking(
        &self,
        symbol: &str,
        ranking_type: &str,
        days: i32,
        start_dt: Option<NaiveDate>,
        broker: Option<&str>,
    ) -> Result<Vec<SymbolRanking>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "symbol 参数应该填入有效的合约代码。".to_string(),
            ));
        }
        if !matches!(ranking_type, "VOLUME" | "LONG" | "SHORT") {
            return Err(TqError::InvalidParameter(
                "ranking_type 参数只支持以下值： 'VOLUME', 'LONG', 'SHORT'。".to_string(),
            ));
        }
        if days < 1 {
            return Err(TqError::InvalidParameter(format!("days 参数 {} 错误。", days)));
        }
        let mut url = url::Url::parse("https://symbol-ranking-system-fc-api.shinnytech.com/srs")
            .map_err(|e| TqError::InternalError(format!("URL 解析失败: {}", e)))?;
        let mut params = vec![
            ("symbol", symbol.to_string()),
            ("days", days.to_string()),
        ];
        if let Some(dt) = start_dt {
            params.push(("start_date", dt.format("%Y%m%d").to_string()));
        }
        if let Some(broker) = broker {
            if broker.is_empty() {
                return Err(TqError::InvalidParameter("broker 不能为空字符串".to_string()));
            }
            params.push(("broker", broker.to_string()));
        }
        url.query_pairs_mut().extend_pairs(params);

        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(url.as_str(), headers).await?;
        let obj = content.as_object().ok_or_else(|| {
            TqError::ParseError("持仓排名数据格式错误，应为对象".to_string())
        })?;

        let mut rows: HashMap<String, SymbolRanking> = HashMap::new();
        for (dt, symbols_val) in obj.iter() {
            let symbols_map = match symbols_val.as_object() {
                Some(map) => map,
                None => continue,
            };
            for (symbol_key, data_val) in symbols_map.iter() {
                if data_val.is_null() {
                    continue;
                }
                let data_map = match data_val.as_object() {
                    Some(map) => map,
                    None => continue,
                };
                for (data_type, broker_val) in data_map.iter() {
                    let broker_map = match broker_val.as_object() {
                        Some(map) => map,
                        None => continue,
                    };
                    for (broker_name, rank_val) in broker_map.iter() {
                        let rank_map = match rank_val.as_object() {
                            Some(map) => map,
                            None => continue,
                        };
                        let key = format!("{}|{}|{}", dt, symbol_key, broker_name);
                        let entry = rows.entry(key).or_insert_with(|| {
                            let (exchange_id, instrument_id) = split_symbol(symbol_key);
                            SymbolRanking {
                                datetime: dt.clone(),
                                symbol: symbol_key.clone(),
                                exchange_id: exchange_id.to_string(),
                                instrument_id: instrument_id.to_string(),
                                broker: broker_name.clone(),
                                volume: f64::NAN,
                                volume_change: f64::NAN,
                                volume_ranking: f64::NAN,
                                long_oi: f64::NAN,
                                long_change: f64::NAN,
                                long_ranking: f64::NAN,
                                short_oi: f64::NAN,
                                short_change: f64::NAN,
                                short_ranking: f64::NAN,
                            }
                        });

                        let volume = rank_map.get("volume").map(value_to_f64).unwrap_or(f64::NAN);
                        let varvolume =
                            rank_map.get("varvolume").map(value_to_f64).unwrap_or(f64::NAN);
                        let ranking =
                            rank_map.get("ranking").map(value_to_f64).unwrap_or(f64::NAN);

                        match data_type.as_str() {
                            "volume_ranking" => {
                                entry.volume = volume;
                                entry.volume_change = varvolume;
                                entry.volume_ranking = ranking;
                            }
                            "long_ranking" => {
                                entry.long_oi = volume;
                                entry.long_change = varvolume;
                                entry.long_ranking = ranking;
                            }
                            "short_ranking" => {
                                entry.short_oi = volume;
                                entry.short_change = varvolume;
                                entry.short_ranking = ranking;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let rank_field = match ranking_type {
            "VOLUME" => "volume_ranking",
            "LONG" => "long_ranking",
            _ => "short_ranking",
        };
        let mut list: Vec<SymbolRanking> = rows.into_values().collect();
        list.retain(|item| {
            let value = match rank_field {
                "volume_ranking" => item.volume_ranking,
                "long_ranking" => item.long_ranking,
                _ => item.short_ranking,
            };
            !value.is_nan()
        });
        list.sort_by(|a, b| {
            let dt_cmp = a.datetime.cmp(&b.datetime);
            if dt_cmp == std::cmp::Ordering::Equal {
                let a_rank = match rank_field {
                    "volume_ranking" => a.volume_ranking,
                    "long_ranking" => a.long_ranking,
                    _ => a.short_ranking,
                };
                let b_rank = match rank_field {
                    "volume_ranking" => b.volume_ranking,
                    "long_ranking" => b.long_ranking,
                    _ => b.short_ranking,
                };
                a_rank
                    .partial_cmp(&b_rank)
                    .unwrap_or(std::cmp::Ordering::Equal)
            } else {
                dt_cmp
            }
        });
        Ok(list)
    }

    pub async fn query_edb_data(
        &self,
        ids: &[i32],
        n: i32,
        align: Option<&str>,
        fill: Option<&str>,
    ) -> Result<Vec<EdbIndexData>> {
        if ids.is_empty() {
            return Err(TqError::InvalidParameter("ids 不能为空。".to_string()));
        }
        if n < 1 {
            return Err(TqError::InvalidParameter(format!(
                "n 参数 {} 错误，应为 >= 1 的整数。",
                n
            )));
        }
        if !matches!(align, None | Some("day")) {
            return Err(TqError::InvalidParameter(format!(
                "align 参数 {:?} 错误，仅支持 None 或 'day'",
                align
            )));
        }
        if !matches!(fill, None | Some("ffill") | Some("bfill")) {
            return Err(TqError::InvalidParameter(format!(
                "fill 参数 {:?} 错误，仅支持 None/'ffill'/'bfill'",
                fill
            )));
        }

        let mut seen = HashSet::new();
        let mut norm_ids: Vec<i32> = Vec::new();
        for &id in ids {
            if seen.insert(id) {
                norm_ids.push(id);
            }
        }
        if norm_ids.len() > 100 {
            return Err(TqError::InvalidParameter(
                "ids 数量超过限制(<=100)".to_string(),
            ));
        }

        let end_date = Utc::now().date_naive();
        let start_date = end_date - ChronoDuration::days((n - 1) as i64);
        let start_s = start_date.format("%Y-%m-%d").to_string();
        let end_s = end_date.format("%Y-%m-%d").to_string();

        let headers = self.auth.read().await.base_header();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .default_headers(headers)
            .build()
            .map_err(|e| TqError::NetworkError(format!("创建 HTTP 客户端失败: {}", e)))?;

        let payload = json!({
            "ids": norm_ids,
            "start": start_s,
            "end": end_s,
        });

        let resp = client
            .post("https://edb.shinnytech.com/data/index_data")
            .json(&payload)
            .send()
            .await
            .map_err(|e| TqError::NetworkError(format!("请求失败: {}", e)))?;

        if !resp.status().is_success() {
            return Err(TqError::NetworkError(format!(
                "HTTP 状态码错误: {}",
                resp.status()
            )));
        }

        let content: Value = resp
            .json()
            .await
            .map_err(|e| TqError::ParseError(format!("JSON 解析失败: {}", e)))?;

        if content
            .get("error_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            != 0
        {
            return Err(TqError::Other(format!(
                "edb 指标取值查询失败: {}",
                content.get("error_msg").and_then(|v| v.as_str()).unwrap_or("")
            )));
        }

        let data = content.get("data").cloned().unwrap_or(Value::Null);
        let ids_from_server: Vec<i32> = data
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64())
                    .map(|v| v as i32)
                    .collect()
            })
            .unwrap_or_else(|| norm_ids.clone());
        let values_map = data
            .get("values")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut rows: HashMap<String, HashMap<i32, f64>> = HashMap::new();
        for (date, row_val) in values_map.iter() {
            let row_arr = match row_val.as_array() {
                Some(arr) => arr,
                None => continue,
            };
            let mut row_map: HashMap<i32, f64> = HashMap::new();
            for (idx, id) in ids_from_server.iter().enumerate() {
                let value = row_arr
                    .get(idx)
                    .map(value_to_f64)
                    .unwrap_or(f64::NAN);
                row_map.insert(*id, value);
            }
            rows.insert(date.clone(), row_map);
        }

        let mut dates: Vec<String> = rows.keys().cloned().collect();
        dates.sort();

        if align == Some("day") {
            let mut all_dates: Vec<String> = Vec::new();
            let mut current = start_date;
            while current <= end_date {
                all_dates.push(current.format("%Y-%m-%d").to_string());
                current += ChronoDuration::days(1);
            }
            let mut aligned: HashMap<String, HashMap<i32, f64>> = HashMap::new();
            for date in all_dates.iter() {
                let row = rows.get(date).cloned().unwrap_or_default();
                aligned.insert(date.clone(), row);
            }
            rows = aligned;
            dates = all_dates;

            if fill == Some("ffill") {
                let mut last_values: HashMap<i32, f64> = HashMap::new();
                for date in dates.iter() {
                    let row = rows.entry(date.clone()).or_default();
                    for id in norm_ids.iter() {
                        let value = row.get(id).copied().unwrap_or(f64::NAN);
                        if value.is_nan() {
                            if let Some(last) = last_values.get(id) {
                                row.insert(*id, *last);
                            }
                        } else {
                            last_values.insert(*id, value);
                        }
                    }
                }
            } else if fill == Some("bfill") {
                let mut next_values: HashMap<i32, f64> = HashMap::new();
                for date in dates.iter().rev() {
                    let row = rows.entry(date.clone()).or_default();
                    for id in norm_ids.iter() {
                        let value = row.get(id).copied().unwrap_or(f64::NAN);
                        if value.is_nan() {
                            if let Some(next) = next_values.get(id) {
                                row.insert(*id, *next);
                            }
                        } else {
                            next_values.insert(*id, value);
                        }
                    }
                }
            }
        }

        let mut result: Vec<EdbIndexData> = Vec::new();
        for date in dates {
            if let Some(row) = rows.get(&date) {
                let mut values: HashMap<i32, f64> = HashMap::new();
                for id in norm_ids.iter() {
                    let value = row.get(id).copied().unwrap_or(f64::NAN);
                    values.insert(*id, value);
                }
                result.push(EdbIndexData { date, values });
            }
        }

        Ok(result)
    }

    pub async fn get_trading_calendar(
        &self,
        start_dt: NaiveDate,
        end_dt: NaiveDate,
    ) -> Result<Vec<TradingCalendarDay>> {
        if start_dt > end_dt {
            return Err(TqError::InvalidParameter(
                "start_dt 必须小于等于 end_dt".to_string(),
            ));
        }
        let url = std::env::var("TQ_CHINESE_HOLIDAY_URL")
            .unwrap_or_else(|_| "https://files.shinnytech.com/shinny_chinese_holiday.json".to_string());
        let headers = self.auth.read().await.base_header();
        let content = fetch_json_with_headers(&url, headers).await?;
        let holidays = content
            .as_array()
            .ok_or_else(|| TqError::ParseError("节假日数据格式错误".to_string()))?;
        let mut holiday_set: HashSet<NaiveDate> = HashSet::new();
        for item in holidays.iter() {
            if let Some(date_str) = item.as_str() {
                if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    holiday_set.insert(date);
                }
            }
        }

        let mut result: Vec<TradingCalendarDay> = Vec::new();
        let mut current = start_dt;
        while current <= end_dt {
            let weekday = current.weekday().number_from_monday();
            let is_weekday = weekday <= 5;
            let trading = is_weekday && !holiday_set.contains(&current);
            result.push(TradingCalendarDay {
                date: current.format("%Y-%m-%d").to_string(),
                trading,
            });
            current += ChronoDuration::days(1);
        }
        Ok(result)
    }

    pub async fn get_trading_status(&self, symbol: &str) -> Result<Receiver<TradingStatus>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "get_trading_status 中合约代码不能为空字符串".to_string(),
            ));
        }
        if !self.auth.read().await.has_feature("tq_trading_status") {
            return Err(TqError::PermissionDenied(
                "账户不支持查看交易状态信息，需要购买后才能使用。升级网址：https://www.shinnytech.com/tqsdk-buy/".to_string(),
            ));
        }
        let ws = self.trading_status_ws.as_ref().ok_or_else(|| {
            TqError::InternalError("交易状态服务未初始化".to_string())
        })?;

        let mut subscribe_symbol = symbol.to_string();
        let mut option_mapping: HashMap<String, String> = HashMap::new();
        if let Some(quote) = self.dm.get_by_path(&["quotes", symbol]) {
            if let Some(obj) = quote.as_object() {
                if let Some(class_str) = obj.get("class").and_then(|v| v.as_str()) {
                    if class_str == "OPTION" {
                        if let Some(underlying) =
                            obj.get("underlying_symbol").and_then(|v| v.as_str())
                        {
                            if !underlying.is_empty() {
                                subscribe_symbol = underlying.to_string();
                                option_mapping.insert(symbol.to_string(), subscribe_symbol.clone());
                            }
                        }
                    }
                }
            }
        }
        if !option_mapping.is_empty() {
            ws.update_option_underlyings(option_mapping);
        }

        {
            let mut guard = self.trading_status_symbols.write().await;
            guard.insert(subscribe_symbol.clone());
            let ins_list = guard.iter().cloned().collect::<Vec<String>>().join(",");
            let req = json!({
                "aid": "subscribe_trading_status",
                "ins_list": ins_list
            });
            ws.send(&req).await?;
        }

        let (tx, rx) = unbounded();
        let dm = Arc::clone(&self.dm);
        let symbol_string = symbol.to_string();
        let watch = dm.watch(vec!["trading_status".to_string(), symbol_string.clone()]);
        tokio::spawn(async move {
            if let Some(val) = dm.get_by_path(&["trading_status", &symbol_string]) {
                if let Ok(status) = dm.convert_to_struct::<TradingStatus>(&val) {
                    let _ = tx.send(status).await;
                }
            }
            loop {
                if watch.recv().await.is_err() {
                    break;
                }
                if let Some(val) = dm.get_by_path(&["trading_status", &symbol_string]) {
                    if let Ok(status) = dm.convert_to_struct::<TradingStatus>(&val) {
                        let _ = tx.send(status).await;
                    }
                }
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Authenticator;
    use crate::datamanager::{DataManager, DataManagerConfig};
    use crate::websocket::WebSocketConfig;
    use crate::{Client, ClientConfig};
    use async_trait::async_trait;
    use chrono::NaiveDate;
    use chrono::TimeZone;
    use reqwest::header::HeaderMap;
    use serde_json::json;
    use std::future::Future;
    use std::collections::HashSet;
    use tokio::time::Duration;

    #[test]
    fn parse_quotes_filters_exchange() {
        let res = json!({
            "result": {
                "multi_symbol_info": [
                    { "instrument_id": "SHFE.cu2405" },
                    { "instrument_id": "DCE.m2405" }
                ]
            }
        });
        let all = parse_query_quotes_result(&res, None);
        assert_eq!(all, vec!["SHFE.cu2405", "DCE.m2405"]);
        let shfe = parse_query_quotes_result(&res, Some("SHFE"));
        assert_eq!(shfe, vec!["SHFE.cu2405"]);
    }

    #[test]
    fn parse_cont_quotes_filters_exchange_product() {
        let res = json!({
            "result": {
                "multi_symbol_info": [
                    {
                        "underlying": {
                            "edges": [
                                { "node": { "instrument_id": "SHFE.cu2405", "exchange_id": "SHFE", "product_id": "cu" } },
                                { "node": { "instrument_id": "DCE.m2405", "exchange_id": "DCE", "product_id": "m" } }
                            ]
                        }
                    }
                ]
            }
        });
        let only_shfe = parse_query_cont_quotes_result(&res, Some("SHFE"), None);
        assert_eq!(only_shfe, vec!["SHFE.cu2405"]);
        let only_m = parse_query_cont_quotes_result(&res, None, Some("m"));
        assert_eq!(only_m, vec!["DCE.m2405"]);
    }

    #[test]
    fn parse_options_filters_conditions() {
        let ts = Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap().timestamp() * 1_000_000_000;
        let res = json!({
            "result": {
                "multi_symbol_info": [
                    {
                        "derivatives": {
                            "edges": [
                                {
                                    "node": {
                                        "instrument_id": "SHFE.cu2405C3000",
                                        "english_name": "AAA",
                                        "call_or_put": "CALL",
                                        "strike_price": 3000.0,
                                        "expired": false,
                                        "last_exercise_datetime": ts
                                    }
                                },
                                {
                                    "node": {
                                        "instrument_id": "SHFE.cu2405P3100",
                                        "english_name": "BBB",
                                        "call_or_put": "PUT",
                                        "strike_price": 3100.0,
                                        "expired": true,
                                        "last_exercise_datetime": ts
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        });

        let calls = parse_query_options_result(
            &res,
            Some("CALL"),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(calls, vec!["SHFE.cu2405C3000"]);

        let year_month = parse_query_options_result(
            &res,
            None,
            Some(2024),
            Some(12),
            None,
            None,
            None,
        );
        assert_eq!(year_month.len(), 2);

        let strike = parse_query_options_result(
            &res,
            None,
            None,
            None,
            Some(3100.0),
            None,
            None,
        );
        assert_eq!(strike, vec!["SHFE.cu2405P3100"]);

        let has_a = parse_query_options_result(
            &res,
            None,
            None,
            None,
            None,
            None,
            Some(true),
        );
        assert_eq!(has_a, vec!["SHFE.cu2405C3000"]);
    }

    #[test]
    fn parse_symbol_info_maps_fields() {
        let expire_ts = 1_731_801_600_i64 * 1_000_000_000;
        let exercise_ts = 1_714_492_800_i64 * 1_000_000_000;
        let res = json!({
            "result": {
                "multi_symbol_info": [
                    {
                        "instrument_id": "SHFE.cu2405",
                        "class": "FUTURE",
                        "exchange_id": "SHFE",
                        "product_id": "cu",
                        "instrument_name": "沪铜",
                        "price_tick": 10.0,
                        "volume_multiple": 5,
                        "delivery_year": 2024,
                        "delivery_month": 5,
                        "expire_datetime": expire_ts,
                        "max_limit_order_volume": 100,
                        "max_market_order_volume": 50
                    },
                    {
                        "instrument_id": "SHFE.cu2405C3000",
                        "class": "OPTION",
                        "exchange_id": "SHFE",
                        "call_or_put": "CALL",
                        "strike_price": 3000.0,
                        "expired": false,
                        "last_exercise_datetime": exercise_ts,
                        "expire_datetime": expire_ts,
                        "underlying": {
                            "edges": [
                                {
                                    "node": {
                                        "instrument_id": "SHFE.cu2405",
                                        "class": "FUTURE",
                                        "exchange_id": "SHFE",
                                        "product_id": "cu",
                                        "delivery_year": 2024,
                                        "delivery_month": 5,
                                        "volume_multiple": 5
                                    }
                                }
                            ]
                        }
                    },
                    {
                        "instrument_id": "SHFE.cu2405&cu2406",
                        "class": "COMBINE",
                        "leg1": { "instrument_id": "SHFE.cu2405" }
                    }
                ]
            }
        });

        let list = parse_query_symbol_info_result(
            &res,
            &vec![
                "SHFE.cu2405C3000".to_string(),
                "SHFE.cu2405&cu2406".to_string(),
            ],
            1_731_715_200,
        );
        let option = list[0].as_object().unwrap();
        assert_eq!(option.get("ins_class").unwrap(), "OPTION");
        assert_eq!(
            option.get("underlying_symbol").unwrap(),
            "SHFE.cu2405"
        );
        assert_eq!(option.get("delivery_year").unwrap(), 2024);
        assert_eq!(option.get("delivery_month").unwrap(), 5);
        assert_eq!(option.get("exercise_year").unwrap(), 2024);
        assert_eq!(option.get("exercise_month").unwrap(), 5);

        let combine = list[1].as_object().unwrap();
        assert_eq!(combine.get("ins_class").unwrap(), "COMBINE");
        assert_eq!(combine.get("volume_multiple").unwrap(), 5);
    }

    #[tokio::test]
    #[ignore]
    async fn graphql_live_smoke() -> Result<()> {
        let user = match std::env::var("TQ_AUTH_USER") {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        let pass = match std::env::var("TQ_AUTH_PASS") {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };

        let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
        client.init_market().await?;

        let quote_sub = match run_with_timeout(client.subscribe_quote(&["SHFE.au2602"]), 30).await {
            Ok(sub) => sub,
            Err(err) => {
                println!("graphql_live_smoke subscribe_quote error: {}", err);
                client.close().await?;
                return Ok(());
            }
        };

        let quotes = match run_with_timeout(
            client.query_quotes(Some("FUTURE"), Some("SHFE"), Some("cu"), Some(false), None),
            30,
        )
        .await
        {
            Ok(list) => list,
            Err(err) => {
                println!("graphql_live_smoke query_quotes error: {}", err);
                quote_sub.close().await?;
                client.close().await?;
                return Ok(());
            }
        };
        let underlying = quotes
            .get(0)
            .cloned()
            .unwrap_or_else(|| "SHFE.cu2405".to_string());

        let _ = match run_with_timeout(
            client.query_cont_quotes(Some("SHFE"), Some("cu"), None),
            30,
        )
        .await
        {
            Ok(list) => list,
            Err(err) => {
                println!("graphql_live_smoke query_cont_quotes error: {}", err);
                quote_sub.close().await?;
                client.close().await?;
                return Ok(());
            }
        };
        let _ = match run_with_timeout(
            client.query_options(&underlying, None, None, None, None, None, None),
            30,
        )
        .await
        {
            Ok(list) => list,
            Err(err) => {
                println!("graphql_live_smoke query_options error: {}", err);
                quote_sub.close().await?;
                client.close().await?;
                return Ok(());
            }
        };

        let query = r#"query($class_:[Class]){ multi_symbol_info(class:$class_){ ... on basic { instrument_id } } }"#;
        let variables = json!({ "class_": ["FUTURE"] });
        let raw = match run_with_timeout(client.query_graphql(query, Some(variables)), 30).await {
            Ok(value) => value,
            Err(err) => {
                println!("graphql_live_smoke query_graphql error: {}", err);
                quote_sub.close().await?;
                client.close().await?;
                return Ok(());
            }
        };
        if raw.get("result").is_none() {
            println!("graphql_live_smoke result missing");
        }
        quote_sub.close().await?;
        client.close().await?;

        Ok(())
    }

    struct DummyAuth {
        features: HashSet<String>,
    }

    #[async_trait]
    impl Authenticator for DummyAuth {
        fn base_header(&self) -> HeaderMap {
            HeaderMap::new()
        }

        async fn login(&mut self) -> Result<()> {
            Ok(())
        }

        async fn get_td_url(&self, _broker_id: &str, _account_id: &str) -> Result<crate::auth::BrokerInfo> {
            Err(TqError::NotLoggedIn)
        }

        async fn get_md_url(&self, _stock: bool, _backtest: bool) -> Result<String> {
            Err(TqError::NotLoggedIn)
        }

        fn has_feature(&self, feature: &str) -> bool {
            self.features.contains(feature)
        }

        fn has_md_grants(&self, _symbols: &[&str]) -> Result<()> {
            Ok(())
        }

        fn has_td_grants(&self, _symbol: &str) -> Result<()> {
            Ok(())
        }

        fn get_auth_id(&self) -> &str {
            ""
        }

        fn get_access_token(&self) -> &str {
            ""
        }
    }

    fn build_ins_api(features: &[&str]) -> InsAPI {
        let dm = Arc::new(DataManager::new(HashMap::new(), DataManagerConfig::default()));
        let ws = Arc::new(TqQuoteWebsocket::new(
            "wss://example.com".to_string(),
            Arc::clone(&dm),
            WebSocketConfig::default(),
        ));
        let mut feature_set = HashSet::new();
        for item in features {
            feature_set.insert(item.to_string());
        }
        let auth = Arc::new(RwLock::new(DummyAuth { features: feature_set }));
        InsAPI::new(dm, ws, None, auth, true)
    }

    #[tokio::test]
    async fn settlement_rejects_invalid_args() {
        let api = build_ins_api(&[]);
        let err = api.query_symbol_settlement(&[], 1, None).await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api
            .query_symbol_settlement(&["SHFE.cu2405"], 0, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn ranking_rejects_invalid_args() {
        let api = build_ins_api(&[]);
        let err = api
            .query_symbol_ranking("", "VOLUME", 1, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api
            .query_symbol_ranking("SHFE.cu2405", "INVALID", 1, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api
            .query_symbol_ranking("SHFE.cu2405", "VOLUME", 1, None, Some(""))
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn edb_rejects_invalid_args() {
        let api = build_ins_api(&[]);
        let err = api.query_edb_data(&[], 1, None, None).await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api.query_edb_data(&[1], 0, None, None).await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api
            .query_edb_data(&[1], 1, Some("week"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let err = api
            .query_edb_data(&[1], 1, None, Some("pad"))
            .await
            .unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
        let ids: Vec<i32> = (1..=101).collect();
        let err = api.query_edb_data(&ids, 1, None, None).await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn trading_calendar_rejects_invalid_range() {
        let api = build_ins_api(&[]);
        let start = NaiveDate::from_ymd_opt(2024, 12, 2).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
        let err = api.get_trading_calendar(start, end).await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn trading_status_rejects_without_permission() {
        let api = build_ins_api(&[]);
        let err = api.get_trading_status("SHFE.cu2405").await.unwrap_err();
        assert!(matches!(err, TqError::PermissionDenied(_)));
        let err = api.get_trading_status("").await.unwrap_err();
        assert!(matches!(err, TqError::InvalidParameter(_)));
    }

    #[tokio::test]
    #[ignore]
    async fn live_high_priority_interfaces() -> Result<()> {
        let auth_user = std::env::var("TQ_AUTH_USER").ok();
        let auth_pass = std::env::var("TQ_AUTH_PASS").ok();
        let symbol = std::env::var("TQ_TEST_SYMBOL").unwrap_or_else(|_| "SHFE.cu2405".to_string());

        let api = build_ins_api(&[]);
        match api.query_symbol_settlement(&[&symbol], 1, None).await {
            Ok(settlement) => println!("query_symbol_settlement: {:?}", settlement),
            Err(err) => println!("query_symbol_settlement error: {}", err),
        }

        match api
            .query_symbol_ranking(&symbol, "VOLUME", 1, None, None)
            .await
        {
            Ok(ranking) => println!("query_symbol_ranking: {:?}", ranking),
            Err(err) => println!("query_symbol_ranking error: {}", err),
        }

        match api.query_edb_data(&[1, 2], 5, Some("day"), Some("ffill")).await {
            Ok(edb) => println!("query_edb_data: {:?}", edb),
            Err(err) => println!("query_edb_data error: {}", err),
        }

        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();
        match api.get_trading_calendar(start, end).await {
            Ok(calendar) => println!("get_trading_calendar: {:?}", calendar),
            Err(err) => println!("get_trading_calendar error: {}", err),
        }

        if let (Some(user), Some(pass)) = (auth_user, auth_pass) {
            let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
            client.init_market().await?;
            match client.get_trading_status(&symbol).await {
                Ok(rx) => {
                    let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
                    match recv {
                        Ok(Ok(status)) => println!("get_trading_status: {:?}", status),
                        Ok(Err(err)) => println!("get_trading_status recv error: {}", err),
                        Err(_) => println!("get_trading_status timeout"),
                    }
                }
                Err(err) => println!("get_trading_status error: {}", err),
            }
            client.close().await?;
        } else {
            println!("get_trading_status skipped: missing TQ_AUTH_USER/TQ_AUTH_PASS");
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn live_query_cont_and_symbol_info() -> Result<()> {
        let user = match std::env::var("TQ_AUTH_USER") {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        let pass = match std::env::var("TQ_AUTH_PASS") {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };

        let mut client = Client::new(&user, &pass, ClientConfig::default()).await?;
        client.init_market().await?;

        match run_with_timeout(
            client.query_cont_quotes(Some("GFEX"), Some("lc"), None),
            30,
        )
        .await
        {
            Ok(list) => println!("query_cont_quotes: {:?}", list),
            Err(err) => println!("query_cont_quotes error: {}", err),
        }

        match run_with_timeout(client.query_symbol_info(&["GFEX.lc2605"]), 30).await {
            Ok(info) => println!("query_symbol_info: {:?}", info),
            Err(err) => println!("query_symbol_info error: {}", err),
        }

        client.close().await?;
        Ok(())
    }

    async fn run_with_timeout<T, F>(future: F, secs: u64) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        match tokio::time::timeout(Duration::from_secs(secs), future).await {
            Ok(result) => result,
            Err(_) => Err(TqError::Timeout),
        }
    }
}
