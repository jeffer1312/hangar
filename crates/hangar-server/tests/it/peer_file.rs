use serde_json::Value;
fn fixture()->Value{let raw=if std::env::var_os("HANGAR_PEER_EXPOSED").is_some(){include_str!("../../../../backend/tests/fixtures/contract/peers/exposed-fields.json")}else if std::env::var_os("HANGAR_PEER_REPLACEMENTS").is_some(){include_str!("../../../../backend/tests/fixtures/contract/peers/file-replacements.json")}else{include_str!("../../../../backend/tests/fixtures/contract/peers/file-http.json")};serde_json::from_str(raw).unwrap()}
#[test]
fn peer_file_http_preserves_permissive_json_and_atomic_effects(){
 let mut failures=vec![];
 for(index,row)in fixture()["cases"].as_array().unwrap().iter().enumerate(){
  let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","peer_file::peer_file_child","--ignored","--nocapture"]).env("HANGAR_PEER_FILE_CASE",index.to_string()).output().unwrap();
  if !output.status.success(){failures.push(format!("{} {} {}: {} {}",row["name"],row["method"],row["path"],String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr)));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
#[tokio::test]
#[ignore="documentos profundos executados em subprocessos isolados"]
async fn peer_file_child(){
 use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},auth::TrustedHosts,config::Config,routes::AppState};
 use std::sync::Arc;
 let cases=fixture();let index:usize=std::env::var("HANGAR_PEER_FILE_CASE").unwrap().parse().unwrap();let row=&cases["cases"][index];
 let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();let book=config.join("peers.json");std::fs::write(&book,row["initial"].as_str().unwrap()).unwrap();std::fs::write(config.join(".env"),"").unwrap();std::fs::write(config.join("peers.json.fixture.tmp"),"synthetic").unwrap();
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let mut state=AppState::new(Config{listen:address,upstream:"127.0.0.1:1".parse().unwrap(),internal_secret:"fixture-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
 let app=hangar_server::routes::router(Arc::new(state));let server=tokio::spawn(async move{axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
 let mut request=reqwest::Client::new().request(row["method"].as_str().unwrap().parse().unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap())).bearer_auth("contract-owner");if !row["body"].is_null(){request=request.json(&row["body"]);}if let Some(wire)=row["wire_body"].as_str(){request=request.header("content-type","application/json").body(wire.to_owned());}
 let response=request.send().await.unwrap();let status=response.status().as_u16();let body=response.text().await.unwrap();server.abort();let complete=&row["approved_complete_write"];
 assert_eq!(status,if complete.is_object(){200}else{row["status"].as_u64().unwrap() as u16});
 if status==500{assert_eq!(body,row["text"].as_str().unwrap());}else{
 let actual:Value=serde_json::from_str(&body).unwrap();let mut expected:Value=if complete.is_object(){complete["response"].clone()}else{serde_json::from_str(row["text"].as_str().unwrap()).unwrap()};
 if row["name"]=="app-surrogate" || (row["name"]=="surrogate"&&row["method"]=="POST") || (row["name"]=="app-\"\\ud800\""&&status==503){expected["detail"]["msg"]=Value::String("texto contém Unicode inválido".into());}
 assert_eq!(actual,expected);
 }
 let effect=std::fs::read(&book).unwrap();let effect_source=if complete.is_object(){complete}else{row};let digest=ring::digest::digest(&ring::digest::SHA256,&effect).as_ref().iter().map(|byte|format!("{byte:02x}")).collect::<String>();assert_eq!(digest,effect_source["effect_sha256"]);assert_eq!(effect.len() as u64,effect_source["effect_len"].as_u64().unwrap());
 let mut temps:Vec<_>=std::fs::read_dir(&config).unwrap().flatten().map(|entry|entry.file_name().to_string_lossy().into_owned()).filter(|name|name.starts_with("peers.json.")&&name.ends_with(".tmp")).collect();temps.sort();assert_eq!(serde_json::json!(temps),row["remaining_tmp"]);
}

#[test]
fn peer_file_replacements_discard_deep_values_without_aborting(){
 let mut failures=vec![];
 for index in 0..11 {
  let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","peer_file::peer_file_child","--ignored","--nocapture"]).env("HANGAR_PEER_REPLACEMENTS","1").env("HANGAR_PEER_FILE_CASE",index.to_string()).output().unwrap();
  if !output.status.success(){failures.push(format!("caso {index}: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr)));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
#[test]
fn peer_file_cli_rejects_the_unconsumed_json_command(){
 let home=tempfile::tempdir().unwrap();let path=home.path().join("peers.json");std::fs::write(&path,r#"{"extra":NaN,"surrogate":"\ud800"}"#).unwrap();
 let(mut out,mut err)=(vec![],vec![]);let code=hangar_server::machine_peers::cli::run(&path,&["json".into()],&mut out,&mut err);
 assert_eq!(code,2);assert!(out.is_empty());assert_eq!(String::from_utf8(err).unwrap(),"uso: hangar-server peers path|config <id>|tsv\n");
}
#[test]
fn peer_file_exposed_fields_preserve_typed_responses(){
 let mut failures=vec![];
 for index in 0..56 {
  let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","peer_file::peer_file_child","--ignored","--nocapture"]).env("HANGAR_PEER_EXPOSED","1").env("HANGAR_PEER_FILE_CASE",index.to_string()).output().unwrap();
  if !output.status.success(){failures.push(format!("caso {index}: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr)));}
 }
 assert!(failures.is_empty(),"{}",failures.join("\n"));
}
