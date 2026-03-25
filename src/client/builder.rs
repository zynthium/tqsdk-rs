use super::{Client, ClientBuilder, ClientConfig};
use crate::auth::{Authenticator, TqAuth};
use crate::datamanager::{DataManager, DataManagerConfig};
use crate::errors::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;

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
            auth: None,
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

    pub fn ins_url(mut self, url: impl Into<String>) -> Self {
        self.config.ins_url = url.into();
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
    /// ```no_run
    /// # use tqsdk_rs::*;
    /// # use tqsdk_rs::auth::TqAuth;
    /// # async fn example() -> Result<()> {
    /// let mut auth = TqAuth::new("username".to_string(), "password".to_string());
    /// auth.login().await?;
    ///
    /// let client = Client::builder("username", "password")
    ///     .auth(auth)
    ///     .build()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn auth<A: Authenticator + 'static>(mut self, auth: A) -> Self {
        self.auth = Some(Arc::new(RwLock::new(auth)));
        self
    }

    /// 构建客户端
    ///
    /// # 错误
    ///
    /// 如果认证失败，返回错误
    pub async fn build(self) -> Result<Client> {
        crate::logger::init_logger(&self.config.log_level, true);

        let auth: Arc<RwLock<dyn Authenticator>> = if let Some(custom_auth) = self.auth {
            custom_auth
        } else {
            let mut auth = TqAuth::new(self.username.clone(), self.password.clone());
            auth.login().await?;
            Arc::new(RwLock::new(auth))
        };

        let dm_config = DataManagerConfig {
            default_view_width: self.config.view_width,
            enable_auto_cleanup: true,
            ..DataManagerConfig::default()
        };
        let dm = Arc::new(DataManager::new(HashMap::new(), dm_config));

        Ok(Client {
            _username: self.username,
            _config: self.config,
            auth,
            dm,
            quotes_ws: None,
            series_api: None,
            ins_api: None,
            market_active: AtomicBool::new(false),
            trade_sessions: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}
