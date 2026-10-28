use super::*;
use serde_json::{Value, json};

fn state(origins: &Origins) -> Value {
    json!(origins.0.iter().map(|(ip,hits)| json!({"ip":ip,"hits":hits})).collect::<Vec<_>>())
}
fn apply(origins: &mut Origins, owner: &[u8], input: &Value) -> Option<AuthDecision> {
    let now = input["now"].as_f64().unwrap_or(1000.0);
    let ip = input["ip"].as_str().unwrap();
    let profile = match input["profile"].as_str().unwrap() {
        "record" => { origins.record_failure(ip, now); return None },
        "blocked" => return Some(if origins.is_blocked(ip, now) { AuthDecision::BLOCKED } else { AuthDecision::OWNER }),
        "http" => Profile::RequireAuthHttp, "browser_ws" => Profile::BrowserWebSocket,
        "voice_ws" => Profile::VoiceWebSocket, "terminal_ws" => Profile::TerminalWebSocket, _ => panic!("perfil do corpus"),
    };
    Some(origins.evaluate(owner, &AuthAttempt { profile, origin: ip.into(),
        presented_token: input["token"].as_str().map(|s| s.as_bytes().to_vec()),
        verified_guest: input["guest"] == "signed", login_guest: input["guest"] == "login" }, now))
}

#[test]
fn original_python_corpus_keeps_every_timestamp_origin_and_insertion_position() {
    let raw = include_bytes!("../../../backend/tests/fixtures/contract/remote-browser/shared-auth.json");
    let corpus: Value = serde_json::from_slice(raw).unwrap();
    assert_eq!(corpus["source_commit"], "ba2db1021ab7fb801eca50a50f9509dcb825ac7e");
    for source in corpus["sources"].as_object().unwrap().values() {
        let digest = ring::digest::digest(&ring::digest::SHA256, source["text"].as_str().unwrap().as_bytes());
        assert_eq!(crate::shared::hex(digest.as_ref()), source["sha256"].as_str().unwrap());
    }
    let owner = corpus["owner_token"].as_str().unwrap().as_bytes();
    for case in corpus["cases"].as_array().unwrap() {
        let mut origins = Origins::default();
        for input in case["setup"].as_array().unwrap() { apply(&mut origins, owner, input); }
        assert_eq!(state(&origins), case["setup_state"]["origins"], "{} setup", case["name"]);
        for step in case["steps"].as_array().unwrap() {
            let result = apply(&mut origins, owner, &step["input"]);
            assert_eq!(state(&origins), step["state"]["origins"], "{} {}", case["name"], step["input"]);
            let profile = step["input"]["profile"].as_str().unwrap();
            if profile == "record" { continue; }
            let result = result.unwrap();
            if profile == "blocked" { assert_eq!(result.status == 3, step["output"]["return"].as_bool().unwrap()); }
            else if profile == "http" {
                let want = step["output"]["raised"]["status_code"].as_u64().unwrap_or(200);
                let got = match result.status { 0 | 1 => 200, 2 => 401, 3 => 429, _ => unreachable!() };
                assert_eq!(got, want, "{} {}", case["name"], step["input"]);
            } else {
                let returned = &step["output"]["return"];
                assert_eq!(result.status <= 1, returned == true || returned.is_array(), "{} {}", case["name"], step["input"]);
            }
        }
    }
}

fn config(path: &std::path::Path, mode: &str, epoch: &str) -> AuthConfig {
    AuthConfig { abi: AUTH_ABI, store_version: AUTH_STORE, mode: mode.into(), state_path: path.to_owned(),
        parent_epoch: epoch.into(), expected_token: "owner-fake".into(), receipt_key: "ab".repeat(32) }
}
fn attempt(token: &str) -> AuthAttempt {
    AuthAttempt { profile: Profile::RequireAuthHttp, origin: "198.51.100.10".into(),
        presented_token: Some(token.as_bytes().to_vec()), verified_guest: false, login_guest: false }
}

#[test]
fn attach_keeps_state_new_parent_is_empty_and_invalid_store_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let context = SharedLimiter::open(config(&path, "create", &"01".repeat(16))).unwrap();
    for _ in 0..8 { assert_eq!(context.evaluate_at(&attempt("wrong"), 1000.0).unwrap().status, 2); }
    drop(context);
    let attached = SharedLimiter::open(config(&path, "attach", &"01".repeat(16))).unwrap();
    assert_eq!(attached.evaluate_at(&attempt("owner-fake"), 1001.0).unwrap().status, 3);
    assert!(matches!(SharedLimiter::open(config(&path, "attach", &"02".repeat(16))), Err(AuthError::EpochMismatch)));
    let next = SharedLimiter::open(config(&directory.path().join("new.json"), "create", &"02".repeat(16))).unwrap();
    assert_eq!(next.evaluate_at(&attempt("owner-fake"), 1001.0).unwrap().status, 0);
    std::fs::write(&path, b"truncated").unwrap();
    assert_eq!(attached.evaluate_at(&attempt("owner-fake"), 1001.0), Err(AuthError::InvalidInput));
}

#[test]
fn receipt_binds_request_and_does_not_clear_a_later_failure() {
    let directory = tempfile::tempdir().unwrap();
    let limiter = SharedLimiter::open(config(&directory.path().join("state.json"), "create", &"03".repeat(16))).unwrap();
    let context = ReceiptContext { connection_kind: "private_upstream".into(), profile: Profile::LegacyOwnerProbe,
        origin: "198.51.100.10".into(), method: "GET".into(), raw_target: "/api/me?a=%25".into(),
        credential_source: "query".into(), presented_token: Some(b"owner-fake".to_vec()) };
    assert_eq!(limiter.evaluate(&attempt("owner-fake")).unwrap().status, 0);
    let receipt = limiter.receipt(&context).unwrap();
    assert!(!receipt.contains("owner-fake"));
    limiter.evaluate(&attempt("wrong")).unwrap();
    for _ in 0..2 { assert_eq!(limiter.verify_receipt(&context, receipt.as_bytes()).unwrap().status, 0); }
    assert_eq!(limiter.transaction(|o| o.0[0].1.len()).unwrap(), 1);
    for field in ["origin", "method", "raw_target", "credential_source", "connection_kind", "presented_token", "profile"] {
        let mut altered = serde_json::to_value(&context).unwrap();
        altered[field] = match field { "profile" => json!(0), "presented_token" => json!([120]), _ => json!("altered") };
        let altered: ReceiptContext = serde_json::from_value(altered).unwrap();
        assert_eq!(limiter.verify_receipt(&altered, receipt.as_bytes()).unwrap().status, 2, "{field}");
    }
}

#[test]
fn ffi_versions_and_errors_are_separate_from_credential_decisions() {
    assert_eq!(std::mem::size_of::<AuthDecision>(), 12);
    assert_eq!(crate::ffi::hangar_auth_abi_v1(), 1);
    let mut handle = std::ptr::null_mut();
    let mut decision = AuthDecision::OWNER;
    unsafe {
        assert_eq!(crate::ffi::hangar_auth_open_v1(std::ptr::null(), 1, &mut handle), 2);
        assert!(handle.is_null());
        assert_eq!(crate::ffi::hangar_auth_evaluate_v1(handle, b"{}".as_ptr(), 2, &mut decision), 2);
        assert_eq!(crate::ffi::hangar_auth_verify_receipt_v1(handle, b"{}".as_ptr(), 2,
            b"x".as_ptr(), 1, &mut decision), 2);
        crate::ffi::hangar_auth_close_v1(handle);
    }
}
