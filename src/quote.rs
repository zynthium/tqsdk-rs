//! Quote 订阅模块
//!
//! 实现行情订阅功能

use crate::datamanager::DataManager;
use crate::errors::Result;
use crate::types::Quote;
use crate::websocket::TqQuoteWebsocket;
use async_channel::{Receiver, Sender, unbounded};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

type QuoteCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<Quote>) + Send + Sync>>>>;
type QuoteErrorCallback = Arc<RwLock<Option<Arc<dyn Fn(Arc<String>) + Send + Sync>>>>;

/// Quote 订阅
pub struct QuoteSubscription {
    id: String,
    dm: Arc<DataManager>,
    ws: Arc<TqQuoteWebsocket>,
    symbols: Arc<RwLock<HashSet<String>>>,
    quote_tx: Sender<Quote>,
    quote_rx: Receiver<Quote>,
    on_quote: QuoteCallback,
    on_error: QuoteErrorCallback,
    running: Arc<RwLock<bool>>,
    data_cb_id: Arc<std::sync::Mutex<Option<i64>>>,
}

impl QuoteSubscription {
    /// 创建新的 Quote 订阅
    pub fn new(
        dm: Arc<DataManager>,
        ws: Arc<TqQuoteWebsocket>,
        initial_symbols: Vec<String>,
    ) -> Self {
        let symbols: HashSet<String> = initial_symbols.into_iter().collect();

        // 创建 async-channel（使用 unbounded）
        let (quote_tx, quote_rx) = unbounded();

        QuoteSubscription {
            id: Uuid::new_v4().to_string(),
            dm,
            ws,
            symbols: Arc::new(RwLock::new(symbols)),
            quote_tx,
            quote_rx,
            on_quote: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            running: Arc::new(RwLock::new(false)),
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

        // 先启动监听（注册数据更新回调）
        self.start_watching().await;

        // 再发送订阅请求（避免错过初始数据）
        if let Err(e) = self.send_subscription().await {
            *self.running.write().await = false;
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
    pub fn quote_channel(&self) -> Receiver<Quote> {
        self.quote_rx.clone()
    }

    /// 注册回调
    pub async fn on_quote<F>(&self, handler: F)
    where
        F: Fn(Arc<Quote>) + Send + Sync + 'static,
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

    /// 启动监听
    async fn start_watching(&self) {
        let dm_clone = Arc::clone(&self.dm);
        let symbols = Arc::clone(&self.symbols);
        let quote_tx = self.quote_tx.clone();
        let on_quote = Arc::clone(&self.on_quote);
        let running = Arc::clone(&self.running);
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_dirty = Arc::new(AtomicBool::new(false));
        let last_processed_epoch = Arc::new(std::sync::Mutex::new(0i64));

        info!("QuoteSubscription 开始监听数据更新");

        // 注册数据更新回调
        let dm_for_callback = Arc::clone(&dm_clone);
        let cb_id = dm_clone.on_data_register(move || {
            worker_dirty.store(true, Ordering::SeqCst);
            if worker_running.swap(true, Ordering::SeqCst) {
                return;
            }
            let dm = Arc::clone(&dm_for_callback);
            let symbols = Arc::clone(&symbols);
            let quote_tx = quote_tx.clone();
            let on_quote = Arc::clone(&on_quote);
            let running = Arc::clone(&running);
            let worker_running = Arc::clone(&worker_running);
            let worker_dirty = Arc::clone(&worker_dirty);
            let last_processed_epoch = Arc::clone(&last_processed_epoch);

            tokio::spawn(async move {
                loop {
                    worker_dirty.store(false, Ordering::SeqCst);
                    let is_running = *running.read().await;
                    if is_running {
                        let symbol_list: Vec<String> = {
                            let s = symbols.read().await;
                            s.iter().cloned().collect()
                        };

                        let current_global_epoch = dm.get_epoch();
                        let last_epoch = { *last_processed_epoch.lock().unwrap() };

                        for symbol in symbol_list {
                            let path: Vec<&str> = vec!["quotes", &symbol];
                            let path_epoch = dm.get_path_epoch(&path);
                            if path_epoch > last_epoch {
                                match dm.get_quote_data(&symbol) {
                                    Ok(quote) => {
                                        debug!(
                                            "获取到 Quote 更新: symbol={}, last_price={}",
                                            symbol, quote.last_price
                                        );
                                        let quote_arc = Arc::new(quote);
                                        let _ = quote_tx.send((*quote_arc).clone()).await;
                                        if let Some(callback) = on_quote.read().await.as_ref() {
                                            callback(Arc::clone(&quote_arc));
                                        }
                                    }
                                    Err(e) => {
                                        warn!("获取 Quote 失败: symbol={}, error={}", symbol, e);
                                    }
                                }
                            }
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

    /// 关闭订阅
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        if let Some(id) = *self.data_cb_id.lock().unwrap() {
            let _ = self.dm.off_data(id);
        }

        info!("关闭 Quote 订阅");
        self.ws.remove_quote_subscription(&self.id).await
    }
}

impl Drop for QuoteSubscription {
    fn drop(&mut self) {
        info!("销毁 Quote 订阅: id={}", self.id);
        if let Some(id) = *self.data_cb_id.lock().unwrap() {
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
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(async move {
                        let _ = ws.remove_quote_subscription(&id).await;
                    });
                }
            });
        }
    }
}
