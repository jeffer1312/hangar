use std::{collections::VecDeque, sync::Mutex, time::Duration};

use hangar_core::process::{CommandError, CommandOutput, Commands};
use serde_json::Value;

struct Recorder {
    answers: Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl Recorder {
    fn new(answers: impl IntoIterator<Item = Result<CommandOutput, CommandError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Commands for Recorder {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        deadline: Duration,
    ) -> futures_util::future::BoxFuture<'a, Result<CommandOutput, CommandError>> {
        assert_eq!(program, "tailscale");
        assert_eq!(deadline, Duration::from_secs(20));
        self.calls
            .lock()
            .unwrap()
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("resposta da CLI prevista");
        Box::pin(async move { answer })
    }
}

fn output(success: bool, stdout: &str, stderr: &str) -> Result<CommandOutput, CommandError> {
    Ok(CommandOutput {
        success,
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
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
async fn start_checks_dns_before_touching_only_the_preview_slot() {
    let commands = Recorder::new([
        output(true, r#"{"Self":{"DNSName":"fixture.ts.net."}}"#, ""),
        output(true, "", ""),
    ]);
    let response = value(hangar_port_preview::start(&commands, 3000).await).await;
    assert_eq!(response.0, 200);
    assert_eq!(response.1["url"], "https://fixture.ts.net:10000/");
    assert_eq!(response.1["port"], 3000);
    let calls = commands.calls.lock().unwrap();
    assert_eq!(calls[0], ["status", "--json"]);
    assert_eq!(
        calls[1],
        ["serve", "--bg", "--https=10000", "localhost:3000"]
    );
}

#[tokio::test]
async fn status_reads_only_the_preview_slot_and_preserves_unknown_ports() {
    for (source, expected_port) in [
        (
            r#"{"Web":{"fixture.ts.net:443":{},"fixture.ts.net:10000":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000/"}}}}}"#,
            Some(3000),
        ),
        (r#"{"Web":{"fixture.ts.net:10000":{}}}"#, None),
        (
            r#"{"Web":{"fixture.ts.net:10000":{"Handlers":{"/":{"Proxy":"http://localhost:99999"}}}}}"#,
            Some(99999),
        ),
    ] {
        let commands = Recorder::new([output(true, source, "")]);
        let response = value(hangar_port_preview::status(&commands).await).await;
        assert_eq!(response.0, 200);
        assert_eq!(response.1["active"], true);
        assert_eq!(response.1["port"].as_i64(), expected_port);
        assert_eq!(response.1["url"], "https://fixture.ts.net:10000/");
        assert_eq!(
            commands.calls.lock().unwrap()[0],
            ["serve", "status", "--json"]
        );
    }
}

#[tokio::test]
async fn stop_is_idempotent_and_never_resets_other_slots() {
    for answer in [
        output(true, "", ""),
        output(false, "handler does not exist", ""),
    ] {
        let commands = Recorder::new([answer]);
        let response = value(hangar_port_preview::stop(&commands).await).await;
        assert_eq!(response.0, 200);
        assert_eq!(response.1["active"], false);
        assert!(response.1["url"].is_null());
        let calls = commands.calls.lock().unwrap();
        assert_eq!(
            calls
                .first()
                .map(|args| args.iter().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["serve", "--https=10000", "off"])
        );
    }
}

#[tokio::test]
async fn absent_status_is_inactive_but_wrong_structural_types_stay_errors() {
    for answer in [
        output(false, "", "falha"),
        output(true, "", ""),
        output(true, "{", ""),
        output(true, r#"{"Web":{"fixture.ts.net:443":{}}}"#, ""),
        output(true, r#"{"Web":[]}"#, ""),
        output(true, r#"{"Web":false}"#, ""),
    ] {
        let commands = Recorder::new([answer]);
        let response = value(hangar_port_preview::status(&commands).await).await;
        assert_eq!(response.0, 200);
        assert_eq!(response.1["active"], false);
        assert!(response.1["port"].is_null());
        assert!(response.1["url"].is_null());
    }
    for source in [
        "[]",
        r#"{"Web":["invalid"]}"#,
        r#"{"Web":{"fixture.ts.net:10000":null}}"#,
    ] {
        let commands = Recorder::new([output(true, source, "")]);
        assert_eq!(hangar_port_preview::status(&commands).await.status(), 500);
    }
}

#[tokio::test]
async fn dns_failure_prevents_changes_and_serve_errors_keep_their_status() {
    for (answer, expected) in [
        (output(false, "", "falha DNS"), 503),
        (output(true, "{}", ""), 503),
        (output(true, r#"{"Self":{"DNSName":""}}"#, ""), 503),
        (output(true, "{", ""), 503),
    ] {
        let commands = Recorder::new([answer]);
        assert_eq!(
            hangar_port_preview::start(&commands, 3000).await.status(),
            expected
        );
        assert!(
            commands
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|args| args[0] == "status")
        );
    }
    for (answer, expected) in [
        (output(false, "Access denied", ""), 403),
        (output(false, "", "operator"), 403),
        (output(false, "", "falha serve"), 502),
    ] {
        let commands = Recorder::new([
            output(true, r#"{"Self":{"DNSName":"fixture.ts.net"}}"#, ""),
            answer,
        ]);
        let response = value(hangar_port_preview::start(&commands, 3000).await).await;
        assert_eq!(response.0, expected);
        assert!(response.1["detail"].is_string());
    }
    let commands = Recorder::new([output(false, "", "falha ao parar")]);
    assert_eq!(hangar_port_preview::stop(&commands).await.status(), 502);
}

#[tokio::test]
async fn missing_cli_and_timeout_are_failures_not_an_inactive_tunnel() {
    for (error, status, text) in [
        (CommandError::Timeout, 504, "timeout"),
        (
            CommandError::Io(std::io::ErrorKind::NotFound.into()),
            500,
            "não encontrado",
        ),
        (
            CommandError::Io(std::io::ErrorKind::PermissionDenied.into()),
            500,
            "tailscale falhou",
        ),
    ] {
        let commands = Recorder::new([Err(error)]);
        let response = value(hangar_port_preview::status(&commands).await).await;
        assert_eq!(response.0, status);
        assert!(response.1["detail"].as_str().unwrap().contains(text));
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {cases:64, rng_seed:proptest::test_runner::RngSeed::Fixed(3), ..Default::default()})]
    #[test]
    fn external_status_never_panics(source in ".{0,2048}") {
        let commands = Recorder::new([output(true,&source,"")]);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let status = runtime.block_on(hangar_port_preview::status(&commands)).status();
        proptest::prop_assert!(status == 200 || status == 500);
    }
}
