use super::TradeSession;
use crate::errors::{Result, TqError};
use crate::types::{Account, InsertOrderRequest, Order, Position, Trade};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::{info, warn};

impl TradeSession {
    /// 下单（返回委托单号）
    pub async fn insert_order(&self, req: &InsertOrderRequest) -> Result<String> {
        if !self.is_ready() {
            return Err(TqError::InternalError("交易会话未就绪".to_string()));
        }

        let exchange_id = req.get_exchange_id();
        let instrument_id = req.get_instrument_id();

        use uuid::Uuid;
        let order_id = format!(
            "TQRS_{}",
            Uuid::new_v4().simple().to_string()[..8].to_uppercase()
        );

        let time_condition = if req.price_type == "ANY" {
            "IOC"
        } else {
            "GFD"
        };

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
    pub async fn get_account(&self) -> Result<Account> {
        self.dm.get_account_data(&self.user_id, "CNY")
    }

    /// 获取指定合约的持仓
    pub async fn get_position(&self, symbol: &str) -> Result<Position> {
        self.dm.get_position_data(&self.user_id, symbol)
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
        let Some(serde_json::Value::Object(orders_map)) =
            self.dm.get_by_path(&["trade", &self.user_id, "orders"])
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
        let Some(serde_json::Value::Object(trades_map)) =
            self.dm.get_by_path(&["trade", &self.user_id, "trades"])
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

        info!(
            "连接交易服务器: broker={}, user_id={}",
            self.broker, self.user_id
        );

        if let Err(e) = self.ws.init(false).await {
            self.running.store(false, Ordering::SeqCst);
            self.logged_in.store(false, Ordering::SeqCst);
            return Err(e);
        }

        self.start_watching().await;

        if let Err(e) = self.send_login().await {
            self.running.store(false, Ordering::SeqCst);
            self.logged_in.store(false, Ordering::SeqCst);
            self.detach_data_callback();
            let _ = self.ws.close().await;
            return Err(e);
        }
        if let Err(e) = self.send_confirm_settlement().await {
            self.running.store(false, Ordering::SeqCst);
            self.logged_in.store(false, Ordering::SeqCst);
            self.detach_data_callback();
            let _ = self.ws.close().await;
            return Err(e);
        }

        Ok(())
    }

    /// 检查交易会话是否已就绪
    pub fn is_ready(&self) -> bool {
        self.logged_in.load(Ordering::SeqCst) && self.ws.is_ready()
    }

    /// 关闭交易会话
    pub async fn close(&self) -> Result<()> {
        if self
            .running
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.logged_in.store(false, Ordering::SeqCst);
            return Ok(());
        }

        info!("关闭交易会话");
        self.logged_in.store(false, Ordering::SeqCst);
        self.detach_data_callback();
        self.ws.close().await?;
        Ok(())
    }
}
