//! Perfil do Help pelo protocolo público do processo, sem credenciais nem acesso à rede.
use hangar_connectors::{protocol, rpc};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test]
async fn help_work_actions_offer_the_existing_plugin_flow_when_negotiated() {
    let temp = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hangar-help-test-connector"));
    command.env_clear().env("HOME", temp.path()).env("USERPROFILE", temp.path()).env("XDG_CONFIG_HOME", temp.path())
        .env("TZ", "UTC").env("LC_ALL", "C").current_dir(temp.path())
        .args(["--help-url", "http://127.0.0.1:9", "--config-dir"]).arg(temp.path()).arg("--claude-config-dir").arg(temp.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for name in ["SystemRoot", "WINDIR"] { if let Some(value) = std::env::var_os(name) { command.env(name, value); } }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let exchange = async {
        let mut replies = Vec::new();
        for (id, method, params) in [
            (1, "initialize", json!({"protocol_version": 1, "host": {"name": "hangar"}, "capabilities": {"work_tracking": 1, "work_actions": 1}})),
            (2, "work_actions.list", json!({"ticket_id": "PM-101"})),
            (3, "shutdown", Value::Null),
        ] {
            rpc::write_message(&mut stdin, &protocol::request(id, method, params)).await.map_err(|e| e.to_string())?;
            let bytes = rpc::read_message(&mut stdout).await.map_err(|e| e.to_string())?.ok_or("o conector encerrou sem resposta")?;
            replies.push(serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?);
        }
        Ok::<_, String>(replies)
    };
    let result = tokio::time::timeout(Duration::from_secs(20), exchange).await;
    if !matches!(&result, Ok(Ok(_))) { let _ = child.start_kill(); }
    let status = child.wait().await.unwrap();
    assert!(status.success(), "o processo precisa encerrar sem deixar a vida aberta");
    let replies = result.expect("o conector travou").expect("o protocolo falhou");
    assert_eq!(replies[0]["result"]["capabilities"]["work_actions"], json!(1));
    let actions = replies[1]["result"]["actions"].as_array().unwrap();
    let action = |id: &str| actions.iter().find(|a| a["id"] == id).unwrap();
    assert_eq!(action("start-pm")["prompt_template"], json!("/pmedico:iniciar-pm {key}"));
    assert_eq!(action("send-review")["prompt_template"], json!("/pmedico:iniciar-revisao-pm {key}"));
    assert_eq!(action("follow-review")["stage"], json!("revisao"));
    assert_eq!(action("finish-pm")["prompt_template"], json!("/pmedico:finalizar-pm {key}"));
}
