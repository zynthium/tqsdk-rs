mod account;
mod core;
mod engine;
mod errors;
mod execution;
mod market;
mod modes;
mod registry;
mod tasks;
mod types;

pub use account::AccountHandle;
pub use core::TqRuntime;
pub(crate) use engine::ExecutionEngine;
pub use errors::{RuntimeError, RuntimeResult};
pub(crate) use execution::{ExecutionAdapter, LiveExecutionAdapter};
pub(crate) use market::{LiveMarketAdapter, MarketAdapter};
pub use modes::RuntimeMode;
pub(crate) use registry::{TaskId, TaskRegistry};
#[cfg(test)]
pub(crate) use tasks::{
    ChildOrderRunner, OffsetAction, OpenLimitBudget, PlannedOffset, compute_plan, parse_offset_priority,
    validate_quote_constraints,
};
pub use tasks::{
    TargetPosBuilder, TargetPosExecutionReport, TargetPosScheduleStep, TargetPosScheduler, TargetPosSchedulerBuilder,
    TargetPosSchedulerConfig, TargetPosTask,
};
pub use types::{OffsetPriority, OrderDirection, PriceMode, PriceResolver, TargetPosConfig, VolumeSplitPolicy};

#[cfg(test)]
mod tests;
