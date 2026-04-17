use crate::errors::{Result, TqError};
use crate::types::{Kline, Quote, Tick};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::watch;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct SymbolId(Arc<str>);

impl SymbolId {
    pub fn new(symbol: impl AsRef<str>) -> Self {
        Self(Arc::from(symbol.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SymbolId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq for SymbolId {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ref() == other.0.as_ref()
    }
}

impl Eq for SymbolId {}

impl Hash for SymbolId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_ref().hash(state);
    }
}

impl From<&str> for SymbolId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for SymbolId {
    fn from(value: String) -> Self {
        Self(Arc::from(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KlineKey {
    pub symbol: SymbolId,
    pub duration_nanos: i64,
}

impl KlineKey {
    pub fn new(symbol: impl Into<SymbolId>, duration: Duration) -> Self {
        Self {
            symbol: symbol.into(),
            duration_nanos: duration_to_nanos(duration),
        }
    }
}

fn duration_to_nanos(duration: Duration) -> i64 {
    (duration.as_secs() as i128 * 1_000_000_000i128 + duration.subsec_nanos() as i128) as i64
}

struct Entry<T> {
    value: Arc<T>,
    epoch: u64,
    tx: watch::Sender<u64>,
}

pub struct MarketDataState {
    global_epoch: AtomicU64,
    global_tx: watch::Sender<u64>,
    close_tx: watch::Sender<bool>,
    updates_tx: broadcast::Sender<UpdateEvent>,
    quotes: RwLock<HashMap<SymbolId, Entry<Quote>>>,
    klines: RwLock<HashMap<KlineKey, Entry<Kline>>>,
    ticks: RwLock<HashMap<SymbolId, Entry<Tick>>>,
}

impl Default for MarketDataState {
    fn default() -> Self {
        let (global_tx, _) = watch::channel(0u64);
        let (close_tx, _) = watch::channel(false);
        let (updates_tx, _) = broadcast::channel(65536);
        Self {
            global_epoch: AtomicU64::new(0),
            global_tx,
            close_tx,
            updates_tx,
            quotes: RwLock::new(HashMap::new()),
            klines: RwLock::new(HashMap::new()),
            ticks: RwLock::new(HashMap::new()),
        }
    }
}

impl MarketDataState {
    pub fn subscribe_global_epoch(&self) -> watch::Receiver<u64> {
        self.global_tx.subscribe()
    }

    pub fn subscribe_close(&self) -> watch::Receiver<bool> {
        self.close_tx.subscribe()
    }

    pub fn close(&self) {
        self.close_tx.send_replace(true);
    }

    pub fn is_closed(&self) -> bool {
        *self.close_tx.borrow()
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<UpdateEvent> {
        self.updates_tx.subscribe()
    }

    pub async fn quote_epoch(&self, symbol: &SymbolId) -> u64 {
        self.quotes
            .read()
            .await
            .get(symbol)
            .map(|entry| entry.epoch)
            .unwrap_or(0)
    }

    pub async fn kline_epoch(&self, key: &KlineKey) -> u64 {
        self.klines.read().await.get(key).map(|entry| entry.epoch).unwrap_or(0)
    }

    pub async fn tick_epoch(&self, symbol: &SymbolId) -> u64 {
        self.ticks
            .read()
            .await
            .get(symbol)
            .map(|entry| entry.epoch)
            .unwrap_or(0)
    }

    pub async fn quote_snapshot(&self, symbol: &SymbolId) -> Option<Arc<Quote>> {
        self.quotes
            .read()
            .await
            .get(symbol)
            .map(|entry| Arc::clone(&entry.value))
    }

    pub async fn kline_snapshot(&self, key: &KlineKey) -> Option<Arc<Kline>> {
        self.klines.read().await.get(key).map(|entry| Arc::clone(&entry.value))
    }

    pub async fn tick_snapshot(&self, symbol: &SymbolId) -> Option<Arc<Tick>> {
        self.ticks
            .read()
            .await
            .get(symbol)
            .map(|entry| Arc::clone(&entry.value))
    }

    pub async fn subscribe_quote_epoch(&self, symbol: &SymbolId) -> watch::Receiver<u64> {
        let mut quotes = self.quotes.write().await;
        if let Some(entry) = quotes.get(symbol) {
            return entry.tx.subscribe();
        }
        let (tx, rx) = watch::channel(0u64);
        quotes.insert(
            symbol.clone(),
            Entry {
                value: Arc::new(Quote::default()),
                epoch: 0,
                tx,
            },
        );
        rx
    }

    pub async fn subscribe_kline_epoch(&self, key: &KlineKey) -> watch::Receiver<u64> {
        let mut klines = self.klines.write().await;
        if let Some(entry) = klines.get(key) {
            return entry.tx.subscribe();
        }
        let (tx, rx) = watch::channel(0u64);
        klines.insert(
            key.clone(),
            Entry {
                value: Arc::new(Kline::default()),
                epoch: 0,
                tx,
            },
        );
        rx
    }

    pub async fn subscribe_tick_epoch(&self, symbol: &SymbolId) -> watch::Receiver<u64> {
        let mut ticks = self.ticks.write().await;
        if let Some(entry) = ticks.get(symbol) {
            return entry.tx.subscribe();
        }
        let (tx, rx) = watch::channel(0u64);
        ticks.insert(
            symbol.clone(),
            Entry {
                value: Arc::new(Tick::default()),
                epoch: 0,
                tx,
            },
        );
        rx
    }

    pub async fn update_quote(&self, symbol: SymbolId, quote: Quote) {
        let epoch = self.update_entry(&self.quotes, symbol.clone(), quote).await;
        let _ = self.updates_tx.send(UpdateEvent {
            epoch,
            key: UpdateKey::Quote(symbol),
        });
        self.global_tx.send_replace(epoch);
    }

    pub async fn update_kline(&self, key: KlineKey, kline: Kline) {
        let epoch = self.update_entry(&self.klines, key.clone(), kline).await;
        let _ = self.updates_tx.send(UpdateEvent {
            epoch,
            key: UpdateKey::Kline(key),
        });
        self.global_tx.send_replace(epoch);
    }

    pub async fn update_tick(&self, symbol: SymbolId, tick: Tick) {
        let epoch = self.update_entry(&self.ticks, symbol.clone(), tick).await;
        let _ = self.updates_tx.send(UpdateEvent {
            epoch,
            key: UpdateKey::Tick(symbol),
        });
        self.global_tx.send_replace(epoch);
    }

    async fn update_entry<K, V>(&self, map: &RwLock<HashMap<K, Entry<V>>>, key: K, value: V) -> u64
    where
        K: Eq + Hash + Clone,
        V: Send + Sync + 'static,
    {
        let global_epoch = self.global_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let mut guard = map.write().await;
        match guard.get_mut(&key) {
            Some(entry) => {
                entry.epoch = global_epoch;
                entry.value = Arc::new(value);
                entry.tx.send_replace(global_epoch);
            }
            None => {
                let (tx, _) = watch::channel(0u64);
                let entry = Entry {
                    value: Arc::new(value),
                    epoch: global_epoch,
                    tx,
                };
                entry.tx.send_replace(global_epoch);
                guard.insert(key, entry);
            }
        }
        global_epoch
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpdateKey {
    Quote(SymbolId),
    Kline(KlineKey),
    Tick(SymbolId),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UpdateEvent {
    pub epoch: u64,
    pub key: UpdateKey,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketDataUpdates {
    pub quotes: Vec<SymbolId>,
    pub klines: Vec<KlineKey>,
    pub ticks: Vec<SymbolId>,
}

#[derive(Clone)]
pub struct TqApi {
    state: Arc<MarketDataState>,
    inner: Arc<TqApiInner>,
}

struct UpdateCollector {
    rx: broadcast::Receiver<UpdateEvent>,
    pending: Option<UpdateEvent>,
}

struct TqApiInner {
    seen_epoch: AtomicU64,
    collector: tokio::sync::Mutex<UpdateCollector>,
}

impl TqApi {
    pub fn new(state: Arc<MarketDataState>) -> Self {
        let rx = state.subscribe_updates();
        Self {
            state,
            inner: Arc::new(TqApiInner {
                seen_epoch: AtomicU64::new(0),
                collector: tokio::sync::Mutex::new(UpdateCollector { rx, pending: None }),
            }),
        }
    }

    pub fn quote(&self, symbol: impl Into<SymbolId>) -> QuoteRef {
        QuoteRef::new(Arc::clone(&self.state), symbol.into())
    }

    pub fn kline(&self, symbol: impl Into<SymbolId>, duration: Duration) -> KlineRef {
        KlineRef::new(Arc::clone(&self.state), KlineKey::new(symbol, duration))
    }

    pub fn tick(&self, symbol: impl Into<SymbolId>) -> TickRef {
        TickRef::new(Arc::clone(&self.state), symbol.into())
    }

    pub async fn wait_update(&self) -> Result<()> {
        let mut rx = self.state.subscribe_global_epoch();
        let mut close_rx = self.state.subscribe_close();
        loop {
            if *close_rx.borrow() {
                return Err(TqError::client_closed("market wait_update"));
            }
            let current = *rx.borrow();
            let seen = self.inner.seen_epoch.load(Ordering::SeqCst);
            if current > seen {
                self.inner.seen_epoch.store(current, Ordering::SeqCst);
                return Ok(());
            }
            tokio::select! {
                changed = rx.changed() => {
                    changed.map_err(|_| TqError::DataNotReady("market data channel closed".to_string()))?;
                }
                changed = close_rx.changed() => {
                    changed.map_err(|_| TqError::client_closed("market wait_update"))?;
                }
            }
        }
    }

    pub async fn drain_updates(&self) -> Result<MarketDataUpdates> {
        let boundary = self.inner.seen_epoch.load(Ordering::SeqCst);
        if boundary == 0 {
            return Ok(MarketDataUpdates::default());
        }

        let mut collector = self.inner.collector.lock().await;
        let mut quotes = HashSet::<SymbolId>::new();
        let mut klines = HashSet::<KlineKey>::new();
        let mut ticks = HashSet::<SymbolId>::new();

        loop {
            let next = if let Some(ev) = collector.pending.take() {
                Some(ev)
            } else {
                match collector.rx.try_recv() {
                    Ok(ev) => Some(ev),
                    Err(broadcast::error::TryRecvError::Empty) => None,
                    Err(broadcast::error::TryRecvError::Closed) => {
                        return Err(TqError::InternalError("market data updates channel closed".to_string()));
                    }
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        return Err(TqError::InternalError("market data updates channel lagged".to_string()));
                    }
                }
            };

            let Some(ev) = next else { break };
            if ev.epoch > boundary {
                collector.pending = Some(ev);
                break;
            }

            match ev.key {
                UpdateKey::Quote(symbol) => {
                    quotes.insert(symbol);
                }
                UpdateKey::Kline(key) => {
                    klines.insert(key);
                }
                UpdateKey::Tick(symbol) => {
                    ticks.insert(symbol);
                }
            }
        }

        let mut updates = MarketDataUpdates {
            quotes: quotes.into_iter().collect(),
            klines: klines.into_iter().collect(),
            ticks: ticks.into_iter().collect(),
        };
        updates.quotes.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        updates.klines.sort_by(|a, b| {
            a.symbol
                .as_str()
                .cmp(b.symbol.as_str())
                .then(a.duration_nanos.cmp(&b.duration_nanos))
        });
        updates.ticks.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(updates)
    }

    pub async fn wait_update_and_drain(&self) -> Result<MarketDataUpdates> {
        self.wait_update().await?;
        self.drain_updates().await
    }
}

pub struct QuoteRef {
    state: Arc<MarketDataState>,
    symbol: SymbolId,
    seen_epoch: AtomicU64,
}

impl QuoteRef {
    fn new(state: Arc<MarketDataState>, symbol: SymbolId) -> Self {
        Self {
            state,
            symbol,
            seen_epoch: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(state: Arc<MarketDataState>, symbol: &str) -> Self {
        Self::new(state, symbol.into())
    }

    pub fn symbol(&self) -> &str {
        self.symbol.as_str()
    }

    pub async fn snapshot(&self) -> Option<Arc<Quote>> {
        self.state.quote_snapshot(&self.symbol).await
    }

    pub async fn is_ready(&self) -> bool {
        self.snapshot().await.is_some()
    }

    pub async fn try_load(&self) -> Result<Arc<Quote>> {
        self.snapshot()
            .await
            .ok_or_else(|| TqError::DataNotReady(format!("quote {}", self.symbol())))
    }

    /// Legacy convenience API.
    /// 在首帧到达前会返回默认值；新代码优先使用 `snapshot()` / `is_ready()` / `try_load()`
    pub async fn load(&self) -> Arc<Quote> {
        self.snapshot().await.unwrap_or_else(|| Arc::new(Quote::default()))
    }

    pub async fn is_changing(&self) -> bool {
        let current = self.state.quote_epoch(&self.symbol).await;
        let seen = self.seen_epoch.load(Ordering::SeqCst);
        if current > seen {
            self.seen_epoch.store(current, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub async fn wait_update(&self) -> Result<()> {
        let mut rx = self.state.subscribe_quote_epoch(&self.symbol).await;
        let mut close_rx = self.state.subscribe_close();
        loop {
            if *close_rx.borrow() {
                return Err(TqError::client_closed("quote wait_update"));
            }
            let current = *rx.borrow();
            let seen = self.seen_epoch.load(Ordering::SeqCst);
            if current > seen {
                self.seen_epoch.store(current, Ordering::SeqCst);
                return Ok(());
            }
            tokio::select! {
                changed = rx.changed() => {
                    changed.map_err(|_| TqError::DataNotReady("quote channel closed".to_string()))?;
                }
                changed = close_rx.changed() => {
                    changed.map_err(|_| TqError::client_closed("quote wait_update"))?;
                }
            }
        }
    }
}

pub struct KlineRef {
    state: Arc<MarketDataState>,
    key: KlineKey,
    seen_epoch: AtomicU64,
}

impl KlineRef {
    fn new(state: Arc<MarketDataState>, key: KlineKey) -> Self {
        Self {
            state,
            key,
            seen_epoch: AtomicU64::new(0),
        }
    }

    pub fn symbol(&self) -> &str {
        self.key.symbol.as_str()
    }

    pub fn duration_nanos(&self) -> i64 {
        self.key.duration_nanos
    }

    pub async fn snapshot(&self) -> Option<Arc<Kline>> {
        self.state.kline_snapshot(&self.key).await
    }

    pub async fn is_ready(&self) -> bool {
        self.snapshot().await.is_some()
    }

    pub async fn try_load(&self) -> Result<Arc<Kline>> {
        self.snapshot()
            .await
            .ok_or_else(|| TqError::DataNotReady(format!("kline {}@{}", self.symbol(), self.duration_nanos())))
    }

    /// Legacy convenience API.
    /// 在首帧到达前会返回默认值；新代码优先使用 `snapshot()` / `is_ready()` / `try_load()`
    pub async fn load(&self) -> Arc<Kline> {
        self.snapshot().await.unwrap_or_else(|| Arc::new(Kline::default()))
    }

    pub async fn is_changing(&self) -> bool {
        let current = self.state.kline_epoch(&self.key).await;
        let seen = self.seen_epoch.load(Ordering::SeqCst);
        if current > seen {
            self.seen_epoch.store(current, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub async fn wait_update(&self) -> Result<()> {
        let mut rx = self.state.subscribe_kline_epoch(&self.key).await;
        let mut close_rx = self.state.subscribe_close();
        loop {
            if *close_rx.borrow() {
                return Err(TqError::client_closed("kline wait_update"));
            }
            let current = *rx.borrow();
            let seen = self.seen_epoch.load(Ordering::SeqCst);
            if current > seen {
                self.seen_epoch.store(current, Ordering::SeqCst);
                return Ok(());
            }
            tokio::select! {
                changed = rx.changed() => {
                    changed.map_err(|_| TqError::DataNotReady("kline channel closed".to_string()))?;
                }
                changed = close_rx.changed() => {
                    changed.map_err(|_| TqError::client_closed("kline wait_update"))?;
                }
            }
        }
    }
}

pub struct TickRef {
    state: Arc<MarketDataState>,
    symbol: SymbolId,
    seen_epoch: AtomicU64,
}

impl TickRef {
    fn new(state: Arc<MarketDataState>, symbol: SymbolId) -> Self {
        Self {
            state,
            symbol,
            seen_epoch: AtomicU64::new(0),
        }
    }

    pub fn symbol(&self) -> &str {
        self.symbol.as_str()
    }

    pub async fn snapshot(&self) -> Option<Arc<Tick>> {
        self.state.tick_snapshot(&self.symbol).await
    }

    pub async fn is_ready(&self) -> bool {
        self.snapshot().await.is_some()
    }

    pub async fn try_load(&self) -> Result<Arc<Tick>> {
        self.snapshot()
            .await
            .ok_or_else(|| TqError::DataNotReady(format!("tick {}", self.symbol())))
    }

    /// Legacy convenience API.
    /// 在首帧到达前会返回默认值；新代码优先使用 `snapshot()` / `is_ready()` / `try_load()`
    pub async fn load(&self) -> Arc<Tick> {
        self.snapshot().await.unwrap_or_else(|| Arc::new(Tick::default()))
    }

    pub async fn is_changing(&self) -> bool {
        let current = self.state.tick_epoch(&self.symbol).await;
        let seen = self.seen_epoch.load(Ordering::SeqCst);
        if current > seen {
            self.seen_epoch.store(current, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub async fn wait_update(&self) -> Result<()> {
        let mut rx = self.state.subscribe_tick_epoch(&self.symbol).await;
        let mut close_rx = self.state.subscribe_close();
        loop {
            if *close_rx.borrow() {
                return Err(TqError::client_closed("tick wait_update"));
            }
            let current = *rx.borrow();
            let seen = self.seen_epoch.load(Ordering::SeqCst);
            if current > seen {
                self.seen_epoch.store(current, Ordering::SeqCst);
                return Ok(());
            }
            tokio::select! {
                changed = rx.changed() => {
                    changed.map_err(|_| TqError::DataNotReady("tick channel closed".to_string()))?;
                }
                changed = close_rx.changed() => {
                    changed.map_err(|_| TqError::client_closed("tick wait_update"))?;
                }
            }
        }
    }
}
