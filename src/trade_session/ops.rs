use super::TradeSession;
use crate::errors::{Result, TqError};
use crate::types::{Account, InsertOrderOptions, InsertOrderRequest, Order, PRICE_TYPE_LIMIT, Position, Trade};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::{info, warn};
use uuid::Uuid;

pub(crate) fn build_insert_order_packet(
    user_id: &str,
    req: &InsertOrderRequest,
    options: &InsertOrderOptions,
) -> Result<(String, serde_json::Value)> {
    let order_id = options
        .order_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("TQRS_{}", Uuid::new_v4().simple().to_string()[..8].to_uppercase()));
    let exchange_id = req.get_exchange_id();
    let instrument_id = req.get_instrument_id();
    let time_condition = options.resolved_time_condition(&req.price_type);
    let volume_condition = options.resolved_volume_condition();

    let mut order_req = serde_json::Map::from_iter([
        ("aid".to_string(), serde_json::json!("insert_order")),
        ("user_id".to_string(), serde_json::json!(user_id)),
        ("order_id".to_string(), serde_json::json!(order_id)),
        ("exchange_id".to_string(), serde_json::json!(exchange_id)),
        ("instrument_id".to_string(), serde_json::json!(instrument_id)),
        ("direction".to_string(), serde_json::json!(req.direction)),
        ("offset".to_string(), serde_json::json!(req.offset)),
        ("volume".to_string(), serde_json::json!(req.volume)),
        ("price_type".to_string(), serde_json::json!(req.price_type)),
        ("volume_condition".to_string(), serde_json::json!(volume_condition)),
        ("time_condition".to_string(), serde_json::json!(time_condition)),
    ]);
    if req.price_type == PRICE_TYPE_LIMIT {
        order_req.insert("limit_price".to_string(), serde_json::json!(req.limit_price));
    }
    Ok((order_id, serde_json::Value::Object(order_req)))
}

impl TradeSession {
    async fn cleanup_after_disconnect(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.logged_in.store(false, Ordering::SeqCst);
        self.close_snapshot_epoch();
        self.stop_watch_task();
        self.reset_trade_events();
        let _ = self.ws.close().await;
    }

    /// 下单（返回委托单号）
    pub async fn insert_order(&self, req: &InsertOrderRequest) -> Result<String> {
        self.insert_order_with_options(req, &InsertOrderOptions::default())
            .await
    }

    /// 下单，并允许覆盖成交时效/数量条件等扩展语义
    pub async fn insert_order_with_options(
        &self,
        req: &InsertOrderRequest,
        options: &InsertOrderOptions,
    ) -> Result<String> {
        if !self.is_ready() {
            return Err(TqError::InternalError("交易会话未就绪".to_string()));
        }

        let (order_id, order_req) = build_insert_order_packet(&self.user_id, req, options)?;

        info!("发送下单请求: order_id={}, symbol={}", order_id, req.symbol);
        self.ws.send_critical(&order_req).await?;
        Ok(order_id)
    }

    /// 撤单
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        if !self.is_ready() {
            return Err(TqError::InternalError("交易会话未就绪".to_string()));
        }

        let cancel_req = serde_json::json!({
            "aid": "cancel_order",
            "user_id": self.user_id,
            "order_id": order_id
        });

        info!("发送撤单请求: order_id={}", order_id);
        self.ws.send_critical(&cancel_req).await?;
        Ok(())
    }

    /// 获取账户信息
    ///
    /// 最新账户/持仓状态应通过快照 getter 或回调消费，而不是 best-effort channel。
    ///
    /// ```compile_fail
    /// fn removed_snapshot_channels(session: &tqsdk_rs::TradeSession) {
    ///     let _ = session.account_channel();
    ///     let _ = session.position_channel();
    /// }
    /// ```
    pub async fn get_account(&self) -> Result<Account> {
        self.dm.get_account_data(&self.user_id, "CNY")
    }

    /// 获取指定合约的持仓
    pub async fn get_position(&self, symbol: &str) -> Result<Position> {
        self.dm.get_position_data(&self.user_id, symbol)
    }

    /// 获取指定委托单
    pub async fn get_order(&self, order_id: &str) -> Result<Order> {
        self.dm.get_order_data(&self.user_id, order_id)
    }

    /// 获取指定成交记录
    pub async fn get_trade(&self, trade_id: &str) -> Result<Trade> {
        self.dm.get_trade_data(&self.user_id, trade_id)
    }

    /// 获取所有持仓
    pub async fn get_positions(&self) -> Result<HashMap<String, Position>> {
        let Some(serde_json::Value::Object(positions_map)) =
            self.dm.get_by_path(&["trade", &self.user_id, "positions"])
        else {
            return Ok(HashMap::new());
        };

        let mut positions = HashMap::new();
        for (symbol, pos_data) in &positions_map {
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
        let Some(serde_json::Value::Object(orders_map)) = self.dm.get_by_path(&["trade", &self.user_id, "orders"])
        else {
            return Ok(HashMap::new());
        };

        let mut orders = HashMap::new();
        for (order_id, order_data) in &orders_map {
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
        let Some(serde_json::Value::Object(trades_map)) = self.dm.get_by_path(&["trade", &self.user_id, "trades"])
        else {
            return Ok(HashMap::new());
        };

        let mut trades = HashMap::new();
        for (trade_id, trade_data) in &trades_map {
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
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }

        self.logged_in.store(false, Ordering::SeqCst);
        self.reset_snapshot_epoch();

        info!("连接交易服务器: broker={}, user_id={}", self.broker, self.user_id);

        if let Err(e) = self.ws.init(false).await {
            self.cleanup_after_disconnect().await;
            return Err(e);
        }

        self.start_watching().await;

        if let Err(e) = self.send_login().await {
            self.cleanup_after_disconnect().await;
            return Err(e);
        }
        if let Err(e) = self.send_confirm_settlement().await {
            self.cleanup_after_disconnect().await;
            return Err(e);
        }

        Ok(())
    }

    /// 检查交易会话是否已就绪
    pub fn is_ready(&self) -> bool {
        self.logged_in.load(Ordering::SeqCst) && self.snapshot_ready.load(Ordering::SeqCst) && self.ws.is_ready()
    }

    /// 关闭交易会话
    pub async fn close(&self) -> Result<()> {
        let was_running = self
            .running
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();

        if was_running {
            info!("关闭交易会话");
        }

        self.logged_in.store(false, Ordering::SeqCst);
        self.close_snapshot_epoch();
        self.stop_watch_task();
        self.reset_trade_events();
        self.ws.close().await?;
        Ok(())
    }
}
