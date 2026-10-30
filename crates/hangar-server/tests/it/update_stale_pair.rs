//! A versão publicada inclui a biblioteca imutável e os metadados do par.
use hangar_server::update::git_engine::{Service,releases::Release};
#[test]
fn update_stale_pair_matches_original_files_and_metadata() {
 let corpus:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/stale-pair.json")).unwrap();let mut differences=Vec::new();
 for row in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let home=temp.path().join("home");let dest=home.join(".hangar/bin");
  for(path,data)in row["files"].as_object().unwrap(){let path=dest.join(path);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,data.as_str().unwrap()).unwrap();}
  let service=Service::new(home,temp.path().join("repo"),temp.path().join("claude"),Default::default());let release=Release{url:"fixture".into(),tag:None,manifest:row["manifest"].clone()};let actual=service.stale_release(Some(&release),row["platform"].as_str());
  if actual!=row["stale"].as_bool().unwrap(){differences.push(format!("{} {}: {actual} != {}",row["platform"],row["mode"],row["stale"]));}
 }
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
