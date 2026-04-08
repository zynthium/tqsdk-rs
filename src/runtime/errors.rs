use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime feature is not available yet: {0}")]
    Unsupported(&'static str),
    #[error("runtime account is not registered: {account_key}")]
    AccountNotFound { account_key: String },
    #[error("offset priority is invalid: {raw}")]
    InvalidOffsetPriority { raw: String },
    #[error("conflicting target task already exists for runtime={runtime_id}, account={account_key}, symbol={symbol}")]
    TaskConflict {
        runtime_id: String,
        account_key: String,
        symbol: String,
    },
    #[error(
        "交易所规定 {symbol} 最小市价开仓手数 ({open_min_market_order_volume}) 或最小限价开仓手数 ({open_min_limit_order_volume}) 大于 1，当前 TargetPosTask 规划器暂不支持该规则"
    )]
    UnsupportedOpenOrderVolume {
        symbol: String,
        open_min_market_order_volume: i32,
        open_min_limit_order_volume: i32,
    },
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;
