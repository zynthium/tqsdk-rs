use super::{SeriesStreamSubscribers, SeriesSubscription};
use super::processing::process_series_update;
use crate::errors::Result;
use crate::types::{ChartInfo, SeriesData, UpdateInfo};
use async_stream::stream;
use chrono::{DateTime, Utc};
use futures::Stream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::error::TrySendError;
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

        Ok(Self {
            dm,
            ws,
            options,
            last_ids: Arc::new(tokio::sync::RwLock::new(last_ids)),
            last_left_id: Arc::new(tokio::sync::RwLock::new(-1)),
            last_right_id: Arc::new(tokio::sync::RwLock::new(-1)),
            chart_ready: Arc::new(tokio::sync::RwLock::new(false)),
            has_chart_sync: Arc::new(tokio::sync::RwLock::new(false)),
            on_update: Arc::new(tokio::sync::RwLock::new(None)),
            on_new_bar: Arc::new(tokio::sync::RwLock::new(None)),
            on_bar_update: Arc::new(tokio::sync::RwLock::new(None)),
            on_error: Arc::new(tokio::sync::RwLock::new(None)),
            stream_subscribers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            running: Arc::new(tokio::sync::RwLock::new(false)),
            unsubscribe_sent: Arc::new(AtomicBool::new(false)),
            data_cb_id: Arc::new(std::sync::Mutex::new(None)),
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

        debug!(
            "启动 Series 订阅: {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );

        self.start_watching().await;
        if let Err(e) = self.send_set_chart().await {
            *self.running.write().await = false;
            self.detach_data_callback();
            return Err(e);
        }
        debug!(
            "send_set_chart done for {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );
        Ok(())
    }

    fn detach_data_callback(&self) {
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
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
        self.send_set_chart().await
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
    async fn start_watching(&self) {
        let dm_clone = Arc::clone(&self.dm);
        let ws = Arc::clone(&self.ws);
        let options = self.options.clone();
        let last_ids = Arc::clone(&self.last_ids);
        let last_left_id = Arc::clone(&self.last_left_id);
        let last_right_id = Arc::clone(&self.last_right_id);
        let chart_ready = Arc::clone(&self.chart_ready);
        let has_chart_sync = Arc::clone(&self.has_chart_sync);

        let on_update = Arc::clone(&self.on_update);
        let on_new_bar = Arc::clone(&self.on_new_bar);
        let on_bar_update = Arc::clone(&self.on_bar_update);
        let on_error = Arc::clone(&self.on_error);
        let stream_subscribers = Arc::clone(&self.stream_subscribers);
        let running = Arc::clone(&self.running);
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_dirty = Arc::new(AtomicBool::new(false));
        let view_width_adjusted = Arc::new(AtomicBool::new(false));
        let last_processed_epoch = Arc::new(std::sync::Mutex::new(0i64));

        let dm_for_callback = Arc::clone(&dm_clone);
        let cb_id = dm_clone.on_data_register(move || {
            let chart_id = options.chart_id.as_deref().unwrap_or("");
            let duration_str = options.duration.to_string();

            let last_epoch = *last_processed_epoch.lock().unwrap();
            let chart_epoch = dm_for_callback.get_path_epoch(&["charts", chart_id]);
            let data_epoch = if options.duration == 0 {
                let symbol = &options.symbols[0];
                dm_for_callback.get_path_epoch(&["ticks", symbol])
            } else if options.symbols.len() > 1 {
                options
                    .symbols
                    .iter()
                    .map(|symbol| {
                        dm_for_callback.get_path_epoch(&["klines", symbol, &duration_str])
                    })
                    .max()
                    .unwrap_or(0)
            } else {
                let symbol = &options.symbols[0];
                dm_for_callback.get_path_epoch(&["klines", symbol, &duration_str])
            };

            if chart_epoch <= last_epoch && data_epoch <= last_epoch {
                return;
            }

            worker_dirty.store(true, Ordering::SeqCst);
            if worker_running.swap(true, Ordering::SeqCst) {
                return;
            }
            let dm = Arc::clone(&dm_for_callback);
            let options = options.clone();
            let last_ids = Arc::clone(&last_ids);
            let last_left_id = Arc::clone(&last_left_id);
            let last_right_id = Arc::clone(&last_right_id);
            let chart_ready = Arc::clone(&chart_ready);
            let has_chart_sync = Arc::clone(&has_chart_sync);
            let on_update = Arc::clone(&on_update);
            let on_new_bar = Arc::clone(&on_new_bar);
            let on_bar_update = Arc::clone(&on_bar_update);
            let on_error = Arc::clone(&on_error);
            let stream_subscribers = Arc::clone(&stream_subscribers);
            let running = Arc::clone(&running);
            let worker_running = Arc::clone(&worker_running);
            let worker_dirty = Arc::clone(&worker_dirty);
            let ws = Arc::clone(&ws);
            let view_width_adjusted = Arc::clone(&view_width_adjusted);
            let last_processed_epoch = Arc::clone(&last_processed_epoch);

            tokio::spawn(async move {
                loop {
                    worker_dirty.store(false, Ordering::SeqCst);
                    let is_running = *running.read().await;
                    if is_running {
                        let chart_id = options.chart_id.as_deref().unwrap_or("");
                        let duration_str = options.duration.to_string();

                        let current_global_epoch = dm.get_epoch();
                        let last_epoch = *last_processed_epoch.lock().unwrap();

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

                        if chart_epoch > last_epoch || data_epoch > last_epoch {
                            if let Some(chart_data) = dm.get_by_path(&["charts", chart_id])
                                && let Ok(chart_info) =
                                    dm.convert_to_struct::<ChartInfo>(&chart_data)
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
                                    last_epoch,
                                )
                                .await
                                {
                                    Ok((series_data, update_info)) => {
                                        if update_info.chart_ready {
                                            let series_data = Arc::new(series_data);
                                            let update_info = Arc::new(update_info);

                                            dispatch_update(
                                                &on_update,
                                                &stream_subscribers,
                                                &series_data,
                                                &update_info,
                                            )
                                            .await;

                                            if update_info.has_new_bar
                                                && let Some(callback) =
                                                    on_new_bar.read().await.as_ref()
                                            {
                                                callback(Arc::clone(&series_data));
                                            }

                                            if update_info.has_bar_update
                                                && let Some(callback) =
                                                    on_bar_update.read().await.as_ref()
                                            {
                                                callback(Arc::clone(&series_data));
                                            }

                                            let use_multi_init_view_width = options.duration != 0
                                                && options.symbols.len() > 1
                                                && options.left_kline_id.is_none()
                                                && options.focus_datetime.is_none()
                                                && options.focus_position.is_none()
                                                && options.view_width < 10000;
                                            if use_multi_init_view_width
                                                && !view_width_adjusted.swap(true, Ordering::SeqCst)
                                            {
                                                let chart_req = build_set_chart_request(
                                                    &options,
                                                    options.view_width,
                                                );
                                                let _ = ws.send(&chart_req).await;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let err_str = e.to_string();
                                        if !err_str.contains("数据未更新") {
                                            warn!("处理 Series 更新失败: {}", e);
                                            if let Some(callback) = on_error.read().await.as_ref() {
                                                callback(Arc::new(err_str));
                                            }
                                        }
                                    }
                                }
                            }
                            *last_processed_epoch.lock().unwrap() = current_global_epoch;
                        }
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

    /// 注册更新回调。
    pub async fn on_update<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>, Arc<UpdateInfo>) + Send + Sync + 'static,
    {
        let mut guard = self.on_update.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册新 Bar 回调。
    pub async fn on_new_bar<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>) + Send + Sync + 'static,
    {
        let mut guard = self.on_new_bar.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册 Bar 更新回调。
    pub async fn on_bar_update<F>(&self, handler: F)
    where
        F: Fn(Arc<SeriesData>) + Send + Sync + 'static,
    {
        let mut guard = self.on_bar_update.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// 注册错误回调。
    pub async fn on_error<F>(&self, handler: F)
    where
        F: Fn(Arc<String>) + Send + Sync + 'static,
    {
        let mut guard = self.on_error.write().await;
        *guard = Some(Arc::new(handler));
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) async fn emit_update(
        &self,
        series_data: Arc<SeriesData>,
        update_info: Arc<UpdateInfo>,
    ) {
        dispatch_update(
            &self.on_update,
            &self.stream_subscribers,
            &series_data,
            &update_info,
        )
        .await;
    }

    /// 获取异步数据流。
    pub async fn data_stream(&self) -> impl Stream<Item = Arc<SeriesData>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(self.ws.message_queue_capacity());
        {
            let mut subscribers = self.stream_subscribers.write().await;
            subscribers.retain(|sender| !sender.is_closed());
            subscribers.push(tx);
        }

        stream! {
            while let Some(data) = rx.recv().await {
                yield data;
            }
        }
    }

    /// 关闭订阅并向服务端发送取消请求。
    pub async fn close(&self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            *running = false;
        }
        self.detach_data_callback();

        info!(
            "关闭 Series 订阅: {}",
            self.options.chart_id.as_deref().unwrap_or("")
        );

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
        if let Some(id) = self.data_cb_id.lock().unwrap().take() {
            let _ = self.dm.off_data(id);
        }
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
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(async move {
                        let _ = ws.send(&cancel_req).await;
                    });
                }
            });
        }
    }
}

async fn dispatch_stream_subscribers(
    stream_subscribers: &SeriesStreamSubscribers,
    series_data: &Arc<SeriesData>,
) {
    let subscribers = stream_subscribers.read().await.clone();
    let mut has_closed = false;

    for sender in subscribers {
        match sender.try_send(Arc::clone(series_data)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!("Series 数据流通道已满，丢弃一次更新");
            }
            Err(TrySendError::Closed(_)) => {
                has_closed = true;
            }
        }
    }

    if has_closed {
        let mut subscribers = stream_subscribers.write().await;
        subscribers.retain(|sender| !sender.is_closed());
    }
}

async fn dispatch_update(
    on_update: &super::UpdateCallback,
    stream_subscribers: &SeriesStreamSubscribers,
    series_data: &Arc<SeriesData>,
    update_info: &Arc<UpdateInfo>,
) {
    if let Some(callback) = on_update.read().await.as_ref() {
        callback(Arc::clone(series_data), Arc::clone(update_info));
    }
    dispatch_stream_subscribers(stream_subscribers, series_data).await;
}

fn build_set_chart_request(
    options: &crate::types::SeriesOptions,
    view_width: usize,
) -> serde_json::Value {
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
    } else if let (Some(focus_datetime), Some(focus_position)) =
        (options.focus_datetime, options.focus_position)
    {
        chart_req["focus_datetime"] = serde_json::json!(focus_datetime.timestamp_nanos_opt());
        chart_req["focus_position"] = serde_json::json!(focus_position);
    }

    chart_req
}
