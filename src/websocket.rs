//! WebSocket 连接封装
//!
//! 基于 yawc 库实现 WebSocket 连接，支持：
//! - deflate 压缩
//! - 自动重连
//! - 消息队列
//! - Debug 日志

use crate::datamanager::DataManager;
use crate::errors::{Result, TqError};
use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use yawc::frame::{FrameView, OpCode};

/// WebSocket 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketStatus {
    /// 连接中
    Connecting,
    /// 已连接
    Open,
    /// 关闭中
    Closing,
    /// 已关闭
    Closed,
}

/// WebSocket 配置
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// HTTP Headers
    pub headers: HeaderMap,
    /// 重连间隔
    pub reconnect_interval: Duration,
    /// 最大重连次数
    pub reconnect_max_times: usize,
    /// 自动发送 peek_message
    pub auto_peek: bool,
    pub auto_peek_interval: Duration,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        WebSocketConfig {
            headers: HeaderMap::new(),
            reconnect_interval: Duration::from_secs(3),
            reconnect_max_times: usize::MAX,
            auto_peek: true,
            auto_peek_interval: Duration::from_secs(15),
        }
    }
}

/// 天勤 WebSocket 基类
///
/// 基于 yawc 库实现 WebSocket 连接
pub struct TqWebsocket {
    url: String,
    config: WebSocketConfig,
    status: Arc<RwLock<WebSocketStatus>>,
    queue: Arc<Mutex<Vec<String>>>,
    reconnect_times: Arc<RwLock<usize>>,
    should_reconnect: Arc<RwLock<bool>>,

    // WebSocket 连接实例
    ws: Arc<Mutex<Option<yawc::WebSocket>>>,

    // 回调函数
    on_message: Arc<RwLock<Option<Box<dyn Fn(Value) + Send + Sync>>>>,
    on_open: Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>,
    on_close: Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>,
    on_error: Arc<RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>>,
}

impl TqWebsocket {
    /// 创建新的 WebSocket 连接
    pub fn new(url: String, config: WebSocketConfig) -> Self {
        TqWebsocket {
            url,
            config,
            status: Arc::new(RwLock::new(WebSocketStatus::Closed)),
            queue: Arc::new(Mutex::new(Vec::new())),
            reconnect_times: Arc::new(RwLock::new(0)),
            should_reconnect: Arc::new(RwLock::new(true)),
            ws: Arc::new(Mutex::new(None)),
            on_message: Arc::new(RwLock::new(None)),
            on_open: Arc::new(RwLock::new(None)),
            on_close: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
        }
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        info!(
            "正在连接 WebSocket: {} (重连: {})",
            self.url, is_reconnection
        );
        *self.status.write().unwrap() = WebSocketStatus::Connecting;

        // 配置 WebSocket 选项，启用 deflate 压缩
        let options = yawc::Options::default()
            .client_no_context_takeover()
            .server_no_context_takeover();

        let parsed_url = url::Url::parse(&self.url)
            .map_err(|e| TqError::WebSocketError(format!("Invalid URL: {}", e)))?;

        // 构建 HTTP 请求头
        let mut http_builder = yawc::HttpRequestBuilder::new();
        for (key, value) in self.config.headers.iter() {
            if let Ok(value_str) = value.to_str() {
                http_builder = http_builder.header(key.as_str(), value_str);
            }
        }

        // 连接 WebSocket
        let ws = match yawc::WebSocket::connect(parsed_url)
            .with_options(options)
            .with_request(http_builder)
            .await
        {
            Ok(ws) => ws,
            Err(e) => {
                error!(url = %self.url, error = %e, "WebSocket 连接失败");

                // 触发错误回调
                if let Some(callback) = self.on_error.read().unwrap().as_ref() {
                    callback(format!("Connection failed: {}", e));
                }

                // 尝试重连
                if *self.should_reconnect.read().unwrap() {
                    let _ = self.handle_reconnect().await;
                }

                return Err(TqError::WebSocketError(format!("Connection failed: {}", e)));
            }
        };

        // 保存连接实例
        {
            let mut ws_guard = self.ws.lock().await;
            *ws_guard = Some(ws);
        }

        *self.status.write().unwrap() = WebSocketStatus::Open;

        // 重置重连次数
        if !is_reconnection {
            *self.reconnect_times.write().unwrap() = 0;
        }

        // 触发 onOpen 回调
        if let Some(callback) = self.on_open.read().unwrap().as_ref() {
            callback();
        }

        // 启动消息接收循环
        self.start_receive_loop().await;
        // 发送队列中的消息
        self.flush_queue().await;
        self.send(&json!({"aid": "peek_message"})).await?;

        info!("WebSocket 连接成功");
        Ok(())
    }

    /// 发送消息
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let json_str = serde_json::to_string(obj)?;

        if self.is_ready() {
            debug!("WebSocket 发送消息: {}", json_str);

            let mut ws_guard = self.ws.lock().await;
            if let Some(ws) = ws_guard.as_mut() {
                // yawc 使用 Sink trait，发送 FrameView
                let frame = FrameView::text(json_str.into_bytes());
                match ws.send(frame).await {
                    Ok(_) => {
                        Ok(())
                    }
                    Err(e) => {
                        error!("消息发送失败: {}", e);
                        Err(TqError::WebSocketError(format!("Send failed: {}", e)))
                    }
                }
            } else {
                debug!("WebSocket 连接不存在，消息加入队列");
                drop(ws_guard);
                self.queue.lock().await.push(json_str);
                Ok(())
            }
        } else {
            debug!("WebSocket 未就绪，消息加入队列: {}", json_str);
            self.queue.lock().await.push(json_str);
            Ok(())
        }
    }

    /// 检查连接是否就绪
    pub fn is_ready(&self) -> bool {
        *self.status.read().unwrap() == WebSocketStatus::Open
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        info!("正在关闭 WebSocket 连接");
        *self.should_reconnect.write().unwrap() = false;

        // 先设置状态为 Closing，这会让接收循环在下次迭代时退出
        *self.status.write().unwrap() = WebSocketStatus::Closing;

        // 关闭 WebSocket 连接
        let mut ws_guard = self.ws.lock().await;
        if let Some(ws) = ws_guard.take() {
            // yawc WebSocket 会在 drop 时自动发送关闭帧并关闭连接
            drop(ws);
            info!("WebSocket 连接已关闭");
        }
        drop(ws_guard);

        // 等待一小段时间让接收循环退出
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        *self.status.write().unwrap() = WebSocketStatus::Closed;

        // 触发 onClose 回调
        if let Some(callback) = self.on_close.read().unwrap().as_ref() {
            callback();
        }

        Ok(())
    }

    /// 发送队列中的消息
    async fn flush_queue(&self) {
        let mut queue = self.queue.lock().await;
        if queue.is_empty() {
            return;
        }

        debug!("发送队列中的 {} 条消息", queue.len());

        let mut ws_guard = self.ws.lock().await;
        if let Some(ws) = ws_guard.as_mut() {
            for msg in queue.drain(..) {
                debug!("发送队列消息: {}", msg);
                let frame = FrameView::text(msg.into_bytes());
                match ws.send(frame).await {
                    Ok(_) => {
                        debug!("队列消息发送成功");
                    }
                    Err(e) => {
                        error!("队列消息发送失败: {}", e);
                    }
                }
            }
        }
    }

    /// 注册消息回调
    pub fn on_message<F>(&self, callback: F)
    where
        F: Fn(Value) + Send + Sync + 'static,
    {
        *self.on_message.write().unwrap() = Some(Box::new(callback));
    }

    /// 注册连接打开回调
    pub fn on_open<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_open.write().unwrap() = Some(Box::new(callback));
    }

    /// 注册连接关闭回调
    pub fn on_close<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_close.write().unwrap() = Some(Box::new(callback));
    }

    /// 注册错误回调
    pub fn on_error<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        *self.on_error.write().unwrap() = Some(Box::new(callback));
    }

    /// 启动消息接收循环
    async fn start_receive_loop(&self) {
        let status = Arc::clone(&self.status);
        let ws = Arc::clone(&self.ws);
        let status_for_peek = Arc::clone(&self.status);
        let ws_for_peek = Arc::clone(&self.ws);
        let _on_message = Arc::clone(&self.on_message);
        let _on_error = Arc::clone(&self.on_error);
        let _on_close = Arc::clone(&self.on_close);
        let _on_close_for_peek = Arc::clone(&self.on_close);
        let _should_reconnect = Arc::clone(&self.should_reconnect);
        let _url = self.url.clone();
        let auto_peek = self.config.auto_peek;
        let auto_peek_interval = self.config.auto_peek_interval;

        tokio::spawn(async move {
            debug!("启动 WebSocket 消息接收循环");

            loop {
                // 检查连接状态
                let current_status = *status.read().unwrap();
                if current_status != WebSocketStatus::Open {
                    debug!("WebSocket 状态不是 Open，退出接收循环");
                    break;
                }

                // 获取 WebSocket 实例
                let mut ws_guard = ws.lock().await;
                if let Some(ws_instance) = ws_guard.as_mut() {
                    // yawc 使用 Stream trait，使用 next() 接收消息
                    // 使用 timeout 避免无限等待
                    let timeout_duration = tokio::time::Duration::from_secs(1);
                    let next_result =
                        tokio::time::timeout(timeout_duration, ws_instance.next()).await;

                    match next_result {
                        Ok(Some(frame)) => {
                            // 处理不同类型的帧
                            match frame.opcode {
                                OpCode::Text | OpCode::Binary => {
                                    // 将 payload 转换为字符串
                                    match String::from_utf8(frame.payload.to_vec()) {
                                        Ok(text) => {
                                            debug!("WebSocket Recv Text: {}", text);

                                            // 解析 JSON
                                            match serde_json::from_str::<Value>(&text) {
                                                Ok(json_value) => {
                                                    // 触发消息回调
                                                    if let Some(callback) =
                                                        _on_message.read().unwrap().as_ref()
                                                    {
                                                        callback(json_value);
                                                    }
                                                }
                                                Err(e) => {
                                                    warn!("解析 JSON 失败: {}", e);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("消息不是有效的 UTF-8: {}", e);
                                        }
                                    }

                                    if auto_peek {
                                        let frame =
                                            FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes());
                                        match ws_instance.send(frame).await {
                                            Ok(_) => {
                                                debug!("Websocket Send -> peek_message");
                                            }
                                            Err(e) => {
                                                error!("Websocket Send `peek_message` failed: {}", e);
                                                break;
                                            }
                                        }
                                    }
                                }
                                OpCode::Close => {
                                    info!("WebSocket 收到关闭帧");
                                    *status.write().unwrap() = WebSocketStatus::Closed;

                                    // 触发关闭回调
                                    if let Some(callback) = _on_close.read().unwrap().as_ref() {
                                        callback();
                                    }

                                    drop(ws_guard);
                                    break;
                                }
                                OpCode::Ping => {
                                    debug!("WebSocket 收到 Ping（自动处理）");
                                }
                                OpCode::Pong => {
                                    debug!("WebSocket 收到 Pong");
                                }
                                OpCode::Continuation => {
                                    debug!("WebSocket 收到 Continuation 帧");
                                }
                            }
                        }
                        Ok(None) => {
                            // Stream 结束，连接关闭
                            info!("WebSocket Stream 结束，连接已关闭");
                            *status.write().unwrap() = WebSocketStatus::Closed;

                            if let Some(callback) = _on_close.read().unwrap().as_ref() {
                                callback();
                            }

                            drop(ws_guard);
                            break;
                        }
                        Err(_) => {
                            // Timeout，继续下一次循环（这样可以检查状态是否变化）
                            trace!("WebSocket 接收超时，继续等待");
                        }
                    }
                } else {
                    trace!("WebSocket 实例不存在，退出接收循环");
                    break;
                }

                drop(ws_guard);
            }

            info!("WebSocket 消息接收循环结束");
        });

        tokio::spawn(async move {
            if !auto_peek {
                return;
            }
            let mut ticker = tokio::time::interval(auto_peek_interval);
            loop {
                ticker.tick().await;
                let current_status = *status_for_peek.read().unwrap();
                if current_status != WebSocketStatus::Open {
                    break;
                }
                let mut ws_guard = ws_for_peek.lock().await;
                if let Some(ws_instance) = ws_guard.as_mut() {
                    let frame = FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes());
                    if let Err(e) = ws_instance.send(frame).await {
                        error!("Websocket Send `peek_message` failed: {}", e);
                        *status_for_peek.write().unwrap() = WebSocketStatus::Closed;
                        if let Some(callback) = _on_close_for_peek.read().unwrap().as_ref() {
                            callback();
                        }
                        break;
                    }
                } else {
                    break;
                }
                drop(ws_guard);
            }
        });
    }

    /// 处理重连
    async fn handle_reconnect(&self) -> bool {
        // 检查重连次数（在锁的作用域内完成）
        let (should_reconnect, times) = {
            let mut reconnect_times = self.reconnect_times.write().unwrap();

            if *reconnect_times >= self.config.reconnect_max_times {
                error!(
                    "已达到最大重连次数 {}，停止重连",
                    self.config.reconnect_max_times
                );
                return false;
            }

            *reconnect_times += 1;
            (true, *reconnect_times)
        }; // reconnect_times 锁在这里释放

        if should_reconnect {
            info!(
                "第 {} 次尝试重连（最多 {} 次）",
                times, self.config.reconnect_max_times
            );

            // 等待重连间隔
            sleep(self.config.reconnect_interval).await;

            info!("重连等待完成，准备重连");
        }
        should_reconnect
    }

    pub async fn reconnect(&self) {
        if !*self.should_reconnect.read().unwrap() {
            return;
        }
        if !self.handle_reconnect().await {
            return;
        }
        let _ = self.init(true).await;
    }
}

/// 行情 WebSocket
pub struct TqQuoteWebsocket {
    base: Arc<TqWebsocket>,
    _dm: Arc<DataManager>,
    subscribe_quote: Arc<RwLock<Option<Value>>>,
    charts: Arc<RwLock<std::collections::HashMap<String, Value>>>,
    pending_ins_query: Arc<RwLock<std::collections::HashMap<String, Value>>>,
    login_ready: Arc<std::sync::atomic::AtomicBool>,
}

impl TqQuoteWebsocket {
    /// 创建行情 WebSocket
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let base = Arc::new(TqWebsocket::new(url, config));
        let dm_clone = Arc::clone(&dm);
        let subscribe_quote: Arc<RwLock<Option<Value>>> = Arc::new(RwLock::new(None));
        let charts: Arc<RwLock<std::collections::HashMap<String, Value>>> =
            Arc::new(RwLock::new(std::collections::HashMap::new()));
        let pending_ins_query: Arc<RwLock<std::collections::HashMap<String, Value>>> =
            Arc::new(RwLock::new(std::collections::HashMap::new()));
        let login_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // 注册消息处理
        base.on_message({
            let dm = Arc::clone(&dm_clone);
            let pending_ins_query = Arc::clone(&pending_ins_query);
            let login_ready = Arc::clone(&login_ready);
            move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|v| v.as_str()) {
                    match aid {
                        "rtn_data" => {
                            if let Some(payload) = data.get("data") {
                                if let Some(array) = payload.as_array() {
                                    for item in array {
                                        if let Some(symbols) = item.get("symbols") {
                                            if let Some(obj) = symbols.as_object() {
                                                let mut pending_guard =
                                                    pending_ins_query.write().unwrap();
                                                for (query_id, value) in obj {
                                                    if !value.is_null() {
                                                        pending_guard.remove(query_id);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                dm.merge_data(payload.clone(), true, true);
                            }
                        }
                        "rsp_login" => {
                            login_ready.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
            }
        });

        // 注册重连回调：重连时重发订阅和图表请求
        {
            let subscribe_quote_clone = Arc::clone(&subscribe_quote);
            let charts_clone = Arc::clone(&charts);
            let pending_ins_query_clone = Arc::clone(&pending_ins_query);
            let login_ready_clone = Arc::clone(&login_ready);
            let base_clone = Arc::clone(&base);

            base.on_open(move || {
                login_ready_clone.store(false, std::sync::atomic::Ordering::SeqCst);
                let sub = subscribe_quote_clone.read().unwrap().clone();
                let charts = charts_clone.read().unwrap().clone();
                let pending_ins_query = pending_ins_query_clone.read().unwrap().clone();
                let base_for_send = Arc::clone(&base_clone);
                let login_ready_for_wait = Arc::clone(&login_ready_clone);

                tokio::spawn(async move {
                    let mut waited = 0usize;
                    while !login_ready_for_wait.load(std::sync::atomic::Ordering::SeqCst)
                        && waited < 100
                    {
                        sleep(Duration::from_millis(50)).await;
                        waited += 1;
                    }
                    for query in pending_ins_query.values() {
                        let _ = base_for_send.send(query).await;
                    }
                    if let Some(sub) = sub {
                        let _ = base_for_send.send(&sub).await;
                    }
                    for (chart_id, chart) in charts.iter() {
                        if let Some(view_width) = chart.get("view_width").and_then(|v| v.as_f64())
                        {
                            if view_width > 0.0 {
                                debug!("重连后重发图表请求: {}", chart_id);
                                let _ = base_for_send.send(chart).await;
                            }
                        }
                    }
                });
            });
        }

        {
            let base_clone = Arc::clone(&base);
            base.on_close(move || {
                let base_for_reconnect = Arc::clone(&base_clone);
                tokio::spawn(async move {
                    base_for_reconnect.reconnect().await;
                });
            });
        }

        TqQuoteWebsocket {
            base,
            _dm: dm_clone,
            subscribe_quote,
            charts,
            pending_ins_query,
            login_ready,
        }
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    /// 发送消息（重写以记录订阅和图表请求）
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let value = serde_json::to_value(obj)?;

        if let Some(aid) = value.get("aid").and_then(|v| v.as_str()) {
            match aid {
                "subscribe_quote" => {
                    // 检查是否需要更新订阅
                    let mut should_send = false;
                    let mut subscribe_guard = self.subscribe_quote.write().unwrap();

                    if let Some(old_sub) = subscribe_guard.as_ref() {
                        // 比较 ins_list
                        let old_list = old_sub.get("ins_list");
                        let new_list = value.get("ins_list");

                        if old_list != new_list {
                            debug!("订阅列表变化，更新订阅");
                            *subscribe_guard = Some(value.clone());
                            should_send = true;
                        } else {
                            debug!("订阅列表未变化，跳过");
                        }
                    } else {
                        debug!("首次订阅");
                        *subscribe_guard = Some(value.clone());
                        should_send = true;
                    }

                    drop(subscribe_guard);

                    if should_send {
                        return self.base.send(&value).await;
                    } else {
                        return Ok(());
                    }
                }
                "set_chart" => {
                    // 记录图表请求
                    if let Some(chart_id) = value.get("chart_id").and_then(|v| v.as_str()) {
                        let mut charts_guard = self.charts.write().unwrap();

                        if let Some(view_width) = value.get("view_width").and_then(|v| v.as_f64()) {
                            if view_width == 0.0 {
                                trace!("删除图表: {}", chart_id);
                                charts_guard.remove(chart_id);
                            } else {
                                trace!("保存图表请求: {}", chart_id);
                                charts_guard.insert(chart_id.to_string(), value.clone());
                            }
                        }

                        drop(charts_guard);
                        return self.base.send(&value).await;
                    }
                }
                "ins_query" => {
                    if let Some(query_id) = value.get("query_id").and_then(|v| v.as_str()) {
                        let mut pending_guard = self.pending_ins_query.write().unwrap();
                        pending_guard.insert(query_id.to_string(), value.clone());
                    }
                }
                _ => {}
            }
        }

        // 其他消息直接发送
        self.base.send(&value).await
    }

    /// 检查是否就绪
    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    pub fn is_logged_in(&self) -> bool {
        self.login_ready
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}

pub struct TqTradingStatusWebsocket {
    base: Arc<TqWebsocket>,
    _dm: Arc<DataManager>,
    subscribe_trading_status: Arc<RwLock<Option<Value>>>,
    option_underlyings: Arc<RwLock<HashMap<String, String>>>,
}

impl TqTradingStatusWebsocket {
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let base = Arc::new(TqWebsocket::new(url, config));
        let dm_clone = Arc::clone(&dm);
        let subscribe_trading_status: Arc<RwLock<Option<Value>>> = Arc::new(RwLock::new(None));
        let option_underlyings: Arc<RwLock<HashMap<String, String>>> =
            Arc::new(RwLock::new(HashMap::new()));

        base.on_message({
            let dm = Arc::clone(&dm_clone);
            let option_underlyings = Arc::clone(&option_underlyings);
            move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|v| v.as_str()) {
                    if aid == "rtn_data" {
                        if let Some(payload) = data.get("data").and_then(|v| v.as_array()) {
                            let mut diffs = payload.clone();
                            let mut received: HashMap<String, String> = HashMap::new();
                            for diff in diffs.iter_mut() {
                                if let Some(map) =
                                    diff.get_mut("trading_status").and_then(|v| v.as_object_mut())
                                {
                                    for (symbol, ts_val) in map.iter_mut() {
                                        if let Some(ts_map) = ts_val.as_object_mut() {
                                            if !ts_map.contains_key("symbol") {
                                                ts_map.insert("symbol".to_string(), Value::String(symbol.clone()));
                                            }
                                            if let Some(status_val) =
                                                ts_map.get_mut("trade_status")
                                            {
                                                if let Some(status) = status_val.as_str() {
                                                    let normalized = if status == "AUCTIONORDERING"
                                                        || status == "CONTINOUS"
                                                    {
                                                        status.to_string()
                                                    } else {
                                                        "NOTRADING".to_string()
                                                    };
                                                    *status_val = Value::String(normalized.clone());
                                                    received.insert(symbol.clone(), normalized);
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            let option_map = option_underlyings.read().unwrap();
                            for (option, underlying) in option_map.iter() {
                                if let Some(status) = received.get(underlying) {
                                    diffs.push(json!({
                                        "trading_status": {
                                            option: {
                                                "symbol": option,
                                                "trade_status": status
                                            }
                                        }
                                    }));
                                }
                            }
                            dm.merge_data(Value::Array(diffs), true, true);
                        }
                    }
                }
            }
        });

        {
            let subscribe_clone = Arc::clone(&subscribe_trading_status);
            let base_clone = Arc::clone(&base);
            base.on_open(move || {
                if let Some(sub) = subscribe_clone.read().unwrap().as_ref() {
                    let base_for_send = Arc::clone(&base_clone);
                    let sub_clone = sub.clone();
                    tokio::spawn(async move {
                        let _ = base_for_send.send(&sub_clone).await;
                    });
                }
            });
        }

        TqTradingStatusWebsocket {
            base,
            _dm: dm_clone,
            subscribe_trading_status,
            option_underlyings,
        }
    }

    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let value = serde_json::to_value(obj)?;

        if let Some(aid) = value.get("aid").and_then(|v| v.as_str()) {
            if aid == "subscribe_trading_status" {
                let mut should_send = false;
                let mut subscribe_guard = self.subscribe_trading_status.write().unwrap();
                if let Some(old_sub) = subscribe_guard.as_ref() {
                    let old_list = old_sub.get("ins_list");
                    let new_list = value.get("ins_list");
                    if old_list != new_list {
                        *subscribe_guard = Some(value.clone());
                        should_send = true;
                    }
                } else {
                    *subscribe_guard = Some(value.clone());
                    should_send = true;
                }
                drop(subscribe_guard);
                if should_send {
                    return self.base.send(&value).await;
                }
                return Ok(());
            }
        }

        self.base.send(&value).await
    }

    pub fn update_option_underlyings(&self, mapping: HashMap<String, String>) {
        let mut guard = self.option_underlyings.write().unwrap();
        for (option, underlying) in mapping {
            guard.insert(option, underlying);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}

/// 交易 WebSocket
pub struct TqTradeWebsocket {
    base: Arc<TqWebsocket>,
    _dm: Arc<DataManager>,
    req_login: Arc<RwLock<Option<Value>>>,
    on_notify: Arc<RwLock<Option<Box<dyn Fn(crate::types::Notification) + Send + Sync>>>>,
}

impl TqTradeWebsocket {
    /// 创建交易 WebSocket
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let base = Arc::new(TqWebsocket::new(url, config));
        let dm_clone = Arc::clone(&dm);
        let req_login: Arc<RwLock<Option<Value>>> = Arc::new(RwLock::new(None));
        let on_notify: Arc<RwLock<Option<Box<dyn Fn(crate::types::Notification) + Send + Sync>>>> =
            Arc::new(RwLock::new(None));

        // 注册消息处理
        {
            let dm = Arc::clone(&dm_clone);
            let on_notify_clone = Arc::clone(&on_notify);

            base.on_message(move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|v| v.as_str()) {
                    match aid {
                        "rtn_data" => {
                            if let Some(payload) = data.get("data") {
                                // 分离通知
                                if let Some(array) = payload.as_array() {
                                    let (notifies, cleaned_data) =
                                        Self::separate_notifies(array.clone());
                                    debug!("notifies: {:?}", notifies);

                                    // 触发通知回调
                                    if let Some(callback) = on_notify_clone.read().unwrap().as_ref()
                                    {
                                        for notify in notifies {
                                            callback(notify);
                                        }
                                    }

                                    // 合并清理后的数据
                                    dm.merge_data(Value::Array(cleaned_data), true, true);
                                } else {
                                    dm.merge_data(payload.clone(), true, true);
                                }
                            }
                        }
                        "rtn_brokers" => {
                            // 期货公司列表（暂不处理全局事件）
                            debug!("收到期货公司列表");
                        }
                        "qry_settlement_info" => {
                            // 历史结算单
                            if let (Some(settlement_info), Some(user_name), Some(trading_day)) = (
                                data.get("settlement_info").and_then(|v| v.as_str()),
                                data.get("user_name").and_then(|v| v.as_str()),
                                data.get("trading_day").and_then(|v| v.as_str()),
                            ) {
                                debug!(
                                    "收到结算单: user={}, trading_day={}",
                                    user_name, trading_day
                                );

                                // 解析结算单内容
                                let settlement = Self::parse_settlement_content(settlement_info);

                                // 合并到 DataManager
                                let settlement_data = serde_json::json!({
                                    "trade": {
                                        user_name: {
                                            "his_settlements": {
                                                trading_day: settlement
                                            }
                                        }
                                    }
                                });

                                dm.merge_data(settlement_data, true, true);
                            }
                        }
                        _ => {}
                    }
                }
            });
        }

        // 注册重连回调：重连时重发登录请求
        {
            let req_login_clone = Arc::clone(&req_login);
            let base_clone = Arc::clone(&base);

            base.on_open(move || {
                if let Some(login) = req_login_clone.read().unwrap().as_ref() {
                    debug!("重连后重发登录请求");
                    let base_for_send = Arc::clone(&base_clone);
                    let login_clone = login.clone();
                    tokio::spawn(async move {
                        let _ = base_for_send.send(&login_clone).await;
                    });
                }
            });
        }

        TqTradeWebsocket {
            base,
            _dm: dm_clone,
            req_login,
            on_notify,
        }
    }

    /// 分离通知
    ///
    /// 从 rtn_data 的 data 数组中提取通知，并返回清理后的数据
    fn separate_notifies(data: Vec<Value>) -> (Vec<crate::types::Notification>, Vec<Value>) {
        let mut notifies = Vec::new();
        let mut cleaned_data = Vec::new();

        for mut item in data {
            if let Some(obj) = item.as_object_mut() {
                // 提取 notify 字段
                if let Some(notify_data) = obj.remove("notify") {
                    if let Some(notify_map) = notify_data.as_object() {
                        for (_key, notify_value) in notify_map {
                            if let Some(n) = notify_value.as_object() {
                                let notification = crate::types::Notification {
                                    code: n
                                        .get("code")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    level: n
                                        .get("level")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    r#type: n
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    content: n
                                        .get("content")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    bid: n
                                        .get("bid")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    user_id: n
                                        .get("user_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                };
                                notifies.push(notification);
                            }
                        }
                    }
                }
            }

            // 添加到清理后的数据
            cleaned_data.push(item);
        }

        (notifies, cleaned_data)
    }

    /// 解析结算单内容
    ///
    /// 简单的结算单解析（仅返回原始内容）
    fn parse_settlement_content(content: &str) -> serde_json::Value {
        serde_json::json!({
            "content": content,
            "parsed": false  // 标记为未解析（可后续扩展）
        })
    }

    /// 注册通知回调
    pub fn on_notify<F>(&self, callback: F)
    where
        F: Fn(crate::types::Notification) + Send + Sync + 'static,
    {
        *self.on_notify.write().unwrap() = Some(Box::new(callback));
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        self.base.init(is_reconnection).await
    }

    /// 发送消息（重写以记录登录请求）
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        // 序列化为 Value 以检查 aid
        let json_str = serde_json::to_string(obj)?;
        let value: Value = serde_json::from_str(&json_str)?;

        // 如果是登录请求，记录下来
        if let Some(aid) = value.get("aid").and_then(|v| v.as_str()) {
            if aid == "req_login" {
                debug!("记录登录请求 {:?}", value);
                *self.req_login.write().unwrap() = Some(value.clone());
            }
        }

        // 发送消息
        self.base.send(&value).await
    }

    /// 检查是否就绪
    pub fn is_ready(&self) -> bool {
        self.base.is_ready()
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        self.base.close().await
    }
}

// WebSocket 实现说明：
// ✅ 1. WebSocket 连接创建 - 已实现（使用 yawc::WebSocket::connect）
// ✅ 2. deflate 压缩启用 - 已实现（使用 Options::default()）
// ✅ 3. 消息发送 - 已实现（使用 Sink trait 和 FrameView）
// ✅ 4. 消息接收 - 已实现（使用 Stream trait 的 next() 方法）
// ✅ 5. 连接状态管理 - 已实现（WebSocketStatus 状态机）
// ✅ 6. 消息接收循环 - 已实现（start_receive_loop）
// ✅ 7. 自动重连机制 - 已实现（handle_reconnect）
// ✅ 8. 消息队列 - 已实现（连接未就绪时缓存消息）
// ✅ 9. 回调机制 - 已实现（on_message, on_open, on_close, on_error）
// ✅ 10. 帧类型处理 - 已实现（Text, Binary, Close, Ping, Pong, Continuation）
//
// TqQuoteWebsocket 特性：
// ✅ 1. Send 方法重写 - 已实现（记录 subscribe_quote 和 charts）
// ✅ 2. 订阅去重 - 已实现（比较 ins_list，只在变化时发送）
// ✅ 3. 图表管理 - 已实现（view_width == 0 时删除，否则保存）
// ✅ 4. 重连恢复 - 已实现（重连时自动重发订阅和图表请求）
//
// TqTradeWebsocket 特性：
// ✅ 1. Send 方法重写 - 已实现（记录 req_login）
// ✅ 2. 重连恢复 - 已实现（重连时自动重发登录请求）
// ✅ 3. 通知分离 - 已实现（separate_notifies）
// ✅ 4. 通知回调 - 已实现（on_notify）
// ✅ 5. 结算单处理 - 已实现（qry_settlement_info）
// ⚠️  6. 期货公司列表 - 已接收但未穿透到上层（rtn_brokers）
//
// yawc API 使用说明：
// - WebSocket 实现了 futures::Stream trait，使用 .next().await 接收消息
// - WebSocket 实现了 futures::Sink<FrameView> trait，使用 .send(frame).await 发送消息
// - FrameView::text(bytes) 创建文本帧
// - frame.opcode 确定帧类型（OpCode::Text, Binary, Close, Ping, Pong, Continuation）
// - frame.payload 包含帧数据（Bytes 类型）
//
// 注意事项：
// - 重连逻辑需要外部监听 on_error 回调并调用 init(true)
// - deflate 压缩通过 Options::default() 配置，支持客户端和服务端压缩
// - 消息接收循环在独立的 tokio task 中运行
// - Ping/Pong 帧由 yawc 自动处理，无需手动响应
// - 重连时自动重发订阅/登录/图表请求（通过 on_open 回调）
