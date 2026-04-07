// K线缓存模块

use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, RangeSet, rangeset_merge};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

const CACHE_DIR: &str = ".tqsdk_cache";
const SEGMENT_FILE_EXT: &str = "kline";

#[cfg(not(test))]
const WRITE_SEGMENT_MAX_BARS: usize = 2048;
#[cfg(test)]
const WRITE_SEGMENT_MAX_BARS: usize = 4;

#[cfg(not(test))]
const COMPACT_SEGMENT_MAX_BARS: usize = 8192;
#[cfg(test)]
const COMPACT_SEGMENT_MAX_BARS: usize = 16;

#[cfg(not(test))]
const SEGMENT_COMPACT_TRIGGER: usize = 256;
#[cfg(test)]
const SEGMENT_COMPACT_TRIGGER: usize = 5;

#[cfg(not(test))]
const SEGMENT_COMPACT_TARGET: usize = 128;
#[cfg(test)]
const SEGMENT_COMPACT_TARGET: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KlineCacheMetadata {
    pub symbol: String,
    pub duration: i64,
    pub ranges: RangeSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedKline {
    id: i64,
    datetime: i64,
    open: f64,
    close: f64,
    high: f64,
    low: f64,
    open_oi: i64,
    close_oi: i64,
    volume: i64,
    epoch: Option<i64>,
}

impl From<&Kline> for CachedKline {
    fn from(value: &Kline) -> Self {
        Self {
            id: value.id,
            datetime: value.datetime,
            open: value.open,
            close: value.close,
            high: value.high,
            low: value.low,
            open_oi: value.open_oi,
            close_oi: value.close_oi,
            volume: value.volume,
            epoch: value.epoch,
        }
    }
}

impl From<CachedKline> for Kline {
    fn from(value: CachedKline) -> Self {
        Self {
            id: value.id,
            datetime: value.datetime,
            open: value.open,
            close: value.close,
            high: value.high,
            low: value.low,
            open_oi: value.open_oi,
            close_oi: value.close_oi,
            volume: value.volume,
            epoch: value.epoch,
        }
    }
}

#[derive(Debug, Clone)]
struct SegmentFile {
    path: PathBuf,
    range: Range,
}

#[derive(Debug, Clone)]
struct SegmentFileMeta {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
}

pub struct DiskKlineCache {
    base_path: PathBuf,
    write_lock: Mutex<()>,
}

struct DiskWriteGuards<'a> {
    _in_process_lock: std::sync::MutexGuard<'a, ()>,
    lock_file: File,
}

impl Drop for DiskWriteGuards<'_> {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

impl DiskKlineCache {
    pub fn new(base_path: Option<PathBuf>) -> Self {
        let mut resolved_base_path = base_path.unwrap_or_else(|| {
            dirs::home_dir()
                .map(|home_dir| home_dir.join(CACHE_DIR))
                .unwrap_or_else(|| std::env::temp_dir().join(CACHE_DIR))
        });
        if let Err(e) = fs::create_dir_all(&resolved_base_path) {
            let fallback = std::env::temp_dir().join(CACHE_DIR);
            if fallback != resolved_base_path && fs::create_dir_all(&fallback).is_ok() {
                warn!(
                    "创建缓存目录失败，回退到临时目录: path={}, error={}",
                    resolved_base_path.display(),
                    e
                );
                resolved_base_path = fallback;
            } else {
                warn!(
                    "创建缓存目录失败，将在首次写入时重试: path={}, error={}",
                    resolved_base_path.display(),
                    e
                );
            }
        }
        Self {
            base_path: resolved_base_path,
            write_lock: Mutex::new(()),
        }
    }

    fn get_segment_dir_path(&self, symbol: &str, duration: i64) -> PathBuf {
        self.base_path.join("kline").join(symbol).join(duration.to_string())
    }

    fn get_segment_file_path(&self, symbol: &str, duration: i64, range: &Range) -> PathBuf {
        self.get_segment_dir_path(symbol, duration)
            .join(format!("{}.{}.{}", range.start, range.end, SEGMENT_FILE_EXT))
    }

    fn get_temp_segment_file_path(&self, symbol: &str, duration: i64, range: &Range) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.get_segment_dir_path(symbol, duration)
            .join(format!("{}.{}.{}.tmp", range.start, range.end, suffix))
    }

    fn get_lock_file_path(&self, symbol: &str, duration: i64) -> PathBuf {
        self.base_path
            .join("kline")
            .join(symbol)
            .join(format!("{duration}.lock"))
    }

    fn ensure_parent_dir(file_path: &Path) -> Result<()> {
        if let Some(parent) = file_path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn ensure_dir(dir: &Path) -> Result<()> {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    fn lock_writes(&self, symbol: &str, duration: i64) -> Result<DiskWriteGuards<'_>> {
        let in_process_lock = self
            .write_lock
            .lock()
            .map_err(|e| TqError::Other(format!("缓存写锁已中毒: {e}")))?;
        let lock_path = self.get_lock_file_path(symbol, duration);
        Self::ensure_parent_dir(&lock_path)?;
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        lock_file
            .lock_exclusive()
            .map_err(|e| TqError::Other(format!("获取缓存文件锁失败: {e}")))?;
        Ok(DiskWriteGuards {
            _in_process_lock: in_process_lock,
            lock_file,
        })
    }

    pub fn read_metadata(&self, symbol: &str, duration: i64) -> Result<Option<KlineCacheMetadata>> {
        let segment_ranges: Vec<Range> = self
            .list_segment_files(symbol, duration)?
            .into_iter()
            .map(|segment| segment.range)
            .collect();

        let ranges = rangeset_merge(segment_ranges);

        if ranges.is_empty() {
            return Ok(None);
        }

        Ok(Some(KlineCacheMetadata {
            symbol: symbol.to_string(),
            duration,
            ranges,
        }))
    }

    pub fn write_metadata(&self, metadata: &KlineCacheMetadata) -> Result<()> {
        let _guard = self.lock_writes(&metadata.symbol, metadata.duration)?;
        let all_klines = self.read_all_segment_klines_locked(&metadata.symbol, metadata.duration)?;
        self.rewrite_segments_from_klines_locked(
            &metadata.symbol,
            metadata.duration,
            &all_klines,
            COMPACT_SEGMENT_MAX_BARS,
        )
    }

    pub fn read_klines(&self, symbol: &str, duration: i64, range: &Range) -> Result<Vec<Kline>> {
        let mut by_id = BTreeMap::<i64, Kline>::new();
        for segment in self.list_segment_files(symbol, duration)? {
            if !ranges_intersection_exists(&segment.range, range) {
                continue;
            }
            for kline in Self::read_segment_klines_from_file(&segment.path)? {
                if range.start <= kline.id && kline.id < range.end {
                    by_id.insert(kline.id, kline);
                }
            }
        }

        Ok(by_id.into_values().collect())
    }

    pub fn append_klines(&self, symbol: &str, duration: i64, klines: &[Kline]) -> Result<()> {
        if klines.is_empty() {
            return Ok(());
        }

        let _guard = self.lock_writes(symbol, duration)?;

        let deduped = dedup_sort_klines_by_id(klines.to_vec());
        for run in split_contiguous_runs(deduped) {
            self.append_contiguous_run_locked(symbol, duration, &run)?;
        }
        self.maybe_compact_segments_locked(symbol, duration)?;
        Ok(())
    }

    pub fn total_segment_bytes(&self) -> Result<u64> {
        let files = self.list_all_segment_files()?;
        Ok(files.into_iter().map(|f| f.size_bytes).sum())
    }

    pub fn enforce_limits(&self, max_bytes: Option<u64>, retention_days: Option<u64>) -> Result<()> {
        let _in_process_lock = self
            .write_lock
            .lock()
            .map_err(|e| TqError::Other(format!("缓存写锁已中毒: {e}")))?;

        self.evict_expired_segments(retention_days)?;
        self.evict_by_total_size(max_bytes)?;
        self.cleanup_empty_kline_dirs()?;
        Ok(())
    }

    fn evict_expired_segments(&self, retention_days: Option<u64>) -> Result<()> {
        let Some(days) = retention_days else {
            return Ok(());
        };

        let ttl = Duration::from_secs(days.saturating_mul(24 * 60 * 60));
        let cutoff = SystemTime::now().checked_sub(ttl).unwrap_or(UNIX_EPOCH);

        for file in self.list_all_segment_files()? {
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

        let mut files = self.list_all_segment_files()?;
        let mut total: u64 = files.iter().map(|f| f.size_bytes).sum();
        if total <= limit {
            return Ok(());
        }

        files.sort_by_key(|f| f.modified);
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

    fn list_all_segment_files(&self) -> Result<Vec<SegmentFileMeta>> {
        let root = self.base_path.join("kline");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        self.collect_segment_files_recursive(&root, &mut out)?;
        Ok(out)
    }

    fn collect_segment_files_recursive(&self, dir: &Path, out: &mut Vec<SegmentFileMeta>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                self.collect_segment_files_recursive(&path, out)?;
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some(SEGMENT_FILE_EXT) {
                continue;
            }
            let metadata = entry.metadata()?;
            out.push(SegmentFileMeta {
                path,
                size_bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            });
        }
        Ok(())
    }

    fn cleanup_empty_kline_dirs(&self) -> Result<()> {
        let root = self.base_path.join("kline");
        if !root.exists() {
            return Ok(());
        }
        self.cleanup_empty_dirs_recursive(&root)?;
        Ok(())
    }

    fn cleanup_empty_dirs_recursive(&self, dir: &Path) -> Result<bool> {
        let mut has_entries = false;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                let child_has_entries = self.cleanup_empty_dirs_recursive(&path)?;
                if !child_has_entries {
                    let _ = fs::remove_dir(&path);
                } else {
                    has_entries = true;
                }
            } else {
                has_entries = true;
            }
        }
        Ok(has_entries)
    }

    fn append_contiguous_run_locked(&self, symbol: &str, duration: i64, run: &[Kline]) -> Result<()> {
        if run.is_empty() {
            return Ok(());
        }
        let Some(run_range) = range_from_klines(run) else {
            return Ok(());
        };

        let segments = self.list_segment_files(symbol, duration)?;
        let touched: Vec<SegmentFile> = segments
            .into_iter()
            .filter(|segment| ranges_touch_or_overlap(&segment.range, &run_range))
            .collect();

        let mut merged = BTreeMap::<i64, Kline>::new();
        for segment in touched {
            for kline in Self::read_segment_klines_from_file(&segment.path)? {
                merged.insert(kline.id, kline);
            }
            let _ = fs::remove_file(&segment.path);
        }
        // 新写入的数据优先级更高，覆盖旧段中的同 id 数据。
        for kline in run {
            merged.insert(kline.id, kline.clone());
        }

        let merged_klines: Vec<Kline> = merged.into_values().collect();
        self.write_contiguous_runs_as_segments(symbol, duration, &merged_klines, WRITE_SEGMENT_MAX_BARS)
    }

    fn write_contiguous_runs_as_segments(
        &self,
        symbol: &str,
        duration: i64,
        klines: &[Kline],
        max_bars_per_segment: usize,
    ) -> Result<()> {
        if klines.is_empty() {
            return Ok(());
        }
        for run in split_contiguous_runs(dedup_sort_klines_by_id(klines.to_vec())) {
            for chunk in split_run_into_chunked_segments(run, max_bars_per_segment.max(1)) {
                let Some(range) = range_from_klines(&chunk) else {
                    continue;
                };
                self.write_segment_file_atomic(symbol, duration, &range, &chunk)?;
            }
        }
        Ok(())
    }

    fn rewrite_segments_from_klines_locked(
        &self,
        symbol: &str,
        duration: i64,
        klines: &[Kline],
        max_bars_per_segment: usize,
    ) -> Result<()> {
        for segment in self.list_segment_files(symbol, duration)? {
            let _ = fs::remove_file(segment.path);
        }
        self.write_contiguous_runs_as_segments(symbol, duration, klines, max_bars_per_segment)
    }

    fn maybe_compact_segments_locked(&self, symbol: &str, duration: i64) -> Result<()> {
        let mut segments = self.list_segment_files(symbol, duration)?;
        if segments.len() <= SEGMENT_COMPACT_TRIGGER {
            return Ok(());
        }

        let mut groups: Vec<Vec<SegmentFile>> = Vec::new();
        let mut current: Vec<SegmentFile> = Vec::new();
        for segment in segments.drain(..) {
            let is_contiguous = current.last().is_some_and(|last| last.range.end == segment.range.start);
            if current.is_empty() || is_contiguous {
                current.push(segment);
                continue;
            }
            groups.push(current);
            current = vec![segment];
        }
        if !current.is_empty() {
            groups.push(current);
        }

        let mut total_segments: usize = groups.iter().map(std::vec::Vec::len).sum();
        if total_segments <= SEGMENT_COMPACT_TRIGGER {
            return Ok(());
        }

        for group in groups {
            if total_segments <= SEGMENT_COMPACT_TARGET || group.len() <= 1 {
                continue;
            }
            let before = group.len();
            let after = self.compact_contiguous_group_locked(symbol, duration, group)?;
            total_segments = total_segments.saturating_sub(before).saturating_add(after);
        }

        Ok(())
    }

    fn compact_contiguous_group_locked(&self, symbol: &str, duration: i64, group: Vec<SegmentFile>) -> Result<usize> {
        let mut merged = Vec::new();
        for segment in &group {
            merged.extend(Self::read_segment_klines_from_file(&segment.path)?);
        }
        let merged = dedup_sort_klines_by_id(merged);
        for segment in &group {
            let _ = fs::remove_file(&segment.path);
        }
        let compacted_count = count_chunked_segments(&merged, COMPACT_SEGMENT_MAX_BARS.max(1));
        self.write_contiguous_runs_as_segments(symbol, duration, &merged, COMPACT_SEGMENT_MAX_BARS)?;
        Ok(compacted_count)
    }

    fn write_segment_file_atomic(&self, symbol: &str, duration: i64, range: &Range, klines: &[Kline]) -> Result<()> {
        let segment_dir = self.get_segment_dir_path(symbol, duration);
        Self::ensure_dir(&segment_dir)?;

        let target_path = self.get_segment_file_path(symbol, duration, range);
        let temp_path = self.get_temp_segment_file_path(symbol, duration, range);

        let mut temp_file = File::create(&temp_path)?;
        let mut writer = BufWriter::new(&mut temp_file);
        for kline in klines {
            let kline_bytes = bincode::serialize(&CachedKline::from(kline))?;
            let kline_len = kline_bytes.len() as u64;
            writer.write_all(&kline_len.to_le_bytes())?;
            writer.write_all(&kline_bytes)?;
        }
        writer.flush()?;
        drop(writer);
        temp_file.sync_all()?;
        fs::rename(&temp_path, &target_path)?;
        Ok(())
    }

    fn list_segment_files(&self, symbol: &str, duration: i64) -> Result<Vec<SegmentFile>> {
        let segment_dir = self.get_segment_dir_path(symbol, duration);
        if !segment_dir.exists() {
            return Ok(Vec::new());
        }

        let mut segments = Vec::new();
        for entry in fs::read_dir(&segment_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(range) = parse_segment_range_from_path(&path) else {
                continue;
            };
            segments.push(SegmentFile { path, range });
        }
        segments.sort_by_key(|seg| (seg.range.start, seg.range.end));
        Ok(segments)
    }

    fn read_all_segment_klines_locked(&self, symbol: &str, duration: i64) -> Result<Vec<Kline>> {
        let mut by_id = BTreeMap::<i64, Kline>::new();
        for segment in self.list_segment_files(symbol, duration)? {
            for kline in Self::read_segment_klines_from_file(&segment.path)? {
                by_id.insert(kline.id, kline);
            }
        }
        Ok(by_id.into_values().collect())
    }

    fn read_segment_klines_from_file(path: &Path) -> Result<Vec<Kline>> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut klines = Vec::new();
        loop {
            let Some(kline) = Self::read_next_kline(&mut reader)? else {
                break;
            };
            klines.push(kline);
        }
        Ok(klines)
    }

    fn read_next_kline(reader: &mut BufReader<File>) -> Result<Option<Kline>> {
        let mut kline_len_bytes = [0u8; 8];
        match reader.read_exact(&mut kline_len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let kline_len = u64::from_le_bytes(kline_len_bytes) as usize;

        let mut kline_bytes = vec![0u8; kline_len];
        reader.read_exact(&mut kline_bytes)?;
        let kline: CachedKline = bincode::deserialize(&kline_bytes)?;
        Ok(Some(kline.into()))
    }
}

fn parse_segment_range_from_path(path: &Path) -> Option<Range> {
    if path.extension().and_then(|ext| ext.to_str()) != Some(SEGMENT_FILE_EXT) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let mut parts = stem.split('.');
    let start = parts.next()?.parse::<i64>().ok()?;
    let end = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || start >= end {
        return None;
    }
    Some(Range::new(start, end))
}

fn range_from_klines(klines: &[Kline]) -> Option<Range> {
    let min_id = klines.iter().map(|k| k.id).min()?;
    let max_id = klines.iter().map(|k| k.id).max()?;
    max_id.checked_add(1).map(|end| Range::new(min_id, end))
}

fn dedup_sort_klines_by_id(klines: Vec<Kline>) -> Vec<Kline> {
    let mut by_id = BTreeMap::<i64, Kline>::new();
    for kline in klines {
        by_id.insert(kline.id, kline);
    }
    by_id.into_values().collect()
}

fn split_contiguous_runs(klines: Vec<Kline>) -> Vec<Vec<Kline>> {
    if klines.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut current = Vec::new();
    for kline in klines {
        let contiguous = current
            .last()
            .and_then(|last: &Kline| last.id.checked_add(1))
            .is_some_and(|next_id| next_id == kline.id);
        if current.is_empty() || contiguous {
            current.push(kline);
            continue;
        }
        runs.push(current);
        current = vec![kline];
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn split_run_into_chunked_segments(run: Vec<Kline>, max_bars_per_segment: usize) -> Vec<Vec<Kline>> {
    if run.is_empty() {
        return Vec::new();
    }
    let chunk_size = max_bars_per_segment.max(1);
    run.chunks(chunk_size).map(|chunk| chunk.to_vec()).collect()
}

fn count_chunked_segments(klines: &[Kline], max_bars_per_segment: usize) -> usize {
    let chunk_size = max_bars_per_segment.max(1);
    split_contiguous_runs(dedup_sort_klines_by_id(klines.to_vec()))
        .into_iter()
        .map(|run| run.len().div_ceil(chunk_size))
        .sum()
}

fn ranges_intersection_exists(a: &Range, b: &Range) -> bool {
    a.start < b.end && b.start < a.end
}

fn ranges_touch_or_overlap(a: &Range, b: &Range) -> bool {
    a.start <= b.end && b.start <= a.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn test_cache_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        env::temp_dir().join(format!("tqsdk_rs_{name}_{nanos}"))
    }

    fn sample_kline(id: i64, close: f64) -> Kline {
        Kline {
            id,
            datetime: id * 1_000_000_000,
            open: close - 1.0,
            close,
            high: close + 1.0,
            low: close - 2.0,
            open_oi: id * 10,
            close_oi: id * 10 + 1,
            volume: id * 100,
            epoch: None,
        }
    }

    fn count_segment_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .expect("segment dir should exist")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some(SEGMENT_FILE_EXT))
            .count()
    }

    #[test]
    fn append_deduplicates_by_id_and_updates_ranges() {
        let dir = test_cache_dir("disk_cache_dedup");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "SHFE.au2606";
        let duration = 60_000_000_000i64;

        cache
            .append_klines(
                symbol,
                duration,
                &[
                    sample_kline(10, 100.0),
                    sample_kline(11, 101.0),
                    sample_kline(12, 102.0),
                ],
            )
            .expect("first append should succeed");
        cache
            .append_klines(symbol, duration, &[sample_kline(11, 111.0), sample_kline(13, 103.0)])
            .expect("second append should succeed");

        let metadata = cache
            .read_metadata(symbol, duration)
            .expect("read metadata should succeed")
            .expect("metadata should exist");
        assert_eq!(metadata.ranges, vec![Range::new(10, 14)]);

        let klines = cache
            .read_klines(symbol, duration, &Range::new(10, 14))
            .expect("read klines should succeed");
        assert_eq!(klines.len(), 4);
        assert_eq!(klines[0].id, 10);
        assert_eq!(klines[1].id, 11);
        assert_eq!(klines[1].close, 111.0);
        assert_eq!(klines[2].id, 12);
        assert_eq!(klines[3].id, 13);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_metadata_preserves_existing_kline_data() {
        let dir = test_cache_dir("disk_cache_write_metadata");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "SHFE.ag2606";
        let duration = 60_000_000_000i64;

        cache
            .append_klines(symbol, duration, &[sample_kline(1, 10.0), sample_kline(2, 20.0)])
            .expect("append should succeed");

        let metadata = KlineCacheMetadata {
            symbol: symbol.to_string(),
            duration,
            ranges: vec![Range::new(1, 3)],
        };
        cache.write_metadata(&metadata).expect("write_metadata should succeed");

        let klines = cache
            .read_klines(symbol, duration, &Range::new(1, 3))
            .expect("read klines should succeed");
        assert_eq!(klines.len(), 2);
        assert_eq!(klines[0].id, 1);
        assert_eq!(klines[1].id, 2);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_non_overlapping_ranges_should_create_multiple_segments() {
        let dir = test_cache_dir("disk_cache_non_overlap_segments");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "CZCE.SR609";
        let duration = 1_000_000_000i64;

        cache
            .append_klines(symbol, duration, &[sample_kline(1, 10.0), sample_kline(2, 20.0)])
            .expect("first append should succeed");
        cache
            .append_klines(symbol, duration, &[sample_kline(10, 30.0), sample_kline(11, 40.0)])
            .expect("second append should succeed");

        let metadata = cache
            .read_metadata(symbol, duration)
            .expect("read metadata should succeed")
            .expect("metadata should exist");
        assert_eq!(metadata.ranges, vec![Range::new(1, 3), Range::new(10, 12)]);

        let segment_dir = cache.get_segment_dir_path(symbol, duration);
        assert_eq!(count_segment_files(&segment_dir), 2);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_bridge_range_should_merge_adjacent_segments() {
        let dir = test_cache_dir("disk_cache_bridge_merge");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "DCE.m2609";
        let duration = 1_000_000_000i64;

        cache
            .append_klines(symbol, duration, &[sample_kline(1, 10.0), sample_kline(2, 20.0)])
            .expect("first append should succeed");
        cache
            .append_klines(symbol, duration, &[sample_kline(4, 40.0), sample_kline(5, 50.0)])
            .expect("second append should succeed");
        cache
            .append_klines(symbol, duration, &[sample_kline(3, 30.0)])
            .expect("bridge append should succeed");

        let metadata = cache
            .read_metadata(symbol, duration)
            .expect("read metadata should succeed")
            .expect("metadata should exist");
        assert_eq!(metadata.ranges, vec![Range::new(1, 6)]);

        let segment_dir = cache.get_segment_dir_path(symbol, duration);
        assert_eq!(
            count_segment_files(&segment_dir),
            5usize.div_ceil(WRITE_SEGMENT_MAX_BARS)
        );

        let klines = cache
            .read_klines(symbol, duration, &Range::new(1, 6))
            .expect("read klines should succeed");
        assert_eq!(klines.iter().map(|k| k.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_long_contiguous_run_should_compact_when_segment_count_exceeds_trigger() {
        let dir = test_cache_dir("disk_cache_compact_trigger");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "DCE.i2609";
        let duration = 1_000_000_000i64;

        let klines: Vec<Kline> = (0..24).map(|id| sample_kline(id, id as f64)).collect();
        cache
            .append_klines(symbol, duration, &klines)
            .expect("append should succeed");

        let segment_dir = cache.get_segment_dir_path(symbol, duration);
        let segment_count = count_segment_files(&segment_dir);
        assert!(
            segment_count <= SEGMENT_COMPACT_TARGET,
            "segment count {segment_count} should be compacted to <= {SEGMENT_COMPACT_TARGET}"
        );

        let loaded = cache
            .read_klines(symbol, duration, &Range::new(0, 24))
            .expect("read klines should succeed");
        assert_eq!(loaded.len(), 24);
        assert_eq!(loaded.first().map(|k| k.id), Some(0));
        assert_eq!(loaded.last().map(|k| k.id), Some(23));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn enforce_limits_should_shrink_total_size_to_limit() {
        let dir = test_cache_dir("disk_cache_limit_size");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "SHFE.au2606";
        let duration = 60_000_000_000i64;

        let klines: Vec<Kline> = (0..40).map(|id| sample_kline(id, 100.0 + id as f64)).collect();
        cache
            .append_klines(symbol, duration, &klines)
            .expect("append should succeed");

        let before = cache.total_segment_bytes().expect("get total size should succeed");
        assert!(before > 0, "cache should contain segment bytes before enforcing limit");

        let limit = (before / 2).max(1);
        cache
            .enforce_limits(Some(limit), None)
            .expect("enforce size limit should succeed");

        let after = cache.total_segment_bytes().expect("get total size should succeed");
        assert!(
            after <= limit,
            "cache size should be <= limit after enforce_limits: after={after}, limit={limit}"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn enforce_limits_with_zero_day_retention_should_purge_segments() {
        let dir = test_cache_dir("disk_cache_retention_zero");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "SHFE.ag2606";
        let duration = 60_000_000_000i64;

        cache
            .append_klines(symbol, duration, &[sample_kline(1, 10.0), sample_kline(2, 20.0)])
            .expect("append should succeed");
        assert!(
            cache.total_segment_bytes().expect("get total size should succeed") > 0,
            "cache should not be empty after append"
        );

        cache
            .enforce_limits(None, Some(0))
            .expect("enforce retention should succeed");

        assert_eq!(cache.total_segment_bytes().expect("get total size should succeed"), 0);
        assert!(
            cache
                .read_metadata(symbol, duration)
                .expect("read metadata should succeed")
                .is_none(),
            "retention should remove all expired segments"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_should_create_lock_file_for_symbol_duration() {
        let dir = test_cache_dir("disk_cache_lock_file");
        let cache = DiskKlineCache::new(Some(dir.clone()));
        let symbol = "DCE.m2609";
        let duration = 60_000_000_000i64;

        cache
            .append_klines(symbol, duration, &[sample_kline(42, 123.0)])
            .expect("append should succeed");

        let lock_path = dir.join("kline").join(symbol).join(format!("{duration}.lock"));
        assert!(
            lock_path.exists(),
            "append should create cross-process lock file at {}",
            lock_path.display()
        );

        let _ = fs::remove_dir_all(dir);
    }
}
