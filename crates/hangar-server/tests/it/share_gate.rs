//! Porteiro da 8766: portados de `backend/tests/test_share_gate.py` + o foco da revisão.
//! Fora daqui: a porta do dono que não passa pelo porteiro, CP_PORT=8766, porta ocupada no boot e
//! o socket principal são do listener (Task 6); `guest_of` sem scope é coisa do Python; vida
//! recriada e cache de antes da troca de modo estão em `share_life.rs`.
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use hangar_server::share::ShareState;
use hangar_server::share::caller::{Caller, GuestGrant, guest_allowed, path_session};
use hangar_server::share::gate::{Gate, GuestConn, guard};
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::Real;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite;

/// Multiplexador falso: `keys` é a vida de cada sessão; `has` o que o tmux diz de quem sumiu.
#[derive(Default)]
struct Probe {
    keys: Mutex<HashMap<String, String>>,
    has: Mutex<Option<bool>>,
}

impl LifeProbe for Probe {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async { None })
    }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> {
        let h = *self.has.lock().unwrap();
        Box::pin(async move { h })
    }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> {
        self.keys.lock().unwrap().get(name).cloned().map(|k| (k, false))
    }
    fn sidecar_exists(&self, _name: &str) -> bool {
        false
    }
}

struct H {
    _dir: tempfile::TempDir,
    share: Arc<ShareState>,
    probe: Arc<Probe>,
    addr: SocketAddr,
}

impl H {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
    fn ws(&self, path: &str) -> String {
        format!("ws://{}{path}", self.addr)
    }
    fn token(&self, session: &str, kind: Kind) -> String {
        let life = self.probe.keys.lock().unwrap().get(session).map(|k| format!("k:{k}"));
        self.share.store.create_redeemed(session, life, kind).unwrap().1
    }
    /// Liga `session` ao token que já existe, como o resgate de um segundo convite.
    fn add(&self, token: &str, session: &str) {
        let other = self.token(session, Kind::Share);
        self.share.store.attach(token, &other).unwrap();
    }
    fn revoke_all(&self) {
        for s in self.share.store.all() {
            self.share.store.revoke(&s.id).unwrap();
        }
    }
    /// tmux que parou de responder: a vida some e a ausência não é confirmada.
    fn unsure(&self, name: &str) {
        self.probe.keys.lock().unwrap().remove(name);
        *self.probe.has.lock().unwrap() = None;
    }
}

async fn echo(req: Request) -> Response {
    let guest = match req.extensions().get::<Caller>() {
        Some(Caller::Guest(g)) => {
            let mut s: Vec<&String> = g.sessions.keys().collect();
            s.sort();
            json!(s.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(","))
        }
        _ => Value::Null,
    };
    let v = json!({
        "path": req.uri().path(),
        "guest": guest,
        "peer": req.extensions().get::<ConnectInfo<SocketAddr>>().is_some(),
        "x_guest": req.headers().contains_key("x-hangar-guest"),
    });
    ([("content-type", "application/json")], v.to_string()).into_response()
}

/// SSE com `n` pedaços, um a cada `ms`: o teste mexe no estado enquanto ele corre.
async fn events(req: Request) -> Response {
    let q: HashMap<String, String> = req.uri().query()
        .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default();
    let n: u64 = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(500);
    let ms: u64 = q.get("ms").and_then(|v| v.parse().ok()).unwrap_or(10);
    let stream = futures_util::stream::unfold(0, move |i| async move {
        if i >= n {
            return None;
        }
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        Some((Ok::<_, std::convert::Infallible>(format!("data: {i}\n\n")), i + 1))
    });
    let mut r = Response::new(Body::from_stream(stream));
    r.headers_mut().insert("content-type", "text/event-stream".parse().unwrap());
    r
}

async fn term(ws: WebSocketUpgrade, Path(name): Path<String>) -> Response {
    ws.on_upgrade(move |mut s| async move {
        let _ = s.send(Message::Text(format!("ok {name}").into())).await;
        let _ = s.send(Message::Close(None)).await;
    })
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(Probe::default());
    for (name, key) in [("cc", "L1"), ("dd", "L1"), ("yy", "L2")] {
        probe.keys.lock().unwrap().insert(name.into(), key.into());
    }
    let facts = Arc::new(LifeFacts::default());
    facts.apply_transfer_snapshot(HashMap::new(), HashSet::new());
    let share = Arc::new(ShareState {
        store: ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 100.0)),
        pairs: PairStore::open(dir.path().join("external_pairs.json")),
        lives: Lives::new(probe.clone(), facts),
        tailscale: Arc::new(Real),
        tunnel: Default::default(),
        connect: None,
    });
    let gate = Arc::new(Gate { share: share.clone(), watch_every: Duration::from_millis(50) });
    let app = Router::new()
        .route("/api/sessions/events", get(events))
        .route("/api/sessions/{name}/events", get(events))
        .route("/api/sessions/{name}/term", get(term))
        .fallback(any(echo))
        .layer(from_fn_with_state(gate, guard));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<GuestConn>()).await.unwrap();
    });
    H { _dir: dir, share, probe, addr }
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn get_as(h: &H, path: &str, token: &str) -> reqwest::Response {
    client().get(h.url(path)).bearer_auth(token).send().await.unwrap()
}

async fn status(h: &H, method: Method, path: &str, token: &str) -> u16 {
    client().request(method, h.url(path)).bearer_auth(token).send().await.unwrap().status().as_u16()
}

async fn json_of(r: reqwest::Response) -> Value {
    serde_json::from_str(&r.text().await.unwrap()).unwrap()
}

async fn detail_code(r: reqwest::Response) -> String {
    let v: Value = json_of(r).await;
    v["detail"]["code"].as_str().unwrap().to_owned()
}

/// Lê o SSE inteiro e conta os pedaços; `after_first` roda depois do primeiro.
async fn count_chunks(r: reqwest::Response, after_first: impl FnOnce()) -> usize {
    let mut body = r.bytes_stream();
    let mut text = String::new();
    let mut hook = Some(after_first);
    let read = async {
        while let Some(chunk) = body.next().await {
            let Ok(chunk) = chunk else { break };
            text.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(f) = hook.take() {
                f();
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), read).await.expect("stream preso");
    text.lines().filter(|l| l.starts_with("data:")).count()
}

/// Aperto de mão recusado como o uvicorn faz com o `close` antes do `accept`: 403 seco.
async fn handshake_refusal(url: &str) -> (u16, Vec<u8>, Option<String>, bool) {
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    req.headers_mut().insert("origin", "https://app-do-convidado.example".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(tungstenite::Error::Http(r)) => {
            let h = r.headers();
            let connection = h.get("connection").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let cors = h.contains_key("access-control-allow-origin");
            (r.status().as_u16(), r.body().clone().unwrap_or_default(), connection, cors)
        }
        Err(e) => panic!("esperava recusa HTTP, veio {e}"),
        Ok(_) => panic!("esperava recusa no aperto de mão, a conexão abriu"),
    }
}

#[tokio::test]
async fn session_route_marks_the_guest() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = get_as(&h, "/api/sessions/cc/history", &t).await;
    assert_eq!(r.status(), 200);
    let v: Value = json_of(r).await;
    assert_eq!(v["guest"], "cc");
    assert_eq!(v["peer"], true, "handlers que leem ConnectInfo<SocketAddr> continuam funcionando");
}

#[tokio::test]
async fn token_in_query_works() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = client().get(h.url(&format!("/api/sessions/cc/history?token={t}"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn owner_token_and_cookie_are_refused() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    assert_eq!(status(&h, Method::GET, "/api/sessions/cc/history", "dono-token").await, 401);
    let r = client().get(h.url("/api/sessions/cc/history"))
        .header("cookie", format!("cp_token={t}; __Host-cp_token={t}")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(detail_code(r).await, "erro_nao_autorizado");
}

#[tokio::test]
async fn unknown_token_is_401_with_envelope() {
    let h = harness().await;
    let r = get_as(&h, "/api/sessions/cc/history", "x").await;
    assert_eq!(r.status(), 401);
    let v: Value = json_of(r).await;
    assert_eq!(v, json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}));
}

#[tokio::test]
async fn other_session_and_unlisted_global_are_403() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    for path in ["/api/sessions/outra/history", "/api/config", "/api/fs/roots", "/"] {
        let r = get_as(&h, path, &t).await;
        assert_eq!(r.status(), 403, "{path}");
        let v: Value = json_of(r).await;
        assert_eq!(v["detail"], json!({"code": "erro_fora_do_convite", "params": {}, "msg": "fora da sessao compartilhada"}));
    }
}

#[tokio::test]
async fn blocked_list() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    for tail in ["pair", "pair-remote", "group-message", "orq/papel", "bastao", "open-terminal", "open-editor",
        "nav", "share", "pair-invite", "pair-accept"] {
        assert_eq!(status(&h, Method::POST, &format!("/api/sessions/cc/{tail}"), &t).await, 403, "{tail}");
    }
    // Painel da orquestração: leitura também fica de fora.
    assert_eq!(status(&h, Method::GET, "/api/sessions/cc/orq/panel", &t).await, 403);
}

#[tokio::test]
async fn chat_globals_pass() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    assert_eq!(status(&h, Method::GET, "/api/model-options", &t).await, 200);
    assert_eq!(status(&h, Method::POST, "/api/pensamento/pt", &t).await, 200);
    assert_eq!(status(&h, Method::GET, "/api/tts/audio/abc", &t).await, 200);
}

#[tokio::test]
async fn token_with_two_sessions_reaches_both() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    h.add(&t, "dd");
    assert_eq!(status(&h, Method::GET, "/api/sessions/cc/history", &t).await, 200);
    assert_eq!(status(&h, Method::GET, "/api/sessions/dd/history", &t).await, 200);
    assert_eq!(status(&h, Method::GET, "/api/sessions/ee/history", &t).await, 403);
}

#[tokio::test]
async fn pair_session_is_read_only() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let p = h.token("yy", Kind::Pair);
    h.share.store.attach(&t, &p).unwrap();
    assert_eq!(status(&h, Method::GET, "/api/sessions/yy/history", &t).await, 200);
    assert_eq!(status(&h, Method::POST, "/api/sessions/yy/input", &t).await, 403);
    assert_eq!(status(&h, Method::GET, "/api/sessions/yy/file?path=/etc/passwd", &t).await, 403);
    assert_eq!(status(&h, Method::POST, "/api/sessions/cc/input", &t).await, 200);
}

#[tokio::test]
async fn guest_cannot_close_the_owner_session() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    assert_eq!(status(&h, Method::DELETE, "/api/sessions/cc", &t).await, 403);
}

#[tokio::test]
async fn revoked_is_410() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    h.revoke_all();
    let r = get_as(&h, "/api/sessions/cc/history", &t).await;
    assert_eq!(r.status(), 410);
    assert_eq!(detail_code(r).await, "erro_convite_encerrado");
}

#[tokio::test]
async fn mode_change_is_503_not_410() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    h.share.lives.facts().begin_mode_change("cc");
    let r = get_as(&h, "/api/sessions/cc/history", &t).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "5");
    assert_eq!(detail_code(r).await, "erro_sessao_indisponivel");
}

#[tokio::test]
async fn silent_mux_is_503_and_confirmed_absence_410() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    h.unsure("cc");
    let r = get_as(&h, "/api/sessions/cc/history", &t).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "5");
    *h.probe.has.lock().unwrap() = Some(false);
    assert_eq!(status(&h, Method::GET, "/api/sessions/cc/history", &t).await, 410);
}

#[tokio::test]
async fn open_routes_need_no_token() {
    let h = harness().await;
    assert_eq!(client().get(h.url("/convite/ABC")).send().await.unwrap().status(), 200);
    assert_eq!(client().get(h.url("/par/ABC")).send().await.unwrap().status(), 200);
    assert_eq!(client().post(h.url("/api/guest/redeem")).send().await.unwrap().status(), 200);
    assert_eq!(client().post(h.url("/api/pair/redeem")).send().await.unwrap().status(), 200);
    assert_eq!(client().get(h.url("/api/guest/redeem")).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn refusal_carries_cors_and_preflight_passes() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = client().get(h.url("/api/config")).bearer_auth(&t)
        .header("origin", "https://app-do-convidado.example").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    let pre = client().request(Method::OPTIONS, h.url("/api/sessions/cc/input"))
        .header("origin", "https://app-do-convidado.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization").send().await.unwrap();
    assert_eq!(pre.status(), 200);
}

#[tokio::test]
async fn pair_reads_only_what_is_listed() {
    let h = harness().await;
    let p = h.token("yy", Kind::Pair);
    for (path, want) in [("/api/sessions/yy/uploads/foto.png", 200), ("/api/sessions/yy/uploads", 403),
        ("/api/sessions/yy/runners", 403), ("/api/sessions/yy/project-shortcuts", 403),
        ("/api/sessions/yy/shortcut-terminals", 403), ("/api/sessions/yy/transcript-image", 200)] {
        assert_eq!(status(&h, Method::GET, path, &p).await, want, "{path}");
    }
}

#[test]
fn path_session_only_strips_term_from_the_terminal() {
    assert_eq!(path_session("/api/sessions/term-cc/term").as_deref(), Some("cc"));
    assert_eq!(path_session("/api/sessions/term-cc/history").as_deref(), Some("term-cc"));
    assert_eq!(path_session("/api/sessions/term-cc").as_deref(), Some("term-cc"));
    assert_eq!(path_session("/api/sessions/cc/history").as_deref(), Some("cc"));
    assert_eq!(path_session("/api/sessions/events"), None);
    assert_eq!(path_session("/api/sessions"), None);
}

/// O Python casa o nome com o `scope["path"]` já decodificado: espaço e acento chegam como `%20` e
/// `%C3%A7`, e a sessão é a mesma.
#[tokio::test]
async fn session_names_in_the_path_are_percent_decoded() {
    let h = harness().await;
    for name in ["a b", "ação"] {
        h.probe.keys.lock().unwrap().insert(name.into(), format!("L-{name}"));
    }
    let t = h.token("a b", Kind::Share);
    let p = h.token("ação", Kind::Pair);
    assert_eq!(path_session("/api/sessions/a%20b/history").as_deref(), Some("a b"));
    assert_eq!(path_session("/api/sessions/term-a%C3%A7%C3%A3o/term").as_deref(), Some("ação"));
    assert_eq!(status(&h, Method::GET, "/api/sessions/a%20b/history", &t).await, 200);
    assert_eq!(status(&h, Method::POST, "/api/sessions/a%20b/input", &t).await, 200);
    assert_eq!(status(&h, Method::GET, "/api/sessions/a%C3%A7%C3%A3o/history", &p).await, 200);
    assert_eq!(status(&h, Method::GET, "/api/sessions/a%20c/history", &t).await, 403);
}

#[tokio::test]
async fn shared_and_paired_on_same_token_pass_as_share() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let p = h.token("cc", Kind::Pair);
    h.share.store.attach(&t, &p).unwrap();
    assert_eq!(status(&h, Method::POST, "/api/sessions/cc/input", &t).await, 200);
}

#[tokio::test]
async fn pair_only_token_does_not_reach_globals() {
    let h = harness().await;
    let p = h.token("yy", Kind::Pair);
    for path in ["/api/engines", "/api/model-options", "/api/harness/codex/opcoes", "/api/tts/audio/abc"] {
        assert_eq!(status(&h, Method::GET, path, &p).await, 403, "{path}");
    }
}

#[test]
fn pair_routes_need_a_root_pair_record() {
    let grant = |roots: Vec<String>| GuestGrant {
        token: "t".into(), token_hash: "h".into(),
        sessions: HashMap::from([("cc".to_string(), Kind::Share)]), pair_roots: roots, invite: true,
    };
    assert!(!guest_allowed(&Method::POST, "/api/pair/message", &grant(vec![])));
    assert!(guest_allowed(&Method::POST, "/api/pair/message", &grant(vec!["cc".into()])));
    assert!(guest_allowed(&Method::DELETE, "/api/pair", &grant(vec!["cc".into()])));
    assert!(guest_allowed(&Method::POST, "/api/guest/attach", &grant(vec![])));
    assert!(guest_allowed(&Method::GET, "/api/sessions/events", &grant(vec![])));
}

// --- conexões longas ---

#[tokio::test]
async fn open_list_stream_ends_when_access_is_revoked() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = get_as(&h, "/api/sessions/events?n=500&ms=10", &t).await;
    let n = count_chunks(r, || h.revoke_all()).await;
    assert!((1..500).contains(&n), "{n}");
}

#[tokio::test]
async fn open_stream_survives_unsure() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    // 3 s: passa do cache de 2 s da vida, então o vigia relê de verdade o "não sei".
    let r = get_as(&h, "/api/sessions/cc/events?n=30&ms=100", &t).await;
    let n = count_chunks(r, || h.unsure("cc")).await;
    assert_eq!(n, 30);
}

#[tokio::test]
async fn open_stream_ends_when_absence_is_confirmed() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = get_as(&h, "/api/sessions/cc/events?n=500&ms=10", &t).await;
    let n = count_chunks(r, || {
        h.unsure("cc");
        *h.probe.has.lock().unwrap() = Some(false);
    }).await;
    assert!((1..500).contains(&n), "{n}");
}

// O fechamento 4410 pelos caminhos reais: `share_guest_serving` (terminal e WebSocket repassado).

#[tokio::test]
async fn guest_terminal_with_foreign_origin_passes_the_gate() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    for target in ["cc", "term-cc"] {
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(
            h.ws(&format!("/api/sessions/{target}/term?token={t}"))).unwrap();
        req.headers_mut().insert("origin", "https://app-do-convidado.example".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), tungstenite::Message::Text(format!("ok {target}").into()));
    }
}

#[tokio::test]
async fn websocket_refusals_are_a_bare_403_at_the_handshake() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    // Fora do convite, token desconhecido e 503 da troca de modo: todos viram o mesmo 403 vazio,
    // sem CORS (o CORSMiddleware não toca WebSocket) e com Connection: close.
    let want = (403, Vec::new(), Some("close".to_owned()), false);
    assert_eq!(handshake_refusal(&h.ws(&format!("/api/sessions/outra/term?token={t}"))).await, want);
    assert_eq!(handshake_refusal(&h.ws("/api/sessions/cc/term?token=x")).await, want);
    h.share.lives.facts().begin_mode_change("cc");
    assert_eq!(handshake_refusal(&h.ws(&format!("/api/sessions/cc/term?token={t}"))).await, want);
}

// --- foco da revisão ---

/// Uma conexão TCP só, pedidos em sequência: o limite é dela.
async fn one_connection(addr: SocketAddr) -> hyper::client::conn::http1::SendRequest<Body> {
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (sender, conn) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    sender
}

async fn send(conn: &mut hyper::client::conn::http1::SendRequest<Body>, token: &str, xff: &str) -> StatusCode {
    conn.ready().await.unwrap();
    let req = axum::http::Request::get("/api/sessions/cc/history")
        .header("host", "x").header("authorization", format!("Bearer {token}"))
        .header("x-forwarded-for", xff).body(Body::empty()).unwrap();
    let r = conn.send_request(req).await.unwrap();
    let status = r.status();
    let _ = http_body_util::BodyExt::collect(r.into_body()).await;
    status
}

/// Atrás do Funnel a conexão é reaproveitada entre clientes da internet: palpites de um não podem
/// travar o convidado certo que chega pela mesma conexão.
#[tokio::test]
async fn bad_tokens_never_lock_the_connection() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let mut c = one_connection(h.addr).await;
    for i in 0..20 {
        assert_eq!(send(&mut c, &format!("palpite-{i}"), &format!("203.0.113.{i}")).await, 401, "{i}");
    }
    assert_eq!(send(&mut c, &t, "198.51.100.1").await, 200);
}

#[tokio::test]
async fn inbound_guest_header_never_reaches_the_handler() {
    let h = harness().await;
    let t = h.token("cc", Kind::Share);
    let r = client().get(h.url("/api/sessions/cc/history")).bearer_auth(&t)
        .header("x-hangar-guest", "forjado").send().await.unwrap();
    let v: Value = json_of(r).await;
    assert_eq!(v["x_guest"], false);
}

#[tokio::test]
async fn proxy_strips_inbound_guest_header() {
    use crate::fake::*;
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client().get(format!("http://{srv}/api/qualquer")).header("x-hangar-guest", "forjado")
        .send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    let (_, headers) = fake.last_hit();
    assert!(!headers.contains_key("x-hangar-guest"));
}
