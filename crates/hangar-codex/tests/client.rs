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

async fn raw_listener() -> (tokio::net::TcpListener,String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}",listener.local_addr().unwrap());
    (listener,url)
}

/// Quadros que o app-server parado não deixa sair. O Winsock aceita inteiros os dois primeiros `send` com
/// o par sem ler, de qualquer tamanho, e só o terceiro para; no Linux o primeiro grande já prende e os
/// outros esperam na fila. Devolve o recibo do último, o que nunca sai do processo nos dois SOs.
const FILL_FRAMES:usize = 2;
async fn stuck_write(outgoing:&tokio::sync::mpsc::Sender<RawWrite>,size:usize) -> tokio::sync::oneshot::Receiver<Result<(),ClientError>> {
    for _ in 0..FILL_FRAMES {
        let (fill,_) = tokio::sync::oneshot::channel();
        assert!(outgoing.send(RawWrite { text:"a".repeat(size),receipt:fill,guard:None }).await.is_ok());
    }
    let (receipt,written) = tokio::sync::oneshot::channel();
    assert!(outgoing.send(RawWrite { text:"b".repeat(size),receipt,guard:None }).await.is_ok());
    written
}

/// Enfileira um quadro e devolve o recibo; `None` = a fila já estava fechada.
async fn raw_send(raw:&RawWebSocket,text:String) -> Option<Result<(),ClientError>> {
    let (receipt,written) = tokio::sync::oneshot::channel();
    raw.outgoing.send(RawWrite { text,receipt,guard:None }).await.ok()?;
    // Recibo solto sem conclusão (queda com o quadro na fila) também não é escrita.
    Some(tokio::time::timeout(Duration::from_secs(5),written).await.expect("recibo sem desfecho").unwrap_or(Err(ClientError::Closed)))
}

#[tokio::test]
async fn raw_websocket_refuses_wss_before_opening_a_connection() {
    let (listener,url) = raw_listener().await;
    let wss = url.replacen("ws://","wss://",1);
    assert!(matches!(connect_raw_ws(&wss).await,Err(ClientError::Io(_))),"wss aceito por um transporte sem TLS");
    assert!(matches!(Client::connect_ws(&wss).await,Err(ClientError::Io(_))),"connect_ws aceitou wss");
    assert!(tokio::time::timeout(Duration::from_millis(200),listener.accept()).await.is_err(),"conexão aberta antes da recusa");
}

/// O app-server com `--ws-auth capability-token` só aceita quem manda o token como Bearer; sem token, nada vai.
#[tokio::test]
async fn raw_websocket_sends_the_app_server_token_only_when_given() {
    for bearer in [Some("token-de-prova"),None] {
        let (listener,url) = raw_listener().await;
        let server = tokio::spawn(async move {
            let (stream,_) = listener.accept().await.unwrap();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
            let keep = seen.clone();
            let callback = move |request:&tokio_tungstenite::tungstenite::handshake::server::Request,response| {
                *keep.lock().unwrap() = request.headers().get("authorization").map(|v|v.to_str().unwrap().to_owned());
                Ok(response)
            };
            drop(tokio_tungstenite::accept_hdr_async(stream,callback).await.unwrap());
            seen.lock().unwrap().clone()
        });
        let raw = connect_raw_ws_with(&url,bearer).await.unwrap();
        assert_eq!(server.await.unwrap(),bearer.map(|token|format!("Bearer {token}")));
        drop(raw);
    }
}

#[tokio::test]
async fn raw_websocket_keeps_engine_id_and_receipts_after_the_send() {
    use futures_util::{SinkExt,StreamExt};
    let (listener,url) = raw_listener().await;
    let (seen_tx,seen) = tokio::sync::oneshot::channel::<String>();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let text = ws.next().await.unwrap().unwrap().to_text().unwrap().to_owned();
        let msg:Value = serde_json::from_str(&text).unwrap();
        let _ = seen_tx.send(text);
        ws.send(tokio_tungstenite::tungstenite::Message::text(json!({"id":msg["id"],"result":{}}).to_string())).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(200),listener.accept()).await.is_err(),"segunda conexão aberta");
        // Exatamente um quadro: depois dele só o fechamento de quem soltou o transporte.
        while let Some(Ok(message)) = ws.next().await { assert!(message.is_close(),"quadro extra no socket"); }
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    let frame = json!({"jsonrpc":"2.0","id":"hangar:3:op-9","method":"turn/start","params":{}}).to_string();
    assert!(matches!(raw_send(&raw,frame.clone()).await,Some(Ok(()))));
    assert_eq!(seen.await.unwrap(),frame,"quadro alterado no caminho");
    let reply:Value = serde_json::from_str(&tokio::time::timeout(Duration::from_secs(5),raw.incoming.recv()).await.unwrap().unwrap()).unwrap();
    assert_eq!(reply["id"],"hangar:3:op-9","resposta consumida ou alterada pelo transporte");
    drop(raw);
    tokio::time::timeout(Duration::from_secs(5),server).await.expect("soltar o transporte não fechou a conexão").unwrap();
}

#[tokio::test]
async fn raw_websocket_closed_before_enqueue_never_receipts_ok() {
    let (listener,url) = raw_listener().await;
    tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        drop(tokio_tungstenite::accept_async(stream).await.unwrap());
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5),raw.incoming.recv()).await.unwrap().is_none());
    assert!(!matches!(raw_send(&raw,"{\"id\":\"hangar:1:x\"}".into()).await,Some(Ok(()))));
}

#[tokio::test]
async fn raw_websocket_closed_after_enqueue_keeps_the_write_receipt() {
    use futures_util::StreamExt;
    let (listener,url) = raw_listener().await;
    tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ws.next().await;
        drop(ws);
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    // A escrita saiu; a falta de resposta é assunto do motor, não do recibo.
    assert!(matches!(raw_send(&raw,"{\"id\":\"hangar:2:y\"}".into()).await,Some(Ok(()))));
    assert!(tokio::time::timeout(Duration::from_secs(5),raw.incoming.recv()).await.unwrap().is_none());
}

#[tokio::test]
async fn raw_websocket_guard_refusal_after_frame_preparation_is_not_written() {
    use futures_util::StreamExt;
    let (listener,url) = raw_listener().await;
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await { assert!(!message.is_text(),"resposta recusada chegou ao app-server"); }
    });
    let raw = connect_raw_ws(&url).await.unwrap();
    let (receipt,written) = tokio::sync::oneshot::channel();
    assert!(raw.outgoing.send(RawWrite { text:"{\"id\":0,\"result\":{}}".into(),receipt,
        guard:Some(std::sync::Arc::new(|_|Err("erro_interacao_desatualizada".into()))) }).await.is_ok());
    let result = tokio::time::timeout(Duration::from_secs(5),written).await.unwrap().unwrap();
    assert!(matches!(result,Err(ClientError::NotWritten { code }) if code == "erro_interacao_desatualizada"));
    drop(raw);
    tokio::time::timeout(Duration::from_secs(5),server).await.unwrap().unwrap();
}

#[tokio::test]
async fn dropping_a_raw_websocket_stuck_in_a_send_frees_the_socket() {
    use tokio::io::AsyncReadExt;
    let (listener,url) = raw_listener().await;
    let (dropped_tx,dropped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        // App-server vivo que não lê: o `send` do cliente para com os buffers cheios.
        dropped.await.unwrap();
        // Passa da folga do fechamento antes de ler: o `send` não pode destravar por esta leitura.
        tokio::time::sleep(Duration::from_secs(3)).await;
        // Solto o transporte, o socket do cliente fecha: a leitura chega ao fim (ou ao reset), não fica pendurada.
        let mut buffer = vec![0u8;1 << 16];
        loop {
            match ws.get_mut().read(&mut buffer).await { Ok(0) | Err(_) => return, Ok(_) => {} }
        }
    });
    let raw = connect_raw_ws(&url).await.unwrap();
    let written = stuck_write(&raw.outgoing,15 << 20).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(raw);
    dropped_tx.send(()).unwrap();
    // Escrita cortada no meio: o recibo é solto sem conclusão (incerto), nunca `Ok`.
    let receipt = tokio::time::timeout(Duration::from_secs(10),written).await.expect("escritor preso não foi abortado");
    assert!(!matches!(receipt,Ok(Ok(()))));
    tokio::time::timeout(Duration::from_secs(30),server).await.expect("socket do cliente seguiu aberto").unwrap();
}

#[tokio::test]
async fn abort_now_frees_a_stuck_socket_before_any_reconnect_could_start() {
    use tokio::io::AsyncReadExt;
    let (listener,url) = raw_listener().await;
    let (aborted_tx,aborted) = tokio::sync::oneshot::channel::<std::time::Instant>();
    let server = tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let at = aborted.await.unwrap();
        // Só lê depois do aborto: a leitura não pode destravar o `send` antes dele.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut buffer = vec![0u8;1 << 16];
        loop {
            match ws.get_mut().read(&mut buffer).await { Ok(0) | Err(_) => break, Ok(_) => {} }
        }
        at.elapsed()
    });
    let RawWebSocket { incoming:_incoming,outgoing,tasks,.. } = connect_raw_ws(&url).await.unwrap();
    let written = stuck_write(&outgoing,15 << 20).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    tasks.abort_now();
    aborted_tx.send(std::time::Instant::now()).unwrap();
    assert!(!matches!(tokio::time::timeout(Duration::from_secs(5),written).await.expect("recibo preso"),Ok(Ok(()))));
    let closed_after = tokio::time::timeout(Duration::from_secs(10),server).await.expect("socket seguiu aberto").unwrap();
    // A primeira reconexão do runtime espera 1 s: o socket antigo já fechou bem antes.
    assert!(closed_after < Duration::from_millis(900),"socket fechou só em {closed_after:?}");
}

#[tokio::test]
async fn raw_websocket_closed_during_the_write_is_not_ok() {
    use tokio::io::AsyncReadExt;
    let (listener,url) = raw_listener().await;
    tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        // Lê o começo do quadro e derruba o TCP com o resto ainda saindo.
        let mut start = [0u8;1024];
        let _ = ws.get_mut().read(&mut start).await;
        drop(ws);
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    // Os primeiros quadros podem caber inteiros no `send` do Windows (ver `stuck_write`); o que ainda está
    // saindo quando o TCP cai nunca é `Ok`.
    let written = stuck_write(&raw.outgoing,12 << 20).await;
    let receipt = tokio::time::timeout(Duration::from_secs(5),written).await.expect("recibo sem desfecho");
    assert!(!matches!(receipt,Ok(Ok(()))));
    assert!(tokio::time::timeout(Duration::from_secs(5),raw.incoming.recv()).await.unwrap().is_none());
}

#[tokio::test]
async fn raw_websocket_receives_a_resume_larger_than_sixteen_megabytes() {
    use futures_util::SinkExt;
    // O `thread/resume` de uma conversa longa já passou de 16 MiB: o teto derrubava a conexão a cada assinatura.
    let (listener,url) = raw_listener().await;
    let big = format!("{{\"id\":1,\"result\":{{\"pad\":\"{}\"}}}}","a".repeat(18 * 1024 * 1024));
    let size = big.len();
    tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(tokio_tungstenite::tungstenite::Message::text(big)).await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10),raw.incoming.recv()).await.unwrap().expect("conexão caiu na mensagem grande");
    assert_eq!(got.len(),size);
}

#[tokio::test]
async fn raw_websocket_keeps_the_close_code_for_the_diary() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::protocol::{CloseFrame,frame::coding::CloseCode};
    let (listener,url) = raw_listener().await;
    tokio::spawn(async move {
        let (stream,_) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ws.send(tokio_tungstenite::tungstenite::Message::Close(Some(CloseFrame { code:CloseCode::Error,reason:"".into() }))).await;
    });
    let mut raw = connect_raw_ws(&url).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5),raw.incoming.recv()).await.unwrap().is_none());
    assert_eq!(*raw.close_code.lock().unwrap(),Some(1011));
}
