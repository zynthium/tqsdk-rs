use super::QuoteSubscription;
use async_channel::TrySendError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

impl QuoteSubscription {
    /// 启动监听
    pub(super) async fn start_watching(&self) {
        let dm_clone = Arc::clone(&self.dm);
        let symbols = Arc::clone(&self.symbols);
        let quote_tx = self.quote_tx.clone();
        let on_quote = Arc::clone(&self.on_quote);
        let running = Arc::clone(&self.running);
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_dirty = Arc::new(AtomicBool::new(false));
        let last_processed_epoch = Arc::new(std::sync::Mutex::new(0i64));

        info!("QuoteSubscription 开始监听数据更新");

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
                                        debug!("获取到 Quote 更新: symbol={}, last_price={}", symbol, quote.last_price);
                                        let quote_arc = Arc::new(quote);
                                        match quote_tx.try_send((*quote_arc).clone()) {
                                            Ok(()) => {}
                                            Err(TrySendError::Full(_)) => {
                                                warn!("Quote 通道已满，丢弃一次更新");
                                            }
                                            Err(TrySendError::Closed(_)) => {}
                                        }
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
}
