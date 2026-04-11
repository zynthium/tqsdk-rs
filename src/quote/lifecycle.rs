use super::QuoteSubscription;
use crate::errors::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

impl QuoteSubscription {
    /// 创建新的 Quote 订阅
    pub(crate) async fn new(ws: Arc<crate::websocket::TqQuoteWebsocket>, initial_symbols: Vec<String>) -> Result<Self> {
        let symbols: HashSet<String> = initial_symbols.into_iter().collect();

        let sub = QuoteSubscription {
            id: Uuid::new_v4().to_string(),
            ws,
            symbols: Arc::new(tokio::sync::RwLock::new(symbols)),
            running: Arc::new(tokio::sync::RwLock::new(false)),
        };
        sub.activate().await?;
        Ok(sub)
    }

    async fn activate(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        debug!("启动 Quote 订阅");

        if let Err(e) = self.send_subscription().await {
            *self.running.write().await = false;
            let _ = self.ws.remove_quote_subscription(&self.id).await;
            return Err(e);
        }

        Ok(())
    }

    /// 添加合约
    pub async fn add_symbols(&self, symbols: &[&str]) -> Result<()> {
        if symbols.is_empty() {
            return Ok(());
        }

        let mut symbol_set = self.symbols.write().await;
        for &symbol in symbols {
            symbol_set.insert(symbol.to_string());
        }
        drop(symbol_set);

        if !*self.running.read().await {
            return Ok(());
        }
        self.send_subscription().await
    }

    /// 移除合约
    pub async fn remove_symbols(&self, symbols: &[&str]) -> Result<()> {
        if symbols.is_empty() {
            return Ok(());
        }

        let mut symbol_set = self.symbols.write().await;
        for &symbol in symbols {
            symbol_set.remove(symbol);
        }
        drop(symbol_set);

        if !*self.running.read().await {
            return Ok(());
        }
        self.send_subscription().await
    }

    /// 发送订阅请求
    async fn send_subscription(&self) -> Result<()> {
        let ins_list: HashSet<String> = {
            let symbols = self.symbols.read().await;
            symbols.iter().cloned().collect()
        };
        debug!("同步 Quote 订阅请求: {} 个合约", ins_list.len());
        self.ws.update_quote_subscription(&self.id, ins_list).await
    }

    /// 关闭订阅
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }

        info!("关闭 Quote 订阅");
        self.ws.remove_quote_subscription(&self.id).await
    }
}

impl Drop for QuoteSubscription {
    fn drop(&mut self) {
        info!("销毁 Quote 订阅: id={}", self.id);
        let ws = Arc::clone(&self.ws);
        let id = self.id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = ws.remove_quote_subscription(&id).await;
            });
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    rt.block_on(async move {
                        let _ = ws.remove_quote_subscription(&id).await;
                    });
                }
            });
        }
    }
}
