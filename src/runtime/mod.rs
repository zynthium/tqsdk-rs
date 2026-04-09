mod account;
mod core;
mod engine;
mod errors;
mod execution;
mod market;
pub mod modes;
mod registry;
pub mod tasks;
mod types;

pub use account::AccountHandle;
pub use core::TqRuntime;
pub use engine::ExecutionEngine;
pub use errors::{RuntimeError, RuntimeResult};
pub use execution::{BacktestExecutionAdapter, ExecutionAdapter, LiveExecutionAdapter};
pub use market::{LiveMarketAdapter, MarketAdapter};
pub use modes::{BacktestRuntimeMode, LiveRuntimeMode, RuntimeMode};
pub use registry::{RegisteredTask, TaskId, TaskRegistry};
pub use tasks::{
    ChildOrderRunner, OffsetAction, PlannedBatch, PlannedOffset, PlannedOrder, TargetPosBuilder,
    TargetPosExecutionReport, TargetPosHandle, TargetPosScheduleStep, TargetPosSchedulerBuilder,
    TargetPosSchedulerConfig, TargetPosSchedulerHandle, compute_plan, parse_offset_priority, resolve_order_price,
    validate_quote_constraints,
};
pub use types::{OffsetPriority, OrderDirection, PriceMode, PriceResolver, TargetPosConfig, VolumeSplitPolicy};

#[cfg(test)]
mod tests;
