use super::*;

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);

/// Servidor de eco numa porta efêmera do loopback.
async fn echo() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    port
}

/// Rota de WebSocket que faz o `relay` até `port`; `ended` resolve quando o `oneshot` dispara.
async fn tunnel(port: u16, ended: Option<oneshot::Receiver<()>>) -> Client {
    let ended = std::sync::Mutex::new(ended);
    let ended = std::sync::Arc::new(ended);
    let app = Router::new().route("/", get(move |ws: WebSocketUpgrade| {
        let ended = ended.lock().unwrap().take();
        async move {
            let tcp = connect(port).await.unwrap();
            ws.on_upgrade(move |socket| relay(socket, tcp, async move {
                match ended {
                    Some(rx) => { let _ = rx.await; }
                    None => std::future::pending().await,
                }
            }))
        }
    }));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio_tungstenite::connect_async(format!("ws://{addr}/")).await.unwrap().0
}

/// Próxima mensagem que não é ping nem pong; `None` quando a conexão acabou.
async fn next(client: &mut Client) -> Option<Ws> {
    loop {
        match tokio::time::timeout(WAIT, client.next()).await.expect("prazo") {
            Some(Ok(Ws::Ping(_) | Ws::Pong(_))) => {}
            Some(Ok(message)) => return Some(message),
            _ => return None,
        }
    }
}

#[test]
fn port_of_accepts_only_1_to_65535() {
    assert_eq!(port_of(Some("port=5432")), Some(5432));
    assert_eq!(port_of(Some("token=x&port=65535")), Some(65535));
    for query in [Some("port=0"), Some("port=65536"), Some("port=-1"), Some("port=abc"), Some("port="), Some("x=1"), None] {
        assert_eq!(port_of(query), None, "{query:?}");
    }
}

#[tokio::test]
async fn bytes_go_and_come_back_through_the_tcp_server() {
    let mut client = tunnel(echo().await, None).await;
    client.send(Ws::Binary(b"SELECT 1;".to_vec().into())).await.unwrap();
    assert_eq!(next(&mut client).await, Some(Ws::Binary(b"SELECT 1;".to_vec().into())));
}

#[tokio::test]
async fn text_messages_are_ignored() {
    let mut client = tunnel(echo().await, None).await;
    client.send(Ws::Text("não é byte".into())).await.unwrap();
    client.send(Ws::Binary(b"ok".to_vec().into())).await.unwrap();
    assert_eq!(next(&mut client).await, Some(Ws::Binary(b"ok".to_vec().into())), "o texto não chegou ao TCP");
}

#[tokio::test]
async fn tcp_closing_closes_the_websocket_with_1000() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"tchau").await.unwrap();
    });
    let mut client = tunnel(port, None).await;
    assert_eq!(next(&mut client).await, Some(Ws::Binary(b"tchau".to_vec().into())));
    match next(&mut client).await {
        Some(Ws::Close(Some(frame))) => assert_eq!(frame.code, CloseCode::Normal),
        other => panic!("esperava fechamento 1000: {other:?}"),
    }
}

#[tokio::test]
async fn websocket_closing_closes_the_tcp_socket() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest).await;
        let _ = tx.send(rest);
    });
    let mut client = tunnel(port, None).await;
    client.send(Ws::Binary(b"fim".to_vec().into())).await.unwrap();
    client.close(None).await.unwrap();
    let received = tokio::time::timeout(WAIT, rx).await.expect("o TCP não fechou").unwrap();
    assert_eq!(received, b"fim");
}

#[tokio::test]
async fn empty_binary_ends_the_client_writes_and_the_reply_still_arrives() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut asked = Vec::new();
        stream.read_to_end(&mut asked).await.unwrap();
        stream.write_all(&[b"resposta a ", &asked[..]].concat()).await.unwrap();
    });
    let mut client = tunnel(port, None).await;
    client.send(Ws::Binary(b"pergunta".to_vec().into())).await.unwrap();
    client.send(Ws::Binary(Vec::new().into())).await.unwrap();
    assert_eq!(next(&mut client).await, Some(Ws::Binary(b"resposta a pergunta".to_vec().into())));
    match next(&mut client).await {
        Some(Ws::Close(Some(frame))) => assert_eq!(frame.code, CloseCode::Normal),
        other => panic!("esperava fechamento 1000: {other:?}"),
    }
}

#[tokio::test]
async fn ended_access_drops_the_tunnel() {
    let (revoke, ended) = oneshot::channel();
    let mut client = tunnel(echo().await, Some(ended)).await;
    client.send(Ws::Binary(b"antes".to_vec().into())).await.unwrap();
    assert_eq!(next(&mut client).await, Some(Ws::Binary(b"antes".to_vec().into())));
    revoke.send(()).unwrap();
    assert_eq!(next(&mut client).await, None, "o túnel continuou aberto depois do fim do acesso");
}

#[tokio::test]
async fn connect_falls_back_to_ipv6_loopback() {
    // Máquina sem IPv6 no loopback: não há o que provar.
    let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else { return };
    let port = listener.local_addr().unwrap().port();
    // Outro programa no mesmo número em IPv4 tornaria a prova ambígua.
    if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_ok() { return }
    let accepted = tokio::spawn(async move { listener.accept().await.is_ok() });
    connect(port).await.expect("o [::1] não foi tentado");
    assert!(accepted.await.unwrap());
}
