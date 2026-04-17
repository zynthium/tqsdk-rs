use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;

#[derive(Debug, Clone)]
pub struct ContinuousMapping {
    pub trading_day: NaiveDate,
    pub symbol: String,
    pub underlying_symbol: String,
}

#[derive(Debug, Clone, Default)]
pub struct ContinuousContractProvider {
    days: BTreeMap<NaiveDate, HashMap<String, String>>,
}

impl ContinuousContractProvider {
    pub fn from_rows(rows: Vec<ContinuousMapping>) -> Self {
        let mut days = BTreeMap::new();
        for row in rows {
            days.entry(row.trading_day)
                .or_insert_with(HashMap::new)
                .insert(row.symbol, row.underlying_symbol);
        }
        Self { days }
    }

    pub fn mapping_for(&self, trading_day: NaiveDate) -> HashMap<String, String> {
        self.days.get(&trading_day).cloned().unwrap_or_default()
    }
}
