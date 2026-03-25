use super::BacktestTime;
use serde_json::Value;

pub(super) fn parse_backtest_time(value: &Value) -> Option<BacktestTime> {
    if let Value::Object(map) = value {
        let start_dt = value_to_i64(map.get("start_dt")?)?;
        let end_dt = value_to_i64(map.get("end_dt")?)?;
        let current_dt = value_to_i64(map.get("current_dt")?)?;
        return Some(BacktestTime {
            start_dt,
            end_dt,
            current_dt,
        });
    }
    None
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(num) => num
            .as_i64()
            .or_else(|| num.as_u64().map(|v| v as i64))
            .or_else(|| num.as_f64().map(|v| v as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}
