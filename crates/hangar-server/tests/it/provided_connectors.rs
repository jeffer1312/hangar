//! O artefato distribuído atende o driver do Help sem iniciar outro servidor.
use hangar_connectors::{protocol,rpc};
use serde_json::{Value,json};
use std::process::Stdio;
use tokio::io::BufReader;

#[tokio::test]
async fn board_action_provided_help_is_served_by_the_distributed_server_cli() {
    let temp=tempfile::tempdir().unwrap();
    let mut command=tokio::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.env_clear().env("HOME",temp.path()).env("USERPROFILE",temp.path()).env("XDG_CONFIG_HOME",temp.path())
        .env("TZ","UTC").env("LC_ALL","C").current_dir(temp.path())
        .args(["help-connector","--id","provided-proof","--help-url","http://127.0.0.1:9","--config-dir"]).arg(temp.path())
        .arg("--claude-config-dir").arg(temp.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for name in ["SystemRoot","WINDIR"] {if let Some(value)=std::env::var_os(name) {command.env(name,value);}}
    let mut child=command.spawn().unwrap();let mut input=child.stdin.take().unwrap();let mut output=BufReader::new(child.stdout.take().unwrap());
    let exchange=async {
        let mut replies=Vec::new();
        for (id,method,params) in [
            (1,"initialize",json!({"protocol_version":1,"host":{"name":"hangar"},"capabilities":{"work_tracking":1,"work_actions":1}})),
            (2,"work_actions.list",json!({"ticket_id":"PM-1"})),
            (3,"shutdown",Value::Null),
        ] {
            rpc::write_message(&mut input,&protocol::request(id,method,params)).await.map_err(|e|e.to_string())?;
            let reply=rpc::read_message(&mut output).await.map_err(|e|e.to_string())?.ok_or("o CLI encerrou sem responder")?;
            replies.push(serde_json::from_slice::<Value>(&reply).map_err(|e|e.to_string())?);
        }
        Ok::<_,String>(replies)
    };
    let replies=tokio::time::timeout(std::time::Duration::from_secs(30),exchange).await;
    if !matches!(&replies,Ok(Ok(_))) {let _=child.start_kill();}
    let exit=child.wait().await.unwrap();
    assert!(exit.success(),"o driver precisa sair sem deixar processo aberto");
    let replies=replies.expect("o driver travou").expect("o protocolo do driver falhou");
    assert_eq!(replies[0]["result"]["id"],"provided-proof");
    assert_eq!(replies[0]["result"]["capabilities"]["work_actions"],1);
    assert!(replies[1]["result"]["actions"].as_array().unwrap().iter().any(|action|action["id"]=="start-pm" && action["prompt_template"]=="/pmedico:iniciar-pm {key}"));
}
