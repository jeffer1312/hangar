use std::{sync::Mutex, time::Duration};

use hangar_core::process::{CommandError, CommandOutput, Commands};
use serde_json::Value;

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, Vec<String>, Duration)>>,
    result: Mutex<Option<Result<CommandOutput, CommandError>>>,
}

impl Commands for Recorder {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        deadline: Duration,
    ) -> futures_util::future::BoxFuture<'a, Result<CommandOutput, CommandError>> {
        self.calls.lock().unwrap().push((
            program.into(),
            args.iter().map(|arg| (*arg).into()).collect(),
            deadline,
        ));
        let result = self.result.lock().unwrap().take().unwrap_or_else(|| {
            Ok(CommandOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        });
        Box::pin(async move { result })
    }
}

async fn value(response: axum::response::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 10240)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn authentication_precedes_event_and_json_handling() {
    let commands = Recorder::default();
    let body = b"not JSON";
    let disabled = value(hangar_deploy::webhook("", "push", "", body, &commands).await).await;
    assert_eq!(disabled.0, 404);
    assert_eq!(disabled.1["detail"]["code"], "erro_nao_encontrado");
    for sig in [
        String::new(),
        signature(body).to_uppercase(),
        signature(b"different body"),
    ] {
        let response =
            value(hangar_deploy::webhook("fixture-secret", "ping", &sig, body, &commands).await)
                .await;
        assert_eq!(response.0, 401);
        assert_eq!(response.1["detail"]["code"], "erro_assinatura_invalida");
    }
    let ping = value(
        hangar_deploy::webhook("fixture-secret", "ping", &signature(body), body, &commands).await,
    )
    .await;
    assert_eq!(ping.0, 200);
    assert_eq!(ping.1["status"], "pong");
    let ignored = value(
        hangar_deploy::webhook(
            "fixture-secret",
            "pull_request",
            &signature(body),
            body,
            &commands,
        )
        .await,
    )
    .await;
    assert_eq!(ignored.1["reason"], "event=pull_request");
    assert!(commands.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn push_requires_an_object_and_only_main_starts_the_service() {
    let commands = Recorder::default();
    for body in [b"{".as_slice(), b"[]", b"null"] {
        let response = value(
            hangar_deploy::webhook("fixture-secret", "push", &signature(body), body, &commands)
                .await,
        )
        .await;
        assert_eq!(response.0, 400);
        assert_eq!(response.1["detail"]["code"], "erro_payload_invalido");
    }
    for (body, reason) in [
        (r#"{}"#, "ref="),
        (r#"{"ref":"refs/heads/dev"}"#, "ref=refs/heads/dev"),
        (r#"{"ref":null}"#, "ref=None"),
    ] {
        let response = value(
            hangar_deploy::webhook(
                "fixture-secret",
                "push",
                &signature(body.as_bytes()),
                body.as_bytes(),
                &commands,
            )
            .await,
        )
        .await;
        assert_eq!(response.0, 200);
        assert_eq!(response.1["status"], "ignored");
        assert_eq!(response.1["reason"], reason);
    }
    assert!(commands.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn process_failure_and_timeout_never_report_acceptance() {
    for result in [
        Ok(CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "falha".into(),
        }),
        Err(CommandError::Timeout),
    ] {
        let commands = Recorder {
            result: Mutex::new(Some(result)),
            ..Default::default()
        };
        let body = br#"{"ref":"refs/heads/main"}"#;
        let response = value(
            hangar_deploy::webhook("fixture-secret", "push", &signature(body), body, &commands)
                .await,
        )
        .await;
        assert_eq!(response.0, 500);
        assert_eq!(response.1["detail"]["code"], "erro_deploy_falhou");
    }
    let commands = Recorder {
        result: Mutex::new(Some(Err(CommandError::Io(
            std::io::ErrorKind::NotFound.into(),
        )))),
        ..Default::default()
    };
    let body = br#"{"ref":"refs/heads/main"}"#;
    assert_eq!(
        hangar_deploy::webhook("fixture-secret", "push", &signature(body), body, &commands)
            .await
            .status(),
        500
    );
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {cases:64, rng_seed:proptest::test_runner::RngSeed::Fixed(2), ..Default::default()})]
    #[test]
    fn signed_external_payload_never_panics(body in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let status = runtime.block_on(hangar_deploy::webhook("fixture-secret", "push", &signature(&body), &body, &Recorder::default())).status();
        proptest::prop_assert!(status == 200 || status == 400);
    }
}

fn signature(body: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture-secret");
    format!(
        "sha256={}",
        ring::hmac::sign(&key, body)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[tokio::test]
async fn signed_main_push_starts_only_the_independent_deploy_service() {
    let commands = Recorder::default();
    let body = br#"{"ref":"refs/heads/main"}"#;
    let response =
        hangar_deploy::webhook("fixture-secret", "push", &signature(body), body, &commands).await;
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["status"], "accepted");
    let calls = commands.calls.lock().unwrap();
    assert_eq!(calls[0].0, "systemctl");
    assert_eq!(
        calls[0].1,
        ["--user", "start", "--no-block", "hangar-deploy.service"]
    );
    assert_eq!(calls[0].2, Duration::from_secs(10));
}
