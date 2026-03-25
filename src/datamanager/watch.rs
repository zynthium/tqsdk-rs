use super::{DataManager, PathWatcher};
use crate::errors::{Result, TqError};
use async_channel::{Receiver, TrySendError, unbounded};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

impl DataManager {
    /// 监听指定路径的数据变化
    ///
    /// 返回一个 receiver，数据变化时会推送到这个 channel
    pub fn watch(&self, path: Vec<String>) -> Receiver<Value> {
        self.watch_register(path).1
    }

    pub(crate) fn watch_register(&self, path: Vec<String>) -> (i64, Receiver<Value>) {
        let path_key = path.join(".");
        let (tx, rx) = unbounded();
        let watcher_id = self.next_watcher_id.fetch_add(1, Ordering::SeqCst);

        let watcher = PathWatcher {
            id: watcher_id,
            path: path.clone(),
            tx,
        };

        let mut watchers = self.watchers.write().unwrap();
        watchers.entry(path_key).or_default().push(watcher);

        (watcher_id, rx)
    }

    /// 取消路径监听
    pub fn unwatch(&self, path: &[String]) -> Result<()> {
        let path_key = path.join(".");
        let mut watchers = self.watchers.write().unwrap();

        if watchers.remove(&path_key).is_none() {
            return Err(TqError::DataNotFound(format!("路径未监听: {}", path_key)));
        }

        Ok(())
    }

    pub(crate) fn unwatch_by_id(&self, path: &[String], watcher_id: i64) -> bool {
        let path_key = path.join(".");
        let mut watchers = self.watchers.write().unwrap();
        let Some(entries) = watchers.get_mut(&path_key) else {
            return false;
        };

        let before = entries.len();
        entries.retain(|watcher| watcher.id != watcher_id);
        let removed = entries.len() < before;
        if entries.is_empty() {
            watchers.remove(&path_key);
        }

        removed
    }

    /// 通知所有 watchers
    pub(crate) fn notify_watchers(&self) {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        let data = self.data.read().unwrap();
        let mut watchers = self.watchers.write().unwrap();
        watchers.retain(|_, entries| {
            entries.retain(|watcher| {
                if !is_path_epoch_changed(&data, &watcher.path, current_epoch) {
                    return true;
                }
                let Some(payload) = get_by_path_ref_strings(&data, &watcher.path).cloned() else {
                    return true;
                };
                match watcher.tx.try_send(payload) {
                    Ok(()) => true,
                    Err(TrySendError::Closed(_)) => false,
                    Err(TrySendError::Full(_)) => true,
                }
            });
            !entries.is_empty()
        });
    }
}

fn get_by_path_ref_strings<'a>(
    data: &'a HashMap<String, Value>,
    path: &[String],
) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = data.get(&path[0])?;
    for key in &path[1..] {
        match current {
            Value::Object(map) => {
                current = map.get(key)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn is_path_epoch_changed(
    data: &HashMap<String, Value>,
    path: &[String],
    current_epoch: i64,
) -> bool {
    if let Some(Value::Object(map)) = get_by_path_ref_strings(data, path)
        && let Some(Value::Number(epoch_val)) = map.get("_epoch")
        && let Some(epoch) = epoch_val.as_i64()
    {
        return epoch == current_epoch;
    }
    false
}
