pub mod feed;
pub mod kernel;
pub mod providers;
pub mod series;
pub mod types;

#[cfg(test)]
mod tests;

pub use feed::{FeedCursor, FeedEvent, HistoricalSource};
pub use kernel::ReplayKernel;
pub use providers::{ContinuousContractProvider, ContinuousMapping};
pub use series::{AlignedKlineHandle, AlignedKlineRow, KlineSeriesRow, SeriesStore};
pub use types::{
    BacktestResult, BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayHandleId, ReplayQuote,
    ReplayStep,
};
