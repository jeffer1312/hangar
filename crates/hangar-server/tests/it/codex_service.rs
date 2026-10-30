use hangar_server::{accounts::{AccountService,AccountKey,Provider,GuardMode,environment::AccountEnvironment,preparation::PREPARED_MARKER},codex_setup::{service::Service,repair::Actions,files,status},config_sync::runtime::RuntimeConfig,machine_peers::dotenv::{SettingsEnvironment,SettingsSnapshot}};
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},sync::Arc,time::Duration};
use tokio::sync::Notify;
fn service(home:&Path)->(AccountService,Arc<Service>){
 let env=AccountEnvironment::from_pairs([("HOME",home.to_string_lossy().to_string()),("USERPROFILE",home.to_string_lossy().to_string()),("PATH",String::new())]);
 let settings=SettingsSnapshot::capture(home,None,SettingsEnvironment::from_pairs(env.ordered_pairs())).unwrap();
 let config=RuntimeConfig::from_snapshot(env.clone(),Arc::new(settings)).unwrap();
 let accounts=AccountService::new(env);
 let service=Service::new(accounts.clone(),config,home.join("repo"),None);(accounts,service)
}
struct ActionsFixture{outcome:Value,entered:Option<Arc<Notify>>,release:Option<Arc<Notify>>,install_failed:bool,native_install:Option<PathBuf>}
impl Actions for ActionsFixture{
 async fn install(&mut self,account:&Path)->Result<(),String>{
  if self.install_failed{return Err("fixture hooks".into());}
  if let Some(repo)=&self.native_install {hangar_server::harness_setup::codex_hooks::sync_checked(account,repo,cfg!(windows))?;}
  else {std::fs::write(account.join("hooks.json"),b"installed").unwrap();}
  Ok(())
 }
 fn available(&self)->bool{!self.outcome.is_null()}
 async fn plugin(&mut self,_root:&Path,_backups:&Path)->Result<(),String>{Ok(())}
 async fn open(&mut self)->Result<(),String>{Ok(())}
 async fn request(&mut self,method:&str,_params:Value)->Result<Value,String>{
  if let Some(entered)=self.entered.take(){entered.notify_one();if let Some(release)=&self.release{release.notified().await;}}
  if self.outcome=="error"{return Err("fixture repair".into());}
  if method=="hooks/list"&&self.outcome==true {Ok(json!({"data":[{"hooks":[{"enabled":true,"trustStatus":"untrusted","source":"project","key":"fixture:hook","currentHash":"hash"}]}]}))}else{Ok(json!({"data":[]}))}
 }
 async fn close(&mut self)->Result<(),String>{Ok(())}
}
fn ready(accounts:&AccountService,account:&hangar_server::accounts::catalog::Account,value:bool){
 let path=if account.is_default{status::root(&accounts.env.home,&account.home).unwrap().join("estado.json")}else{
  let key=AccountKey::new(Provider::Codex,&account.home).unwrap();accounts.locks.sidecar(&key,"prepare-result.json").unwrap()
 };
 std::fs::create_dir_all(path.parent().unwrap()).unwrap();
 let data=if account.is_default{json!({"status":{"estado":if value{"ok"}else{"parcial"}}})}else{json!({"status":if value{"ready"}else{"partial"}})};
 std::fs::write(path,files::bytes(&data).unwrap()).unwrap();
}
#[tokio::test]
async fn codex_service_repair_matches_original_adoption_and_changed_account(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/repair-lifecycle.json")).unwrap();
 let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let (accounts,service)=service(temp.path());
  let mut account=if case["default"]==true{accounts.resolve(Provider::Codex,"default").unwrap()}else{accounts.create(Provider::Codex,"fixture",|_|Ok(())).unwrap()};
  std::fs::create_dir_all(&account.home).unwrap();ready(&accounts,&account,case["ready"]==true);
  let marker=account.home.join(PREPARED_MARKER);
  if case["outcome"]=="changed"{account.home=temp.path().join("changed");}
  let actions=ActionsFixture{outcome:case["outcome"].clone(),entered:None,release:None,install_failed:false,native_install:None};
  let actual=match service.repair_with(account,case["adopt"]==true,actions).await{Ok(value)=>json!({"ok":value}),Err(message)=>json!({"message":message})};
  let expected=if case["result"].get("error").is_some(){json!({"message":case["result"]["message"]})}else{case["result"].clone()};
  let marked=std::fs::read_to_string(&marker).ok();
  if actual!=expected||json!(marked)!=case["marker"] {failures.push(json!({"case":case,"actual":actual,"marker":marked}));}
  service.shutdown().await.unwrap();
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
#[tokio::test]
async fn codex_service_repair_failure_cannot_adopt_prepared_marker(){
 let temp=tempfile::tempdir().unwrap();let(accounts,service)=service(temp.path());let account=accounts.resolve(Provider::Codex,"default").unwrap();
 std::fs::create_dir_all(&account.home).unwrap();ready(&accounts,&account,true);
 let result=service.repair_with(account.clone(),true,ActionsFixture{outcome:json!(false),entered:None,release:None,install_failed:true,native_install:None}).await;
 assert_eq!(result,Err("fixture hooks".into()));
 assert!(!account.home.join(PREPARED_MARKER).exists());
 assert!(!account.home.join("hooks.json").exists());
 service.shutdown().await.unwrap();
}
#[tokio::test]
async fn codex_service_repair_retains_both_guards_after_waiter_cancellation(){
 let temp=tempfile::tempdir().unwrap();let(accounts,service)=service(temp.path());let account=accounts.resolve(Provider::Codex,"default").unwrap();
 std::fs::create_dir_all(&account.home).unwrap();ready(&accounts,&account,true);
 let key=AccountKey::new(Provider::Codex,&account.home).unwrap();let entered=Arc::new(Notify::new());let release=Arc::new(Notify::new());
 let worker=service.clone();let target=account.clone();let signal=entered.clone();let wait=release.clone();
 let caller=tokio::spawn(async move{worker.repair_with(target,true,ActionsFixture{outcome:json!(false),entered:Some(signal),release:Some(wait),install_failed:false,native_install:None}).await});
 let began=tokio::time::timeout(Duration::from_secs(2),entered.notified()).await.is_ok();
 if !began{release.notify_one();caller.abort();let _=caller.await;panic!("o reparo não entrou no pedido nativo");}
 caller.abort();let _=caller.await;
 let lifecycle_busy=accounts.locks.try_acquire(&key,GuardMode::Exclusive).is_err();
 let integration=std::fs::OpenOptions::new().read(true).write(true).open(account.home.join(".hangar-integracao.lock")).unwrap();
 let integration_busy=integration.try_lock().is_err();
 release.notify_one();service.shutdown().await.unwrap();
 assert!(lifecycle_busy&&integration_busy,"as guardas foram liberadas antes da mutação terminar");
 assert!(accounts.locks.try_acquire(&key,GuardMode::Exclusive).is_ok());
 assert!(integration.try_lock().is_ok());
 assert_eq!(std::fs::read(account.home.join(PREPARED_MARKER)).unwrap(),files::bytes(&json!({"version":1})).unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn codex_service_native_hooks_failures_do_not_publish_original_false_readiness(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/repair-hook-failure.json")).unwrap();
 let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  assert_eq!(case["result"],json!({"ok":false}));
  assert_eq!(serde_json::from_str::<Value>(case["marker"].as_str().unwrap()).unwrap(),json!({"version":1}));
  let temp=tempfile::tempdir().unwrap();let(accounts,service)=service(temp.path());let account=accounts.resolve(Provider::Codex,"default").unwrap();
  std::fs::create_dir_all(&account.home).unwrap();ready(&accounts,&account,true);
  let source=case["input_hex"].as_str().unwrap();let raw:Vec<u8>=(0..source.len()).step_by(2).map(|i|u8::from_str_radix(&source[i..i+2],16).unwrap()).collect();
  let hook=account.home.join("hooks.json");std::fs::write(&hook,&raw).unwrap();
  let repo=temp.path().join("repo");let python=repo.join("python/bin/python3");std::fs::create_dir_all(python.parent().unwrap()).unwrap();std::fs::write(python,[]).unwrap();std::fs::create_dir_all(repo.join("backend/hooks")).unwrap();
  let result=service.repair_with(account.clone(),true,ActionsFixture{outcome:json!(false),entered:None,release:None,install_failed:false,native_install:Some(repo)}).await;
  let valid=matches!(case["name"].as_str(),Some("python-space"|"empty"|"valid"));
  let expected=if valid{Ok(Some(false))}else{Err(if case["name"]=="utf8"{"hooks.json não está em UTF-8"}else{"hooks.json inválido; hooks preservados"}.into())};
  let final_bytes=std::fs::read(&hook).unwrap();let normalized=String::from_utf8_lossy(&final_bytes).replace(&*temp.path().to_string_lossy(),"$ROOT");
  let final_hex:String=normalized.as_bytes().iter().map(|b|format!("{b:02x}")).collect();
  let marked=account.home.join(PREPARED_MARKER).exists();
  if result!=expected || marked!=valid || (valid&&json!(final_hex)!=case["final_hex"]) || (!valid&&final_bytes!=raw){failures.push(json!({"name":case["name"],"actual":format!("{result:?}"),"expected":format!("{expected:?}"),"marked":marked}));}
  assert_eq!(std::fs::read(temp.path().join(".hangar/codex-prepare-once/backups/default/hooks.json")).unwrap(),raw);
  service.shutdown().await.unwrap();
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
#[tokio::test]
async fn codex_service_review_shutdown_survives_concurrent_waiters_and_cancellation(){
 let temp=tempfile::tempdir().unwrap();let(accounts,service)=service(temp.path());let account=accounts.resolve(Provider::Codex,"default").unwrap();std::fs::create_dir_all(&account.home).unwrap();ready(&accounts,&account,true);
 let key=AccountKey::new(Provider::Codex,&account.home).unwrap();let entered=Arc::new(Notify::new());let release=Arc::new(Notify::new());let worker=service.clone();let target=account.clone();let signal=entered.clone();let wait=release.clone();
 let repair=tokio::spawn(async move{worker.repair_with(target,true,ActionsFixture{outcome:json!(false),entered:Some(signal),release:Some(wait),install_failed:false,native_install:None}).await});
 tokio::time::timeout(Duration::from_secs(2),entered.notified()).await.unwrap();
 let mut first=Box::pin(service.shutdown());assert!(tokio::time::timeout(Duration::from_millis(30),&mut first).await.is_err());
 let concurrent_waited=tokio::time::timeout(Duration::from_millis(30),service.shutdown()).await.is_err();drop(first);
 let later_waited=tokio::time::timeout(Duration::from_millis(30),service.shutdown()).await.is_err();
 let lifecycle_busy=accounts.locks.try_acquire(&key,GuardMode::Exclusive).is_err();let integration=std::fs::OpenOptions::new().read(true).write(true).open(account.home.join(".hangar-integracao.lock")).unwrap();let integration_busy=integration.try_lock().is_err();
 release.notify_one();repair.await.unwrap().unwrap();service.shutdown().await.unwrap();
 assert!(concurrent_waited,"segundo shutdown declarou sucesso com trabalho ativo");assert!(later_waited,"cancelar o primeiro aguardante perdeu a conclusão compartilhada");assert!(lifecycle_busy&&integration_busy);assert!(accounts.locks.try_acquire(&key,GuardMode::Exclusive).is_ok());assert!(integration.try_lock().is_ok());
}
