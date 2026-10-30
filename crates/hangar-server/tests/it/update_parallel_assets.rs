//! A preparação usa a mesma junção concorrente e o mesmo portão de publicação do driver.
use hangar_server::update::git_engine::{Service,releases::Held};
use serde_json::{Value,json};
use std::sync::{Mutex,atomic::{AtomicBool,Ordering},mpsc};
#[test]
fn update_parallel_assets_preserve_both_warnings_and_publish_after_screen(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/parallel-assets.json")).unwrap();
 for row in fixture["cases"].as_array().unwrap(){let temp=tempfile::tempdir().unwrap();let service=Service::new(temp.path().join("home"),temp.path().join("repo"),temp.path().join("claude"),Default::default());let name=row["name"].as_str().unwrap();let parallel=name.contains("juntos_e_guarda");let hold=name.starts_with("test_binario");let fails=row["fails"]==true||name.contains("explode");let destination=temp.path().join("hangar-server");let ready=AtomicBool::new(false);let published=AtomicBool::new(false);
  let (ui_tx,ui_rx)=mpsc::channel();let (bin_tx,bin_rx)=mpsc::channel();let bin_rx=Mutex::new(bin_rx);
  let result=service.prepare_assets(&||{if parallel{ui_tx.send(()).unwrap();bin_rx.lock().unwrap().recv_timeout(std::time::Duration::from_secs(2)).expect("binários precisam começar antes de a tela acabar");}if fails{return Err("build travou".into());}ready.store(true,Ordering::SeqCst);Ok(parallel.then(||"tela não atualizada: X".into()))},&|held|{if parallel{bin_tx.send(()).unwrap();ui_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("tela precisa começar antes de os binários acabarem");}assert!(!destination.exists());if hold{held.push(Held{data:b"fixture-server".to_vec(),target:destination.clone(),name:"hangar-server".into()});Some(vec![])}else{Some(vec!["binários: Y".into()])}},&|held|{assert!(ready.load(Ordering::SeqCst),"publicação antes da tela pronta");published.store(true,Ordering::SeqCst);for item in held{std::fs::write(&item.target,&item.data).unwrap();}vec![]});
  let expected=&row["calls"][0];assert_eq!(result.is_ok(),expected["ok"].as_bool().unwrap(),"{name}");if let Err(error)=result{assert_eq!(error,expected["error"].as_str().unwrap());}assert_eq!(service.state().read().get("avisos").cloned().unwrap_or(json!([])),expected["warnings"],"{name}");assert_eq!(published.load(Ordering::SeqCst),!fails);assert_eq!(destination.exists(),hold&&!fails);if destination.exists(){assert_eq!(std::fs::read(destination).unwrap(),b"fixture-server");}
 }
}
