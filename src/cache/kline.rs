// K线缓存模块

use crate::errors::{Result, TqError};
use crate::types::{Kline, Range, RangeSet, rangeset_merge, rangeset_union};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

const CACHE_DIR: &str = ".tqsdk_cache";

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

pub struct DiskKlineCache {
    base_path: PathBuf,
    write_lock: Mutex<()>,
}

impl DiskKlineCache {
    pub fn new(base_path: Option<PathBuf>) -> Self {
        let base_path = base_path.unwrap_or_else(|| {
            let home_dir = dirs::home_dir().expect("无法获取用户主目录");
            home_dir.join(CACHE_DIR)
        });
        // 确保缓存目录存在
        if let Err(e) = std::fs::create_dir_all(&base_path) {
            panic!("无法创建缓存目录 {}: {}", base_path.display(), e);
        }
        Self {
            base_path,
            write_lock: Mutex::new(()),
        }
    }

    fn get_cache_file_path(&self, symbol: &str, duration: i64) -> PathBuf {
        self.base_path
            .join("kline")
            .join(symbol)
            .join(format!("{}.kline", duration))
    }

    fn ensure_parent_dir(file_path: &std::path::Path) -> Result<()> {
        if let Some(parent) = file_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn lock_writes(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.write_lock
            .lock()
            .map_err(|e| TqError::Other(format!("缓存写锁已中毒: {e}")))
    }

    pub fn read_metadata(&self, symbol: &str, duration: i64) -> Result<Option<KlineCacheMetadata>> {
        let file_path = self.get_cache_file_path(symbol, duration);
        if !file_path.exists() {
            return Ok(None);
        }

        let file = File::open(&file_path)?;
        let mut reader = BufReader::new(file);
        Ok(Some(Self::read_metadata_from_reader(&mut reader)?))
    }

    pub fn write_metadata(&self, metadata: &KlineCacheMetadata) -> Result<()> {
        let _guard = self.lock_writes()?;
        let existing_klines = self.read_all_klines(&metadata.symbol, metadata.duration)?;
        self.rewrite_file(&metadata.symbol, metadata.duration, metadata, &existing_klines)
    }

    pub fn read_klines(&self, symbol: &str, duration: i64, range: &Range) -> Result<Vec<Kline>> {
        let file_path = self.get_cache_file_path(symbol, duration);
        if !file_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&file_path)?;
        let mut reader = BufReader::new(file);
        Self::skip_metadata(&mut reader)?;

        let mut klines = Vec::new();
        loop {
            let Some(kline) = Self::read_next_kline(&mut reader)? else {
                break;
            };
            if kline.id >= range.start && kline.id < range.end {
                klines.push(kline);
            }
        }
        Ok(klines)
    }

    pub fn append_klines(&self, symbol: &str, duration: i64, klines: &[Kline]) -> Result<()> {
        if klines.is_empty() {
            return Ok(());
        }

        let _guard = self.lock_writes()?;

        let mut metadata = self
            .read_metadata(symbol, duration)?
            .unwrap_or_else(|| KlineCacheMetadata {
                symbol: symbol.to_string(),
                duration,
                ranges: Vec::new(),
            });

        let mut merged = BTreeMap::<i64, Kline>::new();
        for kline in self.read_all_klines(symbol, duration)? {
            merged.insert(kline.id, kline);
        }
        for kline in klines {
            merged.insert(kline.id, kline.clone());
        }

        let added_ranges: RangeSet = rangeset_merge(
            klines
                .iter()
                .filter_map(|k| k.id.checked_add(1).map(|end| Range::new(k.id, end)))
                .collect(),
        );
        metadata.ranges = rangeset_union(&metadata.ranges, &added_ranges);

        let merged_klines: Vec<Kline> = merged.into_values().collect();
        self.rewrite_file(symbol, duration, &metadata, &merged_klines)?;
        Ok(())
    }

    fn read_metadata_from_reader(reader: &mut BufReader<File>) -> Result<KlineCacheMetadata> {
        // 读取元数据长度
        let mut metadata_len_bytes = [0u8; 8];
        reader.read_exact(&mut metadata_len_bytes)?;
        let metadata_len = u64::from_le_bytes(metadata_len_bytes) as usize;

        // 读取元数据
        let mut metadata_bytes = vec![0u8; metadata_len];
        reader.read_exact(&mut metadata_bytes)?;
        let metadata: KlineCacheMetadata = bincode::deserialize(&metadata_bytes)?;
        Ok(metadata)
    }

    fn skip_metadata(reader: &mut BufReader<File>) -> Result<()> {
        let mut metadata_len_bytes = [0u8; 8];
        reader.read_exact(&mut metadata_len_bytes)?;
        let metadata_len = u64::from_le_bytes(metadata_len_bytes);
        reader.seek(SeekFrom::Current(metadata_len as i64))?;
        Ok(())
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

    fn rewrite_file(&self, symbol: &str, duration: i64, metadata: &KlineCacheMetadata, klines: &[Kline]) -> Result<()> {
        let file_path = self.get_cache_file_path(symbol, duration);
        Self::ensure_parent_dir(&file_path)?;
        let temp_file_path = file_path.with_extension("kline.tmp");

        let mut temp_file = File::create(&temp_file_path)?;
        let mut writer = BufWriter::new(&mut temp_file);

        let metadata_bytes = bincode::serialize(metadata)?;
        let metadata_len = metadata_bytes.len() as u64;

        writer.write_all(&metadata_len.to_le_bytes())?;
        writer.write_all(&metadata_bytes)?;

        for kline in klines {
            let kline_bytes = bincode::serialize(&CachedKline::from(kline))?;
            let kline_len = kline_bytes.len() as u64;
            writer.write_all(&kline_len.to_le_bytes())?;
            writer.write_all(&kline_bytes)?;
        }
        writer.flush()?;
        drop(writer);
        temp_file.sync_all()?;

        std::fs::rename(&temp_file_path, &file_path)?;
        Ok(())
    }

    fn read_all_klines(&self, symbol: &str, duration: i64) -> Result<Vec<Kline>> {
        let file_path = self.get_cache_file_path(symbol, duration);
        if !file_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&file_path)?;
        let mut reader = BufReader::new(file);

        Self::skip_metadata(&mut reader)?;

        let mut klines = Vec::new();
        loop {
            let Some(kline) = Self::read_next_kline(&mut reader)? else {
                break;
            };
            klines.push(kline);
        }
        Ok(klines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        let _ = std::fs::remove_dir_all(dir);
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

        let _ = std::fs::remove_dir_all(dir);
    }
}
