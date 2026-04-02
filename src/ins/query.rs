use super::InsAPI;
use super::parse::{
    filter_option_nodes, parse_option_nodes, parse_query_cont_quotes_result, parse_query_options_result,
    parse_query_quotes_result, parse_query_symbol_info_result, sort_options_and_get_atm_index,
};
use super::validation::{
    validate_finance_nearbys, validate_finance_underlying, validate_option_class, validate_price_level,
};
use crate::errors::{Result, TqError};
use chrono::Utc;
use serde_json::{Map, Value, json};

impl InsAPI {
    /// 发送 GraphQL 查询并等待结果
    ///
    /// `query` 为 GraphQL 文本，`variables` 为变量对象。
    pub async fn query_graphql(&self, query: &str, variables: Option<Value>) -> Result<Value> {
        let vars = variables.unwrap_or_else(|| Value::Object(Map::new()));
        self.send_ins_query(query.to_string(), Some(vars), None, 60).await
    }

    /// 查询合约列表
    ///
    /// 支持按合约类型、交易所、品种、是否到期与夜盘筛选。
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
        if let Some(ex) = exchange_id
            && matches!(ins_class, Some("INDEX") | Some("CONT"))
            && matches!(ex, "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX")
        {
            target_ex = Some(ex.to_string());
        }
        Ok(parse_query_quotes_result(&res, target_ex.as_deref()))
    }

    /// 查询主连合约列表
    ///
    /// 仅在股票行情系统下可用。
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

    #[expect(clippy::too_many_arguments, reason = "对外 API 保持与 Python SDK 查询参数一致")]
    /// 查询期权列表
    ///
    /// `underlying_symbol` 为标的合约，其余参数用于筛选期权集合。
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
            return Err(TqError::InvalidParameter("underlying_symbol 不能为空".to_string()));
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
        all_vars.insert(
            "instrument_id".to_string(),
            Value::Array(vec![json!(underlying_symbol)]),
        );
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

    #[expect(clippy::too_many_arguments, reason = "对外 API 保持与 Python SDK 查询参数一致")]
    pub async fn query_atm_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        price_level: &[i32],
        option_class: &str,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        has_a: Option<bool>,
    ) -> Result<Vec<Option<String>>> {
        if !self.stock {
            return Err(TqError::InvalidParameter(
                "期货行情系统(_stock = False)不支持当前接口调用".to_string(),
            ));
        }
        if underlying_symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "underlying_symbol 不能为空字符串。".to_string(),
            ));
        }
        validate_option_class(option_class)?;
        validate_price_level(price_level)?;

        let query = r#"query($instrument_id:[String], $derivative_class:[Class]){
  multi_symbol_info(instrument_id: $instrument_id) {
    ... on basic {
      instrument_id
      derivatives(class: $derivative_class) {
        edges {
          node {
            ... on basic {
              instrument_id
              english_name
            }
            ... on option {
              expired
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
        let mut vars = Map::new();
        vars.insert(
            "instrument_id".to_string(),
            Value::Array(vec![json!(underlying_symbol)]),
        );
        vars.insert("derivative_class".to_string(), json!(["OPTION"]));

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(vars)), None, 60)
            .await?;
        let nodes = parse_option_nodes(&res);
        let mut nodes = filter_option_nodes(nodes, Some(option_class), exercise_year, exercise_month, has_a, None);

        if nodes.is_empty() {
            return Ok(price_level.iter().map(|_| None).collect());
        }

        let atm_index = sort_options_and_get_atm_index(&mut nodes, underlying_price, option_class)?;
        let mut result = Vec::with_capacity(price_level.len());
        for pl in price_level {
            let idx = atm_index as i64 - *pl as i64;
            if idx >= 0 && (idx as usize) < nodes.len() {
                result.push(Some(nodes[idx as usize].instrument_id.clone()));
            } else {
                result.push(None);
            }
        }
        Ok(result)
    }

    pub async fn query_all_level_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        option_class: &str,
        exercise_year: Option<i32>,
        exercise_month: Option<i32>,
        has_a: Option<bool>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        if !self.stock {
            return Err(TqError::InvalidParameter(
                "期货行情系统(_stock = False)不支持当前接口调用".to_string(),
            ));
        }
        if underlying_symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "underlying_symbol 不能为空字符串。".to_string(),
            ));
        }
        validate_option_class(option_class)?;

        let query = r#"query($instrument_id:[String], $derivative_class:[Class]){
  multi_symbol_info(instrument_id: $instrument_id) {
    ... on basic {
      instrument_id
      derivatives(class: $derivative_class) {
        edges {
          node {
            ... on basic {
              instrument_id
              english_name
            }
            ... on option {
              expired
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
        let mut vars = Map::new();
        vars.insert(
            "instrument_id".to_string(),
            Value::Array(vec![json!(underlying_symbol)]),
        );
        vars.insert("derivative_class".to_string(), json!(["OPTION"]));

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(vars)), None, 60)
            .await?;
        let nodes = parse_option_nodes(&res);
        let mut nodes = filter_option_nodes(nodes, Some(option_class), exercise_year, exercise_month, has_a, None);

        if nodes.is_empty() {
            return Ok((vec![], vec![], vec![]));
        }

        let atm_index = sort_options_and_get_atm_index(&mut nodes, underlying_price, option_class)?;

        let in_money = nodes[..atm_index]
            .iter()
            .map(|o| o.instrument_id.clone())
            .collect::<Vec<_>>();
        let at_money = vec![nodes[atm_index].instrument_id.clone()];
        let out_money = nodes[atm_index + 1..]
            .iter()
            .map(|o| o.instrument_id.clone())
            .collect::<Vec<_>>();
        Ok((in_money, at_money, out_money))
    }

    pub async fn query_all_level_finance_options(
        &self,
        underlying_symbol: &str,
        underlying_price: f64,
        option_class: &str,
        nearbys: &[i32],
        has_a: Option<bool>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        if !self.stock {
            return Err(TqError::InvalidParameter(
                "期货行情系统(_stock = False)不支持当前接口调用".to_string(),
            ));
        }
        if underlying_symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "underlying_symbol 不能为空字符串。".to_string(),
            ));
        }
        validate_finance_underlying(underlying_symbol)?;
        validate_option_class(option_class)?;
        validate_finance_nearbys(underlying_symbol, nearbys)?;

        let query = r#"query($instrument_id:[String], $derivative_class:[Class]){
  multi_symbol_info(instrument_id: $instrument_id) {
    ... on basic {
      instrument_id
      derivatives(class: $derivative_class) {
        edges {
          node {
            ... on basic {
              instrument_id
              english_name
            }
            ... on option {
              expired
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
        let mut vars = Map::new();
        vars.insert(
            "instrument_id".to_string(),
            Value::Array(vec![json!(underlying_symbol)]),
        );
        vars.insert("derivative_class".to_string(), json!(["OPTION"]));

        let res = self
            .send_ins_query(query.to_string(), Some(Value::Object(vars)), None, 60)
            .await?;
        let nodes = parse_option_nodes(&res);
        let mut nodes = filter_option_nodes(nodes, Some(option_class), None, None, has_a, Some(nearbys));

        if nodes.is_empty() {
            return Ok((vec![], vec![], vec![]));
        }

        let atm_index = sort_options_and_get_atm_index(&mut nodes, underlying_price, option_class)?;

        let in_money = nodes[..atm_index]
            .iter()
            .map(|o| o.instrument_id.clone())
            .collect::<Vec<_>>();
        let at_money = vec![nodes[atm_index].instrument_id.clone()];
        let out_money = nodes[atm_index + 1..]
            .iter()
            .map(|o| o.instrument_id.clone())
            .collect::<Vec<_>>();
        Ok((in_money, at_money, out_money))
    }

    /// 查询合约基础信息
    ///
    /// 入参为合约代码列表，返回每个合约的基础字段对象。
    pub async fn query_symbol_info(&self, symbols: &[&str]) -> Result<Vec<Value>> {
        if symbols.is_empty() {
            return Err(TqError::InvalidParameter("symbol 不能为空列表".to_string()));
        }
        if symbols.iter().any(|s| s.is_empty()) {
            return Err(TqError::InvalidParameter("symbol 参数中不能有空字符串".to_string()));
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
}
