use super::{OrderCallback, PositionCallback, TradeCallback, TradeSession};
use crate::datamanager::DataManager;
use crate::types::{Order, PositionUpdate, Trade};
use async_channel::{Sender, TrySendError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info, warn};

impl TradeSession {
    /// 启动数据监听
    pub(super) async fn start_watching(&self) {
        let dm_clone = Arc::clone(&self.dm);
        let user_id = self.user_id.clone();
        let logged_in = Arc::clone(&self.logged_in);
        let running = Arc::clone(&self.running);
        let account_tx = self.account_tx.clone();
        let position_tx = self.position_tx.clone();
        let order_tx = self.order_tx.clone();
        let trade_tx = self.trade_tx.clone();
        let on_account = Arc::clone(&self.on_account);
        let on_position = Arc::clone(&self.on_position);
        let on_order = Arc::clone(&self.on_order);
        let on_trade = Arc::clone(&self.on_trade);
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_dirty = Arc::new(AtomicBool::new(false));
        let last_processed_epoch = Arc::new(std::sync::Mutex::new(0i64));

        info!("TradeSession 开始监听数据更新");

        let dm_for_callback = Arc::clone(&dm_clone);
        let cb_id = dm_clone.on_data_register(move || {
            worker_dirty.store(true, Ordering::SeqCst);
            if worker_running.swap(true, Ordering::SeqCst) {
                return;
            }
            let dm = Arc::clone(&dm_for_callback);
            let user_id = user_id.clone();
            let logged_in = Arc::clone(&logged_in);
            let running = Arc::clone(&running);
            let account_tx = account_tx.clone();
            let position_tx = position_tx.clone();
            let order_tx = order_tx.clone();
            let trade_tx = trade_tx.clone();
            let on_account = Arc::clone(&on_account);
            let on_position = Arc::clone(&on_position);
            let on_order = Arc::clone(&on_order);
            let on_trade = Arc::clone(&on_trade);
            let worker_running = Arc::clone(&worker_running);
            let worker_dirty = Arc::clone(&worker_dirty);
            let last_processed_epoch = Arc::clone(&last_processed_epoch);

            tokio::spawn(async move {
                loop {
                    worker_dirty.store(false, Ordering::SeqCst);
                    if running.load(Ordering::SeqCst) {
                        let current_global_epoch = dm.get_epoch();
                        let last_epoch = { *last_processed_epoch.lock().unwrap() };

                        if let Some(serde_json::Value::Object(session_map)) =
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

                        if dm.get_path_epoch(&["trade", &user_id, "accounts", "CNY"]) > last_epoch {
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

                        if dm.get_path_epoch(&["trade", &user_id, "positions"]) > last_epoch {
                            Self::process_position_update(&dm, &user_id, &position_tx, &on_position, last_epoch).await;
                        }

                        if dm.get_path_epoch(&["trade", &user_id, "orders"]) > last_epoch {
                            Self::process_order_update(&dm, &user_id, &order_tx, &on_order, last_epoch).await;
                        }

                        if dm.get_path_epoch(&["trade", &user_id, "trades"]) > last_epoch {
                            Self::process_trade_update(&dm, &user_id, &trade_tx, &on_trade, last_epoch).await;
                        }

                        *last_processed_epoch.lock().unwrap() = current_global_epoch;
                    }
                    if !worker_dirty.load(Ordering::SeqCst) {
                        worker_running.store(false, Ordering::SeqCst);
                        if worker_dirty.load(Ordering::SeqCst) && !worker_running.swap(true, Ordering::SeqCst) {
                            continue;
                        }
                        break;
                    }
                }
            });
        });
        *self.data_cb_id.lock().unwrap() = Some(cb_id);
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
        order_tx: &Sender<Order>,
        on_order: &OrderCallback,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(orders_map)) = dm.get_by_path(&["trade", user_id, "orders"]) {
            for (order_id, order_data) in &orders_map {
                if order_id.starts_with('_') {
                    continue;
                }

                if dm.get_path_epoch(&["trade", user_id, "orders", order_id]) > last_epoch {
                    if let Ok(order) = serde_json::from_value::<Order>(order_data.clone()) {
                        debug!("订单更新: order_id={}", order_id);

                        match order_tx.try_send(order.clone()) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                warn!("TradeSession 订单队列已满，丢弃一次更新");
                            }
                            Err(TrySendError::Closed(_)) => {}
                        }

                        let callback = on_order.read().await.clone();
                        if let Some(callback) = callback {
                            callback(order);
                        }
                    } else {
                        warn!("订单数据解析失败: order_id={}", order_id);
                    }
                }
            }
        }
    }

    /// 处理成交更新
    async fn process_trade_update(
        dm: &Arc<DataManager>,
        user_id: &str,
        trade_tx: &Sender<Trade>,
        on_trade: &TradeCallback,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(trades_map)) = dm.get_by_path(&["trade", user_id, "trades"]) {
            for (trade_id, trade_data) in &trades_map {
                if trade_id.starts_with('_') {
                    continue;
                }

                if dm.get_path_epoch(&["trade", user_id, "trades", trade_id]) > last_epoch
                    && let Ok(trade) = serde_json::from_value::<Trade>(trade_data.clone())
                {
                    debug!("成交更新: trade_id={}", trade_id);

                    match trade_tx.try_send(trade.clone()) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            warn!("TradeSession 成交队列已满，丢弃一次更新");
                        }
                        Err(TrySendError::Closed(_)) => {}
                    }

                    let callback = on_trade.read().await.clone();
                    if let Some(callback) = callback {
                        callback(trade);
                    }
                }
            }
        }
    }
}
