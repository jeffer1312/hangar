//! Rotas de escrita de sessão Claude e Codex. O Rust as reivindica na tabela (`table`) e decide por
//! pedido; `/input` e `/steer` (`input`), o controle (`control`) e `/answer` (`answer`) têm corpo no
//! Rust; as rotas só do Codex moram em `codex`. Sessão do Rust com corpo que a rota não aceita recebe
//! `erro_corpo_invalido` (`invalid_body`), nunca o repasse.
//!
//! Ordem fixa de `admit`: quem pede (dono, ou convidado na sessão do convite) → porta (`enter`) → entrada (`writable`) → decisão → corpo. A entrada
//! é procurada DEPOIS da porta: a achada antes de esperar pode ser a que o relançamento parou.
//! Repasse nunca acontece com o passe na mão: o `freeze` do Python fecha a porta e esperaria o
//! nosso próprio passe.
pub mod account;
pub mod answer;
pub mod bridge;
pub mod btw;
pub mod codex;
pub mod control;
pub mod engine;
pub mod input;
pub(crate) mod delivery_timing;
pub mod limit_switch;
mod model_effort;
pub mod permissions;
pub mod relaunch;
pub mod resume;
pub mod table;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::Request;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};

use crate::proxy::Forward;
use crate::routes::{AppState, cors, pass, who};
use crate::runtime::gateway::{RuntimeRegistry, WriteTarget};
use crate::runtime::ingress::{GateClosed, IngressPass};
pub use table::{Owner, Provider, WriteRoute, decide};

/// Mesmo teto do middleware de corpo do Python (`uploads.MAX_BYTES`), que recusa antes da rota.
pub(crate) const BODY_LIMIT: usize = 100 * 1024 * 1024;
const BUSY_MSG: &str = "A sessão está trocando de agente; tente novamente quando terminar.";
const INVALID_BODY_MSG: &str = "Pedido em formato inválido para esta rota; nada foi enviado.";
const UNHEALTHY_MSG: &str = "A sessão está com problema no Rust; tente de novo em instantes.";

/// O que a rota precisa para escrever: o passe mantém a porta aberta até o fim da escrita.
pub(crate) struct Ctx {
    pub(crate) st: Arc<AppState>,
    pub(crate) name: String,
    pub(crate) target: WriteTarget,
    // Só segura a porta: o `Drop` a solta quando a escrita termina.
    pub(crate) _pass: IngressPass,
    parts: Parts,
}

impl Ctx {
    pub(crate) fn headers(&self) -> &axum::http::HeaderMap { &self.parts.headers }
}

/// Envelope do `HTTPException(detail=erro(...))` do Python (`mensagens.py`).
pub(crate) fn detail(status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
    json_response(status, detail_body(code, msg, params))
}

pub fn detail_body(code: &str, msg: &str, params: Value) -> Value {
    json!({"detail": {"code": code, "params": params, "msg": msg}})
}

pub(crate) fn json_response(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// Entra na porta e SÓ ENTÃO procura a entrada; sem entrada o passe volta a sair (solto aqui).
pub async fn enter_then_find(runtime: &RuntimeRegistry, name: &str, wait: Duration) -> Result<Option<(IngressPass, WriteTarget)>, GateClosed> {
    let pass = runtime.ingress().enter(name, wait).await?;
    Ok(runtime.writable(name).await.map(|target| (pass, target)))
}

/// Nome da sessão em `/api/sessions/{name}/...`, já sem o escape da URL.
pub(crate) fn session_name(path: &str) -> Option<String> {
    let raw = path.strip_prefix("/api/sessions/")?.split('/').next().filter(|n| !n.is_empty())?;
    percent_decode_str(raw).decode_utf8().ok().map(|n| n.into_owned())
}

pub(crate) async fn forward_whole(st: &AppState, parts: Parts, bytes: Bytes, fwd: &Forward) -> Response {
    pass(st, Request::from_parts(parts, Body::from(bytes)), fwd).await
}

/// Decide quem atende. `Ok`: o Rust atende, com o passe na mão e o corpo já lido. `Err`: a resposta
/// pronta (repasse ao Python com o corpo intacto, recusa da porta fechada, ou a recusa do Rust).
pub(crate) async fn admit(st: &Arc<AppState>, peer: SocketAddr, req: Request, route: WriteRoute) -> Result<(Ctx, Bytes), Response> {
    let (fwd, who) = who(st, peer, &req);
    let runtime = st.state.runtime.get().cloned();
    let name = session_name(req.uri().path()).filter(|n| who.as_ref().is_some_and(|c| c.can(n, req.method())));
    let (Some(runtime), Some(name)) = (runtime, name) else {
        return Err(pass(st, req, &fwd).await);
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return Err(too_large(&parts.headers)) };
    let shaped = table::body_ok(route, &bytes);
    match route_write(&runtime, &name, route, st.write_gate_wait).await {
        Write::Rust(_, _) if !shaped => Err(invalid_body(&parts.headers)),
        Write::Rust(pass_in, target) => Ok((Ctx { st: st.clone(), name, target, _pass: pass_in, parts }, bytes)),
        Write::Python => Err(forward_whole(st, parts, bytes, &fwd).await),
        Write::Busy => {
            let mut response = json_response(StatusCode::CONFLICT, busy_body());
            cors(&parts.headers, response.headers_mut());
            Err(response)
        }
        Write::Opening(key) => Err(input::opening_response(st, &name, &key, &parts.headers, &bytes).await),
        Write::Failed(error) => {
            let mut response = detail(failed_status(&error), &error.code, &error.message, json!({}));
            cors(&parts.headers, response.headers_mut());
            Err(response)
        }
    }
}

/// Recusa do Rust a uma sessão dele: esperar a conversa abrir é 409; o resto, 503.
pub(crate) fn failed_status(error: &crate::runtime::protocol::RuntimeError) -> StatusCode {
    if error.code == crate::runtime::codex_relaunch::STARTING { StatusCode::CONFLICT } else { StatusCode::SERVICE_UNAVAILABLE }
}

/// Igual ao `_BodySizeLimitMiddleware`: texto puro, e o CORS por fora dele.
pub(crate) fn too_large(headers: &axum::http::HeaderMap) -> Response {
    let mut response = (StatusCode::PAYLOAD_TOO_LARGE, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "request body too large").into_response();
    cors(headers, response.headers_mut());
    response
}

/// Quem atende uma escrita. `Failed`: a sessão é do Rust e não pôde ser atendida (entrada doente,
/// Codex com terminal que não deu para adotar); a escrita nunca cai no Python.
/// `Opening`: texto para o Codex com terminal que ainda abre a conversa, com a chave da fila dela.
pub(crate) enum Write { Rust(IngressPass, WriteTarget), Python, Busy, Opening(String), Failed(crate::runtime::protocol::RuntimeError) }

/// Porta, entrada e tabela, nessa ordem. O passe só volta com `Rust`: repasse nunca o segura. Sem
/// entrada, o Codex com terminal do Rust é adotado na hora (`adopt_now`) e a entrada procurada de novo;
/// Codex com terminal ainda abrindo a conversa: texto vira `Opening`, o resto espera (`codex_terminal_starting`);
/// sem entrada nem Codex com terminal (Pi/omp/Kimi), segue ao Python.
pub(crate) async fn route_write(runtime: &RuntimeRegistry, name: &str, route: WriteRoute, wait: Duration) -> Write {
    use crate::runtime::protocol::RuntimeError;
    let (pass_in, target) = match enter_then_find(runtime, name, wait).await {
        Err(GateClosed) => return Write::Busy,
        Ok(Some(found)) => found,
        Ok(None) => match runtime.adopt_now(name).await {
            None => return Write::Python,
            Some(Err(error)) if error.code == crate::runtime::codex_relaunch::STARTING && route == WriteRoute::Input => {
                return match runtime.codex_opening(name).await {
                    Some(Ok(key)) => Write::Opening(key),
                    Some(Err(other)) => Write::Failed(other),
                    None => Write::Failed(error),
                };
            }
            Some(Err(error)) => return Write::Failed(error),
            Some(Ok(())) => match enter_then_find(runtime, name, wait).await {
                Err(GateClosed) => return Write::Busy,
                Ok(Some(found)) => found,
                Ok(None) => return Write::Failed(RuntimeError::new("codex_terminal_unadopted", "o Codex com terminal foi adotado e fechou em seguida")),
            },
        },
    };
    let Some(provider) = Provider::from_str(&target.provider) else { return Write::Python };
    if decide(route, provider, target.terminal) == Owner::Python { return Write::Python; }
    // Exceção da entrada doente: o modo de permissão do Codex sem terminal cujo processo caiu e não voltou
    // é lido e gravado no arquivo da sessão, e vale na próxima subida (como no Python).
    if !target.healthy && route == WriteRoute::CodexPermission && dead_headless_codex(&target).await { return Write::Rust(pass_in, target); }
    if !target.healthy {
        // O 503 fica; a reabertura que o Python fazia antes de entregar é pedida agora.
        runtime.request_reopen(name);
        return Write::Failed(RuntimeError::new("runtime_unhealthy", UNHEALTHY_MSG));
    }
    Write::Rust(pass_in, target)
}

async fn dead_headless_codex(target: &WriteTarget) -> bool {
    target.provider == "codex" && !target.terminal && target.handle.view().await.is_ok_and(|view| view["alive"] == false)
}

pub(crate) fn busy_body() -> Value { detail_body("session_transfer_busy", BUSY_MSG, json!({})) }

/// Recusa por sessão ocupada (trocando de agente ou nas frases do `_OCUPADA`); `None` = outra recusa.
pub(crate) fn busy_refusal(error: &crate::runtime::protocol::RuntimeError) -> Option<(StatusCode, Value)> {
    if error.code == "session_transfer_busy" { return Some((StatusCode::CONFLICT, busy_body())); }
    crate::runtime::claude_reopen::busy_message(&error.code).map(|message| (StatusCode::CONFLICT, detail_body(&error.code, message, json!({}))))
}

/// Corpo que a rota do Rust não aceita: 422 com código, nunca o repasse ao Python.
pub(crate) fn invalid_body(headers: &axum::http::HeaderMap) -> Response {
    let mut response = detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", INVALID_BODY_MSG, json!({}));
    cors(headers, response.headers_mut());
    response
}
