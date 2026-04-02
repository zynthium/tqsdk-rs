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

/// 版本号
pub(crate) const VERSION: &str = "3.8.1";
const NS_URL: &str = "https://api.shinnytech.com/ns";
/// 客户端 ID
pub(crate) const CLIENT_ID: &str = "shinny_tq";
/// 客户端密钥
pub(crate) const CLIENT_SECRET: &str = "be30b9f4-6862-488a-99ad-21bde0400081";

#[derive(Debug, Clone)]
pub(crate) struct TqAuthConfig {
    pub auth_url: String,
}

/// 认证器接口
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// 获取包含认证信息的 HTTP Header
    fn base_header(&self) -> HeaderMap;

    /// 执行登录操作
    async fn login(&mut self) -> crate::errors::Result<()>;

    /// 获取指定期货公司的交易服务器地址
    async fn get_td_url(&self, broker_id: &str, account_id: &str) -> crate::errors::Result<BrokerInfo>;

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
pub(crate) struct Grants {
    /// 功能权限
    pub(crate) features: HashSet<String>,
    /// 账户权限
    pub(crate) accounts: HashSet<String>,
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
pub(crate) struct TqAuth {
    username: String,
    password: String,
    config: TqAuthConfig,
    access_token: String,
    refresh_token: String,
    auth_id: String,
    grants: Grants,
}

impl TqAuth {
    pub(crate) fn ns_url(&self) -> &str {
        NS_URL
    }
}
