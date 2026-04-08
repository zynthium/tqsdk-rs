use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, RangeSet, Tick, rangeset_intersection};
use fs2::FileExt;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PAGE_VIEW_WIDTH: usize = 2000;
pub const KLINE_DATA_COLS: &[&str] = &["open", "high", "low", "close", "volume", "open_oi", "close_oi"];
pub const TICK_5_LEVEL_DATA_COLS: &[&str] = &[
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
];
pub const TICK_1_LEVEL_DATA_COLS: &[&str] = &[
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
];

const DEFAULT_CACHE_DIR: &str = ".tqsdk/data_series_1";

pub struct DataSeriesCache {
    base_path: PathBuf,
    in_process_lock: Mutex<()>,
}

pub struct DataSeriesCacheLock<'a> {
    _in_process_lock: std::sync::MutexGuard<'a, ()>,
    lock_file: File,
}

#[derive(Debug, Clone)]
struct CacheFileMeta {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
}

#[derive(Debug, Clone, Copy)]
enum SeriesLayout {
    Kline { dur_nano: i64 },
    Tick { five_level: bool },
}

impl Drop for DataSeriesCacheLock<'_> {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

impl DataSeriesCache {
    pub fn new(base_path: Option<PathBuf>) -> Self {
        let base_path = base_path.unwrap_or_else(|| {
            dirs::home_dir()
                .map(|home| home.join(DEFAULT_CACHE_DIR))
                .unwrap_or_else(|| std::env::temp_dir().join("tqsdk_data_series_1"))
        });
        let _ = fs::create_dir_all(&base_path);
        Self {
            base_path,
            in_process_lock: Mutex::new(()),
        }
    }

    pub fn lock_series(&self, symbol: &str, dur_nano: i64) -> Result<DataSeriesCacheLock<'_>> {
        let in_process_lock = self
            .in_process_lock
            .lock()
            .map_err(|e| TqError::Other(format!("缓存写锁已中毒: {e}")))?;
        fs::create_dir_all(&self.base_path)?;
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.get_lock_path(symbol, dur_nano))?;
        lock_file
            .lock_exclusive()
            .map_err(|e| TqError::Other(format!("获取缓存文件锁失败: {e}")))?;
        Ok(DataSeriesCacheLock {
            _in_process_lock: in_process_lock,
            lock_file,
        })
    }

    pub fn get_rangeset_id(&self, symbol: &str, dur_nano: i64) -> Result<RangeSet> {
        let mut rangeset = Vec::new();
        if !self.base_path.exists() {
            return Ok(rangeset);
        }
        for entry in fs::read_dir(&self.base_path)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let filename = entry.file_name();
            let filename = filename.to_string_lossy();
            let Some((file_symbol, file_dur, range)) = parse_data_file_name(&filename) else {
                continue;
            };
            if file_symbol == symbol && file_dur == dur_nano {
                rangeset.push(range);
            }
        }
        rangeset.sort_by_key(|range| (range.start, range.end));
        Ok(rangeset)
    }

    pub fn get_rangeset_dt(&self, symbol: &str, dur_nano: i64, rangeset_id: &[Range]) -> Result<RangeSet> {
        let layout = if dur_nano == 0 {
            SeriesLayout::Tick {
                five_level: tick_uses_five_levels(symbol),
            }
        } else {
            SeriesLayout::Kline { dur_nano }
        };
        let mut ranges = Vec::new();
        for range in rangeset_id {
            let path = self.get_data_file_path(symbol, dur_nano, range);
            let mut file = File::open(&path)?;
            let row_count = row_count(&file, layout)?;
            if row_count == 0 {
                continue;
            }
            let first_dt = read_datetime_at(&mut file, layout, 0)?;
            let last_dt = read_datetime_at(&mut file, layout, row_count - 1)?;
            let end_dt = match layout {
                SeriesLayout::Kline { dur_nano } => last_dt
                    .checked_add(dur_nano)
                    .ok_or_else(|| TqError::ParseError("K 线时间范围溢出".to_string()))?,
                SeriesLayout::Tick { .. } => last_dt
                    .checked_add(100)
                    .ok_or_else(|| TqError::ParseError("Tick 时间范围溢出".to_string()))?,
            };
            ranges.push(Range::new(first_dt, end_dt));
        }
        Ok(ranges)
    }

    pub fn write_kline_segment(&self, symbol: &str, dur_nano: i64, rows: &[Kline]) -> Result<Option<Range>> {
        if rows.is_empty() {
            return Ok(None);
        }
        fs::create_dir_all(&self.base_path)?;
        let temp_path = self.get_temp_path(symbol, dur_nano);
        let mut file = File::create(&temp_path)?;
        {
            let mut writer = BufWriter::new(&mut file);
            for row in rows {
                write_i64(&mut writer, row.id)?;
                write_i64(&mut writer, row.datetime)?;
                write_f64(&mut writer, row.open)?;
                write_f64(&mut writer, row.high)?;
                write_f64(&mut writer, row.low)?;
                write_f64(&mut writer, row.close)?;
                write_f64(&mut writer, row.volume as f64)?;
                write_f64(&mut writer, row.open_oi as f64)?;
                write_f64(&mut writer, row.close_oi as f64)?;
            }
            writer.flush()?;
        }
        file.sync_all()?;
        let range = range_from_ids(rows.iter().map(|row| row.id))?
            .ok_or_else(|| TqError::ParseError("K 线缓存段为空".to_string()))?;
        let target_path = self.get_data_file_path(symbol, dur_nano, &range);
        fs::rename(temp_path, target_path)?;
        Ok(Some(range))
    }

    pub fn write_tick_segment(&self, symbol: &str, rows: &[Tick]) -> Result<Option<Range>> {
        if rows.is_empty() {
            return Ok(None);
        }
        fs::create_dir_all(&self.base_path)?;
        let temp_path = self.get_temp_path(symbol, 0);
        let mut file = File::create(&temp_path)?;
        let five_level = tick_uses_five_levels(symbol);
        {
            let mut writer = BufWriter::new(&mut file);
            for row in rows {
                write_i64(&mut writer, row.id)?;
                write_i64(&mut writer, row.datetime)?;
                write_f64(&mut writer, row.last_price)?;
                write_f64(&mut writer, row.highest)?;
                write_f64(&mut writer, row.lowest)?;
                write_f64(&mut writer, row.average)?;
                write_f64(&mut writer, row.volume as f64)?;
                write_f64(&mut writer, row.amount)?;
                write_f64(&mut writer, row.open_interest as f64)?;
                write_f64(&mut writer, row.bid_price1)?;
                write_f64(&mut writer, row.bid_volume1 as f64)?;
                write_f64(&mut writer, row.ask_price1)?;
                write_f64(&mut writer, row.ask_volume1 as f64)?;
                if five_level {
                    write_f64(&mut writer, row.bid_price2)?;
                    write_f64(&mut writer, row.bid_volume2 as f64)?;
                    write_f64(&mut writer, row.ask_price2)?;
                    write_f64(&mut writer, row.ask_volume2 as f64)?;
                    write_f64(&mut writer, row.bid_price3)?;
                    write_f64(&mut writer, row.bid_volume3 as f64)?;
                    write_f64(&mut writer, row.ask_price3)?;
                    write_f64(&mut writer, row.ask_volume3 as f64)?;
                    write_f64(&mut writer, row.bid_price4)?;
                    write_f64(&mut writer, row.bid_volume4 as f64)?;
                    write_f64(&mut writer, row.ask_price4)?;
                    write_f64(&mut writer, row.ask_volume4 as f64)?;
                    write_f64(&mut writer, row.bid_price5)?;
                    write_f64(&mut writer, row.bid_volume5 as f64)?;
                    write_f64(&mut writer, row.ask_price5)?;
                    write_f64(&mut writer, row.ask_volume5 as f64)?;
                }
            }
            writer.flush()?;
        }
        file.sync_all()?;
        let range = range_from_ids(rows.iter().map(|row| row.id))?
            .ok_or_else(|| TqError::ParseError("Tick 缓存段为空".to_string()))?;
        let target_path = self.get_data_file_path(symbol, 0, &range);
        fs::rename(temp_path, target_path)?;
        Ok(Some(range))
    }

    pub fn merge_adjacent_files(&self, symbol: &str, dur_nano: i64) -> Result<()> {
        let rangeset = self.get_rangeset_id(symbol, dur_nano)?;
        if rangeset.len() <= 1 {
            return Ok(());
        }
        let layout = if dur_nano == 0 {
            SeriesLayout::Tick {
                five_level: tick_uses_five_levels(symbol),
            }
        } else {
            SeriesLayout::Kline { dur_nano }
        };
        let groups = build_merge_groups(&rangeset)?;
        for group in groups {
            if group.len() <= 1 {
                continue;
            }
            let first = &group[0];
            let last = group.last().expect("group should not be empty");
            let temp_path = self
                .base_path
                .join(format!("{symbol}.{dur_nano}.merge.{}", monotonic_suffix()));
            let mut temp_file = File::create(&temp_path)?;
            {
                let mut writer = BufWriter::new(&mut temp_file);
                for (range, rows_to_copy) in &group {
                    let path = self.get_data_file_path(symbol, dur_nano, range);
                    copy_rows(&path, layout, *rows_to_copy, &mut writer)?;
                }
                writer.flush()?;
            }
            temp_file.sync_all()?;
            for (range, _) in &group {
                let _ = fs::remove_file(self.get_data_file_path(symbol, dur_nano, range));
            }
            let final_path = self.get_data_file_path(symbol, dur_nano, &Range::new(first.0.start, last.0.end));
            fs::rename(temp_path, final_path)?;
        }
        Ok(())
    }

    pub fn read_kline_window(&self, symbol: &str, dur_nano: i64, start_dt: i64, end_dt: i64) -> Result<Vec<Kline>> {
        if end_dt <= start_dt {
            return Ok(Vec::new());
        }
        let id_ranges = self.get_rangeset_id(symbol, dur_nano)?;
        let dt_ranges = self.get_rangeset_dt(symbol, dur_nano, &id_ranges)?;
        self.read_window(
            symbol,
            SeriesLayout::Kline { dur_nano },
            start_dt,
            end_dt,
            &id_ranges,
            &dt_ranges,
        )
        .map(WindowRows::into_klines)
    }

    pub fn read_tick_window(&self, symbol: &str, start_dt: i64, end_dt: i64) -> Result<Vec<Tick>> {
        if end_dt <= start_dt {
            return Ok(Vec::new());
        }
        let id_ranges = self.get_rangeset_id(symbol, 0)?;
        let dt_ranges = self.get_rangeset_dt(symbol, 0, &id_ranges)?;
        self.read_window(
            symbol,
            SeriesLayout::Tick {
                five_level: tick_uses_five_levels(symbol),
            },
            start_dt,
            end_dt,
            &id_ranges,
            &dt_ranges,
        )
        .map(WindowRows::into_ticks)
    }

    pub fn enforce_limits(&self, max_bytes: Option<u64>, retention_days: Option<u64>) -> Result<()> {
        let _cleanup_lock = self
            .in_process_lock
            .lock()
            .map_err(|e| TqError::Other(format!("缓存写锁已中毒: {e}")))?;
        self.evict_expired_files(retention_days)?;
        self.evict_by_total_size(max_bytes)?;
        Ok(())
    }

    fn read_window(
        &self,
        symbol: &str,
        layout: SeriesLayout,
        start_dt: i64,
        end_dt: i64,
        id_ranges: &[Range],
        dt_ranges: &[Range],
    ) -> Result<WindowRows> {
        let requested = vec![Range::new(start_dt, end_dt)];
        let mut rows = match layout {
            SeriesLayout::Kline { .. } => WindowRows::Kline(Vec::new()),
            SeriesLayout::Tick { .. } => WindowRows::Tick(Vec::new()),
        };
        for (range_id, range_dt) in id_ranges.iter().zip(dt_ranges.iter()) {
            let target_ranges = rangeset_intersection(&requested, &vec![range_dt.clone()]);
            let Some(target_range) = target_ranges.first() else {
                continue;
            };
            let path = self.get_data_file_path(symbol, layout.duration_nano(), range_id);
            let mut file = File::open(path)?;
            let row_count = row_count(&file, layout)?;
            if row_count == 0 {
                continue;
            }
            let Some(start_index) = last_index_where(&mut file, layout, row_count, |dt| dt <= target_range.start)?
            else {
                continue;
            };
            let Some(end_index) = last_index_where(&mut file, layout, row_count, |dt| dt < target_range.end)? else {
                continue;
            };
            if start_index > end_index {
                continue;
            }
            for index in start_index..=end_index {
                rows.push_row(read_row(&mut file, layout, index)?);
            }
        }
        Ok(rows)
    }

    fn get_data_file_path(&self, symbol: &str, dur_nano: i64, range: &Range) -> PathBuf {
        self.base_path
            .join(format!("{symbol}.{dur_nano}.{}.{}", range.start, range.end))
    }

    fn get_temp_path(&self, symbol: &str, dur_nano: i64) -> PathBuf {
        self.base_path.join(format!("{symbol}.{dur_nano}.temp"))
    }

    fn get_lock_path(&self, symbol: &str, dur_nano: i64) -> PathBuf {
        self.base_path.join(format!(".{symbol}.{dur_nano}.lock"))
    }

    fn list_cache_files(&self) -> Result<Vec<CacheFileMeta>> {
        if !self.base_path.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.base_path)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let filename = entry.file_name();
            let filename = filename.to_string_lossy();
            if parse_data_file_name(&filename).is_none() {
                continue;
            }
            let metadata = entry.metadata()?;
            files.push(CacheFileMeta {
                path: entry.path(),
                size_bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            });
        }
        Ok(files)
    }

    fn evict_expired_files(&self, retention_days: Option<u64>) -> Result<()> {
        let Some(days) = retention_days else {
            return Ok(());
        };
        let ttl = Duration::from_secs(days.saturating_mul(24 * 60 * 60));
        let cutoff = SystemTime::now().checked_sub(ttl).unwrap_or(UNIX_EPOCH);
        for file in self.list_cache_files()? {
            if file.modified <= cutoff {
                let _ = fs::remove_file(file.path);
            }
        }
        Ok(())
    }

    fn evict_by_total_size(&self, max_bytes: Option<u64>) -> Result<()> {
        let Some(limit) = max_bytes else {
            return Ok(());
        };
        let mut files = self.list_cache_files()?;
        let mut total: u64 = files.iter().map(|file| file.size_bytes).sum();
        if total <= limit {
            return Ok(());
        }
        files.sort_by_key(|file| file.modified);
        for file in files {
            if total <= limit {
                break;
            }
            if fs::remove_file(&file.path).is_ok() {
                total = total.saturating_sub(file.size_bytes);
            }
        }
        Ok(())
    }
}

impl SeriesLayout {
    fn row_size(self) -> u64 {
        let cols = match self {
            SeriesLayout::Kline { .. } => KLINE_DATA_COLS.len(),
            SeriesLayout::Tick { five_level: true } => TICK_5_LEVEL_DATA_COLS.len(),
            SeriesLayout::Tick { five_level: false } => TICK_1_LEVEL_DATA_COLS.len(),
        };
        ((2 + cols) * 8) as u64
    }

    fn duration_nano(self) -> i64 {
        match self {
            SeriesLayout::Kline { dur_nano } => dur_nano,
            SeriesLayout::Tick { .. } => 0,
        }
    }
}

enum DecodedRow {
    Kline(Kline),
    Tick(Tick),
}

enum WindowRows {
    Kline(Vec<Kline>),
    Tick(Vec<Tick>),
}

impl WindowRows {
    fn push_row(&mut self, row: DecodedRow) {
        match (self, row) {
            (WindowRows::Kline(rows), DecodedRow::Kline(row)) => rows.push(row),
            (WindowRows::Tick(rows), DecodedRow::Tick(row)) => rows.push(row),
            _ => {}
        }
    }

    fn into_klines(self) -> Vec<Kline> {
        match self {
            WindowRows::Kline(rows) => dedup_klines(rows),
            WindowRows::Tick(_) => Vec::new(),
        }
    }

    fn into_ticks(self) -> Vec<Tick> {
        match self {
            WindowRows::Tick(rows) => dedup_ticks(rows),
            WindowRows::Kline(_) => Vec::new(),
        }
    }
}

fn monotonic_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn parse_data_file_name(filename: &str) -> Option<(String, i64, Range)> {
    if filename.starts_with('.') || filename.ends_with(".lock") || filename.ends_with(".temp") {
        return None;
    }
    let parts: Vec<&str> = filename.split('.').collect();
    if parts.len() < 4 {
        return None;
    }
    let end = parts.last()?.parse::<i64>().ok()?;
    let start = parts.get(parts.len() - 2)?.parse::<i64>().ok()?;
    let dur_nano = parts.get(parts.len() - 3)?.parse::<i64>().ok()?;
    if start >= end {
        return None;
    }
    let symbol = parts[..parts.len() - 3].join(".");
    if symbol.is_empty() {
        return None;
    }
    Some((symbol, dur_nano, Range::new(start, end)))
}

fn tick_uses_five_levels(symbol: &str) -> bool {
    matches!(symbol.split('.').next(), Some("SHFE" | "SSE" | "SZSE"))
}

fn trim_last_datetime_range_inner(ranges: &mut RangeSet, dur_nano: i64) {
    let Some(last) = ranges.last_mut() else {
        return;
    };
    let new_end = last.end.saturating_sub(dur_nano);
    last.end = new_end.max(last.start);
    if last.is_empty() {
        let _ = ranges.pop();
    }
}

pub(crate) fn trim_last_datetime_range(ranges: &mut RangeSet, dur_nano: i64) {
    trim_last_datetime_range_inner(ranges, dur_nano);
}

fn row_count(file: &File, layout: SeriesLayout) -> Result<usize> {
    let len = file.metadata()?.len();
    let row_size = layout.row_size();
    if len == 0 {
        return Ok(0);
    }
    if len % row_size != 0 {
        return Err(TqError::ParseError("缓存文件长度与记录宽度不匹配".to_string()));
    }
    usize::try_from(len / row_size).map_err(|_| TqError::ParseError("缓存文件过大".to_string()))
}

fn last_index_where<F>(file: &mut File, layout: SeriesLayout, row_count: usize, predicate: F) -> Result<Option<usize>>
where
    F: Fn(i64) -> bool,
{
    let mut lo = 0usize;
    let mut hi = row_count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if predicate(read_datetime_at(file, layout, mid)?) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 { Ok(None) } else { Ok(Some(lo - 1)) }
}

fn read_datetime_at(file: &mut File, layout: SeriesLayout, index: usize) -> Result<i64> {
    let offset = (index as u64)
        .checked_mul(layout.row_size())
        .and_then(|base| base.checked_add(8))
        .ok_or_else(|| TqError::ParseError("缓存偏移量溢出".to_string()))?;
    file.seek(SeekFrom::Start(offset))?;
    read_i64(file)
}

fn read_row(file: &mut File, layout: SeriesLayout, index: usize) -> Result<DecodedRow> {
    let offset = (index as u64)
        .checked_mul(layout.row_size())
        .ok_or_else(|| TqError::ParseError("缓存偏移量溢出".to_string()))?;
    file.seek(SeekFrom::Start(offset))?;
    let id = read_i64(file)?;
    let datetime = read_i64(file)?;
    match layout {
        SeriesLayout::Kline { .. } => {
            let row = Kline {
                id,
                datetime,
                open: read_f64(file)?,
                high: read_f64(file)?,
                low: read_f64(file)?,
                close: read_f64(file)?,
                volume: f64_to_i64(read_f64(file)?),
                open_oi: f64_to_i64(read_f64(file)?),
                close_oi: f64_to_i64(read_f64(file)?),
                epoch: None,
            };
            Ok(DecodedRow::Kline(row))
        }
        SeriesLayout::Tick { five_level } => {
            let mut row = Tick {
                id,
                datetime,
                last_price: read_f64(file)?,
                highest: read_f64(file)?,
                lowest: read_f64(file)?,
                average: read_f64(file)?,
                volume: f64_to_i64(read_f64(file)?),
                amount: read_f64(file)?,
                open_interest: f64_to_i64(read_f64(file)?),
                ..Tick::default()
            };
            row.bid_price1 = read_f64(file)?;
            row.bid_volume1 = f64_to_i64(read_f64(file)?);
            row.ask_price1 = read_f64(file)?;
            row.ask_volume1 = f64_to_i64(read_f64(file)?);
            if five_level {
                row.bid_price2 = read_f64(file)?;
                row.bid_volume2 = f64_to_i64(read_f64(file)?);
                row.ask_price2 = read_f64(file)?;
                row.ask_volume2 = f64_to_i64(read_f64(file)?);
                row.bid_price3 = read_f64(file)?;
                row.bid_volume3 = f64_to_i64(read_f64(file)?);
                row.ask_price3 = read_f64(file)?;
                row.ask_volume3 = f64_to_i64(read_f64(file)?);
                row.bid_price4 = read_f64(file)?;
                row.bid_volume4 = f64_to_i64(read_f64(file)?);
                row.ask_price4 = read_f64(file)?;
                row.ask_volume4 = f64_to_i64(read_f64(file)?);
                row.bid_price5 = read_f64(file)?;
                row.bid_volume5 = f64_to_i64(read_f64(file)?);
                row.ask_price5 = read_f64(file)?;
                row.ask_volume5 = f64_to_i64(read_f64(file)?);
            }
            Ok(DecodedRow::Tick(row))
        }
    }
}

fn copy_rows(path: &Path, layout: SeriesLayout, rows_to_copy: usize, writer: &mut BufWriter<&mut File>) -> Result<()> {
    let mut file = File::open(path)?;
    let total_rows = row_count(&file, layout)?;
    let rows_to_copy = rows_to_copy.min(total_rows);
    let bytes_to_copy = (rows_to_copy as u64)
        .checked_mul(layout.row_size())
        .ok_or_else(|| TqError::ParseError("缓存拷贝长度溢出".to_string()))?;
    let mut remaining = bytes_to_copy as usize;
    let mut buffer = [0u8; 8192];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len());
        file.read_exact(&mut buffer[..chunk])?;
        writer.write_all(&buffer[..chunk])?;
        remaining -= chunk;
    }
    Ok(())
}

fn build_merge_groups(rangeset: &[Range]) -> Result<Vec<Vec<(Range, usize)>>> {
    if rangeset.is_empty() {
        return Ok(Vec::new());
    }
    let mut groups = vec![vec![(rangeset[0].clone(), rangeset[0].len() as usize)]];
    for range in rangeset.iter().skip(1) {
        let previous = groups
            .last()
            .and_then(|group| group.last())
            .map(|entry| entry.0.clone())
            .ok_or_else(|| TqError::ParseError("合并分组状态损坏".to_string()))?;
        let contiguous = previous.end == range.start;
        let tail_overlap = previous
            .end
            .checked_sub(1)
            .is_some_and(|end_minus_one| end_minus_one == range.start);
        if contiguous {
            groups
                .last_mut()
                .expect("groups should not be empty")
                .push((range.clone(), range.len() as usize));
        } else if tail_overlap {
            let last_group = groups.last_mut().expect("groups should not be empty");
            if let Some(last) = last_group.last_mut() {
                last.1 = last.1.saturating_sub(1);
            }
            last_group.push((range.clone(), range.len() as usize));
        } else {
            groups.push(vec![(range.clone(), range.len() as usize)]);
        }
    }
    Ok(groups)
}

fn range_from_ids<I>(ids: I) -> Result<Option<Range>>
where
    I: Iterator<Item = i64>,
{
    let mut min_id = None;
    let mut max_id = None;
    for id in ids {
        min_id = Some(min_id.map_or(id, |current: i64| current.min(id)));
        max_id = Some(max_id.map_or(id, |current: i64| current.max(id)));
    }
    match (min_id, max_id) {
        (Some(start), Some(end)) => {
            let exclusive_end = end
                .checked_add(1)
                .ok_or_else(|| TqError::ParseError("缓存 id 范围溢出".to_string()))?;
            Ok(Some(Range::new(start, exclusive_end)))
        }
        _ => Ok(None),
    }
}

fn dedup_klines(rows: Vec<Kline>) -> Vec<Kline> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        by_id.insert(row.id, row);
    }
    by_id.into_values().collect()
}

fn dedup_ticks(rows: Vec<Tick>) -> Vec<Tick> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        by_id.insert(row.id, row);
    }
    by_id.into_values().collect()
}

fn f64_to_i64(value: f64) -> i64 {
    if value.is_finite() { value as i64 } else { 0 }
}

fn write_i64<W: Write>(writer: &mut W, value: i64) -> Result<()> {
    writer.write_all(&value.to_ne_bytes())?;
    Ok(())
}

fn write_f64<W: Write>(writer: &mut W, value: f64) -> Result<()> {
    writer.write_all(&value.to_ne_bytes())?;
    Ok(())
}

fn read_i64<R: Read>(reader: &mut R) -> Result<i64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_ne_bytes(bytes))
}

fn read_f64<R: Read>(reader: &mut R) -> Result<f64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_ne_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        DataSeriesCache, KLINE_DATA_COLS, PAGE_VIEW_WIDTH, TICK_1_LEVEL_DATA_COLS, TICK_5_LEVEL_DATA_COLS,
        trim_last_datetime_range,
    };
    use crate::types::{Kline, Range, Tick};
    use std::env;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_cache_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        env::temp_dir().join(format!("tqsdk_rs_data_series_{name}_{nanos}"))
    }

    fn write_native_i64(file: &mut File, value: i64) {
        file.write_all(&value.to_ne_bytes()).expect("write i64 should succeed");
    }

    fn write_native_f64(file: &mut File, value: f64) {
        file.write_all(&value.to_ne_bytes()).expect("write f64 should succeed");
    }

    fn write_kline_file(path: &Path, rows: &[Kline]) {
        let mut file = File::create(path).expect("create file should succeed");
        for row in rows {
            write_native_i64(&mut file, row.id);
            write_native_i64(&mut file, row.datetime);
            write_native_f64(&mut file, row.open);
            write_native_f64(&mut file, row.high);
            write_native_f64(&mut file, row.low);
            write_native_f64(&mut file, row.close);
            write_native_f64(&mut file, row.volume as f64);
            write_native_f64(&mut file, row.open_oi as f64);
            write_native_f64(&mut file, row.close_oi as f64);
        }
        file.flush().expect("flush should succeed");
    }

    fn write_tick_file(path: &Path, rows: &[Tick], five_level: bool) {
        let mut file = File::create(path).expect("create file should succeed");
        for row in rows {
            write_native_i64(&mut file, row.id);
            write_native_i64(&mut file, row.datetime);
            write_native_f64(&mut file, row.last_price);
            write_native_f64(&mut file, row.highest);
            write_native_f64(&mut file, row.lowest);
            write_native_f64(&mut file, row.average);
            write_native_f64(&mut file, row.volume as f64);
            write_native_f64(&mut file, row.amount);
            write_native_f64(&mut file, row.open_interest as f64);
            write_native_f64(&mut file, row.bid_price1);
            write_native_f64(&mut file, row.bid_volume1 as f64);
            write_native_f64(&mut file, row.ask_price1);
            write_native_f64(&mut file, row.ask_volume1 as f64);
            if five_level {
                write_native_f64(&mut file, row.bid_price2);
                write_native_f64(&mut file, row.bid_volume2 as f64);
                write_native_f64(&mut file, row.ask_price2);
                write_native_f64(&mut file, row.ask_volume2 as f64);
                write_native_f64(&mut file, row.bid_price3);
                write_native_f64(&mut file, row.bid_volume3 as f64);
                write_native_f64(&mut file, row.ask_price3);
                write_native_f64(&mut file, row.ask_volume3 as f64);
                write_native_f64(&mut file, row.bid_price4);
                write_native_f64(&mut file, row.bid_volume4 as f64);
                write_native_f64(&mut file, row.ask_price4);
                write_native_f64(&mut file, row.ask_volume4 as f64);
                write_native_f64(&mut file, row.bid_price5);
                write_native_f64(&mut file, row.bid_volume5 as f64);
                write_native_f64(&mut file, row.ask_price5);
                write_native_f64(&mut file, row.ask_volume5 as f64);
            }
        }
        file.flush().expect("flush should succeed");
    }

    fn sample_kline(id: i64, dt: i64, close: f64) -> Kline {
        Kline {
            id,
            datetime: dt,
            open: close - 1.0,
            high: close + 1.0,
            low: close - 2.0,
            close,
            volume: id * 10,
            open_oi: id * 100,
            close_oi: id * 100 + 1,
            epoch: None,
        }
    }

    fn sample_tick(id: i64, dt: i64, px: f64) -> Tick {
        Tick {
            id,
            datetime: dt,
            last_price: px,
            highest: px + 10.0,
            lowest: px - 10.0,
            average: px + 1.0,
            volume: id * 10,
            amount: px * 1000.0,
            open_interest: id * 100,
            bid_price1: px - 1.0,
            bid_volume1: 11,
            bid_price2: px - 2.0,
            bid_volume2: 12,
            bid_price3: px - 3.0,
            bid_volume3: 13,
            bid_price4: px - 4.0,
            bid_volume4: 14,
            bid_price5: px - 5.0,
            bid_volume5: 15,
            ask_price1: px + 1.0,
            ask_volume1: 21,
            ask_price2: px + 2.0,
            ask_volume2: 22,
            ask_price3: px + 3.0,
            ask_volume3: 23,
            ask_price4: px + 4.0,
            ask_volume4: 24,
            ask_price5: px + 5.0,
            ask_volume5: 25,
            epoch: None,
        }
    }

    #[test]
    fn constants_match_python_layout() {
        assert_eq!(KLINE_DATA_COLS.len(), 7);
        assert_eq!(TICK_5_LEVEL_DATA_COLS.len(), 27);
        assert_eq!(TICK_1_LEVEL_DATA_COLS.len(), 11);
        assert_eq!(PAGE_VIEW_WIDTH, 2000);
    }

    #[test]
    fn scan_python_named_kline_files_and_derive_datetime_ranges() {
        let dir = test_cache_dir("scan_ranges");
        fs::create_dir_all(&dir).expect("create dir should succeed");
        let symbol = "SHFE.au2606";
        let duration = 60_000_000_000i64;
        let path = dir.join(format!("{symbol}.{duration}.10.13"));
        write_kline_file(
            &path,
            &[
                sample_kline(10, 1_000, 100.0),
                sample_kline(11, 61_000_000_000, 101.0),
                sample_kline(12, 121_000_000_000, 102.0),
            ],
        );
        let cache = DataSeriesCache::new(Some(dir.clone()));

        let id_ranges = cache
            .get_rangeset_id(symbol, duration)
            .expect("scan id ranges should succeed");
        let dt_ranges = cache
            .get_rangeset_dt(symbol, duration, &id_ranges)
            .expect("scan dt ranges should succeed");

        assert_eq!(id_ranges, vec![Range::new(10, 13)]);
        assert_eq!(dt_ranges, vec![Range::new(1_000, 181_000_000_000)]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn trim_last_datetime_range_matches_python_behavior() {
        let mut ranges = vec![Range::new(10, 20), Range::new(30, 90)];

        trim_last_datetime_range(&mut ranges, 60);

        assert_eq!(ranges, vec![Range::new(10, 20)]);
    }

    #[test]
    fn merge_adjacent_files_handles_tail_overlap() {
        let dir = test_cache_dir("merge_overlap");
        fs::create_dir_all(&dir).expect("create dir should succeed");
        let symbol = "SHFE.rb2610";
        let duration = 1_000_000_000i64;
        write_kline_file(
            &dir.join(format!("{symbol}.{duration}.0.3")),
            &[
                sample_kline(0, 0, 10.0),
                sample_kline(1, 1_000_000_000, 11.0),
                sample_kline(2, 2_000_000_000, 12.0),
            ],
        );
        write_kline_file(
            &dir.join(format!("{symbol}.{duration}.2.4")),
            &[
                sample_kline(2, 2_000_000_000, 120.0),
                sample_kline(3, 3_000_000_000, 13.0),
            ],
        );

        let cache = DataSeriesCache::new(Some(dir.clone()));
        cache
            .merge_adjacent_files(symbol, duration)
            .expect("merge should succeed");

        let id_ranges = cache
            .get_rangeset_id(symbol, duration)
            .expect("scan id ranges should succeed");
        let rows = cache
            .read_kline_window(symbol, duration, 0, 4_000_000_000)
            .expect("read merged rows should succeed");

        assert_eq!(id_ranges, vec![Range::new(0, 4)]);
        assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert_eq!(rows[2].close, 120.0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_kline_window_uses_python_boundary_rules() {
        let dir = test_cache_dir("read_kline_window");
        fs::create_dir_all(&dir).expect("create dir should succeed");
        let symbol = "SHFE.ag2606";
        let duration = 100i64;
        write_kline_file(
            &dir.join(format!("{symbol}.{duration}.20.24")),
            &[
                sample_kline(20, 100, 10.0),
                sample_kline(21, 200, 11.0),
                sample_kline(22, 300, 12.0),
                sample_kline(23, 400, 13.0),
            ],
        );

        let cache = DataSeriesCache::new(Some(dir.clone()));
        let rows = cache
            .read_kline_window(symbol, duration, 250, 350)
            .expect("read window should succeed");

        assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![21, 22]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_tick_window_supports_python_5_level_and_1_level_layouts() {
        let dir = test_cache_dir("read_tick_window");
        fs::create_dir_all(&dir).expect("create dir should succeed");

        let shfe_symbol = "SHFE.au2606";
        write_tick_file(
            &dir.join(format!("{shfe_symbol}.0.1.4")),
            &[
                sample_tick(1, 100, 10.0),
                sample_tick(2, 200, 11.0),
                sample_tick(3, 300, 12.0),
            ],
            true,
        );

        let dce_symbol = "DCE.m2609";
        write_tick_file(
            &dir.join(format!("{dce_symbol}.0.5.8")),
            &[
                sample_tick(5, 100, 20.0),
                sample_tick(6, 200, 21.0),
                sample_tick(7, 300, 22.0),
            ],
            false,
        );

        let cache = DataSeriesCache::new(Some(dir.clone()));

        let shfe_rows = cache
            .read_tick_window(shfe_symbol, 150, 350)
            .expect("read shfe ticks should succeed");
        let dce_rows = cache
            .read_tick_window(dce_symbol, 150, 350)
            .expect("read dce ticks should succeed");

        assert_eq!(shfe_rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(shfe_rows[0].bid_price5, 5.0);
        assert_eq!(shfe_rows[0].ask_volume5, 25);
        assert_eq!(dce_rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![5, 6, 7]);
        assert!(dce_rows[0].bid_price2.is_nan());
        assert_eq!(dce_rows[0].ask_volume1, 21);

        let _ = fs::remove_dir_all(dir);
    }
}
