use super::{Client, ClientBuilder, ClientConfig, EndpointConfig, PendingTradeSessionConfig};
use crate::auth::{Authenticator, TqAuth};
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::Result;
use crate::marketdata::MarketDataState;
use crate::runtime::TqRuntime;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock as AsyncRwLock;

impl ClientBuilder {
    /// 创建新的客户端构建器
    ///
    /// # 参数
    ///
    /// * `username` - 用户名
    /// * `password` - 密码
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::client::ClientBuilder;
    /// # async fn example() -> Result<()> {
    /// let client = ClientBuilder::new("username", "password")
    ///     .log_level("debug")
    ///     .view_width(5000)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            config: ClientConfig::default(),
            endpoints: EndpointConfig::default(),
            auth: None,
            trade_session_configs: Vec::new(),
        }
    }

    /// 设置日志级别
    pub fn log_level(mut self, level: impl Into<String>) -> Self {
        self.config.log_level = level.into();
        self
    }

    /// 设置默认视图宽度
    pub fn view_width(mut self, width: usize) -> Self {
        self.config.view_width = width;
        self
    }

    /// 设置开发模式
    pub fn development(mut self, dev: bool) -> Self {
        self.config.development = dev;
        self
    }

    pub fn endpoints(mut self, endpoints: EndpointConfig) -> Self {
        self.endpoints = endpoints;
        self
    }

    pub fn auth_url(mut self, url: impl Into<String>) -> Self {
        self.endpoints.auth_url = url.into();
        self
    }

    pub fn md_url(mut self, url: impl Into<String>) -> Self {
        self.endpoints.md_url = Some(url.into());
        self
    }

    pub fn td_url(mut self, url: impl Into<String>) -> Self {
        self.endpoints.td_url = Some(url.into());
        self
    }

    pub fn ins_url(mut self, url: impl Into<String>) -> Self {
        self.endpoints.ins_url = url.into();
        self
    }

    pub fn holiday_url(mut self, url: impl Into<String>) -> Self {
        self.endpoints.holiday_url = url.into();
        self
    }

    pub fn message_queue_capacity(mut self, capacity: usize) -> Self {
        self.config.message_queue_capacity = capacity.max(1);
        self
    }

    pub fn message_backlog_warn_step(mut self, step: usize) -> Self {
        self.config.message_backlog_warn_step = step.max(1);
        self
    }

    pub fn message_batch_max(mut self, batch: usize) -> Self {
        self.config.message_batch_max = batch.max(1);
        self
    }

    /// 是否启用 Series 磁盘缓存。
    ///
    /// 默认值为 `false`（关闭）。
    pub fn series_disk_cache_enabled(mut self, enabled: bool) -> Self {
        self.config.series_disk_cache_enabled = enabled;
        self
    }

    /// 设置 Series 磁盘缓存总大小上限（字节）。
    ///
    /// 传入 `None` 或 `Some(0)` 表示不限制。
    pub fn series_disk_cache_max_bytes(mut self, max_bytes: Option<u64>) -> Self {
        self.config.series_disk_cache_max_bytes = max_bytes.filter(|v| *v > 0);
        self
    }

    /// 设置 Series 磁盘缓存保留天数。
    ///
    /// 传入 `None` 或 `Some(0)` 表示不按保留天数清理。
    pub fn series_disk_cache_retention_days(mut self, days: Option<u64>) -> Self {
        self.config.series_disk_cache_retention_days = days.filter(|v| *v > 0);
        self
    }

    /// 设置完整配置
    pub fn config(mut self, config: ClientConfig) -> Self {
        self.config = config;
        self
    }

    /// 使用自定义认证器（高级用法）
    ///
    /// 如果不设置，将使用默认的 TqAuth
    ///
    /// # 示例
    ///
    /// ```ignore
    /// use tqsdk_rs::Client;
    ///
    /// // 传入你自己的实现了 Authenticator 的认证器实例
    /// let client = Client::builder("username", "password")
    ///     .auth(custom_authenticator)
    ///     .build()
    ///     .await?;
    /// ```
    pub fn auth<A: Authenticator + 'static>(mut self, auth: A) -> Self {
        self.auth = Some(Arc::new(AsyncRwLock::new(auth)));
        self
    }

    pub fn trade_session(
        mut self,
        broker: impl Into<String>,
        user_id: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.trade_session_configs.push(PendingTradeSessionConfig {
            broker: broker.into(),
            user_id: user_id.into(),
            password: password.into(),
            options: super::TradeSessionOptions::default(),
        });
        self
    }

    pub fn trade_session_with_options(
        mut self,
        broker: impl Into<String>,
        user_id: impl Into<String>,
        password: impl Into<String>,
        options: super::TradeSessionOptions,
    ) -> Self {
        self.trade_session_configs.push(PendingTradeSessionConfig {
            broker: broker.into(),
            user_id: user_id.into(),
            password: password.into(),
            options,
        });
        self
    }

    /// 构建客户端
    ///
    /// # 错误
    ///
    /// 如果认证失败，返回错误
    pub async fn build(self) -> Result<Client> {
        crate::logger::init_logger(&self.config.log_level, true);

        let auth: Arc<AsyncRwLock<dyn Authenticator>> = if let Some(custom_auth) = self.auth {
            custom_auth
        } else {
            let mut auth = TqAuth::new(
                self.username.clone(),
                self.password.clone(),
                self.endpoints.auth_url.clone(),
            );
            auth.login().await?;
            Arc::new(AsyncRwLock::new(auth))
        };

        let dm_config = DataManagerConfig {
            default_view_width: self.config.view_width,
            enable_auto_cleanup: true,
            ..DataManagerConfig::default()
        };
        let dm = Arc::new(DataManager::new(HashMap::new(), dm_config));
        let market_state = Arc::new(MarketDataState::default());
        let live_api = crate::marketdata::TqApi::new(Arc::clone(&market_state));

        let client = Client {
            username: self.username,
            config: self.config,
            endpoints: self.endpoints,
            auth,
            dm,
            market_state,
            live_api,
            quotes_ws: None,
            series_api: None,
            ins_api: None,
            market_active: AtomicBool::new(false),
            trade_sessions: Arc::new(std::sync::RwLock::new(HashMap::new())),
        };

        let trade_session_configs = self.trade_session_configs;
        for session in trade_session_configs {
            client
                .create_trade_session_with_options(
                    &session.broker,
                    &session.user_id,
                    &session.password,
                    session.options.clone(),
                )
                .await?;
        }

        Ok(client)
    }

    pub async fn build_runtime(self) -> Result<Arc<TqRuntime>> {
        Ok(self.build().await?.into_runtime())
    }
}
