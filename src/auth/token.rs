use super::{AccessTokenClaims, AuthResponse, Grants, TqAuth, VERSION};
use crate::errors::{Result, TqError};
use reqwest::header::{ACCEPT, USER_AGENT};
use tracing::{debug, info};

impl TqAuth {
    /// 请求 Token
    pub(super) async fn request_token(&mut self) -> Result<()> {
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

    pub(super) async fn parse_token(&mut self) -> Result<()> {
        use jsonwebtoken::dangerous::insecure_decode;

        let token_data =
            insecure_decode::<AccessTokenClaims>(&self.access_token).map_err(|e| TqError::Jwt {
                context: "解析 token 失败".to_string(),
                source: e,
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
            let expected_iss = format!(
                "{}/auth/realms/shinnytech",
                self.config.auth_url.trim_end_matches('/')
            );
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
