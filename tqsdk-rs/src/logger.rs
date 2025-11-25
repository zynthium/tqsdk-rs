//! 日志系统
//!
//! 使用 tracing 实现日志系统，支持：
//! - 多级别日志（debug, info, warn, error）
//! - 日志过滤（只显示本库日志）
//! - WebSocket 消息详细日志

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Debug 级别
    Debug,
    /// Info 级别
    Info,
    /// Warn 级别
    Warn,
    /// Error 级别
    Error,
}

impl LogLevel {
    /// 转换为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

impl From<&str> for LogLevel {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

/// 初始化日志系统
///
/// # 参数
///
/// * `level` - 日志级别（debug, info, warn, error）
/// * `filter_crate_only` - 是否只显示本库的日志
///
/// # 示例
///
/// ```no_run
/// use tqsdk_rs::logger;
///
/// // 只显示本库的 debug 日志
/// logger::init_logger("debug", true);
/// ```
pub fn init_logger(level: &str, filter_crate_only: bool) {
    let log_level = LogLevel::from(level);

    // 构建过滤器
    let filter = if filter_crate_only {
        // 只显示本库的日志
        EnvFilter::new(format!("tqsdk_rs={}", log_level.as_str()))
    } else {
        // 显示所有日志
        EnvFilter::new(log_level.as_str())
    };

    // 配置格式化输出
    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(true)
        .compact();

    // 初始化全局订阅者
    // 使用 try_init 避免重复初始化时 panic
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init();
}

/// 初始化日志系统（带默认值）
///
/// 默认为 info 级别，只显示本库日志
pub fn init_default_logger() {
    init_logger("info", true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from("DEBUG"), LogLevel::Debug);
        assert_eq!(LogLevel::from("info"), LogLevel::Info);
        assert_eq!(LogLevel::from("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from("error"), LogLevel::Error);
        assert_eq!(LogLevel::from("unknown"), LogLevel::Info); // 默认
    }

    #[test]
    fn test_log_level_as_str() {
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }
}
