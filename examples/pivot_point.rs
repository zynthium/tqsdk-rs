//! 枢轴点反转策略回放示例。
//!
//! 参照 `tqsdk-python/tqsdk/demo/example/pivot_point.py`，使用
//! `ReplaySession` 在 Rust 中演示等价的日线回放交易逻辑。
//!
//! 关键对齐点：
//! - 信号在“新的一天开始”时触发，即处理日线 `Opening` 事件
//! - 枢轴点使用上一交易日的 high/low/close 计算
//! - 调仓通过 `TqRuntime` + `TargetPosTask` 完成

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;
use tqsdk_rs::Position;
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const DAILY_BAR: Duration = Duration::from_secs(60 * 60 * 24);
const MIN_DAILY_WIDTH: usize = 32;
const DEFAULT_SYMBOL: &str = "SHFE.cu2606";
const DEFAULT_POSITION_SIZE: i64 = 100;
const DEFAULT_START_DATE: (i32, u32, u32) = (2026, 4, 1);
const DEFAULT_END_DATE: (i32, u32, u32) = (2026, 4, 9);
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
    last_observed_price: Option<f64>,
}

fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env_date(name: &str, default: (i32, u32, u32)) -> StdResult<NaiveDate, Box<dyn Error>> {
    match env::var(name) {
        Ok(raw) => Ok(NaiveDate::parse_from_str(&raw, "%Y-%m-%d")?),
        Err(_) => {
            Ok(NaiveDate::from_ymd_opt(default.0, default.1, default.2)
                .ok_or_else(|| format!("默认日期无效: {name}"))?)
        }
    }
}

fn parse_env_i64(name: &str, default: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(default)
}

fn shanghai_range(
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StdResult<(DateTime<Utc>, DateTime<Utc>), Box<dyn Error>> {
    let tz = FixedOffset::east_opt(8 * 3600).ok_or("无法创建 Asia/Shanghai 时区")?;
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or("无效开始时间")?)
        .single()
        .ok_or("开始时间存在歧义")?
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or("无效结束时间")?)
        .single()
        .ok_or("结束时间存在歧义")?
        .with_timezone(&Utc);
    Ok((start_dt, end_dt))
}

fn history_view_width(start_date: NaiveDate, end_date: NaiveDate) -> usize {
    let span_days = (end_date - start_date).num_days().max(0) as usize;
    (span_days + 16).max(MIN_DAILY_WIDTH)
}

fn format_shanghai_nanos(nanos: i64) -> String {
    let dt = DateTime::<Utc>::from_timestamp_nanos(nanos);
    let tz = FixedOffset::east_opt(8 * 3600).expect("固定东八区时区应可用");
    dt.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()
}

fn net_position(position: &Position) -> i64 {
    (position.volume_long_today + position.volume_long_his) - (position.volume_short_today + position.volume_short_his)
}

fn find_final_position<'a>(positions: &'a [Position], account_key: &str, symbol: &str) -> Option<&'a Position> {
    positions
        .iter()
        .find(|position| {
            position.user_id == account_key && format!("{}.{}", position.exchange_id, position.instrument_id) == symbol
        })
        .or_else(|| positions.iter().find(|position| position.user_id == account_key))
        .or_else(|| positions.first())
}

async fn apply_target_and_wait(task: &TargetPosTask, volume: i64) -> StdResult<(), Box<dyn Error>> {
    task.set_target_volume(volume)?;
    task.wait_target_reached().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", DEFAULT_SYMBOL);
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let position_size = parse_env_i64("TQ_POSITION_SIZE", DEFAULT_POSITION_SIZE);
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
    let daily_bars = session.kline(&symbol, DAILY_BAR, daily_width).await?;
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let account = runtime.account(ACCOUNT_KEY).expect("configured account should exist");
    let task = account.target_pos(&symbol).build()?;

    println!("开始运行枢轴点反转策略...");
    println!("  合约: {}", symbol);
    println!("  区间: {} -> {}", start_date, end_date);
    println!("  日线窗口宽度: {}", daily_width);
    println!("  反转确认点数: {}", REVERSAL_CONFIRM);
    println!("  止损点数: {}", STOP_LOSS_POINTS);

    let mut strategy = StrategyState::default();
    let mut processed_bar_id = None;

    while let Some(step) = session.step().await? {
        if !step.updated_handles.iter().any(|id| id == daily_bars.id()) {
            continue;
        }

        let rows = daily_bars.rows().await;
        if rows.len() < 2 {
            continue;
        }

        let current = rows.last().expect("rows len checked");
        if !current.state.is_opening() || processed_bar_id == Some(current.kline.id) {
            continue;
        }

        let previous = &rows[rows.len() - 2];
        if !previous.state.is_closed() {
            continue;
        }

        processed_bar_id = Some(current.kline.id);
        strategy.processed_days += 1;
        strategy.last_observed_price = Some(current.kline.close);

        let current_price = current.kline.close;
        let current_high = current.kline.high;
        let current_low = current.kline.low;
        let levels = PivotLevels::from_previous_day(previous.kline.high, previous.kline.low, previous.kline.close);

        println!();
        println!("新的一天开始:");
        println!(
            "前一日数据 - 最高价: {:.2}, 最低价: {:.2}, 收盘价: {:.2}",
            previous.kline.high, previous.kline.low, previous.kline.close
        );
        println!("日期: {}", format_shanghai_nanos(current.kline.datetime));
        println!("当前价格: {:.2}", current_price);
        println!("枢轴点: {:.2}", levels.pivot);
        println!("支撑位: S1={:.2}, S2={:.2}, S3={:.2}", levels.s1, levels.s2, levels.s3);
        println!("阻力位: R1={:.2}, R2={:.2}, R3={:.2}", levels.r1, levels.r2, levels.r3);
        println!();
        println!("多头信号条件:");
        println!(
            "1. 价格在S1附近: {}",
            current_price <= levels.s1 + REVERSAL_CONFIRM && current_price > levels.s1 - REVERSAL_CONFIRM
        );
        println!("2. 价格高于当日最低价: {}", current_price > current_low);
        println!("3. 价格高于前一日收盘价: {}", current_price > previous.kline.close);
        println!();
        println!("空头信号条件:");
        println!(
            "1. 价格在R1附近: {}",
            current_price >= levels.r1 - REVERSAL_CONFIRM && current_price < levels.r1 + REVERSAL_CONFIRM
        );
        println!("2. 价格低于当日最高价: {}", current_price < current_high);
        println!("3. 价格低于前一日收盘价: {}", current_price < previous.kline.close);

        if strategy.current_direction == 0 {
            if current_price < levels.s1 {
                strategy.current_direction = 1;
                strategy.entry_price = current_price;
                strategy.stop_loss_price = current_price - STOP_LOSS_POINTS;
                strategy.signal_count += 1;
                apply_target_and_wait(&task, position_size).await?;
                println!(
                    "\n多头开仓信号! 开仓价: {:.2}, 止损价: {:.2}",
                    strategy.entry_price, strategy.stop_loss_price
                );
            } else if current_price > levels.r1 {
                strategy.current_direction = -1;
                strategy.entry_price = current_price;
                strategy.stop_loss_price = current_price + STOP_LOSS_POINTS;
                strategy.signal_count += 1;
                apply_target_and_wait(&task, -position_size).await?;
                println!(
                    "\n空头开仓信号! 开仓价: {:.2}, 止损价: {:.2}",
                    strategy.entry_price, strategy.stop_loss_price
                );
            }
            continue;
        }

        if strategy.current_direction > 0 {
            if current_price >= levels.pivot {
                let profit = (current_price - strategy.entry_price) * position_size as f64;
                strategy.realized_pnl += profit;
                strategy.current_direction = 0;
                strategy.signal_count += 1;
                apply_target_and_wait(&task, 0).await?;
                println!("多头止盈平仓: 价格={:.2}, 盈利={:.2}", current_price, profit);
            } else if current_price <= strategy.stop_loss_price {
                let loss = (strategy.entry_price - current_price) * position_size as f64;
                strategy.realized_pnl -= loss;
                strategy.current_direction = 0;
                strategy.signal_count += 1;
                apply_target_and_wait(&task, 0).await?;
                println!("多头止损平仓: 价格={:.2}, 亏损={:.2}", current_price, loss);
            }
        } else if current_price <= levels.pivot {
            let profit = (strategy.entry_price - current_price) * position_size as f64;
            strategy.realized_pnl += profit;
            strategy.current_direction = 0;
            strategy.signal_count += 1;
            apply_target_and_wait(&task, 0).await?;
            println!("空头止盈平仓: 价格={:.2}, 盈利={:.2}", current_price, profit);
        } else if current_price >= strategy.stop_loss_price {
            let loss = (current_price - strategy.entry_price) * position_size as f64;
            strategy.realized_pnl -= loss;
            strategy.current_direction = 0;
            strategy.signal_count += 1;
            apply_target_and_wait(&task, 0).await?;
            println!("空头止损平仓: 价格={:.2}, 亏损={:.2}", current_price, loss);
        }
    }

    task.cancel().await?;
    task.wait_finished().await?;

    let result = session.finish().await?;
    let final_net_position = find_final_position(&result.final_positions, ACCOUNT_KEY, &symbol)
        .map(net_position)
        .unwrap_or(0);
    let unrealized_pnl = match (strategy.current_direction, strategy.last_observed_price) {
        (1, Some(price)) => (price - strategy.entry_price) * position_size as f64,
        (-1, Some(price)) => (strategy.entry_price - price) * position_size as f64,
        _ => 0.0,
    };

    println!();
    println!("回测结束");
    println!("策略汇总:");
    println!("  处理交易日: {}", strategy.processed_days);
    println!("  信号次数: {}", strategy.signal_count);
    println!("  已实现盈亏: {:.2}", strategy.realized_pnl);
    println!("  未实现盈亏: {:.2}", unrealized_pnl);
    println!("  总成交笔数: {}", result.trades.len());
    println!("  最终净持仓: {}", final_net_position);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_python_reference_script() {
        assert_eq!(DEFAULT_SYMBOL, "SHFE.cu2606");
        assert_eq!(DEFAULT_POSITION_SIZE, 100);
        assert_eq!(
            NaiveDate::from_ymd_opt(DEFAULT_START_DATE.0, DEFAULT_START_DATE.1, DEFAULT_START_DATE.2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
        assert_eq!(
            NaiveDate::from_ymd_opt(DEFAULT_END_DATE.0, DEFAULT_END_DATE.1, DEFAULT_END_DATE.2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 4, 9).unwrap()
        );
    }
}
