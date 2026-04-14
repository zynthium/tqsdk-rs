use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;

#[path = "support/python_trade_log.rs"]
mod python_trade_log;
#[path = "support/replay_dates.rs"]
mod replay_dates;

use chrono::{DateTime, NaiveDate, Utc};
use tqsdk_rs::prelude::*;
use tqsdk_rs::replay::BacktestResult;
use tqsdk_rs::replay::BarState;
use tqsdk_rs::{Account, Position};

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

fn should_refresh_levels(state: BarState, last_daily_bar_id: Option<i64>, current_bar_id: i64) -> bool {
    state.is_opening() && last_daily_bar_id != Some(current_bar_id)
}

fn should_process_quote_update(last_seen_price: Option<f64>, next_price: f64) -> bool {
    last_seen_price != Some(next_price)
}

fn should_prime_initial_quote(quote_stream_started: bool) -> bool {
    !quote_stream_started
}

fn history_view_width(start_date: NaiveDate, end_date: NaiveDate) -> usize {
    let span_days = (end_date - start_date).num_days().max(0) as usize;
    (span_days + 16).max(NDAY + 8)
}

fn find_final_account<'a>(result: &'a BacktestResult, account_key: &str) -> Option<&'a Account> {
    result
        .final_accounts
        .iter()
        .find(|account| account.user_id == account_key)
        .or_else(|| result.final_accounts.first())
}

fn net_position(position: &Position) -> i64 {
    position.volume_long - position.volume_short
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "warn");
    let start_date = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)?;
    let end_date = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)?;
    let replay_start_dt = strategy_start_dt(replay_start_date(start_date, NDAY))?;
    let replay_end_dt = strategy_end_dt(end_date)?;
    let replay_config = ReplayConfig::new(replay_start_dt, replay_end_dt)?;
    let daily_width = history_view_width(start_date, end_date);

    init_logger(&log_level, false);

    let client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client.create_backtest_session(replay_config.clone()).await?;
    let quote = session.quote(&symbol).await?;
    let daily_bars = session.kline(&symbol, DAILY_BAR, daily_width).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;
    let order_log_task = python_trade_log::spawn_order_logger(&account)?;

    println!("策略开始运行");

    let mut lines = None;
    let mut active_levels_day = None;
    let mut last_daily_bar_id = None;
    let mut last_seen_price = None;
    let mut last_sent_target = None;
    let mut quote_stream_started = false;

    while let Some(step) = session.step().await? {
        let trading_day = trading_day_of(step.current_dt);
        if trading_day < start_date || trading_day > end_date {
            continue;
        }
        if active_levels_day != Some(trading_day) {
            lines = None;
        }

        if step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            let rows = daily_bars.rows().await;
            if rows.len() > NDAY
                && let Some(last) = rows.last()
                && should_refresh_levels(last.state, last_daily_bar_id, last.kline.id)
            {
                let Some(previous) = rows.get(rows.len() - 2) else {
                    continue;
                };
                if !previous.state.is_closed() {
                    continue;
                }

                last_daily_bar_id = Some(last.kline.id);
                let bars = rows
                    .iter()
                    .map(|row| DayBar {
                        open: row.kline.open,
                        high: row.kline.high,
                        low: row.kline.low,
                        close: row.kline.close,
                    })
                    .collect::<Vec<_>>();
                lines = compute_dual_thrust(&bars, NDAY, K1, K2);
                active_levels_day = Some(trading_day);
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
        if should_prime_initial_quote(quote_stream_started) {
            quote_stream_started = true;
            last_seen_price = Some(snapshot.last_price);
            continue;
        }
        if !should_process_quote_update(last_seen_price, snapshot.last_price) {
            continue;
        }
        last_seen_price = Some(snapshot.last_price);

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
        if last_sent_target == Some(next_target) {
            continue;
        }
        if next_target > 0 {
            println!("高于上轨,目标持仓 多头3手");
        } else {
            println!("低于下轨,目标持仓 空头3手");
        }
        task.set_target_volume(next_target)?;
        last_sent_target = Some(next_target);
    }

    task.cancel().await?;
    task.wait_finished().await?;
    order_log_task.abort();
    let result = session.finish().await?;
    let final_account = find_final_account(&result, ACCOUNT_KEY);
    let final_net_position = result
        .final_positions
        .iter()
        .find(|position| {
            position.user_id == ACCOUNT_KEY && format!("{}.{}", position.exchange_id, position.instrument_id) == symbol
        })
        .map(net_position)
        .unwrap_or(0);

    println!();
    println!("回测结束");
    println!("  总成交笔数: {}", result.trades.len());
    println!("  最终净持仓: {}", final_net_position);
    if let Some(account) = final_account {
        let pnl = account.balance - replay_config.initial_balance;
        let pnl_ratio = pnl / replay_config.initial_balance * 100.0;
        println!("  初始资金: {:.2}", replay_config.initial_balance);
        println!("  结束资金: {:.2}", account.balance);
        println!("  收益率: {:.4}%", pnl_ratio);
        println!("  盈亏: {:.2}", pnl);
        println!("  平仓盈亏: {:.2}", account.close_profit);
        println!("  浮动盈亏: {:.2}", account.float_profit);
        println!("  持仓盈亏: {:.2}", account.position_profit);
        println!("  手续费: {:.2}", account.commission);
        println!("  可用资金: {:.2}", account.available);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

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

    #[test]
    fn replay_warmup_moves_back_nday_trading_days() {
        let start = NaiveDate::from_ymd_opt(2020, 9, 1).unwrap();
        assert_eq!(
            replay_start_date(start, NDAY),
            NaiveDate::from_ymd_opt(2020, 8, 25).unwrap()
        );
    }

    #[test]
    fn refreshes_levels_only_on_opening_bar_once_per_day() {
        assert!(should_refresh_levels(BarState::Opening, Some(1), 2));
        assert!(!should_refresh_levels(BarState::Closed, Some(1), 2));
        assert!(!should_refresh_levels(BarState::Opening, Some(2), 2));
    }

    #[test]
    fn processes_quote_only_when_last_price_changes() {
        assert!(should_process_quote_update(None, 3518.0));
        assert!(!should_process_quote_update(Some(3518.0), 3518.0));
        assert!(should_process_quote_update(Some(3518.0), 3519.0));
    }

    #[test]
    fn primes_only_the_first_quote_event() {
        assert!(should_prime_initial_quote(false));
        assert!(!should_prime_initial_quote(true));
    }
}
