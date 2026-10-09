#![cfg(unix)]
use hangar_server::{runtime::{actor::PolicyClient,protocol::{OperationKind,RuntimeCommand,Disposition},queue::{self,Store,QueueActor},terminal::{TerminalActor,TerminalTarget,TerminalOptions}},terminal_input::*};
use serde_json::{Value,json};
use std::{sync::{Arc,atomic::AtomicU64},time::{Duration,SystemTime,UNIX_EPOCH}};
use tokio::{process::Command,sync::broadcast};
struct IsolatedMux(String);
impl Drop for IsolatedMux {fn drop(&mut self){let _=std::process::Command::new("tmux").args(["-L",&self.0,"kill-server"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();}}
#[tokio::test]
async fn terminal_runtime_tmux_isolated_persists_confirms_and_does_not_repeat() {
    let dir=tempfile::tempdir().unwrap(); let cli=dir.path().join("fake_cli.py"); let transcript=dir.path().join("chat.jsonl");
    std::fs::write(&cli,r#"import os,sys,tty,codecs,json,datetime
fd=sys.stdin.fileno(); tty.setraw(fd); text=''; decoder=codecs.getincrementaldecoder('utf-8')()
def render():
 sys.stdout.write('\x1b[2J\x1b[Hhistory\r\n'+'─'*40+'\r\n❯ '+text+'\r\n'+'─'*40+'\r\n⏵⏵ bypass permissions\r\n');sys.stdout.flush()
render()
while True:
 for c in decoder.decode(os.read(fd,4096)):
  if c=='\r':
   event={'type':'user','sessionId':'fake-conversation','uuid':str(datetime.datetime.now().timestamp()),'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'message':{'content':text}}
   with open(sys.argv[1],'a',encoding='utf-8') as f:f.write(json.dumps(event,ensure_ascii=False)+'\n')
   text=''
  elif c=='\x15':text=''
  else:text+=c
  render()
"#).unwrap();
    let label=format!("hangar-runtime-test-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()); let _mux=IsolatedMux(label.clone());
    let cli_command=format!("python3 '{}' '{}'",cli.display(),transcript.display());
    let output=Command::new("tmux").args(["-L",&label,"new-session","-d","-s","test","-x","100","-y","40",&cli_command]).env_remove("HANGAR_INTERNAL_SECRET").env_remove("HANGAR_RUNTIME_INSTANCE").env_remove("CP_AUTH_TOKEN").output().await.unwrap(); assert!(output.status.success());
    let meta=Command::new("tmux").args(["-L",&label,"display-message","-p","-t","=test:0.0","#{pane_id}\t#{session_created}"]).output().await.unwrap(); assert!(meta.status.success());
    let meta=String::from_utf8(meta.stdout).unwrap(); let mut fields=meta.trim().split('\t'); let pane=fields.next().unwrap().to_string(); let created=fields.next().unwrap().parse::<u64>().unwrap();
    let binding=TerminalBinding {name:"test".into(),pane:pane.clone(),conversation:"fake-conversation".into(),generation:1,created,mux_argv:vec!["tmux".into(),"-L".into(),label.clone()],windows:false,clipboard_lock_path:None};
    for _ in 0..100 {
        let capture=Command::new("tmux").args(["-L",&label,"capture-pane","-p","-t",&pane]).output().await.unwrap();
        if ComposerSnapshot::parse(&String::from_utf8_lossy(&capture.stdout)).is_some(){break;}tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let state_path=dir.path().join("state"); let policy_path=state_path.clone();
    let router=axum::Router::new().route("/internal/runtime/policy",axum::routing::post(move |body:String| {let (b,path)=(binding.clone(),policy_path.clone()); async move {
        let request:Value=serde_json::from_str(&body).unwrap(); let state:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        // Leitura de fatos não passa pelo diário.
        assert!(state["operations"][request["phase_id"].as_str().unwrap()].is_null()); assert_eq!(request["kind"],"terminal_facts");
        ([("content-type","application/json")],json!({"ok":true,"data":{"binding":b,"ready":true,"idle":true,"open_question":false,"plugin_live":false,"plugin_user":false,"native":null}}).to_string())
    }}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=listener.local_addr().unwrap(); let policy_server=tokio::spawn(async move{axum::serve(listener,router).await.unwrap()});
    let target=TerminalTarget {key:"isolated".into(),generation:1,name:"test".into(),binding:TerminalBinding {name:"test".into(),pane,conversation:"fake-conversation".into(),generation:1,created,mux_argv:vec!["tmux".into(),"-L".into(),label],windows:false,clipboard_lock_path:None},state_path:state_path.clone(),lease_path:dir.path().join("lease"),projection_dir:dir.path().join("projection"),transcript:transcript.clone(),created:created as f64,plugin_key:None};
    let lease=queue::acquire_lease(&target.lease_path).unwrap(); let store=Store::open(&target.state_path,&target.projection_dir,queue::State::new("isolated",1,"test",vec![])).unwrap();
    let options=TerminalOptions {io:Arc::new(ProcessIo::default()),limits:InputLimits {settle:Duration::from_millis(10),literal_settle:Duration::from_millis(25),multiline_settle:Duration::from_millis(25),slash_settle:Duration::from_millis(25),proof_attempts:40,ready_attempts:40,cleanup_attempts:4},tick:Duration::from_millis(100),stall_notice:Duration::from_secs(30),..TerminalOptions::default()};
    let handle=TerminalActor::spawn(target,QueueActor::start(store,lease),PolicyClient::new(address,"test".into(),"instance".into()),options,broadcast::channel(128).0,Arc::new(AtomicU64::new(0)));
    let command=RuntimeCommand {operation_id:"real-io".into(),kind:OperationKind::Input,payload:json!({"text":"ação 😀 C:\\Users\\test","pre_transcript":false})};
    assert_eq!(handle.command(command.clone()).await.unwrap().disposition,Disposition::Accepted); handle.command(command).await.unwrap(); handle.confirm().await.unwrap();
    let state:Value=serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap(); assert_eq!(state["rows"][0]["confirmed"],true);
    let contents=std::fs::read_to_string(transcript).unwrap(); assert_eq!(contents.lines().count(),1); let received:Value=serde_json::from_str(contents.trim()).unwrap(); assert_eq!(received["message"]["content"],"ação 😀 C:\\Users\\test");
    handle.stop().await.unwrap(); policy_server.abort();
}
