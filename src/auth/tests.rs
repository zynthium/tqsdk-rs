use super::TqAuth;

#[test]
fn test_tq_auth_creation() {
    let auth = TqAuth::new("test_user".to_string(), "test_pass".to_string());
    assert_eq!(auth.username, "test_user");
    assert_eq!(auth.password, "test_pass");
}
