//! Correlação e causas do transporte de registro sem registrar corpo ou credencial.
use hangar_server::groups::peers::{PeerBook,PeerClient};
use serde_json::{Value,json};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
#[tokio::test]
async fn peer_diagnostic_matches_original_without_credentials() {
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/diagnostic.json")).unwrap();
 for case in fixture["cases"].as_array().unwrap() {
  let name=case["name"].as_str().unwrap().to_owned();let temp=tempfile::tempdir().unwrap();let diary=temp.path().join("diary");let book=temp.path().join("peers.json");
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let kind=name.clone();
  let server=tokio::spawn(async move {
   let(mut socket,_)=listener.accept().await.unwrap();let mut raw=vec![];let mut block=[0;1024];
   loop {let n=socket.read(&mut block).await.unwrap();raw.extend_from_slice(&block[..n]);if n==0 || raw.windows(4).any(|w|w==b"\r\n\r\n"){break;}}
   let request=String::from_utf8_lossy(&raw).into_owned();
   if kind=="transporte" {tokio::time::sleep(std::time::Duration::from_secs(9)).await;}else{
    let(status,body)=if kind=="http"{("403 Forbidden","SECRET")}else if kind=="json"{("200 OK","SECRET")}else{("200 OK","{\"token\":\"SECRET\"}")};
    let response=format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());let _=socket.write_all(response.as_bytes()).await;
   }
   request
  });
  std::fs::write(&book,json!({"outro":{"base_url":format!("http://{address}"),"token":"SECRET"}}).to_string()).unwrap();
  let client=PeerClient::new(PeerBook::new(Some(book))).with_diagnostics(diary.clone(),"req-diagnostico".into());
  let result=client.call("outro",reqwest::Method::POST,"/api/pair",Some(&json!({"texto":"SECRET"}))).await;
  if name=="ok" {assert_eq!(result.unwrap(),Some(json!({"token":"SECRET"})));} else {assert_eq!(result.unwrap_err().is_transport(),case["error"]["transport"].as_bool().unwrap());}
  let request=server.await.unwrap();assert!(request.to_lowercase().contains("x-hangar-req: req-diagnostico"),"{name}: falta correlação");
  let mut events=vec![];if let Ok(files)=std::fs::read_dir(&diary){for file in files {for line in std::fs::read_to_string(file.unwrap().path()).unwrap().lines(){events.push(serde_json::from_str::<Value>(line).unwrap());}}}
  assert!(!serde_json::to_string(&events).unwrap().contains("SECRET"));let mut operation=None;
  for event in &mut events {for key in ["ts","backend","pid_backend","origem","inicio_backend"]{event.as_object_mut().unwrap().remove(key);}if event.get("ms").is_some(){assert!(event["ms"].as_u64().is_some());event["ms"]=json!("<MS>");}let id=event["operacao"].as_str().unwrap().to_owned();assert!(!id.is_empty());if let Some(previous)=&operation{assert_eq!(&id,previous);}else{operation=Some(id);}event["operacao"]=json!("<OPERATION>");}
  let mut expected=case["events"].as_array().unwrap().clone();for event in &mut expected{event.as_object_mut().unwrap().remove("inicio_backend");}
  if name=="transporte" {assert_eq!(expected[2]["causa_tipo"],"URLError");expected[2]["causa_tipo"]=json!("TimeoutError");}
  if name=="json" {assert_eq!(expected[2]["causa_tipo"],"StopIteration");expected[2]["causa_tipo"]=json!("JSONDecodeError");}
  assert_eq!(events,expected,"{name}");
 }
}

#[test]
fn update_declared_steps_remain_valid_and_unique_on_both_platforms() {
 let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/atualizacoes");
 assert!(root.is_dir());
 for windows in [false,true] {
  let (steps,invalid)=hangar_server::update::steps::read_steps(&root,windows);
  assert!(invalid.is_empty(),"passos inválidos: {invalid:?}");
  let mut ids=std::collections::HashSet::new();
  for step in steps {assert!(ids.insert(step.id.clone()),"id repetido: {}",step.id);}
 }
}
