use super::{Authenticator, BrokerInfo, MdUrlResponse, TqAuth};
use crate::errors::{Result, TqError};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use std::collections::HashMap;
use tracing::info;

#[async_trait]
impl Authenticator for TqAuth {
    fn base_header(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("tqsdk-python 3.8.1"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if !self.access_token.is_empty()
            && let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
        {
            headers.insert(AUTHORIZATION, value);
        }
        headers
    }

    async fn login(&mut self) -> Result<()> {
        self.request_token().await?;
        self.parse_token().await?;
        info!("TqAuth 登录成功, User: {},  AuthId: {}", self.username, self.auth_id);
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
            return Err(TqError::ConfigError(format!("不支持该期货公司: {}", broker_id)));
        }

        let broker_infos: HashMap<String, BrokerInfo> = response.json().await.map_err(|e| TqError::Reqwest {
            context: format!("解析 broker 配置失败: GET {}", url),
            source: e,
        })?;

        broker_infos
            .get(broker_id)
            .cloned()
            .ok_or_else(|| TqError::ConfigError(format!("该期货公司 {} 暂不支持 TqSdk 登录", broker_id)))
    }

    async fn get_md_url(&self, stock: bool, backtest: bool) -> Result<String> {
        let ns_url = self.ns_url().to_string();
        let client = self.build_http_client()?;

        let response = client
            .get(&ns_url)
            .query(&[("stock", stock.to_string()), ("backtest", backtest.to_string())])
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

            if matches!(*symbol, "SSE.000016" | "SSE.000300" | "SSE.000905" | "SSE.000852") {
                if !self.has_feature("lmt_idx") {
                    return Err(TqError::PermissionDenied(format!(
                        "您的账户不支持查看 {} 的行情数据",
                        symbol
                    )));
                }
                continue;
            }

            if matches!(
                prefix,
                "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX" | "SSWE" | "KQ" | "KQD"
            ) {
                if !self.has_feature("futr") {
                    return Err(TqError::permission_denied_futures());
                }
                continue;
            }

            if prefix == "CSI" || matches!(prefix, "SSE" | "SZSE") {
                if !self.has_feature("sec") {
                    return Err(TqError::permission_denied_stocks());
                }
                continue;
            }

            return Err(TqError::PermissionDenied(format!("不支持的合约: {}", symbol)));
        }

        Ok(())
    }

    fn has_td_grants(&self, symbol: &str) -> Result<()> {
        let prefix = symbol.split('.').next().unwrap_or("");

        if matches!(
            prefix,
            "CFFEX" | "SHFE" | "DCE" | "CZCE" | "INE" | "GFEX" | "SSWE" | "KQ" | "KQD"
        ) {
            if self.has_feature("futr") {
                return Ok(());
            }
            return Err(TqError::PermissionDenied(format!("您的账户不支持交易 {}", symbol)));
        }

        if prefix == "CSI" || matches!(prefix, "SSE" | "SZSE") {
            if self.has_feature("sec") {
                return Ok(());
            }
            return Err(TqError::PermissionDenied(format!("您的账户不支持交易 {}", symbol)));
        }

        Err(TqError::PermissionDenied(format!("不支持的合约: {}", symbol)))
    }

    fn get_auth_id(&self) -> &str {
        &self.auth_id
    }

    fn get_access_token(&self) -> &str {
        &self.access_token
    }
}
