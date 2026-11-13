//! Portão do dono: quem pede, se é o dono, e as marcas que o servidor põe no pedido.
//! O repasse ao Python fica no servidor.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::auth::{self, Auth, AuthError, ReceiptContext, TrustedHosts};
use crate::diag::DiagClient;
use crate::guest::{Caller, GuestConn, GuestGrant, caller};

/// Marca do pedido repassado pela ponte: o `gate` o trata como do dono e o `pass` nunca o devolve ao Python.
#[derive(Clone, Copy, Debug)]
pub struct BridgedOwner;

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
#[derive(Clone, Default)]
pub struct RequestAuth(Arc<tokio::sync::Mutex<Option<(ReceiptContext, Option<String>)>>>);

impl RequestAuth {
    /// O recibo da admissão que o `gate` já fez neste pedido; só ele grava.
    pub async fn receipt(&self) -> Option<String> {
        self.0.lock().await.as_ref().and_then(|(_, receipt)| receipt.clone())
    }
}

/// Cliente já resolvido pelo `TrustedHosts`. Vai ao Python como valor único do X-Forwarded-For:
/// o uvicorn confia em 127.0.0.1 (o Rust) e chega ao mesmo cliente que chegaria sem o Rust.
#[derive(Clone)]
pub struct Forward {
    pub client_ip: String,
    pub https: bool,
    /// Convidado da 8766 decidido pelo porteiro; o `pass` do servidor assina o cabeçalho na hora do repasse.
    pub guest: Option<Arc<GuestGrant>>,
    /// Só uma admissão deste pedido, assinada pelo contexto privado do pai.
    pub auth_receipt: Option<String>,
}

/// O que um recurso precisa para decidir quem pede, sem o estado do servidor.
#[derive(Clone)]
pub struct Edge {
    pub trusted: TrustedHosts,
    pub auth: Auth,
    pub diag: DiagClient,
}

impl Edge {
    pub fn gate(&self, peer: SocketAddr, req: &Request)
        -> impl Future<Output = Result<(Forward, bool), AuthError>> + Send + use<> {
        gate(&self.trusted, &self.auth, peer, req)
    }
    pub fn owner_gate(&self, peer: SocketAddr, req: &Request)
        -> impl Future<Output = Result<Forward, Response>> + Send + use<> {
        owner_gate(&self.trusted, &self.auth, peer, req)
    }
    pub fn who(&self, peer: SocketAddr, req: &Request)
        -> impl Future<Output = Result<(Forward, Option<Caller>), AuthError>> + Send + use<> {
        who(&self.trusted, &self.auth, peer, req)
    }
    pub fn forward_for(&self, peer: SocketAddr, req: &Request) -> Forward {
        forward_for(&self.trusted, peer, req)
    }
    /// Só o dono passa. Convidado autenticado recebe 403 com `refused` (código e frase), sem contar
    /// falha de senha; token errado e indisponibilidade saem como no `gate`.
    pub fn owner_only(&self, peer: SocketAddr, req: &Request, refused: (&'static str, &'static str))
        -> impl Future<Output = Result<(), Response>> + Send + use<> {
        // Os dois portões leem o pedido agora; nada roda antes do `await`.
        let (gate, owner) = (self.gate(peer, req), self.owner_gate(peer, req));
        async move {
            match gate.await {
                Ok((_, true)) => Ok(()),
                Ok((_, false)) => match owner.await {
                    Ok(_) => Err(crate::http::detail(StatusCode::FORBIDDEN, refused.0, refused.1, serde_json::json!({}))),
                    Err(response) => Err(response),
                },
                Err(error) => Err(crate::auth::unavailable(error)),
            }
        }
    }
    pub fn route_failed(&self, req: &HeaderMap, event: &'static str, name: &str, code: &'static str, reason: &'static str) -> Response {
        route_failed(&self.diag, req, event, name, code, reason)
    }
}

pub fn gate(trusted: &TrustedHosts, auth: &Auth, peer: SocketAddr, req: &Request)
    -> impl Future<Output = Result<(Forward, bool), AuthError>> + Send + use<> {
    let (client_ip, https) = trusted.resolve(peer.ip(), req.headers());
    // A ponte privada do Codex com terminal recebe o pedido já autenticado pela porta do Python.
    let bridged = req.extensions().get::<BridgedOwner>().is_some();
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let source = if req.headers().get(header::AUTHORIZATION).is_some_and(|v| v.as_bytes().starts_with(b"Bearer ")) { "bearer" }
        else if auth::query_param(req.uri().query(), "token").is_some_and(|t| !t.is_empty()) { "query" }
        else if token.is_some() { "cookie" } else { "ausente" };
    let context = ReceiptContext { connection_kind: "private_upstream".into(), profile: auth::Profile::LegacyOwnerProbe,
        origin: client_ip.clone(), method: req.method().as_str().into(),
        raw_target: req.uri().path_and_query().map_or("/", |p| p.as_str()).into(),
        credential_source: source.into(), presented_token: token.clone() };
    let request_auth = req.extensions().get::<RequestAuth>().cloned().unwrap_or_default();
    let auth = auth.clone();
    async move {
    if bridged { return Ok((Forward { client_ip, https, guest: None, auth_receipt: None }, true)); }
    let mut admitted = request_auth.0.lock().await;
    if let Some((_, receipt)) = admitted.as_ref().filter(|(previous, _)| *previous == context) {
        return Ok((Forward { client_ip, https, guest: None, auth_receipt: receipt.clone() }, true));
    }
    let owner = auth.owner(&client_ip, token.as_deref()).await?;
    let receipt = if owner { auth.receipt(&context).await? } else { None };
    if owner { *admitted = Some((context, receipt.clone())); }
    Ok((Forward { client_ip, https, guest: None, auth_receipt: receipt }, owner))
    }
}

/// Porta exclusiva do dono: conta a recusa aqui, pois ela não passa pelo Python.
pub fn owner_gate(trusted: &TrustedHosts, auth: &Auth, peer: SocketAddr, req: &Request)
    -> impl Future<Output = Result<Forward, Response>> + Send + use<> {
    let admission = gate(trusted, auth, peer, req);
    let (_, https) = trusted.resolve(peer.ip(), req.headers());
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let auth = auth.clone();
    async move {
    let (forward, owner) = admission.await.map_err(auth::unavailable)?;
    if owner { return Ok(forward); }
    let decision = auth.evaluate(auth::AuthAttempt {
        profile: auth::Profile::RequireAuthHttp, origin: forward.client_ip.clone(),
        presented_token: token, verified_guest: false, login_guest: false,
    }).await.map_err(auth::unavailable)?;
    if decision.status == 0 { return Ok(forward); }
    let blocked = decision.status == auth::AuthDecision::BLOCKED.status;
    let (status, code, msg) = if blocked {
        (StatusCode::TOO_MANY_REQUESTS, "erro_muitas_tentativas", "muitas tentativas — aguarde")
    } else {
        (StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized")
    };
    let mut response = crate::http::json_response(status, serde_json::json!({"detail":{"code":code,"params":{},"msg":msg}}));
    if blocked { response.headers_mut().insert(header::RETRY_AFTER, axum::http::HeaderValue::from_static("30")); }
    Err(response)
    }
}

/// Entrou pela 8766 ou um porteiro de convidado já disse quem pede: o token do dono não decide.
fn guest_side(req: &Request) -> bool {
    on_guest_port(req) || caller(req).is_some()
}

/// Quem pede. Na 8766 só o convidado que o porteiro pôs (nunca `is_owner`, nem `X-Forwarded-For`);
/// na 8765, o dono pelo token.
pub fn who(trusted: &TrustedHosts, auth: &Auth, peer: SocketAddr, req: &Request)
    -> impl Future<Output = Result<(Forward, Option<Caller>), AuthError>> + Send + use<> {
    let guest = guest_side(req).then(|| (forward_for(trusted, peer, req), caller(req).cloned()));
    let owner = gate(trusted, auth, peer, req);
    async move {
    if let Some(guest) = guest { return Ok(guest); }
    let (fwd, owner) = owner.await?;
    Ok((fwd, owner.then_some(Caller::Owner)))
    }
}

/// O repasse de quem pede, sem decidir se é o dono. Convidado vai como loopback para o Python
/// aceitar o cabeçalho assinado.
pub fn forward_for(trusted: &TrustedHosts, peer: SocketAddr, req: &Request) -> Forward {
    if guest_side(req) {
        let guest = caller(req).and_then(Caller::grant).cloned();
        let client_ip = if guest.is_some() { "127.0.0.1".into() } else { peer.ip().to_string() };
        let https = !on_guest_port(req) && trusted.resolve(peer.ip(), req.headers()).1;
        return Forward { client_ip, https, guest, auth_receipt: None };
    }
    let (client_ip, https) = trusted.resolve(peer.ip(), req.headers());
    Forward { client_ip, https, guest: None, auth_receipt: None }
}

/// Chegou pela 8766: o `GuestConn` só existe no listener do convidado.
pub fn on_guest_port(req: &Request) -> bool {
    req.extensions().get::<ConnectInfo<GuestConn>>().is_some()
}

/// Falha do Rust numa rota que é dele: 503 com o código, que o app mostra, e uma linha no diário.
/// `detail` é o envelope que o `lerErro` do app já traduz; `reason` é frase fixa, nunca conversa.
pub fn route_failed(diag: &DiagClient, req: &HeaderMap, event: &'static str, name: &str, code: &'static str, reason: &'static str) -> Response {
    diag.report(event, name, code, reason);
    let body = serde_json::json!({"ok": false, "error_code": code, "message": reason,
        "detail": {"code": code, "params": {"motivo": reason}, "msg": format!("{reason} — {code}")}}).to_string();
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json")], body).into_response();
    crate::http::cors(req, resp.headers_mut());
    resp
}
