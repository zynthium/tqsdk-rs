use super::InsAPI;
use crate::errors::{Result, TqError};
use crate::types::TradingStatus;
use async_channel::{Receiver, unbounded};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

impl InsAPI {
    /// 订阅合约交易状态
    ///
    /// 需要账号具备交易状态权限，返回异步接收器。
    pub async fn get_trading_status(&self, symbol: &str) -> Result<Receiver<TradingStatus>> {
        if symbol.is_empty() {
            return Err(TqError::InvalidParameter(
                "get_trading_status 中合约代码不能为空字符串".to_string(),
            ));
        }
        if !self.auth.read().await.has_feature("tq_trading_status") {
            return Err(TqError::PermissionDenied(
                "账户不支持查看交易状态信息，需要购买后才能使用。升级网址：https://www.shinnytech.com/tqsdk-buy/".to_string(),
            ));
        }
        let ws = self
            .trading_status_ws
            .as_ref()
            .ok_or_else(|| TqError::InternalError("交易状态服务未初始化".to_string()))?;

        let mut subscribe_symbol = symbol.to_string();
        let mut option_mapping: HashMap<String, String> = HashMap::new();
        let mut need_query = true;
        if let Some(quote) = self.dm.get_by_path(&["quotes", symbol])
            && let Some(obj) = quote.as_object()
            && let Some(class_str) = obj.get("class").and_then(|v| v.as_str())
        {
            need_query = false;
            if class_str == "OPTION" {
                if let Some(underlying) = obj.get("underlying_symbol").and_then(|v| v.as_str()) {
                    if !underlying.is_empty() {
                        subscribe_symbol = underlying.to_string();
                        option_mapping.insert(symbol.to_string(), subscribe_symbol.clone());
                    } else {
                        need_query = true;
                    }
                } else {
                    need_query = true;
                }
            }
        }
        if need_query
            && let Ok(info_list) = self.query_symbol_info(&[symbol]).await
            && let Some(Value::Object(info)) = info_list.first()
            && let Some(Value::String(class_str)) = info.get("ins_class")
            && class_str == "OPTION"
            && let Some(Value::String(underlying)) = info.get("underlying_symbol")
            && !underlying.is_empty()
        {
            subscribe_symbol = underlying.clone();
            option_mapping.insert(symbol.to_string(), subscribe_symbol.clone());
        }
        if !option_mapping.is_empty() {
            ws.update_option_underlyings(option_mapping);
        }

        {
            let mut guard = self.trading_status_symbols.write().await;
            guard.insert(subscribe_symbol.clone());
            let ins_list = guard.iter().cloned().collect::<Vec<String>>().join(",");
            let req = json!({
                "aid": "subscribe_trading_status",
                "ins_list": ins_list
            });
            ws.send(&req).await?;
        }

        let (tx, rx) = unbounded();
        let dm = Arc::clone(&self.dm);
        let symbol_string = symbol.to_string();
        let watch_path = vec!["trading_status".to_string(), symbol_string.clone()];
        let (watch_id, watch) = dm.watch_register(watch_path.clone());
        tokio::spawn(async move {
            if let Some(val) = dm.get_by_path(&["trading_status", &symbol_string])
                && let Ok(status) = dm.convert_to_struct::<TradingStatus>(&val)
                && tx.send(status).await.is_err()
            {
                let _ = dm.unwatch_by_id(&watch_path, watch_id);
                return;
            }
            loop {
                if watch.recv().await.is_err() {
                    break;
                }
                if let Some(val) = dm.get_by_path(&["trading_status", &symbol_string])
                    && let Ok(status) = dm.convert_to_struct::<TradingStatus>(&val)
                    && tx.send(status).await.is_err()
                {
                    break;
                }
            }
            let _ = dm.unwatch_by_id(&watch_path, watch_id);
        });

        Ok(rx)
    }
}
