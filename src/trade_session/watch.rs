use super::{TradeEventHub, TradeSession};
use crate::datamanager::DataManager;
use crate::types::{Order, Trade};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

impl TradeSession {
    /// 启动数据监听
    pub(super) async fn start_watching(&self) {
        let mut guard = self.watch_task.lock().unwrap();
        if guard.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }

        let dm = Arc::clone(&self.dm);
        let user_id = self.user_id.clone();
        let logged_in = Arc::clone(&self.logged_in);
        let running = Arc::clone(&self.running);
        let trade_events = Arc::clone(&self.trade_events);
        let snapshot_epoch_tx = self.snapshot_epoch_tx.clone();
        let mut epoch_rx = dm.subscribe_epoch();

        info!("TradeSession 开始监听数据更新");

        *guard = Some(tokio::spawn(async move {
            let mut last_processed_epoch = 0i64;

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let current_global_epoch = *epoch_rx.borrow_and_update();
                if current_global_epoch <= last_processed_epoch {
                    if epoch_rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }

                let session_changed = dm.get_path_epoch(&["trade", &user_id, "session"]) > last_processed_epoch;
                let account_changed = dm.get_path_epoch(&["trade", &user_id, "accounts", "CNY"]) > last_processed_epoch;
                let positions_changed = dm.get_path_epoch(&["trade", &user_id, "positions"]) > last_processed_epoch;
                let orders_changed = dm.get_path_epoch(&["trade", &user_id, "orders"]) > last_processed_epoch;
                let trades_changed = dm.get_path_epoch(&["trade", &user_id, "trades"]) > last_processed_epoch;

                if session_changed
                    && let Some(serde_json::Value::Object(session_map)) =
                        dm.get_by_path(&["trade", &user_id, "session"])
                    && session_map
                        .get("trading_day")
                        .is_some_and(|trading_day| !trading_day.is_null())
                    && logged_in
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    info!("交易会话已登录: user_id={}", user_id);
                }

                if account_changed {
                    debug!("账户快照已更新: user_id={}", user_id);
                }

                if positions_changed {
                    debug!("持仓快照已更新: user_id={}", user_id);
                }

                if orders_changed {
                    Self::process_order_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
                }

                if trades_changed {
                    Self::process_trade_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
                }

                if session_changed || account_changed || positions_changed || orders_changed || trades_changed {
                    let _ = snapshot_epoch_tx.send_replace(Some(current_global_epoch));
                }

                last_processed_epoch = current_global_epoch;
            }
        }));
    }

    /// 处理委托单更新
    pub(crate) async fn process_order_update(
        dm: &Arc<DataManager>,
        user_id: &str,
        trade_events: &Arc<TradeEventHub>,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(orders_map)) = dm.get_by_path(&["trade", user_id, "orders"]) {
            for (order_id, order_data) in &orders_map {
                if order_id.starts_with('_') {
                    continue;
                }

                if dm.get_path_epoch(&["trade", user_id, "orders", order_id]) <= last_epoch {
                    continue;
                }

                match serde_json::from_value::<Order>(order_data.clone()) {
                    Ok(order) => {
                        debug!("订单更新: order_id={}", order_id);
                        let _ = trade_events.publish_order(order_id.clone(), order);
                    }
                    Err(_) => {
                        warn!("订单数据解析失败: order_id={}", order_id);
                    }
                }
            }
        }
    }

    /// 处理成交更新
    pub(crate) async fn process_trade_update(
        dm: &Arc<DataManager>,
        user_id: &str,
        trade_events: &Arc<TradeEventHub>,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(trades_map)) = dm.get_by_path(&["trade", user_id, "trades"]) {
            for (trade_id, trade_data) in &trades_map {
                if trade_id.starts_with('_') {
                    continue;
                }

                if dm.get_path_epoch(&["trade", user_id, "trades", trade_id]) <= last_epoch {
                    continue;
                }

                match serde_json::from_value::<Trade>(trade_data.clone()) {
                    Ok(trade) => {
                        debug!("成交更新: trade_id={}", trade_id);
                        let _ = trade_events.publish_trade(trade_id.clone(), trade);
                    }
                    Err(_) => {
                        warn!("成交数据解析失败: trade_id={}", trade_id);
                    }
                }
            }
        }
    }
}
