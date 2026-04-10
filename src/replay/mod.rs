pub mod feed;
pub mod kernel;
pub mod providers;
pub mod quote;
pub mod series;
pub mod sim;
pub mod types;

#[cfg(test)]
mod tests;

pub use feed::{FeedCursor, FeedEvent, HistoricalSource};
pub use kernel::ReplayKernel;
pub use providers::{ContinuousContractProvider, ContinuousMapping};
pub use quote::{QuoteSelection, QuoteSynthesizer, QuoteUpdate};
pub use series::{AlignedKlineHandle, AlignedKlineRow, KlineSeriesRow, ReplayKlineHandle, SeriesStore, TickSeriesRow};
pub use sim::SimBroker;
pub use types::{
    BacktestResult, BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayHandleId, ReplayQuote,
    ReplayStep,
};
