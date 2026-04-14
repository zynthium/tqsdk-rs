use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const SHORT: usize = 30;
const LONG: usize = 60;
const TARGET_VOLUME: i64 = 3;
const BAR_DURATION: Duration = Duration::from_secs(60);
const DEFAULT_SYMBOL: &str = "SHFE.bu2012";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 9, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 11, 30);

fn sma(values: &[f64], window: usize) -> Option<f64> {
    if values.len() < window {
        return None;
    }

    let tail = &values[values.len() - window..];
    Some(tail.iter().sum::<f64>() / window as f64)
}

fn cross_signal(closes: &[f64], short: usize, long: usize, target: i64) -> Option<i64> {
    if closes.len() < long + 1 {
        return None;
    }

    let prev = &closes[..closes.len() - 1];
    let prev_short = sma(prev, short)?;
    let prev_long = sma(prev, long)?;
    let curr_short = sma(closes, short)?;
    let curr_long = sma(closes, long)?;

    if prev_long < prev_short && curr_long > curr_short {
        Some(-target)
    } else if prev_short < prev_long && curr_short > curr_long {
        Some(target)
    } else {
        None
    }
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> StdResult<NaiveDate, Box<dyn Error>> {
    match env::var(name) {
        Ok(raw) => Ok(NaiveDate::parse_from_str(&raw, "%Y-%m-%d")?),
        Err(_) => Ok(NaiveDate::from_ymd_opt(default.0, default.1, default.2)
            .ok_or_else(|| format!("invalid default date: {name}"))?),
    }
}

fn shanghai_range(
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StdResult<(DateTime<Utc>, DateTime<Utc>), Box<dyn Error>> {
    let tz = FixedOffset::east_opt(8 * 3600).ok_or("unable to build Asia/Shanghai offset")?;
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or("invalid start time")?)
        .single()
        .ok_or("ambiguous start time")?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or("invalid end time")?)
        .single()
        .ok_or("ambiguous end time")?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let (start_dt, strategy_end_dt) = shanghai_range(start_date, end_date)?;
    let replay_end_date = end_date
        .succ_opt()
        .ok_or_else(|| format!("cannot extend replay end date beyond {end_date}"))?;
    let (_, replay_end_dt) = shanghai_range(start_date, replay_end_date)?;

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, replay_end_dt)?)
        .await?;
    let _quote = session.quote(&symbol).await?;
    let bars = session.kline(&symbol, BAR_DURATION, LONG + 4).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut processed_bar_id = None;
    let mut drained = false;
    while let Some(step) = session.step().await? {
        if step.current_dt > strategy_end_dt {
            if !drained {
                task.set_target_volume(0)?;
                drained = true;
            }
            continue;
        }

        if !step.updated_handles.iter().any(|id| id == bars.id()) {
            continue;
        }

        let rows = bars.rows().await;
        if rows.len() < LONG + 1 {
            continue;
        }

        let last = rows.last().expect("rows checked");
        if !last.state.is_closed() || processed_bar_id == Some(last.kline.id) {
            continue;
        }

        processed_bar_id = Some(last.kline.id);
        let closes = rows.iter().map(|row| row.kline.close).collect::<Vec<_>>();
        if let Some(target) = cross_signal(&closes, SHORT, LONG, TARGET_VOLUME) {
            if target > 0 {
                println!("均线上穿，做多");
            } else {
                println!("均线下穿，做空");
            }
            task.set_target_volume(target)?;
        }
    }

    task.cancel().await?;
    task.wait_finished().await?;
    session.finish().await?;
    println!("回测结束");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cross_signal;

    #[test]
    fn bullish_cross_sets_long_target() {
        let closes = vec![10.0, 10.0, 10.0, 9.0, 12.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(3));
    }

    #[test]
    fn bearish_cross_sets_short_target() {
        let closes = vec![10.0, 10.0, 10.0, 11.0, 8.0];
        assert_eq!(cross_signal(&closes, 2, 3, 3), Some(-3));
    }
}
