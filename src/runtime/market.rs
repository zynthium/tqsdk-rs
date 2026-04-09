use async_trait::async_trait;
use std::sync::Arc;

use crate::datamanager::DataManager;
use crate::types::Quote;

use super::{RuntimeError, RuntimeResult};

#[async_trait]
pub trait MarketAdapter: Send + Sync {
    fn dm(&self) -> Arc<DataManager>;

    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        Ok(self.dm().get_quote_data(symbol)?)
    }

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()> {
        let rx = self.dm().watch(vec!["quotes".to_string(), symbol.to_string()]);
        rx.recv()
            .await
            .map(|_| ())
            .map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "market quote updates",
            })
    }
}

pub struct LiveMarketAdapter {
    dm: Arc<DataManager>,
}

impl LiveMarketAdapter {
    pub fn new(dm: Arc<DataManager>) -> Self {
        Self { dm }
    }
}

#[async_trait]
impl MarketAdapter for LiveMarketAdapter {
    fn dm(&self) -> Arc<DataManager> {
        Arc::clone(&self.dm)
    }
}
