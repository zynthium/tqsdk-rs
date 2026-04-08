use crate::runtime::{OffsetPriority, PriceMode, TargetPosConfig};

#[test]
fn target_pos_config_default_uses_active_price_mode() {
    let cfg = TargetPosConfig::default();
    assert!(matches!(cfg.price_mode, PriceMode::Active));
    assert!(matches!(
        cfg.offset_priority,
        OffsetPriority::TodayYesterdayThenOpenWait
    ));
    assert!(cfg.split_policy.is_none());
}
