use crate::types::{Notification, Order, Trade};
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
    NotificationReceived { notification: Notification },
    TransportError { message: String },
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
struct JournalState {
    events: VecDeque<Arc<TradeSessionEvent>>,
    subscribers: HashMap<u64, SubscriberState>,
}

#[derive(Debug)]
struct PublishState {
    last_emitted_orders: HashMap<String, Order>,
    seen_trade_ids: HashSet<String>,
}

#[derive(Debug)]
struct EventJournal {
    next_subscriber_id: AtomicU64,
    max_retained_events: usize,
    closed: AtomicBool,
    notify: Notify,
    state: Mutex<JournalState>,
}

impl EventJournal {
    fn new(max_retained_events: usize) -> Self {
        Self {
            next_subscriber_id: AtomicU64::new(1),
            max_retained_events: max_retained_events.max(1),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
            state: Mutex::new(JournalState {
                events: VecDeque::new(),
                subscribers: HashMap::new(),
            }),
        }
    }

    fn subscribe_tail(self: &Arc<Self>) -> TradeEventStream {
        let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let last_seen_seq = self
            .state
            .lock()
            .unwrap()
            .events
            .back()
            .map(|event| event.seq)
            .unwrap_or(0);
        self.state.lock().unwrap().subscribers.insert(
            subscriber_id,
            SubscriberState {
                last_seen_seq,
                invalidated_before_seq: None,
            },
        );
        TradeEventStream {
            journal: Arc::clone(self),
            subscriber_id,
        }
    }

    fn publish(&self, event: Arc<TradeSessionEvent>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }

        let mut state = self.state.lock().unwrap();
        state.events.push_back(event);
        self.trim_locked(&mut state);
        drop(state);
        self.notify.notify_waiters();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn trim_locked(&self, state: &mut JournalState) {
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

    fn trim_consumed_locked(&self, state: &mut JournalState) {
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
    fn max_retained_events_for_test(&self) -> usize {
        self.max_retained_events
    }
}

#[derive(Debug)]
pub(crate) struct TradeEventHub {
    next_seq: AtomicU64,
    closed: AtomicBool,
    all_events: Arc<EventJournal>,
    order_events: Arc<EventJournal>,
    trade_events: Arc<EventJournal>,
    publish_state: Mutex<PublishState>,
}

impl TradeEventHub {
    pub(crate) fn new(max_retained_events: usize) -> Self {
        Self {
            next_seq: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            all_events: Arc::new(EventJournal::new(max_retained_events)),
            order_events: Arc::new(EventJournal::new(max_retained_events)),
            trade_events: Arc::new(EventJournal::new(max_retained_events)),
            publish_state: Mutex::new(PublishState {
                last_emitted_orders: HashMap::new(),
                seen_trade_ids: HashSet::new(),
            }),
        }
    }

    pub(crate) fn subscribe_tail(self: &Arc<Self>) -> TradeEventStream {
        self.all_events.subscribe_tail()
    }

    pub(crate) fn subscribe_orders(self: &Arc<Self>) -> OrderEventStream {
        OrderEventStream {
            inner: self.order_events.subscribe_tail(),
        }
    }

    pub(crate) fn subscribe_trades(self: &Arc<Self>) -> TradeOnlyEventStream {
        TradeOnlyEventStream {
            inner: self.trade_events.subscribe_tail(),
        }
    }

    pub(crate) fn publish_order(&self, order_id: String, order: Order) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let mut publish_state = self.publish_state.lock().unwrap();
        if publish_state
            .last_emitted_orders
            .get(&order_id)
            .is_some_and(|previous| previous == &order)
        {
            return None;
        }
        publish_state
            .last_emitted_orders
            .insert(order_id.clone(), order.clone());
        drop(publish_state);

        let event = self.next_event(TradeSessionEventKind::OrderUpdated { order_id, order });
        let seq = event.seq;
        self.all_events.publish(Arc::clone(&event));
        self.order_events.publish(event);
        Some(seq)
    }

    pub(crate) fn publish_trade(&self, trade_id: String, trade: Trade) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let mut publish_state = self.publish_state.lock().unwrap();
        if !publish_state.seen_trade_ids.insert(trade_id.clone()) {
            return None;
        }
        drop(publish_state);

        let event = self.next_event(TradeSessionEventKind::TradeCreated { trade_id, trade });
        let seq = event.seq;
        self.all_events.publish(Arc::clone(&event));
        self.trade_events.publish(event);
        Some(seq)
    }

    pub(crate) fn publish_notification(&self, notification: Notification) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let event = self.next_event(TradeSessionEventKind::NotificationReceived { notification });
        let seq = event.seq;
        self.all_events.publish(event);
        Some(seq)
    }

    pub(crate) fn publish_transport_error(&self, message: String) -> Option<u64> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }

        let event = self.next_event(TradeSessionEventKind::TransportError { message });
        let seq = event.seq;
        self.all_events.publish(event);
        Some(seq)
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.all_events.close();
        self.order_events.close();
        self.trade_events.close();
    }

    fn next_event(&self, kind: TradeSessionEventKind) -> Arc<TradeSessionEvent> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        Arc::new(TradeSessionEvent { seq, kind })
    }

    #[cfg(test)]
    pub(crate) fn max_retained_events_for_test(&self) -> usize {
        self.all_events.max_retained_events_for_test()
    }
}

#[derive(Debug)]
pub struct TradeEventStream {
    journal: Arc<EventJournal>,
    subscriber_id: u64,
}

impl TradeEventStream {
    pub async fn recv(&mut self) -> Result<TradeSessionEvent, TradeEventRecvError> {
        loop {
            let notified = self.journal.notify.notified();
            {
                let mut state = self.journal.state.lock().unwrap();
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

                if self.journal.closed.load(Ordering::SeqCst) {
                    return Err(TradeEventRecvError::Closed);
                }
            }
            notified.await;
        }
    }
}

impl Drop for TradeEventStream {
    fn drop(&mut self) {
        if let Ok(mut state) = self.journal.state.lock() {
            state.subscribers.remove(&self.subscriber_id);
            self.journal.trim_consumed_locked(&mut state);
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

    fn notification(content: &str) -> Notification {
        Notification {
            code: "2019112901".to_string(),
            level: "INFO".to_string(),
            r#type: "MESSAGE".to_string(),
            content: content.to_string(),
            bid: "simnow".to_string(),
            user_id: "u".to_string(),
        }
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

    #[tokio::test]
    async fn order_stream_does_not_lag_due_to_unrelated_notification_or_error_events() {
        let hub = Arc::new(TradeEventHub::new(2));
        let mut stream = hub.subscribe_orders();

        hub.publish_notification(notification("connected"));
        hub.publish_transport_error("transport failed".to_string());
        hub.publish_order("o1".to_string(), order("o1", "ALIVE"));

        let event = stream.recv().await.unwrap();
        assert!(matches!(
            event.kind,
            TradeSessionEventKind::OrderUpdated { ref order_id, .. } if order_id == "o1"
        ));
    }

    #[tokio::test]
    async fn trade_stream_does_not_lag_due_to_unrelated_notification_or_error_events() {
        let hub = Arc::new(TradeEventHub::new(2));
        let mut stream = hub.subscribe_trades();

        hub.publish_notification(notification("connected"));
        hub.publish_transport_error("transport failed".to_string());
        hub.publish_trade("t1".to_string(), trade("o1", "t1"));

        let event = stream.recv().await.unwrap();
        assert!(matches!(
            event.kind,
            TradeSessionEventKind::TradeCreated { ref trade_id, .. } if trade_id == "t1"
        ));
    }
}
