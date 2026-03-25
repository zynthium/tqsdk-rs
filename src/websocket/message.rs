use serde_json::{Value, json};

pub(crate) fn extract_notify_code(value: &Value) -> Option<i64> {
    if let Some(code) = value.as_i64() {
        return Some(code);
    }
    value.as_str().and_then(|s| s.parse::<i64>().ok())
}

pub(crate) fn build_connection_notify(
    code: i64,
    level: &str,
    content: String,
    url: String,
    conn_id: String,
) -> Value {
    let notify_id = uuid::Uuid::new_v4().to_string();
    json!({
        "aid": "rtn_data",
        "data": [{
            "notify": {
                notify_id: {
                    "type": "MESSAGE",
                    "level": level,
                    "code": code,
                    "conn_id": conn_id,
                    "content": content,
                    "url": url
                }
            }
        }]
    })
}

pub(crate) fn sanitize_log_pack_value(value: &Value) -> String {
    if value
        .get("aid")
        .and_then(|v| v.as_str())
        .map(|aid| aid == "req_login")
        .unwrap_or(false)
        && let Some(obj) = value.as_object()
    {
        let mut cloned = obj.clone();
        cloned.remove("password");
        return Value::Object(cloned).to_string();
    }
    value.to_string()
}

pub(crate) fn has_reconnect_notify(item: &Value) -> bool {
    let notify = match item.get("notify").and_then(|v| v.as_object()) {
        Some(notify) => notify,
        None => return false,
    };
    notify.values().any(|notify_value| {
        notify_value
            .get("code")
            .and_then(extract_notify_code)
            .map(|code| code == 2019112902)
            .unwrap_or(false)
    })
}
