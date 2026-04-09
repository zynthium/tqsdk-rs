use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::datamanager::DataManager;
use crate::types::Quote;

use super::{RuntimeError, RuntimeResult};

/// Runtime market adapters are no longer derived from `DataManager`.
/// Implementations must provide the async market access methods directly.
#[async_trait]
pub trait MarketAdapter: Send + Sync {
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote>;

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()>;

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>>;
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
    async fn latest_quote(&self, symbol: &str) -> RuntimeResult<Quote> {
        Ok(self.dm.get_quote_data(symbol)?)
    }

    async fn wait_quote_update(&self, symbol: &str) -> RuntimeResult<()> {
        let rx = self.dm.watch(vec!["quotes".to_string(), symbol.to_string()]);
        rx.recv()
            .await
            .map(|_| ())
            .map_err(|_| RuntimeError::AdapterChannelClosed {
                resource: "market quote updates",
            })
    }

    async fn trading_time(&self, symbol: &str) -> RuntimeResult<Option<Value>> {
        Ok(self.dm.get_by_path(&["quotes", symbol, "trading_time"]))
    }
}
