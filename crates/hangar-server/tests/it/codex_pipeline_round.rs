use hangar_server::codex_setup::{files,phase::{Error,State},pipeline_round::{self,Actions,Context},status};
use serde_json::{Value,json};
struct Fake{scenario:String,events:Vec<Value>}
impl Actions for Fake{
 fn now(&self)->f64{100.0}
 fn available(&self)->bool{true}
 fn fingerprint(&mut self,sources:bool,phase:Option<&str>,_:Option<&Value>)->Result<String,Error>{
  let prefix=if sources{"source-"}else{"result-"};if let Some(phase)=phase{return Ok(format!("{prefix}{phase}"))}let values=Value::Object(["instructions","plugins","fragments","skills"].into_iter().map(|phase|(phase.into(),json!(format!("{prefix}{phase}")))).collect());Ok(files::hash(&files::bytes(&values).unwrap()))
 }
 async fn execute(&mut self,phase:&str,_:&mut Value,_:&State,force:bool)->Result<(),Error>{
  match phase{
   "instructions"=>{self.events.push(json!(["instructions",force]));if self.scenario=="unexpected-error"{return Err(Error{kind:"TypeError",message:"causa técnica token=segredo-sintetico".into()})}if self.scenario=="instructions-error"{return Err(Error{kind:"ValueError",message:"causa técnica token=segredo-sintetico".into()})}self.events.extend([json!(["legacy"]),json!(["hooks"])]);},
   "plugins"=>{self.events.push(json!(["plugins",force]));if self.scenario=="plugins-error"{return Err(Error{kind:"RuntimeError",message:"fixture".into()})}},
   "fragments"=>{self.events.extend([json!(["config"]),json!(["fragments"])]);if self.scenario=="fragments-error"{return Err(Error{kind:"ValueError",message:"causa técnica token=segredo-sintetico".into()})}if self.scenario=="cancelled"{return Err(Error{kind:"CancelledError",message:"".into()})}self.events.push(json!(["files"]));},
   "skills"=>self.events.push(json!(["skills"])),_=>panic!("fase desconhecida")
  }Ok(())
 }
 async fn open(&mut self)->Result<(),Error>{self.events.push(json!(["open"]));Ok(())}
 async fn close(&mut self)->Result<(),Error>{self.events.push(json!(["close"]));Ok(())}
 async fn trust(&mut self,_:&State)->Result<(),Error>{self.events.push(json!(["trust"]));Ok(())}
}
#[tokio::test]
async fn codex_service_pipeline_round_matches_original_phase_order_errors_and_checkpoints(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/pipeline-round.json")).unwrap();let mut differences=vec![];
 for case in fixture["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let account=home.join(".codex");std::fs::create_dir_all(&account).unwrap();let input=&case["input"];let name=input["name"].as_str().unwrap();if name!="missing-source"{std::fs::create_dir_all(home.join(".claude")).unwrap();std::fs::write(home.join(".claude/settings.json"),"{}").unwrap();}let work=status::root(&home,&account).unwrap();
  if name=="cached"{std::fs::create_dir_all(&work).unwrap();let phases=Value::Object(["instructions","plugins","fragments","skills"].into_iter().map(|p|(p.into(),json!({"fingerprint":format!("result-{p}")}))).collect());std::fs::write(work.join("estado.json"),files::bytes(&json!({"phases":phases,"marketplaces_em":100})).unwrap()).unwrap();}
  let state=State::new(status::snapshot());let(_sender,cancel)=tokio::sync::watch::channel(false);let context=Context{home:&home,account:&account,work:&work,state:&state,force:input["force"].as_bool().unwrap(),memory:false,cancel};let mut actions=Fake{scenario:name.into(),events:vec![]};let result=pipeline_round::run(&context,&mut actions).await;let(value,error)=match result{Ok(value)=>(value,Value::Null),Err(e)=>(status::read(&home,&account,Some(&state.snapshot()),100.0).unwrap(),json!(e.kind))};let record=files::object(&work.join("estado.json")).unwrap();let actual=json!({"input":input,"result":value,"error":error,"events":actions.events,"record":record});if actual!=*case{differences.push(json!({"actual":actual,"expected":case}));}
 }
 assert!(differences.is_empty(),"Rodada divergente: {differences:#?}");
}

#[test]
fn codex_service_review_deep_registry_replacement_is_stack_safe(){
 for field in ["status","fingerprint"]{
  let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["codex_service_review_deep_child","--ignored"])
   .env("HANGAR_CODEX_DEEP_FIELD",field).env("RUST_MIN_STACK","262144").output().unwrap();
  assert!(output.status.success(),"{field}: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
  assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
 }
}
#[tokio::test]
#[ignore="Filho isolado para detectar estouro da pilha sem encerrar o executor"]
async fn codex_service_review_deep_child(){
 let field=std::env::var("HANGAR_CODEX_DEEP_FIELD").unwrap();let temp=tempfile::tempdir().unwrap();let home=temp.path();let account=home.join(".codex");std::fs::create_dir_all(&account).unwrap();std::fs::create_dir(home.join(".claude")).unwrap();std::fs::write(home.join(".claude/settings.json"),"{}").unwrap();let work=status::root(home,&account).unwrap();std::fs::create_dir_all(&work).unwrap();
 let deep=format!("{}0{}","[".repeat(10000),"]".repeat(10000));let raw=if field=="status"{format!("{{\"status\":{{\"unknown\":{deep}}}}}")}else{format!("{{\"fingerprint\":{deep}}}")};std::fs::write(work.join("estado.json"),raw).unwrap();
 let state=State::new(status::snapshot());let(_,cancel)=tokio::sync::watch::channel(false);let context=Context{home,account:&account,work:&work,state:&state,force:true,memory:false,cancel};let mut actions=Fake{scenario:"instructions-error".into(),events:vec![]};let response=pipeline_round::run(&context,&mut actions).await.unwrap();assert_eq!(response["estado"],"erro");hangar_server::orchestration::codec::discard(response);
 let record=files::object(&work.join("estado.json")).unwrap();assert_eq!(record["status"]["estado"],"erro");assert!(record["fingerprint"].is_string());hangar_server::orchestration::codec::discard(record);
}
#[tokio::test]
async fn codex_service_review_failure_keeps_private_diagnostic_without_secret(){
 use tracing::instrument::WithSubscriber;
 #[derive(Clone)]struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
 impl std::io::Write for Capture{fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())}fn flush(&mut self)->std::io::Result<()>{Ok(())}}
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/pipeline-diagnostics.json")).unwrap();
 for case in fixture["cases"].as_array().unwrap(){
  let scenario=case["name"].as_str().unwrap();let expected=case["logs"].as_array().unwrap().last().unwrap();let kind=expected["kind"].as_str().unwrap();
  let temp=tempfile::tempdir().unwrap();let home=temp.path();let account=home.join(".codex");std::fs::create_dir_all(&account).unwrap();std::fs::create_dir(home.join(".claude")).unwrap();std::fs::write(home.join(".claude/settings.json"),"{}").unwrap();let work=status::root(home,&account).unwrap();
  let state=State::new(status::snapshot());let(_,cancel)=tokio::sync::watch::channel(false);let context=Context{home,account:&account,work:&work,state:&state,force:true,memory:false,cancel};let mut actions=Fake{scenario:scenario.into(),events:vec![]};
  let capture=Capture(Default::default());let writer=capture.clone();let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move||writer.clone()).finish();
  let response=pipeline_round::run(&context,&mut actions).with_subscriber(subscriber).await.unwrap();assert_eq!(response["estado"],"erro");assert!(!response.to_string().contains("causa técnica"));
  let logs=String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();assert!(logs.contains("causa técnica")&&logs.contains(kind),"Diagnóstico ausente: {logs}");assert!(!logs.contains("segredo-sintetico"),"Credencial vazou no diagnóstico");assert!(logs.contains(expected["message"].as_str().unwrap()));assert!(logs.contains(if expected["level"]=="warning"{"WARN"}else{"ERROR"}));
 }
}
