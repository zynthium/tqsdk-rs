use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime feature is not available yet: {0}")]
    Unsupported(&'static str),
    #[error("runtime account is not registered: {account_key}")]
    AccountNotFound { account_key: String },
    #[error("conflicting target task already exists for runtime={runtime_id}, account={account_key}, symbol={symbol}")]
    TaskConflict {
        runtime_id: String,
        account_key: String,
        symbol: String,
    },
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;
