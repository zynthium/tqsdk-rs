use crate::types::{Order, Trade};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq)]
pub struct TradeSessionEvent {
    pub seq: u64,
    pub kind: TradeSessionEventKind,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TradeSessionEventKind {
    OrderUpdated { order_id: String, order: Order },
    TradeCreated { trade_id: String, trade: Trade },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeEventRecvError {
    Closed,
    Lagged { missed_before_seq: u64 },
}

#[derive(Debug)]
struct SubscriberState {
    last_seen_seq: u64,
    invalidated_before_seq: Option<u64>,
}

#[derive(Debug)]
struct HubState {
    events: VecDeque<Arc<TradeSessionEvent>>,
    subscribers: HashMap<u64, SubscriberState>,
    last_emitted_orders: HashMap<String, Order>,
    seen_trade_ids: HashSet<String>,
}

#[derive(Debug)]
pub(crate) struct TradeEventHub {
    next_seq: AtomicU64,
    next_subscriber_id: AtomicU64,
    max_retained_events: usize,
    closed: AtomicBool,
    notify: Notify,
    state: Mutex<HubState>,
}

impl TradeEventHub {
    pub(crate) fn new(max_retained_events: usize) -> Self {
        Self {
            next_seq: AtomicU64::new(0),
            next_subscriber_id: AtomicU64::new(1),
            max_retained_events: max_retained_events.max(1),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
            state: Mutex::new(HubState {
                events: VecDeque::new(),
                subscribers: HashMap::new(),
                last_emitted_orders: HashMap::new(),
                seen_trade_ids: HashSet::new(),
            }),
        }
    }

    pub(crate) fn subscribe_tail(self: &Arc<Self>) -> TradeEventStream {
        let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let last_seen_seq = self.next_seq.load(Ordering::SeqCst);
        self.state.lock().unwrap().subscribers.insert(
            subscriber_id,
            SubscriberState {
                last_seen_seq,
                invalidated_before_seq: None,
            },
        );
        TradeEventStream {
            hub: Arc::clone(self),
            subscriber_id,
        }
    }

    pub(crate) fn subscribe_orders(self: &Arc<Self>) -> OrderEventStream {
        OrderEventStream {
            inner: self.subscribe_tail(),
        }
    }

    pub(crate) fn subscribe_trades(self: &Arc<Self>) -> TradeOnlyEventStream {
        TradeOnlyEventStream {
            inner: self.subscribe_tail(),
        }
    }

    pub(crate) fn publish_order(&self, order_id: String, order: Order) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let mut state = self.state.lock().unwrap();
        if state
            .last_emitted_orders
            .get(&order_id)
            .is_some_and(|previous| previous == &order)
        {
            return None;
        }

        state.last_emitted_orders.insert(order_id.clone(), order.clone());
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        state.events.push_back(Arc::new(TradeSessionEvent {
            seq,
            kind: TradeSessionEventKind::OrderUpdated { order_id, order },
        }));
        self.trim_locked(&mut state);
        drop(state);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) fn publish_trade(&self, trade_id: String, trade: Trade) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let mut state = self.state.lock().unwrap();
        if !state.seen_trade_ids.insert(trade_id.clone()) {
            return None;
        }

        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        state.events.push_back(Arc::new(TradeSessionEvent {
            seq,
            kind: TradeSessionEventKind::TradeCreated { trade_id, trade },
        }));
        self.trim_locked(&mut state);
        drop(state);
        self.notify.notify_waiters();
        Some(seq)
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn trim_locked(&self, state: &mut HubState) {
        self.trim_consumed_locked(state);

        while state.events.len() > self.max_retained_events {
            let Some(event) = state.events.pop_front() else {
                break;
            };
            for subscriber in state.subscribers.values_mut() {
                if subscriber.invalidated_before_seq.is_none() && subscriber.last_seen_seq < event.seq {
                    subscriber.invalidated_before_seq = Some(event.seq);
                }
            }
        }

        self.trim_consumed_locked(state);
    }

    fn trim_consumed_locked(&self, state: &mut HubState) {
        let min_retained_seq = state
            .subscribers
            .values()
            .filter(|subscriber| subscriber.invalidated_before_seq.is_none())
            .map(|subscriber| subscriber.last_seen_seq)
            .min();

        match min_retained_seq {
            Some(min_seen_seq) => {
                while state.events.front().is_some_and(|event| event.seq <= min_seen_seq) {
                    state.events.pop_front();
                }
            }
            None => state.events.clear(),
        }
    }

    #[cfg(test)]
    pub(crate) fn max_retained_events_for_test(&self) -> usize {
        self.max_retained_events
    }
}

#[derive(Debug)]
pub struct TradeEventStream {
    hub: Arc<TradeEventHub>,
    subscriber_id: u64,
}

impl TradeEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError> {
        loop {
            let notified = self.hub.notify.notified();
            {
                let mut state = self.hub.state.lock().unwrap();
                let Some((last_seen_seq, invalidated_before_seq)) = state
                    .subscribers
                    .get(&self.subscriber_id)
                    .map(|subscriber| (subscriber.last_seen_seq, subscriber.invalidated_before_seq))
                else {
                    return Err(TradeEventRecvError::Closed);
                };

                if let Some(missed_before_seq) = invalidated_before_seq {
                    return Err(TradeEventRecvError::Lagged { missed_before_seq });
                }

                if let Some(event) = state.events.iter().find(|event| event.seq > last_seen_seq).cloned() {
                    if let Some(subscriber) = state.subscribers.get_mut(&self.subscriber_id) {
                        subscriber.last_seen_seq = event.seq;
                    }
                    return Ok((*event).clone());
                }

                if self.hub.closed.load(Ordering::SeqCst) {
                    return Err(TradeEventRecvError::Closed);
                }
            }
            notified.await;
        }
    }
}

impl Drop for TradeEventStream {
    fn drop(&mut self) {
        if let Ok(mut state) = self.hub.state.lock() {
            state.subscribers.remove(&self.subscriber_id);
            self.hub.trim_consumed_locked(&mut state);
        }
    }
}

#[derive(Debug)]
pub struct OrderEventStream {
    inner: TradeEventStream,
}

impl OrderEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError> {
        loop {
            let event = self.inner.recv().await?;
            if matches!(event.kind, TradeSessionEventKind::OrderUpdated { .. }) {
                return Ok(event);
            }
        }
    }
}

#[derive(Debug)]
pub struct TradeOnlyEventStream {
    inner: TradeEventStream,
}

impl TradeOnlyEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError> {
        loop {
            let event = self.inner.recv().await?;
            if matches!(event.kind, TradeSessionEventKind::TradeCreated { .. }) {
                return Ok(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    fn order(order_id: &str, status: &str) -> Order {
        serde_json::from_value(json!({
            "order_id": order_id,
            "status": status,
            "direction": "BUY",
            "offset": "OPEN",
            "volume_orign": 1,
            "volume_left": if status == "FINISHED" { 0 } else { 1 },
            "price_type": "LIMIT",
            "limit_price": 520.0
        }))
        .unwrap()
    }

    fn trade(order_id: &str, trade_id: &str) -> Trade {
        serde_json::from_value(json!({
            "order_id": order_id,
            "trade_id": trade_id,
            "direction": "BUY",
            "offset": "OPEN",
            "volume": 1,
            "price": 520.0
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn trade_event_hub_subscriber_starts_at_tail() {
        let hub = Arc::new(TradeEventHub::new(8));
        hub.publish_order("o0".to_string(), order("o0", "ALIVE"));

        let mut stream = hub.subscribe_tail();

        hub.publish_order("o1".to_string(), order("o1", "ALIVE"));
        let event = timeout(Duration::from_millis(100), stream.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(event.seq, 2);
        assert!(matches!(
            event.kind,
            TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
        ));
    }

    #[tokio::test]
    async fn trade_event_hub_marks_lagged_subscriber_when_retention_is_exceeded() {
        let hub = Arc::new(TradeEventHub::new(2));
        let mut stream = hub.subscribe_tail();

        hub.publish_order("o1".to_string(), order("o1", "ALIVE"));
        hub.publish_trade("t1".to_string(), trade("o1", "t1"));
        hub.publish_order("o1".to_string(), order("o1", "FINISHED"));

        let err = stream.recv().await.unwrap_err();
        assert!(matches!(err, TradeEventRecvError::Lagged { missed_before_seq: 1 }));
    }

    #[tokio::test]
    async fn trade_event_hub_close_wakes_waiters() {
        let hub = Arc::new(TradeEventHub::new(8));
        let mut stream = hub.subscribe_tail();

        let waiter = tokio::spawn(async move { stream.recv().await });
        hub.close();

        let result = timeout(Duration::from_millis(100), waiter).await.unwrap().unwrap();
        assert!(matches!(result, Err(TradeEventRecvError::Closed)));
    }
}
