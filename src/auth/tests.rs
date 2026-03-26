use super::{Authenticator, Grants, TqAuth};
use std::collections::HashSet;

#[test]
fn test_tq_auth_creation() {
    let auth = TqAuth::new("test_user".to_string(), "test_pass".to_string());
    assert_eq!(auth.username, "test_user");
    assert_eq!(auth.password, "test_pass");
}

fn auth_with_features(features: &[&str]) -> TqAuth {
    let mut auth = TqAuth::new("test".to_string(), "pass".to_string());
    auth.grants = Grants {
        features: features.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        accounts: HashSet::new(),
    };
    auth
}

// ---------------------------------------------------------------------------
// has_md_grants
// ---------------------------------------------------------------------------

#[test]
fn has_md_grants_allows_futures_with_futr_feature() {
    let auth = auth_with_features(&["futr"]);
    assert!(auth.has_md_grants(&["SHFE.au2602", "DCE.m2512", "CZCE.SR501"]).is_ok());
}

#[test]
fn has_md_grants_denies_futures_without_futr_feature() {
    let auth = auth_with_features(&[]);
    let err = auth.has_md_grants(&["SHFE.au2602"]).unwrap_err();
    assert!(err.to_string().contains("期货行情"));
}

#[test]
fn has_md_grants_allows_stocks_with_sec_feature() {
    let auth = auth_with_features(&["sec"]);
    assert!(auth.has_md_grants(&["SSE.600000", "SZSE.000001"]).is_ok());
}

#[test]
fn has_md_grants_denies_stocks_without_sec_feature() {
    let auth = auth_with_features(&[]);
    let err = auth.has_md_grants(&["SSE.600000"]).unwrap_err();
    assert!(err.to_string().contains("股票行情"));
}

#[test]
fn has_md_grants_denies_csi_without_sec_feature() {
    let auth = auth_with_features(&[]);
    let err = auth.has_md_grants(&["CSI.000300"]).unwrap_err();
    assert!(err.to_string().contains("股票行情"));
}

#[test]
fn has_md_grants_allows_all_futures_exchanges() {
    let auth = auth_with_features(&["futr"]);
    let exchanges = ["CFFEX.IF2601", "INE.sc2601", "GFEX.si2601", "KQ.m@DCE.m2512", "KQD.test"];
    assert!(auth.has_md_grants(&exchanges.iter().map(|s| *s).collect::<Vec<_>>()).is_ok());
}

#[test]
fn has_md_grants_rejects_unsupported_exchange() {
    let auth = auth_with_features(&["futr", "sec"]);
    let err = auth.has_md_grants(&["UNKNOWN.test"]).unwrap_err();
    assert!(err.to_string().contains("不支持的合约"));
}

// ---------------------------------------------------------------------------
// has_td_grants
// ---------------------------------------------------------------------------

#[test]
fn has_td_grants_allows_futures_with_futr_feature() {
    let auth = auth_with_features(&["futr"]);
    assert!(auth.has_td_grants("SHFE.au2602").is_ok());
}

#[test]
fn has_td_grants_denies_futures_without_futr_feature() {
    let auth = auth_with_features(&[]);
    let err = auth.has_td_grants("SHFE.au2602").unwrap_err();
    assert!(err.to_string().contains("不支持交易"));
}

#[test]
fn has_td_grants_allows_stocks_with_sec_feature() {
    let auth = auth_with_features(&["sec"]);
    assert!(auth.has_td_grants("SSE.600000").is_ok());
}

#[test]
fn has_td_grants_denies_stocks_without_sec_feature() {
    let auth = auth_with_features(&[]);
    let err = auth.has_td_grants("SSE.600000").unwrap_err();
    assert!(err.to_string().contains("不支持交易"));
}

#[test]
fn has_td_grants_rejects_unsupported_exchange() {
    let auth = auth_with_features(&["futr", "sec"]);
    let err = auth.has_td_grants("UNKNOWN.test").unwrap_err();
    assert!(err.to_string().contains("不支持的合约"));
}
