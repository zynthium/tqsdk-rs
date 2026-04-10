mod feed;
mod kernel;
mod providers;
mod quote;
mod runtime;
mod series;
mod session;
mod sim;
mod types;

#[cfg(test)]
mod tests;

pub use feed::{FeedCursor, FeedEvent, HistoricalSource};
pub use kernel::ReplayKernel;
pub use providers::{ContinuousContractProvider, ContinuousMapping};
pub use quote::{QuoteSelection, QuoteSynthesizer, QuoteUpdate};
pub use series::{AlignedKlineHandle, AlignedKlineRow, KlineSeriesRow, ReplayKlineHandle, SeriesStore, TickSeriesRow};
pub use session::{ReplayQuoteHandle, ReplaySeriesSession, ReplaySession};
pub use sim::SimBroker;
pub use types::{
    BacktestResult, BarState, DailySettlementLog, InstrumentMetadata, ReplayConfig, ReplayHandleId, ReplayQuote,
    ReplayStep,
};
