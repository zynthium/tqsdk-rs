use std::env;
use tqsdk_rs::{Client, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 创建客户端
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let mut client = Client::new(&username, &password, ClientConfig::default()).await?;

    // 2. 初始化行情连接
    client.init_market().await?;

    // 3. 订阅行情
    let symbol = "SHFE.rb2310";
    let quote_sub = client.subscribe_quote(&[symbol]).await?;
    println!("已订阅: {}", symbol);

    // 4. 注册回调
    quote_sub.on_quote(|quote| {
        println!("行情更新: {} = {}", quote.instrument_id, quote.last_price);
    }).await;

    // 5. 启动订阅
    quote_sub.start().await?;

    // 6. 保持运行直到收到 Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("停止运行");

    Ok(())
}
