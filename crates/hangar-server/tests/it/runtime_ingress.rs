use hangar_server::runtime::gateway::{self,RuntimeRegistry};
use std::sync::Arc;

async fn serve() -> (std::net::SocketAddr,tokio::task::JoinHandle<std::io::Result<()>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"secret-test".into(),"instance-test".into()));
    (address,tokio::spawn(gateway::serve(listener,registry,"secret-test".into(),"instance-test".into(),hangar_server::INTERNAL_PROTOCOL)))
}

async fn send(address:std::net::SocketAddr,command:serde_json::Value) -> (u16,serde_json::Value) {
    let response = reqwest::Client::new().post(format!("http://{address}/runtime/op"))
        .header("x-hangar-internal","secret-test").header("x-hangar-runtime-instance","instance-test")
        .header("content-type","application/json")
        .body(serde_json::json!({"protocol":hangar_server::INTERNAL_PROTOCOL,"instance":"instance-test","key":"key",
            "generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":command}).to_string())
        .send().await.unwrap();
    let status = response.status().as_u16();
    (status,serde_json::from_str(&response.text().await.unwrap()).unwrap())
}

#[tokio::test]
async fn ingress_command_closes_and_opens_without_an_entry() {
    let (address,server) = serve().await;
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":true})).await;
    assert_eq!((status,body),(200,serde_json::json!({"ok":true,"result":{"closed":true}})));
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":false})).await;
    assert_eq!((status,body),(200,serde_json::json!({"ok":true,"result":{"closed":false}})));
    server.abort();
}

#[tokio::test]
async fn ingress_command_carries_the_hold() {
    let (address,server) = serve().await;
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":true,"held":true})).await;
    assert_eq!((status,body),(200,serde_json::json!({"ok":true,"result":{"closed":true}})));
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":false,"held":true})).await;
    assert_eq!((status,body),(200,serde_json::json!({"ok":true,"result":{"closed":false}})));
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":true,"held":"sim"})).await;
    assert_eq!((status,body["error_code"].as_str()),(503,Some("ingress_payload")));
    server.abort();
}

#[tokio::test]
async fn ingress_command_rejects_extra_field_and_bad_payload() {
    let (address,server) = serve().await;
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s","closed":true,"extra":1})).await;
    assert_eq!((status,body["error_code"].as_str()),(503,Some("command_fields")));
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"","closed":true})).await;
    assert_eq!((status,body["error_code"].as_str()),(503,Some("ingress_payload")));
    let (status,body) = send(address,serde_json::json!({"kind":"ingress","name":"s"})).await;
    assert_eq!((status,body["error_code"].as_str()),(503,Some("ingress_payload")));
    server.abort();
}

#[tokio::test]
async fn writable_is_none_without_an_entry() {
    let registry = RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"s".into(),"i".into());
    assert!(registry.writable("s").await.is_none());
}
