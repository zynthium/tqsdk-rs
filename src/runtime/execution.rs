use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::trade_session::TradeSession;

pub trait ExecutionAdapter: Send + Sync {
    fn known_accounts(&self) -> Vec<String>;
}

#[derive(Default)]
pub struct LiveExecutionAdapter {
    trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>,
}

impl LiveExecutionAdapter {
    pub fn new(trade_sessions: Arc<RwLock<HashMap<String, Arc<TradeSession>>>>) -> Self {
        Self { trade_sessions }
    }
}

impl ExecutionAdapter for LiveExecutionAdapter {
    fn known_accounts(&self) -> Vec<String> {
        match self.trade_sessions.try_read() {
            Ok(sessions) => sessions.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}
