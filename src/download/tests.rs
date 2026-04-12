use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};

use crate::types::{Kline, Tick};

use super::{DataDownloadRequest, DataDownloadSource, DataDownloader};

struct FakeDownloadSource {
    klines: HashMap<(String, i64), Vec<Kline>>,
    ticks: HashMap<String, Vec<Tick>>,
}

#[async_trait(?Send)]
impl DataDownloadSource for FakeDownloadSource {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Kline>> {
        Ok(self
            .klines
            .get(&(symbol.to_string(), duration.as_nanos() as i64))
            .cloned()
            .unwrap_or_default())
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<Tick>> {
        Ok(self.ticks.get(symbol).cloned().unwrap_or_default())
    }
}

#[tokio::test]
async fn data_downloader_aligns_secondary_kline_symbols_to_primary_timeline() {
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::from([
            (
                ("SHFE.rb2605".to_string(), 60_000_000_000),
                vec![
                    Kline {
                        id: 1,
                        datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                        open: 10.0,
                        high: 11.0,
                        low: 9.0,
                        close: 10.5,
                        open_oi: 100,
                        close_oi: 101,
                        volume: 5,
                        epoch: None,
                    },
                    Kline {
                        id: 2,
                        datetime: shanghai_nanos(2026, 4, 9, 9, 2, 0),
                        open: 11.0,
                        high: 12.0,
                        low: 10.0,
                        close: 11.5,
                        open_oi: 101,
                        close_oi: 102,
                        volume: 6,
                        epoch: None,
                    },
                ],
            ),
            (
                ("SHFE.hc2605".to_string(), 60_000_000_000),
                vec![Kline {
                    id: 8,
                    datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                    open: 20.0,
                    high: 21.0,
                    low: 19.0,
                    close: 20.5,
                    open_oi: 200,
                    close_oi: 201,
                    volume: 8,
                    epoch: None,
                }],
            ),
        ]),
        ticks: HashMap::new(),
    });

    let output = unique_temp_csv("kline-align");
    let request = DataDownloadRequest {
        symbols: vec!["SHFE.rb2605".to_string(), "SHFE.hc2605".to_string()],
        duration: Duration::from_secs(60),
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: output.clone(),
    };

    let downloader = DataDownloader::spawn_with_source(source, request).unwrap();
    downloader.wait().await.unwrap();

    let csv = std::fs::read_to_string(output).unwrap();
    assert!(csv.contains("SHFE.rb2605.open"));
    assert!(csv.contains("SHFE.hc2605.open"));
    assert!(csv.contains("#N/A"));
}

#[tokio::test]
async fn data_downloader_rejects_multi_symbol_tick_requests() {
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::new(),
        ticks: HashMap::new(),
    });

    let output = unique_temp_csv("tick-invalid");
    let request = DataDownloadRequest {
        symbols: vec!["SHFE.rb2605".to_string(), "SHFE.hc2605".to_string()],
        duration: Duration::ZERO,
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: output,
    };

    let err = DataDownloader::spawn_with_source(source, request).unwrap_err();
    assert!(err.to_string().contains("Tick"));
}

#[tokio::test]
async fn data_downloader_reports_completion_progress() {
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::from([(
            ("SHFE.rb2605".to_string(), 60_000_000_000),
            vec![Kline {
                id: 1,
                datetime: shanghai_nanos(2026, 4, 9, 9, 1, 0),
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                open_oi: 100,
                close_oi: 101,
                volume: 5,
                epoch: None,
            }],
        )]),
        ticks: HashMap::new(),
    });

    let output = unique_temp_csv("progress");
    let request = DataDownloadRequest {
        symbols: vec!["SHFE.rb2605".to_string()],
        duration: Duration::from_secs(60),
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: output,
    };

    let downloader = DataDownloader::spawn_with_source(source, request).unwrap();
    downloader.wait().await.unwrap();
    assert!(downloader.is_finished());
    assert_eq!(downloader.get_progress(), 100.0);
}

fn unique_temp_csv(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nonce}.csv"))
}

fn shanghai_dt(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
        .with_timezone(&Utc)
}

fn shanghai_nanos(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    shanghai_dt(year, month, day, hour, minute, second)
        .timestamp_nanos_opt()
        .unwrap()
}
