use std::sync::Arc;

use crate::datamanager::DataManager;

pub trait MarketAdapter: Send + Sync {
    fn dm(&self) -> Arc<DataManager>;
}

pub struct LiveMarketAdapter {
    dm: Arc<DataManager>,
}

impl LiveMarketAdapter {
    pub fn new(dm: Arc<DataManager>) -> Self {
        Self { dm }
    }
}

impl MarketAdapter for LiveMarketAdapter {
    fn dm(&self) -> Arc<DataManager> {
        Arc::clone(&self.dm)
    }
}
