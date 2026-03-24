//! 认证模块
//!
//! 实现天勤账号认证和权限检查

use crate::errors::{Result, TqError};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, info};

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
    async fn login(&mut self) -> Result<()>;

    /// 获取指定期货公司的交易服务器地址
    async fn get_td_url(&self, broker_id: &str, account_id: &str) -> Result<BrokerInfo>;

    /// 获取行情服务器地址
    async fn get_md_url(&self, stock: bool, backtest: bool) -> Result<String>;

    /// 检查是否具有指定的功能权限
    fn has_feature(&self, feature: &str) -> bool;

    /// 检查是否有查看指定合约行情数据的权限
    fn has_md_grants(&self, symbols: &[&str]) -> Result<()>;

    /// 检查是否有交易指定合约的权限
    fn has_td_grants(&self, symbol: &str) -> Result<()>;

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

impl TqAuth {
    /// 创建新的认证器
    pub fn new(username: String, password: String) -> Self {
        TqAuth {
            username,
            password,
            config: TqAuthConfig::default(),
            access_token: String::new(),
            refresh_token: String::new(),
            auth_id: String::new(),
            grants: Grants::default(),
        }
    }

    pub fn with_config(mut self, config: TqAuthConfig) -> Self {
        self.config = config;
        self
    }

    fn build_http_client(&self) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder().timeout(self.config.http_timeout);
        if self.config.no_proxy {
            builder = builder.no_proxy();
        }
        if let Some(proxy) = self.config.proxy.as_deref() {
            let px = reqwest::Proxy::all(proxy).map_err(|e| TqError::Reqwest {
                context: format!("配置代理失败: {}", proxy),
                source: e,
            })?;
            builder = builder.proxy(px);
        }
        builder.build().map_err(|e| TqError::Reqwest {
            context: "创建 HTTP 客户端失败".to_string(),
            source: e,
        })
    }

    /// 请求 Token
    async fn request_token(&mut self) -> Result<()> {
        let url = format!(
            "{}/auth/realms/shinnytech/protocol/openid-connect/token",
            self.config.auth_url
        );

        let params = [
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("username", &self.username),
            ("password", &self.password),
            ("grant_type", "password"),
        ];

        info!("正在请求认证 token...");

        let client = self.build_http_client()?;

        let response = client
            .post(&url)
            .form(&params)
            .header(USER_AGENT, format!("tqsdk-python {}", VERSION))
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| TqError::Reqwest {
                context: format!("请求失败: POST {}", url),
                source: e,
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.map_err(|e| TqError::Reqwest {
                context: format!("读取响应失败: POST {}", url),
                source: e,
            })?;
            return Err(TqError::HttpStatus {
                method: "POST".to_string(),
                url,
                status,
                body_snippet: TqError::truncate_body(body),
            });
        }

        let auth_resp: AuthResponse = response.json().await.map_err(|e| TqError::Reqwest {
            context: "解析 token 响应失败".to_string(),
            source: e,
        })?;
        self.access_token = auth_resp.access_token;
        self.refresh_token = auth_resp.refresh_token;

        debug!("Token 获取成功");
        Ok(())
    }

    async fn parse_token(&mut self) -> Result<()> {
        use jsonwebtoken::dangerous::insecure_decode;
        let token_data = insecure_decode::<AccessTokenClaims>(&self.access_token).map_err(|e| {
            TqError::Jwt {
                context: "解析 token 失败".to_string(),
                source: e,
            }
        })?;
        let claims = token_data.claims;
        if self.config.verify_jwt {
            let now = chrono::Utc::now().timestamp();
            if claims.exp <= now {
                return Err(TqError::Jwt {
                    context: "token 已过期".to_string(),
                    source: jsonwebtoken::errors::ErrorKind::ExpiredSignature.into(),
                });
            }
            let expected_iss = format!("{}/auth/realms/shinnytech", self.config.auth_url.trim_end_matches('/'));
            if !claims.iss.starts_with(&expected_iss) {
                return Err(TqError::Jwt {
                    context: "token issuer 无效".to_string(),
                    source: jsonwebtoken::errors::ErrorKind::InvalidIssuer.into(),
                });
            }
            if claims.azp != self.config.client_id {
                return Err(TqError::Jwt {
                    context: "token azp 不匹配".to_string(),
                    source: jsonwebtoken::errors::ErrorKind::InvalidAudience.into(),
                });
            }
        }
        self.auth_id = claims.sub;
        self.grants = Grants::default();

        for feature in claims.grants.features {
            self.grants.features.insert(feature);
        }
        for account in claims.grants.accounts {
            self.grants.accounts.insert(account);
        }

        debug!(
            "权限解析完成: {} 个功能, {} 个账户",
            self.grants.features.len(),
            self.grants.accounts.len()
        );

        Ok(())
    }
}

#[async_trait]
impl Authenticator for TqAuth {
    fn base_header(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("tqsdk-python 3.8.1"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if !self.access_token.is_empty() {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.access_token)) {
                headers.insert(AUTHORIZATION, value);
            }
        }
        headers
    }

    async fn login(&mut self) -> Result<()> {
        self.request_token().await?;
        self.parse_token().await?;
        info!(
            "TqAuth 登录成功, User: {},  AuthId: {}",
            self.username, self.auth_id
        );
        Ok(())
    }

    async fn get_td_url(&self, broker_id: &str, account_id: &str) -> Result<BrokerInfo> {
        let url = format!("https://files.shinnytech.com/{}.json", broker_id);

        let client = self.build_http_client()?;

        let response = client
            .get(&url)
            .query(&[("account_id", account_id), ("auth", &self.username)])
            .headers(self.base_header())
            .send()
            .await
            .map_err(|e| TqError::Reqwest {
                context: format!("请求失败: GET {}", url),
                source: e,
            })?;

        if !response.status().is_success() {
            return Err(TqError::ConfigError(format!(
                "不支持该期货公司: {}",
                broker_id
            )));
        }

        let broker_infos: std::collections::HashMap<String, BrokerInfo> =
            response.json().await.map_err(|e| TqError::Reqwest {
                context: format!("解析 broker 配置失败: GET {}", url),
                source: e,
            })?;

        broker_infos.get(broker_id).cloned().ok_or_else(|| {
            TqError::ConfigError(format!("该期货公司 {} 暂不支持 TqSdk 登录", broker_id))
        })
    }

    async fn get_md_url(&self, stock: bool, backtest: bool) -> Result<String> {
        if let Ok(md_url) = std::env::var(TQ_MD_URL_ENV) {
            let trimmed = md_url.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        let ns_url = self.config.ns_url.clone();
        let client = self.build_http_client()?;

        let response = client
            .get(&ns_url)
            .query(&[
                ("stock", stock.to_string()),
                ("backtest", backtest.to_string()),
            ])
            .headers(self.base_header())
            .send()
            .await
            .map_err(|e| TqError::Reqwest {
                context: format!("请求失败: GET {}", ns_url),
                source: e,
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.map_err(|e| TqError::Reqwest {
                context: format!("读取响应失败: GET {}", ns_url),
                source: e,
            })?;
            return Err(TqError::HttpStatus {
                method: "GET".to_string(),
                url: ns_url,
                status,
                body_snippet: TqError::truncate_body(body),
            });
        }

        let md_url_resp: MdUrlResponse = response.json().await.map_err(|e| TqError::Reqwest {
            context: "解析名称服务响应失败".to_string(),
            source: e,
        })?;
        let md_url = md_url_resp.mdurl.trim();
        if md_url.is_empty() {
            return Err(TqError::ConfigError("名称服务返回空行情地址".to_string()));
        }
        Ok(md_url.to_string())
    }

    fn has_feature(&self, feature: &str) -> bool {
        self.grants.features.contains(feature)
    }

    fn has_md_grants(&self, symbols: &[&str]) -> Result<()> {
        for symbol in symbols {
            let prefix = symbol.split('.').next().unwrap_or("");

            // 检查期货、现货、KQ、KQD 交易所
            if matches!(
                prefix,
                "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX" | "SSWE" | "KQ" | "KQD"
            ) {
                if !self.has_feature("futr") {
                    return Err(TqError::permission_denied_futures());
                }
                continue;
            }

            // 检查股票交易所
            if prefix == "CSI" || matches!(prefix, "SSE" | "SZSE") {
                if !self.has_feature("sec") {
                    return Err(TqError::permission_denied_stocks());
                }
                continue;
            }

            // 检查限制指数
            if matches!(
                *symbol,
                "SSE.000016" | "SSE.000300" | "SSE.000905" | "SSE.000852"
            ) {
                if !self.has_feature("lmt_idx") {
                    return Err(TqError::PermissionDenied(format!(
                        "您的账户不支持查看 {} 的行情数据",
                        symbol
                    )));
                }
                continue;
            }

            // 未知交易所
            return Err(TqError::PermissionDenied(format!(
                "不支持的合约: {}",
                symbol
            )));
        }

        Ok(())
    }

    fn has_td_grants(&self, symbol: &str) -> Result<()> {
        let prefix = symbol.split('.').next().unwrap_or("");

        // 检查期货、现货、KQ、KQD 交易所
        if matches!(
            prefix,
            "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX" | "SSWE" | "KQ" | "KQD"
        ) {
            if self.has_feature("futr") {
                return Ok(());
            }
            return Err(TqError::PermissionDenied(format!(
                "您的账户不支持交易 {}",
                symbol
            )));
        }

        // 检查股票交易所
        if prefix == "CSI" || matches!(prefix, "SSE" | "SZSE") {
            if self.has_feature("sec") {
                return Ok(());
            }
            return Err(TqError::PermissionDenied(format!(
                "您的账户不支持交易 {}",
                symbol
            )));
        }

        Err(TqError::PermissionDenied(format!(
            "不支持的合约: {}",
            symbol
        )))
    }

    fn get_auth_id(&self) -> &str {
        &self.auth_id
    }

    fn get_access_token(&self) -> &str {
        &self.access_token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tq_auth_creation() {
        let auth = TqAuth::new("test_user".to_string(), "test_pass".to_string());
        assert_eq!(auth.username, "test_user");
        assert_eq!(auth.password, "test_pass");
    }
}
