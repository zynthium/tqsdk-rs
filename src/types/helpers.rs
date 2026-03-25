use serde::{Deserialize, Deserializer};

/// 返回 NaN 作为默认值
pub(super) fn default_nan() -> f64 {
    f64::NAN
}

/// 将 null 转换为 NaN
pub(super) fn deserialize_f64_or_nan<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<f64>::deserialize(deserializer)?;
    Ok(opt.unwrap_or(f64::NAN))
}

/// 将 null 转换为 0
pub(super) fn deserialize_i64_or_zero<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<i64>::deserialize(deserializer)?;
    Ok(opt.unwrap_or(0))
}

/// 将 null 或 "-" 转换为 NaN
pub(super) fn deserialize_f64_or_nan_or_dash<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| Error::custom("invalid number")),
        serde_json::Value::String(s) if s == "-" => Ok(f64::NAN),
        serde_json::Value::Null => Ok(f64::NAN),
        _ => Err(Error::custom("expected number, string \"-\", or null")),
    }
}

pub(super) fn default_currency() -> String {
    "CNY".to_string()
}

pub(super) fn deserialize_f64_default<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<f64>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}
