pub use crate::{
    AccountHandle, Client, ClientConfig, DataDownloadAdjType, DataDownloadOptions, DataDownloadRequest,
    DataDownloadWriteMode, DataDownloadWriter, DataDownloader, EndpointConfig, KlineRef, MarketDataUpdates,
    OffsetPriority, OrderDirection, OrderEventStream, PriceMode, QuoteRef, QuoteSubscription, ReplayConfig,
    ReplayReport, ReplaySession, Result, RuntimeResult, SeriesSubscription, TargetPosConfig, TargetPosScheduleStep,
    TargetPosScheduler, TargetPosTask, TickRef, TqError, TqRuntime, TradeEventRecvError, TradeEventStream,
    TradeFrontConfig, TradeLoginOptions, TradeOnlyEventStream, TradeSession, TradeSessionEvent, TradeSessionEventKind,
    TradeSessionOptions, VolumeSplitPolicy, create_logger_layer, init_logger,
};

#[cfg(feature = "polars")]
pub use crate::polars_ext::{KlineBuffer, TickBuffer};

#[cfg(test)]
mod tests {
    #[test]
    fn root_and_prelude_export_trade_session_event() {
        let _root_event: Option<crate::TradeSessionEvent> = None;

        use crate::prelude::*;
        let _prelude_event: Option<TradeSessionEvent> = None;
    }

    #[test]
    fn root_and_prelude_export_market_data_updates() {
        let _root_updates: Option<crate::MarketDataUpdates> = None;

        use crate::prelude::*;
        let _prelude_updates: Option<MarketDataUpdates> = None;
    }

    #[test]
    fn root_and_prelude_export_replay_report() {
        let _root_report: Option<crate::ReplayReport> = None;

        use crate::prelude::*;
        let _prelude_report: Option<ReplayReport> = None;
    }
}
