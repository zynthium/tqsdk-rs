use super::{TqAuth, TqAuthConfig};
use crate::errors::{Result, TqError};

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
            grants: super::Grants::default(),
        }
    }

    pub fn with_config(mut self, config: TqAuthConfig) -> Self {
        self.config = config;
        self
    }

    pub(super) fn build_http_client(&self) -> Result<reqwest::Client> {
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
}
