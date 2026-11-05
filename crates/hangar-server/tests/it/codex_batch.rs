use hangar_server::{codex_setup::batch,accounts::catalog::Account};
use serde_json::{Value,json};
struct Actions {name:String,calls:Vec<Value>,sessions:Vec<Value>}
impl batch::Actions for Actions {
 async fn repair(&mut self,account:&Account)->Result<(),String>{self.calls.push(json!(["repair",account.id,{"adotar_preparo":true}]));if self.name=="repair-failure"{Err("fixture repair".into())}else{Ok(())}}
 fn sessions(&mut self)->Result<Vec<Value>,String>{self.calls.push(json!(["sessions",{"include_incomplete":true}]));Ok(self.sessions.clone())}
 fn alive(&self,_pid:&Value)->Result<bool,String>{if self.name=="probe-error"{Err("consulta de processo falhou".into())}else{Ok(self.name!="dead")}}
 async fn reload(&mut self,endpoint:&str,cwd:&str,account:&Account)->Result<bool,String>{self.calls.push(json!(["reload",endpoint,cwd,{"recarregar":true,"codex_home":account.home}]));Ok(self.name!="reload-false")}
}
#[test]
fn codex_service_batch_cli_works_without_python_or_a_server() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar/current/backend")).unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.env_clear().env("HOME", home.path()).env("USERPROFILE", home.path()).env("PATH", "");
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) { command.env(key, value); }
    }
    let output = command.args(["harness-setup", "repair-codex"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let proof:Value = serde_json::from_slice(&std::fs::read(home.path().join(".hangar/codex-prepare-once-v1.json")).unwrap()).unwrap();
    assert_eq!(proof, json!({"version":1,"accounts":[]}));
}

#[tokio::test]
async fn codex_service_batch_does_not_publish_success_when_liveness_fails() {
    let home=tempfile::tempdir().unwrap();let account=Account{id:"default".into(),home:home.path().join(".codex"),is_default:true};
    std::fs::create_dir(&account.home).unwrap();
    let mut actions=Actions{name:"probe-error".into(),calls:vec![],sessions:vec![json!({"endpoint":"ws://127.0.0.1:1","codex_home":account.home,"app_pid":42})]};
    assert_eq!(batch::run(home.path(),&[account],&mut actions).await.unwrap_err(),"consulta de processo falhou");
    assert!(!home.path().join(".hangar/codex-prepare-once-v1.json").exists());
    assert_eq!(actions.calls.len(),2);
}
