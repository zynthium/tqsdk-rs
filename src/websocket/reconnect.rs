use crate::datamanager::DataManager;
use rand::Rng;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

struct SharedReconnectTimer {
    next_reconnect_at: Instant,
}

static RECONNECT_TIMER: OnceLock<std::sync::Mutex<SharedReconnectTimer>> = OnceLock::new();

pub(crate) fn is_ops_maintenance_window_cst() -> bool {
    use chrono::Timelike;

    let cst_now = chrono::Utc::now() + chrono::Duration::hours(8);
    cst_now.hour() == 19 && cst_now.minute() <= 30
}

pub(crate) fn next_shared_reconnect_delay(reconnect_count: u32, fallback: Duration) -> Duration {
    let timer = RECONNECT_TIMER.get_or_init(|| {
        let initial = Duration::from_secs(rand::rng().random_range(10..21));
        std::sync::Mutex::new(SharedReconnectTimer {
            next_reconnect_at: Instant::now() + initial,
        })
    });
    let now = Instant::now();
    let mut guard = timer.lock().unwrap();
    let wait = if guard.next_reconnect_at > now {
        guard.next_reconnect_at.duration_since(now)
    } else {
        Duration::ZERO
    };

    if guard.next_reconnect_at <= now {
        let exp = reconnect_count.min(6);
        let base_secs = (1u64 << exp) * 10;
        let lower = Duration::from_secs(base_secs).max(fallback);
        let upper = lower.saturating_mul(2);
        let jitter = pseudo_duration_between(lower, upper);
        guard.next_reconnect_at = now + jitter;
    }

    wait.max(fallback)
}

pub(crate) fn is_md_reconnect_complete(
    dm: &DataManager,
    charts: &HashMap<String, Value>,
    subscribe_quote: &Option<Value>,
) -> bool {
    let set_chart_packs: Vec<(&String, &Value)> = charts
        .iter()
        .filter(|(_, value)| {
            value.get("aid").and_then(|aid| aid.as_str()) == Some("set_chart")
                && value
                    .get("ins_list")
                    .and_then(|ins_list| ins_list.as_str())
                    .map(|ins_list| !ins_list.is_empty())
                    .unwrap_or(false)
        })
        .collect();

    for (chart_id, request) in set_chart_packs.iter() {
        let state = match dm.get_by_path(&["charts", chart_id.as_str(), "state"]) {
            Some(Value::Object(state)) => state,
            _ => return false,
        };
        for key in [
            "ins_list",
            "duration",
            "view_width",
            "left_kline_id",
            "focus_datetime",
            "focus_position",
        ] {
            if let Some(req_val) = request.get(key)
                && state.get(key) != Some(req_val)
            {
                return false;
            }
        }

        let chart = match dm.get_by_path(&["charts", chart_id.as_str()]) {
            Some(Value::Object(chart)) => chart,
            _ => return false,
        };
        let left_id = get_i64(chart.get("left_id"), -1);
        let right_id = get_i64(chart.get("right_id"), -1);
        let more_data = get_bool(chart.get("more_data"), true);
        if left_id == -1 && right_id == -1 {
            return false;
        }
        if more_data {
            return false;
        }
        if get_bool(dm.get_by_path(&["mdhis_more_data"]).as_ref(), true) {
            return false;
        }

        let ins_list = request
            .get("ins_list")
            .and_then(|ins_list| ins_list.as_str())
            .unwrap_or("");
        let duration = get_i64(request.get("duration"), -1);
        for symbol in ins_list.split(',').filter(|symbol| !symbol.is_empty()) {
            let last_id = if duration == 0 {
                dm.get_by_path(&["ticks", symbol])
                    .and_then(|value| value.get("last_id").cloned())
            } else {
                let duration_str = duration.to_string();
                dm.get_by_path(&["klines", symbol, &duration_str])
                    .and_then(|value| value.get("last_id").cloned())
            };
            if get_i64(last_id.as_ref(), -1) == -1 {
                return false;
            }
        }
    }

    if let Some(subscribe) = subscribe_quote.as_ref()
        && let Some(sub_ins_list) = subscribe.get("ins_list").and_then(|ins_list| ins_list.as_str())
    {
        match dm.get_by_path(&["ins_list"]) {
            Some(Value::String(data_ins_list)) => {
                if data_ins_list != sub_ins_list {
                    return false;
                }
            }
            _ => return false,
        }
    }

    true
}

pub(crate) fn extract_trade_positions(dm: &DataManager) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();
    let trade = match dm.get_by_path(&["trade"]) {
        Some(Value::Object(trade)) => trade,
        _ => return result,
    };
    for (user, value) in trade.iter() {
        if let Value::Object(user_obj) = value
            && let Some(Value::Object(positions)) = user_obj.get("positions")
        {
            let symbols = positions.keys().cloned().collect::<HashSet<_>>();
            result.insert(user.to_string(), symbols);
        }
    }
    result
}

pub(crate) fn is_trade_reconnect_complete(
    dm: &DataManager,
    prev_positions: &HashMap<String, HashSet<String>>,
) -> Option<Vec<Value>> {
    let users = if prev_positions.is_empty() {
        trade_users_from_dm(dm)
    } else {
        prev_positions.keys().cloned().collect()
    };
    if users.is_empty() {
        return Some(Vec::new());
    }
    for user in users.iter() {
        let more_data = get_bool(dm.get_by_path(&["trade", user, "trade_more_data"]).as_ref(), true);
        if more_data {
            return None;
        }
    }

    let current_positions = extract_trade_positions(dm);
    let mut removal_diffs = Vec::new();
    for (user, previous) in prev_positions.iter() {
        if let Some(current) = current_positions.get(user) {
            let removed: Vec<String> = previous.difference(current).cloned().collect();
            if !removed.is_empty() {
                let mut positions = serde_json::Map::new();
                for symbol in removed {
                    positions.insert(symbol, Value::Null);
                }
                let mut user_map = serde_json::Map::new();
                user_map.insert("positions".to_string(), Value::Object(positions));
                let mut trade_map = serde_json::Map::new();
                trade_map.insert(user.clone(), Value::Object(user_map));
                let mut root = serde_json::Map::new();
                root.insert("trade".to_string(), Value::Object(trade_map));
                removal_diffs.push(Value::Object(root));
            }
        }
    }
    Some(removal_diffs)
}

fn get_i64(value: Option<&Value>, default: i64) -> i64 {
    value
        .and_then(|value| value.as_i64())
        .or_else(|| value.and_then(|value| value.as_str()?.parse::<i64>().ok()))
        .unwrap_or(default)
}

fn get_bool(value: Option<&Value>, default: bool) -> bool {
    value.and_then(|value| value.as_bool()).unwrap_or(default)
}

fn trade_users_from_dm(dm: &DataManager) -> Vec<String> {
    let trade = match dm.get_by_path(&["trade"]) {
        Some(Value::Object(trade)) => trade,
        _ => return Vec::new(),
    };
    trade.keys().cloned().collect()
}

fn pseudo_duration_between(lower: Duration, upper: Duration) -> Duration {
    if upper <= lower {
        return lower;
    }
    let range = upper - lower;
    let nanos = range.as_nanos() as u64;
    if nanos == 0 {
        return lower;
    }
    let offset = rand::rng().random_range(0..nanos);
    lower + Duration::from_nanos(offset)
}
