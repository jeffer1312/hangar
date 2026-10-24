//! Python falso para os testes do hangar-server: rotas internas (info, fatos, diário) e o resto
//! respondendo "from-python", com o que chegou anotado.
#![allow(dead_code)]

use std::collections::HashMap;
use std::convert::Infallible;
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
use axum::routing::get;
use bytes::Bytes;
use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use hangar_server::auth::TrustedHosts;
use hangar_server::config::Config;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

pub const OWNER: &str = "dono-token";
pub const SECRET: &str = "segredo-interno";

pub struct Fake {
    info: Mutex<Value>,
    info_calls: AtomicUsize,
    hits: Mutex<Vec<(String, HeaderMap)>>,
    info_status: Mutex<Option<StatusCode>>,
    diag: Mutex<Vec<Value>>,
    pub release: Notify,
    /// Resposta e demora de `/internal/list/facts`; quantos pedidos chegaram e o último.
    pub list_facts: Mutex<(Value, Duration)>,
    pub list_facts_calls: AtomicUsize,
    pub list_facts_last: Mutex<Value>,
    /// Demora do `info` antes de responder (Python lento).
    info_delay: Mutex<Duration>,
    /// Corpos que chegaram em `/api/plugin/ui` (a cópia da faixa que o Rust manda).
    plugin_ui: Mutex<Vec<Value>>,
    /// Corpo cru do último pedido de escrita repassado.
    last_body: Mutex<Vec<u8>>,
    /// Enquanto ligado, `/input` só responde depois de `release`.
    hold_input: std::sync::atomic::AtomicBool,
    /// A antiga rota da pergunta do plugin (`/internal/sessions/{name}/plugin`), só para provar que o Rust
    /// não a chama mais: o status que a faz falhar, as leituras e os avisos que chegaram.
    plugin_status: Mutex<Option<StatusCode>>,
    plugin_gets: AtomicUsize,
    plugin_posts: Mutex<Vec<Value>>,
    /// Resposta do `/api/sessions/{n}/input` repassado; `None` = o "from-python" de sempre.
    input_reply: Mutex<Option<(StatusCode, Value)>>,
    /// Rotas `/internal/pair/*`, `/internal/orq/*` e `/internal/external-pairs/*` (caminho sem
    /// `/internal/`): resposta e corpos recebidos.
    internal_replies: Mutex<HashMap<String, (StatusCode, Value)>>,
    internal_bodies: Mutex<Vec<(String, Value)>>,
    /// Demora dessas rotas depois de anotar o corpo (Python lento segurando quem chamou).
    pub internal_delay: Mutex<Duration>,
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
    /// Corpos recebidos em `/internal/diag`.
    pub fn diag(&self) -> Vec<Value> {
        self.diag.lock().unwrap().clone()
    }
    pub fn info_calls(&self) -> usize {
        self.info_calls.load(SeqCst)
    }
    pub fn hits_to(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| p.split('?').next() == Some(path)).count()
    }
    pub fn last_hit(&self) -> (String, HeaderMap) {
        // As tarefas de fundo do servidor (varredura de worktrees) também chamam `/internal/*`.
        self.hits.lock().unwrap().iter().rev().find(|(p, _)| !p.starts_with("/internal/")).cloned().expect("algum pedido repassado")
    }
    /// O `info` demora isto antes de responder.
    pub fn set_info_delay(&self, delay: Duration) {
        *self.info_delay.lock().unwrap() = delay;
    }
    /// Bytes do corpo do último pedido de escrita repassado.
    pub fn last_body(&self) -> Vec<u8> {
        self.last_body.lock().unwrap().clone()
    }
    /// `/input` fica preso no Python falso até `release.notify_one()`.
    pub fn hold_input(&self, on: bool) {
        self.hold_input.store(on, SeqCst);
    }
    /// A rota interna do plugin responde só este status (o Python caído ou recusando).
    pub fn fail_plugin(&self, s: Option<StatusCode>) {
        *self.plugin_status.lock().unwrap() = s;
    }
    pub fn plugin_gets(&self) -> usize {
        self.plugin_gets.load(SeqCst)
    }
    pub fn plugin_posts(&self) -> Vec<Value> {
        self.plugin_posts.lock().unwrap().clone()
    }
    /// Os corpos que chegaram em `/api/plugin/ui`, na ordem.
    pub fn plugin_ui_bodies(&self) -> Vec<Value> {
        self.plugin_ui.lock().unwrap().clone()
    }
    pub fn set_input_reply(&self, reply: Option<(StatusCode, Value)>) {
        *self.input_reply.lock().unwrap() = reply;
    }
    /// Quantos `/api/sessions/{n}/input` chegaram, de qualquer sessão.
    pub fn input_calls(&self) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| {
            let path = p.split('?').next().unwrap_or_default();
            path.starts_with("/api/sessions/") && path.ends_with("/input")
        }).count()
    }
    /// `path` sem `/internal/`, ex. `orq/promote`.
    pub fn set_internal(&self, path: &str, status: StatusCode, body: Value) {
        self.internal_replies.lock().unwrap().insert(path.to_owned(), (status, body));
    }
    pub fn internal_bodies(&self, path: &str) -> Vec<Value> {
        self.internal_bodies.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, b)| b.clone()).collect()
    }
}

pub async fn spawn_fake() -> (Arc<Fake>, SocketAddr) {
    let fake = Arc::new(Fake {
        info: Mutex::new(Value::Null),
        info_calls: AtomicUsize::new(0),
        hits: Mutex::default(),
        info_status: Mutex::default(),
        diag: Mutex::default(),
        release: Notify::new(),
        list_facts: Mutex::new((json!({"states": {}}), Duration::ZERO)),
        list_facts_calls: AtomicUsize::new(0),
        list_facts_last: Mutex::new(Value::Null),
        info_delay: Mutex::default(),
        plugin_ui: Mutex::default(),
        last_body: Mutex::default(),
        hold_input: std::sync::atomic::AtomicBool::new(false),
        plugin_status: Mutex::default(),
        plugin_gets: AtomicUsize::new(0),
        plugin_posts: Mutex::default(),
        input_reply: Mutex::default(),
        internal_replies: Mutex::new(HashMap::from([
            ("orq/group-phase".to_owned(), (StatusCode::OK, json!({"phase": null}))),
            ("orq/promote".to_owned(), (StatusCode::OK, json!({}))),
            ("orq/is-orchestrator".to_owned(), (StatusCode::OK, json!({"names": []}))),
            ("orq/associate".to_owned(), (StatusCode::OK, json!({"ok": true}))),
            ("external-pairs/end".to_owned(), (StatusCode::OK, json!({"errors": []}))),
        ])),
        internal_bodies: Mutex::default(),
        internal_delay: Mutex::default(),
    });
    let app = Router::new()
        .route("/internal/sessions/{name}/info", get(fake_info))
        .route("/internal/diag", axum::routing::post(fake_diag))
        .route("/internal/list/facts", axum::routing::post(fake_list_facts))
        .route("/internal/sessions/{name}/plugin", get(fake_plugin_get).post(fake_plugin_post))
        .route("/internal/pair/{op}", axum::routing::post(fake_internal_json))
        .route("/internal/orq/{op}", axum::routing::post(fake_internal_json))
        .route("/internal/external-pairs/{op}", axum::routing::post(fake_internal_json))
        .fallback(fake_python)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, addr)
}

fn internal_ok(h: &HeaderMap) -> bool {
    h.get("x-hangar-internal").is_some_and(|v| v.as_bytes() == SECRET.as_bytes())
}

fn status(s: StatusCode) -> Response {
    Response::builder().status(s).body(Body::empty()).unwrap()
}

async fn fake_info(State(f): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    f.info_calls.fetch_add(1, SeqCst);
    let delay = *f.info_delay.lock().unwrap();
    tokio::time::sleep(delay).await;
    if let Some(s) = *f.info_status.lock().unwrap() {
        return status(s);
    }
    let info = f.info.lock().unwrap().clone();
    if !internal_ok(&headers) || info.is_null() {
        return status(StatusCode::NOT_FOUND);
    }
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(info.to_string()))
        .unwrap()
}

async fn fake_list_facts(State(f): State<Arc<Fake>>, headers: HeaderMap, body: Bytes) -> Response {
    if !internal_ok(&headers) {
        return status(StatusCode::NOT_FOUND);
    }
    f.list_facts_calls.fetch_add(1, SeqCst);
    *f.list_facts_last.lock().unwrap() = serde_json::from_slice(&body).unwrap();
    let (reply, delay) = f.list_facts.lock().unwrap().clone();
    tokio::time::sleep(delay).await;
    Response::builder().header("content-type", "application/json").body(Body::from(reply.to_string())).unwrap()
}

async fn fake_diag(State(f): State<Arc<Fake>>, headers: HeaderMap, body: Bytes) -> Response {
    if !internal_ok(&headers) {
        return status(StatusCode::NOT_FOUND);
    }
    f.diag.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
    status(StatusCode::OK)
}

async fn fake_plugin_get(State(f): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    if !internal_ok(&headers) {
        return status(StatusCode::NOT_FOUND);
    }
    f.plugin_gets.fetch_add(1, SeqCst);
    if let Some(s) = *f.plugin_status.lock().unwrap() {
        return status(s);
    }
    Response::builder().header("content-type", "application/json").body(Body::from(r#"{"pending":null}"#)).unwrap()
}

async fn fake_plugin_post(State(f): State<Arc<Fake>>, headers: HeaderMap, body: Bytes) -> Response {
    if !internal_ok(&headers) {
        return status(StatusCode::NOT_FOUND);
    }
    f.plugin_posts.lock().unwrap().push(serde_json::from_slice(&body).unwrap_or(Value::Null));
    if let Some(s) = *f.plugin_status.lock().unwrap() {
        return status(s);
    }
    Response::builder().header("content-type", "application/json").body(Body::from(r#"{"ok":true}"#)).unwrap()
}

async fn fake_internal_json(State(f): State<Arc<Fake>>, req: Request) -> Response {
    if !internal_ok(req.headers()) {
        return status(StatusCode::NOT_FOUND);
    }
    let path = req.uri().path().trim_start_matches("/internal/").to_owned();
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap_or_default();
    f.internal_bodies.lock().unwrap().push((path.clone(), serde_json::from_slice(&bytes).unwrap_or(Value::Null)));
    let delay = *f.internal_delay.lock().unwrap();
    tokio::time::sleep(delay).await;
    let Some((code, body)) = f.internal_replies.lock().unwrap().get(&path).cloned() else { return status(StatusCode::NOT_FOUND) };
    Response::builder().status(code).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()
}

async fn fake_python(State(f): State<Arc<Fake>>, mut req: Request) -> Response {
    let full = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
    f.hits.lock().unwrap().push((full, req.headers().clone()));
    let path = req.uri().path().to_owned();
    if path == "/api/plugin/ui" {
        let bytes = axum::body::to_bytes(std::mem::take(req.body_mut()), 1 << 20).await.unwrap_or_default();
        f.plugin_ui.lock().unwrap().push(serde_json::from_slice(&bytes).unwrap_or(Value::Null));
    }
    if req.method() != axum::http::Method::GET && path != "/ws" && path != "/api/plugin/ui" {
        let bytes = axum::body::to_bytes(std::mem::take(req.body_mut()), 1 << 24).await.unwrap_or_default();
        *f.last_body.lock().unwrap() = bytes.to_vec();
    }
    if path.ends_with("/input") && f.hold_input.load(SeqCst) {
        f.release.notified().await;
    }
    if path.ends_with("/input") && let Some((code, body)) = f.input_reply.lock().unwrap().clone() {
        return Response::builder().status(code).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    }
    match path.as_str() {
        "/redirect" => Response::builder().status(302).header("location", "/outro").body(Body::empty()).unwrap(),
        "/probe" => status(StatusCode::UNAUTHORIZED),
        "/limited" => status(StatusCode::TOO_MANY_REQUESTS),
        "/stream" => {
            let f2 = f.clone();
            let s = futures_util::stream::once(async { Ok::<_, Infallible>(Bytes::from_static(b"um")) })
                .chain(futures_util::stream::once(async move {
                    f2.release.notified().await;
                    Ok(Bytes::from_static(b"dois"))
                }));
            Response::new(Body::from_stream(s))
        }
        // O navegador remoto do convidado também é WebSocket no Python.
        p if p == "/ws" || p.ends_with("/nav-remoto") => {
            let upgrade = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                let Ok(up) = upgrade.await else { return };
                let mut io = TokioIo::new(up);
                let mut buf = [0u8; 64];
                while let Ok(n) = io.read(&mut buf).await {
                    if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
            Response::builder()
                .status(101)
                .header("connection", "upgrade")
                .header("upgrade", "eco")
                .body(Body::empty())
                .unwrap()
        }
        _ => Response::new(Body::from("from-python")),
    }
}

pub fn config(upstream: SocketAddr, trusted: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream,
        internal_secret: SECRET.into(),
        auth_token: OWNER.into(),
        log_path: None,
        trusted: TrustedHosts::parse(trusted),
    }
}

/// Descoberta dos testes: a sessão é o que o Python falso tem em `set_info` (`Value::Null` =
/// inexistente, `fail_info` = descoberta fora do ar), lida pela rota `info` dele, que conta e demora.
pub struct FakeLocate {
    pub upstream: SocketAddr,
    pub http: hangar_server::proxy::HttpClient,
    pub secret: String,
}

impl hangar_server::locate::Locate for FakeLocate {
    fn info<'a>(&'a self, name: &'a str) -> hangar_server::locate::BoxFuture<'a,
        Result<Option<hangar_server::transcript::InternalInfo>, hangar_server::locate::InfoFailed>> {
        use hangar_server::locate::InfoFailed;
        use http_body_util::BodyExt;
        const FAIL: InfoFailed = InfoFailed("fake_info");
        Box::pin(async move {
            let name = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
            let url = format!("http://{}/internal/sessions/{name}/info", self.upstream);
            let req = axum::http::Request::get(url).header("x-hangar-internal", &self.secret).body(Body::empty()).map_err(|_| FAIL)?;
            let resp = self.http.request(req).await.map_err(|_| FAIL)?;
            match resp.status() {
                StatusCode::NOT_FOUND => return Ok(None),
                s if !s.is_success() => return Err(FAIL),
                _ => {}
            }
            let body = resp.into_body().collect().await.map_err(|_| FAIL)?.to_bytes();
            serde_json::from_slice(&body).map(Some).map_err(|_| FAIL)
        })
    }
}

pub fn fake_locate(upstream: SocketAddr) -> Arc<dyn hangar_server::locate::Locate> {
    Arc::new(FakeLocate { upstream, http: hangar_server::proxy::client(), secret: SECRET.into() })
}

pub async fn spawn_server(cfg: Config) -> SocketAddr {
    spawn_state(hangar_server::routes::AppState::new(cfg)).await
}

/// Servidor com um `AppState` montado pelo teste (para mexer no `Mods` dele por fora).
pub async fn spawn_state(mut state: hangar_server::routes::AppState) -> SocketAddr {
    state.side.locate = fake_locate(state.cfg.upstream);
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
    let (python, upstream) = spawn_fake().await;
    let state = hangar_server::routes::AppState::new(config(upstream, "127.0.0.1"));
    let mods = state.mods.clone();
    let registry = Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(upstream, "secret-test".into(), "instance-test".into())
        .with_mods(mods.clone()));
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
