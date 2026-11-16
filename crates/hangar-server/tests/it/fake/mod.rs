//! Dublês dos testes do hangar-server: a descoberta da sessão (`set_info`), o diário isolado e um
//! servidor HTTP que só anota o que recebe.
#![allow(dead_code)]

use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use hangar_server::auth::TrustedHosts;
use hangar_server::config::Config;
use serde_json::{Value, json};

pub const OWNER: &str = "dono-token";
pub const SECRET: &str = "segredo-interno";

pub struct Fake {
    info: Mutex<Value>,
    info_calls: AtomicUsize,
    hits: Mutex<Vec<(String, HeaderMap)>>,
    info_status: Mutex<Option<StatusCode>>,
    diary: tempfile::TempDir,
    /// Demora do `info` antes de responder (descoberta lenta).
    info_delay: Mutex<Duration>,
    /// Corpo cru do último pedido recebido.
    last_body: Mutex<Vec<u8>>,
}

impl Fake {
    /// O que /internal/.../info devolve. `Value::Null` = sessão inexistente (404).
    pub fn set_info(&self, v: Value) {
        *self.info.lock().unwrap() = v;
    }
    /// `info` responde só este status, como o Python quando a projeção falha (503).
    pub fn fail_info(&self, s: StatusCode) {
        *self.info_status.lock().unwrap() = Some(s);
    }
    /// Eventos que o Rust gravou no diário isolado desta prova.
    pub fn diag(&self) -> Vec<Value> { read_diary(self.diary.path()) }
    pub fn diary_dir(&self) -> &Path { self.diary.path() }
    pub fn diagnostics(&self) -> hangar_server::diag::DiagClient {
        hangar_server::diag::DiagClient::new()
            .with_diary(Some(self.diary.path().to_owned()))
    }
    pub fn bind_diary(&self, client: hangar_server::diag::DiagClient) -> hangar_server::diag::DiagClient {
        client.with_diary(Some(self.diary.path().to_owned()))
    }
    pub fn info_calls(&self) -> usize {
        self.info_calls.load(SeqCst)
    }
    pub fn hits_to(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| p.split('?').next() == Some(path)).count()
    }
    pub fn last_hit(&self) -> (String, HeaderMap) {
        self.hits.lock().unwrap().last().cloned().expect("algum pedido recebido")
    }
    /// O `info` demora isto antes de responder.
    pub fn set_info_delay(&self, delay: Duration) {
        *self.info_delay.lock().unwrap() = delay;
    }
    /// Bytes do corpo do último pedido recebido.
    pub fn last_body(&self) -> Vec<u8> {
        self.last_body.lock().unwrap().clone()
    }
}

pub async fn spawn_fake() -> (Arc<Fake>, SocketAddr) {
    hangar_server::diag::disable_diary();
    let fake = Arc::new(Fake {
        info: Mutex::new(Value::Null),
        info_calls: AtomicUsize::new(0),
        hits: Mutex::default(),
        info_status: Mutex::default(),
        diary: tempfile::tempdir().unwrap(),
        info_delay: Mutex::default(),
        last_body: Mutex::default(),
    });
    let app = Router::new().fallback(record).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, addr)
}

/// O servidor que o comando `hangar-server open` chama nos testes: anota o pedido e responde 200.
async fn record(State(f): State<Arc<Fake>>, req: Request) -> Response {
    let full = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
    f.hits.lock().unwrap().push((full, req.headers().clone()));
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 24).await.unwrap_or_default();
    *f.last_body.lock().unwrap() = bytes.to_vec();
    Response::new(Body::empty())
}

pub fn config(trusted: &str) -> Config {
    // A abertura leria a cota pela rede com o token falso das contas de teste.
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        internal_secret: SECRET.into(),
        auth_token: OWNER.into(),
        log_path: None,
        trusted: TrustedHosts::parse(trusted),
    }
}

/// Descoberta dos testes: a sessão é o que `set_info` guardou (`Value::Null` = inexistente,
/// `fail_info` = descoberta fora do ar), contada e com a demora de `set_info_delay`.
struct FakeLocate(Arc<Fake>);

impl hangar_server::locate::Locate for FakeLocate {
    fn info<'a>(&'a self, _name: &'a str) -> hangar_server::locate::BoxFuture<'a,
        Result<Option<hangar_server::transcript::InternalInfo>, hangar_server::locate::InfoFailed>> {
        use hangar_server::locate::InfoFailed;
        const FAIL: InfoFailed = InfoFailed("fake_info");
        Box::pin(async move {
            let f = &self.0;
            f.info_calls.fetch_add(1, SeqCst);
            let delay = *f.info_delay.lock().unwrap();
            tokio::time::sleep(delay).await;
            match *f.info_status.lock().unwrap() {
                Some(StatusCode::NOT_FOUND) => return Ok(None),
                Some(_) => return Err(FAIL),
                None => {}
            }
            let info = f.info.lock().unwrap().clone();
            if info.is_null() { return Ok(None); }
            serde_json::from_value(info).map(Some).map_err(|_| FAIL)
        })
    }
}

impl Fake {
    pub fn locate(self: &Arc<Self>) -> Arc<dyn hangar_server::locate::Locate> {
        Arc::new(FakeLocate(self.clone()))
    }
}

pub fn read_diary(dir: &Path) -> Vec<Value> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("leitura do diário temporário: {error}"),
    };
    let mut paths: Vec<_> = entries.map(|entry| entry.unwrap().path()).filter(|path| {
        path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("uso-") && name.ends_with(".jsonl"))
    }).collect();
    paths.sort();
    paths.into_iter().flat_map(|path| {
        let raw = std::fs::read_to_string(path).unwrap();
        let complete = raw.rsplit_once('\n').map_or("", |(complete,_)| complete);
        complete.lines().filter(|line| !line.is_empty()).map(|line| serde_json::from_str(line).unwrap()).collect::<Vec<_>>()
    }).collect()
}

pub async fn wait_diary(dir: &Path, matches: impl Fn(&Value) -> bool) -> Vec<Value> {
    use notify::Watcher;
    let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| { let _ = tx.send(event); }).unwrap();
    watcher.watch(dir, notify::RecursiveMode::NonRecursive).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rows = read_diary(dir);
        if rows.iter().any(&matches) { return rows; }
        let changed = tokio::time::timeout_at(deadline, changes.recv()).await.expect("o evento não chegou ao diário temporário");
        changed.expect("a observação do diário encerrou").expect("a observação do diário falhou");
    }
}

pub async fn spawn_server_with_diary(cfg: Config, fake: &Arc<Fake>) -> SocketAddr {
    let mut state = hangar_server::routes::AppState::new(cfg);
    state.diag = fake.bind_diary(state.diag.clone());
    state.side.locate = fake.locate();
    spawn_state(state).await
}

/// Servidor que acha a sessão no que `fake.set_info` guardou.
pub async fn spawn_server_for(fake: &Arc<Fake>, cfg: Config) -> SocketAddr {
    spawn_state_for(fake, hangar_server::routes::AppState::new(cfg)).await
}

pub async fn spawn_state_for(fake: &Arc<Fake>, mut state: hangar_server::routes::AppState) -> SocketAddr {
    state.side.locate = fake.locate();
    spawn_state(state).await
}

pub async fn spawn_server(cfg: Config) -> SocketAddr {
    spawn_state(hangar_server::routes::AppState::new(cfg)).await
}

/// Servidor com um `AppState` montado pelo teste (para mexer no `Mods` dele por fora).
pub async fn spawn_state(mut state: hangar_server::routes::AppState) -> SocketAddr {
    hangar_server::accounts::quotas::disable_open_network_read();
    hangar_server::diag::disable_diary();
    // Teste que não escolheu pasta: sem isto, a limpeza de worktrees do servidor lê as configurações do HOME real.
    if std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).is_some_and(|home| state.origins_home.as_os_str() == home) {
        state.origins_home = std::env::temp_dir().join("hangar-test-empty-home");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(hangar_server::routes::serve_with_state(listener, state));
    addr
}

/// Python falso e servidor com a sessão `name` atendida pelo `Mods` (vida 1) por `link`: o começo dos
/// testes da interface dos mods.
pub async fn serve_mods(name: &str, link: Arc<dyn hangar_server::mods::state::SurfaceLink>)
    -> (Arc<Fake>, SocketAddr, hangar_server::mods::state::Mods) {
    let (python, server, mods, _registry) = serve_mods_gated(name, link).await;
    (python, server, mods)
}

/// `serve_mods` com o registro do runtime no estado, como no `lib.rs`: dele sai a porta de entrada que a
/// troca de agente fecha.
pub async fn serve_mods_gated(name: &str, link: Arc<dyn hangar_server::mods::state::SurfaceLink>)
    -> (Arc<Fake>, SocketAddr, hangar_server::mods::state::Mods, Arc<hangar_server::runtime::gateway::RuntimeRegistry>) {
    let (python, _) = spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.diag = python.bind_diary(state.diag.clone());
    state.side.locate = python.locate();
    let mods = state.mods.clone();
    let registry = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new("instance-test".into())
        .with_mods(mods.clone()).with_diagnostics(python.diagnostics()));
    let _ = state.state.runtime.set(registry.clone());
    let server = spawn_state(state).await;
    mods.attach(name, 1, link);
    (python, server, mods, registry)
}

pub fn client() -> reqwest::Client {
    hangar_server::http_client().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}
/// Linha sintética no formato do Claude (nunca conversa real): user com texto e relógio crescente.
pub fn claude_line(i: usize) -> String {
    format!(
        "{{\"type\":\"user\",\"uuid\":\"u{i}\",\"timestamp\":\"2026-10-02T10:{m:02}:{s:02}.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"linha {i}\"}}}}\n",
        m = i / 60 % 60,
        s = i % 60
    )
}

/// Acrescenta as linhas e devolve o offset do início de cada uma.
pub fn append_lines(path: &Path, range: std::ops::Range<usize>) -> Vec<u64> {
    let mut f = OpenOptions::new().create(true).append(true).open(path).unwrap();
    let mut at = f.metadata().unwrap().len();
    let mut offs = Vec::new();
    for i in range {
        let line = claude_line(i);
        f.write_all(line.as_bytes()).unwrap();
        offs.push(at);
        at += line.len() as u64;
    }
    offs
}

pub fn append_raw(path: &Path, s: &str) {
    OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(s.as_bytes()).unwrap();
}

/// Campo `history` do `info` nos testes: o mesmo formato de `info_payload` (internal_api.py).
pub fn history_field(jsonl: &Path) -> Value {
    json!({"queue": jsonl.with_extension("fila.jsonl")})
}

pub fn info_json(provider: &str, jsonl: &Path) -> Value {
    json!({
        "provider": provider,
        "jsonl": jsonl,
        "session_key": jsonl.file_stem().unwrap().to_str().unwrap(),
        "history": history_field(jsonl),
    })
}

pub type Events = BoxStream<'static, Result<Event, EventStreamError<reqwest::Error>>>;

pub async fn open_events(srv: SocketAddr, name: &str, query: &str, headers: &[(&str, &str)]) -> reqwest::Response {
    let mut url = format!("http://{srv}/api/sessions/{name}/events?token={OWNER}");
    if !query.is_empty() {
        url.push('&');
        url.push_str(query);
    }
    let mut req = client().get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.unwrap()
}

pub fn sse(resp: reqwest::Response) -> Events {
    resp.bytes_stream().eventsource().boxed()
}

pub async fn next_any(es: &mut Events) -> Event {
    tokio::time::timeout(Duration::from_secs(5), es.next())
        .await
        .expect("evento em 5 s")
        .expect("stream aberto")
        .expect("SSE válido")
}

pub async fn next_non_ping(es: &mut Events) -> Event {
    loop {
        let ev = next_any(es).await;
        if ev.event != "ping" {
            return ev;
        }
    }
}

pub async fn next_named(es: &mut Events, name: &str) -> Event {
    loop {
        let ev = next_any(es).await;
        if ev.event == name {
            return ev;
        }
    }
}

pub async fn messages(es: &mut Events, n: usize) -> Vec<Event> {
    let mut out = Vec::new();
    while out.len() < n {
        out.push(next_named(es, "message").await);
    }
    out
}

pub async fn stream_ends(es: &mut Events) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), es.next()).await {
            Ok(None) => return true,
            // O `Monitor` de Claude com terminal publica o estado dele a qualquer momento.
            Ok(Some(Ok(ev))) if ev.event == "ping" || ev.event == "state" => continue,
            _ => return false,
        }
    }
}

pub fn id_of(ev: &Event) -> String {
    serde_json::from_str::<Value>(&ev.data).unwrap()["id"].as_str().unwrap().to_owned()
}

pub async fn wait_until(cond: impl Fn() -> bool) {
    // Teto folgado: só estoura se o código travar, não com a máquina carregada.
    for _ in 0..750 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condição não chegou em 15 s");
}

/// Espera a descoberta ser lida `n` vezes depois de agora (a vigia do hub lê a cada 2 s).
pub async fn readings(fake: &Fake, n: usize) {
    for _ in 0..n {
        let at = fake.info_calls();
        wait_until(|| fake.info_calls() > at).await;
    }
}
