//! R-Breaker 策略回放示例。
//!
//! 参照 `tqsdk-python/tqsdk/demo/example/rbreaker.py` 实现。
//! Python 原始默认合约 `SHFE.au2006` 在当前 ReplaySession 中会被判定为不支持的交易标的，
//! 因此这里改用仓库内已验证可回放的黄金期货合约，保留策略公式和判定逻辑不变。

use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const INTRADAY_BAR: Duration = Duration::from_secs(60);
const DEFAULT_SYMBOL: &str = "SHFE.au2606";
const DEFAULT_START_DATE: (i32, u32, u32) = (2026, 4, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2026, 4, 30);
const TARGET_VOLUME: i64 = 3;
const STOP_LOSS_PRICE: f64 = 10.0;

#[derive(Debug, Clone, Copy)]
struct DayBar {
    high: f64,
    low: f64,
    close: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RBreakerLevels {
    pivot: f64,
    b_break: f64,
    s_setup: f64,
    s_enter: f64,
    b_enter: f64,
    b_setup: f64,
    s_break: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StrategyState {
    target: i64,
    open_price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QuoteState {
    last_price: f64,
    highest: f64,
    lowest: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RBreakerAction {
    None,
    StopLossFlat,
    ReverseShort,
    ReverseLong,
    BreakLong,
    BreakShort,
}

fn compute_rbreaker_levels(previous: DayBar) -> RBreakerLevels {
    let pivot = (previous.high + previous.low + previous.close) / 3.0;
    let b_break = previous.high + 2.0 * (pivot - previous.low);
    let s_setup = pivot + (previous.high - previous.low);
    let s_enter = 2.0 * pivot - previous.low;
    let b_enter = 2.0 * pivot - previous.high;
    let b_setup = pivot - (previous.high - previous.low);
    let s_break = previous.low - 2.0 * (previous.high - pivot);

    RBreakerLevels {
        pivot,
        b_break,
        s_setup,
        s_enter,
        b_enter,
        b_setup,
        s_break,
    }
}

fn evaluate_rbreaker(
    state: StrategyState,
    levels: RBreakerLevels,
    quote: QuoteState,
    stop_loss_price: f64,
    target_volume: i64,
) -> (StrategyState, RBreakerAction) {
    let mut next = state;
    let mut stopped_out = false;

    if (next.target > 0 && next.open_price - quote.last_price >= stop_loss_price)
        || (next.target < 0 && quote.last_price - next.open_price >= stop_loss_price)
    {
        next.target = 0;
        stopped_out = true;
    }

    let action = if next.target > 0 {
        if quote.highest > levels.s_setup && quote.last_price < levels.s_enter {
            next.target = -target_volume;
            next.open_price = quote.last_price;
            RBreakerAction::ReverseShort
        } else {
            RBreakerAction::None
        }
    } else if next.target < 0 {
        if quote.lowest < levels.b_setup && quote.last_price > levels.b_enter {
            next.target = target_volume;
            next.open_price = quote.last_price;
            RBreakerAction::ReverseLong
        } else {
            RBreakerAction::None
        }
    } else if quote.last_price > levels.b_break {
        next.target = target_volume;
        next.open_price = quote.last_price;
        RBreakerAction::BreakLong
    } else if quote.last_price < levels.s_break {
        next.target = -target_volume;
        next.open_price = quote.last_price;
        RBreakerAction::BreakShort
    } else if stopped_out && state.target != 0 {
        RBreakerAction::StopLossFlat
    } else {
        RBreakerAction::None
    };

    (next, action)
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
    (span_days + 16).max(32)
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
    let intraday_bars = session.kline(&symbol, INTRADAY_BAR, 8).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("策略开始运行");

    let mut levels = None;
    let mut processed_daily_bar_id = None;
    let mut processed_intraday_bar_id = None;
    let mut state = StrategyState {
        target: 0,
        open_price: 0.0,
    };

    while let Some(step) = session.step().await? {
        if step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            let rows = daily_bars.rows().await;
            if rows.len() >= 2 {
                let current = rows.last().expect("rows checked");
                if processed_daily_bar_id != Some(current.kline.id) {
                    let previous = &rows[rows.len() - 2];
                    processed_daily_bar_id = Some(current.kline.id);
                    levels = Some(compute_rbreaker_levels(DayBar {
                        high: previous.kline.high,
                        low: previous.kline.low,
                        close: previous.kline.close,
                    }));
                    if let Some(current_levels) = levels {
                        println!(
                            "已计算新标志线, 枢轴点: {:.6}, 突破买入价: {:.6}, 观察卖出价: {:.6}, 反转卖出价: {:.6}, 反转买入价: {:.6}, 观察买入价: {:.6}, 突破卖出价: {:.6}",
                            current_levels.pivot,
                            current_levels.b_break,
                            current_levels.s_setup,
                            current_levels.s_enter,
                            current_levels.b_enter,
                            current_levels.b_setup,
                            current_levels.s_break
                        );
                    }
                }
            }
        }

        if !step.updated_handles.iter().any(|id| id == intraday_bars.id()) {
            continue;
        }
        let intraday_rows = intraday_bars.rows().await;
        let Some(last_intraday) = intraday_rows.last() else {
            continue;
        };
        if !last_intraday.state.is_closed() || processed_intraday_bar_id == Some(last_intraday.kline.id) {
            continue;
        }
        processed_intraday_bar_id = Some(last_intraday.kline.id);

        let Some(current_levels) = levels else {
            continue;
        };
        let Some(snapshot) = quote.snapshot().await else {
            continue;
        };

        let quote_state = QuoteState {
            last_price: snapshot.last_price,
            highest: snapshot.highest,
            lowest: snapshot.lowest,
        };
        let (next_state, action) =
            evaluate_rbreaker(state, current_levels, quote_state, STOP_LOSS_PRICE, TARGET_VOLUME);
        if action == RBreakerAction::None || next_state.target == state.target {
            state = next_state;
            continue;
        }

        match action {
            RBreakerAction::StopLossFlat => println!("触发止损: 平仓"),
            RBreakerAction::ReverseShort => println!("多头持仓,当日内最高价超过观察卖出价后跌破反转卖出价: 反手做空"),
            RBreakerAction::ReverseLong => println!("空头持仓,当日最低价低于观察买入价后超过反转买入价: 反手做多"),
            RBreakerAction::BreakLong => println!("空仓,盘中价格超过突破买入价: 开仓做多"),
            RBreakerAction::BreakShort => println!("空仓,盘中价格跌破突破卖出价: 开仓做空"),
            RBreakerAction::None => {}
        }

        state = next_state;
        task.set_target_volume(state.target)?;
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
    fn computes_rbreaker_lines_from_previous_day() {
        let levels = compute_rbreaker_levels(DayBar {
            high: 110.0,
            low: 90.0,
            close: 100.0,
        });

        assert!((levels.pivot - 100.0).abs() < 1e-9);
        assert!((levels.b_break - 130.0).abs() < 1e-9);
        assert!((levels.s_setup - 120.0).abs() < 1e-9);
        assert!((levels.s_enter - 110.0).abs() < 1e-9);
        assert!((levels.b_enter - 90.0).abs() < 1e-9);
        assert!((levels.b_setup - 80.0).abs() < 1e-9);
        assert!((levels.s_break - 70.0).abs() < 1e-9);
    }

    #[test]
    fn breakout_and_reverse_follow_python_ordering() {
        let levels = compute_rbreaker_levels(DayBar {
            high: 110.0,
            low: 90.0,
            close: 100.0,
        });
        let flat = StrategyState {
            target: 0,
            open_price: 0.0,
        };
        let (long_state, long_action) = evaluate_rbreaker(
            flat,
            levels,
            QuoteState {
                last_price: 131.0,
                highest: 131.0,
                lowest: 99.0,
            },
            STOP_LOSS_PRICE,
            TARGET_VOLUME,
        );
        assert_eq!(long_action, RBreakerAction::BreakLong);
        assert_eq!(long_state.target, 3);
        assert_eq!(long_state.open_price, 131.0);

        let (short_state, short_action) = evaluate_rbreaker(
            StrategyState {
                target: 3,
                open_price: 115.0,
            },
            levels,
            QuoteState {
                last_price: 109.0,
                highest: 121.0,
                lowest: 95.0,
            },
            STOP_LOSS_PRICE,
            TARGET_VOLUME,
        );
        assert_eq!(short_action, RBreakerAction::ReverseShort);
        assert_eq!(short_state.target, -3);
        assert_eq!(short_state.open_price, 109.0);
    }

    #[test]
    fn stop_loss_flattens_when_no_new_breakout_appears() {
        let levels = compute_rbreaker_levels(DayBar {
            high: 110.0,
            low: 90.0,
            close: 100.0,
        });
        let state = StrategyState {
            target: 3,
            open_price: 120.0,
        };
        let (next_state, action) = evaluate_rbreaker(
            state,
            levels,
            QuoteState {
                last_price: 109.5,
                highest: 115.0,
                lowest: 100.0,
            },
            STOP_LOSS_PRICE,
            TARGET_VOLUME,
        );
        assert_eq!(action, RBreakerAction::StopLossFlat);
        assert_eq!(next_state.target, 0);
    }
}
