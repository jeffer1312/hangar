// crates/hangar-server/src/routes.rs
//! Rotas do hangar-server: saúde, histórico e chat ao vivo do Claude e do Codex para o dono;
//! todo o resto é repasse ao Python.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use http_body_util::BodyExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::proxy::{self, Forward, HttpClient};
use crate::side::{Binding, Hubs, INFO_TTL, Lease, Out, SideCtx, remember_info};
use crate::tail::{self, Watchers};
use crate::transcript::{InternalInfo, SKIPPED_LINES, history_etag, merged_history};

const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const REFUSED_WARN_EVERY: Duration = Duration::from_secs(60);

pub struct AppState {
    pub cfg: Config,
    pub auth: Auth,
    pub http: HttpClient,
    pub side: SideCtx,
    pub terminal: crate::terminal_control::TerminalPool,
    pub terminal_address: Option<SocketAddr>,
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_read_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_meta_slots: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(cfg: Config) -> AppState {
        Self::with_terminal_pool(cfg, crate::terminal_control::TerminalPool::new())
    }

    pub fn with_terminal_pool(cfg: Config, terminal: crate::terminal_control::TerminalPool) -> AppState {
        let http = proxy::client();
        let side = SideCtx {
            upstream: cfg.upstream,
            secret: cfg.internal_secret.clone(),
            http: http.clone(),
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            infos: Default::default(),
        };
        AppState { auth: Auth::new(&cfg.auth_token), http, side, cfg, terminal, terminal_address: None,
            workspace_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            workspace_read_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            workspace_meta_slots: Arc::new(tokio::sync::Semaphore::new(4)) }
    }

    /// `info` da sessão com cache curto: várias telas abrindo juntas viram uma consulta só. Só o
    /// `/events` usa, porque o primeiro `info` da conexão interna corrige um valor velho com `reset`.
    async fn info(&self, name: &str) -> Option<InternalInfo> {
        if let Some((at, v)) = self.side.infos.lock().unwrap().get(name) {
            if at.elapsed() < INFO_TTL {
                return v.clone();
            }
        }
        let v = fetch_info(&self.http, self.cfg.upstream, &self.cfg.internal_secret, name).await;
        remember_info(&self.side.infos, name, v.clone());
        v
    }
}

async fn fetch_info(http: &HttpClient, upstream: SocketAddr, secret: &str, name: &str) -> Option<InternalInfo> {
    let url = format!("http://{upstream}/internal/sessions/{}/info", utf8_percent_encode(name, NON_ALPHANUMERIC));
    let req = axum::http::Request::get(url).header("x-hangar-internal", secret).body(Body::empty()).ok()?;
    let resp = match tokio::time::timeout(INFO_TIMEOUT, http.request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(session = %name, "info interna falhou: {e}");
            return None;
        }
        Err(_) => {
            tracing::warn!(session = %name, "info interna sem resposta");
            return None;
        }
    };
    if !resp.status().is_success() {
        // 404 é sessão inexistente (normal) ou segredo recusado: `warn_if_internal_refused` separa.
        if resp.status() != StatusCode::NOT_FOUND {
            tracing::warn!(session = %name, status = %resp.status(), "info interna recusada");
        }
        return None;
    }
    let body = tokio::time::timeout(INFO_TIMEOUT, resp.into_body().collect()).await.ok()?.ok()?.to_bytes();
    match serde_json::from_slice(&body) {
        Ok(v) => Some(v),
        Err(e) => {
            // Só a posição: a mensagem do serde pode citar o valor.
            tracing::warn!(session = %name, line = e.line(), column = e.column(), "info interna inválida");
            None
        }
    }
}

/// O `/internal` responde 404 tanto à sessão inexistente quanto ao segredo recusado. Sem `info` e
/// com a sessão atendida pelo repasse, o atalho está desligado sem ninguém saber: avisa, no máximo
/// uma vez por minuto. true = avisou.
fn warn_if_internal_refused(name: &str, info_missing: bool, status: StatusCode) -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if !info_missing || !status.is_success() {
        return false;
    }
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < REFUSED_WARN_EVERY) {
        return false;
    }
    *last = Some(Instant::now());
    tracing::warn!(
        session = %name,
        "sem info interna, mas o Python atendeu a sessão: /internal recusou (segredo interno?) ou falhou; atalho do Rust desligado"
    );
    true
}

pub async fn serve(listener: TcpListener, cfg: Config) -> std::io::Result<()> {
    serve_with_terminal_pool(listener, cfg, crate::terminal_control::TerminalPool::new()).await
}

pub async fn serve_with_terminal_pool(listener: TcpListener, cfg: Config, pool: crate::terminal_control::TerminalPool) -> std::io::Result<()> {
    // Bind LAN específico não recebe tráfego de loopback: a observação tem uma porta própria.
    let private = TcpListener::bind("127.0.0.1:0").await?;
    let mut state = AppState::with_terminal_pool(cfg, pool);
    state.terminal_address = Some(private.local_addr()?);
    let state = Arc::new(state);
    tokio::select! {
        result = axum::serve(listener, router(state.clone()).into_make_service_with_connect_info::<SocketAddr>()) => result,
        result = axum::serve(private, terminal_router(state).into_make_service_with_connect_info::<SocketAddr>()) => result,
    }
}

pub fn terminal_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/__hangar_server/terminal", axum::routing::post(crate::terminal_routes::terminal))
        .route("/__hangar_server/workspace", axum::routing::post(crate::workspace_routes::private))
        .with_state(state)
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/__hangar_server/health", get(health))
        .route("/__hangar_server/terminal", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/workspace", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        // Outro método nessas rotas (preflight OPTIONS, HEAD) segue ao Python.
        .route("/api/sessions/{name}/history", get(history).fallback(pass_any))
        .route("/api/sessions/{name}/events", get(events).fallback(pass_any))
        .fallback(pass_any)
        .with_state(state)
}

async fn health(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let body = serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION"),
        "protocol": crate::INTERNAL_PROTOCOL,
        "terminal_address": st.terminal_address.map(|a| a.to_string())}).to_string();
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, bool) {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let owner = st.auth.is_owner(&client_ip, token.as_deref());
    (Forward { client_ip, https }, owner)
}

pub(crate) async fn pass(st: &AppState, req: Request, fwd: &Forward) -> Response {
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let resp = proxy::forward(&st.http, st.cfg.upstream, req, fwd).await;
    match resp.status() {
        StatusCode::UNAUTHORIZED => st.auth.record_fail(&fwd.client_ip),
        // WebSocket recusado antes do aceite chega como 403 e o Python já contou a falha.
        // Um 403 de origem também conta: só desliga o atalho, o lado seguro.
        StatusCode::FORBIDDEN if upgrade => st.auth.record_fail(&fwd.client_ip),
        StatusCode::TOO_MANY_REQUESTS => st.auth.mark_blocked(&fwd.client_ip),
        _ => {}
    }
    resp
}

async fn pass_any(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if crate::workspace_routes::matches(req.method(), req.uri().path()) {
        let (forward, owner) = gate(&st, peer, &req);
        if owner {
            let headers = req.headers().clone();
            let mut response = crate::workspace_routes::public(st, req, forward).await;
            cors(&headers, response.headers_mut());
            return response;
        }
    }
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    pass(&st, req, &Forward { client_ip, https }).await
}

/// Id de correlação do diário: cabeçalho x-hangar-req; o EventSource só manda ?diag_req.
fn diag_req(req: &Request) -> String {
    let h: String = req
        .headers()
        .get("x-hangar-req")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(32)
        .collect();
    if !h.is_empty() {
        return h;
    }
    auth::query_param(req.uri().query(), "diag_req")
        .filter(|c| (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .unwrap_or_default()
}

async fn history(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let limit = match auth::query_param(req.uri().query(), "limit") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            // Como o Python: `limit <= 0` é o histórico inteiro (api.py:2918), nunca um 400.
            Ok(n) if n > 0 => usize::try_from(n).ok(),
            Ok(_) => None,
            // Texto ou vazio fica com o FastAPI e o 422 dele.
            Err(_) => return pass(&st, req, &fwd).await,
        },
    };
    // Sem o cache: sessão recriada com o mesmo nome dentro do TTL devolveria a conversa morta, e
    // nada a corrigiria depois (o `/events` só a corrige com `reset` por estar ligado ao hub).
    let info = fetch_info(&st.http, st.cfg.upstream, &st.cfg.internal_secret, &name).await;
    remember_info(&st.side.infos, &name, info.clone());
    let info_missing = info.is_none();
    let Some(hreq) = info.and_then(|i| i.history_request(limit)) else {
        let resp = pass(&st, req, &fwd).await;
        warn_if_internal_refused(&name, info_missing, resp.status());
        return resp;
    };
    tracing::debug!(session = %name, req = %diag_req(&req), "history");
    let inm = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let log_name = name.clone();
    let done = tokio::task::spawn_blocking(move || -> std::io::Result<(Option<String>, Option<Vec<u8>>)> {
        let etag = history_etag(&hreq);
        if etag.is_some() && etag == inm {
            return Ok((etag, None));
        }
        let before = SKIPPED_LINES.load(Ordering::Relaxed);
        // Já vem com o corte `evs[-limit:]` da rota (Task 9).
        let evs = merged_history(&hreq)?;
        tail::log_skipped(&log_name, before);
        Ok((etag, Some(serde_json::to_vec(&evs)?)))
    })
    .await;
    let (etag, body) = match done {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!(session = %name, "history no Rust falhou; repassa: {e}");
            return pass(&st, req, &fwd).await;
        }
        Err(e) => {
            // Sem `{e}`: a mensagem do pânico pode citar texto da conversa.
            tracing::warn!(session = %name, panic = e.is_panic(), cancelled = e.is_cancelled(), "history no Rust caiu; repassa");
            return pass(&st, req, &fwd).await;
        }
    };
    let mut resp = match body {
        None => StatusCode::NOT_MODIFIED.into_response(),
        Some(body) => {
            let mut r = Response::new(Body::empty());
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let body = maybe_gzip(req.headers(), r.headers_mut(), body);
            *r.body_mut() = Body::from(body);
            r
        }
    };
    if let Some(v) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    cors(req.headers(), resp.headers_mut());
    resp
}

async fn events(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let info = st.info(&name).await;
    let Some(binding) = info.as_ref().and_then(Binding::from_info) else {
        let resp = pass(&st, req, &fwd).await;
        warn_if_internal_refused(&name, info.is_none(), resp.status());
        return resp;
    };
    // A query vence: o app recria o EventSource a cada queda, e objeto novo não manda o cabeçalho.
    let resume = auth::query_param(req.uri().query(), "last_event_id")
        .filter(|v| !v.is_empty())
        .or_else(|| req.headers().get("last-event-id").and_then(|v| v.to_str().ok()).map(str::to_owned));
    tracing::debug!(session = %name, req = %diag_req(&req), retomada = resume.is_some(), "events: abriu");
    let lease = st.side.hubs.acquire(&name, binding, &st.side);
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    tokio::spawn(client_loop(lease, resume, tx));
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<Bytes, Infallible>(b), rx))
    });
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(req.headers(), h);
    resp
}

async fn client_loop(lease: Lease, resume: Option<String>, out: mpsc::Sender<Bytes>) {
    let hub = lease.hub.clone();
    // O front dá 10 s para o primeiro quadro: o ping sai antes de qualquer leitura.
    if !push(&out, tail::ping_frame()).await {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    let mut resume = resume;
    loop {
        let Some(att) = hub.attach(resume.take()).await else {
            let _ = push(&out, tail::reset_frame()).await;
            return;
        };
        for f in att.frames {
            if !push(&out, f).await {
                return;
            }
        }
        let (generation, mut rx) = (att.generation, att.rx);
        let rebind = loop {
            tokio::select! {
                _ = out.closed() => return,
                _ = ping.tick() => if !push(&out, tail::ping_frame()).await { return },
                _ = comment.tick() => if !push(&out, tail::comment_frame()).await { return },
                msg = rx.recv() => match msg {
                    Ok(Out::Tail(g, f)) if g == generation => if !push(&out, f).await { return },
                    Ok(Out::Tail(..)) => {}
                    Ok(Out::Side(f)) => if !push(&out, f).await { return },
                    Ok(Out::Rebind) => break true,
                    Ok(Out::Close) | Err(broadcast::error::RecvError::Closed) => break false,
                    // Atrasado demais para o canal: fecha, e o aparelho retoma pelo último id.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::info!(session = %hub.name, "aparelho atrasado; conexão fechada para retomar");
                        return;
                    }
                },
            }
        };
        if !push(&out, tail::reset_frame()).await || !rebind {
            return;
        }
    }
}

/// Envio preso por 30 s fecha a conexão, como o send_timeout do Python.
async fn push(out: &mpsc::Sender<Bytes>, frame: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(frame)).await, Ok(Ok(())))
}

/// CORS das respostas do próprio Rust, como o CORSMiddleware do Python: `*`, sem credenciais,
/// ETag legível pelo JS. Preflight não chega aqui: vai sem token e é repassado.
pub(crate) fn cors(req: &HeaderMap, resp: &mut HeaderMap) {
    if req.contains_key(header::ORIGIN) {
        resp.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        resp.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("ETag"));
    }
}

/// Gzip só quando o cliente pede e o corpo passa de 1 KB. SSE nunca: o gzip seguraria os quadros.
pub(crate) fn maybe_gzip(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>) -> Vec<u8> {
    let wants = req
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let sse = resp
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !wants || sse || body.len() < 1024 {
        return body;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
    std::io::Write::write_all(&mut enc, &body).expect("escrita em memória");
    resp.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    resp.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    enc.finish().expect("escrita em memória")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Estado com um Python mínimo: `/ws` recusa com 403, `/limited` responde 429.
    async fn state_with_upstream() -> Arc<AppState> {
        use axum::routing::any;
        let app = Router::new()
            .route("/ws", any(|| async { StatusCode::FORBIDDEN }))
            .route("/limited", any(|| async { StatusCode::TOO_MANY_REQUESTS }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            internal_secret: "s".into(),
            auth_token: "dono".into(),
            log_path: None,
            trusted: auth::TrustedHosts::parse("127.0.0.1"),
        };
        Arc::new(AppState::new(cfg))
    }

    fn request(path: &str, ip: &str, ws: bool, token: &str) -> Request {
        let mut b = Request::builder()
            .uri(path)
            .header("x-forwarded-for", ip)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if ws {
            b = b.header("connection", "Upgrade").header("upgrade", "websocket");
        }
        b.body(axum::body::Body::empty()).unwrap()
    }

    fn is_owner(st: &AppState, ip: &str) -> bool {
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        gate(st, peer, &request("/x", ip, false, "dono")).1
    }

    #[tokio::test]
    async fn websocket_403_and_429_from_python_lock_the_shortcut() {
        let st = state_with_upstream().await;
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let fwd = |ip: &str| gate(&st, peer, &request("/x", ip, false, "x")).0;

        // 8 palpites errados em WebSocket: o Python conta e fecha com 403.
        for _ in 0..8 {
            let r = pass(&st, request("/ws", "198.51.100.7", true, "errado"), &fwd("198.51.100.7")).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
        }
        assert!(!is_owner(&st, "198.51.100.7"), "o token certo não abre o atalho nessa origem");
        assert!(is_owner(&st, "198.51.100.8"), "outra origem segue normal");

        // Um 429 do Python satura a origem de uma vez.
        let r = pass(&st, request("/limited", "198.51.100.9", false, "errado"), &fwd("198.51.100.9")).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(!is_owner(&st, "198.51.100.9"));

        // 403 sem Upgrade não é falha de token.
        let st2 = state_with_upstream().await;
        for _ in 0..8 {
            pass(&st2, request("/ws", "198.51.100.10", false, "x"), &fwd("198.51.100.10")).await;
        }
        assert!(is_owner(&st2, "198.51.100.10"));

        // Loopback é isento.
        let r = pass(&st, request("/limited", "127.0.0.1", false, "x"), &fwd("127.0.0.1")).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(is_owner(&st, "127.0.0.1"));
    }

    #[test]
    fn missing_info_on_a_session_python_serves_warns_once_a_minute() {
        assert!(!warn_if_internal_refused("s", true, StatusCode::NOT_FOUND), "sessão inexistente é normal");
        assert!(!warn_if_internal_refused("s", false, StatusCode::OK), "provider fora do Rust é normal");
        assert!(warn_if_internal_refused("s", true, StatusCode::OK));
        assert!(!warn_if_internal_refused("s", true, StatusCode::OK), "no máximo uma vez por minuto");
    }

    #[test]
    fn cors_only_with_origin() {
        let mut resp = HeaderMap::new();
        cors(&HeaderMap::new(), &mut resp);
        assert!(resp.is_empty());
        let mut req = HeaderMap::new();
        req.insert(header::ORIGIN, HeaderValue::from_static("http://outra"));
        cors(&req, &mut resp);
        assert_eq!(resp[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(resp[header::ACCESS_CONTROL_EXPOSE_HEADERS], "ETag");
    }

    #[test]
    fn gzip_only_when_asked_and_large() {
        let big = vec![b'a'; 2048];
        let mut resp = HeaderMap::new();
        assert_eq!(maybe_gzip(&HeaderMap::new(), &mut resp, big.clone()), big);
        let mut req = HeaderMap::new();
        req.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert_eq!(maybe_gzip(&req, &mut resp, b"curto".to_vec()), b"curto");
        assert!(resp.get(header::CONTENT_ENCODING).is_none());
        let packed = maybe_gzip(&req, &mut resp, big.clone());
        assert_eq!(resp[header::CONTENT_ENCODING], "gzip");
        assert_eq!(resp[header::VARY], "Accept-Encoding");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, big);

        let mut sse = HeaderMap::new();
        sse.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
        assert_eq!(maybe_gzip(&req, &mut sse, big.clone()), big);
        assert!(sse.get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn diag_req_prefers_header_then_valid_query() {
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=abc_1-Z")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "abc_1-Z");
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=tem%20espaco")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "");
        let long = "x".repeat(40);
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=q")
            .header("x-hangar-req", long.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "x".repeat(32));
    }
}
