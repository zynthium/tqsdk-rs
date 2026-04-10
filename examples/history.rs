use std::env;
use std::pin::pin;
use std::time::Duration;

use futures::StreamExt;
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

    let tqapi = client.tqapi();
    let kline_ref = tqapi.kline(symbol.as_str(), duration);

    let series = client.series()?;
    let sub = series
        .kline_history(symbol.as_str(), duration, 8000, left_kline_id)
        .await?;
    sub.start().await?;

    let mut stream = pin!(sub.data_stream().await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }

        tokio::select! {
            update = kline_ref.wait_update() => {
                update?;
                let k = kline_ref.load().await;
                if k.id > 0 {
                    info!(
                        "Kline {} id={} O={:.2} H={:.2} L={:.2} C={:.2} V={}",
                        kline_ref.symbol(),
                        k.id,
                        k.open,
                        k.high,
                        k.low,
                        k.close,
                        k.volume
                    );
                }
            }
            item = stream.next() => {
                let Some(series_data) = item else { break; };
                if let Some(single) = &series_data.single
                    && let Some(chart) = &single.chart
                    && chart.ready
                {
                    info!(
                        "历史窗口同步完成 {} range=[{},{}] view_width={}",
                        symbol,
                        chart.left_id,
                        chart.right_id,
                        chart.view_width
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
        }
    }

    sub.close().await?;
    client.close().await?;
    Ok(())
}
