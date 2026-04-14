use std::env;
use std::time::Duration;

use tqsdk_rs::prelude::*;

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

fn derive_data_length(bar_secs: u64, lookback_minutes: i64) -> usize {
    let lookback_secs = (lookback_minutes.max(1) as u64) * 60;
    let bars = (lookback_secs + bar_secs.saturating_sub(1)) / bar_secs.max(1);
    bars.clamp(1, 10_000) as usize
}

fn example_log_level() -> String {
    env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "warn".to_string())
}

async fn build_client(username: &str, password: &str) -> Result<Client> {
    Client::builder(username, password)
        .config(ClientConfig {
            log_level: example_log_level(),
            view_width: 10000,
            ..Default::default()
        })
        .endpoints(EndpointConfig::from_env())
        .build()
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger(&example_log_level(), false);

    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");

    let mut client = build_client(&username, &password).await?;
    client.init_market().await?;

    let symbol = env::var("TQ_TEST_SYMBOL").unwrap_or_else(|_| "SHFE.au2512".to_string());
    let bar_secs = parse_env_u64("TQ_HISTORY_BAR_SECONDS", 60);
    let lookback_minutes = parse_env_i64("TQ_HISTORY_LOOKBACK_MINUTES", 240);
    let data_length = derive_data_length(bar_secs, lookback_minutes);

    println!(
        "订阅 K 线 serial symbol={} duration={}s data_length={} capped_at=10000",
        symbol, bar_secs, data_length
    );

    let sub = client
        .get_kline_serial(symbol.as_str(), Duration::from_secs(bar_secs), data_length)
        .await?;

    loop {
        let snapshot = sub.wait_update().await?;
        if snapshot.update.chart_ready {
            break;
        }
    }

    let window = sub.load().await?;
    let series = window
        .get_symbol_klines(symbol.as_str())
        .expect("single-symbol serial should exist");

    println!("K 线 serial 准备完成 count={}", series.data.len());
    if let (Some(first), Some(last)) = (series.data.first(), series.data.last()) {
        println!(
            "样本 first_id={} first_dt={} last_id={} last_dt={} last_close={}",
            first.id, first.datetime, last.id, last.datetime, last.close
        );
    }

    sub.close().await?;
    client.close().await?;
    Ok(())
}
