//! Porteiro da porta do convidado (8766) e vigia das conexões longas.
//!
//! O filtro delimita a INTERFACE do convidado, não é fronteira de segurança: quem entra numa
//! sessão em bypass já roda comando como o dono.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::serve::IncomingStream;
use futures_util::StreamExt;
use tokio::net::TcpListener;

use super::ShareState;
use super::caller::{Caller, GuestGrant, SessionPath, allowed};
use super::life::{Verdict, verdict};

pub const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Fecha o WebSocket quando o acesso acaba: espelho do HTTP 410, o cliente não reconecta.
pub const CLOSE_ENDED: u16 = 4410;

/// Dado de cada conexão TCP. Sem limite de tokens errados: atrás do Funnel a conexão é dividida
/// entre clientes da internet, e o token de 256 bits não se adivinha.
/// Servir com `into_make_service_with_connect_info::<GuestConn>()` num `TcpListener` puro.
#[derive(Clone, Debug)]
pub struct GuestConn {
    pub peer: SocketAddr,
    /// Endereço desta ponta da conexão: decide entre o link do Funnel e o da rede da casa.
    pub local: Option<SocketAddr>,
}

impl Connected<IncomingStream<'_, TcpListener>> for GuestConn {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self { peer: *stream.remote_addr(), local: stream.io().local_addr().ok() }
    }
}

pub struct Gate {
    pub share: Arc<ShareState>,
    pub watch_every: Duration,
}

impl Gate {
    pub fn new(share: Arc<ShareState>) -> Self {
        Self { share, watch_every: WATCH_EVERY }
    }
}

/// O que o handler de WebSocket precisa para o vigia; o porteiro põe nas extensões.
#[derive(Clone)]
pub struct Watcher {
    share: Arc<ShareState>,
    token: String,
    session: Option<String>,
    every: Duration,
}

impl Watcher {
    /// Resolve só num fim de verdade: "não sei" (troca de modo, tmux mudo) mantém aberto.
    pub async fn until_ended(&self) {
        loop {
            tokio::time::sleep(self.every).await;
            let ended = match self.share.store.lookup_token(&self.token) {
                None => true,
                Some(guest) => verdict(&guest, self.session.as_deref(), &self.share.lives).await == Verdict::Ended,
            };
            if ended {
                return;
            }
        }
    }

    /// Fim do acesso de quem tem vigia; sem vigia (o dono) nunca resolve.
    pub async fn ended(watcher: Option<&Watcher>) {
        match watcher {
            Some(w) => w.until_ended().await,
            None => std::future::pending().await,
        }
    }
}

fn is_open(method: &Method, path: &str) -> bool {
    method == Method::OPTIONS || path.starts_with("/convite/") || path.starts_with("/par/")
        || (method == Method::POST && (path == "/api/guest/redeem" || path == "/api/pair/redeem"))
}

/// Só Bearer ou `?token=`: cookie nunca, porque a origem do convidado não é a do dono.
fn token_of(req: &Request) -> String {
    if let Some(t) = req.headers().get(header::AUTHORIZATION).and_then(|v| v.as_bytes().strip_prefix(b"Bearer ")) {
        return String::from_utf8_lossy(t).into_owned();
    }
    // `parse_qs` descarta valor vazio e fica com o primeiro.
    req.uri().query().and_then(|q| {
        form_urlencoded::parse(q.as_bytes()).find(|(k, v)| k == "token" && !v.is_empty()).map(|(_, v)| v.into_owned())
    }).unwrap_or_default()
}

fn is_websocket(req: &Request) -> bool {
    req.headers().get(header::UPGRADE).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

async fn deny(req: Request, status: StatusCode, code: &str, msg: &str) -> Response {
    if is_websocket(&req) {
        // O que o uvicorn responde ao `close` antes do `accept`: 403 vazio, sem CORS, e a
        // conexão fecha. Os clientes tratam a recusa no aperto de mão.
        return (StatusCode::FORBIDDEN, [(header::CONNECTION, "close")]).into_response();
    }
    let body = serde_json::json!({"detail": {"code": code, "params": {}, "msg": msg}}).to_string();
    let mut resp = (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
    if status == StatusCode::SERVICE_UNAVAILABLE {
        resp.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    }
    // O CORS do Python fica por fora do porteiro: a recusa também leva o cabeçalho.
    crate::routes::cors(req.headers(), resp.headers_mut());
    resp
}

/// O preflight que o `CORSMiddleware` do Python (por fora do porteiro) respondia com `*` em origem,
/// método e cabeçalho, sem credenciais.
pub(crate) fn preflight(req: &Request) -> Option<Response> {
    let h = req.headers();
    if req.method() != Method::OPTIONS || !h.contains_key(header::ORIGIN) || !h.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD) {
        return None;
    }
    let (status, body) = if h.contains_key("access-control-request-private-network") {
        (StatusCode::BAD_REQUEST, "Disallowed CORS private-network")
    } else {
        (StatusCode::OK, "OK")
    };
    let mut resp = (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response();
    let out = resp.headers_mut();
    out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    out.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT"));
    out.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    if let Some(asked) = h.get(header::ACCESS_CONTROL_REQUEST_HEADERS) {
        out.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, asked.clone());
    }
    Some(resp)
}

/// Middleware da 8766. Ordem das recusas: 401 → 403 sessão fora → 410/503 → 403 por rota.
pub async fn guard(State(gate): State<Arc<Gate>>, ConnectInfo(conn): ConnectInfo<GuestConn>, mut req: Request, next: Next) -> Response {
    // Os handlers continuam lendo o par TCP como na 8765.
    req.extensions_mut().insert(ConnectInfo(conn.peer));
    if let Some(local) = conn.local {
        req.extensions_mut().insert(super::guest_routes::LocalAddr(local));
    }
    // Só o Rust assina esse cabeçalho; vindo de fora é forjado.
    req.headers_mut().remove(super::forward::HEADER);
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    if let Some(resp) = preflight(&req) {
        return resp;
    }
    if is_open(&method, &path) {
        return next.run(req).await;
    }
    let token = token_of(&req);
    let Some(guest) = gate.share.store.lookup_token(&token) else {
        return deny(req, StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized").await;
    };
    let parsed = SessionPath::parse(&path);
    let session = parsed.as_ref().map(|p| p.session.clone());
    // `%2F` no nome: o Starlette nunca casava `/` dentro de `{name}`, e o handler leria outra sessão.
    if session.as_deref().is_some_and(|s| s.contains('/') || guest.share_for(s).is_none()) {
        return deny(req, StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessao compartilhada").await;
    }
    match verdict(&guest, session.as_deref(), &gate.share.lives).await {
        Verdict::Ended => {
            return deny(req, StatusCode::GONE, "erro_convite_encerrado", "compartilhamento encerrado").await;
        }
        Verdict::Unsure => {
            return deny(req, StatusCode::SERVICE_UNAVAILABLE, "erro_sessao_indisponivel", "sessao indisponivel por instantes").await;
        }
        Verdict::Ok => {}
    }
    let grant = Arc::new(GuestGrant::new(&token, &guest));
    if !allowed(&method, &path, parsed.as_ref(), &grant) {
        return deny(req, StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessao compartilhada").await;
    }
    req.extensions_mut().insert(Caller::Guest(grant));
    if let Some(parsed) = parsed {
        req.extensions_mut().insert(parsed);
    }
    let websocket = is_websocket(&req);
    if !websocket && !path.ends_with("/events") {
        return next.run(req).await;
    }
    // Stream e WebSocket passam pelo porteiro uma vez só: sem o vigia, revogar não derrubaria
    // um terminal ou uma lista já abertos.
    let watcher = Watcher { share: gate.share.clone(), token, session, every: gate.watch_every };
    if websocket {
        req.extensions_mut().insert(watcher);
        return next.run(req).await;
    }
    next.run(req).await.map(|body| watch(body, watcher))
}

/// Corpo que termina quando o acesso acaba de verdade.
pub fn watch(body: Body, watcher: Watcher) -> Body {
    let (mut tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        // Sai sozinho quando o corpo é largado (cliente foi embora ou stream acabou).
        let ended = tokio::select! {
            _ = watcher.until_ended() => true,
            _ = tx.closed() => false,
        };
        if ended {
            let _ = tx.send(());
        }
    });
    let stream = futures_util::stream::unfold((body.into_data_stream(), rx), |(mut data, mut rx)| async move {
        tokio::select! {
            biased;
            _ = &mut rx => None,
            item = data.next() => item.map(|item| (item, (data, rx))),
        }
    });
    Body::from_stream(stream)
}
