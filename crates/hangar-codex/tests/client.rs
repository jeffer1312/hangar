use hangar_codex::client::*;
use hangar_codex::proto::*;
use serde_json::{Value,json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};

#[tokio::test]
async fn account_login_preserves_early_completion_and_cancel_id() {
    let (ours,theirs)=tokio::io::duplex(1<<16);
    let (read,mut write)=tokio::io::split(theirs);
    let server=tokio::spawn(async move {
        let mut lines=BufReader::new(read).lines();
        let start:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(start["method"],"account/login/start");
        assert_eq!(start["params"],json!({"type":"chatgptDeviceCode"}));
        for value in [json!({"method":"account/login/completed","params":{"loginId":"login-test","success":true,"error":null}}),
            json!({"id":start["id"],"result":{"type":"chatgptDeviceCode","loginId":"login-test","verificationUrl":"https://example.test/device","userCode":"synthetic"}})] {
            write.write_all(format!("{value}\n").as_bytes()).await.unwrap();
        }
        let cancel:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(cancel["method"],"account/login/cancel");
        assert_eq!(cancel["params"],json!({"loginId":"login-test"}));
        write.write_all(format!("{}\n",json!({"id":cancel["id"],"result":{"status":"canceled"}})).as_bytes()).await.unwrap();
    });
    let (r,w)=tokio::io::split(ours);let (client,mut incoming)=Client::over_lines(r,w);
    let response:LoginAccountResponse=client.request(ClientRequest::AccountLoginStart(LoginAccountParams::ChatgptDeviceCode),Duration::from_secs(3)).await.unwrap();
    assert!(matches!(response,LoginAccountResponse::ChatgptDeviceCode {login_id,..} if login_id=="login-test"));
    let Some(Incoming::Notification {method,params})=incoming.recv().await else {panic!("evento antecipado perdido")};
    assert!(matches!(ServerNotification::decode(&method,&params).unwrap(),ServerNotification::AccountLoginCompleted(n) if n.success && n.login_id.as_deref()==Some("login-test")));
    let result:CancelLoginAccountResponse=client.request(ClientRequest::AccountLoginCancel(CancelLoginAccountParams {login_id:"login-test".into()}),Duration::from_secs(3)).await.unwrap();
    assert_eq!(result.status,"canceled");server.await.unwrap();
}

/// Servidor falso no outro lado de um duplex: responde `model/list`, empurra uma notificação e um pedido.
async fn fake(stream:tokio::io::DuplexStream) {
    let (read,mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg:Value = serde_json::from_str(&line).unwrap();
        if msg["method"] == "model/list" {
            let out = [json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u"}}}),
                json!({"id":"srv-1","method":"item/tool/requestUserInput","params":{"threadId":"t","questions":[]}}),
                json!({"id":msg["id"],"result":{"data":[{"model":"m"}]}})];
            for o in out { write.write_all(format!("{o}\n").as_bytes()).await.unwrap(); }
        } else if msg["method"] == "turn/interrupt" {
            write.write_all(format!("{}\n",json!({"id":msg["id"],"error":{"code":-32600,"message":"não"}})).as_bytes()).await.unwrap();
        } else if msg.get("result").is_some() {
            write.write_all(format!("{}\n",json!({"method":"echo","params":msg})).as_bytes()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn request_notification_and_server_request() {
    let (ours,theirs) = tokio::io::duplex(1 << 16);
    tokio::spawn(fake(theirs));
    let (r,w) = tokio::io::split(ours);
    let (client,mut incoming) = Client::over_lines(r,w);
    let list:ModelListResponse = client.request(ClientRequest::ModelList(Default::default()),Duration::from_secs(5)).await.unwrap();
    assert_eq!(list.data[0].model,"m");
    assert!(matches!(incoming.recv().await,Some(Incoming::Notification { method,.. }) if method == "turn/started"));
    let Some(Incoming::Request { id,method,.. }) = incoming.recv().await else { panic!() };
    assert_eq!(method,"item/tool/requestUserInput");
    client.respond(id,Ok(json!({"answers":{}}))).await.unwrap();
    let Some(Incoming::Notification { params,.. }) = incoming.recv().await else { panic!() };
    assert_eq!(params["id"],"srv-1");
}

#[tokio::test]
async fn rpc_error_comes_back_typed() {
    let (ours,theirs) = tokio::io::duplex(1 << 16);
    tokio::spawn(fake(theirs));
    let (r,w) = tokio::io::split(ours);
    let (client,_incoming) = Client::over_lines(r,w);
    let err = client.request::<Value>(ClientRequest::TurnInterrupt(TurnInterruptParams { thread_id:"t".into(),turn_id:"u".into() }),Duration::from_secs(5)).await.unwrap_err();
    assert!(matches!(err,ClientError::Rpc { code:-32600,.. }));
}

#[tokio::test]
async fn server_dying_fails_pending_at_once() {
    let (ours,theirs) = tokio::io::duplex(1 << 16);
    let (r,w) = tokio::io::split(ours);
    let (client,mut incoming) = Client::over_lines(r,w);
    let call = tokio::spawn({ let client = client.clone(); async move {
        client.request::<Value>(ClientRequest::ModelList(Default::default()),Duration::from_secs(60)).await } });
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(theirs);
    let result = tokio::time::timeout(Duration::from_secs(2),call).await.expect("não pode esperar o prazo").unwrap();
    assert!(matches!(result,Err(ClientError::Closed)));
    assert!(incoming.recv().await.is_none());
}

#[tokio::test]
async fn timeout_is_reported() {
    let (ours,_theirs) = tokio::io::duplex(1 << 16);
    let (r,w) = tokio::io::split(ours);
    let (client,_incoming) = Client::over_lines(r,w);
    let err = client.request::<Value>(ClientRequest::ModelList(Default::default()),Duration::from_millis(50)).await.unwrap_err();
    assert!(matches!(err,ClientError::Timeout));
}

#[tokio::test]
async fn oversized_line_closes_the_connection() {
    let (ours,mut theirs) = tokio::io::duplex(1 << 20);
    let (r,w) = tokio::io::split(ours);
    let (_client,mut incoming) = Client::over_lines(r,w);
    tokio::spawn(async move {
        let chunk = vec![b'a';1 << 20];
        for _ in 0..(MAX_LINE / chunk.len() + 2) { if theirs.write_all(&chunk).await.is_err() { return; } }
    });
    assert!(tokio::time::timeout(Duration::from_secs(10),incoming.recv()).await.unwrap().is_none());
}

#[tokio::test]
async fn websocket_transport() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use futures_util::{SinkExt,StreamExt};
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            let Ok(text) = message.to_text() else { continue };
            let msg:Value = serde_json::from_str(text).unwrap();
            let reply = json!({"id":msg["id"],"result":{"userAgent":"x/0.159.3 (y)"}});
            ws.send(tokio_tungstenite::tungstenite::Message::text(reply.to_string())).await.unwrap();
        }
    });
    let (client,_incoming) = Client::connect_ws(&format!("ws://{address}")).await.unwrap();
    let init:InitializeResponse = client.request(ClientRequest::Initialize(Default::default()),Duration::from_secs(5)).await.unwrap();
    assert_eq!(init.user_agent,"x/0.159.3 (y)");
}

#[tokio::test]
async fn request_with_invalid_id_is_rejected() {
    let (ours,theirs) = tokio::io::duplex(1 << 16);
    let (r,w) = tokio::io::split(ours);
    let (_client,_incoming) = Client::over_lines(r,w);
    let (read,mut write) = tokio::io::split(theirs);
    write.write_all(b"{\"id\":1.5,\"method\":\"item/tool/requestUserInput\",\"params\":{}}\n").await.unwrap();
    let mut lines = BufReader::new(read).lines();
    let reply = tokio::time::timeout(Duration::from_secs(2),lines.next_line()).await.expect("sem resposta ao id inválido").unwrap().unwrap();
    let reply:Value = serde_json::from_str(&reply).unwrap();
    assert!(reply["id"].is_null());
    assert_eq!(reply["error"]["code"],-32600);
}

#[tokio::test]
async fn writer_dying_fails_pending_at_once() {
    let (ours,_theirs) = tokio::io::duplex(1 << 16);
    let (r,_) = tokio::io::split(ours);
    let (broken,gone) = tokio::io::duplex(64);
    drop(gone);
    let (client,_incoming) = Client::over_lines(r,broken);
    let call = client.request::<Value>(ClientRequest::ModelList(Default::default()),Duration::from_secs(60));
    let result = tokio::time::timeout(Duration::from_secs(2),call).await.expect("não pode esperar o prazo");
    assert!(matches!(result,Err(ClientError::Closed)));
}

#[tokio::test]
async fn writer_dying_closes_incoming() {
    let (ours,_theirs) = tokio::io::duplex(1 << 16);
    let (r,_) = tokio::io::split(ours);
    let (broken,gone) = tokio::io::duplex(64);
    drop(gone);
    let (client,mut incoming) = Client::over_lines(r,broken);
    let _ = client.notify("initialized",json!({})).await;
    let next = tokio::time::timeout(Duration::from_secs(2),incoming.recv()).await.expect("leitor seguiu vivo sem escritor");
    assert!(next.is_none());
}
