use hangar_server::runtime::protocol::*;
use serde_json::json;

#[test]
fn request_ids_keep_type() {
    let number: RequestId = serde_json::from_str("1").unwrap();
    let string: RequestId = serde_json::from_str("\"1\"").unwrap();
    assert_ne!(number, string);
    assert!(serde_json::from_str::<RequestId>("true").is_err());
    assert!(serde_json::from_str::<RequestId>("1.5").is_err());
}

#[test]
fn clock_domains_stay_separate() {
    let clock = ClockSample { monotonic_s: 10.0, epoch_s: 1_800_000_000.0 };
    let encoded = serde_json::to_value(clock).unwrap();
    assert_eq!(encoded["monotonic_s"], 10.0);
    assert_eq!(encoded["epoch_s"], 1_800_000_000.0);
}

#[test]
fn command_rejects_unknown_fields() {
    let command = json!({"operation_id":"op-1","kind":"input","payload":{"text":"Olá"}});
    assert!(serde_json::from_value::<RuntimeCommand>(command.clone()).is_ok());
    let mut extra = command;
    extra["legacy_fallback"] = json!(true);
    assert!(serde_json::from_value::<RuntimeCommand>(extra).is_err());
    assert!(serde_json::from_value::<OperationKind>(json!("unknown")).is_err());
}

#[test]
fn snapshot_checks_type_version_and_complete_pending_lines() {
    let snapshot = json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,
        "aberto":false,"pendentes":["{\"id\":1,\"method\":\"approval\"}"],
        "ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,"inflight":{}});
    assert!(CanoSnapshot::parse(snapshot.clone()).is_ok());
    for (field, invalid) in [("type", json!("result")), ("versao", json!(1)),
        ("pendentes", json!(["{\"id\":" ]))] {
        let mut wrong = snapshot.clone();
        wrong[field] = invalid;
        assert!(CanoSnapshot::parse(wrong).is_err());
    }
}

#[test]
fn error_format_does_not_include_conversation() {
    let error = RuntimeError::new("transport", "o cano não respondeu");
    assert_eq!(error.to_string(), "transport: o cano não respondeu");
    assert_eq!(MAX_FRAME, 16 * 1024 * 1024);
    assert_eq!(MAX_ENVELOPE, 2 * MAX_FRAME + 1024);
}
