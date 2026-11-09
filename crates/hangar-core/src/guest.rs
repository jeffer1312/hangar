//! O convidado como o portão o vê: o registro do convite, o que o token alcança e as marcas que o
//! porteiro da 8766 põe no pedido. Quem decide o acesso continua no servidor.
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::extract::connect_info::Connected;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::serve::IncomingStream;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

/// Fecha o WebSocket quando o acesso acaba: espelho do HTTP 410, o cliente não reconecta.
pub const CLOSE_ENDED: u16 = 4410;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Share,
    Pair,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub id: String,
    pub session: String,
    pub life: Option<String>,
    pub created_at: f64,
    pub code_expires_at: f64,
    pub code_hash: String,
    #[serde(default)]
    pub token_hash: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub redeemed_at: Option<f64>,
    #[serde(default)]
    pub revoked_at: Option<f64>,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Nível que o convite de par dará no resgate. No arquivo: reiniciar não pode rebaixá-lo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Kind>,
}

impl Share {
    pub fn active(&self, now: f64) -> bool {
        self.revoked_at.is_none() && (self.redeemed_at.is_some() || now <= self.code_expires_at)
    }
}

/// Tudo o que um token de convidado alcança: um registro por sessão.
#[derive(Debug, Clone)]
pub struct Guest {
    pub token_hash: String,
    pub records: Vec<Share>,
    /// Tem convite de compartilhamento vivo (não acesso vindo de par): só ele gasta o crédito do dono.
    pub invite: bool,
}

impl Guest {
    pub fn share_for(&self, session: &str) -> Option<&Share> {
        let cand: Vec<&Share> = self.records.iter().filter(|s| s.session == session).collect();
        let live: Vec<&Share> = cand.iter().copied().filter(|s| s.revoked_at.is_none()).collect();
        // Só revogado: devolve mesmo assim, para o porteiro dizer "encerrado" e não "fora do convite".
        let pool = if live.is_empty() { cand } else { live };
        // Compartilhada E pareada sob o mesmo token: o convite vence a cópia do par.
        let key = |s: &Share| (s.kind == Kind::Share, s.created_at);
        pool.into_iter().fold(None, |best: Option<&Share>, s| match best {
            Some(b) if key(s).partial_cmp(&key(b)) != Some(std::cmp::Ordering::Greater) => Some(b),
            _ => Some(s),
        })
    }
    pub fn sessions(&self) -> HashSet<String> {
        self.records.iter().filter(|s| s.revoked_at.is_none()).map(|s| s.session.clone()).collect()
    }
    pub fn sees(&self, name: &str) -> bool {
        self.records.iter().any(|s| s.revoked_at.is_none() && s.session == name)
    }
    pub fn kind_of(&self, name: &str) -> Option<Kind> {
        self.share_for(name).map(|s| s.kind)
    }
    pub fn pair_shares(&self) -> Vec<&Share> {
        self.records.iter()
            .filter(|s| s.kind == Kind::Pair && s.parent_id.is_none() && s.revoked_at.is_none())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum Caller {
    Owner,
    Guest(Arc<GuestGrant>),
}

/// Fotografia do token no momento do pedido: sessões vivas e o nível de cada uma.
#[derive(Clone, Debug)]
pub struct GuestGrant {
    pub token: String,
    pub token_hash: String,
    pub sessions: HashMap<String, Kind>,
    /// Sessões dos registros de par que são raiz (não cópia de `attach`).
    pub pair_roots: Vec<String>,
    /// Convite de compartilhamento de verdade: só ele alcança as rotas globais do chat.
    pub invite: bool,
}

impl Caller {
    /// Quem pode usar a sessão nesse método: o dono sempre; o convidado só nas do convite, e o de
    /// par só lendo. O recorte fino por rota é do porteiro; este é o do handler.
    pub fn can(&self, name: &str, method: &Method) -> bool {
        match self {
            Caller::Owner => true,
            Caller::Guest(grant) => match grant.sessions.get(name) {
                Some(Kind::Share) => true,
                Some(Kind::Pair) => *method == Method::GET,
                None => false,
            },
        }
    }

    pub fn grant(&self) -> Option<&Arc<GuestGrant>> {
        match self { Caller::Guest(grant) => Some(grant), Caller::Owner => None }
    }
}

/// O `Caller` que o porteiro da 8766 pôs no pedido; na 8765 nunca há um.
pub fn caller(req: &Request) -> Option<&Caller> {
    req.extensions().get::<Caller>()
}

impl GuestGrant {
    pub fn new(token: &str, guest: &Guest) -> Self {
        let sessions = guest.sessions().into_iter()
            .filter_map(|name| guest.kind_of(&name).map(|kind| (name, kind)))
            .collect();
        let pair_roots = guest.pair_shares().into_iter().map(|s| s.session.clone()).collect();
        Self { token: token.into(), token_hash: guest.token_hash.clone(), sessions, pair_roots, invite: guest.invite }
    }
}

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

pub fn is_websocket(req: &Request) -> bool {
    req.headers().get(header::UPGRADE).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

/// O preflight que o `CORSMiddleware` do Python (por fora do porteiro) respondia com `*` em origem,
/// método e cabeçalho, sem credenciais.
pub fn preflight(req: &Request) -> Option<Response> {
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
