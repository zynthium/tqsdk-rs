use super::{
    CloseCallback, ErrorCallback, MessageCallback, OpenCallback, build_connection_notify,
    derive_message_backlog_max,
    is_ops_maintenance_window_cst, next_shared_reconnect_delay, sanitize_log_pack_value,
};
use crate::errors::{Result, TqError};
use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use yawc::frame::{FrameView, OpCode};

const DEFAULT_MESSAGE_QUEUE_CAPACITY: usize = 2048;
const DEFAULT_MESSAGE_BACKLOG_WARN_STEP: usize = 1024;
const DEFAULT_MESSAGE_BATCH_MAX: usize = 32;

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
    pub peek_timeout: Option<Duration>,
    pub quote_subscribe_only_add: bool,
    pub message_queue_capacity: usize,
    pub message_backlog_warn_step: usize,
    pub message_batch_max: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            reconnect_interval: Duration::from_secs(3),
            reconnect_max_times: usize::MAX,
            auto_peek: true,
            auto_peek_interval: Duration::ZERO,
            peek_timeout: None,
            quote_subscribe_only_add: false,
            message_queue_capacity: DEFAULT_MESSAGE_QUEUE_CAPACITY,
            message_backlog_warn_step: DEFAULT_MESSAGE_BACKLOG_WARN_STEP,
            message_batch_max: DEFAULT_MESSAGE_BATCH_MAX,
        }
    }
}

/// Socket I/O 采用单所有者 actor，避免跨 await 持有锁。
///
/// - 发送通过有界 mpsc 队列进入 actor（提供背压）。
/// - close 通过 Notify 触发（不依赖队列容量，避免 close 被 send 队列阻塞）。
#[derive(Clone)]
pub(crate) struct WsIoHandle {
    tx: tokio::sync::mpsc::Sender<WsIoCommand>,
    close_notify: Arc<Notify>,
}

impl WsIoHandle {
    async fn send_text(&self, payload: String) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(WsIoCommand::SendText {
                payload,
                resp: resp_tx,
            })
            .await
            .map_err(|_| TqError::WebSocketError("websocket io actor is closed".to_string()))?;
        resp_rx.await.unwrap_or_else(|_| {
            Err(TqError::WebSocketError(
                "websocket io actor dropped".to_string(),
            ))
        })
    }

    fn request_close(&self) {
        self.close_notify.notify_waiters();
        let _ = self.tx.try_send(WsIoCommand::Shutdown);
    }
}

pub(crate) enum WsIoCommand {
    SendText {
        payload: String,
        resp: oneshot::Sender<Result<()>>,
    },
    Shutdown,
}

pub(crate) struct WsActorContext {
    connection_id: u64,
    current_connection_id: Arc<RwLock<u64>>,
    status: Arc<RwLock<WebSocketStatus>>,
    pending_peek: Arc<AtomicBool>,
    last_peek_sent: Arc<std::sync::Mutex<std::time::Instant>>,
    disconnect_reason: Arc<RwLock<Option<String>>>,
    on_message: MessageCallback,
    on_close: CloseCallback,
    url: String,
    conn_id: String,
    auto_peek: bool,
    auto_peek_interval: Duration,
    peek_timeout: Option<Duration>,
}

impl WsActorContext {
    fn is_current_connection(&self) -> bool {
        *self.current_connection_id.read().unwrap() == self.connection_id
    }

    fn current_status(&self) -> WebSocketStatus {
        *self.status.read().unwrap()
    }

    fn mark_disconnect_reason(&self, reason: impl Into<String>) {
        *self.disconnect_reason.write().unwrap() = Some(reason.into());
    }

    fn set_status(&self, status: WebSocketStatus) {
        *self.status.write().unwrap() = status;
    }

    fn clear_pending_peek(&self) {
        self.pending_peek.store(false, Ordering::SeqCst);
    }

    fn mark_pending_peek(&self) -> bool {
        self.pending_peek.swap(true, Ordering::SeqCst)
    }

    fn pending_peek(&self) -> bool {
        self.pending_peek.load(Ordering::SeqCst)
    }

    fn update_last_peek_sent(&self) {
        if let Ok(mut time) = self.last_peek_sent.lock() {
            *time = std::time::Instant::now();
        }
    }

    fn last_recv_elapsed(
        &self,
        last_recv_time: &Arc<std::sync::Mutex<std::time::Instant>>,
    ) -> Option<std::time::Duration> {
        last_recv_time.lock().ok().map(|time| time.elapsed())
    }

    fn last_peek_elapsed(&self) -> Option<std::time::Duration> {
        self.last_peek_sent.lock().ok().map(|time| time.elapsed())
    }

    fn emit_disconnect_notify(&self) {
        let callback = self.on_message.read().unwrap().clone();
        if let Some(callback) = callback {
            callback(build_connection_notify(
                2019112911,
                "WARNING",
                format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", self.url),
                self.url.clone(),
                self.conn_id.clone(),
            ));
        }
    }

    fn fire_on_close(&self, closed_callback_fired: &mut bool) {
        let callback = self.on_close.read().unwrap().clone();
        if let Some(callback) = callback {
            *closed_callback_fired = true;
            callback();
        }
    }

    fn disconnect_and_close(&self, reason: impl Into<String>, closed_callback_fired: &mut bool) {
        self.mark_disconnect_reason(reason);
        self.set_status(WebSocketStatus::Closed);
        self.emit_disconnect_notify();
        self.fire_on_close(closed_callback_fired);
    }
}

/// 天勤 WebSocket 基类
///
/// 基于 yawc 库实现 WebSocket 连接
pub struct TqWebsocket {
    url: String,
    conn_id: String,
    config: WebSocketConfig,
    status: Arc<RwLock<WebSocketStatus>>,
    queue: Arc<Mutex<VecDeque<String>>>,
    pending_queue_max: usize,
    reconnect_times: Arc<RwLock<usize>>,
    should_reconnect: Arc<RwLock<bool>>,
    connecting: Arc<AtomicBool>,
    connected_once: Arc<AtomicBool>,
    pending_peek: Arc<AtomicBool>,
    query_max_length: usize,
    ins_list_max_length: usize,
    subscribed_ins_list_throttle: usize,
    subscribed_counts: AtomicUsize,
    last_peek_sent: Arc<std::sync::Mutex<std::time::Instant>>,
    connection_id: Arc<RwLock<u64>>,
    last_disconnect_reason: Arc<RwLock<Option<String>>>,
    io: Arc<std::sync::Mutex<Option<WsIoHandle>>>,
    on_message: MessageCallback,
    on_open: OpenCallback,
    on_close: CloseCallback,
    on_error: ErrorCallback,
}

impl TqWebsocket {
    fn emit_connection_notify(&self, code: i64, level: &str, content: String) {
        let callback = self.on_message.read().unwrap().clone();
        if let Some(callback) = callback {
            callback(build_connection_notify(
                code,
                level,
                content,
                self.url.clone(),
                self.conn_id.clone(),
            ));
        }
    }

    fn emit_send_guard_warnings(&self, value: &Value) {
        let Some(aid) = value.get("aid").and_then(|aid| aid.as_str()) else {
            return;
        };
        if aid == "subscribe_quote" {
            let ins_list = value
                .get("ins_list")
                .and_then(|list| list.as_str())
                .unwrap_or("");
            if ins_list.len() > self.ins_list_max_length {
                warn!(
                    "订阅合约字符串总长度大于 {}，可能会引起服务器限制。",
                    self.ins_list_max_length
                );
            }
            let ins_list_counts = ins_list
                .split(',')
                .filter(|symbol| !symbol.trim().is_empty())
                .count();
            let subscribed_counts = self.subscribed_counts.fetch_add(1, Ordering::SeqCst) + 1;
            if ins_list_counts > self.subscribed_ins_list_throttle
                && (ins_list_counts as f64) / (subscribed_counts as f64) < 2.0
            {
                warn!("订阅多合约时建议合并为一次 subscribe_quote 调用。");
            }
            return;
        }
        if aid == "ins_query" {
            let query = value
                .get("query")
                .and_then(|query| query.as_str())
                .unwrap_or("");
            if query.len() > self.query_max_length {
                warn!(
                    "订阅合约信息字段总长度大于 {}，可能会引起服务器限制。",
                    self.query_max_length
                );
            }
        }
    }

    /// 创建新的 WebSocket 连接
    pub fn new(url: String, config: WebSocketConfig) -> Self {
        let pending_queue_max = derive_message_backlog_max(
            config.message_queue_capacity,
            config.message_backlog_warn_step,
        );
        Self {
            url,
            conn_id: uuid::Uuid::new_v4().to_string(),
            config,
            status: Arc::new(RwLock::new(WebSocketStatus::Closed)),
            queue: Arc::new(Mutex::new(VecDeque::new())),
            pending_queue_max,
            reconnect_times: Arc::new(RwLock::new(0)),
            should_reconnect: Arc::new(RwLock::new(true)),
            connecting: Arc::new(AtomicBool::new(false)),
            connected_once: Arc::new(AtomicBool::new(false)),
            pending_peek: Arc::new(AtomicBool::new(false)),
            query_max_length: 50000,
            ins_list_max_length: 100000,
            subscribed_ins_list_throttle: 100,
            subscribed_counts: AtomicUsize::new(0),
            last_peek_sent: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
            connection_id: Arc::new(RwLock::new(0)),
            last_disconnect_reason: Arc::new(RwLock::new(None)),
            io: Arc::new(std::sync::Mutex::new(None)),
            on_message: Arc::new(RwLock::new(None)),
            on_open: Arc::new(RwLock::new(None)),
            on_close: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
        }
    }

    fn set_disconnect_reason(&self, reason: impl Into<String>) {
        *self.last_disconnect_reason.write().unwrap() = Some(reason.into());
    }

    fn take_disconnect_reason(&self) -> Option<String> {
        self.last_disconnect_reason.write().unwrap().take()
    }

    async fn enqueue_pending_message(&self, json_str: String) {
        let mut queue = self.queue.lock().await;
        if queue.len() >= self.pending_queue_max {
            queue.pop_front();
            warn!(
                "WebSocket 待发送队列达到上限，丢弃最旧消息: queue_max={}",
                self.pending_queue_max
            );
        }
        queue.push_back(json_str);
    }

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
        *self.should_reconnect.write().unwrap() = true;
        let status = *self.status.read().unwrap();
        if status == WebSocketStatus::Open || status == WebSocketStatus::Connecting {
            return Ok(());
        }
        if self.connecting.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        info!(
            "正在连接 WebSocket: {} (重连: {})",
            self.url, is_reconnection
        );
        if is_reconnection {
            debug!("websocket connection connecting");
            self.emit_connection_notify(
                2019112910,
                "WARNING",
                format!("开始与 {} 重新建立网络连接", self.url),
            );
        }
        *self.status.write().unwrap() = WebSocketStatus::Connecting;

        let options = yawc::Options::default()
            .client_no_context_takeover()
            .server_no_context_takeover();

        let parsed_url = url::Url::parse(&self.url)
            .map_err(|error| TqError::WebSocketError(format!("Invalid URL: {}", error)))?;

        let mut http_builder = yawc::HttpRequestBuilder::new();
        for (key, value) in self.config.headers.iter() {
            if let Ok(value_str) = value.to_str() {
                http_builder = http_builder.header(key.as_str(), value_str);
            }
        }

        let ws = match yawc::WebSocket::connect(parsed_url)
            .with_options(options)
            .with_request(http_builder)
            .await
        {
            Ok(ws) => ws,
            Err(error) => {
                error!(url = %self.url, error = %error, "WebSocket 连接失败");
                debug!(error = %error, "websocket connection closed");
                let in_ops_time = is_ops_maintenance_window_cst();
                let mut content =
                    format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", self.url);
                if in_ops_time {
                    content.push_str("，每日 19:00-19:30 为日常运维时间，请稍后再试");
                }

                let callback = self.on_error.read().unwrap().clone();
                if let Some(callback) = callback {
                    callback(format!("Connection failed: {}", error));
                }
                self.emit_connection_notify(2019112911, "WARNING", content);
                if !is_reconnection && in_ops_time {
                    self.connecting.store(false, Ordering::SeqCst);
                    return Err(TqError::WebSocketError(format!(
                        "与 {} 的连接失败，每日 19:00-19:30 为日常运维时间，请稍后再试",
                        self.url
                    )));
                }

                if *self.should_reconnect.read().unwrap() {
                    let _ = self.handle_reconnect().await;
                }

                self.connecting.store(false, Ordering::SeqCst);
                return Err(TqError::WebSocketError(format!(
                    "Connection failed: {}",
                    error
                )));
            }
        };

        let connection_id = {
            let mut guard = self.connection_id.write().unwrap();
            *guard += 1;
            *guard
        };
        if let Some(old) = self.io.lock().unwrap().take() {
            old.request_close();
        }

        *self.status.write().unwrap() = WebSocketStatus::Open;
        *self.last_disconnect_reason.write().unwrap() = None;
        if !is_reconnection {
            *self.reconnect_times.write().unwrap() = 0;
        }

        let callback = self.on_open.read().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
        let first_connected = !self.connected_once.swap(true, Ordering::SeqCst);
        if first_connected && !is_reconnection {
            debug!("websocket connected");
            self.emit_connection_notify(
                2019112901,
                "INFO",
                format!("与 {} 的网络连接已建立", self.url),
            );
        } else {
            debug!("websocket reconnected");
            self.emit_connection_notify(
                2019112902,
                "WARNING",
                format!("与 {} 的网络连接已恢复", self.url),
            );
        }

        self.install_io_actor(ws, connection_id);
        self.flush_queue().await;
        self.send_peek_message().await?;

        info!("WebSocket 连接成功");
        self.connecting.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// 发送消息
    pub async fn send<T: Serialize>(&self, obj: &T) -> Result<()> {
        let value = serde_json::to_value(obj).map_err(|source| TqError::Json {
            context: "序列化 websocket 消息失败".to_string(),
            source,
        })?;
        self.emit_send_guard_warnings(&value);
        let json_str = value.to_string();
        let log_pack = sanitize_log_pack_value(&value);
        debug!(pack = %log_pack, "websocket send data");

        if self.is_ready() {
            debug!("WebSocket 发送消息: {}", log_pack);
            let io = self.io.lock().unwrap().clone();
            if let Some(io) = io {
                match io.send_text(json_str).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        if *self.status.read().unwrap() == WebSocketStatus::Open {
                            self.set_disconnect_reason(format!("send_frame_failed: {}", error));
                            *self.status.write().unwrap() = WebSocketStatus::Closed;
                            self.emit_connection_notify(
                                2019112911,
                                "WARNING",
                                format!(
                                    "与 {} 的网络连接断开，请检查客户端及网络是否正常",
                                    self.url
                                ),
                            );
                            let callback = self.on_close.read().unwrap().clone();
                            if let Some(callback) = callback {
                                callback();
                            }
                        }
                        Err(error)
                    }
                }
            } else {
                debug!("WebSocket I/O actor 不存在，消息加入队列");
                self.enqueue_pending_message(json_str).await;
                Ok(())
            }
        } else {
            debug!("WebSocket 未就绪，消息加入队列: {}", log_pack);
            self.enqueue_pending_message(json_str).await;
            Ok(())
        }
    }

    /// 检查连接是否就绪
    pub fn is_ready(&self) -> bool {
        *self.status.read().unwrap() == WebSocketStatus::Open
    }

    pub fn auto_peek_enabled(&self) -> bool {
        self.config.auto_peek
    }

    pub(crate) fn message_queue_capacity(&self) -> usize {
        self.config.message_queue_capacity.max(1)
    }

    pub async fn send_peek_message(&self) -> Result<()> {
        if !self.config.auto_peek {
            return Ok(());
        }
        if self.pending_peek.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let res = self.send(&json!({"aid": "peek_message"})).await;
        if res.is_ok()
            && let Ok(mut time) = self.last_peek_sent.lock()
        {
            *time = std::time::Instant::now();
        }
        if res.is_err() {
            self.pending_peek.store(false, Ordering::SeqCst);
        }
        res
    }

    /// 关闭连接
    pub async fn close(&self) -> Result<()> {
        info!("正在关闭 WebSocket 连接");
        *self.should_reconnect.write().unwrap() = false;
        self.pending_peek.store(false, Ordering::SeqCst);
        *self.status.write().unwrap() = WebSocketStatus::Closing;
        self.set_disconnect_reason("manual_close");
        self.connecting.store(false, Ordering::SeqCst);

        if let Some(io) = self.io.lock().unwrap().take() {
            io.request_close();
        } else {
            *self.status.write().unwrap() = WebSocketStatus::Closed;
            let callback = self.on_close.read().unwrap().clone();
            if let Some(callback) = callback {
                callback();
            }
        }

        Ok(())
    }

    async fn flush_queue(&self) {
        let pending = {
            let mut queue = self.queue.lock().await;
            if queue.is_empty() {
                return;
            }
            debug!("发送队列中的 {} 条消息", queue.len());
            std::mem::take(&mut *queue)
        };

        let io = self.io.lock().unwrap().clone();
        let Some(io) = io else {
            let mut queue = self.queue.lock().await;
            queue.extend(pending);
            return;
        };

        for (idx, msg) in pending.iter().enumerate() {
            if let Err(error) = io.send_text(msg.clone()).await {
                error!("队列消息发送失败: {}", error);
                self.set_disconnect_reason(format!("flush_queue_send_failed: {}", error));
                let mut queue = self.queue.lock().await;
                queue.extend(pending.iter().skip(idx).cloned());
                break;
            }
        }
    }

    /// 注册消息回调
    pub fn on_message<F>(&self, callback: F)
    where
        F: Fn(Value) + Send + Sync + 'static,
    {
        *self.on_message.write().unwrap() = Some(Arc::new(callback));
    }

    /// 注册连接打开回调
    pub fn on_open<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_open.write().unwrap() = Some(Arc::new(callback));
    }

    /// 注册连接关闭回调
    pub fn on_close<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.on_close.write().unwrap() = Some(Arc::new(callback));
    }

    /// 注册错误回调
    pub fn on_error<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        *self.on_error.write().unwrap() = Some(Arc::new(callback));
    }

    fn install_io_actor(&self, ws: yawc::WebSocket, connection_id: u64) {
        let capacity = self.config.message_queue_capacity.max(1);
        let (tx, rx) = tokio::sync::mpsc::channel::<WsIoCommand>(capacity);
        let close_notify = Arc::new(Notify::new());

        *self.io.lock().unwrap() = Some(WsIoHandle {
            tx: tx.clone(),
            close_notify: Arc::clone(&close_notify),
        });

        let ctx = WsActorContext {
            connection_id,
            current_connection_id: Arc::clone(&self.connection_id),
            status: Arc::clone(&self.status),
            pending_peek: Arc::clone(&self.pending_peek),
            last_peek_sent: Arc::clone(&self.last_peek_sent),
            disconnect_reason: Arc::clone(&self.last_disconnect_reason),
            on_message: Arc::clone(&self.on_message),
            on_close: Arc::clone(&self.on_close),
            url: self.url.clone(),
            conn_id: self.conn_id.clone(),
            auto_peek: self.config.auto_peek,
            auto_peek_interval: self.config.auto_peek_interval,
            peek_timeout: self.config.peek_timeout,
        };

        tokio::spawn(async move {
            ws_io_actor_loop(ws, rx, close_notify, ctx).await;
        });
    }

    async fn handle_reconnect(&self) -> bool {
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
        };

        if should_reconnect {
            info!(
                "第 {} 次尝试重连（最多 {} 次）",
                times, self.config.reconnect_max_times
            );

            let wait_duration =
                next_shared_reconnect_delay(times as u32, self.config.reconnect_interval);
            if wait_duration > Duration::ZERO {
                sleep(wait_duration).await;
            }

            info!("重连等待完成，准备重连");
        }
        should_reconnect
    }

    pub async fn reconnect(&self) {
        if !*self.should_reconnect.read().unwrap() {
            return;
        }
        if let Some(reason) = self.take_disconnect_reason() {
            warn!("进入重连流程，最近一次断线原因: {}", reason);
        } else {
            warn!("进入重连流程，最近一次断线原因: unknown");
        }

        loop {
            if !*self.should_reconnect.read().unwrap() {
                info!("自动重连已禁用，停止重连流程");
                break;
            }
            if !self.handle_reconnect().await {
                error!("已达到最大重连次数，放弃重连");
                break;
            }

            if !*self.should_reconnect.read().unwrap() {
                info!("自动重连已禁用，停止重连流程");
                break;
            }

            match self.init(true).await {
                Ok(_) => {
                    info!("重连成功");
                    *self.reconnect_times.write().unwrap() = 0;
                    break;
                }
                Err(error) => {
                    error!("重连失败: {}, 继续尝试...", error);
                }
            }
        }
    }
}

async fn ws_io_actor_loop(
    mut ws: yawc::WebSocket,
    mut cmd_rx: tokio::sync::mpsc::Receiver<WsIoCommand>,
    close_notify: Arc<Notify>,
    ctx: WsActorContext,
) {
    debug!(
        connection_id = ctx.connection_id,
        "启动 WebSocket I/O actor"
    );

    let last_recv_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let mut consecutive_none = 0u8;
    let mut closed_callback_fired = false;
    let timeout_duration = tokio::time::Duration::from_secs(15);
    let mut peek_ticker = if ctx.auto_peek && !ctx.auto_peek_interval.is_zero() {
        Some(tokio::time::interval(ctx.auto_peek_interval))
    } else {
        None
    };

    loop {
        if !ctx.is_current_connection() {
            debug!("WebSocket 连接 ID 不匹配，退出 I/O actor");
            break;
        }
        if ctx.current_status() != WebSocketStatus::Open {
            debug!("WebSocket 状态不是 Open，退出 I/O actor");
            break;
        }

        tokio::select! {
            _ = close_notify.notified() => {
                debug!("收到 close 信号，退出 I/O actor");
                break;
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsIoCommand::SendText { payload, resp }) => {
                        let frame = FrameView::text(payload.into_bytes());
                        match ws.send(frame).await {
                            Ok(()) => {
                                let _ = resp.send(Ok(()));
                            }
                            Err(error) => {
                                let err_msg = format!("Send failed: {}", error);
                                let _ = resp.send(Err(TqError::WebSocketError(err_msg.clone())));
                                ctx.disconnect_and_close(
                                    format!("send_frame_failed: {}", error),
                                    &mut closed_callback_fired,
                                );
                                break;
                            }
                        }
                    }
                    Some(WsIoCommand::Shutdown) | None => {
                        debug!("收到 Shutdown 或 command channel 关闭，退出 I/O actor");
                        break;
                    }
                }
            }

            tick = async {
                if let Some(ticker) = &mut peek_ticker {
                    ticker.tick().await;
                    true
                } else {
                    std::future::pending::<bool>().await
                }
            } => {
                if tick {
                    if ctx.pending_peek() {
                        if let Some(timeout) = ctx.peek_timeout {
                            let last_recv_elapsed = ctx.last_recv_elapsed(&last_recv_time);
                            let last_peek_elapsed = ctx.last_peek_elapsed();
                            if let (Some(recv_elapsed), Some(peek_elapsed)) =
                                (last_recv_elapsed, last_peek_elapsed)
                                && recv_elapsed > timeout
                                && peek_elapsed > timeout
                            {
                                ctx.clear_pending_peek();
                            }
                        }
                        continue;
                    }

                    if ctx.mark_pending_peek() {
                        continue;
                    }
                    if let Err(error) = ws.send(peek_frame()).await {
                        error!("Websocket Timer Send `peek_message` failed: {}", error);
                        ctx.clear_pending_peek();
                        ctx.disconnect_and_close(
                            format!("timer_send_peek_failed: {}", error),
                            &mut closed_callback_fired,
                        );
                        break;
                    }
                    ctx.update_last_peek_sent();
                    debug!("Websocket Timer Send -> peek_message");
                }
            }

            next_result = tokio::time::timeout(timeout_duration, ws.next()) => {
                match next_result {
                    Ok(Some(frame)) => {
                        consecutive_none = 0;
                        if let Ok(mut time) = last_recv_time.lock() {
                            *time = std::time::Instant::now();
                        }
                        if ctx.auto_peek {
                            ctx.clear_pending_peek();
                        }

                        match frame.opcode {
                            OpCode::Text | OpCode::Binary => {
                                match serde_json::from_slice::<Value>(&frame.payload) {
                                    Ok(json_value) => {
                                        let text = String::from_utf8_lossy(&frame.payload);
                                        debug!("WebSocket Recv Text: {}", text);
                                        debug!(pack = %text, "websocket received data");
                                        let callback = ctx.on_message.read().unwrap().clone();
                                        if let Some(callback) = callback {
                                            callback(json_value);
                                        }
                                    }
                                    Err(error) => {
                                        warn!(
                                            "解析 JSON 失败: {}, payload={}",
                                            error,
                                            String::from_utf8_lossy(&frame.payload)
                                        );
                                    }
                                }

                                if ctx.auto_peek && !ctx.mark_pending_peek() {
                                    match ws.send(peek_frame()).await {
                                        Ok(()) => {
                                            ctx.update_last_peek_sent();
                                            debug!("Websocket Send -> peek_message");
                                        }
                                        Err(error) => {
                                            ctx.clear_pending_peek();
                                            error!("Websocket Send `peek_message` failed: {}", error);
                                            ctx.disconnect_and_close(
                                                format!("recv_loop_send_peek_failed: {}", error),
                                                &mut closed_callback_fired,
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                            OpCode::Close => {
                                info!("WebSocket 收到关闭帧");
                                ctx.disconnect_and_close(
                                    "server_close_frame".to_string(),
                                    &mut closed_callback_fired,
                                );
                                break;
                            }
                            OpCode::Ping => debug!("WebSocket 收到 Ping（自动处理）"),
                            OpCode::Pong => debug!("WebSocket 收到 Pong"),
                            OpCode::Continuation => debug!("WebSocket 收到 Continuation 帧"),
                        }
                    }
                    Ok(None) => {
                        consecutive_none = consecutive_none.saturating_add(1);
                        if consecutive_none < 3 {
                            warn!(
                                "WebSocket next() 返回 None（第 {} 次），等待确认后再断线处理",
                                consecutive_none
                            );
                            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                            continue;
                        }

                        match ws.send(peek_frame()).await {
                            Ok(()) => {
                                ctx.update_last_peek_sent();
                                ctx.pending_peek.store(true, Ordering::SeqCst);
                                consecutive_none = 0;
                                warn!("WebSocket next() 连续返回 None，但探测发送成功，继续保持连接");
                                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                continue;
                            }
                            Err(error) => {
                                ctx.mark_disconnect_reason(format!(
                                    "stream_ended_probe_send_failed: {}",
                                    error
                                ));
                            }
                        }

                        info!("WebSocket Stream 结束，连接已关闭");
                        ctx.set_status(WebSocketStatus::Closed);
                        ctx.emit_disconnect_notify();
                        ctx.fire_on_close(&mut closed_callback_fired);
                        break;
                    }
                    Err(_) => {
                        trace!("WebSocket 接收超时，继续等待");
                    }
                }
            }
        }
    }

    if ctx.is_current_connection() && !closed_callback_fired {
        let reason = ctx.disconnect_reason.read().unwrap().clone();
        if reason.as_deref() != Some("manual_close")
            && ctx.current_status() == WebSocketStatus::Open
        {
            ctx.set_status(WebSocketStatus::Closed);
            ctx.emit_disconnect_notify();
        } else {
            ctx.set_status(WebSocketStatus::Closed);
        }
        let callback = ctx.on_close.read().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    debug!(
        connection_id = ctx.connection_id,
        "WebSocket I/O actor 结束"
    );
}

fn peek_frame() -> FrameView {
    FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes())
}

#[cfg(test)]
impl TqWebsocket {
    pub(crate) fn force_send_failure_for_test(&self) {
        *self.status.write().unwrap() = WebSocketStatus::Open;
        let (tx, rx) = tokio::sync::mpsc::channel::<WsIoCommand>(1);
        drop(rx);
        *self.io.lock().unwrap() = Some(WsIoHandle {
            tx,
            close_notify: Arc::new(Notify::new()),
        });
    }

    pub(crate) async fn pending_queue_len_for_test(&self) -> usize {
        self.queue.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_does_not_block_when_send_queue_full() {
        let ws = TqWebsocket::new("ws://127.0.0.1/".to_string(), WebSocketConfig::default());
        *ws.status.write().unwrap() = WebSocketStatus::Open;

        let (tx, _rx) = tokio::sync::mpsc::channel::<WsIoCommand>(1);
        let close_notify = Arc::new(Notify::new());
        *ws.io.lock().unwrap() = Some(WsIoHandle {
            tx: tx.clone(),
            close_notify: Arc::clone(&close_notify),
        });

        let (resp_tx, _resp_rx) = oneshot::channel();
        tx.try_send(WsIoCommand::SendText {
            payload: r#"{"aid":"test"}"#.to_string(),
            resp: resp_tx,
        })
        .expect("queue should accept the first item");

        let res = tokio::time::timeout(Duration::from_millis(50), ws.close()).await;
        assert!(res.is_ok(), "close() should not block on full send queue");
    }

    #[tokio::test]
    async fn callback_reentrancy_does_not_deadlock() {
        let ws = Arc::new(TqWebsocket::new(
            "ws://127.0.0.1/".to_string(),
            WebSocketConfig::default(),
        ));
        let ws2 = Arc::clone(&ws);
        ws.on_message(move |_value| {
            ws2.on_message(|_value| {});
        });

        let ws3 = Arc::clone(&ws);
        let handle = tokio::task::spawn_blocking(move || {
            ws3.emit_connection_notify(1, "INFO", "x".to_string());
        });

        let res = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(res.is_ok());
        res.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pending_send_queue_is_bounded_while_socket_not_ready() {
        let ws = TqWebsocket::new(
            "ws://127.0.0.1/".to_string(),
            WebSocketConfig {
                message_queue_capacity: 1,
                message_backlog_warn_step: 1,
                ..WebSocketConfig::default()
            },
        );

        for seq in 0..80 {
            ws.send(&json!({"aid":"x","seq":seq})).await.unwrap();
        }

        assert_eq!(ws.pending_queue_len_for_test().await, 64);
    }
}
