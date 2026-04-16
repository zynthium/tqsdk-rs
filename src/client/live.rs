use crate::datamanager::{DataManager, DataManagerConfig};
use crate::ins::InsAPI;
use crate::marketdata::{MarketDataState, TqApi};
use crate::series::SeriesAPI;
use crate::websocket::TqQuoteWebsocket;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct LiveContext {
    pub(crate) dm: Arc<DataManager>,
    pub(crate) market_state: Arc<MarketDataState>,
    pub(crate) live_api: TqApi,
    pub(crate) quotes_ws: Option<Arc<TqQuoteWebsocket>>,
    pub(crate) series_api: Option<Arc<SeriesAPI>>,
    pub(crate) ins_api: Option<Arc<InsAPI>>,
    active: AtomicBool,
}

impl LiveContext {
    pub(crate) fn new(dm_config: DataManagerConfig) -> Self {
        let dm = Arc::new(DataManager::new(HashMap::new(), dm_config));
        let market_state = Arc::new(MarketDataState::default());
        let live_api = TqApi::new(Arc::clone(&market_state));
        Self {
            dm,
            market_state,
            live_api,
            quotes_ws: None,
            series_api: None,
            ins_api: None,
            active: AtomicBool::new(false),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub(crate) fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
    }
}
