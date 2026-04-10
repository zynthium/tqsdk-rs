use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use std::env;
use std::time::Duration;
use tqsdk_rs::Position;
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const KLINE_WIDTH: usize = 64;

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> tqsdk_rs::Result<NaiveDate> {
    match env::var(name) {
        Ok(raw) => NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
            .map_err(|err| TqError::InvalidParameter(format!("invalid {name}: {err}"))),
        Err(_) => NaiveDate::from_ymd_opt(default.0, default.1, default.2)
            .ok_or_else(|| TqError::InvalidParameter(format!("invalid default date for {name}"))),
    }
}

fn shanghai_range(start_date: NaiveDate, end_date: NaiveDate) -> tqsdk_rs::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let tz = FixedOffset::east_opt(8 * 3600)
        .ok_or_else(|| TqError::InternalError("failed to construct Asia/Shanghai offset".to_string()))?;
    let start_dt = tz
        .from_local_datetime(
            &start_date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| TqError::InvalidParameter("invalid start datetime".to_string()))?,
        )
        .single()
        .ok_or_else(|| TqError::InvalidParameter("ambiguous start datetime".to_string()))?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(
            &end_date
                .and_hms_opt(23, 59, 59)
                .ok_or_else(|| TqError::InvalidParameter("invalid end datetime".to_string()))?,
        )
        .single()
        .ok_or_else(|| TqError::InvalidParameter("ambiguous end datetime".to_string()))?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_i64(name: &str, default: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(default)
}

fn net_position(position: &Position) -> i64 {
    position.volume_long - position.volume_short
}

fn runtime_err(err: RuntimeError) -> TqError {
    TqError::InternalError(format!("runtime error: {err}"))
}

#[tokio::main]
async fn main() -> tqsdk_rs::Result<()> {
    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");
    let symbol = env_or_default("TQ_TEST_SYMBOL", "SHFE.rb2605");
    let position_size = parse_env_i64("TQ_POSITION_SIZE", 1);
    let start_date = parse_env_date("TQ_START_DT", (2026, 4, 6))?;
    let end_date = parse_env_date("TQ_END_DT", (2026, 4, 7))?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;

    init_logger(&env_or_default("TQ_LOG_LEVEL", "info"), false);

    let mut client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(env_or_default("TQ_LOG_LEVEL", "info"))
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;

    let quote = session.quote(&symbol).await?;
    let klines = session
        .series()
        .kline(&symbol, Duration::from_secs(60), KLINE_WIDTH)
        .await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let task = TargetPosTask::new(runtime.clone(), ACCOUNT_KEY, &symbol, TargetPosTaskOptions::default())
        .await
        .map_err(runtime_err)?;

    println!("replay backtest");
    println!("  symbol: {symbol}");
    println!("  range:  {start_date} -> {end_date}");
    println!("  size:   {position_size}");
    println!("  runtime id: {}", runtime.id());

    let mut processed_bar_id = None;
    let mut step_count = 0usize;
    let mut signal_count = 0usize;

    while let Some(step) = session.step().await? {
        step_count += 1;

        let rows = klines.rows().await;
        let closed_rows = rows.iter().filter(|row| row.state.is_closed()).collect::<Vec<_>>();
        if closed_rows.len() < 2 {
            continue;
        }

        let current = closed_rows[closed_rows.len() - 1];
        if processed_bar_id == Some(current.kline.id) {
            continue;
        }

        let previous = closed_rows[closed_rows.len() - 2];
        let target = if current.kline.close > previous.kline.close {
            position_size
        } else if current.kline.close < previous.kline.close {
            -position_size
        } else {
            0
        };
        task.set_target_volume(target).map_err(runtime_err)?;
        processed_bar_id = Some(current.kline.id);
        signal_count += 1;

        if let Some(snapshot) = quote.snapshot().await {
            println!(
                "{} close={:.2} prev_close={:.2} quote={:.2} target={} updated_quotes={}",
                step.current_dt,
                current.kline.close,
                previous.kline.close,
                snapshot.last_price,
                target,
                step.updated_quotes.len(),
            );
        }
    }

    let final_position = runtime
        .execution()
        .position(ACCOUNT_KEY, &symbol)
        .await
        .map_err(runtime_err)?;
    let result = session.finish().await?;
    let final_account = result
        .final_accounts
        .iter()
        .find(|account| account.user_id == ACCOUNT_KEY)
        .or_else(|| result.final_accounts.first());

    println!("\nsummary");
    println!("  steps:        {step_count}");
    println!("  signals:      {signal_count}");
    println!("  trades:       {}", result.trades.len());
    println!("  settlements:  {}", result.settlements.len());
    println!("  net position: {}", net_position(&final_position));
    if let Some(account) = final_account {
        println!("  balance:      {:.2}", account.balance);
        println!("  available:    {:.2}", account.available);
        println!("  close profit: {:.2}", account.close_profit);
    }

    Ok(())
}
