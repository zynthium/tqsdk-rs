mod common;
mod scheduler;
mod target_pos;

pub(crate) use common::{ChildOrderControl, ChildOrderStatus};
pub use common::{ChildOrderRunner, OffsetAction, PlannedBatch, PlannedOffset, PlannedOrder, resolve_order_price};
pub use scheduler::{
    TargetPosExecutionReport, TargetPosScheduleStep, TargetPosScheduler, TargetPosSchedulerBuilder,
    TargetPosSchedulerConfig,
};
pub use target_pos::{
    TargetPosBuilder, TargetPosTask, compute_plan, parse_offset_priority, validate_quote_constraints,
};

#[cfg(test)]
mod tests;
