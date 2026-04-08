use crate::runtime::OrderDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetAction {
    Today,
    Yesterday,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlannedOffset {
    Open,
    Close,
    CloseToday,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOrder {
    pub direction: OrderDirection,
    pub offset: PlannedOffset,
    pub volume: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedBatch {
    pub orders: Vec<PlannedOrder>,
}
