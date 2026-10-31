use hangar_server::update::git_engine::Service;
use std::{fs, path::Path, process::{Child, Command}, time::{Duration, Instant}};
fn service(root:&Path)->Service {Service::new(root.join("home"),root.join("repo"),root.join("claude"),Default::default())}
fn wait_file(path:&Path) {
    let deadline=Instant::now()+Duration::from_secs(15);
    while !path.exists() {assert!(Instant::now()<deadline,"arquivo esperado não apareceu: {}",path.display());std::thread::sleep(Duration::from_millis(2));}
}
fn child(root:&Path,index:usize)->Child {
    Command::new(std::env::current_exe().unwrap()).args(["--exact","update_lock_contract::update_turn_child","--ignored","--nocapture"])
        .env("HANGAR_TURN_PROOF_ROOT",root).env("HANGAR_TURN_PROOF_INDEX",index.to_string()).spawn().unwrap()
}
#[test]
#[ignore="Filho isolado da prova de exclusão entre processos."]
fn update_turn_child() {
    let root=std::path::PathBuf::from(std::env::var_os("HANGAR_TURN_PROOF_ROOT").unwrap());
    let index=std::env::var("HANGAR_TURN_PROOF_INDEX").unwrap();
    fs::write(root.join(format!("ready-{index}")),b"ready").unwrap();wait_file(&root.join("go"));
    let turn=service(&root).take_turn();
    let report=match &turn {Ok(Some(_))=>"acquired".to_owned(),Ok(None)=>"busy".to_owned(),Err(error)=>format!("error:{error}")};
    // O pai espera o arquivo existir: ele só pode aparecer já com o conteúdo inteiro.
    let partial=root.join(format!("report-{index}.tmp"));fs::write(&partial,report).unwrap();fs::rename(&partial,root.join(format!("report-{index}"))).unwrap();
    wait_file(&root.join("release"));drop(turn);
}
#[test]
fn update_turn_respects_persistent_process_guard() {
    let temp=tempfile::tempdir().unwrap();let service=service(temp.path());fs::create_dir_all(&service.state_dir).unwrap();
    let guard=fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(service.state_dir.join("rodando.guard")).unwrap();guard.lock().unwrap();
    let mut child=child(temp.path(),0);wait_file(&temp.path().join("ready-0"));fs::write(temp.path().join("go"),b"go").unwrap();
    wait_file(&temp.path().join("report-0"));let report=fs::read_to_string(temp.path().join("report-0")).unwrap();
    fs::write(temp.path().join("release"),b"release").unwrap();assert!(child.wait().unwrap().success());
    assert_eq!(report,"busy","a guarda cobre a janela de leitura e publicação");
    assert!(!service.state_dir.join("rodando.lock").exists());drop(guard);
}
#[test]
fn update_turn_has_one_winner_across_processes_and_keeps_guard_identity() {
    for stale in [false,true] {
        let temp=tempfile::tempdir().unwrap();let service=service(temp.path());fs::create_dir_all(&service.state_dir).unwrap();
        if stale {fs::write(service.state_dir.join("rodando.lock"),"999999999").unwrap();}
        let mut children=(0..6).map(|index|child(temp.path(),index)).collect::<Vec<_>>();
        for index in 0..6 {wait_file(&temp.path().join(format!("ready-{index}")));}
        fs::write(temp.path().join("go"),b"go").unwrap();
        let reports=(0..6).map(|index|{let path=temp.path().join(format!("report-{index}"));wait_file(&path);fs::read_to_string(path).unwrap()}).collect::<Vec<_>>();
        let owner=fs::read_to_string(service.state_dir.join("rodando.lock")).unwrap();
        fs::write(temp.path().join("release"),b"release").unwrap();for child in &mut children {assert!(child.wait().unwrap().success());}
        assert_eq!(reports.iter().filter(|v|*v=="acquired").count(),1,"stale={stale}: {reports:?}");
        assert!(reports.iter().all(|v|v=="acquired"||v=="busy"));assert!(owner.parse::<u32>().unwrap()>0);
        assert!(!service.state_dir.join("rodando.lock").exists());assert!(service.state_dir.join("rodando.guard").is_file());
    }
}

#[test]
fn update_turn_publication_failure_is_visible_and_never_overwrites_live_owner() {
    for kind in [std::io::ErrorKind::Unsupported,std::io::ErrorKind::PermissionDenied] {
        let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
        let result=service.take_turn_with_publisher(&|source,target| {
            assert_eq!(fs::read_to_string(source).unwrap(),std::process::id().to_string());
            assert!(!target.exists());Err(std::io::Error::new(kind,"fixture-publication"))
        });
        let error=match result {Err(error)=>error,_=>panic!("publicação recusada precisa retornar erro")};
        assert_eq!(error.kind(),kind);assert_eq!(error.to_string(),"update_lock_unavailable: fixture-publication");
        assert!(!service.state_dir.join("rodando.lock").exists());
        assert_eq!(fs::read_dir(&service.state_dir).unwrap().count(),1);
        fs::write(service.state_dir.join("rodando.lock"),std::process::id().to_string()).unwrap();
        assert!(service.take_turn_with_publisher(&|_,_|panic!("não pode publicar sobre dono vivo")).unwrap().is_none());
        assert_eq!(fs::read_to_string(service.state_dir.join("rodando.lock")).unwrap(),std::process::id().to_string());
    }
}
#[test]
fn update_turn_transfer_releases_guard_and_cleanup_checks_current_owner() {
    let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
    let turn=service.take_turn().unwrap().unwrap();let path=service.state_dir.join("rodando.lock");
    turn.transferred();assert!(path.exists());
    let guard=fs::OpenOptions::new().read(true).write(true).open(service.state_dir.join("rodando.guard")).unwrap();
    // Filho de outro teste entre o fork e o exec segura a cópia da guarda por instantes.
    let deadline=Instant::now()+Duration::from_secs(5);
    while guard.try_lock().is_err() {assert!(Instant::now()<deadline,"a guarda continuou travada depois da transferência");std::thread::sleep(Duration::from_millis(1));}
    drop(guard);
    fs::write(&path,"999999999").unwrap();service.release_turn().unwrap();assert_eq!(fs::read_to_string(&path).unwrap(),"999999999");
    fs::write(&path,std::process::id().to_string()).unwrap();service.release_turn().unwrap();assert!(!path.exists());
    assert!(service.state_dir.join("rodando.guard").exists());
}

#[test]
fn update_screen_cannot_reconcile_during_owner_transition() {
    let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
    service.state().try_write(serde_json::json!({"fase":"rodando","pid":999999999,"texto":"anterior"})).unwrap();
    let turn=service.take_turn().unwrap().unwrap();
    let state_before=fs::read(service.state_dir.join("estado.json")).unwrap();
    let owner_before=fs::read(service.state_dir.join("rodando.lock")).unwrap();
    let result=service.screen_state();
    assert_eq!(result["fase"],"rodando");
    assert_eq!(fs::read(service.state_dir.join("estado.json")).unwrap(),state_before);
    assert_eq!(fs::read(service.state_dir.join("rodando.lock")).unwrap(),owner_before);
    drop(turn);
}

#[test]
fn update_screen_preserves_new_live_owner_when_state_still_has_previous_pid() {
 let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
 service.state().try_write(serde_json::json!({"fase":"rodando","pid":999999999,"texto":"anterior"})).unwrap();
 let turn=service.take_turn().unwrap().unwrap();turn.transferred();
 let before=fs::read(service.state_dir.join("estado.json")).unwrap();
 assert_eq!(service.screen_state()["fase"],"rodando");
 assert_eq!(fs::read(service.state_dir.join("estado.json")).unwrap(),before);
 assert_eq!(fs::read_to_string(service.state_dir.join("rodando.lock")).unwrap(),std::process::id().to_string());
}

#[test]
fn update_checkout_reaper_never_finishes_another_owner_or_transition() {
 for occupied in [false,true] {
  let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
  service.state().try_write(serde_json::json!({"fase":"rodando","pid":null,"texto":"nova tentativa"})).unwrap();
  let turn=service.take_turn().unwrap().unwrap();
  let held=if occupied{
   fs::write(service.state_dir.join("rodando.lock"),"999999999").unwrap();Some(turn)
  }else{turn.transferred();None};
  let before=fs::read(service.state_dir.join("estado.json")).unwrap();
  hangar_server::update::executor::conclude_checkout_early_exit(&service.state_dir,999999999,Some(1),None);
  assert_eq!(fs::read(service.state_dir.join("estado.json")).unwrap(),before,"occupied={occupied}");
  drop(held);
 }
}

#[test]
fn update_checkout_reaper_finishes_only_its_published_owner() {
 let temp=tempfile::tempdir().unwrap();let service=service(temp.path());
 service.state().try_write(serde_json::json!({"fase":"rodando","pid":null})).unwrap();
 fs::write(service.state_dir.join("rodando.lock"),"999999999").unwrap();
 hangar_server::update::executor::conclude_checkout_early_exit(&service.state_dir,999999999,Some(7),None);
 let result=service.state().read();assert_eq!(result["fase"],"pronto");assert_eq!(result["ok"],false);
 assert_eq!(result["pid"],999999999_u32);assert!(result["erro"].as_str().unwrap().contains("saiu com 7"));
}
