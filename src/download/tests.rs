use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokio::sync::mpsc::UnboundedSender;

use crate::types::{Kline, Tick};

use super::{
    DataDownloadAdjType, DataDownloadOptions, DataDownloadPageObserver, DataDownloadRequest, DataDownloadSource,
    DataDownloadSymbolInfo, DataDownloadWriteMode, DataDownloadWriter, DataDownloader, DividendAdjustment,
};

struct FakeDownloadSource {
    klines: HashMap<(String, i64), Vec<Kline>>,
    ticks: HashMap<String, Vec<Tick>>,
    infos: HashMap<String, DataDownloadSymbolInfo>,
    dividends: HashMap<String, Vec<DividendAdjustment>>,
    previous_closes: HashMap<(String, i64), f64>,
    first_page_probe: Option<Arc<FirstPageProbe>>,
}

#[async_trait(?Send)]
impl DataDownloadSource for FakeDownloadSource {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> crate::Result<Vec<Kline>> {
        let rows = self
            .klines
            .get(&(symbol.to_string(), duration.as_nanos() as i64))
            .cloned()
            .unwrap_or_default();
        emit_page_progress(&rows, observer, self.first_page_probe.clone());
        Ok(rows)
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> crate::Result<Vec<Tick>> {
        let rows = self.ticks.get(symbol).cloned().unwrap_or_default();
        emit_page_progress(&rows, observer, self.first_page_probe.clone());
        Ok(rows)
    }

    async fn symbol_info(&self, symbol: &str) -> crate::Result<DataDownloadSymbolInfo> {
        self.infos
            .get(symbol)
            .cloned()
            .ok_or_else(|| crate::TqError::DataNotFound(format!("symbol info not found: {symbol}")))
    }

    async fn dividend_adjustments(
        &self,
        symbol: &str,
        _start_dt: DateTime<Utc>,
        _end_dt: DateTime<Utc>,
    ) -> crate::Result<Vec<DividendAdjustment>> {
        Ok(self.dividends.get(symbol).cloned().unwrap_or_default())
    }

    async fn previous_close_before(&self, symbol: &str, before_dt: DateTime<Utc>) -> crate::Result<Option<f64>> {
        Ok(self
            .previous_closes
            .get(&(symbol.to_string(), before_dt.timestamp_nanos_opt().unwrap()))
            .copied())
    }
}

#[tokio::test]
async fn data_downloader_writes_to_stream_writer_in_append_mode() {
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
        infos: HashMap::from([(
            "SHFE.rb2605".to_string(),
            DataDownloadSymbolInfo {
                ins_class: "FUTURE".to_string(),
                price_decs: 1,
            },
        )]),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: None,
    });

    let buffer = Arc::new(std::sync::Mutex::new(b"existing\n".to_vec()));
    let writer = DataDownloadWriter::new(BufferWriter {
        shared: Arc::clone(&buffer),
    });
    let request = DataDownloadRequest {
        symbols: vec!["SHFE.rb2605".to_string()],
        duration: Duration::from_secs(60),
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: unique_temp_csv("writer-append"),
    };
    let options = DataDownloadOptions {
        write_mode: DataDownloadWriteMode::Append,
        adj_type: None,
    };

    let downloader = DataDownloader::spawn_with_source_to_writer(source, request, writer, options).unwrap();
    downloader.wait().await.unwrap();

    let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(output.starts_with("existing\n"));
    assert!(!output.contains("SHFE.rb2605.open"));
    assert!(output.contains("2026-04-09 09:01:00.000000000"));
}

#[tokio::test]
async fn data_downloader_applies_forward_adjustment_to_single_symbol_stock_kline() {
    let symbol = "SSE.600000";
    let event_dt = shanghai_dt(2026, 4, 10, 0, 0, 0);
    let factor = (1.0 - 1.0 / 10.0) / 1.1;
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::from([(
            (symbol.to_string(), 86_400_000_000_000),
            vec![
                Kline {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 15, 0, 0),
                    open: 9.0,
                    high: 10.0,
                    low: 8.0,
                    close: 10.0,
                    open_oi: 100,
                    close_oi: 101,
                    volume: 5,
                    epoch: None,
                },
                Kline {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 10, 15, 0, 0),
                    open: 10.0,
                    high: 12.0,
                    low: 9.0,
                    close: 11.0,
                    open_oi: 101,
                    close_oi: 102,
                    volume: 6,
                    epoch: None,
                },
            ],
        )]),
        ticks: HashMap::new(),
        infos: HashMap::from([(
            symbol.to_string(),
            DataDownloadSymbolInfo {
                ins_class: "STOCK".to_string(),
                price_decs: 4,
            },
        )]),
        dividends: HashMap::from([(
            symbol.to_string(),
            vec![DividendAdjustment {
                datetime: event_dt.timestamp_nanos_opt().unwrap(),
                stock_dividend: 0.1,
                cash_dividend: 1.0,
            }],
        )]),
        previous_closes: HashMap::new(),
        first_page_probe: None,
    });

    let output = unique_temp_csv("adj-forward");
    let request = DataDownloadRequest {
        symbols: vec![symbol.to_string()],
        duration: Duration::from_secs(86400),
        start_dt: shanghai_dt(2026, 4, 9, 0, 0, 0),
        end_dt: shanghai_dt(2026, 4, 11, 0, 0, 0),
        csv_file: output.clone(),
    };
    let options = DataDownloadOptions {
        write_mode: DataDownloadWriteMode::Overwrite,
        adj_type: Some(DataDownloadAdjType::Forward),
    };

    let downloader = DataDownloader::spawn_with_source_and_options(source, request, options).unwrap();
    downloader.wait().await.unwrap();

    let csv = std::fs::read_to_string(output).unwrap();
    assert!(csv.contains(&(9.0 * factor).to_string()));
    assert!(csv.contains(&(10.0 * factor).to_string()));
    assert!(csv.contains(",10,12,9,11,6,101,102"));
}

#[tokio::test]
async fn data_downloader_applies_backward_adjustment_to_ticks_without_touching_average() {
    let symbol = "SSE.600001";
    let event_dt = shanghai_dt(2026, 4, 10, 0, 0, 0);
    let factor = (1.0 - 1.0 / 10.0) / 1.1;
    let backward = 1.0 / factor;
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::new(),
        ticks: HashMap::from([(
            symbol.to_string(),
            vec![
                Tick {
                    id: 1,
                    datetime: shanghai_nanos(2026, 4, 9, 14, 59, 59),
                    last_price: 10.0,
                    average: 10.0,
                    highest: 10.0,
                    lowest: 10.0,
                    ask_price1: 10.1,
                    ask_volume1: 1,
                    bid_price1: 9.9,
                    bid_volume1: 1,
                    ..Tick::default()
                },
                Tick {
                    id: 2,
                    datetime: shanghai_nanos(2026, 4, 10, 9, 30, 0),
                    last_price: 11.0,
                    average: 11.0,
                    highest: 11.0,
                    lowest: 11.0,
                    ask_price1: 11.1,
                    ask_volume1: 2,
                    bid_price1: 10.9,
                    bid_volume1: 2,
                    ..Tick::default()
                },
            ],
        )]),
        infos: HashMap::from([(
            symbol.to_string(),
            DataDownloadSymbolInfo {
                ins_class: "FUND".to_string(),
                price_decs: 4,
            },
        )]),
        dividends: HashMap::from([(
            symbol.to_string(),
            vec![DividendAdjustment {
                datetime: event_dt.timestamp_nanos_opt().unwrap(),
                stock_dividend: 0.1,
                cash_dividend: 1.0,
            }],
        )]),
        previous_closes: HashMap::new(),
        first_page_probe: None,
    });

    let output = unique_temp_csv("adj-backward-tick");
    let request = DataDownloadRequest {
        symbols: vec![symbol.to_string()],
        duration: Duration::ZERO,
        start_dt: shanghai_dt(2026, 4, 9, 0, 0, 0),
        end_dt: shanghai_dt(2026, 4, 11, 0, 0, 0),
        csv_file: output.clone(),
    };
    let options = DataDownloadOptions {
        write_mode: DataDownloadWriteMode::Overwrite,
        adj_type: Some(DataDownloadAdjType::Backward),
    };

    let downloader = DataDownloader::spawn_with_source_and_options(source, request, options).unwrap();
    downloader.wait().await.unwrap();

    let csv = std::fs::read_to_string(output).unwrap();
    let second_row = csv.lines().nth(2).unwrap();
    let cols = second_row.split(',').collect::<Vec<_>>();
    assert_eq!(cols[2], (11.0 * backward).to_string());
    assert_eq!(cols[9], (10.9 * backward).to_string());
    assert_eq!(cols[11], (11.1 * backward).to_string());
    assert_eq!(cols[5], "11");
}

#[tokio::test]
async fn data_downloader_reports_page_level_progress_before_completion() {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::from([(
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
        )]),
        ticks: HashMap::new(),
        infos: HashMap::from([(
            "SHFE.rb2605".to_string(),
            DataDownloadSymbolInfo {
                ins_class: "FUTURE".to_string(),
                price_decs: 1,
            },
        )]),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: Some(Arc::new(FirstPageProbe {
            sender: notify_tx,
            release_rx: std::sync::Mutex::new(release_rx),
            armed: AtomicBool::new(true),
        })),
    });

    let output = unique_temp_csv("progress-pages");
    let request = DataDownloadRequest {
        symbols: vec!["SHFE.rb2605".to_string()],
        duration: Duration::from_secs(60),
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: output,
    };

    let downloader = DataDownloader::spawn_with_source(source, request).unwrap();
    let _ = notify_rx.recv().await.unwrap();
    let progress = downloader.get_progress();
    assert!(progress > 0.0);
    assert!(progress < 100.0);
    release_tx.send(()).unwrap();
    downloader.wait().await.unwrap();
    assert_eq!(downloader.get_progress(), 100.0);
}

#[tokio::test]
async fn data_downloader_rejects_multi_symbol_adjusted_kline_requests() {
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::new(),
        ticks: HashMap::new(),
        infos: HashMap::new(),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: None,
    });

    let request = DataDownloadRequest {
        symbols: vec!["SSE.600000".to_string(), "SSE.600004".to_string()],
        duration: Duration::from_secs(60),
        start_dt: shanghai_dt(2026, 4, 9, 9, 0, 0),
        end_dt: shanghai_dt(2026, 4, 9, 15, 0, 0),
        csv_file: unique_temp_csv("multi-adj"),
    };
    let options = DataDownloadOptions {
        write_mode: DataDownloadWriteMode::Overwrite,
        adj_type: Some(DataDownloadAdjType::Forward),
    };

    let err = DataDownloader::spawn_with_source_and_options(source, request, options).unwrap_err();
    assert!(err.to_string().contains("复权"));
}

#[tokio::test]
async fn data_downloader_rejects_multi_symbol_tick_requests() {
    let source = Arc::new(FakeDownloadSource {
        klines: HashMap::new(),
        ticks: HashMap::new(),
        infos: HashMap::new(),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: None,
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
        infos: HashMap::from([
            (
                "SHFE.rb2605".to_string(),
                DataDownloadSymbolInfo {
                    ins_class: "FUTURE".to_string(),
                    price_decs: 1,
                },
            ),
            (
                "SHFE.hc2605".to_string(),
                DataDownloadSymbolInfo {
                    ins_class: "FUTURE".to_string(),
                    price_decs: 1,
                },
            ),
        ]),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: None,
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
        infos: HashMap::from([(
            "SHFE.rb2605".to_string(),
            DataDownloadSymbolInfo {
                ins_class: "FUTURE".to_string(),
                price_decs: 1,
            },
        )]),
        dividends: HashMap::new(),
        previous_closes: HashMap::new(),
        first_page_probe: None,
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

fn emit_page_progress<T>(
    rows: &[T],
    observer: Option<Arc<dyn DataDownloadPageObserver>>,
    probe: Option<Arc<FirstPageProbe>>,
) where
    T: HasDatetime,
{
    let Some(observer) = observer else {
        return;
    };
    let mid = (rows.len() / 2).max(1);
    let pages = [rows.get(..mid), rows.get(mid..)]
        .into_iter()
        .flatten()
        .filter(|page| !page.is_empty());

    for page in pages {
        let latest = page.last().unwrap().datetime_nanos();
        observer.on_page(latest);
        if let Some(probe) = probe.as_ref()
            && probe.armed.swap(false, Ordering::SeqCst)
        {
            let _ = probe.sender.send(latest);
            let _ = probe.release_rx.lock().unwrap().recv();
        }
    }
}

trait HasDatetime {
    fn datetime_nanos(&self) -> i64;
}

impl HasDatetime for Kline {
    fn datetime_nanos(&self) -> i64 {
        self.datetime
    }
}

impl HasDatetime for Tick {
    fn datetime_nanos(&self) -> i64 {
        self.datetime
    }
}

struct FirstPageProbe {
    sender: UnboundedSender<i64>,
    release_rx: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    armed: AtomicBool,
}

struct BufferWriter {
    shared: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.shared.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
