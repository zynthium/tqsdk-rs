use std::env;
use std::time::Duration;

use chrono::Utc;
use tqsdk_rs::prelude::*;
use tracing::info;

fn parse_env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_env_i32(name: &str) -> Option<i32> {
    env::var(name).ok().and_then(|raw| raw.parse::<i32>().ok())
}

async fn resolve_left_kline_id(client: &Client, symbol: &str, duration: Duration, data_length: usize) -> Result<i64> {
    let sub = client.kline(symbol, duration, data_length.max(2)).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            sub.close().await?;
            return Err(TqError::Timeout);
        }

        let snapshot = match tokio::time::timeout(remaining, sub.wait_update()).await {
            Ok(result) => result?,
            Err(_) => {
                sub.close().await?;
                return Err(TqError::Timeout);
            }
        };

        if !snapshot.update.chart_ready {
            continue;
        }

        let series = sub.load().await?;
        if let Some(single) = &series.single
            && let Some(last) = single.data.last()
        {
            let left_id = last.id.saturating_sub(data_length.saturating_sub(1) as i64);
            sub.close().await?;
            return Ok(left_id.max(0));
        }
    }
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
    let duration = Duration::from_secs(60);
    let data_length = parse_env_usize("TQ_HISTORY_VIEW_WIDTH", 8000);
    let focus_position = parse_env_i32("TQ_HISTORY_FOCUS_POSITION").unwrap_or(0);
    let left_kline_id = match env::var("TQ_LEFT_KLINE_ID").ok().as_deref() {
        Some("auto") => {
            let resolved = resolve_left_kline_id(&client, symbol.as_str(), duration, data_length).await?;
            info!("基于实时 K 线推导 left_kline_id={}", resolved);
            Some(resolved)
        }
        Some(raw) => raw.parse::<i64>().ok(),
        None => None,
    };
    let sub = if let Some(left_kline_id) = left_kline_id {
        info!("按 left_kline_id={} 拉取历史窗口", left_kline_id);
        client
            .kline_history(symbol.as_str(), duration, data_length, left_kline_id)
            .await?
    } else {
        info!(
            "未设置 TQ_LEFT_KLINE_ID，按当前时间聚焦最近历史窗口 view_width={} focus_position={}",
            data_length, focus_position
        );
        client
            .kline_history_with_focus(symbol.as_str(), duration, data_length, Utc::now(), focus_position)
            .await?
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    let mut synced = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let snapshot = match tokio::time::timeout(remaining, sub.wait_update()).await {
            Ok(result) => result?,
            Err(_) => break,
        };
        if !snapshot.update.chart_ready {
            continue;
        }

        let series_data = sub.load().await?;
        if let Some(single) = &series_data.single
            && let Some(chart) = &single.chart
            && chart.ready
        {
            info!(
                "历史窗口同步完成 {} range=[{},{}] view_width={}",
                symbol, chart.left_id, chart.right_id, chart.view_width
            );
            if let (Some(first), Some(last)) = (single.data.first(), single.data.last()) {
                info!(
                    "样本 first_id={} last_id={} count={}",
                    first.id,
                    last.id,
                    single.data.len()
                );
            }
            synced = true;
            break;
        }
    }

    sub.close().await?;
    client.close().await?;
    if !synced {
        return Err(TqError::Timeout);
    }
    Ok(())
}
