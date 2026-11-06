use super::*;

fn json_headers() -> HeaderMap {
    HeaderMap::from_iter([(axum::http::header::CONTENT_TYPE,axum::http::HeaderValue::from_static("application/json"))])
}

async fn error_body(response: Response) -> Value {
    assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
    let bytes=axum::body::to_bytes(response.into_body(),usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn typed_body_errors_preserve_original_locations_inputs_and_order() {
    let cases:Vec<(&[u8],Value)>=vec![
        (b"",json!({"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]})),
        (b"{}",json!({"detail":[{"type":"missing","loc":["body","repo"],"msg":"Field required","input":{}}]})),
        (br#"{"repo":""}"#,json!({"detail":[{"type":"string_too_short","loc":["body","repo"],"msg":"String should have at least 1 character","input":"","ctx":{"min_length":1}}]})),
        (br#"{"repo":null}"#,json!({"detail":[{"type":"string_type","loc":["body","repo"],"msg":"Input should be a valid string","input":null}]})),
        (br#"{"repo":7,"extra":true}"#,json!({"detail":[{"type":"string_type","loc":["body","repo"],"msg":"Input should be a valid string","input":7},{"type":"extra_forbidden","loc":["body","extra"],"msg":"Extra inputs are not permitted","input":true}]})),
        (b"[]",json!({"detail":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":[]}]})),
        (b"null",json!({"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]})),
        (b"{",json!({"detail":[{"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}}]})),
    ];
    for (input,want) in cases {
        assert_eq!(error_body(validate(input,&json_headers()).unwrap_err()).await,want,"{input:?}");
    }
    assert_eq!(validate(br#"{"repo":" r "}"#,&json_headers()).unwrap()," r ");
}

#[tokio::test]
async fn missing_content_type_does_not_silently_choose_json() {
    let raw=br#"{"repo":"r"}"#;
    let body=error_body(validate(raw,&HeaderMap::new()).unwrap_err()).await;
    assert_eq!(body,json!({"detail":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":"{\"repo\":\"r\"}"}]}));
}

#[test]
fn stderr_credentials_keep_original_port_validation() {
    for (input,want) in [
        ("https://u:p/a@host","https://***@host"),
        ("http://localhost:8317/path@home","http://localhost:8317/path@home"),
        ("https://host:99999/path@home","https://***@home"),
        ("https://host:/path@home","https://host:/path@home"),
        ("https://[::1]:8317/path@home","https://[::1]:8317/path@home"),
        ("https://[foo]:8317/path@home","https://***@home"),
        ("https://a::1/path@home","https://***@home"),
        ("https://host/p:a@home","https://host/p:a@home"),
    ] { assert_eq!(scrub(input),want); }
}

#[test]
fn absent_git_remains_a_500_not_generic_workspace_unavailability() {
    let error=process_error(hangar_workspace::unavailable("git não encontrado"));
    assert_eq!(error.status,500);
    assert_eq!(error.detail,"git nao encontrado");
}

#[test]
fn spawn_failure_preserves_os_error_code_in_the_original_envelope() {
    let code=if cfg!(windows){216}else{8};
    let native=std::io::Error::from_raw_os_error(code);
    let error=process_error(hangar_workspace::unavailable(format!("git não iniciou: {native}")));
    assert_eq!(error.status,500);
    let detail=error.detail.as_str().unwrap();
    assert!(detail.starts_with(if cfg!(windows){"git falhou: [WinError 216] "}else{"git falhou: [Errno 8] "}),"{detail}");
    assert!(!detail.contains("(os error"));
    #[cfg(unix)] assert_eq!(detail,"git falhou: [Errno 8] Exec format error: 'git'");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5754_4645_5443_4801),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn untrusted_request_bytes_do_not_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096),is_json in proptest::prelude::any::<bool>()) {
        let headers=if is_json {json_headers()} else {HeaderMap::new()};
        let _=validate(&bytes,&headers);
    }
}
