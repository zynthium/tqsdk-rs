//! 交易会话实现
//!
//! 实现实盘交易功能

use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use crate::types::{
    Account, InsertOrderRequest, Notification, Order, Position, PositionUpdate, Trade,
};
use crate::websocket::{TqTradeWebsocket, WebSocketConfig};
use async_channel::{Receiver, Sender, TrySendError, bounded};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

type AccountCallback = Arc<RwLock<Option<Arc<dyn Fn(Account) + Send + Sync>>>>;
type PositionCallback = Arc<RwLock<Option<Arc<dyn Fn(String, Position) + Send + Sync>>>>;
type OrderCallback = Arc<RwLock<Option<Arc<dyn Fn(Order) + Send + Sync>>>>;
type TradeCallback = Arc<RwLock<Option<Arc<dyn Fn(Trade) + Send + Sync>>>>;
type NotificationCallback = Arc<RwLock<Option<Arc<dyn Fn(Notification) + Send + Sync>>>>;
type ErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(String) + Send + Sync>>>>;

/// 交易会话
#[allow(unused)]
pub struct TradeSession {
    broker: String,
    user_id: String,
    password: String,
    dm: Arc<DataManager>,
    ws: Arc<TqTradeWebsocket>,

    // Channels（使用 async-channel）
    account_tx: Sender<Account>,
    account_rx: Receiver<Account>,
    position_tx: Sender<PositionUpdate>,
    position_rx: Receiver<PositionUpdate>,
    order_tx: Sender<Order>,
    order_rx: Receiver<Order>,
    trade_tx: Sender<Trade>,
    trade_rx: Receiver<Trade>,
    notification_tx: Sender<Notification>,
    notification_rx: Receiver<Notification>,

    // 回调
    on_account: AccountCallback,
    on_position: PositionCallback,
    on_order: OrderCallback,
    on_trade: TradeCallback,
    on_notification: NotificationCallback,
    on_error: ErrorCallback,

    // 状态（使用 Arc<AtomicBool> 避免锁开销，支持跨线程共享）
    logged_in: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}

impl TradeSession {
    /// 创建交易会话
    pub fn new(
        broker: String,
        user_id: String,
        password: String,
        dm: Arc<DataManager>,
        ws_url: String,
        ws_config: WebSocketConfig,
    ) -> Self {
        let trade_channel_capacity = ws_config.message_queue_capacity.max(1);

        let (account_tx, account_rx) = bounded(trade_channel_capacity);
        let (position_tx, position_rx) = bounded(trade_channel_capacity);
        let (order_tx, order_rx) = bounded(trade_channel_capacity);
        let (trade_tx, trade_rx) = bounded(trade_channel_capacity);
        let (notification_tx, notification_rx) = bounded(trade_channel_capacity);

        let ws = Arc::new(TqTradeWebsocket::new(ws_url, Arc::clone(&dm), ws_config));

        let on_notification: NotificationCallback = Arc::new(RwLock::new(None));
        let on_error: ErrorCallback = Arc::new(RwLock::new(None));

        let noti_tx = notification_tx.clone();
        let on_notification_for_ws = Arc::clone(&on_notification);
        ws.on_notify(move |noti| {
            let noti_tx = noti_tx.clone();
            let on_notification_for_ws = Arc::clone(&on_notification_for_ws);
            tokio::spawn(async move {
                match noti_tx.try_send(noti.clone()) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        warn!("TradeSession 通知队列已满，丢弃一条通知");
                    }
                    Err(TrySendError::Closed(_)) => {}
                }

                let callback = on_notification_for_ws.read().await.clone();
                if let Some(callback) = callback {
                    callback(noti);
                }
            });
        });

        let on_error_for_ws = Arc::clone(&on_error);
        ws.on_error(move |msg| {
            let on_error_for_ws = Arc::clone(&on_error_for_ws);
            tokio::spawn(async move {
                let callback = on_error_for_ws.read().await.clone();
                if let Some(callback) = callback {
                    callback(msg);
                }
            });
        });

        TradeSession {
            broker,
            user_id,
            password,
            dm,
            ws,
            account_tx,
            account_rx,
            position_tx,
            position_rx,
            order_tx,
            order_rx,
            trade_tx,
            trade_rx,
            notification_tx,
            notification_rx,
            on_account: Arc::new(RwLock::new(None)),
            on_position: Arc::new(RwLock::new(None)),
            on_order: Arc::new(RwLock::new(None)),
            on_trade: Arc::new(RwLock::new(None)),
            on_notification,
            on_error,
            logged_in: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            data_cb_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// 发送登录请求
    async fn send_login(&self) -> Result<()> {
        let login_req = serde_json::json!({
            "aid": "req_login",
            "bid": self.broker,
            "user_name": self.user_id,
            "password": self.password
        });

        info!(
            "发送交易登录请求: broker={}, user_id={}",
            self.broker, self.user_id
        );
        self.ws.send(&login_req).await?;
        Ok(())
    }

    async fn send_confirm_settlement(&self) -> Result<()> {
        let confirm_req = serde_json::json!({
            "aid": "confirm_settlement"
        });
        self.ws.send(&confirm_req).await?;
        Ok(())
    }

    /// 启动数据监听
    async fn start_watching(&self) {
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

        // 注册数据更新回调
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
                        {
                            if session_map
                                .get("trading_day")
                                .is_some_and(|trading_day| !trading_day.is_null())
                                && logged_in
                                    .compare_exchange(
                                        false,
                                        true,
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    )
                                    .is_ok()
                            {
                                info!("交易会话已登录: user_id={}", user_id);
                            }
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
                            Self::process_position_update(
                                &dm,
                                &user_id,
                                &position_tx,
                                &on_position,
                                last_epoch,
                            )
                            .await;
                        }

                        if dm.get_path_epoch(&["trade", &user_id, "orders"]) > last_epoch {
                            Self::process_order_update(
                                &dm, &user_id, &order_tx, &on_order, last_epoch,
                            )
                            .await;
                        }

                        if dm.get_path_epoch(&["trade", &user_id, "trades"]) > last_epoch {
                            Self::process_trade_update(
                                &dm, &user_id, &trade_tx, &on_trade, last_epoch,
                            )
                            .await;
                        }

                        *last_processed_epoch.lock().unwrap() = current_global_epoch;
                    }
                    if !worker_dirty.load(Ordering::SeqCst) {
                        worker_running.store(false, Ordering::SeqCst);
                        if worker_dirty.load(Ordering::SeqCst)
                            && !worker_running.swap(true, Ordering::SeqCst)
                        {
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
        if let Some(serde_json::Value::Object(positions_map)) =
            dm.get_by_path(&["trade", user_id, "positions"])
        {
            for (symbol, _) in positions_map.iter() {
                // 跳过内部元数据字段（以 _ 开头）
                if symbol.starts_with('_') {
                    continue;
                }

                // 检查单个持仓是否有更新
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
    async fn process_order_update(
        dm: &Arc<DataManager>,
        user_id: &str,
        order_tx: &Sender<Order>,
        on_order: &OrderCallback,
        last_epoch: i64,
    ) {
        if let Some(serde_json::Value::Object(orders_map)) =
            dm.get_by_path(&["trade", user_id, "orders"])
        {
            for (order_id, order_data) in orders_map.iter() {
                // 跳过内部元数据字段（以 _ 开头）
                if order_id.starts_with('_') {
                    continue;
                }

                // 检查单个订单是否有更新
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
        if let Some(serde_json::Value::Object(trades_map)) =
            dm.get_by_path(&["trade", user_id, "trades"])
        {
            for (trade_id, trade_data) in trades_map.iter() {
                // 跳过内部元数据字段（以 _ 开头）
                if trade_id.starts_with('_') {
                    continue;
                }

                // 检查单个成交是否有更新
                if dm.get_path_epoch(&["trade", user_id, "trades", trade_id]) > last_epoch {
                    if let Ok(trade) = serde_json::from_value::<Trade>(trade_data.clone()) {
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

    /// 注册账户更新回调
    pub async fn on_account<F>(&self, handler: F)
    where
        F: Fn(Account) + Send + Sync + 'static,
    {
        let mut guard = self.on_account.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册持仓更新回调
    pub async fn on_position<F>(&self, handler: F)
    where
        F: Fn(String, Position) + Send + Sync + 'static,
    {
        let mut guard = self.on_position.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册委托单更新回调
    pub async fn on_order<F>(&self, handler: F)
    where
        F: Fn(Order) + Send + Sync + 'static,
    {
        let mut guard = self.on_order.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册成交记录回调
    pub async fn on_trade<F>(&self, handler: F)
    where
        F: Fn(Trade) + Send + Sync + 'static,
    {
        let mut guard = self.on_trade.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册通知回调
    pub async fn on_notification<F>(&self, handler: F)
    where
        F: Fn(Notification) + Send + Sync + 'static,
    {
        let mut guard = self.on_notification.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册错误回调
    pub async fn on_error<F>(&self, handler: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut guard = self.on_error.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 获取账户更新 Channel（克隆接收端）
    pub fn account_channel(&self) -> Receiver<Account> {
        self.account_rx.clone()
    }

    /// 获取持仓更新 Channel（克隆接收端）
    pub fn position_channel(&self) -> Receiver<PositionUpdate> {
        self.position_rx.clone()
    }

    /// 获取委托单更新 Channel（克隆接收端）
    pub fn order_channel(&self) -> Receiver<Order> {
        self.order_rx.clone()
    }

    /// 获取成交记录 Channel（克隆接收端）
    pub fn trade_channel(&self) -> Receiver<Trade> {
        self.trade_rx.clone()
    }

    /// 获取通知 Channel（克隆接收端）
    pub fn notification_channel(&self) -> Receiver<Notification> {
        self.notification_rx.clone()
    }
}

impl TradeSession {
    /// 下单（返回委托单号）
    pub async fn insert_order(&self, req: &InsertOrderRequest) -> Result<String> {
        if !self.is_ready() {
            return Err(TqError::InternalError("交易会话未就绪".to_string()));
        }

        let exchange_id = req.get_exchange_id();
        let instrument_id = req.get_instrument_id();

        // 生成订单 ID
        use uuid::Uuid;
        let order_id = format!(
            "TQRS_{}",
            Uuid::new_v4().simple().to_string()[..8].to_uppercase()
        );

        // 确定时间条件
        let time_condition = if req.price_type == "ANY" {
            "IOC"
        } else {
            "GFD"
        };

        // 发送下单请求
        let order_req = serde_json::json!({
            "aid": "insert_order",
            "user_id": self.user_id,
            "order_id": order_id,
            "exchange_id": exchange_id,
            "instrument_id": instrument_id,
            "direction": req.direction,
            "offset": req.offset,
            "volume": req.volume,
            "price_type": req.price_type,
            "limit_price": req.limit_price,
            "volume_condition": "ANY",
            "time_condition": time_condition,
        });

        info!("发送下单请求: order_id={}, symbol={}", order_id, req.symbol);
        self.ws.send(&order_req).await?;
        Ok(order_id)
    }

    /// 撤单
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        if !self.is_ready() {
            return Err(TqError::InternalError("交易会话未就绪".to_string()));
        }

        // 发送撤单请求
        let cancel_req = serde_json::json!({
            "aid": "cancel_order",
            "user_id": self.user_id,
            "order_id": order_id
        });

        info!("发送撤单请求: order_id={}", order_id);
        self.ws.send(&cancel_req).await?;
        Ok(())
    }

    /// 获取账户信息
    pub async fn get_account(&self) -> Result<Account> {
        self.dm.get_account_data(&self.user_id, "CNY")
    }

    /// 获取指定合约的持仓
    pub async fn get_position(&self, symbol: &str) -> Result<Position> {
        self.dm.get_position_data(&self.user_id, symbol)
    }

    /// 获取所有持仓
    pub async fn get_positions(&self) -> Result<HashMap<String, Position>> {
        let data = self.dm.get_by_path(&["trade", &self.user_id, "positions"]);
        if data.is_none() {
            return Ok(HashMap::new());
        }

        let positions_map = match data.unwrap() {
            serde_json::Value::Object(map) => map,
            _ => return Ok(HashMap::new()),
        };

        let mut positions = HashMap::new();
        for (symbol, pos_data) in positions_map.iter() {
            // 跳过内部元数据字段（以 _ 开头）
            if symbol.starts_with('_') {
                continue;
            }

            if let Ok(position) = serde_json::from_value::<Position>(pos_data.clone()) {
                positions.insert(symbol.clone(), position);
            } else {
                warn!("持仓数据解析失败: symbol={}", symbol);
            }
        }

        Ok(positions)
    }

    /// 获取所有委托单
    pub async fn get_orders(&self) -> Result<HashMap<String, Order>> {
        let data = self.dm.get_by_path(&["trade", &self.user_id, "orders"]);
        if data.is_none() {
            return Ok(HashMap::new());
        }

        let orders_map = match data.unwrap() {
            serde_json::Value::Object(map) => map,
            _ => return Ok(HashMap::new()),
        };

        let mut orders = HashMap::new();
        for (order_id, order_data) in orders_map.iter() {
            // 跳过内部元数据字段（以 _ 开头）
            if order_id.starts_with('_') {
                continue;
            }

            if let Ok(order) = serde_json::from_value::<Order>(order_data.clone()) {
                orders.insert(order_id.clone(), order);
            } else {
                warn!("委托数据解析失败: order_id={}", order_id);
            }
        }

        Ok(orders)
    }

    /// 获取所有成交记录
    pub async fn get_trades(&self) -> Result<HashMap<String, Trade>> {
        let data = self.dm.get_by_path(&["trade", &self.user_id, "trades"]);
        if data.is_none() {
            return Ok(HashMap::new());
        }

        let trades_map = match data.unwrap() {
            serde_json::Value::Object(map) => map,
            _ => return Ok(HashMap::new()),
        };

        let mut trades = HashMap::new();
        for (trade_id, trade_data) in trades_map.iter() {
            // 跳过内部元数据字段（以 _ 开头）
            if trade_id.starts_with('_') {
                continue;
            }

            if let Ok(trade) = serde_json::from_value::<Trade>(trade_data.clone()) {
                trades.insert(trade_id.clone(), trade);
            } else {
                warn!("成交数据解析失败: trade_id={}", trade_id);
            }
        }

        Ok(trades)
    }

    /// 连接交易服务器
    pub async fn connect(&self) -> Result<()> {
        // 使用 compare_exchange 确保只连接一次
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(()); // 已经在运行
        }

        info!(
            "连接交易服务器: broker={}, user_id={}",
            self.broker, self.user_id
        );

        // 初始化 WebSocket
        if let Err(e) = self.ws.init(false).await {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }

        // 先启动数据监听（注册数据更新回调）
        self.start_watching().await;

        // 再发送登录请求（避免错过初始数据）
        if let Err(e) = self.send_login().await {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }
        if let Err(e) = self.send_confirm_settlement().await {
            self.running.store(false, Ordering::SeqCst);
            return Err(e);
        }

        Ok(())
    }

    /// 检查交易会话是否已就绪
    pub fn is_ready(&self) -> bool {
        // 简化实现：检查是否已登录（原子操作，无需异步）
        // TODO: 添加更详细的就绪检查
        self.logged_in.load(Ordering::SeqCst) && self.ws.is_ready()
    }

    /// 关闭交易会话
    pub async fn close(&self) -> Result<()> {
        // 使用 compare_exchange 确保只关闭一次
        if self
            .running
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(()); // 已经关闭
        }

        info!("关闭交易会话");
        if let Some(id) = *self.data_cb_id.lock().unwrap() {
            let _ = self.dm.off_data(id);
        }
        self.ws.close().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamanager::{DataManager, DataManagerConfig};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    #[tokio::test]
    async fn trade_session_callback_executes_outside_lock() {
        let dm = Arc::new(DataManager::new(
            HashMap::new(),
            DataManagerConfig::default(),
        ));
        dm.merge_data(
            json!({
                "trade": {
                    "u": {
                        "orders": {
                            "o1": {}
                        }
                    }
                }
            }),
            true,
            true,
        );

        let (order_tx, _order_rx) = bounded(8);
        let on_order: OrderCallback = Arc::new(RwLock::new(None));

        {
            let on_order_for_cb = Arc::clone(&on_order);
            *on_order.write().await = Some(Arc::new(move |_order: Order| {
                assert!(on_order_for_cb.try_write().is_ok());
            }));
        }

        TradeSession::process_order_update(&dm, "u", &order_tx, &on_order, -1).await;
    }

    #[tokio::test]
    async fn trade_session_channel_overflow_drops_but_callbacks_still_fire() {
        let dm = Arc::new(DataManager::new(
            HashMap::new(),
            DataManagerConfig::default(),
        ));
        dm.merge_data(
            json!({
                "trade": {
                    "u": {
                        "orders": {
                            "o1": {},
                            "o2": {}
                        }
                    }
                }
            }),
            true,
            true,
        );

        let (order_tx, order_rx) = bounded(1);
        let fired = Arc::new(AtomicUsize::new(0));
        let on_order: OrderCallback = Arc::new(RwLock::new(None));
        {
            let fired = Arc::clone(&fired);
            *on_order.write().await = Some(Arc::new(move |_order: Order| {
                fired.fetch_add(1, AtomicOrdering::SeqCst);
            }));
        }

        TradeSession::process_order_update(&dm, "u", &order_tx, &on_order, -1).await;

        assert_eq!(fired.load(AtomicOrdering::SeqCst), 2);
        assert!(order_rx.try_recv().is_ok());
        assert!(order_rx.try_recv().is_err());
    }
}
