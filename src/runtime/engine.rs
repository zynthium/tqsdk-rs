use crate::types::InsertOrderRequest;

use super::{RuntimeError, RuntimeResult, TqRuntime};

#[derive(Debug, Default)]
pub struct ExecutionEngine;

impl ExecutionEngine {
    pub async fn insert_manual_order(
        &self,
        runtime: &TqRuntime,
        account_key: &str,
        req: &InsertOrderRequest,
    ) -> RuntimeResult<String> {
        let symbol = req.symbol.clone();
        if runtime
            .registry()
            .task_for_symbol(runtime.id(), account_key, &symbol)
            .is_some()
        {
            return Err(RuntimeError::ManualOrderConflict {
                account_key: account_key.to_string(),
                symbol,
            });
        }

        runtime.execution().insert_order(account_key, req).await
    }
}
