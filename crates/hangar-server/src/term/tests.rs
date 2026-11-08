//! Painel contra um tmux de soquete próprio (`-S` numa pasta temporária) e um Python falso.
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::any;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, Message as Ws};

use super::*;
use crate::auth::TrustedHosts;
use crate::config::Config;

struct Tmux { _dir: tempfile::TempDir, socket: PathBuf }

impl Tmux {
    fn new() -> Tmux {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("t.sock");
        Tmux { _dir: dir, socket }
    }
    fn raw(&self, args: &[&str]) -> Output {
        Command::new("tmux").arg("-S").arg(&self.socket).args(args).env_remove("TMUX").output().unwrap()
    }
    fn run(&self, args: &[&str]) -> String {
        let out = self.raw(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
    fn session(&self, name: &str) {
        self.run(&["-f", "/dev/null", "new-session", "-d", "-s", name, "-x", "120", "-y", "30", "cat"]);
    }
    fn window(&self, name: &str) -> String {
        self.run(&["display", "-p", "-t", &format!("={name}:"), "#{window_width}x#{window_height}"]).trim().to_string()
    }
    /// `tty tamanho` de cada cliente anexado.
    fn clients(&self, name: &str) -> Vec<String> {
        let out = self.raw(&["list-clients", "-t", &format!("={name}"), "-F", "#{client_tty} #{client_width}x#{client_height}"]);
        String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
    }
    fn option(&self, name: &str) -> String {
        let out = self.raw(&["show-options", "-v", "-t", &format!("={name}:"), pty::SIZE_OPTION]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

impl Drop for Tmux {
    fn drop(&mut self) { let _ = self.raw(&["kill-server"]); }
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "não aconteceu: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Python falso: conta os terminais que chegaram a ele.
#[derive(Default)]
struct Python {
    term_hits: AtomicUsize,
}

async fn python() -> (SocketAddr, Arc<Python>) {
    let py = Arc::new(Python::default());
    let p2 = py.clone();
    let app = Router::new()
        .route("/api/sessions/{name}/term", any(move || {
            let p2 = p2.clone();
            async move {
                p2.term_hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::FORBIDDEN
            }
        }))
        .route("/api/hangar-terminals/{ident}/term", any(|| async { StatusCode::FORBIDDEN }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, py)
}

struct Server { addr: SocketAddr, terms: Arc<Terms>, py: Arc<Python> }

async fn server(tmux: &Tmux, tune: impl FnOnce(&mut TermConfig)) -> Server {
    let (upstream, py) = python().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config { listen: addr, upstream, internal_secret: "s".into(), auth_token: "dono".into(),
                       log_path: None, trusted: TrustedHosts::parse("127.0.0.1") };
    let mut state = AppState::new(cfg);
    let mut tc = TermConfig { socket: Some(tmux.socket.clone()), ..TermConfig::default() };
    tune(&mut tc);
    let terms = Arc::new(Terms::new(tc));
    state.term = terms.clone();
    tokio::spawn(crate::routes::serve_with_state(listener, state));
    Server { addr, terms, py }
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Conecta; `Err(status)` quando o aperto de mão é recusado.
async fn connect_with(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Result<Client, u16> {
    use tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{addr}{path}").into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("conexão falhou: {e}"),
    }
}

async fn connect(addr: SocketAddr, path: &str) -> Result<Client, u16> {
    connect_with(addr, path, &[]).await
}

async fn read_until(ws: &mut Client, needle: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), async {
        while !got.windows(needle.len()).any(|w| w == needle) {
            match ws.next().await {
                Some(Ok(Ws::Binary(b))) => got.extend_from_slice(&b),
                Some(Ok(_)) => {}
                other => panic!("terminal fechou antes do eco: {other:?}"),
            }
        }
    }).await.expect("eco não chegou");
    got
}

/// Próximo fechamento (código, motivo), ignorando os bytes da tela.
async fn close_of(ws: &mut Client) -> (u16, String) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match ws.next().await {
                Some(Ok(Ws::Close(Some(f)))) => return (u16::from(f.code), f.reason.to_string()),
                Some(Ok(Ws::Close(None))) | None | Some(Err(_)) => return (0, String::new()),
                Some(Ok(_)) => {}
            }
        }
    }).await.expect("o servidor não fechou")
}

#[tokio::test]
async fn echo_roundtrip() {
    let tmux = Tmux::new();
    tmux.session("s1");
    let srv = server(&tmux, |_| {}).await;
    let mut ws = connect(srv.addr, "/api/sessions/s1/term?token=dono&cols=100&rows=30").await.expect("abre");
    ws.send(Ws::Binary("eco-7391\r".into())).await.unwrap();
    read_until(&mut ws, b"eco-7391").await;
    // O eco do tty sozinho já devolveria o texto; ele tem que ter chegado ao pane.
    until("texto no pane", || tmux.run(&["capture-pane", "-p", "-t", "=s1:"]).contains("eco-7391")).await;
    assert_eq!(tmux.clients("s1").len(), 1);
    assert_eq!(srv.terms.active(), vec!["s1".to_string()]);
    assert_eq!(srv.py.term_hits.load(Ordering::SeqCst), 0, "o dono não passa pelo Python");
}

#[tokio::test]
async fn resize_clamped() {
    let tmux = Tmux::new();
    tmux.session("s2");
    let srv = server(&tmux, |_| {}).await;
    let mut ws = connect(srv.addr, "/api/sessions/s2/term?token=dono&cols=5&rows=1").await.expect("abre");
    until("abre em 20x5", || tmux.clients("s2").iter().any(|c| c.ends_with(" 20x5"))).await;
    ws.send(Ws::Text(r#"{"t":"resize","cols":99999,"rows":-3}"#.into())).await.unwrap();
    until("500x5", || tmux.clients("s2").iter().any(|c| c.ends_with(" 500x5"))).await;
    // Quadro torto ou que não é resize: descartado, o terminal segue.
    for bad in ["nada", "\"5\"", "[1,2]", r#"{"t":"resize","cols":[1],"rows":3}"#, r#"{"t":"outro"}"#] {
        ws.send(Ws::Text(bad.into())).await.unwrap();
    }
    ws.send(Ws::Text(r#"{"t":"resize","cols":"100","rows":40.7}"#.into())).await.unwrap();
    until("100x40", || tmux.clients("s2").iter().any(|c| c.ends_with(" 100x40"))).await;
    assert_eq!(connect(srv.addr, "/api/sessions/s2/term?token=dono&cols=abc").await.err(), Some(403));
}

#[tokio::test]
async fn second_connection_takes_over() {
    let tmux = Tmux::new();
    tmux.session("s3");
    let srv = server(&tmux, |_| {}).await;
    let mut a = connect(srv.addr, "/api/sessions/s3/term?token=dono").await.expect("A");
    until("A anexado", || tmux.clients("s3").len() == 1).await;
    let mut b = connect(srv.addr, "/api/sessions/s3/term?token=dono").await.expect("B");
    assert_eq!(close_of(&mut a).await, (1000, TAKEN_OVER.to_string()));
    until("só o cliente de B", || tmux.clients("s3").len() == 1).await;
    b.send(Ws::Binary("assumiu-55\r".into())).await.unwrap();
    read_until(&mut b, b"assumiu-55").await;
    assert!(tmux.raw(&["has-session", "-t", "=s3"]).status.success(), "a troca não fecha a sessão");
}

#[tokio::test]
async fn teardown_detaches_own_client_and_restores_size() {
    let tmux = Tmux::new();
    tmux.session("s4");
    let before = tmux.window("s4");
    let srv = server(&tmux, |_| {}).await;
    let mut ws = connect(srv.addr, "/api/sessions/s4/term?token=dono&cols=80&rows=24").await.expect("abre");
    until("janela no tamanho do painel", || tmux.window("s4") != before).await;
    assert_eq!(tmux.option("s4"), before, "tamanho guardado no tmux ao anexar");
    ws.close(None).await.unwrap();
    until("cliente solto", || tmux.clients("s4").is_empty()).await;
    // Fechar o painel não digita nada no pane: um Enter + Ctrl-D encerraria o `cat` e a sessão.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tmux.raw(&["has-session", "-t", "=s4"]).status.success(), "a sessão morreu ao fechar o painel");
    until("tamanho reposto", || tmux.window("s4") == before).await;
    until("opção limpa", || tmux.option("s4").is_empty()).await;
    until("painel fora da lista", || srv.terms.active().is_empty()).await;

    // Um `tmux attach` nativo ao lado: a desmontagem solta só o nosso.
    let native = portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows: 40, cols: 100, ..Default::default() }).unwrap();
    let mut cmd = portable_pty::CommandBuilder::new("tmux");
    cmd.args(["-S", tmux.socket.to_str().unwrap(), "attach", "-t", "=s4:"]);
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("TMUX");
    let mut native_child = native.slave.spawn_command(cmd).unwrap();
    until("nativo anexado", || tmux.clients("s4").len() == 1).await;
    let native_tty = tmux.clients("s4")[0].split(' ').next().unwrap().to_string();
    let mut ws = connect(srv.addr, "/api/sessions/s4/term?token=dono&cols=80&rows=24").await.expect("abre");
    until("dois clientes", || tmux.clients("s4").len() == 2).await;
    ws.close(None).await.unwrap();
    until("só o nativo ficou", || {
        let c = tmux.clients("s4");
        c.len() == 1 && c[0].starts_with(&native_tty)
    }).await;
    let _ = native_child.kill();
}

#[test]
fn backpressure_pauses_reader() {
    struct Endless(Arc<AtomicUsize>, Arc<std::sync::atomic::AtomicBool>);
    impl std::io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.load(Ordering::SeqCst) {
                return Ok(0);
            }
            self.0.fetch_add(1, Ordering::SeqCst);
            buf.fill(b'x');
            Ok(buf.len())
        }
    }
    let reads = Arc::new(AtomicUsize::new(0));
    let eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (tx, rx) = mpsc::channel(pty::OUTPUT_SLOTS);
    let (r, e) = (reads.clone(), eof.clone());
    let reader = std::thread::spawn(move || pty::pump(Endless(r, e), tx));
    std::thread::sleep(Duration::from_millis(300));
    // O canal enche (1 MiB) e a leitura seguinte fica presa esperando vaga.
    assert_eq!(reads.load(Ordering::SeqCst), pty::OUTPUT_SLOTS + 1);
    assert_eq!(pty::OUTPUT_SLOTS * pty::CHUNK, 1 << 20);
    // Sem ouvinte o leitor segue drenando até o fim: o ConPTY só fecha com a saída esvaziada.
    drop(rx);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while reads.load(Ordering::SeqCst) < pty::OUTPUT_SLOTS * 4 {
        assert!(std::time::Instant::now() < deadline, "leitor parou de drenar sem ouvinte");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!reader.is_finished(), "leitor saiu antes do fim do PTY");
    eof.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !reader.is_finished() {
        assert!(std::time::Instant::now() < deadline, "leitor não saiu no fim do PTY");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn ping_timeout_closes_and_tears_down() {
    let tmux = Tmux::new();
    tmux.session("s6");
    tmux.session("s6b");
    let srv = server(&tmux, |c| { c.ping_every = Duration::from_millis(100); c.ping_timeout = Duration::from_millis(300); }).await;
    // Quem lê responde o ping e continua aberto.
    let mut alive = connect(srv.addr, "/api/sessions/s6b/term?token=dono").await.expect("abre");
    let reading = tokio::spawn(async move { while let Some(Ok(_)) = alive.next().await {} });
    // Quem não lê nunca devolve o pong.
    let _mute = connect(srv.addr, "/api/sessions/s6/term?token=dono").await.expect("abre");
    until("anexados", || tmux.clients("s6").len() == 1 && tmux.clients("s6b").len() == 1).await;
    until("painel mudo desmontado", || tmux.clients("s6").is_empty() && srv.terms.active() == vec!["s6b".to_string()]).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(tmux.clients("s6b").len(), 1, "o que responde segue aberto");
    reading.abort();
}

#[tokio::test]
async fn size_restored_after_crash() {
    let tmux = Tmux::new();
    tmux.session("s7");
    tmux.session("s7b");
    let before = tmux.window("s7");
    tmux.run(&["set-option", "-t", "=s7:", pty::SIZE_OPTION, &before]);
    tmux.run(&["resize-window", "-t", "=s7", "-x", "50", "-y", "10"]);
    assert_eq!(tmux.window("s7"), "50x10");
    let terms = Terms::new(TermConfig { socket: Some(tmux.socket.clone()), ..TermConfig::default() });
    terms.restore_after_crash().await;
    assert_eq!(tmux.window("s7"), before);
    assert_eq!(tmux.option("s7"), "");
    assert_eq!(tmux.window("s7b"), before, "sessão sem a opção fica como está");
}

#[tokio::test]
async fn query_token_only() {
    let tmux = Tmux::new();
    tmux.session("s8");
    let srv = server(&tmux, |_| {}).await;
    let path = "/api/sessions/s8/term";
    assert_eq!(connect_with(srv.addr, path, &[("authorization", "Bearer dono")]).await.err(), Some(403));
    assert_eq!(connect_with(srv.addr, path, &[("cookie", "cp_token=dono")]).await.err(), Some(403));
    assert_eq!(connect(srv.addr, &format!("{path}?token=errado")).await.err(), Some(403));
    assert_eq!(srv.py.term_hits.load(Ordering::SeqCst), 3, "os três foram decididos pelo Python");
    assert!(tmux.clients("s8").is_empty());
    let _ws = connect(srv.addr, &format!("{path}?token=dono")).await.expect("só o ?token= abre");
    assert_eq!(srv.py.term_hits.load(Ordering::SeqCst), 3);
}

/// O `?token=` do dono não é credencial ambiente: página estranha não o tem, então a Origin não é conferida.
#[tokio::test]
async fn owner_token_opens_from_any_origin() {
    let tmux = Tmux::new();
    tmux.session("s9");
    let srv = server(&tmux, |_| {}).await;
    let path = "/api/sessions/s9/term?token=dono";
    let ws = connect_with(srv.addr, path, &[("origin", "https://evil.com")]).await.expect("Origin estranha com token abre");
    drop(ws);
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{}{path}", srv.addr)).unwrap();
    req.headers_mut().insert("origin", axum::http::HeaderValue::from_bytes(b"https://\xe9vil.com").unwrap());
    tokio_tungstenite::connect_async(req).await.expect("Origin ilegível com token abre");
    assert_eq!(connect_with(srv.addr, "/api/sessions/s9/term", &[("origin", "https://evil.com")]).await.err(), Some(403),
               "sem token segue ao Python");
    assert_eq!(srv.py.term_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shortcut_resolves_owner() {
    let tmux = Tmux::new();
    let mine = "shortcut-own-abc123";
    let hangar = "shortcut-s-def456";
    for (name, owner, id) in [(mine, "own", "abc123"), (hangar, "", "def456")] {
        tmux.session(name);
        tmux.run(&["set-option", "-t", &format!("={name}:"), "@cp_shortcut_owner", owner]);
        tmux.run(&["set-option", "-t", &format!("={name}:"), "@cp_shortcut_id", id]);
    }
    let srv = server(&tmux, |_| {}).await;
    let _a = connect(srv.addr, "/api/sessions/own/term?token=dono&shortcut=abc123").await.expect("atalho do dono");
    until("anexado ao atalho", || tmux.clients(mine).len() == 1).await;
    for path in ["/api/sessions/outra/term?token=dono&shortcut=abc123", "/api/sessions/own/term?token=dono&shortcut=ABC123",
                 "/api/sessions/own/term?token=dono&shortcut=", "/api/hangar-terminals/abc123/term?token=dono"] {
        assert_eq!(connect(srv.addr, path).await.err(), Some(403), "{path}");
    }
    let _h = connect(srv.addr, "/api/hangar-terminals/def456/term?token=dono").await.expect("No Hangar");
    until("anexado ao No Hangar", || tmux.clients(hangar).len() == 1).await;
}

#[tokio::test]
async fn panel_cap_refuses_1013() {
    let tmux = Tmux::new();
    tmux.session("sa");
    tmux.session("sb");
    let srv = server(&tmux, |c| c.max_panels = 1).await;
    let _a = connect(srv.addr, "/api/sessions/sa/term?token=dono").await.expect("primeiro");
    let mut b = connect(srv.addr, "/api/sessions/sb/term?token=dono").await.expect("aceita para fechar");
    assert_eq!(close_of(&mut b).await, (1013, "limite de paineis".to_string()));
    assert!(tmux.clients("sb").is_empty());

    let down = server(&tmux, |c| c.program = "/nao/existe/tmux".into()).await;
    let mut c = connect(down.addr, "/api/sessions/sa/term?token=dono").await.expect("aceita para fechar");
    assert_eq!(close_of(&mut c).await, (1013, "multiplexador indisponivel".to_string()));
}

/// Porta privada do servidor, lida da saúde como o Python lê.
async fn private_addr(addr: SocketAddr) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /__hangar_server/health HTTP/1.0\r\nHost: x\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(v["terminal_panel"], true, "a saúde diz que o painel é do Rust");
    v["terminal_address"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn private_route_requires_secret() {
    let tmux = Tmux::new();
    tmux.session("p1");
    let srv = server(&tmux, |_| {}).await;
    let private = private_addr(srv.addr).await;
    let path = "/__hangar_server/term?target=p1&cols=9999&rows=30";
    assert_eq!(connect(private, path).await.err(), Some(404), "sem segredo");
    assert_eq!(connect_with(private, path, &[("x-hangar-internal", "outro")]).await.err(), Some(404));
    assert_eq!(connect_with(private, path, &[("x-hangar-internal", "s"), ("x-forwarded-for", "203.0.113.9")]).await.err(), Some(404),
               "repasse de fora não é o Python");
    assert_eq!(connect_with(srv.addr, path, &[("x-hangar-internal", "s")]).await.err(), Some(404), "nunca na porta pública");
    assert_eq!(connect_with(private, "/__hangar_server/term?cols=90", &[("x-hangar-internal", "s")]).await.err(), Some(400));
    assert_eq!(connect_with(private, "/__hangar_server/term?target=p1&cols=abc", &[("x-hangar-internal", "s")]).await.err(), Some(400));
    assert_eq!(connect_with(private, "/__hangar_server/term?target=morta", &[("x-hangar-internal", "s")]).await.err(), Some(403),
               "sessão que morreu depois da porta do Python não vira painel");
    assert!(srv.terms.active().is_empty());
    assert!(tmux.clients("p1").is_empty());
    let mut ws = connect_with(private, path, &[("x-hangar-internal", "s")]).await.expect("o Python liga com o segredo");
    until("anexado em 500x30, com o clamp", || tmux.clients("p1").iter().any(|c| c.ends_with(" 500x30"))).await;
    ws.send(Ws::Binary("priv-4410\r".into())).await.unwrap();
    read_until(&mut ws, b"priv-4410").await;
    assert_eq!(srv.py.term_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn one_panel_across_owner_and_guest() {
    let tmux = Tmux::new();
    tmux.session("p2");
    let srv = server(&tmux, |_| {}).await;
    let private = private_addr(srv.addr).await;
    let guest_path = "/__hangar_server/term?target=p2";
    let mut owner = connect(srv.addr, "/api/sessions/p2/term?token=dono").await.expect("dono");
    until("dono anexado", || tmux.clients("p2").len() == 1).await;
    let mut guest = connect_with(private, guest_path, &[("x-hangar-internal", "s")]).await.expect("convidado");
    assert_eq!(close_of(&mut owner).await, (1000, TAKEN_OVER.to_string()));
    until("só o convidado", || tmux.clients("p2").len() == 1).await;
    assert_eq!(srv.terms.active(), vec!["p2".to_string()]);
    let _owner = connect(srv.addr, "/api/sessions/p2/term?token=dono").await.expect("dono de volta");
    assert_eq!(close_of(&mut guest).await, (1000, TAKEN_OVER.to_string()));
    until("só o dono", || tmux.clients("p2").len() == 1).await;
}

/// `term.active` da ponte: o 409 do Python pergunta por aqui.
mod bridge {
    use super::*;

    async fn ask(private: SocketAddr, name: &str) -> serde_json::Value {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = serde_json::json!({"op": "term.active", "args": {"name": name}}).to_string();
        let mut s = tokio::net::TcpStream::connect(private).await.unwrap();
        s.write_all(format!("POST /__hangar_server/list HTTP/1.0\r\nHost: x\r\nx-hangar-internal: s\r\n\
            content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn term_active() {
        let tmux = Tmux::new();
        tmux.session("p3");
        let srv = server(&tmux, |_| {}).await;
        let private = private_addr(srv.addr).await;
        assert_eq!(ask(private, "p3").await, serde_json::json!({"ok": true, "result": {"active": false}}));
        let ws = connect(srv.addr, "/api/sessions/p3/term?token=dono").await.expect("abre");
        until("anexado", || tmux.clients("p3").len() == 1).await;
        assert_eq!(ask(private, "p3").await, serde_json::json!({"ok": true, "result": {"active": true}}));
        assert_eq!(ask(private, "outra").await["result"]["active"], false);
        drop(ws);
        until("painel fechado", || srv.terms.active().is_empty()).await;
        assert_eq!(ask(private, "p3").await["result"]["active"], false);
    }
}
