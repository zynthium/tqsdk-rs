use super::SeriesSubscription;
use super::processing::process_series_update;
use crate::errors::{Result, TqError};
use crate::types::{ChartInfo, SeriesData, SeriesSnapshot};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

impl SeriesSubscription {
    /// 创建订阅
    pub(crate) fn new(
        dm: Arc<crate::datamanager::DataManager>,
        ws: Arc<crate::websocket::TqQuoteWebsocket>,
        options: crate::types::SeriesOptions,
    ) -> Result<Self> {
        let mut last_ids = std::collections::HashMap::new();
        for symbol in &options.symbols {
            last_ids.insert(symbol.clone(), -1);
        }
        let (snapshot_tx, snapshot_rx) = tokio::sync::watch::channel(None);

        Ok(Self {
            dm,
            ws,
            options,
            last_ids: Arc::new(tokio::sync::RwLock::new(last_ids)),
            last_left_id: Arc::new(tokio::sync::RwLock::new(-1)),
            last_right_id: Arc::new(tokio::sync::RwLock::new(-1)),
            chart_ready: Arc::new(tokio::sync::RwLock::new(false)),
            has_chart_sync: Arc::new(tokio::sync::RwLock::new(false)),
            running: Arc::new(tokio::sync::RwLock::new(false)),
            unsubscribe_sent: Arc::new(AtomicBool::new(false)),
            snapshot_tx,
            wait_rx: Arc::new(tokio::sync::Mutex::new(snapshot_rx)),
            latest_snapshot: Arc::new(tokio::sync::RwLock::new(None)),
            watch_task: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// 发送 set_chart 请求
    async fn send_set_chart(&self) -> Result<()> {
        let mut view_width = self.options.view_width.min(10000);
        if view_width == 0 {
            view_width = 1;
        }
        let use_multi_init_view_width = self.options.duration != 0
            && self.options.symbols.len() > 1
            && self.options.left_kline_id.is_none()
            && self.options.focus_datetime.is_none()
            && self.options.focus_position.is_none();
        if use_multi_init_view_width {
            let ready = *self.chart_ready.read().await;
            if !ready {
                view_width = 10000;
            }
        }

        let chart_req = build_set_chart_request(&self.options, view_width);
        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        debug!(
            "发送 set_chart 请求: chart_id={}, symbols={:?}, view_width={}, duration={}",
            chart_id, self.options.symbols, view_width, self.options.duration
        );

        self.ws.send(&chart_req).await?;
        Ok(())
    }

    /// 在已存在的图表订阅上更新历史焦点位置。
    pub async fn update_focus(&self, focus_time: DateTime<Utc>, focus_position: i32) -> Result<()> {
        let view_width = if self.options.view_width > 10000 {
            10000
        } else {
            self.options.view_width
        };
        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        let chart_req = serde_json::json!({
            "aid": "set_chart",
            "chart_id": chart_id,
            "ins_list": self.options.symbols.join(","),
            "duration": self.options.duration,
            "view_width": view_width,
            "focus_datetime": focus_time.timestamp_nanos_opt(),
            "focus_position": focus_position
        });
        self.ws.send(&chart_req).await?;
        Ok(())
    }

    /// 启动订阅。
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);
        self.unsubscribe_sent.store(false, Ordering::SeqCst);

        debug!("启动 Series 订阅: {}", self.options.chart_id.as_deref().unwrap_or(""));

        self.start_watching();
        if let Err(e) = self.send_set_chart().await {
            *self.running.write().await = false;
            self.abort_watch_task();
            return Err(e);
        }
        debug!(
            "send_set_chart done for {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );
        Ok(())
    }

    fn abort_watch_task(&self) {
        if let Some(handle) = self.watch_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    async fn clear_snapshot_state(&self) {
        *self.latest_snapshot.write().await = None;
        let _ = self.snapshot_tx.send_replace(None);
    }

    /// 刷新订阅状态并重新发起 `set_chart` 请求。
    pub async fn refresh(&self) -> Result<()> {
        {
            let mut ids = self.last_ids.write().await;
            for value in ids.values_mut() {
                *value = -1;
            }
        }
        *self.last_left_id.write().await = -1;
        *self.last_right_id.write().await = -1;
        *self.chart_ready.write().await = false;
        *self.has_chart_sync.write().await = false;
        *self.running.write().await = true;
        self.unsubscribe_sent.store(false, Ordering::SeqCst);
        self.clear_snapshot_state().await;
        self.start_watching();
        if let Err(e) = self.send_set_chart().await {
            *self.running.write().await = false;
            self.abort_watch_task();
            return Err(e);
        }
        Ok(())
    }

    fn build_cancel_chart_request(&self) -> serde_json::Value {
        let view_width = if self.options.view_width == 0 {
            1
        } else if self.options.view_width > 10000 {
            10000
        } else {
            self.options.view_width
        };
        let chart_id = self.options.chart_id.as_deref().unwrap_or("");
        serde_json::json!({
            "aid": "set_chart",
            "chart_id": chart_id,
            "ins_list": "",
            "duration": self.options.duration,
            "view_width": view_width
        })
    }

    /// 启动监听数据更新
    fn start_watching(&self) {
        let mut guard = self.watch_task.lock().unwrap();
        if guard.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }

        let dm = Arc::clone(&self.dm);
        let ws = Arc::clone(&self.ws);
        let options = self.options.clone();
        let last_ids = Arc::clone(&self.last_ids);
        let last_left_id = Arc::clone(&self.last_left_id);
        let last_right_id = Arc::clone(&self.last_right_id);
        let chart_ready = Arc::clone(&self.chart_ready);
        let has_chart_sync = Arc::clone(&self.has_chart_sync);
        let running = Arc::clone(&self.running);
        let snapshot_tx = self.snapshot_tx.clone();
        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let mut epoch_rx = dm.subscribe_epoch();

        info!("Series 开始监听数据更新");

        *guard = Some(tokio::spawn(async move {
            let view_width_adjusted = Arc::new(AtomicBool::new(false));
            let mut last_processed_epoch = 0i64;

            loop {
                if !*running.read().await {
                    break;
                }

                let current_global_epoch = *epoch_rx.borrow_and_update();
                if current_global_epoch <= last_processed_epoch {
                    if epoch_rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }

                let chart_id = options.chart_id.as_deref().unwrap_or("");
                let duration_str = options.duration.to_string();
                let chart_epoch = dm.get_path_epoch(&["charts", chart_id]);
                let data_epoch = if options.duration == 0 {
                    let symbol = &options.symbols[0];
                    dm.get_path_epoch(&["ticks", symbol])
                } else if options.symbols.len() > 1 {
                    options
                        .symbols
                        .iter()
                        .map(|symbol| dm.get_path_epoch(&["klines", symbol, &duration_str]))
                        .max()
                        .unwrap_or(0)
                } else {
                    let symbol = &options.symbols[0];
                    dm.get_path_epoch(&["klines", symbol, &duration_str])
                };

                if (chart_epoch > last_processed_epoch || data_epoch > last_processed_epoch)
                    && let Some(chart_data) = dm.get_by_path(&["charts", chart_id])
                    && let Ok(chart_info) = dm.convert_to_struct::<ChartInfo>(&chart_data)
                    && chart_info.ready
                    && !chart_info.more_data
                {
                    match process_series_update(
                        &dm,
                        &options,
                        &last_ids,
                        &last_left_id,
                        &last_right_id,
                        &chart_ready,
                        &has_chart_sync,
                        last_processed_epoch,
                    )
                    .await
                    {
                        Ok((series_data, update_info)) => {
                            if update_info.chart_ready {
                                let snapshot = SeriesSnapshot {
                                    data: Arc::new(series_data),
                                    update: Arc::new(update_info),
                                    epoch: current_global_epoch,
                                };

                                *latest_snapshot.write().await = Some(snapshot.clone());
                                let _ = snapshot_tx.send_replace(Some(snapshot));

                                let use_multi_init_view_width = options.duration != 0
                                    && options.symbols.len() > 1
                                    && options.left_kline_id.is_none()
                                    && options.focus_datetime.is_none()
                                    && options.focus_position.is_none()
                                    && options.view_width < 10000;
                                if use_multi_init_view_width && !view_width_adjusted.swap(true, Ordering::SeqCst) {
                                    let chart_req = build_set_chart_request(&options, options.view_width);
                                    let _ = ws.send(&chart_req).await;
                                }
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if !err_str.contains("数据未更新") {
                                warn!("处理 Series 更新失败: {}", e);
                            }
                        }
                    }
                }

                last_processed_epoch = current_global_epoch;
            }
        }));
    }

    /// 等待下一次窗口快照更新。
    pub async fn wait_update(&self) -> Result<SeriesSnapshot> {
        if !*self.running.read().await {
            return Err(TqError::InternalError("Series 订阅尚未启动".to_string()));
        }
        let mut rx = self.wait_rx.lock().await;
        loop {
            if rx.changed().await.is_err() {
                return Err(TqError::InternalError("Series 快照订阅已关闭".to_string()));
            }
            if let Some(snapshot) = rx.borrow().clone() {
                return Ok(snapshot);
            }
        }
    }

    /// 获取当前最新快照。
    pub async fn snapshot(&self) -> Option<SeriesSnapshot> {
        self.latest_snapshot.read().await.clone()
    }

    /// 获取当前最新序列数据。
    pub async fn load(&self) -> Result<Arc<SeriesData>> {
        self.snapshot()
            .await
            .map(|snapshot| snapshot.data)
            .ok_or_else(|| TqError::DataNotFound("Series 快照尚未就绪".to_string()))
    }

    #[cfg(test)]
    pub(super) async fn emit_snapshot(&self, snapshot: SeriesSnapshot) {
        *self.latest_snapshot.write().await = Some(snapshot.clone());
        let _ = self.snapshot_tx.send_replace(Some(snapshot));
    }

    #[cfg(test)]
    pub(super) fn watch_task_active_for_test(&self) -> bool {
        self.watch_task
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// 关闭订阅并向服务端发送取消请求。
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        self.abort_watch_task();

        info!("关闭 Series 订阅: {}", self.options.chart_id.as_deref().unwrap_or(""));

        if self.unsubscribe_sent.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let cancel_req = self.build_cancel_chart_request();
        self.ws.send(&cancel_req).await?;
        Ok(())
    }
}

impl Drop for SeriesSubscription {
    fn drop(&mut self) {
        self.abort_watch_task();
        if self.unsubscribe_sent.swap(true, Ordering::SeqCst) {
            return;
        }
        let chart_id = self.options.chart_id.clone().unwrap_or_default();
        info!(
            "销毁 Series 订阅: chart_id={}, symbols={:?}, duration={}",
            chart_id, self.options.symbols, self.options.duration
        );
        let ws = Arc::clone(&self.ws);
        let cancel_req = self.build_cancel_chart_request();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = ws.send(&cancel_req).await;
            });
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    rt.block_on(async move {
                        let _ = ws.send(&cancel_req).await;
                    });
                }
            });
        }
    }
}

fn build_set_chart_request(options: &crate::types::SeriesOptions, view_width: usize) -> serde_json::Value {
    let chart_id = options.chart_id.as_deref().unwrap_or("");
    let mut chart_req = serde_json::json!({
        "aid": "set_chart",
        "chart_id": chart_id,
        "ins_list": options.symbols.join(","),
        "duration": options.duration,
        "view_width": view_width,
    });

    if let Some(left_kline_id) = options.left_kline_id {
        chart_req["left_kline_id"] = serde_json::json!(left_kline_id);
    } else if let (Some(focus_datetime), Some(focus_position)) = (options.focus_datetime, options.focus_position) {
        chart_req["focus_datetime"] = serde_json::json!(focus_datetime.timestamp_nanos_opt());
        chart_req["focus_position"] = serde_json::json!(focus_position);
    }

    chart_req
}
