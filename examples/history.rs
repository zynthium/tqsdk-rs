use std::env;
use std::time::Duration;

use tqsdk_rs::prelude::*;
use tracing::info;

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
    let left_kline_id = env::var("TQ_LEFT_KLINE_ID")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(105761);

    let sub = client
        .kline_history(symbol.as_str(), duration, 8000, left_kline_id)
        .await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }

        let snapshot = sub.wait_update().await?;
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
            break;
        }
    }

    sub.close().await?;
    client.close().await?;
    Ok(())
}
