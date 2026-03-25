//! 认证模块
//!
//! 实现天勤账号认证和权限检查

mod core;
mod permissions;
mod token;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

/// 版本号
pub const VERSION: &str = "3.8.1";
pub const TQ_AUTH_URL: &str = "https://auth.shinnytech.com";
pub const TQ_NS_URL: &str = "https://api.shinnytech.com/ns";
pub const TQ_MD_URL_ENV: &str = "TQ_MD_URL";
/// 客户端 ID
pub const CLIENT_ID: &str = "shinny_tq";
/// 客户端密钥
pub const CLIENT_SECRET: &str = "be30b9f4-6862-488a-99ad-21bde0400081";

#[derive(Debug, Clone)]
pub struct TqAuthConfig {
    pub auth_url: String,
    pub ns_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub proxy: Option<String>,
    pub no_proxy: bool,
    pub verify_jwt: bool,
    pub http_timeout: Duration,
}

impl Default for TqAuthConfig {
    fn default() -> Self {
        let auth_url = std::env::var("TQ_AUTH_URL").unwrap_or_else(|_| TQ_AUTH_URL.to_string());
        let ns_url = std::env::var("TQ_NS_URL").unwrap_or_else(|_| TQ_NS_URL.to_string());
        let client_id = std::env::var("TQ_CLIENT_ID").unwrap_or_else(|_| CLIENT_ID.to_string());
        let client_secret =
            std::env::var("TQ_CLIENT_SECRET").unwrap_or_else(|_| CLIENT_SECRET.to_string());
        let proxy = std::env::var("TQ_AUTH_PROXY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let no_proxy = matches!(
            std::env::var("TQ_AUTH_NO_PROXY").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        );
        let verify_jwt = matches!(
            std::env::var("TQ_AUTH_VERIFY_JWT").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        );
        let http_timeout_secs: u64 = std::env::var("TQ_HTTP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);

        TqAuthConfig {
            auth_url,
            ns_url,
            client_id,
            client_secret,
            proxy,
            no_proxy,
            verify_jwt,
            http_timeout: Duration::from_secs(http_timeout_secs),
        }
    }
}

/// 认证器接口
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// 获取包含认证信息的 HTTP Header
    fn base_header(&self) -> HeaderMap;

    /// 执行登录操作
    async fn login(&mut self) -> crate::errors::Result<()>;

    /// 获取指定期货公司的交易服务器地址
    async fn get_td_url(
        &self,
        broker_id: &str,
        account_id: &str,
    ) -> crate::errors::Result<BrokerInfo>;

    /// 获取行情服务器地址
    async fn get_md_url(&self, stock: bool, backtest: bool) -> crate::errors::Result<String>;

    /// 检查是否具有指定的功能权限
    fn has_feature(&self, feature: &str) -> bool;

    /// 检查是否有查看指定合约行情数据的权限
    fn has_md_grants(&self, symbols: &[&str]) -> crate::errors::Result<()>;

    /// 检查是否有交易指定合约的权限
    fn has_td_grants(&self, symbol: &str) -> crate::errors::Result<()>;

    /// 获取认证 ID
    fn get_auth_id(&self) -> &str;

    /// 获取访问令牌
    fn get_access_token(&self) -> &str;
}

/// 权限信息
#[derive(Debug, Clone, Default)]
pub struct Grants {
    /// 功能权限
    pub features: HashSet<String>,
    /// 账户权限
    pub accounts: HashSet<String>,
}

/// 认证响应
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct AuthResponse {
    access_token: String,
    expires_in: i64,
    refresh_expires_in: i64,
    refresh_token: String,
    token_type: String,
    #[serde(rename = "not-before-policy")]
    not_before_policy: i32,
    session_state: String,
    scope: String,
}

/// Access Token Claims
#[derive(Debug, Serialize, Deserialize)]
struct AccessTokenClaims {
    jti: String,
    exp: i64,
    nbf: i64,
    iat: i64,
    iss: String,
    sub: String,
    typ: String,
    azp: String,
    auth_time: i64,
    session_state: String,
    acr: String,
    scope: String,
    grants: GrantsClaims,
    creation_time: i64,
    setname: bool,
    mobile: String,
    #[serde(rename = "mobileVerified")]
    mobile_verified: String,
    preferred_username: String,
    id: String,
    username: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GrantsClaims {
    features: Vec<String>,
    otg_ids: String,
    expiry_date: String,
    accounts: Vec<String>,
}

/// 期货公司信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerInfo {
    pub category: Vec<String>,
    pub url: String,
    pub broker_type: Option<String>,
    pub smtype: Option<String>,
    pub smconfig: Option<String>,
    pub condition_type: Option<String>,
    pub condition_config: Option<String>,
}

/// 行情服务器 URL 响应
#[derive(Debug, Deserialize)]
struct MdUrlResponse {
    mdurl: String,
}

/// 天勤认证实现
pub struct TqAuth {
    username: String,
    password: String,
    config: TqAuthConfig,
    access_token: String,
    refresh_token: String,
    auth_id: String,
    grants: Grants,
}
