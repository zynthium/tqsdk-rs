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
const FETCH_PROGRESS_SCALE: u64 = 9_000;
const WRITE_PROGRESS_SCALE: u64 = PROGRESS_SCALE - FETCH_PROGRESS_SCALE;
const CST_OFFSET_SECONDS: i32 = 8 * 3600;

#[derive(Debug, Clone)]
pub struct DataDownloadRequest {
    pub symbols: Vec<String>,
    pub duration: Duration,
    pub start_dt: DateTime<Utc>,
    pub end_dt: DateTime<Utc>,
    pub csv_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataDownloadWriteMode {
    #[default]
    Overwrite,
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDownloadAdjType {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataDownloadOptions {
    pub write_mode: DataDownloadWriteMode,
    pub adj_type: Option<DataDownloadAdjType>,
}

#[derive(Clone)]
pub struct DataDownloadWriter {
    inner: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
}

impl DataDownloadWriter {
    pub fn new<W>(writer: W) -> Self
    where
        W: Write + Send + 'static,
    {
        Self {
            inner: Arc::new(std::sync::Mutex::new(Box::new(writer))),
        }
    }

    fn write_all(&self, bytes: &[u8]) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| TqError::InternalError("下载 writer 锁已中毒".to_string()))?;
        guard.write_all(bytes)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| TqError::InternalError("下载 writer 锁已中毒".to_string()))?;
        guard.flush()?;
        Ok(())
    }
}

impl std::fmt::Debug for DataDownloadWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataDownloadWriter").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DataDownloadSymbolInfo {
    pub ins_class: String,
    pub price_decs: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct DividendAdjustment {
    pub datetime: i64,
    pub stock_dividend: f64,
    pub cash_dividend: f64,
}

pub(crate) trait DataDownloadPageObserver: Send + Sync {
    fn on_page(&self, latest_datetime_nanos: i64);
}

#[async_trait(?Send)]
pub(crate) trait DataDownloadSource: Send + Sync {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> Result<Vec<Kline>>;

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> Result<Vec<Tick>>;

    async fn symbol_info(&self, symbol: &str) -> Result<DataDownloadSymbolInfo>;

    async fn dividend_adjustments(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<DividendAdjustment>>;

    async fn previous_close_before(&self, symbol: &str, before_dt: DateTime<Utc>) -> Result<Option<f64>>;
}

#[async_trait(?Send)]
impl DataDownloadSource for SeriesAPI {
    async fn load_klines(
        &self,
        symbol: &str,
        duration: Duration,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> Result<Vec<Kline>> {
        self.kline_data_series_with_progress(symbol, duration, start_dt, end_dt, observer)
            .await
    }

    async fn load_ticks(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
        observer: Option<Arc<dyn DataDownloadPageObserver>>,
    ) -> Result<Vec<Tick>> {
        self.tick_data_series_with_progress(symbol, start_dt, end_dt, observer)
            .await
    }

    async fn symbol_info(&self, symbol: &str) -> Result<DataDownloadSymbolInfo> {
        self.download_symbol_info(symbol).await
    }

    async fn dividend_adjustments(
        &self,
        symbol: &str,
        start_dt: DateTime<Utc>,
        end_dt: DateTime<Utc>,
    ) -> Result<Vec<DividendAdjustment>> {
        self.download_dividend_adjustments(symbol, start_dt, end_dt).await
    }

    async fn previous_close_before(&self, symbol: &str, before_dt: DateTime<Utc>) -> Result<Option<f64>> {
        self.download_previous_close_before(symbol, before_dt).await
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
        Self::spawn_with_source_and_options(source, request, DataDownloadOptions::default())
    }

    pub(crate) fn spawn_with_source_and_options<S>(
        source: Arc<S>,
        request: DataDownloadRequest,
        options: DataDownloadOptions,
    ) -> Result<Self>
    where
        S: DataDownloadSource + 'static,
    {
        Self::spawn_with_source_target(source, request, DownloadTarget::File, options)
    }

    pub(crate) fn spawn_with_source_to_writer<S>(
        source: Arc<S>,
        request: DataDownloadRequest,
        writer: DataDownloadWriter,
        options: DataDownloadOptions,
    ) -> Result<Self>
    where
        S: DataDownloadSource + 'static,
    {
        Self::spawn_with_source_target(source, request, DownloadTarget::Writer(writer), options)
    }

    fn spawn_with_source_target<S>(
        source: Arc<S>,
        request: DataDownloadRequest,
        target: DownloadTarget,
        options: DataDownloadOptions,
    ) -> Result<Self>
    where
        S: DataDownloadSource + 'static,
    {
        validate_request(&request, options)?;

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
            let result = runtime.block_on(run_download(source, request, target, options, progress_ref));
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

    pub(crate) fn spawn(series: Arc<SeriesAPI>, request: DataDownloadRequest) -> Result<Self> {
        Self::spawn_with_source(series, request)
    }

    pub(crate) fn spawn_with_options(
        series: Arc<SeriesAPI>,
        request: DataDownloadRequest,
        options: DataDownloadOptions,
    ) -> Result<Self> {
        Self::spawn_with_source_and_options(series, request, options)
    }

    pub(crate) fn spawn_to_writer(
        series: Arc<SeriesAPI>,
        request: DataDownloadRequest,
        writer: DataDownloadWriter,
        options: DataDownloadOptions,
    ) -> Result<Self> {
        Self::spawn_with_source_to_writer(series, request, writer, options)
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

enum DownloadTarget {
    File,
    Writer(DataDownloadWriter),
}

struct CsvOutput {
    inner: CsvOutputInner,
}

enum CsvOutputInner {
    File(BufWriter<std::fs::File>),
    Writer(DataDownloadWriter),
}

impl CsvOutput {
    fn open(target: DownloadTarget, path: &PathBuf, mode: DataDownloadWriteMode) -> Result<Self> {
        let inner = match target {
            DownloadTarget::File => CsvOutputInner::File(open_file_writer(path, mode)?),
            DownloadTarget::Writer(writer) => CsvOutputInner::Writer(writer),
        };
        Ok(Self { inner })
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        match &mut self.inner {
            CsvOutputInner::File(file) => {
                file.write_all(line.as_bytes())?;
                file.write_all(b"\n")?;
            }
            CsvOutputInner::Writer(writer) => {
                writer.write_all(line.as_bytes())?;
                writer.write_all(b"\n")?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        match &mut self.inner {
            CsvOutputInner::File(file) => file.flush()?,
            CsvOutputInner::Writer(writer) => writer.flush()?,
        }
        Ok(())
    }
}

struct ProgressTracker {
    start_nanos: i64,
    end_nanos: i64,
    overall: Arc<AtomicU64>,
    symbol_progress: std::sync::Mutex<Vec<u64>>,
}

impl ProgressTracker {
    fn new(start_dt: DateTime<Utc>, end_dt: DateTime<Utc>, symbol_count: usize, overall: Arc<AtomicU64>) -> Arc<Self> {
        Arc::new(Self {
            start_nanos: start_dt.timestamp_nanos_opt().unwrap_or_default(),
            end_nanos: end_dt.timestamp_nanos_opt().unwrap_or_default(),
            overall,
            symbol_progress: std::sync::Mutex::new(vec![0; symbol_count.max(1)]),
        })
    }

    fn observer(self: &Arc<Self>, symbol_index: usize) -> Arc<dyn DataDownloadPageObserver> {
        Arc::new(SymbolProgressObserver {
            tracker: Arc::clone(self),
            symbol_index,
        })
    }

    fn mark_symbol_complete(&self, symbol_index: usize) {
        self.update_symbol_progress(symbol_index, self.end_nanos);
    }

    fn update_symbol_progress(&self, symbol_index: usize, latest_datetime_nanos: i64) {
        let scaled = self.scale_time_progress(latest_datetime_nanos);
        let mut guard = match self.symbol_progress.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if let Some(entry) = guard.get_mut(symbol_index) {
            *entry = (*entry).max(scaled);
        }
        let total: u64 = guard.iter().copied().sum();
        let average = total / guard.len() as u64;
        self.overall
            .store(average * FETCH_PROGRESS_SCALE / PROGRESS_SCALE, Ordering::Relaxed);
    }

    fn mark_write_progress(&self, written_rows: usize, total_rows: usize) {
        if total_rows == 0 {
            self.overall.store(PROGRESS_SCALE, Ordering::Relaxed);
            return;
        }
        let scaled = written_rows as u64 * WRITE_PROGRESS_SCALE / total_rows as u64;
        self.overall.store(FETCH_PROGRESS_SCALE + scaled, Ordering::Relaxed);
    }

    fn scale_time_progress(&self, latest_datetime_nanos: i64) -> u64 {
        if self.end_nanos <= self.start_nanos {
            return PROGRESS_SCALE;
        }
        let clamped = latest_datetime_nanos.clamp(self.start_nanos, self.end_nanos);
        let span = (self.end_nanos - self.start_nanos) as u128;
        let progressed = (clamped - self.start_nanos) as u128;
        ((progressed * PROGRESS_SCALE as u128) / span) as u64
    }
}

struct SymbolProgressObserver {
    tracker: Arc<ProgressTracker>,
    symbol_index: usize,
}

impl DataDownloadPageObserver for SymbolProgressObserver {
    fn on_page(&self, latest_datetime_nanos: i64) {
        self.tracker
            .update_symbol_progress(self.symbol_index, latest_datetime_nanos);
    }
}

async fn run_download<S>(
    source: Arc<S>,
    request: DataDownloadRequest,
    target: DownloadTarget,
    options: DataDownloadOptions,
    progress: Arc<AtomicU64>,
) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    if request.duration.is_zero() {
        download_ticks_to_csv(source, request, target, options, progress).await
    } else {
        download_klines_to_csv(source, request, target, options, progress).await
    }
}

async fn download_klines_to_csv<S>(
    source: Arc<S>,
    request: DataDownloadRequest,
    target: DownloadTarget,
    options: DataDownloadOptions,
    progress: Arc<AtomicU64>,
) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    let tracker = ProgressTracker::new(
        request.start_dt,
        request.end_dt,
        request.symbols.len(),
        Arc::clone(&progress),
    );
    let mut symbol_rows = Vec::with_capacity(request.symbols.len());

    for (index, symbol) in request.symbols.iter().enumerate() {
        let observer = tracker.observer(index);
        let mut rows = source
            .load_klines(
                symbol,
                request.duration,
                request.start_dt,
                request.end_dt,
                Some(observer),
            )
            .await?;
        tracker.mark_symbol_complete(index);
        if index == 0 {
            maybe_adjust_rows(
                source.as_ref(),
                symbol,
                &mut rows,
                request.start_dt,
                request.end_dt,
                options.adj_type,
            )
            .await?;
        }
        symbol_rows.push(rows);
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

    let mut writer = CsvOutput::open(target, &request.csv_file, options.write_mode)?;
    if matches!(options.write_mode, DataDownloadWriteMode::Overwrite) {
        writer.write_line(&kline_headers(&request.symbols))?;
    }

    let total_rows = primary_rows.len();
    for (index, row) in primary_rows.iter().enumerate() {
        let mut line = String::new();
        write!(&mut line, "{},{}", format_nano_datetime(row.datetime), row.datetime).unwrap();
        append_kline_values(&mut line, Some(row));

        for secondary in &secondary_maps {
            let other = secondary.get(&row.datetime);
            append_kline_values(&mut line, other);
        }

        writer.write_line(&line)?;
        tracker.mark_write_progress(index + 1, total_rows);
    }

    writer.flush()?;
    Ok(())
}

async fn download_ticks_to_csv<S>(
    source: Arc<S>,
    request: DataDownloadRequest,
    target: DownloadTarget,
    options: DataDownloadOptions,
    progress: Arc<AtomicU64>,
) -> Result<()>
where
    S: DataDownloadSource + 'static,
{
    let symbol = request
        .symbols
        .first()
        .ok_or_else(|| TqError::InvalidParameter("symbols 不能为空".to_string()))?;
    let tracker = ProgressTracker::new(request.start_dt, request.end_dt, 1, Arc::clone(&progress));
    let observer = tracker.observer(0);
    let mut rows = source
        .load_ticks(symbol, request.start_dt, request.end_dt, Some(observer))
        .await?;
    tracker.mark_symbol_complete(0);

    maybe_adjust_rows(
        source.as_ref(),
        symbol,
        &mut rows,
        request.start_dt,
        request.end_dt,
        options.adj_type,
    )
    .await?;

    let mut writer = CsvOutput::open(target, &request.csv_file, options.write_mode)?;
    if matches!(options.write_mode, DataDownloadWriteMode::Overwrite) {
        writer.write_line(&tick_headers(symbol))?;
    }

    let total_rows = rows.len();
    for (index, row) in rows.iter().enumerate() {
        writer.write_line(&tick_line(row))?;
        tracker.mark_write_progress(index + 1, total_rows);
    }

    writer.flush()?;
    Ok(())
}

fn validate_request(request: &DataDownloadRequest, options: DataDownloadOptions) -> Result<()> {
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
    if options.adj_type.is_some() && request.symbols.len() != 1 {
        return Err(TqError::InvalidParameter("复权下载暂不支持多合约对齐 K 线".to_string()));
    }
    Ok(())
}

fn open_file_writer(path: &PathBuf, mode: DataDownloadWriteMode) -> Result<BufWriter<std::fs::File>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(matches!(mode, DataDownloadWriteMode::Overwrite))
        .append(matches!(mode, DataDownloadWriteMode::Append))
        .open(path)?;
    Ok(BufWriter::new(file))
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

fn tick_line(row: &Tick) -> String {
    let mut line = String::new();
    write!(&mut line, "{},{}", format_nano_datetime(row.datetime), row.datetime).unwrap();

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

async fn maybe_adjust_rows<S, T>(
    source: &S,
    symbol: &str,
    rows: &mut [T],
    start_dt: DateTime<Utc>,
    end_dt: DateTime<Utc>,
    adj_type: Option<DataDownloadAdjType>,
) -> Result<()>
where
    S: DataDownloadSource + ?Sized,
    T: AdjustableRow,
{
    let Some(adj_type) = adj_type else {
        return Ok(());
    };
    if rows.is_empty() {
        return Ok(());
    }

    let info = source.symbol_info(symbol).await?;
    let _ = info.price_decs;
    if !matches!(info.ins_class.as_str(), "STOCK" | "FUND") {
        return Ok(());
    }

    let factors = build_dividend_factors(source, symbol, rows, start_dt, end_dt).await?;
    if factors.is_empty() {
        return Ok(());
    }

    match adj_type {
        DataDownloadAdjType::Forward => apply_forward_adjustment(rows, &factors),
        DataDownloadAdjType::Backward => apply_backward_adjustment(rows, &factors),
    }
    Ok(())
}

async fn build_dividend_factors<S, T>(
    source: &S,
    symbol: &str,
    rows: &[T],
    start_dt: DateTime<Utc>,
    end_dt: DateTime<Utc>,
) -> Result<Vec<ComputedDividendFactor>>
where
    S: DataDownloadSource + ?Sized,
    T: AdjustableRow,
{
    let mut events = source.dividend_adjustments(symbol, start_dt, end_dt).await?;
    events.sort_by_key(|event| event.datetime);

    let mut factors = Vec::new();
    for event in events {
        let row_index = rows.partition_point(|row| row.datetime_nanos() < event.datetime);
        if row_index >= rows.len() {
            continue;
        }

        let prev_close = if row_index > 0 {
            Some(rows[row_index - 1].reference_price())
        } else {
            source
                .previous_close_before(symbol, DateTime::<Utc>::from_timestamp_nanos(event.datetime))
                .await?
        };
        let Some(prev_close) = prev_close else {
            continue;
        };
        if !prev_close.is_finite() || prev_close <= 0.0 {
            continue;
        }

        let factor = (1.0 - event.cash_dividend / prev_close) / (1.0 + event.stock_dividend);
        if factor.is_finite() && factor > 0.0 && (factor - 1.0).abs() > f64::EPSILON {
            factors.push(ComputedDividendFactor {
                datetime: event.datetime,
                factor,
            });
        }
    }
    Ok(factors)
}

fn apply_forward_adjustment<T>(rows: &mut [T], factors: &[ComputedDividendFactor])
where
    T: AdjustableRow,
{
    let mut current_factor = factors.iter().fold(1.0, |acc, factor| acc * factor.factor);
    let mut next_factor = 0;

    for row in rows {
        while next_factor < factors.len() && row.datetime_nanos() >= factors[next_factor].datetime {
            current_factor /= factors[next_factor].factor;
            next_factor += 1;
        }
        if (current_factor - 1.0).abs() > f64::EPSILON {
            row.apply_factor(current_factor);
        }
    }
}

fn apply_backward_adjustment<T>(rows: &mut [T], factors: &[ComputedDividendFactor])
where
    T: AdjustableRow,
{
    let mut current_factor = 1.0;
    let mut next_factor = 0;

    for row in rows {
        while next_factor < factors.len() && row.datetime_nanos() >= factors[next_factor].datetime {
            current_factor *= 1.0 / factors[next_factor].factor;
            next_factor += 1;
        }
        if (current_factor - 1.0).abs() > f64::EPSILON {
            row.apply_factor(current_factor);
        }
    }
}

trait AdjustableRow {
    fn datetime_nanos(&self) -> i64;
    fn reference_price(&self) -> f64;
    fn apply_factor(&mut self, factor: f64);
}

impl AdjustableRow for Kline {
    fn datetime_nanos(&self) -> i64 {
        self.datetime
    }

    fn reference_price(&self) -> f64 {
        self.close
    }

    fn apply_factor(&mut self, factor: f64) {
        self.open *= factor;
        self.high *= factor;
        self.low *= factor;
        self.close *= factor;
    }
}

impl AdjustableRow for Tick {
    fn datetime_nanos(&self) -> i64 {
        self.datetime
    }

    fn reference_price(&self) -> f64 {
        self.last_price
    }

    fn apply_factor(&mut self, factor: f64) {
        self.last_price *= factor;
        self.highest *= factor;
        self.lowest *= factor;
        self.bid_price1 *= factor;
        self.bid_price2 *= factor;
        self.bid_price3 *= factor;
        self.bid_price4 *= factor;
        self.bid_price5 *= factor;
        self.ask_price1 *= factor;
        self.ask_price2 *= factor;
        self.ask_price3 *= factor;
        self.ask_price4 *= factor;
        self.ask_price5 *= factor;
    }
}

struct ComputedDividendFactor {
    datetime: i64,
    factor: f64,
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
