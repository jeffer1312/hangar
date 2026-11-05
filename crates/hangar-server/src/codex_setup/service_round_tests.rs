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
fn isolated_service(home:&std::path::Path)->Arc<Service>{
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().to_string()),("USERPROFILE",home.to_string_lossy().to_string()),("PATH",String::new())]);
 let settings=SettingsSnapshot::capture(home,None,SettingsEnvironment::from_pairs(env.ordered_pairs())).unwrap();let config=RuntimeConfig::from_snapshot(env.clone(),Arc::new(settings)).unwrap();
 Service::new(AccountService::new(env),config,home.join("repo"),None)
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
