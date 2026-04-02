use super::{TqAuth, TqAuthConfig};
use crate::errors::{Result, TqError};

impl TqAuth {
    /// 创建新的认证器
    pub fn new(username: String, password: String, auth_url: String) -> Self {
        TqAuth {
            username,
            password,
            config: TqAuthConfig { auth_url },
            access_token: String::new(),
            refresh_token: String::new(),
            auth_id: String::new(),
            grants: super::Grants::default(),
        }
    }

    pub(super) fn build_http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| TqError::Reqwest {
                context: "创建 HTTP 客户端失败".to_string(),
                source: e,
            })
    }
}
