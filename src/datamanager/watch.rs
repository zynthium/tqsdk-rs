use super::{DataManager, PathWatcher, WatchRegistration};
use crate::errors::{Result, TqError};
use async_channel::{Receiver, TrySendError, bounded};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::warn;

// Public path watchers are best-effort; after repeated full-buffer sends, stop
// spending merge-time work on a consumer that is no longer draining updates.
const MAX_CONSECUTIVE_FULL_WATCHER_SENDS: usize = 16;

impl DataManager {
    /// 监听指定路径的数据变化
    ///
    /// 返回一个 receiver，数据变化时会推送到这个 channel
    pub fn watch(&self, path: Vec<String>) -> Receiver<Value> {
        self.watch_register(path).into_receiver()
    }

    pub(crate) fn watch_register(&self, path: Vec<String>) -> WatchRegistration {
        let path_key = path.join(".");
        let (tx, rx) = bounded(self.config.watch_channel_capacity.max(1));
        let watcher_id = self.next_watcher_id.fetch_add(1, Ordering::SeqCst);

        let watcher = PathWatcher {
            id: watcher_id,
            path: path.clone(),
            tx,
            consecutive_fulls: 0,
        };

        let mut watchers = self.watchers.write().unwrap();
        watchers.entry(path_key).or_default().push(watcher);

        WatchRegistration {
            path,
            watcher_id,
            rx,
            watchers: Some(std::sync::Arc::downgrade(&self.watchers)),
        }
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

    /// 通知所有 watchers
    pub(crate) fn notify_watchers(&self) {
        let current_epoch = self.epoch.load(Ordering::SeqCst);
        let data = self.data.read().unwrap();
        let mut watchers = self.watchers.write().unwrap();
        watchers.retain(|_, entries| {
            entries.retain_mut(|watcher| {
                if !is_path_epoch_changed(&data, &watcher.path, current_epoch) {
                    return true;
                }
                let Some(payload) = get_by_path_ref_strings(&data, &watcher.path).cloned() else {
                    return true;
                };
                match watcher.tx.try_send(payload) {
                    Ok(()) => {
                        watcher.consecutive_fulls = 0;
                        true
                    }
                    Err(TrySendError::Closed(_)) => false,
                    Err(TrySendError::Full(_)) => {
                        watcher.consecutive_fulls += 1;
                        if watcher.consecutive_fulls >= MAX_CONSECUTIVE_FULL_WATCHER_SENDS {
                            warn!(
                                watcher_id = watcher.id,
                                path = %watcher.path.join("."),
                                full_attempts = watcher.consecutive_fulls,
                                "dropping persistently full datamanager watcher"
                            );
                            false
                        } else {
                            true
                        }
                    }
                }
            });
            !entries.is_empty()
        });
    }
}

impl WatchRegistration {
    pub(crate) fn receiver(&self) -> &Receiver<Value> {
        &self.rx
    }

    pub(crate) fn cancel(&mut self) -> bool {
        let Some(watchers) = self.watchers.take() else {
            return false;
        };
        let Some(watchers) = watchers.upgrade() else {
            return false;
        };
        remove_watcher(&watchers, &self.path, self.watcher_id)
    }

    fn into_receiver(mut self) -> Receiver<Value> {
        self.watchers = None;
        self.rx.clone()
    }
}

impl Drop for WatchRegistration {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

fn remove_watcher(
    watchers: &std::sync::RwLock<HashMap<String, Vec<PathWatcher>>>,
    path: &[String],
    watcher_id: i64,
) -> bool {
    let path_key = path.join(".");
    let mut watchers = watchers.write().unwrap();
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

fn get_by_path_ref_strings<'a>(data: &'a HashMap<String, Value>, path: &[String]) -> Option<&'a Value> {
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

fn is_path_epoch_changed(data: &HashMap<String, Value>, path: &[String], current_epoch: i64) -> bool {
    if let Some(Value::Object(map)) = get_by_path_ref_strings(data, path)
        && let Some(Value::Number(epoch_val)) = map.get("_epoch")
        && let Some(epoch) = epoch_val.as_i64()
    {
        return epoch == current_epoch;
    }
    false
}
