use super::*;
use crate::{accounts::environment::AccountEnvironment,machine_peers::dotenv::{SettingsEnvironment,SettingsSnapshot}};
use std::{future::Future,pin::Pin};
use tokio::sync::Notify;
struct FakeRunner{scenario:String,events:Arc<Mutex<Vec<Value>>>,rounds:Arc<Mutex<Vec<Arc<Notify>>>>,cleanup:Arc<Notify>}
impl RoundRunner for FakeRunner{
 fn run(self:Arc<Self>,service:Arc<Service>,reason:String,force:bool,mut cancel:watch::Receiver<bool>)->Pin<Box<dyn Future<Output=Result<(),crate::codex_setup::phase::Error>>+Send>>{
  Box::pin(async move{
   let release=Arc::new(Notify::new());let index={let mut rounds=self.rounds.lock().unwrap();let index=rounds.len();rounds.push(release.clone());index};self.events.lock().unwrap().push(json!(["run",reason,force]));
   let cancelled=tokio::select!{biased;_=crate::codex_setup::cancellation::stopped(&mut cancel)=>true,_=release.notified()=>false};
   if cancelled{self.events.lock().unwrap().push(json!(["cancel",index]));}else{service.state.update(|v|v["estado"]=json!("ok"));}
   self.events.lock().unwrap().push(json!(["cleanup",index]));if self.scenario=="close-waits-cleanup"{self.cleanup.notified().await;}self.events.lock().unwrap().push(json!(["released",index]));
   if cancelled{Err(crate::codex_setup::phase::Error{kind:"CancelledError",message:"".into()})}else{Ok(())}
  })
 }
}
async fn until(rounds:&Mutex<Vec<Arc<Notify>>>,count:usize){tokio::time::timeout(Duration::from_secs(2),async{while rounds.lock().unwrap().len()<count{tokio::task::yield_now().await;}}).await.unwrap();}
#[tokio::test]
async fn codex_service_rounds_share_workers_force_followup_and_wait_cleanup(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/service-rounds.json")).unwrap();let mut differences=vec![];
 for case in fixture["cases"].as_array().unwrap(){
  let name=case["name"].as_str().unwrap();let root=tempfile::tempdir().unwrap();let home=root.path();std::fs::create_dir(home.join(".codex")).unwrap();let marker=home.join(".codex").join(PREPARED_MARKER);std::fs::write(&marker,"{}").unwrap();
  let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().to_string()),("USERPROFILE",home.to_string_lossy().to_string()),("PATH",String::new())]);let settings=SettingsSnapshot::capture(home,None,SettingsEnvironment::from_pairs(env.ordered_pairs())).unwrap();let config=RuntimeConfig::from_snapshot(env.clone(),Arc::new(settings)).unwrap();let events=Arc::new(Mutex::new(vec![]));let rounds=Arc::new(Mutex::new(vec![]));let cleanup=Arc::new(Notify::new());let runner=Arc::new(FakeRunner{scenario:name.into(),events:events.clone(),rounds:rounds.clone(),cleanup:cleanup.clone()});let service=Service::new_with_runner(AccountService::new(env),config,home.join("repo"),None,runner);
  let first=service.start("auto",false).await.unwrap();if first["estado"]!="executando"{differences.push(json!({"case":name,"initial":first}));continue;}until(&rounds,1).await;
  match name{
   "start-coalesces"=>{let second=service.start("config_sync",true).await.unwrap();let forced=service.rounds.lock().unwrap().as_ref().unwrap().forced;events.lock().unwrap().push(json!(["status",first["estado"],second["estado"],forced]));rounds.lock().unwrap()[0].notify_one();service.wait_current().await.unwrap();},
   "forced-after-auto"=>{let copy=service.clone();let waiter=tokio::spawn(async move{copy.update_and_wait("manual",true).await});tokio::task::yield_now().await;rounds.lock().unwrap()[0].notify_one();until(&rounds,2).await;rounds.lock().unwrap()[1].notify_one();let value=waiter.await.unwrap().unwrap();events.lock().unwrap().push(json!(["status",value["estado"]]));},
   "cancel-waiter"=>{let copy=service.clone();let waiter=tokio::spawn(async move{copy.update_and_wait("manual",true).await});tokio::task::yield_now().await;waiter.abort();assert!(waiter.await.unwrap_err().is_cancelled());events.lock().unwrap().push(json!(["waiter-cancelled",service.rounds.lock().unwrap().as_ref().unwrap().result.borrow().is_none()]));rounds.lock().unwrap()[0].notify_one();service.wait_current().await.unwrap();},
   "close-waits-cleanup"=>{let copy=service.clone();let closing=tokio::spawn(async move{copy.shutdown().await});tokio::time::timeout(Duration::from_secs(2),async{loop{if events.lock().unwrap().iter().any(|e|e[0]=="cleanup"){break}tokio::task::yield_now().await;}}).await.unwrap();events.lock().unwrap().push(json!(["close-pending",!closing.is_finished()]));cleanup.notify_one();closing.await.unwrap().unwrap();},
   "finished-starts-again"=>{rounds.lock().unwrap()[0].notify_one();service.wait_current().await.unwrap();service.start("config_sync",true).await.unwrap();until(&rounds,2).await;rounds.lock().unwrap()[1].notify_one();service.wait_current().await.unwrap();},_=>panic!("caso desconhecido")
  }
  let actual=json!({"events":events.lock().unwrap().clone(),"marker_removed":!marker.exists(),"rounds":rounds.lock().unwrap().len()});if actual!=case["result"]{differences.push(json!({"case":name,"actual":actual,"expected":case["result"]}));}service.shutdown().await.unwrap();
 }
 assert!(differences.is_empty(),"Coordenação divergente: {differences:#?}");
}

pub(super) fn isolated_service(home:&std::path::Path)->Arc<Service>{
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().to_string()),("USERPROFILE",home.to_string_lossy().to_string()),("PATH",String::new())]);
 let settings=SettingsSnapshot::capture(home,None,SettingsEnvironment::from_pairs(env.ordered_pairs())).unwrap();let config=RuntimeConfig::from_snapshot(env.clone(),Arc::new(settings)).unwrap();
 Service::new(AccountService::new(env),config,home.join("repo"),None)
}
#[tokio::test]
async fn codex_service_round_public_call_reports_missing_source_without_python(){
 let root=tempfile::tempdir().unwrap();std::fs::create_dir(root.path().join(".codex")).unwrap();let service=isolated_service(root.path());
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/pipeline-round.json")).unwrap();let expected=&fixture["cases"].as_array().unwrap().iter().find(|case|case["input"]["name"]=="missing-source").unwrap()["result"];
 assert_eq!(service.start("config_sync",true).await.unwrap()["estado"],"executando");service.wait_current().await.unwrap();assert_eq!(&service.status().unwrap(),expected);service.shutdown().await.unwrap();
}
struct FailedRunner;
impl RoundRunner for FailedRunner{
 fn run(self:Arc<Self>,_:Arc<Service>,_:String,_:bool,_:watch::Receiver<bool>)->Pin<Box<dyn Future<Output=Result<(),crate::codex_setup::phase::Error>>+Send>>{Box::pin(async{Err(crate::codex_setup::phase::Error{kind:"AccountError",message:"account_busy".into()})})}
}
#[tokio::test]
async fn codex_service_round_worker_failure_does_not_leave_executando(){
 let root=tempfile::tempdir().unwrap();let mut service=isolated_service(root.path());Arc::get_mut(&mut service).unwrap().runner=Some(Arc::new(FailedRunner));
 service.start("config_sync",true).await.unwrap();assert_eq!(service.wait_current().await.unwrap_err(),"account_busy");
 let status=service.status().unwrap();assert_eq!(status["estado"],"erro");assert!(!status["erros"].as_array().unwrap().is_empty());service.shutdown().await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn codex_service_review_force_racing_automatic_start_keeps_forced_followup(){
 let root=tempfile::tempdir().unwrap();let mut service=isolated_service(root.path());
 let events=Arc::new(Mutex::new(vec![]));let rounds=Arc::new(Mutex::new(vec![]));
 let runner=Arc::new(FakeRunner{scenario:"race".into(),events:events.clone(),rounds:rounds.clone(),cleanup:Arc::new(Notify::new())});
 let entered=Arc::new(std::sync::Barrier::new(2));let release=Arc::new(std::sync::Barrier::new(2));
 let hook_entered=entered.clone();let hook_release=release.clone();let once=std::sync::atomic::AtomicBool::new(false);
 let mutable=Arc::get_mut(&mut service).unwrap();mutable.runner=Some(runner);mutable.before_start=Some(Arc::new(move|reason|{if reason=="manual"&&!once.swap(true,std::sync::atomic::Ordering::SeqCst){hook_entered.wait();hook_release.wait();}}));
 let copy=service.clone();let waiter=tokio::spawn(async move{copy.update_and_wait("manual",true).await});
 tokio::task::spawn_blocking(move||entered.wait()).await.unwrap();
 service.start("auto",false).await.unwrap();until(&rounds,1).await;
 tokio::task::spawn_blocking(move||release.wait()).await.unwrap();
 tokio::task::yield_now().await;rounds.lock().unwrap()[0].notify_one();
 let followed=tokio::time::timeout(Duration::from_secs(2),async{while rounds.lock().unwrap().len()<2{tokio::task::yield_now().await;}}).await.is_ok();
 if followed{rounds.lock().unwrap()[1].notify_one();}
 let outcome=waiter.await.unwrap();service.shutdown().await.unwrap();
 assert!(followed,"A chamada forçada perdeu a rodada após compartilhar a automática: {:?}",events.lock().unwrap());
 assert_eq!(outcome.unwrap()["estado"],"ok");assert_eq!(events.lock().unwrap().iter().filter(|v|v[0]=="run").cloned().collect::<Vec<_>>(),vec![json!(["run","auto",false]),json!(["run","manual",true])]);
}
