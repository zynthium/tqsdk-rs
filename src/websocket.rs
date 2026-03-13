//! WebSocket 连接封装
//!
//! 基于 yawc 库实现 WebSocket 连接，支持：
//! - deflate 压缩
//! - 自动重连
//! - 消息队列
//! - Debug 日志

use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::{Result, TqError};
use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use yawc::frame::{FrameView, OpCode};

type MessageCallback = Arc<RwLock<Option<Box<dyn Fn(Value) + Send + Sync>>>>;
type OpenCallback = Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>;
type CloseCallback = Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>;
type ErrorCallback = Arc<RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>>;
type NotifyCallback = Arc<RwLock<Option<Box<dyn Fn(crate::types::Notification) + Send + Sync>>>>;

struct SharedReconnectTimer {
    next_reconnect_at: Instant,
}

static RECONNECT_TIMER: OnceLock<std::sync::Mutex<SharedReconnectTimer>> = OnceLock::new();

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
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        WebSocketConfig {
            headers: HeaderMap::new(),
            reconnect_interval: Duration::from_secs(3),
            reconnect_max_times: usize::MAX,
            auto_peek: true,
            auto_peek_interval: Duration::ZERO,
            peek_timeout: None,
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
    connecting: Arc<AtomicBool>,
    connected_once: Arc<AtomicBool>,
    pending_peek: Arc<AtomicBool>,
    last_peek_sent: Arc<std::sync::Mutex<std::time::Instant>>,
    connection_id: Arc<RwLock<u64>>,
    last_disconnect_reason: Arc<RwLock<Option<String>>>,

    // WebSocket 连接实例
    ws: Arc<Mutex<Option<yawc::WebSocket>>>,

    // 回调函数
    on_message: MessageCallback,
    on_open: OpenCallback,
    on_close: CloseCallback,
    on_error: ErrorCallback,
}

impl TqWebsocket {
    fn emit_connection_notify(&self, code: i64, level: &str, content: String) {
        if let Some(callback) = self.on_message.read().unwrap().as_ref() {
            callback(build_connection_notify(
                code,
                level,
                content,
                self.url.clone(),
            ));
        }
    }

    /// 创建新的 WebSocket 连接
    pub fn new(url: String, config: WebSocketConfig) -> Self {
        TqWebsocket {
            url,
            config,
            status: Arc::new(RwLock::new(WebSocketStatus::Closed)),
            queue: Arc::new(Mutex::new(Vec::new())),
            reconnect_times: Arc::new(RwLock::new(0)),
            should_reconnect: Arc::new(RwLock::new(true)),
            connecting: Arc::new(AtomicBool::new(false)),
            connected_once: Arc::new(AtomicBool::new(false)),
            pending_peek: Arc::new(AtomicBool::new(false)),
            last_peek_sent: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
            connection_id: Arc::new(RwLock::new(0)),
            last_disconnect_reason: Arc::new(RwLock::new(None)),
            ws: Arc::new(Mutex::new(None)),
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

    /// 初始化连接
    pub async fn init(&self, is_reconnection: bool) -> Result<()> {
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
                self.emit_connection_notify(
                    2019112911,
                    "WARNING",
                    format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", self.url),
                );

                // 尝试重连
                if *self.should_reconnect.read().unwrap() {
                    let _ = self.handle_reconnect().await;
                }

                self.connecting.store(false, Ordering::SeqCst);
                return Err(TqError::WebSocketError(format!("Connection failed: {}", e)));
            }
        };

        // 保存连接实例
        {
            let mut ws_guard = self.ws.lock().await;
            *ws_guard = Some(ws);
        }

        *self.status.write().unwrap() = WebSocketStatus::Open;
        *self.last_disconnect_reason.write().unwrap() = None;

        // 重置重连次数
        if !is_reconnection {
            *self.reconnect_times.write().unwrap() = 0;
        }

        // 触发 onOpen 回调
        if let Some(callback) = self.on_open.read().unwrap().as_ref() {
            callback();
        }
        let first_connected = !self.connected_once.swap(true, Ordering::SeqCst);
        if first_connected && !is_reconnection {
            self.emit_connection_notify(
                2019112901,
                "INFO",
                format!("与 {} 的网络连接已建立", self.url),
            );
        } else {
            self.emit_connection_notify(
                2019112902,
                "WARNING",
                format!("与 {} 的网络连接已恢复", self.url),
            );
        }

        // 启动消息接收循环
        let id = {
            let mut guard = self.connection_id.write().unwrap();
            *guard += 1;
            *guard
        };
        self.start_receive_loop(id).await;
        // 发送队列中的消息
        self.flush_queue().await;
        self.send_peek_message().await?;

        info!("WebSocket 连接成功");
        self.connecting.store(false, Ordering::SeqCst);
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
                        self.set_disconnect_reason(format!("send_frame_failed: {}", e));
                        *self.status.write().unwrap() = WebSocketStatus::Closed;
                        drop(ws_guard);
                        self.emit_connection_notify(
                            2019112911,
                            "WARNING",
                            format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", self.url),
                        );
                        if let Some(callback) = self.on_close.read().unwrap().as_ref() {
                            callback();
                        }
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

    pub fn auto_peek_enabled(&self) -> bool {
        self.config.auto_peek
    }

    pub async fn send_peek_message(&self) -> Result<()> {
        if !self.config.auto_peek {
            return Ok(());
        }
        if self.pending_peek.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let res = self.send(&json!({"aid": "peek_message"})).await;
        if res.is_ok() {
            if let Ok(mut t) = self.last_peek_sent.lock() {
                *t = std::time::Instant::now();
            }
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
        self.set_disconnect_reason("manual_close");
        self.connecting.store(false, Ordering::SeqCst);
        self.pending_peek.store(false, Ordering::SeqCst);

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

        // 复制队列并清空，发送失败时将消息放回队列并触发关闭回调
        let pending = queue.clone();
        queue.clear();
        drop(queue);

        let mut ws_guard = self.ws.lock().await;
        if let Some(ws) = ws_guard.as_mut() {
            for (idx, msg) in pending.iter().enumerate() {
                debug!("发送队列消息: {}", msg);
                let frame = FrameView::text(msg.clone().into_bytes());
                match ws.send(frame).await {
                    Ok(_) => {
                        debug!("队列消息发送成功");
                    }
                    Err(e) => {
                        error!("队列消息发送失败: {}", e);
                        self.set_disconnect_reason(format!("flush_queue_send_failed: {}", e));
                        // 连接异常，恢复状态、回退消息到队列并触发关闭回调
                        *self.status.write().unwrap() = WebSocketStatus::Closed;
                        drop(ws_guard);
                        let mut q = self.queue.lock().await;
                        q.extend_from_slice(&pending[idx..]);
                        drop(q);
                        self.emit_connection_notify(
                            2019112911,
                            "WARNING",
                            format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", self.url),
                        );
                        if let Some(callback) = self.on_close.read().unwrap().as_ref() {
                            callback();
                        }
                        break;
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
    async fn start_receive_loop(&self, connection_id: u64) {
        let status = Arc::clone(&self.status);
        let ws = Arc::clone(&self.ws);
        let status_for_peek = Arc::clone(&self.status);
        let ws_for_peek = Arc::clone(&self.ws);
        let on_message_for_recv = Arc::clone(&self.on_message);
        let on_message_for_peek = Arc::clone(&self.on_message);
        let _on_error = Arc::clone(&self.on_error);
        let _on_close = Arc::clone(&self.on_close);
        let _on_close_for_peek = Arc::clone(&self.on_close);
        let _should_reconnect = Arc::clone(&self.should_reconnect);
        let url_for_recv = self.url.clone();
        let url_for_peek = self.url.clone();
        let auto_peek = self.config.auto_peek;
        let auto_peek_interval = self.config.auto_peek_interval;
        let peek_timeout = self.config.peek_timeout;
        let pending_peek_for_recv = Arc::clone(&self.pending_peek);
        let pending_peek_for_timer = Arc::clone(&self.pending_peek);
        let disconnect_reason_for_recv = Arc::clone(&self.last_disconnect_reason);
        let disconnect_reason_for_peek = Arc::clone(&self.last_disconnect_reason);
        // last_recv_time: 上次收到消息的时间
        let last_recv_time = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        let last_recv_time_for_recv = Arc::clone(&last_recv_time);
        let last_recv_time_for_timer = Arc::clone(&last_recv_time);
        let current_connection_id = Arc::clone(&self.connection_id);
        let current_connection_id_for_peek = Arc::clone(&self.connection_id);
        let last_peek_sent = Arc::clone(&self.last_peek_sent);
        let last_peek_sent_for_timer = Arc::clone(&self.last_peek_sent);

        tokio::spawn(async move {
            debug!("启动 WebSocket 消息接收循环");
            let mut consecutive_none = 0u8;

            loop {
                if *current_connection_id.read().unwrap() != connection_id {
                    debug!("WebSocket 连接 ID 不匹配，退出接收循环");
                    break;
                }
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
                    let timeout_duration = tokio::time::Duration::from_secs(15);
                    let next_result =
                        tokio::time::timeout(timeout_duration, ws_instance.next()).await;

                    match next_result {
                    Ok(Some(frame)) => {
                        consecutive_none = 0;
                        // 收到任何消息都更新 last_recv_time
                        if let Ok(mut t) = last_recv_time_for_recv.lock() {
                            *t = std::time::Instant::now();
                        }
                        if auto_peek {
                            pending_peek_for_recv.store(false, Ordering::SeqCst);
                        }

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
                                                    if let Some(callback) =
                                                        on_message_for_recv.read().unwrap().as_ref()
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

                                    if auto_peek
                                        && !pending_peek_for_recv.swap(true, Ordering::SeqCst)
                                    {
                                        let frame =
                                            FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes());
                                        match ws_instance.send(frame).await {
                                            Ok(_) => {
                                                if let Ok(mut t) = last_peek_sent.lock() {
                                                    *t = std::time::Instant::now();
                                                }
                                                debug!("Websocket Send -> peek_message");
                                            }
                                            Err(e) => {
                                                pending_peek_for_recv
                                                    .store(false, Ordering::SeqCst);
                                                *disconnect_reason_for_recv.write().unwrap() =
                                                    Some(format!(
                                                        "recv_loop_send_peek_failed: {}",
                                                        e
                                                    ));
                                                error!(
                                                    "Websocket Send `peek_message` failed: {}",
                                                    e
                                                );
                                                *status.write().unwrap() = WebSocketStatus::Closed;
                                                if let Some(callback) =
                                                    on_message_for_recv.read().unwrap().as_ref()
                                                {
                                                    callback(build_connection_notify(
                                                        2019112911,
                                                        "WARNING",
                                                        format!(
                                                            "与 {} 的网络连接断开，请检查客户端及网络是否正常",
                                                            url_for_recv
                                                        ),
                                                        url_for_recv.clone(),
                                                    ));
                                                }
                                                if let Some(callback) = _on_close.read().unwrap().as_ref() {
                                                    callback();
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                                OpCode::Close => {
                                    *disconnect_reason_for_recv.write().unwrap() =
                                        Some("server_close_frame".to_string());
                                    info!("WebSocket 收到关闭帧");
                                    *status.write().unwrap() = WebSocketStatus::Closed;
                                    if let Some(callback) = on_message_for_recv.read().unwrap().as_ref() {
                                        callback(build_connection_notify(
                                            2019112911,
                                            "WARNING",
                                            format!(
                                                "与 {} 的网络连接断开，请检查客户端及网络是否正常",
                                                url_for_recv
                                            ),
                                            url_for_recv.clone(),
                                        ));
                                    }

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
                            consecutive_none = consecutive_none.saturating_add(1);
                            if consecutive_none < 3 {
                                warn!(
                                    "WebSocket next() 返回 None（第 {} 次），等待确认后再断线处理",
                                    consecutive_none
                                );
                                drop(ws_guard);
                                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                                continue;
                            }
                            let probe = FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes());
                            match ws_instance.send(probe).await {
                                Ok(_) => {
                                    if let Ok(mut t) = last_peek_sent.lock() {
                                        *t = std::time::Instant::now();
                                    }
                                    pending_peek_for_recv.store(true, Ordering::SeqCst);
                                    consecutive_none = 0;
                                    warn!(
                                        "WebSocket next() 连续返回 None，但探测发送成功，继续保持连接"
                                    );
                                    drop(ws_guard);
                                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                                    continue;
                                }
                                Err(e) => {
                                    *disconnect_reason_for_recv.write().unwrap() =
                                        Some(format!("stream_ended_probe_send_failed: {}", e));
                                }
                            }
                            info!("WebSocket Stream 结束，连接已关闭");
                            *status.write().unwrap() = WebSocketStatus::Closed;
                            if let Some(callback) = on_message_for_recv.read().unwrap().as_ref() {
                                callback(build_connection_notify(
                                    2019112911,
                                    "WARNING",
                                    format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", url_for_recv),
                                    url_for_recv.clone(),
                                ));
                            }

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
            if !auto_peek || auto_peek_interval.is_zero() {
                return;
            }
            let mut ticker = tokio::time::interval(auto_peek_interval);
            loop {
                ticker.tick().await;

                if *current_connection_id_for_peek.read().unwrap() != connection_id {
                    debug!("WebSocket 连接 ID 不匹配，退出 peek 循环");
                    break;
                }

                let current_status = *status_for_peek.read().unwrap();
                if current_status != WebSocketStatus::Open {
                    break;
                }
                if pending_peek_for_timer.load(Ordering::SeqCst) {
                    if let Some(timeout) = peek_timeout {
                        let last_recv_elapsed = last_recv_time_for_timer
                            .lock()
                            .ok()
                            .map(|t| t.elapsed());
                        let last_peek_elapsed = last_peek_sent_for_timer
                            .lock()
                            .ok()
                            .map(|t| t.elapsed());
                        if let (Some(recv_elapsed), Some(peek_elapsed)) =
                            (last_recv_elapsed, last_peek_elapsed)
                        {
                            if recv_elapsed > timeout && peek_elapsed > timeout {
                                pending_peek_for_timer.store(false, Ordering::SeqCst);
                                continue;
                            }
                        }
                        continue;
                    } else {
                        continue;
                    }
                }

                // 主动发送 peek message
                let mut ws_guard = ws_for_peek.lock().await;
                if let Some(ws_instance) = ws_guard.as_mut() {
                    // 如果已经有 pending 的 peek（在获取锁的过程中被设置），则跳过
                    if pending_peek_for_timer.swap(true, Ordering::SeqCst) {
                        drop(ws_guard);
                        continue;
                    }
                    let frame = FrameView::text(r#"{"aid": "peek_message"}"#.as_bytes());
                    if let Err(e) = ws_instance.send(frame).await {
                        error!("Websocket Send `peek_message` failed: {}", e);
                        *disconnect_reason_for_peek.write().unwrap() =
                            Some(format!("timer_send_peek_failed: {}", e));
                        *status_for_peek.write().unwrap() = WebSocketStatus::Closed;
                        pending_peek_for_timer.store(false, Ordering::SeqCst);
                        if let Some(callback) = on_message_for_peek.read().unwrap().as_ref() {
                            callback(build_connection_notify(
                                2019112911,
                                "WARNING",
                                format!("与 {} 的网络连接断开，请检查客户端及网络是否正常", url_for_peek),
                                url_for_peek.clone(),
                            ));
                        }
                        if let Some(callback) = _on_close_for_peek.read().unwrap().as_ref() {
                            callback();
                        }
                        break;
                    }
                    if let Ok(mut t) = last_peek_sent_for_timer.lock() {
                        *t = std::time::Instant::now();
                    }
                    debug!("Websocket Timer Send -> peek_message");
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

            let wait_duration = next_shared_reconnect_delay(times as u32, self.config.reconnect_interval);
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
            if !self.handle_reconnect().await {
                error!("已达到最大重连次数，放弃重连");
                break;
            }

            match self.init(true).await {
                Ok(_) => {
                    info!("重连成功");
                    // 重置重连次数
                    *self.reconnect_times.write().unwrap() = 0;
                    break;
                }
                Err(e) => {
                    error!("重连失败: {}, 继续尝试...", e);
                }
            }
        }
    }
}

fn extract_notify_code(value: &Value) -> Option<i64> {
    if let Some(code) = value.as_i64() {
        return Some(code);
    }
    value
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
}

fn build_connection_notify(code: i64, level: &str, content: String, url: String) -> Value {
    let notify_id = uuid::Uuid::new_v4().to_string();
    json!({
        "aid": "rtn_data",
        "data": [{
            "notify": {
                notify_id: {
                    "type": "MESSAGE",
                    "level": level,
                    "code": code,
                    "content": content,
                    "url": url
                }
            }
        }]
    })
}

fn next_shared_reconnect_delay(reconnect_count: u32, fallback: Duration) -> Duration {
    let timer = RECONNECT_TIMER.get_or_init(|| {
        let initial = Duration::from_secs(10 + pseudo_jitter_seconds(11));
        std::sync::Mutex::new(SharedReconnectTimer {
            next_reconnect_at: Instant::now() + initial,
        })
    });
    let now = Instant::now();
    let mut guard = timer.lock().unwrap();
    let wait = if guard.next_reconnect_at > now {
        guard.next_reconnect_at.duration_since(now)
    } else {
        Duration::ZERO
    };

    if guard.next_reconnect_at <= now {
        let exp = reconnect_count.min(6);
        let base_secs = (1u64 << exp) * 10;
        let lower = Duration::from_secs(base_secs).max(fallback);
        let upper = lower.saturating_mul(2);
        let jitter = pseudo_duration_between(lower, upper);
        guard.next_reconnect_at = now + jitter;
    }

    wait.max(fallback)
}

fn pseudo_duration_between(lower: Duration, upper: Duration) -> Duration {
    if upper <= lower {
        return lower;
    }
    let range = upper - lower;
    let nanos = range.as_nanos() as u64;
    if nanos == 0 {
        return lower;
    }
    let offset = pseudo_jitter_nanos(nanos);
    lower + Duration::from_nanos(offset)
}

fn pseudo_jitter_seconds(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    (now.as_nanos() as u64) % max
}

fn pseudo_jitter_nanos(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    (now.as_nanos() as u64) % max
}

fn has_reconnect_notify(item: &Value) -> bool {
    let notify = match item.get("notify").and_then(|v| v.as_object()) {
        Some(notify) => notify,
        None => return false,
    };
    notify.values().any(|n| {
        n.get("code")
            .and_then(extract_notify_code)
            .map(|code| code == 2019112902)
            .unwrap_or(false)
    })
}

fn get_i64(value: Option<&Value>, default: i64) -> i64 {
    value
        .and_then(|v| v.as_i64())
        .or_else(|| value.and_then(|v| v.as_str()?.parse::<i64>().ok()))
        .unwrap_or(default)
}

fn get_bool(value: Option<&Value>, default: bool) -> bool {
    value
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

fn is_md_reconnect_complete(
    dm: &DataManager,
    charts: &HashMap<String, Value>,
    subscribe_quote: &Option<Value>,
) -> bool {
    let set_chart_packs: Vec<(&String, &Value)> = charts
        .iter()
        .filter(|(_, v)| {
            v.get("aid").and_then(|v| v.as_str()) == Some("set_chart")
                && v.get("ins_list")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
        })
        .collect();

    for (chart_id, req) in set_chart_packs.iter() {
        let state = match dm.get_by_path(&["charts", chart_id.as_str(), "state"]) {
            Some(Value::Object(state)) => state,
            _ => return false,
        };
        for key in [
            "ins_list",
            "duration",
            "view_width",
            "left_kline_id",
            "focus_datetime",
            "focus_position",
        ] {
            if let Some(req_val) = req.get(key) {
                if state.get(key) != Some(req_val) {
                    return false;
                }
            }
        }

        let chart = match dm.get_by_path(&["charts", chart_id.as_str()]) {
            Some(Value::Object(chart)) => chart,
            _ => return false,
        };
        let left_id = get_i64(chart.get("left_id"), -1);
        let right_id = get_i64(chart.get("right_id"), -1);
        let more_data = get_bool(chart.get("more_data"), true);
        if left_id == -1 && right_id == -1 {
            return false;
        }
        if more_data {
            return false;
        }
        if get_bool(dm.get_by_path(&["mdhis_more_data"]).as_ref(), true) {
            return false;
        }

        let ins_list = req.get("ins_list").and_then(|v| v.as_str()).unwrap_or("");
        let duration = get_i64(req.get("duration"), -1);
        for symbol in ins_list.split(',').filter(|s| !s.is_empty()) {
            let last_id = if duration == 0 {
                dm.get_by_path(&["ticks", symbol])
                    .and_then(|v| v.get("last_id").cloned())
            } else {
                let duration_str = duration.to_string();
                dm.get_by_path(&["klines", symbol, &duration_str])
                    .and_then(|v| v.get("last_id").cloned())
            };
            if get_i64(last_id.as_ref(), -1) == -1 {
                return false;
            }
        }
    }

    if let Some(sub) = subscribe_quote.as_ref() {
        if let Some(sub_ins_list) = sub.get("ins_list").and_then(|v| v.as_str()) {
            match dm.get_by_path(&["ins_list"]) {
                Some(Value::String(data_ins_list)) => {
                    if data_ins_list != sub_ins_list {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    true
}

fn extract_trade_positions(dm: &DataManager) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();
    let trade = match dm.get_by_path(&["trade"]) {
        Some(Value::Object(trade)) => trade,
        _ => return result,
    };
    for (user, value) in trade.iter() {
        if let Value::Object(user_obj) = value {
            if let Some(Value::Object(positions)) = user_obj.get("positions") {
                let symbols = positions.keys().cloned().collect::<HashSet<_>>();
                result.insert(user.to_string(), symbols);
            }
        }
    }
    result
}

fn trade_users_from_dm(dm: &DataManager) -> Vec<String> {
    let trade = match dm.get_by_path(&["trade"]) {
        Some(Value::Object(trade)) => trade,
        _ => return Vec::new(),
    };
    trade.keys().cloned().collect()
}

fn is_trade_reconnect_complete(
    dm: &DataManager,
    prev_positions: &HashMap<String, HashSet<String>>,
) -> Option<Vec<Value>> {
    let users = if prev_positions.is_empty() {
        trade_users_from_dm(dm)
    } else {
        prev_positions.keys().cloned().collect()
    };
    if users.is_empty() {
        return Some(Vec::new());
    }
    for user in users.iter() {
        let more_data = get_bool(
            dm.get_by_path(&["trade", user, "trade_more_data"])
                .as_ref(),
            true,
        );
        if more_data {
            return None;
        }
    }
    let current_positions = extract_trade_positions(dm);
    let mut removal_diffs = Vec::new();
    for (user, prev) in prev_positions.iter() {
        if let Some(current) = current_positions.get(user) {
            let removed: Vec<String> = prev.difference(current).cloned().collect();
            if !removed.is_empty() {
                let mut positions = serde_json::Map::new();
                for symbol in removed {
                    positions.insert(symbol, Value::Null);
                }
                let mut user_map = serde_json::Map::new();
                user_map.insert("positions".to_string(), Value::Object(positions));
                let mut trade_map = serde_json::Map::new();
                trade_map.insert(user.clone(), Value::Object(user_map));
                let mut root = serde_json::Map::new();
                root.insert("trade".to_string(), Value::Object(trade_map));
                removal_diffs.push(Value::Object(root));
            }
        }
    }
    Some(removal_diffs)
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
        let reconnect_pending = Arc::new(AtomicBool::new(false));
        let reconnect_diffs: Arc<RwLock<Vec<Value>>> = Arc::new(RwLock::new(Vec::new()));
        let reconnect_dm: Arc<RwLock<Option<Arc<DataManager>>>> = Arc::new(RwLock::new(None));

        // 注册消息处理
        base.on_message({
            let dm = Arc::clone(&dm_clone);
            let pending_ins_query = Arc::clone(&pending_ins_query);
            let login_ready = Arc::clone(&login_ready);
            let charts_clone = Arc::clone(&charts);
            let subscribe_quote_clone = Arc::clone(&subscribe_quote);
            let reconnect_pending = Arc::clone(&reconnect_pending);
            let reconnect_diffs = Arc::clone(&reconnect_diffs);
            let reconnect_dm = Arc::clone(&reconnect_dm);
            let base_clone = Arc::clone(&base);
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

                                    let reconnect_index =
                                        array.iter().position(has_reconnect_notify);
                                    if let Some(index) = reconnect_index {
                                        reconnect_pending.store(true, Ordering::SeqCst);
                                        let mut diffs = reconnect_diffs.write().unwrap();
                                        diffs.clear();
                                        diffs.extend(array[index..].iter().cloned());
                                        let dm_temp = Arc::new(DataManager::new(
                                            HashMap::new(),
                                            DataManagerConfig::default(),
                                        ));
                                        dm_temp.merge_data(Value::Array(diffs.clone()), true, true);
                                        *reconnect_dm.write().unwrap() = Some(Arc::clone(&dm_temp));
                                        let sub = subscribe_quote_clone.read().unwrap().clone();
                                        let charts = charts_clone.read().unwrap().clone();
                                        let base_for_send = Arc::clone(&base_clone);
                                        tokio::spawn(async move {
                                            if let Some(sub) = sub {
                                                let _ = base_for_send.send(&sub).await;
                                            }
                                            for chart in charts.values() {
                                                if let Some(view_width) =
                                                    chart.get("view_width").and_then(|v| v.as_f64())
                                                {
                                                    if view_width > 0.0 {
                                                        let _ = base_for_send.send(chart).await;
                                                    }
                                                }
                                            }
                                            let _ = base_for_send.send_peek_message().await;
                                        });
                                    } else if reconnect_pending.load(Ordering::SeqCst) {
                                        let mut diffs = reconnect_diffs.write().unwrap();
                                        diffs.extend(array.iter().cloned());
                                        if let Some(dm_temp) =
                                            reconnect_dm.read().unwrap().as_ref().cloned()
                                        {
                                            dm_temp.merge_data(
                                                Value::Array(array.clone()),
                                                true,
                                                true,
                                            );
                                        }
                                    }

                                    if reconnect_pending.load(Ordering::SeqCst) {
                                        let dm_temp =
                                            reconnect_dm.read().unwrap().as_ref().cloned();
                                        let charts_snapshot =
                                            charts_clone.read().unwrap().clone();
                                        let subscribe_snapshot =
                                            subscribe_quote_clone.read().unwrap().clone();
                                        if let Some(dm_temp) = dm_temp {
                                            if is_md_reconnect_complete(
                                                &dm_temp,
                                                &charts_snapshot,
                                                &subscribe_snapshot,
                                            ) {
                                                let mut diffs =
                                                    reconnect_diffs.write().unwrap();
                                                let pending = diffs.clone();
                                                diffs.clear();
                                                reconnect_pending
                                                    .store(false, Ordering::SeqCst);
                                                *reconnect_dm.write().unwrap() = None;
                                                dm.merge_data(
                                                    Value::Array(pending),
                                                    true,
                                                    true,
                                                );
                                            } else {
                                                let base_for_peek =
                                                    Arc::clone(&base_clone);
                                                tokio::spawn(async move {
                                                    let _ = base_for_peek
                                                        .send_peek_message()
                                                        .await;
                                                });
                                            }
                                        }
                                        return;
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

        {
            let base_clone = Arc::clone(&base);
            let reconnect_pending = Arc::clone(&reconnect_pending);
            let reconnect_diffs = Arc::clone(&reconnect_diffs);
            let reconnect_dm = Arc::clone(&reconnect_dm);
            let subscribe_quote = Arc::clone(&subscribe_quote);
            let charts = Arc::clone(&charts);
            let pending_ins_query = Arc::clone(&pending_ins_query);
            base.on_close(move || {
                reconnect_pending.store(false, Ordering::SeqCst);
                reconnect_diffs.write().unwrap().clear();
                *reconnect_dm.write().unwrap() = None;
                let has_quote_interest = subscribe_quote
                    .read()
                    .unwrap()
                    .as_ref()
                    .and_then(|v| v.get("ins_list").and_then(|s| s.as_str()))
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                let has_chart_interest = charts.read().unwrap().values().any(|chart| {
                    let width_ok = chart
                        .get("view_width")
                        .and_then(|v| v.as_f64())
                        .map(|w| w > 0.0)
                        .unwrap_or(false);
                    let symbols_ok = chart
                        .get("ins_list")
                        .and_then(|v| v.as_str())
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false);
                    width_ok && symbols_ok
                });
                let has_pending_query = !pending_ins_query.read().unwrap().is_empty();
                let should_reconnect = has_quote_interest || has_chart_interest || has_pending_query;
                if !should_reconnect {
                    info!("无活跃订阅意图，跳过自动重连");
                    return;
                }
                let base_for_reconnect = Arc::clone(&base_clone);
                tokio::spawn(async move {
                    base_for_reconnect.reconnect().await;
                });
            });
        }

        {
            let base_clone = Arc::clone(&base);
            let subscribe_quote = Arc::clone(&subscribe_quote);
            let charts = Arc::clone(&charts);
            let pending_ins_query = Arc::clone(&pending_ins_query);
            base.on_open(move || {
                debug!("WebSocket 连接建立，重新发送订阅和图表请求");
                let base = Arc::clone(&base_clone);
                let sub = subscribe_quote.read().unwrap().clone();
                let charts = charts.read().unwrap().clone();
                let pending_queries = pending_ins_query.read().unwrap().clone();
                tokio::spawn(async move {
                    // 重新发送订阅
                    if let Some(sub) = sub {
                        debug!("重新发送订阅: {:?}", sub);
                        let _ = base.send(&sub).await;
                    }
                    // 重新发送图表
                    for (id, chart) in charts {
                        debug!("重新发送图表: {} -> {:?}", id, chart);
                        let _ = base.send(&chart).await;
                    }
                    // 重新发送未完成的合约查询
                    for (id, query) in pending_queries {
                        debug!("重新发送合约查询: {} -> {:?}", id, query);
                        let _ = base.send(&query).await;
                    }
                    // 发送 peek
                    let _ = base.send_peek_message().await;
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
                    let should_send = {
                        let mut subscribe_guard = self.subscribe_quote.write().unwrap();
                        let mut should = false;

                        if let Some(old_sub) = subscribe_guard.as_ref() {
                            // 比较 ins_list
                            let old_list = old_sub.get("ins_list");
                            let new_list = value.get("ins_list");

                            if old_list != new_list {
                                debug!("订阅列表变化，更新订阅");
                                *subscribe_guard = Some(value.clone());
                                should = true;
                            } else {
                                debug!("订阅列表未变化，跳过");
                            }
                        } else {
                            debug!("首次订阅");
                            *subscribe_guard = Some(value.clone());
                            should = true;
                        }
                        should
                    };

                    if should_send {
                        let res = self.base.send(&value).await;
                        if res.is_ok() {
                            let _ = self.base.send_peek_message().await;
                        }
                        return res;
                    }
                    let _ = self.base.send_peek_message().await;
                    return Ok(());
                }
                "set_chart" => {
                    // 记录图表请求
                    if let Some(chart_id) = value.get("chart_id").and_then(|v| v.as_str()) {
                        {
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
                        }

                        let res = self.base.send(&value).await;
                        if res.is_ok() {
                            let _ = self.base.send_peek_message().await;
                        }
                        return res;
                    }
                }
                "ins_query" => {
                    if let Some(query_id) = value.get("query_id").and_then(|v| v.as_str()) {
                        {
                            let mut pending_guard = self.pending_ins_query.write().unwrap();
                            pending_guard.insert(query_id.to_string(), value.clone());
                        }
                    }
                    let res = self.base.send(&value).await;
                    if res.is_ok() {
                        let _ = self.base.send_peek_message().await;
                    }
                    return res;
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
            let subscribe_trading_status = Arc::clone(&subscribe_trading_status);
            let base_clone = Arc::clone(&base);
            move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|v| v.as_str()) {
                    if aid == "rtn_data" {
                        if let Some(payload) = data.get("data").and_then(|v| v.as_array()) {
                            if payload.iter().any(has_reconnect_notify) {
                                let sub = subscribe_trading_status.read().unwrap().clone();
                                let base_for_send = Arc::clone(&base_clone);
                                tokio::spawn(async move {
                                    if let Some(sub) = sub {
                                        let _ = base_for_send.send(&sub).await;
                                    }
                                    let _ = base_for_send.send_peek_message().await;
                                });
                            }
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
            let base_clone = Arc::clone(&base);
            base.on_close(move || {
                let base_for_reconnect = Arc::clone(&base_clone);
                tokio::spawn(async move {
                    base_for_reconnect.reconnect().await;
                });
            });
        }

        {
            let base_clone = Arc::clone(&base);
            let subscribe_trading_status = Arc::clone(&subscribe_trading_status);
            base.on_open(move || {
                let base = Arc::clone(&base_clone);
                let sub = subscribe_trading_status.read().unwrap().clone();
                tokio::spawn(async move {
                    if let Some(sub) = sub {
                         let _ = base.send(&sub).await;
                    }
                    let _ = base.send_peek_message().await;
                });
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
                {
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
                }
                if should_send {
                    let res = self.base.send(&value).await;
                    if res.is_ok() {
                        let _ = self.base.send_peek_message().await;
                    }
                    return res;
                }
                let _ = self.base.send_peek_message().await;
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
    confirm_settlement: Arc<RwLock<Option<Value>>>,
    on_notify: NotifyCallback,
}

impl TqTradeWebsocket {
    /// 创建交易 WebSocket
    pub fn new(url: String, dm: Arc<DataManager>, config: WebSocketConfig) -> Self {
        let base = Arc::new(TqWebsocket::new(url, config));
        let dm_clone = Arc::clone(&dm);
        let req_login: Arc<RwLock<Option<Value>>> = Arc::new(RwLock::new(None));
        let confirm_settlement: Arc<RwLock<Option<Value>>> = Arc::new(RwLock::new(None));
        let on_notify: NotifyCallback = Arc::new(RwLock::new(None));
        let reconnect_pending = Arc::new(AtomicBool::new(false));
        let reconnect_diffs: Arc<RwLock<Vec<Value>>> = Arc::new(RwLock::new(Vec::new()));
        let reconnect_dm: Arc<RwLock<Option<Arc<DataManager>>>> = Arc::new(RwLock::new(None));
        let reconnect_prev_positions: Arc<RwLock<HashMap<String, HashSet<String>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // 注册消息处理
        {
            let dm = Arc::clone(&dm_clone);
            let on_notify_clone = Arc::clone(&on_notify);
            let reconnect_pending = Arc::clone(&reconnect_pending);
            let reconnect_diffs = Arc::clone(&reconnect_diffs);
            let reconnect_dm = Arc::clone(&reconnect_dm);
            let reconnect_prev_positions = Arc::clone(&reconnect_prev_positions);
            let base_clone = Arc::clone(&base);
            let req_login_clone = Arc::clone(&req_login);
            let confirm_settlement_clone = Arc::clone(&confirm_settlement);

            base.on_message(move |data: Value| {
                if let Some(aid) = data.get("aid").and_then(|v| v.as_str()) {
                    match aid {
                        "rtn_data" => {
                            if let Some(payload) = data.get("data") {
                                // 分离通知
                                if let Some(array) = payload.as_array() {
                                    let reconnect_index =
                                        array.iter().position(has_reconnect_notify);
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

                                    if let Some(index) = reconnect_index {
                                        reconnect_pending.store(true, Ordering::SeqCst);
                                        *reconnect_prev_positions.write().unwrap() =
                                            extract_trade_positions(&dm);
                                        let mut diffs = reconnect_diffs.write().unwrap();
                                        diffs.clear();
                                        diffs.extend(cleaned_data[index..].iter().cloned());
                                        let dm_temp = Arc::new(DataManager::new(
                                            HashMap::new(),
                                            DataManagerConfig::default(),
                                        ));
                                        dm_temp.merge_data(Value::Array(diffs.clone()), true, true);
                                        *reconnect_dm.write().unwrap() = Some(Arc::clone(&dm_temp));
                                        let base_for_send = Arc::clone(&base_clone);
                                        let req_login = req_login_clone.read().unwrap().clone();
                                        let confirm_settlement =
                                            confirm_settlement_clone.read().unwrap().clone();
                                        tokio::spawn(async move {
                                            if let Some(login) = req_login {
                                                let _ = base_for_send.send(&login).await;
                                            }
                                            if let Some(confirm) = confirm_settlement {
                                                let _ = base_for_send.send(&confirm).await;
                                            }
                                            let _ = base_for_send.send_peek_message().await;
                                        });
                                    } else if reconnect_pending.load(Ordering::SeqCst) {
                                        let mut diffs = reconnect_diffs.write().unwrap();
                                        diffs.extend(cleaned_data.iter().cloned());
                                        if let Some(dm_temp) =
                                            reconnect_dm.read().unwrap().as_ref().cloned()
                                        {
                                            dm_temp.merge_data(
                                                Value::Array(cleaned_data.clone()),
                                                true,
                                                true,
                                            );
                                        }
                                    }

                                    if reconnect_pending.load(Ordering::SeqCst) {
                                        let dm_temp =
                                            reconnect_dm.read().unwrap().as_ref().cloned();
                                        let prev_positions =
                                            reconnect_prev_positions.read().unwrap().clone();
                                        if let Some(dm_temp) = dm_temp {
                                            if let Some(removal_diffs) =
                                                is_trade_reconnect_complete(
                                                    &dm_temp,
                                                    &prev_positions,
                                                )
                                            {
                                                let mut diffs =
                                                    reconnect_diffs.write().unwrap();
                                                let mut pending = diffs.clone();
                                                diffs.clear();
                                                reconnect_pending
                                                    .store(false, Ordering::SeqCst);
                                                *reconnect_dm.write().unwrap() = None;
                                                if !removal_diffs.is_empty() {
                                                    pending.extend(removal_diffs);
                                                }
                                                dm.merge_data(
                                                    Value::Array(pending),
                                                    true,
                                                    true,
                                                );
                                            } else {
                                                let base_for_peek =
                                                    Arc::clone(&base_clone);
                                                tokio::spawn(async move {
                                                    let _ = base_for_peek
                                                        .send_peek_message()
                                                        .await;
                                                });
                                            }
                                        }
                                        return;
                                    }

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

        {
            let base_clone = Arc::clone(&base);
            let reconnect_pending = Arc::clone(&reconnect_pending);
            let reconnect_diffs = Arc::clone(&reconnect_diffs);
            let reconnect_dm = Arc::clone(&reconnect_dm);
            let reconnect_prev_positions = Arc::clone(&reconnect_prev_positions);
            base.on_close(move || {
                reconnect_pending.store(false, Ordering::SeqCst);
                reconnect_diffs.write().unwrap().clear();
                *reconnect_dm.write().unwrap() = None;
                reconnect_prev_positions.write().unwrap().clear();
                let base_for_reconnect = Arc::clone(&base_clone);
                tokio::spawn(async move {
                    base_for_reconnect.reconnect().await;
                });
            });
        }

        {
            let base_clone = Arc::clone(&base);
            let req_login = Arc::clone(&req_login);
            let confirm_settlement = Arc::clone(&confirm_settlement);
            base.on_open(move || {
                let base = Arc::clone(&base_clone);
                let login = req_login.read().unwrap().clone();
                let confirm = confirm_settlement.read().unwrap().clone();
                tokio::spawn(async move {
                    if let Some(login) = login {
                        let _ = base.send(&login).await;
                    }
                    if let Some(confirm) = confirm {
                        let _ = base.send(&confirm).await;
                    }
                    let _ = base.send_peek_message().await;
                });
            });
        }

        TqTradeWebsocket {
            base,
            _dm: dm_clone,
            req_login,
            confirm_settlement,
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
                                        .and_then(extract_notify_code)
                                        .map(|v| v.to_string())
                                        .unwrap_or_default(),
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
            } else if aid == "confirm_settlement" {
                *self.confirm_settlement.write().unwrap() = Some(value.clone());
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
