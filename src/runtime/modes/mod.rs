mod backtest;
mod live;

pub use backtest::BacktestRuntimeMode;
pub use live::LiveRuntimeMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RuntimeMode {
    #[default]
    Live,
    Backtest,
}
