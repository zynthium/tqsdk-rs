use std::env;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tqsdk_rs::prelude::*;
use tracing::info;

fn parse_env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_env_i64(name: &str, default: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

async fn build_client(username: &str, password: &str) -> Result<Client> {
    Client::builder(username, password)
        .config(ClientConfig {
            log_level: env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            view_width: 10000,
            ..Default::default()
        })
        .endpoints(EndpointConfig::from_env())
        .build()
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger(&env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()), false);

    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");

    let mut client = build_client(&username, &password).await?;
    client.init_market().await?;

    let symbol = env::var("TQ_TEST_SYMBOL").unwrap_or_else(|_| "SHFE.au2512".to_string());
    let bar_secs = parse_env_u64("TQ_HISTORY_BAR_SECONDS", 60);
    let lookback_minutes = parse_env_i64("TQ_HISTORY_LOOKBACK_MINUTES", 240);
    let end_dt = Utc::now();
    let start_dt = end_dt - ChronoDuration::minutes(lookback_minutes);

    info!(
        "拉取历史 K 线 data_series symbol={} duration={}s range=[{}, {})",
        symbol, bar_secs, start_dt, end_dt
    );

    let rows = client
        .get_kline_data_series(symbol.as_str(), Duration::from_secs(bar_secs), start_dt, end_dt)
        .await?;

    info!("历史 K 线 data_series 下载完成 count={}", rows.len());
    if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
        info!(
            "样本 first_id={} first_dt={} last_id={} last_dt={} last_close={}",
            first.id, first.datetime, last.id, last.datetime, last.close
        );
    }

    client.close().await?;
    Ok(())
}
