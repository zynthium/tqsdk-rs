use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc::{Sender, error::TrySendError};
use tracing::warn;

type OverflowHandler = Arc<dyn Fn() + Send + Sync>;

pub(crate) fn derive_message_backlog_max(
    message_queue_capacity: usize,
    message_backlog_warn_step: usize,
) -> usize {
    let capacity = message_queue_capacity.max(1);
    let warn_step = message_backlog_warn_step.max(1);
    capacity
        .saturating_mul(4)
        .max(warn_step.saturating_mul(2))
        .max(64)
}

fn try_merge_rtn_data_inplace(base: &mut Value, next: Value) -> std::result::Result<(), Value> {
    let Some(base_obj) = base.as_object_mut() else {
        return Err(next);
    };
    if base_obj.get("aid").and_then(|v| v.as_str()) != Some("rtn_data") {
        return Err(next);
    }

    let Value::Object(next_obj) = next else {
        return Err(next);
    };
    if next_obj.get("aid").and_then(|v| v.as_str()) != Some("rtn_data") {
        return Err(Value::Object(next_obj));
    }

    let Some(Value::Array(base_data)) = base_obj.get_mut("data") else {
        return Err(Value::Object(next_obj));
    };
    let Some(Value::Array(next_data)) = next_obj.get("data") else {
        return Err(Value::Object(next_obj));
    };

    base_data.extend(next_data.iter().cloned());
    Ok(())
}

#[derive(Clone)]
pub(crate) struct BackpressureState {
    sender: Sender<Value>,
    backlog: Arc<Mutex<VecDeque<Value>>>,
    draining: Arc<AtomicBool>,
    dropped_counter: Arc<AtomicUsize>,
    overflow_notified: Arc<AtomicBool>,
    overflow_handler: Arc<RwLock<Option<OverflowHandler>>>,
    backlog_max: usize,
    backlog_warn_step: usize,
    batch_max: usize,
    channel_name: &'static str,
}

impl BackpressureState {
    pub(crate) fn new(
        sender: Sender<Value>,
        backlog_max: usize,
        backlog_warn_step: usize,
        batch_max: usize,
        channel_name: &'static str,
    ) -> Self {
        Self {
            sender,
            backlog: Arc::new(Mutex::new(VecDeque::new())),
            draining: Arc::new(AtomicBool::new(false)),
            dropped_counter: Arc::new(AtomicUsize::new(0)),
            overflow_notified: Arc::new(AtomicBool::new(false)),
            overflow_handler: Arc::new(RwLock::new(None)),
            backlog_max: backlog_max.max(1),
            backlog_warn_step: backlog_warn_step.max(1),
            batch_max: batch_max.max(1),
            channel_name,
        }
    }

    pub(crate) fn set_overflow_handler<F>(&self, handler: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.overflow_handler.write().unwrap() = Some(Arc::new(handler));
    }

    fn notify_overflow_once(&self) {
        if self.overflow_notified.swap(true, Ordering::SeqCst) {
            return;
        }
        let handler = self.overflow_handler.read().unwrap().clone();
        if let Some(handler) = handler {
            handler();
        }
    }

    pub(crate) fn enqueue(&self, data: Value) {
        match self.sender.try_send(data) {
            Ok(()) => {}
            Err(TrySendError::Closed(_)) => {
                let dropped_total = self.dropped_counter.fetch_add(1, Ordering::Relaxed) + 1;
                warn!(
                    "{} 消息处理队列已关闭，丢弃消息: dropped_total={}",
                    self.channel_name, dropped_total
                );
            }
            Err(TrySendError::Full(data)) => {
                let mut queue = self.backlog.lock().unwrap();
                let mut data_opt = Some(data);
                if let Some(tail) = queue.back_mut()
                    && let Some(payload) = data_opt.take()
                {
                    match try_merge_rtn_data_inplace(tail, payload) {
                        Ok(()) => {}
                        Err(unmerged) => {
                            data_opt = Some(unmerged);
                        }
                    }
                }

                if let Some(payload) = data_opt {
                    if queue.len() >= self.backlog_max {
                        let overflow = queue.len() + 1 - self.backlog_max;
                        for _ in 0..overflow {
                            queue.pop_front();
                        }
                        let dropped_total =
                            self.dropped_counter.fetch_add(overflow, Ordering::Relaxed) + overflow;
                        if dropped_total == overflow
                            || dropped_total.is_multiple_of(self.backlog_warn_step)
                        {
                            warn!(
                                "{} 消息 backlog 达到上限，丢弃最旧消息: dropped_now={} dropped_total={} backlog_max={}",
                                self.channel_name, overflow, dropped_total, self.backlog_max
                            );
                        }
                        self.notify_overflow_once();
                    }

                    queue.push_back(payload);
                }

                let backlog_len = queue.len();
                drop(queue);
                if backlog_len == 1 || backlog_len.is_multiple_of(self.backlog_warn_step) {
                    warn!(
                        "{} 消息处理队列积压: backlog={}/{}",
                        self.channel_name, backlog_len, self.backlog_max
                    );
                }

                if !self.draining.swap(true, Ordering::SeqCst) {
                    let state = self.clone();
                    tokio::spawn(async move {
                        state.drain_backlog().await;
                    });
                }
            }
        }
    }

    async fn drain_backlog(self) {
        loop {
            let next = {
                let mut queue = self.backlog.lock().unwrap();
                match queue.pop_front() {
                    Some(mut payload) => {
                        let mut merged = 1usize;
                        while merged < self.batch_max {
                            let Some(next_payload) = queue.pop_front() else {
                                break;
                            };
                            match try_merge_rtn_data_inplace(&mut payload, next_payload) {
                                Ok(()) => {
                                    merged += 1;
                                }
                                Err(unmerged) => {
                                    queue.push_front(unmerged);
                                    break;
                                }
                            }
                        }
                        Some(payload)
                    }
                    None => None,
                }
            };

            match next {
                Some(payload) => {
                    if self.sender.send(payload).await.is_err() {
                        self.draining.store(false, Ordering::SeqCst);
                        let cleared = {
                            let mut queue = self.backlog.lock().unwrap();
                            let count = queue.len();
                            queue.clear();
                            count
                        };
                        if cleared > 0 {
                            let dropped_total =
                                self.dropped_counter.fetch_add(cleared, Ordering::Relaxed)
                                    + cleared;
                            warn!(
                                "{} 消息处理队列已关闭，清理积压消息: cleared={} dropped_total={}",
                                self.channel_name, cleared, dropped_total
                            );
                        } else {
                            warn!("{} 消息处理队列已关闭，清理积压消息", self.channel_name);
                        }
                        break;
                    }
                }
                None => {
                    self.draining.store(false, Ordering::SeqCst);
                    self.overflow_notified.store(false, Ordering::SeqCst);
                    let has_more = !self.backlog.lock().unwrap().is_empty();
                    if has_more && !self.draining.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    #[tokio::test]
    async fn enqueue_backlog_is_bounded_and_drops_oldest() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Value>(1);
        tx.try_send(json!({"aid":"x","seq":0})).unwrap();

        let state = BackpressureState::new(tx, 3, 2, 1, "test");

        for seq in 1..=5 {
            state.enqueue(json!({"aid":"x","seq":seq}));
        }

        let queue = state.backlog.lock().unwrap();
        assert_eq!(queue.len(), 3);
        let seqs: Vec<i64> = queue
            .iter()
            .map(|value| value.get("seq").and_then(|seq| seq.as_i64()).unwrap())
            .collect();
        assert_eq!(seqs, vec![3, 4, 5]);
        drop(queue);

        assert_eq!(state.dropped_counter.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn enqueue_merges_rtn_data_in_backlog() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Value>(1);
        tx.try_send(json!({"aid":"x"})).unwrap();

        let state = BackpressureState::new(tx, 2, 16, 1, "test");

        for i in 0..10 {
            state.enqueue(json!({"aid":"rtn_data","data":[{"i":i}]}));
        }

        let queue = state.backlog.lock().unwrap();
        assert_eq!(queue.len(), 1);
        let data = queue[0].get("data").unwrap().as_array().unwrap();
        assert_eq!(data.len(), 10);
        drop(queue);

        assert_eq!(state.dropped_counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn overflow_triggers_resync_callback_once() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Value>(1);
        tx.try_send(json!({"aid":"x","seq":0})).unwrap();

        let state = BackpressureState::new(tx, 2, 16, 1, "test");
        let overflowed = Arc::new(AtomicUsize::new(0));
        let overflowed_for_cb = Arc::clone(&overflowed);
        state.set_overflow_handler(move || {
            overflowed_for_cb.fetch_add(1, Ordering::Relaxed);
        });

        for seq in 1..=5 {
            state.enqueue(json!({"aid":"x","seq":seq}));
        }

        assert_eq!(overflowed.load(Ordering::Relaxed), 1);
    }
}
