use crate::errors::{Result, TqError};
use serde_json::{Map, Value};

pub(super) fn validate_query_variables(variables: &Value) -> Result<()> {
    let obj = variables
        .as_object()
        .ok_or_else(|| TqError::InvalidParameter("variables 必须是对象".to_string()))?;
    for value in obj.values() {
        match value {
            Value::String(s) => {
                if s.is_empty() {
                    return Err(TqError::InvalidParameter("variables 不支持空字符串".to_string()));
                }
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return Err(TqError::InvalidParameter("variables 不支持空列表".to_string()));
                }
                for item in items {
                    if let Value::String(s) = item
                        && s.is_empty()
                    {
                        return Err(TqError::InvalidParameter(
                            "variables 不支持列表中包含空字符串".to_string(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn match_query_cache(symbol: &Map<String, Value>, query: &Value, variables: &Value) -> bool {
    match (symbol.get("query"), symbol.get("variables")) {
        (Some(q), Some(v)) => q == query && v == variables,
        _ => false,
    }
}

pub(super) fn validate_option_class(option_class: &str) -> Result<()> {
    if option_class != "CALL" && option_class != "PUT" {
        return Err(TqError::InvalidParameter(
            "option_class 参数错误，option_class 必须是 'CALL' 或者 'PUT'".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_price_level(price_level: &[i32]) -> Result<()> {
    if !price_level.iter().all(|pl| (-100..=100).contains(pl)) {
        return Err(TqError::InvalidParameter(
            "price_level 必须为 -100 ~ 100 之间的整数".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_finance_underlying(underlying_symbol: &str) -> Result<()> {
    let allowed = [
        "SSE.000300",
        "SSE.510050",
        "SSE.510300",
        "SZSE.159919",
        "SZSE.159915",
        "SZSE.159922",
        "SSE.510500",
        "SSE.000016",
        "SSE.000852",
    ];
    if !allowed.contains(&underlying_symbol) {
        return Err(TqError::InvalidParameter("不支持的标的合约".to_string()));
    }
    Ok(())
}

pub(super) fn validate_finance_nearbys(underlying_symbol: &str, nearbys: &[i32]) -> Result<()> {
    let is_index = matches!(underlying_symbol, "SSE.000300" | "SSE.000852" | "SSE.000016");
    if is_index {
        if nearbys.iter().any(|v| !matches!(v, 0..=5)) {
            return Err(TqError::InvalidParameter(format!(
                "股指期权标的为：{}，exercise_date 参数应该是在 [0, 1, 2, 3, 4, 5] 之间。",
                underlying_symbol
            )));
        }
    } else if nearbys.iter().any(|v| !matches!(v, 0..=3)) {
        return Err(TqError::InvalidParameter(format!(
            "ETF 期权标的为：{}，exercise_date 参数应该是在 [0, 1, 2, 3] 之间。",
            underlying_symbol
        )));
    }
    Ok(())
}
