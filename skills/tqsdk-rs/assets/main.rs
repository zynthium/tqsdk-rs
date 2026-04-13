use std::env;
use std::time::Duration;

use tqsdk_rs::{Client, ClientConfig, EndpointConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;

    let mut client = Client::builder(&username, &password)
        .config(ClientConfig::default())
        .endpoints(EndpointConfig::from_env())
        .build()
        .await?;

    client.init_market().await?;

    let symbol = "SHFE.au2602";
    let _quote_sub = client.subscribe_quote(&[symbol]).await?;
    let quote = client.quote(symbol);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        let updates = match tokio::time::timeout(remaining, client.wait_update_and_drain()).await {
            Ok(result) => result?,
            Err(_) => break,
        };

        if updates.quotes.iter().any(|item| item.as_str() == symbol) {
            let q = quote.load().await;
            println!("{} = {}", q.instrument_id, q.last_price);
        }
    }

    client.close().await?;
    Ok(())
}
