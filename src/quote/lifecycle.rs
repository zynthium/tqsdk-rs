use super::QuoteSubscription;
use crate::errors::Result;
use async_channel::{Receiver, bounded};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

impl QuoteSubscription {
    /// 创建新的 Quote 订阅
    pub fn new(
        dm: Arc<crate::datamanager::DataManager>,
        ws: Arc<crate::websocket::TqQuoteWebsocket>,
        initial_symbols: Vec<String>,
    ) -> Self {
        let symbols: HashSet<String> = initial_symbols.into_iter().collect();
        let (quote_tx, quote_rx) = bounded(ws.message_queue_capacity());

        QuoteSubscription {
            id: Uuid::new_v4().to_string(),
            dm,
            ws,
            symbols: Arc::new(tokio::sync::RwLock::new(symbols)),
            quote_tx,
            quote_rx,
            on_quote: Arc::new(tokio::sync::RwLock::new(None)),
            on_error: Arc::new(tokio::sync::RwLock::new(None)),
            running: Arc::new(tokio::sync::RwLock::new(false)),
            data_cb_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// 启动订阅监听
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        debug!("启动 Quote 订阅");

        self.start_watching().await;

        if let Err(e) = self.send_subscription().await {
            *self.running.write().await = false;
            self.detach_data_callback();
            return Err(e);
        }

        Ok(())
    }

    pub(super) fn detach_data_callback(&self) {
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
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

    /// 获取 Quote 更新通道（克隆接收端）
    pub fn quote_channel(&self) -> Receiver<crate::types::Quote> {
        self.quote_rx.clone()
    }

    /// 注册回调
    pub async fn on_quote<F>(&self, handler: F)
    where
        F: Fn(Arc<crate::types::Quote>) + Send + Sync + 'static,
    {
        let mut guard = self.on_quote.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册错误回调
    pub async fn on_error<F>(&self, handler: F)
    where
        F: Fn(Arc<String>) + Send + Sync + 'static,
    {
        let mut guard = self.on_error.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 关闭订阅
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        self.detach_data_callback();

        info!("关闭 Quote 订阅");
        self.ws.remove_quote_subscription(&self.id).await
    }
}

impl Drop for QuoteSubscription {
    fn drop(&mut self) {
        info!("销毁 Quote 订阅: id={}", self.id);
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
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
