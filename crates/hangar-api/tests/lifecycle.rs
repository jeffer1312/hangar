use hangar_api::session::runtime_key;

#[test]
fn session_lifecycle_key_is_shared_by_both_sides() {
    let key="0123456789abcdef0123456789abcdef";
    assert_eq!(runtime_key(Some("k:0123456789abcdef0123456789abcdef")),Some(key));
    assert_eq!(runtime_key(Some("k:0123456789abcdef0123456789abcdef:g:7")),Some(key));
    assert_eq!(runtime_key(Some("t:1234")),None);
    assert_eq!(runtime_key(None),None);
    assert_eq!(runtime_key(Some("k:legacy-key")),Some("legacy-key"));
}
