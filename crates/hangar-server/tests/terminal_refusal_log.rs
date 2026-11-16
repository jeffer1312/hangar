//! T-06 em processo próprio: o aviso sai pelo `tracing`, e o interesse de cada ponto de log é
//! guardado para o processo inteiro. Com outros testes criando e soltando assinantes em paralelo, um
//! `set_default` desta thread chegava a ficar sem a linha.
use std::sync::{Arc, Mutex};

use hangar_server::auth::TrustedHosts;
use hangar_server::config::Config;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Log {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(data); Ok(data.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

async fn spawn_server() -> std::net::SocketAddr {
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    let mut state = hangar_server::routes::AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        internal_secret: "segredo-interno".into(),
        auth_token: "dono-token".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    // A limpeza de worktrees do servidor leria as configurações do HOME real.
    state.origins_home = std::env::temp_dir().join("hangar-test-empty-home");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(hangar_server::routes::serve_with_state(listener, state));
    addr
}

async fn term_handshake(srv: std::net::SocketAddr, query_token: &str, origin: &str) -> (u16, Vec<u8>) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{srv}/api/sessions/s0/term?token={query_token}").into_client_request().unwrap();
    request.headers_mut().insert("x-forwarded-for", origin.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (response.status().as_u16(), response.body().clone().unwrap_or_default()),
        other => panic!("o aperto de mão não foi recusado: {:?}", other.map(|_| ())),
    }
}

/// T-06: a recusa do terminal que bloqueia a origem deixa um aviso no log com a origem (o Python:
/// "<ip> errou o token 8 vezes em 30s — bloqueado por 30s"); as sete anteriores não avisam.
/// Um teste só neste binário: o assinante de log é global.
#[tokio::test]
async fn t06_the_terminal_refusal_that_blocks_an_origin_logs_a_warning_with_it() {
    let log = Log::default();
    let writer = log.clone();
    tracing::subscriber::set_global_default(tracing_subscriber::fmt().without_time().with_ansi(false)
        .with_writer(move || writer.clone()).finish()).unwrap();
    let srv = spawn_server().await;
    const ORIGIN: &str = "198.51.100.230";
    const WARNING: &str = "token errado 8 vezes em 30 s; origem bloqueada";
    let text = || String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    for _ in 0..7 {
        assert_eq!(term_handshake(srv, "errado", ORIGIN).await, (403, Vec::new()));
    }
    assert!(!text().contains(WARNING), "aviso antes do bloqueio: {}", text());
    assert_eq!(term_handshake(srv, "errado", ORIGIN).await, (403, Vec::new()), "a oitava recusa");
    let text = text();
    let warnings: Vec<&str> = text.lines().filter(|line| line.contains(WARNING)).collect();
    assert_eq!(warnings.len(), 1, "{text}");
    assert!(warnings[0].contains("WARN") && warnings[0].contains(&format!("ip={ORIGIN}")), "{text}");
}
