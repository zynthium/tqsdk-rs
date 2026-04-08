use std::{fmt, sync::Arc};

use crate::{Quote, Result as TqResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    Buy,
    Sell,
}

pub type PriceResolver = Arc<dyn Fn(OrderDirection, &Quote) -> TqResult<f64> + Send + Sync>;

#[derive(Clone, Default)]
pub enum PriceMode {
    #[default]
    Active,
    Passive,
    Custom(PriceResolver),
}

impl fmt::Debug for PriceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => f.write_str("Active"),
            Self::Passive => f.write_str("Passive"),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OffsetPriority {
    #[default]
    TodayYesterdayThenOpenWait,
    TodayYesterdayThenOpen,
    YesterdayThenOpen,
    OpenOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VolumeSplitPolicy {
    pub min_volume: i64,
    pub max_volume: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TargetPosConfig {
    pub price_mode: PriceMode,
    pub offset_priority: OffsetPriority,
    pub split_policy: Option<VolumeSplitPolicy>,
}
