//! Autorização do filho do atualizador, sem serviços reais, em qualquer plataforma.
use hangar_server::update::{executor::Spawner,git_engine::Service};
use std::sync::Mutex;
fn service(root:&std::path::Path,_name:&str)->Service {
 let checkout=root.join("repo");let home=root.join("home");
 std::fs::create_dir_all(checkout.join("backend")).unwrap();
 Service::new(home.clone(),checkout,home.join(".claude"),[("PATH".into(),String::new()),("HOME".into(),home.display().to_string())].into())
}

#[test]
#[ignore="Filho isolado que prova o portão anterior ao trabalho da atualização."]
fn update_launch_authorization_child() {
 let root=std::path::PathBuf::from(std::env::var_os("HANGAR_LAUNCH_PROOF_ROOT").unwrap());
 let nonce=std::env::var("HANGAR_LAUNCH_PROOF_NONCE").unwrap();
 let service=service(&root,"child");
 std::fs::write(root.join("child-started"),b"ready").unwrap();
 let allowed=service.await_launch_authorization(&nonce);
 if allowed.is_ok(){std::fs::write(root.join("work-started"),b"work").unwrap();}
 std::fs::write(root.join("child-result"),if allowed.is_ok(){"allowed"}else{"refused"}).unwrap();
}
struct RealChild<'a>{root:&'a std::path::Path,child:Mutex<Option<std::process::Child>>}
impl Spawner for RealChild<'_> {
 fn spawn(&self,args:&[String])->std::io::Result<u32>{
  let child=std::process::Command::new(std::env::current_exe()?).args(["--exact","update_launch_gate::update_launch_authorization_child","--ignored","--nocapture"])
   .env("HANGAR_LAUNCH_PROOF_ROOT",self.root).env("HANGAR_LAUNCH_PROOF_NONCE",&args[8]).spawn()?;
  let pid=child.id();*self.child.lock().unwrap()=Some(child);
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
  while !self.root.join("child-started").exists(){assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2));}
  assert!(!self.root.join("work-started").exists(),"filho não trabalha enquanto pai publica a posse");
  Ok(pid)
 }
}
#[test]
fn update_launch_child_requires_completed_publication_and_consumes_authorization(){
 for partial_failure in [false,true] {
  let temp=tempfile::tempdir().unwrap();let service=service(temp.path(),"child");
  let spawner=RealChild{root:temp.path(),child:Mutex::new(None)};
  let result=service.start_with_owner_writer(8765,None,false,&spawner,&|path,bytes|{
   std::fs::write(path,bytes)?;
   if partial_failure {Err(std::io::Error::other("fixture-after-write"))}else{Ok(())}
  });
  let mut child=spawner.child.lock().unwrap().take().unwrap();
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
  while child.try_wait().unwrap().is_none(){if std::time::Instant::now()>=deadline{child.kill().unwrap();panic!("filho não encerrou após decisão do pai");}std::thread::sleep(std::time::Duration::from_millis(2));}
  assert_eq!(result.is_err(),partial_failure);
  assert_eq!(temp.path().join("work-started").exists(),!partial_failure);
  assert_eq!(std::fs::read_to_string(temp.path().join("child-result")).unwrap(),if partial_failure{"refused"}else{"allowed"});
  assert!(!std::fs::read_dir(&service.state_dir).unwrap().any(|item|item.unwrap().file_name().to_string_lossy().ends_with(".ready")));
 }
}
#[test]
fn update_launch_missing_or_other_attempt_authorization_never_starts_work(){
 let temp=tempfile::tempdir().unwrap();let service=service(temp.path(),"missing");
 std::fs::create_dir_all(&service.state_dir).unwrap();
 // Pai morto antes de autorizar: o PID do filho pode estar publicado, mas não é suficiente.
 std::fs::write(service.state_dir.join("rodando.lock"),std::process::id().to_string()).unwrap();
 let start=std::time::Instant::now();assert!(service.await_launch_authorization("aa").is_err());
 assert!(start.elapsed()<std::time::Duration::from_secs(1));
 let other=service.state_dir.join(".launch-bb.ready");std::fs::write(&other,std::process::id().to_string()).unwrap();
 assert!(service.await_launch_authorization("aa").is_err());assert!(other.exists());
 assert!(service.await_launch_authorization("bb").is_ok());assert!(!other.exists());
 assert!(service.await_launch_authorization("bb").is_err(),"autorização não pode ser reutilizada");
}

#[test]
#[ignore="Pai isolado termina sem Drop para provar a queda anterior à autorização."]
fn update_launch_parent_exits_before_authorizing_child() {
 let root=std::path::PathBuf::from(std::env::var_os("HANGAR_LAUNCH_PROOF_ROOT").unwrap());
 let service=service(&root,"crash");let _turn=service.take_turn().unwrap().unwrap();
 let child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_launch_gate::update_launch_authorization_child","--ignored","--nocapture"])
  .env("HANGAR_LAUNCH_PROOF_ROOT",&root).env("HANGAR_LAUNCH_PROOF_NONCE","cc").spawn().unwrap();
 std::fs::write(service.state_dir.join("rodando.lock"),child.id().to_string()).unwrap();
 std::process::exit(0);
}
#[test]
fn update_launch_parent_death_leaves_child_without_authorization() {
 let temp=tempfile::tempdir().unwrap();
 let status=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","update_launch_gate::update_launch_parent_exits_before_authorizing_child","--ignored","--nocapture"])
  .env("HANGAR_LAUNCH_PROOF_ROOT",temp.path()).status().unwrap();assert!(status.success());
 let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
 while !temp.path().join("child-result").exists(){assert!(std::time::Instant::now()<deadline,"filho não recusou a queda do pai");std::thread::sleep(std::time::Duration::from_millis(2));}
 assert_eq!(std::fs::read_to_string(temp.path().join("child-result")).unwrap(),"refused");
 assert!(!temp.path().join("work-started").exists());
}
