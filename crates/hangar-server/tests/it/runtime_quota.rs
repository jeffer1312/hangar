use hangar_server::runtime::actor::PolicyClient;
use std::sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}};
use tokio::io::{AsyncBufReadExt,AsyncWriteExt,BufReader};

/// Python falso só da cota: conta os pedidos e responde 500 quando mandado.
async fn fake_quota(fail:Arc<AtomicBool>) -> (std::net::SocketAddr,Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    tokio::spawn(async move {
        loop {
            let (stream,_) = listener.accept().await.unwrap();
            let (counter,fail) = (counter.clone(),fail.clone());
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).await.unwrap_or(0) == 0 { return; }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" { break; }
                }
                assert!(first.starts_with("GET /internal/quota?config_dir=%2F"),"{first}");
                counter.fetch_add(1,Ordering::SeqCst);
                let response = if fail.load(Ordering::SeqCst) { "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned() }
                    else { let body = r#"{"windows":[{"rotulo":"5h","pct":42}]}"#; format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",body.len()) };
                reader.get_mut().write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    (address,calls)
}

#[tokio::test]
async fn quota_is_fetched_once_per_account_inside_the_cache_window() {
    let (address,calls) = fake_quota(Arc::new(AtomicBool::new(false))).await;
    let client = PolicyClient::new(address,"secret".into(),"instance".into());
    for _ in 0..3 {
        let quota = client.quota_windows("key","/conta/a").await.unwrap();
        assert_eq!(quota["windows"][0]["pct"],42);
    }
    assert_eq!(calls.load(Ordering::SeqCst),1);
    client.quota_windows("key","/conta/b").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),2,"cada conta tem a sua entrada");
}

#[tokio::test]
async fn quota_failure_formats_without_windows_and_the_next_call_retries() {
    let fail = Arc::new(AtomicBool::new(true));
    let (address,calls) = fake_quota(fail.clone()).await;
    let client = PolicyClient::new(address,"secret".into(),"instance".into());
    assert!(client.quota_windows("key","/conta/a").await.is_none());
    fail.store(false,Ordering::SeqCst);
    assert!(client.quota_windows("key","/conta/a").await.is_some(),"falha não fica guardada");
    assert_eq!(calls.load(Ordering::SeqCst),2);
}

#[tokio::test]
async fn quota_transport_failure_is_remembered_per_account() {
    // Aceita e derruba a conexão: falha de transporte, contada.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    tokio::spawn(async move { loop { let (stream,_) = listener.accept().await.unwrap(); counter.fetch_add(1,Ordering::SeqCst); drop(stream); } });
    let client = PolicyClient::new(address,"secret".into(),"instance".into());
    for _ in 0..3 { assert!(client.quota_windows("key","/conta/a").await.is_none()); }
    assert_eq!(calls.load(Ordering::SeqCst),1,"Python fora do ar não é consultado de novo na janela curta");
    assert!(client.quota_windows("key","/conta/b").await.is_none());
    assert_eq!(calls.load(Ordering::SeqCst),2,"cada conta tem a sua entrada");
}

#[tokio::test]
async fn quota_cache_holds_at_most_sixteen_accounts() {
    let (address,calls) = fake_quota(Arc::new(AtomicBool::new(false))).await;
    let client = PolicyClient::new(address,"secret".into(),"instance".into());
    for index in 0..17 { client.quota_windows("key",&format!("/conta/{index}")).await.unwrap(); }
    assert_eq!(calls.load(Ordering::SeqCst),17);
    client.quota_windows("key","/conta/16").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),17,"a mais nova continua guardada");
    client.quota_windows("key","/conta/0").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),18,"a mais antiga saiu para caber a 17ª");
}
