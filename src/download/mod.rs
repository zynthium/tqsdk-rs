use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle as ThreadJoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use tokio::sync::Mutex;

use crate::errors::{Result, TqError};
use crate::series::SeriesAPI;
use crate::types::{Kline, Tick};

#[cfg(test)]
mod tests;

const PROGRESS_SCALE: u64 = 10_000;
const CST_OFFSET_SECONDS: i32 = 8 * 3600;

#[derive(Debug, Clone)]
pub struct DataDownloadRequest {
    pub symbols: Vec<String>,
    pub duration: Duration,
    pub start_dt: DateTime<Utc>,
    pub end_dt: DateTime<Utc>,
    pub csv_file: PathBuf,
}

#[async_trait(?Send)]
pub(crate) trait DataDownloadSource: Send + Sync {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>>;

    async fn load_ticks(&self, symbol: &str, start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Result<Vec<Tick>>;
}

#[async_trait(?Send)]
impl DataDownloadSource for SeriesAPI {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<Kline>> {
        self.kline_data_series(symbol, duration, start_dt, end_dt).await
    }

    async fn load_ticks(&self, symbol: &str, start_dt: DateTime<Utc>, end_dt: DateTime<Utc>) -> Result<Vec<Tick>> {
        self.tick_data_series(symbol, start_dt, end_dt).await
    }
}

pub struct DataDownloader {
    progress: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    task: Mutex<Option<ThreadJoinHandle<Result<()>>>>,
}

impl std::fmt::Debug for DataDownloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataDownloader")
            .field("progress", &self.get_progress())
            .field("finished", &self.is_finished())
            .finish()
    }
}

impl DataDownloader {
    pub(crate) fn spawn_with_source<S>(source: Arc<S>, request: DataDownloadRequest) -> Result<Self>
    where
        S: DataDownloadSource + 'static,
    {
        validate_request(&request)?;

        let progress = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let progress_ref = Arc::clone(&progress);
        let progress_done_ref = Arc::clone(&progress);
        let finished_ref = Arc::clone(&finished);

        let task = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| TqError::InternalError(format!("创建下载运行时失败: {err}")))?;
            let result = runtime.block_on(run_download(source, request, progress_ref));
            if result.is_ok() {
                progress_done_ref.store(PROGRESS_SCALE, Ordering::Relaxed);
            }
            finished_ref.store(true, Ordering::Relaxed);
            result
        });

        Ok(Self {
            progress,
            finished,
            task: Mutex::new(Some(task)),
        })
    }

    pub fn spawn(series: Arc<SeriesAPI>, request: DataDownloadRequest) -> Result<Self> {
        Self::spawn_with_source(series, request)
    }

    pub fn is_finished(&self) -> bool {
        if self.finished.load(Ordering::Relaxed) {
            return true;
        }
        self.task
            .try_lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(ThreadJoinHandle::is_finished))
            .unwrap_or(false)
    }

    pub fn get_progress(&self) -> f64 {
        self.progress.load(Ordering::Relaxed) as f64 / 100.0
    }

    pub async fn wait(&self) -> Result<()> {
        let Some(task) = self.task.lock().await.take() else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || {
            task.join()
                .map_err(|_| TqError::InternalError("下载任务线程异常结束".to_string()))?
        })
        .await
        .map_err(|err| TqError::InternalError(format!("等待下载任务失败: {err}")))?
    }
}

async fn run_download<S>(source: Arc<S>, request: DataDownloadRequest, progress: Arc<AtomicU64>) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    if request.duration.is_zero() {
        download_ticks_to_csv(source, request, progress).await
    } else {
        download_klines_to_csv(source, request, progress).await
    }
}

async fn download_klines_to_csv<S>(source: Arc<S>, request: DataDownloadRequest, progress: Arc<AtomicU64>) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    let total_symbols = request.symbols.len() as u64;
    let mut symbol_rows = Vec::with_capacity(request.symbols.len());
    for (index, symbol) in request.symbols.iter().enumerate() {
        let rows = source
            .load_klines(symbol, request.duration, request.start_dt, request.end_dt)
            .await?;
        symbol_rows.push(rows);
        let fetched = ((index as u64 + 1) * (PROGRESS_SCALE / 2)) / total_symbols.max(1);
        progress.store(fetched, Ordering::Relaxed);
    }

    let Some(primary_rows) = symbol_rows.first() else {
        return Err(TqError::InvalidParameter("symbols 不能为空".to_string()));
    };

    let mut secondary_maps = Vec::with_capacity(symbol_rows.len().saturating_sub(1));
    for rows in symbol_rows.iter().skip(1) {
        let map = rows
            .iter()
            .cloned()
            .map(|row| (row.datetime, row))
            .collect::<HashMap<_, _>>();
        secondary_maps.push(map);
    }

    let mut writer = csv_writer(&request.csv_file)?;
    writeln!(writer, "{}", kline_headers(&request.symbols))?;

    let total_rows = primary_rows.len() as u64;
    for (index, row) in primary_rows.iter().enumerate() {
        let mut line = String::new();
        write!(&mut line, "{},{}", format_nano_datetime(row.datetime), row.datetime).unwrap();
        append_kline_values(&mut line, Some(row));

        for secondary in &secondary_maps {
            let other = secondary.get(&row.datetime);
            append_kline_values(&mut line, other);
        }

        writeln!(writer, "{line}")?;
        let written = (index as u64 + 1) * (PROGRESS_SCALE / 2) / total_rows.max(1);
        progress.store(PROGRESS_SCALE / 2 + written, Ordering::Relaxed);
    }

    writer.flush()?;
    Ok(())
}

async fn download_ticks_to_csv<S>(source: Arc<S>, request: DataDownloadRequest, progress: Arc<AtomicU64>) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    let symbol = request
        .symbols
        .first()
        .ok_or_else(|| TqError::InvalidParameter("symbols 不能为空".to_string()))?;
    let rows = source.load_ticks(symbol, request.start_dt, request.end_dt).await?;
    progress.store(PROGRESS_SCALE / 2, Ordering::Relaxed);

    let mut writer = csv_writer(&request.csv_file)?;
    writeln!(writer, "{}", tick_headers(symbol))?;

    let total_rows = rows.len() as u64;
    for (index, row) in rows.iter().enumerate() {
        writeln!(writer, "{}", tick_line(symbol, row))?;
        let written = (index as u64 + 1) * (PROGRESS_SCALE / 2) / total_rows.max(1);
        progress.store(PROGRESS_SCALE / 2 + written, Ordering::Relaxed);
    }

    writer.flush()?;
    Ok(())
}

fn validate_request(request: &DataDownloadRequest) -> Result<()> {
    if request.symbols.is_empty() {
        return Err(TqError::InvalidParameter("symbols 不能为空".to_string()));
    }
    if request.symbols.iter().any(|symbol| symbol.trim().is_empty()) {
        return Err(TqError::InvalidParameter("symbols 不能包含空字符串".to_string()));
    }
    if request.end_dt <= request.start_dt {
        return Err(TqError::InvalidParameter("end_dt 必须晚于 start_dt".to_string()));
    }
    if request.duration.is_zero() && request.symbols.len() != 1 {
        return Err(TqError::InvalidParameter("Tick 序列不支持多合约下载".to_string()));
    }
    Ok(())
}

fn csv_writer(path: &PathBuf) -> Result<BufWriter<std::fs::File>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(BufWriter::new(std::fs::File::create(path)?))
}

fn kline_headers(symbols: &[String]) -> String {
    let mut columns = vec!["datetime".to_string(), "datetime_nano".to_string()];
    for symbol in symbols {
        for suffix in ["open", "high", "low", "close", "volume", "open_oi", "close_oi"] {
            columns.push(format!("{symbol}.{suffix}"));
        }
    }
    columns.join(",")
}

fn tick_headers(symbol: &str) -> String {
    let mut columns = vec!["datetime".to_string(), "datetime_nano".to_string()];
    for suffix in [
        "last_price",
        "highest",
        "lowest",
        "average",
        "volume",
        "amount",
        "open_interest",
        "bid_price1",
        "bid_volume1",
        "ask_price1",
        "ask_volume1",
        "bid_price2",
        "bid_volume2",
        "ask_price2",
        "ask_volume2",
        "bid_price3",
        "bid_volume3",
        "ask_price3",
        "ask_volume3",
        "bid_price4",
        "bid_volume4",
        "ask_price4",
        "ask_volume4",
        "bid_price5",
        "bid_volume5",
        "ask_price5",
        "ask_volume5",
    ] {
        columns.push(format!("{symbol}.{suffix}"));
    }
    columns.join(",")
}

fn append_kline_values(line: &mut String, row: Option<&Kline>) {
    match row {
        Some(row) => {
            for value in [
                format_f64(row.open),
                format_f64(row.high),
                format_f64(row.low),
                format_f64(row.close),
                row.volume.to_string(),
                row.open_oi.to_string(),
                row.close_oi.to_string(),
            ] {
                line.push(',');
                line.push_str(&value);
            }
        }
        None => {
            for _ in 0..7 {
                line.push_str(",#N/A");
            }
        }
    }
}

fn tick_line(symbol: &str, row: &Tick) -> String {
    let mut line = String::new();
    write!(&mut line, "{},{}", format_nano_datetime(row.datetime), row.datetime).unwrap();

    let _ = symbol;
    for value in [
        format_f64(row.last_price),
        format_f64(row.highest),
        format_f64(row.lowest),
        format_f64(row.average),
        row.volume.to_string(),
        format_f64(row.amount),
        row.open_interest.to_string(),
        format_f64(row.bid_price1),
        row.bid_volume1.to_string(),
        format_f64(row.ask_price1),
        row.ask_volume1.to_string(),
        format_f64(row.bid_price2),
        row.bid_volume2.to_string(),
        format_f64(row.ask_price2),
        row.ask_volume2.to_string(),
        format_f64(row.bid_price3),
        row.bid_volume3.to_string(),
        format_f64(row.ask_price3),
        row.ask_volume3.to_string(),
        format_f64(row.bid_price4),
        row.bid_volume4.to_string(),
        format_f64(row.ask_price4),
        row.ask_volume4.to_string(),
        format_f64(row.bid_price5),
        row.bid_volume5.to_string(),
        format_f64(row.ask_price5),
        row.ask_volume5.to_string(),
    ] {
        line.push(',');
        line.push_str(&value);
    }
    line
}

fn format_f64(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "#N/A".to_string()
    }
}

fn format_nano_datetime(datetime_nanos: i64) -> String {
    DateTime::<Utc>::from_timestamp_nanos(datetime_nanos)
        .with_timezone(&FixedOffset::east_opt(CST_OFFSET_SECONDS).expect("CST offset must be valid"))
        .format("%Y-%m-%d %H:%M:%S%.9f")
        .to_string()
}
