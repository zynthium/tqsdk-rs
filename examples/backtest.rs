//! ReplaySession 回测示例
//!
//! 演示以下能力：
//! - 通过 `Client::create_backtest_session` 创建历史回放会话
//! - 注册回测 Quote 与 1 分钟 K 线
//! - 使用 `TqRuntime` + `TargetPosTask` 在回测时间轴上驱动调仓
//! - 回放结束后读取成交、账户和持仓汇总

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use std::env;
use std::error::Error;
use std::result::Result as StdResult;
use std::time::Duration;
use tqsdk_rs::Position;
use tqsdk_rs::prelude::*;

const ACCOUNT_KEY: &str = "TQSIM";
const KLINE_DURATION: Duration = Duration::from_secs(60);
const DEFAULT_KLINE_WIDTH: usize = 128;
const DEFAULT_POSITION_SIZE: i64 = 1;

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

fn format_shanghai(dt: DateTime<Utc>) -> String {
    let tz = FixedOffset::east_opt(8 * 3600).expect("固定东八区时区应可用");
    dt.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()
}

fn net_position(position: &Position) -> i64 {
    (position.volume_long_today + position.volume_long_his) - (position.volume_short_today + position.volume_short_his)
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn Error>> {
    let username = env::var("TQ_AUTH_USER")?;
    let password = env::var("TQ_AUTH_PASS")?;
    let symbol = env_or_default("TQ_TEST_SYMBOL", "SHFE.au2606");
    let log_level = env_or_default("TQ_LOG_LEVEL", "info");
    let position_size = parse_env_i64("TQ_POSITION_SIZE", DEFAULT_POSITION_SIZE);
    let start_date = parse_env_date("TQ_START_DT", (2026, 4, 6))?;
    let end_date = parse_env_date("TQ_END_DT", (2026, 4, 7))?;
    let (start_dt, end_dt) = shanghai_range(start_date, end_date)?;

    init_logger(&log_level, false);

    let mut client = Client::builder(&username, &password)
        .endpoints(EndpointConfig::from_env())
        .log_level(log_level)
        .view_width(2048)
        .build()
        .await?;

    let mut session = client
        .create_backtest_session(ReplayConfig::new(start_dt, end_dt)?)
        .await?;
    let quote = session.quote(&symbol).await?;
    let bars = {
        let mut series = session.series();
        series.kline(&symbol, KLINE_DURATION, DEFAULT_KLINE_WIDTH).await?
    };
    let runtime = session.runtime([ACCOUNT_KEY]).await?;
    let task = TargetPosTask::new(runtime.clone(), ACCOUNT_KEY, &symbol, TargetPosTaskOptions::default()).await?;

    println!("开始运行 ReplaySession 回测示例...");
    println!("  合约: {}", symbol);
    println!("  区间: {} -> {}", format_shanghai(start_dt), format_shanghai(end_dt));
    println!("  K线周期: {} 秒", KLINE_DURATION.as_secs());
    println!("  调仓手数: {}", position_size);

    let mut processed_bar_id = None;
    let mut step_count = 0usize;
    let mut signal_count = 0usize;

    while let Some(step) = session.step().await? {
        step_count += 1;

        if !step.updated_handles.iter().any(|id| id == bars.id()) {
            continue;
        }

        let rows = bars.rows().await;
        if rows.len() < 2 {
            continue;
        }

        let last = rows.last().expect("rows len checked");
        if !last.state.is_closed() || processed_bar_id == Some(last.kline.id) {
            continue;
        }

        let prev = &rows[rows.len() - 2];
        let target = if last.kline.close > prev.kline.close {
            position_size
        } else {
            0
        };

        processed_bar_id = Some(last.kline.id);
        signal_count += 1;
        task.set_target_volume(target)?;

        let replay_quote = quote.snapshot().await;
        println!(
            "[{}] prev_close={:.2} close={:.2} latest_quote={:.2} target={}",
            format_shanghai(step.current_dt),
            prev.kline.close,
            last.kline.close,
            replay_quote.map(|item| item.last_price).unwrap_or(f64::NAN),
            target
        );
    }

    task.cancel().await?;
    task.wait_finished().await?;

    let result = session.finish().await?;
    let final_position = runtime.execution().position(ACCOUNT_KEY, &symbol).await?;
    let final_account = result
        .final_accounts
        .iter()
        .find(|account| account.user_id == ACCOUNT_KEY)
        .or_else(|| result.final_accounts.first());

    println!();
    println!("回测结束");
    println!("  推进步数: {}", step_count);
    println!("  调仓信号: {}", signal_count);
    println!("  总成交笔数: {}", result.trades.len());
    println!("  最终净持仓: {}", net_position(&final_position));
    if let Some(account) = final_account {
        println!("  账户权益: {:.2}", account.balance);
        println!("  可用资金: {:.2}", account.available);
    }

    Ok(())
}
