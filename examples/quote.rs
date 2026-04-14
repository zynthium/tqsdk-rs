use std::env;
use std::time::Duration;

use tqsdk_rs::prelude::*;

fn credentials() -> (String, String) {
    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");
    (username, password)
}

fn example_log_level() -> String {
    env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "warn".to_string())
}

async fn build_client(username: &str, password: &str) -> Result<Client> {
    let config = ClientConfig {
        log_level: example_log_level(),
        view_width: 10000,
        ..Default::default()
    };

    Client::builder(username, password)
        .config(config)
        .endpoints(EndpointConfig::from_env())
        .build()
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger(&example_log_level(), false);

    let (username, password) = credentials();
    let mut client = build_client(&username, &password).await?;
    client.init_market().await?;

    let au = env::var("TQ_QUOTE_AU").unwrap_or_else(|_| "SHFE.au2602".to_string());
    let ag = env::var("TQ_QUOTE_AG").unwrap_or_else(|_| "SHFE.ag2512".to_string());
    let m = env::var("TQ_QUOTE_M").unwrap_or_else(|_| "DCE.m2512".to_string());

    let quote_sub = client.subscribe_quote(&[au.as_str(), ag.as_str(), m.as_str()]).await?;

    let kline_duration = Duration::from_secs(60);
    let kline_sub = client.get_kline_serial(au.as_str(), kline_duration, 256).await?;

    let tick_sub = client.get_tick_serial(au.as_str(), 256).await?;

    let au_quote = client.quote(au.as_str());
    let ag_quote = client.quote(ag.as_str());
    let au_kline = client.kline_ref(au.as_str(), kline_duration);
    let au_tick = client.tick_ref(au.as_str());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let updates = match tokio::time::timeout(remaining, client.wait_update_and_drain()).await {
            Ok(result) => result?,
            Err(_) => break,
        };

        for symbol in updates.quotes {
            if symbol.as_str() == au.as_str() {
                let q = au_quote.load().await;
                println!(
                    "Quote {} last={:.2} change={:.2} bid1={:.2} ask1={:.2}",
                    q.instrument_id, q.last_price, q.change, q.bid_price1, q.ask_price1
                );
            } else if symbol.as_str() == ag.as_str() {
                let q = ag_quote.load().await;
                println!(
                    "Quote {} last={:.2} change={:.2} bid1={:.2} ask1={:.2}",
                    q.instrument_id, q.last_price, q.change, q.bid_price1, q.ask_price1
                );
            }
        }

        for key in updates.klines {
            if key.symbol.as_str() == au.as_str() && key.duration_nanos == au_kline.duration_nanos() {
                let k = au_kline.load().await;
                println!(
                    "Kline {} id={} O={:.2} H={:.2} L={:.2} C={:.2} V={}",
                    au_kline.symbol(),
                    k.id,
                    k.open,
                    k.high,
                    k.low,
                    k.close,
                    k.volume
                );
            }
        }

        for symbol in updates.ticks {
            if symbol.as_str() == au.as_str() {
                let t = au_tick.load().await;
                println!(
                    "Tick {} id={} last={:.2} bid1={:.2}({}) ask1={:.2}({}) V={}",
                    au_tick.symbol(),
                    t.id,
                    t.last_price,
                    t.bid_price1,
                    t.bid_volume1,
                    t.ask_price1,
                    t.ask_volume1,
                    t.volume
                );
            }
        }
    }

    tick_sub.close().await?;
    kline_sub.close().await?;
    quote_sub.close().await?;
    client.close().await?;
    Ok(())
}
