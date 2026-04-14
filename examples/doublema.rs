use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

#[path = "support/replay_dates.rs"]
mod replay_dates;

use chrono::{DateTime, NaiveDate, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const SHORT: usize = 30;
const LONG: usize = 60;
const TARGET_VOLUME: i64 = 3;
const BAR_DURATION: Duration = Duration::from_secs(60);
const DEFAULT_SYMBOL: &str = "SHFE.bu2012";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 9, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 11, 30);
const WARMUP_TRADING_DAYS: usize = 1;

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

fn strategy_start_dt(date: NaiveDate) -> StdResult<DateTime<Utc>, Box<dyn Error>> {
    replay_dates::trading_day_start_dt(date)
}

fn strategy_end_dt(end_date: NaiveDate) -> StdResult<DateTime<Utc>, Box<dyn Error>> {
    replay_dates::trading_day_end_dt(end_date)
}

fn previous_trading_day(date: NaiveDate) -> NaiveDate {
    replay_dates::previous_trading_day(date)
}

fn replay_start_date(mut start_date: NaiveDate, warmup_trading_days: usize) -> NaiveDate {
    for _ in 0..warmup_trading_days {
        start_date = previous_trading_day(start_date);
    }
    start_date
}

fn trading_day_of(dt: DateTime<Utc>) -> NaiveDate {
    replay_dates::trading_day_of(dt)
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "warn");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let replay_start_dt = strategy_start_dt(replay_start_date(start_date, WARMUP_TRADING_DAYS))?;
    let replay_end_dt = strategy_end_dt(end_date)?;

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(replay_start_dt, replay_end_dt)?)
        .await?;
    let _quote = session.quote(&symbol).await?;
    let bars = session.kline(&symbol, BAR_DURATION, LONG + 4).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut processed_bar_id = None;
    while let Some(step) = session.step().await? {
        let trading_day = trading_day_of(step.current_dt);
        if trading_day < start_date || trading_day > end_date {
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
        if !last.state.is_opening() || processed_bar_id == Some(last.kline.id) {
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
    use chrono::{FixedOffset, TimeZone, Timelike};

    use super::{cross_signal, replay_start_date, strategy_end_dt, strategy_start_dt, trading_day_of};

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

    #[test]
    fn night_session_counts_as_next_trading_day() {
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        let night = cst
            .with_ymd_and_hms(2020, 8, 31, 22, 21, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end_night = cst
            .with_ymd_and_hms(2020, 11, 30, 21, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            trading_day_of(night),
            chrono::NaiveDate::from_ymd_opt(2020, 9, 1).unwrap()
        );
        assert_eq!(
            trading_day_of(end_night),
            chrono::NaiveDate::from_ymd_opt(2020, 12, 1).unwrap()
        );
    }

    #[test]
    fn replay_warmup_moves_back_one_trading_day() {
        let start = chrono::NaiveDate::from_ymd_opt(2020, 9, 1).unwrap();
        assert_eq!(
            replay_start_date(start, 1),
            chrono::NaiveDate::from_ymd_opt(2020, 8, 31).unwrap()
        );
    }

    #[test]
    fn replay_window_start_matches_python_trading_day_semantics() {
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        let replay_start = strategy_start_dt(chrono::NaiveDate::from_ymd_opt(2020, 8, 31).unwrap()).unwrap();
        let expected = cst
            .with_ymd_and_hms(2020, 8, 28, 18, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(replay_start, expected);
    }

    #[test]
    fn strategy_end_stops_before_end_date_night_session() {
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        let end = strategy_end_dt(chrono::NaiveDate::from_ymd_opt(2020, 11, 30).unwrap()).unwrap();
        let expected = cst
            .with_ymd_and_hms(2020, 11, 30, 17, 59, 59)
            .single()
            .unwrap()
            .with_nanosecond(999_999_999)
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(end, expected);
    }
}
