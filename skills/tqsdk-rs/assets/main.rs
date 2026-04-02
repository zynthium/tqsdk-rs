use std::env;
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
    let quote_sub = client.subscribe_quote(&[symbol]).await?;
    println!("已订阅: {}", symbol);

    quote_sub
        .on_quote(|quote| {
            println!("行情更新: {} = {}", quote.instrument_id, quote.last_price);
        })
        .await;

    quote_sub.start().await?;

    tokio::signal::ctrl_c().await?;
    println!("停止运行");

    Ok(())
}
