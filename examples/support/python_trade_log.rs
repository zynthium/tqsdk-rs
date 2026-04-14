use chrono::{DateTime, FixedOffset, Utc};
use tokio::task::JoinHandle;
use tqsdk_rs::{AccountHandle, ORDER_STATUS_ALIVE, Order, RuntimeResult, TradeSessionEventKind};

fn python_float(value: f64) -> String {
    let mut rendered = format!("{value:.10}");
    while rendered.contains('.') && rendered.ends_with('0') && !rendered.ends_with(".0") {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.push('0');
    }
    rendered
}

fn format_shanghai_nanos(nanos: i64) -> String {
    let dt = DateTime::<Utc>::from_timestamp_nanos(nanos);
    let tz = FixedOffset::east_opt(8 * 3600).expect("fixed CST offset should be available");
    dt.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

fn format_order_event(order_id: &str, order: &Order) -> Option<String> {
    if order.status == ORDER_STATUS_ALIVE {
        Some(format!(
            "    INFO - 模拟交易下单 {}, {}: 时间: {}, 合约: {}.{}, 开平: {}, 方向: {}, 手数: {}, 价格: {}",
            order.user_id,
            order_id,
            format_shanghai_nanos(order.insert_date_time),
            order.exchange_id,
            order.instrument_id,
            order.offset,
            order.direction,
            order.volume_orign,
            python_float(order.price())
        ))
    } else if !order.last_msg.is_empty() {
        Some(format!(
            "    INFO - 模拟交易委托单 {}, {}: {}",
            order.user_id, order_id, order.last_msg
        ))
    } else {
        None
    }
}

pub fn spawn_order_logger(account: &AccountHandle) -> RuntimeResult<JoinHandle<()>> {
    let mut order_events = account.subscribe_order_events()?;
    Ok(tokio::spawn(async move {
        while let Ok(event) = order_events.recv().await {
            if let TradeSessionEventKind::OrderUpdated { order_id, order } = event.kind
                && let Some(line) = format_order_event(&order_id, &order)
            {
                println!("{line}");
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::format_order_event;
    use serde_json::json;
    use tqsdk_rs::{DIRECTION_SELL, OFFSET_OPEN, ORDER_STATUS_ALIVE, ORDER_STATUS_FINISHED, Order, PRICE_TYPE_LIMIT};

    fn sample_order() -> Order {
        serde_json::from_value(json!({
            "user_id": "TQSIM",
            "order_id": "sim-order-1",
            "exchange_id": "SHFE",
            "instrument_id": "bu2012",
            "direction": DIRECTION_SELL,
            "offset": OFFSET_OPEN,
            "volume_orign": 3,
            "price_type": PRICE_TYPE_LIMIT,
            "limit_price": 2580.0,
            "insert_date_time": 1_599_225_480_000_000_000i64,
            "status": ORDER_STATUS_ALIVE,
            "volume_left": 3
        }))
        .unwrap()
    }

    #[test]
    fn formats_alive_order_like_python_sim_log() {
        let line = format_order_event("sim-order-1", &sample_order()).unwrap();
        assert_eq!(
            line,
            "    INFO - 模拟交易下单 TQSIM, sim-order-1: 时间: 2020-09-04 21:18:00.000000, 合约: SHFE.bu2012, 开平: OPEN, 方向: SELL, 手数: 3, 价格: 2580.0"
        );
    }

    #[test]
    fn formats_finished_order_like_python_sim_log() {
        let mut order = sample_order();
        order.status = ORDER_STATUS_FINISHED.to_string();
        order.last_msg = "全部成交".to_string();
        let line = format_order_event("sim-order-1", &order).unwrap();
        assert_eq!(line, "    INFO - 模拟交易委托单 TQSIM, sim-order-1: 全部成交");
    }
}
