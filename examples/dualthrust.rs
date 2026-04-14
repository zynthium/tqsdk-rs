use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const DEFAULT_SYMBOL: &str = "DCE.jd2011";
const DEFAULT_START_DATE: (i32, u32, u32) = (2020, 9, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2020, 10, 30);
const NDAY: usize = 5;
const K1: f64 = 0.2;
const K2: f64 = 0.2;
const TARGET_VOLUME: i64 = 3;

#[derive(Debug, Clone, Copy)]
struct DayBar {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DualThrustLines {
    current_open: f64,
    buy_line: f64,
    sell_line: f64,
}

fn compute_dual_thrust(days: &[DayBar], nday: usize, k1: f64, k2: f64) -> Option<DualThrustLines> {
    if days.len() < nday + 1 {
        return None;
    }

    let current = *days.last()?;
    let history = &days[days.len() - nday - 1..days.len() - 1];
    let hh = history.iter().map(|bar| bar.high).fold(f64::NEG_INFINITY, f64::max);
    let hc = history.iter().map(|bar| bar.close).fold(f64::NEG_INFINITY, f64::max);
    let lc = history.iter().map(|bar| bar.close).fold(f64::INFINITY, f64::min);
    let ll = history.iter().map(|bar| bar.low).fold(f64::INFINITY, f64::min);
    let range = (hh - lc).max(hc - ll);

    Some(DualThrustLines {
        current_open: current.open,
        buy_line: current.open + range * k1,
        sell_line: current.open - range * k2,
    })
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

fn history_view_width(start_date: NaiveDate, end_date: NaiveDate) -> usize {
    let span_days = (end_date - start_date).num_days().max(0) as usize;
    (span_days + 16).max(NDAY + 8)
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;
    let daily_width = history_view_width(start_date, end_date);

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;
    let quote = session.quote(&symbol).await?;
    let daily_bars = session.kline(&symbol, DAILY_BAR, daily_width).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut lines = None;
    let mut last_daily_bar_id = None;
    let mut current_target = 0_i64;

    while let Some(step) = session.step().await? {
        if step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            let rows = daily_bars.rows().await;
            let bars = rows
                .iter()
                .map(|row| DayBar {
                    open: row.kline.open,
                    high: row.kline.high,
                    low: row.kline.low,
                    close: row.kline.close,
                })
                .collect::<Vec<_>>();

            if let Some(last) = rows.last()
                && last_daily_bar_id != Some(last.kline.id)
            {
                last_daily_bar_id = Some(last.kline.id);
                lines = compute_dual_thrust(&bars, NDAY, K1, K2);
                if let Some(levels) = lines {
                    println!(
                        "当前开盘价: {:.6}, 上轨: {:.6}, 下轨: {:.6}",
                        levels.current_open, levels.buy_line, levels.sell_line
                    );
                }
            }
        }

        if !step.updated_quotes.iter().any(|updated| updated == &symbol) {
            continue;
        }

        let Some(levels) = lines else {
            continue;
        };
        let Some(snapshot) = quote.snapshot().await else {
            continue;
        };

        let next_target = if snapshot.last_price > levels.buy_line {
            Some(TARGET_VOLUME)
        } else if snapshot.last_price < levels.sell_line {
            Some(-TARGET_VOLUME)
        } else {
            None
        };

        let Some(next_target) = next_target else {
            continue;
        };
        if next_target == current_target {
            continue;
        }

        current_target = next_target;
        if next_target > 0 {
            println!("高于上轨,目标持仓 多头3手");
        } else {
            println!("低于下轨,目标持仓 空头3手");
        }
        task.set_target_volume(next_target)?;
    }

    task.cancel().await?;
    task.wait_finished().await?;
    session.finish().await?;
    println!("回测结束");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_dual_thrust_levels_from_python_formula() {
        let bars = vec![
            DayBar {
                open: 100.0,
                high: 110.0,
                low: 95.0,
                close: 105.0,
            },
            DayBar {
                open: 106.0,
                high: 111.0,
                low: 96.0,
                close: 107.0,
            },
            DayBar {
                open: 108.0,
                high: 112.0,
                low: 97.0,
                close: 109.0,
            },
            DayBar {
                open: 110.0,
                high: 115.0,
                low: 98.0,
                close: 111.0,
            },
            DayBar {
                open: 112.0,
                high: 116.0,
                low: 99.0,
                close: 113.0,
            },
            DayBar {
                open: 114.0,
                high: 118.0,
                low: 101.0,
                close: 115.0,
            },
        ];

        let levels = compute_dual_thrust(&bars, 5, 0.2, 0.2).unwrap();
        assert_eq!(levels.current_open, 114.0);
        assert!((levels.buy_line - 117.6).abs() < 1e-9);
        assert!((levels.sell_line - 110.4).abs() < 1e-9);
    }

    #[test]
    fn needs_current_day_and_nday_history() {
        let bars = vec![DayBar {
            open: 100.0,
            high: 110.0,
            low: 90.0,
            close: 105.0,
        }];
        assert!(compute_dual_thrust(&bars, 5, 0.2, 0.2).is_none());
    }
}
