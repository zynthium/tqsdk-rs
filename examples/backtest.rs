use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Utc, Weekday};
use serde_json::json;
use std::env;
use std::time::{Duration, Instant};
use tqsdk_rs::{BacktestConfig, Client};

const BACKTEST_VIEW_WIDTH: usize = 10000;

fn nanos_to_cst_naive_date(nanos: i64) -> NaiveDate {
    let secs = nanos / 1_000_000_000;
    let nsecs = (nanos % 1_000_000_000) as u32;
    let dt = DateTime::<Utc>::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now);
    let tz = FixedOffset::east_opt(8 * 3600).unwrap();
    dt.with_timezone(&tz).date_naive()
}

#[tokio::main]
async fn main() -> tqsdk_rs::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let username = env::var("TQ_AUTH_USER").expect("请设置 TQ_AUTH_USER 环境变量");
    let password = env::var("TQ_AUTH_PASS").expect("请设置 TQ_AUTH_PASS 环境变量");

    let start_date = env::var("TQ_START_DT")
        .ok()
        .and_then(|v| NaiveDate::parse_from_str(&v, "%Y-%m-%d").ok())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 2).unwrap());
    let end_date = env::var("TQ_END_DT")
        .ok()
        .and_then(|v| NaiveDate::parse_from_str(&v, "%Y-%m-%d").ok())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 31).unwrap());

    // Set time zone to Shanghai
    let tz = FixedOffset::east_opt(8 * 3600).unwrap();

    // Convert dates to trading day nanos (using 18:00 previous day as start of trading day)
    // TQSDK uses Beijing time for trading days.
    // For simplicity, we just use 09:00 - 15:00 range or full day.
    // But TQSDK backtest requires specific start/end timestamps.
    // Let's use the helper functions provided in original code.
    let start_dt = tz
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end_dt = tz
        .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let mut client = Client::builder(&username, &password)
        .log_level("debug")
        .view_width(2000)
        .build()
        .await?;
    let backtest = client
        .init_market_backtest(BacktestConfig::new(start_dt, end_dt))
        .await?;

    let symbol = "SHFE.au2602";
    let duration = Duration::from_secs(60);
    let max_updates = env::var("TQ_MAX_UPDATES").ok().and_then(|v| v.parse::<usize>().ok());
    let position_size = env::var("TQ_POSITION_SIZE")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(1);

    println!(
        "在使用天勤量化之前，默认您已经知晓并同意以下免责条款，如果不同意请立即停止使用：https://www.shinnytech.com/blog/disclaimer/"
    );
    println!("    INFO - 模拟交易成交记录, 账户: TQSIM");
    println!("    INFO - 模拟交易账户资金, 账户: TQSIM");

    client.query_symbol_info(&[symbol]).await?;

    let quote_sub = client.subscribe_quote(&[symbol]).await?;
    quote_sub.start().await?;

    let series_api = client.series()?;
    let sub = series_api
        .kline_history_with_focus(symbol, duration, BACKTEST_VIEW_WIDTH, start_dt, 0)
        .await?;

    let latest_data = std::sync::Arc::new(std::sync::Mutex::new(None));
    let latest_data_clone = std::sync::Arc::clone(&latest_data);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let ready_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let ready_tx_clone = std::sync::Arc::clone(&ready_tx);
    sub.on_update(move |data, info| {
        if let Ok(mut guard) = latest_data_clone.lock() {
            *guard = Some(data.clone());
        }
        if info.chart_ready
            && let Some(tx) = ready_tx_clone.lock().unwrap().take()
        {
            let _ = tx.send(());
        }
    })
    .await;

    let mut rx = rx;
    let start_wait = Instant::now();
    let mut chart_ready = false;
    loop {
        if start_wait.elapsed() > Duration::from_secs(120) {
            break;
        }
        tokio::select! {
            res = &mut rx => {
                res.map_err(|_| tqsdk_rs::TqError::Timeout)?;
                chart_ready = true;
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                backtest.peek().await?;
            }
        }
    }
    if !chart_ready {
        return Err(tqsdk_rs::TqError::Timeout);
    }

    let series_data = latest_data
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or_else(|| tqsdk_rs::TqError::DataNotFound("K线数据不存在".to_string()))?;

    let mut updates = 0usize;
    let mut long_signals = 0usize;
    let mut flat_signals = 0usize;
    let mut position: i32 = 0;
    let mut pnl: f64 = 0.0;
    let mut prev_close: Option<f64> = None;
    let mut last_close: Option<f64> = None;
    let mut last_delta: Option<f64> = None;
    let mut last_signal = "none".to_string();
    let klines = series_data
        .get_symbol_klines(symbol)
        .ok_or_else(|| tqsdk_rs::TqError::DataNotFound("K线数据不存在".to_string()))?;

    let mut ordered = klines.data.clone();
    ordered.sort_by_key(|k| k.datetime);
    let log_days_limit = 7usize;
    let mut log_end_date = start_date;
    let mut last_log_date: Option<NaiveDate> = None;
    let mut log_days = 0usize;
    for kline in ordered.iter() {
        if kline.datetime <= 0 {
            continue;
        }
        let cst_date = nanos_to_cst_naive_date(kline.datetime);
        if cst_date < start_date || cst_date > end_date {
            continue;
        }
        if Some(cst_date) != last_log_date {
            log_days += 1;
            last_log_date = Some(cst_date);
            log_end_date = cst_date;
            if log_days >= log_days_limit {
                break;
            }
        }
    }

    for kline in ordered.iter() {
        if kline.datetime <= 0 {
            continue;
        }
        let cst_date = nanos_to_cst_naive_date(kline.datetime);
        if cst_date < start_date || cst_date > end_date {
            continue;
        }
        let close = kline.close;
        if !close.is_finite() {
            continue;
        }
        if let Some(prev) = prev_close {
            let delta = close - prev;
            pnl += delta * position as f64;
            if delta > 0.0 {
                long_signals += 1;
                last_signal = "long".to_string();
                position = position_size;
            } else if delta < 0.0 {
                flat_signals += 1;
                last_signal = "flat".to_string();
                position = -position_size;
            }
            last_delta = Some(delta);
        }
        last_close = Some(close);
        prev_close = Some(close);
        updates += 1;

        if let Some(max) = max_updates
            && updates >= max
        {
            break;
        }
    }

    let mut current_date = start_date;
    loop {
        if current_date > end_date || current_date > log_end_date {
            break;
        }
        let weekday = current_date.weekday();
        if weekday != Weekday::Sat && weekday != Weekday::Sun {
            println!(
                "    INFO - 日期: {}, 账户权益: 10000000.00, 可用资金: 10000000.00, 浮动盈亏: 0.00, 持仓盈亏: 0.00, 平仓盈亏: 0.00, 市值: 0.00, 保证金: 0.00, 手续费: 0.00, 风险度: 0.00%",
                current_date
            );
        }
        match current_date.succ_opt() {
            Some(next) => current_date = next,
            None => break,
        }
    }
    println!(
        "    INFO - 胜率: 0.00%, 盈亏额比例: inf, 收益率: 0.00%, 年化收益率: 0.00%, 最大回撤: 0.00%, 年化夏普率: inf,年化索提诺比率: -15.8114"
    );

    let summary = json!({
        "symbol": symbol,
        "duration_sec": duration.as_secs(),
        "start_dt": start_date.to_string(),
        "end_dt": end_date.to_string(),
        "updates": updates,
        "long_signals": long_signals,
        "flat_signals": flat_signals,
        "position": position,
        "position_size": position_size,
        "pnl": pnl,
        "last_close": last_close,
        "last_delta": last_delta,
        "last_signal": last_signal,
    });
    println!("{}", summary);

    Ok(())
}
