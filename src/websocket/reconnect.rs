use crate::datamanager::DataManager;
use rand::Rng;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub(crate) struct ReconnectTimer {
    next_reconnect_at: Instant,
}

impl ReconnectTimer {
    pub(crate) fn new() -> Self {
        Self {
            next_reconnect_at: Instant::now() + Duration::from_secs(rand::rng().random_range(10..21)),
        }
    }
}

pub(crate) fn is_ops_maintenance_window_cst() -> bool {
    use chrono::Timelike;

    let cst_now = chrono::Utc::now() + chrono::Duration::hours(8);
    cst_now.hour() == 19 && cst_now.minute() <= 30
}

pub(crate) fn next_reconnect_delay(
    timer: &std::sync::Mutex<ReconnectTimer>,
    reconnect_count: u32,
    fallback: Duration,
) -> Duration {
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
        let state_matches = dm.with_path_ref(&["charts", chart_id.as_str(), "state"], |state| {
            let Some(Value::Object(state)) = state else {
                return false;
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
            true
        });
        if !state_matches {
            return false;
        }

        let chart_ready = dm.with_path_ref(&["charts", chart_id.as_str()], |chart| {
            let Some(Value::Object(chart)) = chart else {
                return false;
            };
            let left_id = get_i64(chart.get("left_id"), -1);
            let right_id = get_i64(chart.get("right_id"), -1);
            let more_data = get_bool(chart.get("more_data"), true);
            !(more_data || (left_id == -1 && right_id == -1))
        });
        if !chart_ready {
            return false;
        }

        let mdhis_ready = dm.with_path_ref(&["mdhis_more_data"], |value| !get_bool(value, true));
        if !mdhis_ready {
            return false;
        }

        let ins_list = request
            .get("ins_list")
            .and_then(|ins_list| ins_list.as_str())
            .unwrap_or("");
        let duration = get_i64(request.get("duration"), -1);
        for symbol in ins_list.split(',').filter(|symbol| !symbol.is_empty()) {
            let ready = if duration == 0 {
                dm.with_path_ref(&["ticks", symbol], |value| {
                    get_i64(value.and_then(|value| value.get("last_id")), -1)
                })
            } else {
                let duration_str = duration.to_string();
                dm.with_path_ref(&["klines", symbol, &duration_str], |value| {
                    get_i64(value.and_then(|value| value.get("last_id")), -1)
                })
            };
            if ready == -1 {
                return false;
            }
        }
    }

    if let Some(subscribe) = subscribe_quote.as_ref()
        && let Some(sub_ins_list) = subscribe.get("ins_list").and_then(|ins_list| ins_list.as_str())
    {
        let ins_list_matches = dm.with_path_ref(&["ins_list"], |value| match value {
            Some(Value::String(data_ins_list)) => data_ins_list == sub_ins_list,
            _ => false,
        });
        if !ins_list_matches {
            return false;
        }
    }

    true
}

pub(crate) fn extract_trade_positions(dm: &DataManager) -> HashMap<String, HashSet<String>> {
    dm.with_path_ref(&["trade"], |trade| {
        let Some(Value::Object(trade)) = trade else {
            return HashMap::new();
        };

        let mut result = HashMap::new();
        for (user, value) in trade {
            if let Value::Object(user_obj) = value
                && let Some(Value::Object(positions)) = user_obj.get("positions")
            {
                let symbols = positions.keys().cloned().collect::<HashSet<_>>();
                result.insert(user.to_string(), symbols);
            }
        }
        result
    })
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
        let more_data = dm.with_path_ref(&["trade", user, "trade_more_data"], |value| get_bool(value, true));
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
    dm.with_path_ref(&["trade"], |trade| match trade {
        Some(Value::Object(trade)) => trade.keys().cloned().collect(),
        _ => Vec::new(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_timer_is_per_connection() {
        let now = Instant::now();
        let first = std::sync::Mutex::new(ReconnectTimer {
            next_reconnect_at: now + Duration::from_secs(18),
        });
        let second = std::sync::Mutex::new(ReconnectTimer {
            next_reconnect_at: now - Duration::from_secs(1),
        });

        let first_delay = next_reconnect_delay(&first, 0, Duration::ZERO);
        let second_delay = next_reconnect_delay(&second, 0, Duration::ZERO);

        assert!(
            first_delay >= Duration::from_secs(1),
            "the first timer should preserve its own scheduled reconnect delay: {first_delay:?}"
        );
        assert!(
            second_delay < Duration::from_secs(1),
            "a second connection should not inherit another connection's reconnect delay: {second_delay:?}"
        );
    }
}
