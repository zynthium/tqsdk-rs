//! 错误类型定义
//!
//! 使用 thiserror 定义所有错误类型

use reqwest::StatusCode;

/// TQSDK Result 类型
pub type Result<T> = std::result::Result<T, TqError>;

/// TQSDK 错误类型
#[derive(Debug, thiserror::Error)]
pub enum TqError {
    /// 权限不足错误
    #[error("权限不足: {0}")]
    PermissionDenied(String),

    /// WebSocket 连接错误
    #[error("WebSocket 连接错误: {0}")]
    WebSocketError(String),

    /// 认证失败
    #[error("认证失败: {0}")]
    AuthenticationError(String),

    /// 数据解析错误
    #[error("数据解析错误: {0}")]
    ParseError(String),

    /// Reqwest 错误（保留 source）
    #[error("{context}")]
    Reqwest {
        context: String,
        #[source]
        source: reqwest::Error,
    },

    /// HTTP 状态码错误（保留关键信息）
    #[error("HTTP 状态码错误: {method} {url} -> {status}, body={body_snippet}")]
    HttpStatus {
        method: String,
        url: String,
        status: StatusCode,
        body_snippet: String,
    },

    /// JSON 错误（保留 source 与上下文）
    #[error("{context}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    /// JWT 错误（保留 source 与上下文）
    #[error("{context}")]
    Jwt {
        context: String,
        #[source]
        source: jsonwebtoken::errors::Error,
    },

    /// 配置错误
    #[error("配置错误: {0}")]
    ConfigError(String),

    /// 无效的合约代码
    #[error("无效的合约代码: {0}")]
    InvalidSymbol(String),

    /// 无效的参数
    #[error("无效的参数: {0}")]
    InvalidParameter(String),

    /// 交易错误
    #[error("交易错误: {0}")]
    TradeError(String),

    /// 订单不存在
    #[error("订单不存在: {0}")]
    OrderNotFound(String),

    /// 账户未登录
    #[error("账户未登录")]
    NotLoggedIn,

    /// 数据未找到
    #[error("数据未找到: {0}")]
    DataNotFound(String),

    /// 内部错误
    #[error("内部错误: {0}")]
    InternalError(String),

    /// 超时错误
    #[error("操作超时")]
    Timeout,

    /// IO 错误（保留 source）
    #[error("IO 错误")]
    Io(#[from] std::io::Error),

    /// 其他错误
    #[error("错误: {0}")]
    Other(String),

    /// URL 解析错误（保留 source）
    #[error("URL 解析错误")]
    UrlParse(#[from] url::ParseError),
}

// 预定义的错误常量
impl TqError {
    /// 权限不足 - 期货行情
    pub fn permission_denied_futures() -> Self {
        TqError::PermissionDenied(
            "您的账户不支持查看期货行情数据，需要购买后才能使用。\
             升级网址: https://www.shinnytech.com/tqsdk-buy/"
                .to_string(),
        )
    }

    /// 权限不足 - 股票行情
    pub fn permission_denied_stocks() -> Self {
        TqError::PermissionDenied(
            "您的账户不支持查看股票行情数据，需要购买后才能使用。\
             升级网址: https://www.shinnytech.com/tqsdk-buy/"
                .to_string(),
        )
    }

    /// 权限不足 - 历史数据
    pub fn permission_denied_history() -> Self {
        TqError::PermissionDenied(
            "数据获取方式仅限专业版用户使用，如需购买专业版或者申请试用，\
             请访问 https://www.shinnytech.com/tqsdk-buy/"
                .to_string(),
        )
    }

    /// 无效的 left_kline_id
    pub fn invalid_left_kline_id() -> Self {
        TqError::InvalidParameter("left_kline_id 必须大于 0".to_string())
    }

    /// 无效的 focus_position
    pub fn invalid_focus_position() -> Self {
        TqError::InvalidParameter(
            "使用 focus_datetime 时必须提供 focus_position，且 focus_position 必须 >= 0"
                .to_string(),
        )
    }

    pub fn truncate_body(body: String) -> String {
        const LIMIT: usize = 2048;
        if body.len() <= LIMIT {
            return body;
        }
        let mut s = body;
        s.truncate(LIMIT);
        s
    }
}
