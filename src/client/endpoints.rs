use std::env;

const DEFAULT_AUTH_URL: &str = "https://auth.shinnytech.com";
const DEFAULT_INS_URL: &str = "https://openmd.shinnytech.com/t/md/symbols/latest.json";
const DEFAULT_HOLIDAY_URL: &str = "https://files.shinnytech.com/shinny_chinese_holiday.json";

fn read_optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_env_or_default(name: &str, default: &str) -> String {
    read_optional_env(name).unwrap_or_else(|| default.to_string())
}

/// 服务端点配置
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub auth_url: String,
    pub md_url: Option<String>,
    pub td_url: Option<String>,
    pub ins_url: String,
    pub holiday_url: String,
}

impl EndpointConfig {
    pub fn from_env() -> Self {
        Self {
            auth_url: read_env_or_default("TQ_AUTH_URL", DEFAULT_AUTH_URL),
            md_url: read_optional_env("TQ_MD_URL"),
            td_url: read_optional_env("TQ_TD_URL"),
            ins_url: read_env_or_default("TQ_INS_URL", DEFAULT_INS_URL),
            holiday_url: read_env_or_default("TQ_CHINESE_HOLIDAY_URL", DEFAULT_HOLIDAY_URL),
        }
    }
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

/// 交易会话选项
#[derive(Debug, Clone, Default)]
pub struct TradeSessionOptions {
    pub td_url_override: Option<String>,
}
