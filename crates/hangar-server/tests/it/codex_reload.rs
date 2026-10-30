use hangar_server::codex_setup::reload;
use serde_json::{Value,json};
use futures_util::{StreamExt,SinkExt};
use std::sync::{Arc,Mutex};
#[tokio::test]
async fn codex_service_reload_matches_original_loopback_exchange(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/reload.json")).unwrap();let mut failures=Vec::new();
 for case in corpus["cases"].as_array().unwrap(){
  let temp=tempfile::tempdir().unwrap();let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=format!("ws://{}",listener.local_addr().unwrap());let calls=Arc::new(Mutex::new(Vec::<Value>::new()));let saved=calls.clone();let fixture=case.clone();let closed=Arc::new(Mutex::new(Vec::<u16>::new()));let closing=closed.clone();
  let mut server=tokio::spawn(async move{
   let (stream,_)=listener.accept().await.unwrap();let mut ws=tokio_tungstenite::accept_async(stream).await.unwrap();
   while let Some(Ok(message))=ws.next().await {
    if let tokio_tungstenite::tungstenite::Message::Close(frame)=message {closing.lock().unwrap().push(frame.map(|f|u16::from(f.code)).unwrap_or(1005));break;}
    if !message.is_text(){continue;}let req:Value=serde_json::from_str(message.to_text().unwrap()).unwrap();let method=req["method"].as_str().unwrap();saved.lock().unwrap().push(json!({"method":method,"params":req["params"]}));
    if fixture["name"]=="closed"{ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame{code:tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,reason:"".into()})).await.unwrap();closing.lock().unwrap().push(1000);break;}
    if fixture["name"]=="notification"{ws.send(tokio_tungstenite::tungstenite::Message::Text(json!({"jsonrpc":"2.0","method":"fixture/event","params":{}}).to_string().into())).await.unwrap();}
    let fail=matches!((fixture["name"].as_str().unwrap(),method),("initialize-error","initialize")|("list-error","hooks/list")|("batch-error","config/batchWrite"));
    let mut reply=json!({"jsonrpc":"2.0","id":req["id"]});if fail{reply["error"]=json!({"code":-32603,"message":"fixture private"});}else{reply["result"]=if method=="hooks/list"{fixture["data"].clone()}else{json!({})};}
    if ws.send(tokio_tungstenite::tungstenite::Message::Text(reply.to_string().into())).await.is_err(){break;}
   }
  });
  let result=reload::reload(&endpoint,"/fixture/project",temp.path(),&temp.path().join("account")).await;
  if !calls.lock().unwrap().is_empty(){let _=tokio::time::timeout(std::time::Duration::from_secs(2),&mut server).await;}server.abort();
  let actual=calls.lock().unwrap().clone();if json!(result)!=case["result"]||json!(actual)!=case["calls"]||json!(*closed.lock().unwrap())!=case["closed"]{failures.push(json!({"name":case["name"],"result":result,"calls":actual,"expected":case}));}
 }
 assert!(failures.is_empty(),"{failures:#?}");
}
