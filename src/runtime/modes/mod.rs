#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RuntimeMode {
    #[default]
    Live,
    Backtest,
}
