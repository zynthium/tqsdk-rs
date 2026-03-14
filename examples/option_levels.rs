use std::env;
use std::time::Duration;
use tqsdk_rs::*;

#[tokio::main]
async fn main() -> Result<()> {
    let user = match env::var("TQ_AUTH_USER") {
        Ok(v) => v,
        Err(_) => {
            println!("missing env: TQ_AUTH_USER");
            return Ok(());
        }
    };
    let pass = match env::var("TQ_AUTH_PASS") {
        Ok(v) => v,
        Err(_) => {
            println!("missing env: TQ_AUTH_PASS");
            return Ok(());
        }
    };

    let underlying = env::var("TQ_UNDERLYING").unwrap_or_else(|_| "SSE.510300".to_string());
    let nearbys = vec![0, 1];
    let price_level = vec![1, 0, -1];

    let mut config = ClientConfig::default();
    config.log_level = env::var("TQ_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    config.stock = true;
    let mut client = Client::new(&user, &pass, config).await?;
    client.init_market().await?;

    let quote_sub = client.subscribe_quote(&[underlying.as_str()]).await?;
    let quote_rx = quote_sub.quote_channel();

    let underlying_price = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let q = quote_rx
                .recv()
                .await
                .map_err(|_| TqError::Other("quote channel closed".to_string()))?;
            if q.instrument_id == underlying && q.last_price.is_finite() && q.last_price > 0.0 {
                return Ok::<f64, TqError>(q.last_price);
            }
        }
    })
    .await
    .map_err(|_| TqError::Timeout)??;

    println!("underlying={}, price={}", underlying, underlying_price);

    let atm = client
        .query_atm_options(
            underlying.as_str(),
            underlying_price,
            &price_level,
            "CALL",
            None,
            None,
            None,
        )
        .await?;
    println!("atm(price_level={:?})={:?}", price_level, atm);

    let (in_money, at_money, out_money) = client
        .query_all_level_options(underlying.as_str(), underlying_price, "CALL", None, None, None)
        .await?;
    println!(
        "all_level_options: in_money={}, at_money={:?}, out_money={}",
        in_money.len(),
        at_money,
        out_money.len()
    );

    let (in_money, at_money, out_money) = client
        .query_all_level_finance_options(
            underlying.as_str(),
            underlying_price,
            "CALL",
            &nearbys,
            None,
        )
        .await?;
    println!(
        "all_level_finance_options(nearbys={:?}): in_money={}, at_money={:?}, out_money={}",
        nearbys,
        in_money.len(),
        at_money,
        out_money.len()
    );

    quote_sub.close().await?;
    client.close().await?;
    Ok(())
}

