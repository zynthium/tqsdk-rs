use super::{PositionCallback, TradeEventHub, TradeSession};
use crate::datamanager::DataManager;
use crate::types::{Order, PositionUpdate, Trade};
use async_channel::{Sender, TrySendError};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{debug, error, info, warn};

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
        let account_tx = self.account_tx.clone();
        let position_tx = self.position_tx.clone();
        let trade_events = Arc::clone(&self.trade_events);
        let on_account = Arc::clone(&self.on_account);
        let on_position = Arc::clone(&self.on_position);
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

                if let Some(serde_json::Value::Object(session_map)) = dm.get_by_path(&["trade", &user_id, "session"])
                    && session_map
                        .get("trading_day")
                        .is_some_and(|trading_day| !trading_day.is_null())
                    && logged_in
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    info!("交易会话已登录: user_id={}", user_id);
                }

                if dm.get_path_epoch(&["trade", &user_id, "accounts", "CNY"]) > last_processed_epoch {
                    match dm.get_account_data(&user_id, "CNY") {
                        Ok(account) => {
                            debug!("账户更新: balance={}", account.balance);
                            match account_tx.try_send(account.clone()) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    warn!("TradeSession 账户队列已满，丢弃一次更新");
                                }
                                Err(TrySendError::Closed(_)) => {}
                            }
                            let callback = on_account.read().await.clone();
                            if let Some(callback) = callback {
                                callback(account);
                            }
                        }
                        Err(e) => {
                            error!("获取账户数据失败（这不应该发生）: {}", e);
                        }
                    }
                }

                if dm.get_path_epoch(&["trade", &user_id, "positions"]) > last_processed_epoch {
                    Self::process_position_update(&dm, &user_id, &position_tx, &on_position, last_processed_epoch)
                        .await;
                }

                if dm.get_path_epoch(&["trade", &user_id, "orders"]) > last_processed_epoch {
                    Self::process_order_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
                }

                if dm.get_path_epoch(&["trade", &user_id, "trades"]) > last_processed_epoch {
                    Self::process_trade_update(&dm, &user_id, &trade_events, last_processed_epoch).await;
                }

                last_processed_epoch = current_global_epoch;
            }
        }));
    }

    /// 处理持仓更新
    async fn process_position_update(
        dm: &Arc<DataManager>,
        user_id: &str,
        position_tx: &Sender<PositionUpdate>,
        on_position: &PositionCallback,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(positions_map)) = dm.get_by_path(&["trade", user_id, "positions"]) {
            for (symbol, _) in &positions_map {
                if symbol.starts_with('_') {
                    continue;
                }

                if dm.get_path_epoch(&["trade", user_id, "positions", symbol]) > last_epoch {
                    match dm.get_position_data(user_id, symbol) {
                        Ok(position) => {
                            debug!("持仓更新: symbol={}", symbol);

                            let update = PositionUpdate {
                                symbol: symbol.clone(),
                                position: position.clone(),
                            };

                            match position_tx.try_send(update) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    warn!("TradeSession 持仓队列已满，丢弃一次更新");
                                }
                                Err(TrySendError::Closed(_)) => {}
                            }

                            let callback = on_position.read().await.clone();
                            if let Some(callback) = callback {
                                callback(symbol.clone(), position);
                            }
                        }
                        Err(e) => {
                            error!("获取持仓数据失败: symbol={}, error={}", symbol, e);
                        }
                    }
                }
            }
        }
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
