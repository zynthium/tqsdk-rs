//! 枢轴点反转策略回放示例。
//!
//! 这个版本基于 `ReplaySession`：
//! - `Client::create_backtest_session`
//! - `ReplaySession::quote`
//! - `ReplaySession::series().kline`
//! - `ReplaySession::runtime`
//! - `ReplaySession::step` / `finish`

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use std::env;
use std::time::Duration;
use tqsdk_rs::Position;
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const DAILY_WIDTH: usize = 64;
const REVERSAL_CONFIRM: f64 = 50.0;
const STOP_LOSS_POINTS: f64 = 100.0;

#[derive(Debug, Clone, Copy)]
struct PivotLevels {
    pivot: f64,
    r1: f64,
    r2: f64,
    r3: f64,
    s1: f64,
    s2: f64,
    s3: f64,
}

impl PivotLevels {
    fn from_previous_day(high: f64, low: f64, close: f64) -> Self {
        let pivot = (high + low + close) / 3.0;
        let r1 = (2.0 * pivot) - low;
        let s1 = (2.0 * pivot) - high;
        let range = high - low;
        let r2 = pivot + range;
        let s2 = pivot - range;
        let r3 = r1 + range;
        let s3 = s1 - range;
        Self {
            pivot,
            r1,
            r2,
            r3,
            s1,
            s2,
            s3,
        }
    }
}

#[derive(Debug, Default)]
struct StrategyState {
    current_direction: i64,
    entry_price: f64,
    stop_loss_price: f64,
    realized_pnl: f64,
    processed_days: usize,
    signal_count: usize,
}

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

fn format_cst_datetime(nanos: i64) -> String {
    let dt = DateTime::<Utc>::from_timestamp_nanos(nanos);
    let tz = FixedOffset::east_opt(8 * 3600).expect("fixed offset should be valid");
    dt.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()
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
    let symbol = env_or_default("TQ_TEST_SYMBOL", "SHFE.cu2606");
    let position_size = parse_env_i64("TQ_POSITION_SIZE", 1);
    let start_date = parse_env_date("TQ_START_DT", (2026, 4, 1))?;
    let end_date = parse_env_date("TQ_END_DT", (2026, 4, 15))?;
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
    let daily = session.series().kline(&symbol, DAILY_BAR, DAILY_WIDTH).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let task = TargetPosTask::new(runtime.clone(), ACCOUNT_KEY, &symbol, TargetPosTaskOptions::default())
        .await
        .map_err(runtime_err)?;

    println!("pivot-point replay");
    println!("  symbol: {symbol}");
    println!("  range:  {start_date} -> {end_date}");
    println!("  size:   {position_size}");
    println!("  reversal_confirm: {REVERSAL_CONFIRM}");
    println!("  stop_loss_points: {STOP_LOSS_POINTS}");

    let mut strategy = StrategyState::default();
    let mut processed_bar_id = None;

    while session.step().await?.is_some() {
        let rows = daily.rows().await;
        let closed_rows = rows.iter().filter(|row| row.state.is_closed()).collect::<Vec<_>>();
        if closed_rows.len() < 2 {
            continue;
        }

        let current_day = closed_rows[closed_rows.len() - 1];
        if processed_bar_id == Some(current_day.kline.id) {
            continue;
        }
        let previous_day = closed_rows[closed_rows.len() - 2];
        processed_bar_id = Some(current_day.kline.id);
        strategy.processed_days += 1;

        let levels = PivotLevels::from_previous_day(
            previous_day.kline.high,
            previous_day.kline.low,
            previous_day.kline.close,
        );
        let current_price = current_day.kline.close;
        let snapshot_price = quote
            .snapshot()
            .await
            .map(|snapshot| snapshot.last_price)
            .unwrap_or(current_price);

        println!("\n日期: {}", format_cst_datetime(current_day.kline.datetime));
        println!(
            "前一日 HLC: {:.2} / {:.2} / {:.2}",
            previous_day.kline.high, previous_day.kline.low, previous_day.kline.close
        );
        println!(
            "枢轴点: P={:.2}, R1={:.2}, R2={:.2}, R3={:.2}, S1={:.2}, S2={:.2}, S3={:.2}",
            levels.pivot, levels.r1, levels.r2, levels.r3, levels.s1, levels.s2, levels.s3
        );
        println!("当前收盘价: {:.2}, replay quote: {:.2}", current_price, snapshot_price);

        if strategy.current_direction == 0 {
            if current_price <= levels.s1 + REVERSAL_CONFIRM && current_price < levels.s1 {
                strategy.current_direction = 1;
                strategy.entry_price = current_price;
                strategy.stop_loss_price = current_price - STOP_LOSS_POINTS;
                strategy.signal_count += 1;
                task.set_target_volume(position_size).map_err(runtime_err)?;
                println!(
                    "多头开仓: entry={:.2}, stop={:.2}",
                    strategy.entry_price, strategy.stop_loss_price
                );
            } else if current_price >= levels.r1 - REVERSAL_CONFIRM && current_price > levels.r1 {
                strategy.current_direction = -1;
                strategy.entry_price = current_price;
                strategy.stop_loss_price = current_price + STOP_LOSS_POINTS;
                strategy.signal_count += 1;
                task.set_target_volume(-position_size).map_err(runtime_err)?;
                println!(
                    "空头开仓: entry={:.2}, stop={:.2}",
                    strategy.entry_price, strategy.stop_loss_price
                );
            }
            continue;
        }

        if strategy.current_direction > 0 {
            if current_price >= levels.pivot {
                strategy.realized_pnl += (current_price - strategy.entry_price) * position_size as f64;
                strategy.current_direction = 0;
                strategy.signal_count += 1;
                task.set_target_volume(0).map_err(runtime_err)?;
                println!("多头止盈: price={current_price:.2}, pnl={:.2}", strategy.realized_pnl);
            } else if current_price <= strategy.stop_loss_price {
                strategy.realized_pnl += (current_price - strategy.entry_price) * position_size as f64;
                strategy.current_direction = 0;
                strategy.signal_count += 1;
                task.set_target_volume(0).map_err(runtime_err)?;
                println!("多头止损: price={current_price:.2}, pnl={:.2}", strategy.realized_pnl);
            }
        } else if current_price <= levels.pivot {
            strategy.realized_pnl += (strategy.entry_price - current_price) * position_size as f64;
            strategy.current_direction = 0;
            strategy.signal_count += 1;
            task.set_target_volume(0).map_err(runtime_err)?;
            println!("空头止盈: price={current_price:.2}, pnl={:.2}", strategy.realized_pnl);
        } else if current_price >= strategy.stop_loss_price {
            strategy.realized_pnl += (strategy.entry_price - current_price) * position_size as f64;
            strategy.current_direction = 0;
            strategy.signal_count += 1;
            task.set_target_volume(0).map_err(runtime_err)?;
            println!("空头止损: price={current_price:.2}, pnl={:.2}", strategy.realized_pnl);
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
    println!("  processed_days: {}", strategy.processed_days);
    println!("  signals:        {}", strategy.signal_count);
    println!("  realized_pnl:   {:.2}", strategy.realized_pnl);
    println!("  trades:         {}", result.trades.len());
    println!("  settlements:    {}", result.settlements.len());
    println!("  net_position:   {}", net_position(&final_position));
    if let Some(account) = final_account {
        println!("  balance:        {:.2}", account.balance);
        println!("  available:      {:.2}", account.available);
        println!("  close_profit:   {:.2}", account.close_profit);
    }

    Ok(())
}
