//! Rotas dos apps para a interface dos mods de uma sessão sem terminal que o Rust atende como
//! superfície remota: clique (`press`), fechar painel (`close`), troca de aba (`show`) e digitação
//! (`input`). Sessão que o Rust não atende assim recebe daqui a recusa fixa de cada rota: não há outro
//! dono para ela.
//!
//! A sessão com terminal que o Rust atende (fase 3) usa as mesmas rotas, com o clique pela tela. O
//! convidado já liberado pelo porteiro (convite da 8766 ou login) é atendido aqui como o dono; quem não
//! é nenhum dos dois segue ao Python, que o autentica. A digitação é recusada logo na entrada, sem
//! esperar a vez da sessão nem entrar na porta da troca de agente.
//!
//! A guarda da troca de agente é a porta de entrada da sessão no Rust (`IngressGates`), a mesma das
//! escritas do Claude: a troca a fecha retida pela operação exclusiva inteira e enquanto o registro não
//! termina. A operação de mod segura o passe até a resposta, então a troca que começa no meio espera ela
//! acabar, e nenhuma operação pergunta ao Python.
//!
//! Prazo: o app desiste em 8 s, contados de quando mandou o pedido. A espera pela vez da sessão, a
//! guarda e a chamada ao mod dividem um orçamento só, medido desde a entrada na rota, para que a
//! resposta (recusa incluída) chegue antes de o app mostrar erro.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::time::Instant;

use super::http::{fits, invalid, reply};
use super::model::*;
use super::state::Turn;
use crate::routes::{AppState, pass, route_failed, who};
use crate::share::caller::Caller;
use crate::runtime::ingress::IngressPass;
use crate::session_write::busy_body;

const BODY_LIMIT: usize = 64 * 1024;
/// Orçamento do pedido inteiro, desde a entrada: abaixo dos 8 s em que o app desiste, com folga para a
/// volta da resposta.
pub(crate) const REQUEST_BUDGET: Duration = Duration::from_millis(7500);
/// O `onPress` do mod costuma abrir ou copiar sem `await`: o efeito pode chegar logo depois do press
/// (mesma espera do `plugin_click.EFFECT_S`).
const EFFECT_WAIT: Duration = Duration::from_millis(300);
const VALUE_MAX: usize = 16384;
const TRANSFER_REASON: &str = "o servidor não tem a porta de entrada das sessões";

/// `plugin`: o mod do controle; a `key` só é única dentro dele. O app de antes desta versão não o manda.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PressBody { site: String, #[serde(default)] plugin: Option<String>, key: String }

/// Corpo de `show` e `close`: só o painel.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteBody { site: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputBody { site: String, #[serde(default)] plugin: Option<String>, key: String, kind: String, value: String }

fn refused(headers: &HeaderMap, error: &ModsError) -> Response {
    reply(Some(headers), StatusCode::CONFLICT, json!({"detail": error.detail()}))
}

/// Resposta pronta que encerra a rota (repasse ao Python ou recusa), em caixa: a `Response` é grande para
/// viajar no `Err`.
type Done = Box<Response>;

/// O Rust atende quando o pedido é do dono (ou do convidado liberado para a sessão); quem sobra segue ao
/// Python, que o autentica (nenhum token tem tratamento pelo formato aqui). Sessão sem a interface dos mods
/// no Rust recebe a recusa fixa da rota (`outside`), sem validar o corpo: não há quem mais a atenda.
async fn owned(st: &Arc<AppState>, peer: SocketAddr, path: Result<Path<String>, PathRejection>, req: Request,
    outside: Outside) -> Result<(String, HeaderMap, Body), Done> {
    let started = Instant::now();
    let (fwd, who) = match who(st, peer, &req).await { Ok(value) => value, Err(e) => return Err(Box::new(crate::auth::unavailable(e))) };
    let owner = matches!(who, Some(Caller::Owner));
    let method = req.method().clone();
    let guest = |name: &str| who.as_ref().is_some_and(|c| c.grant().is_some() && c.can(name, &method));
    let name = match path {
        Ok(Path(name)) if owner || guest(&name) => name,
        _ => return Err(Box::new(pass(st, req, &fwd).await)),
    };
    // O Claude com terminal ainda sem ator é adotado aqui, como na escrita.
    if let Err(error) = adopt(st, &name).await {
        return Err(Box::new(reply(Some(req.headers()), StatusCode::SERVICE_UNAVAILABLE,
            json!({"detail": {"code": error.code, "params": {}, "msg": error.message}}))));
    }
    if st.mods.owns(&name) {
        let headers = req.headers().clone();
        return Ok((name, headers, req.into_body()));
    }
    // Terminal no Rust sem a interface dos mods aqui: nada dirige o pane do convidado.
    let (status, error) = if !owner && let Some(runtime) = st.state.runtime.get() && runtime.is_terminal(&name).await {
        (StatusCode::SERVICE_UNAVAILABLE, no_answer())
    } else {
        (outside.status, (outside.error)())
    };
    // A linha `api.servidor` que o Python gravava nesta recusa (D21).
    st.diag.record_line(json!({"evento": "api.servidor", "nivel": if status.is_server_error() { "erro" } else { "aviso" },
        "sessao": name, "codigo": status.as_u16().to_string(), "detalhe": format!("POST /api/sessions/{{name}}/plugin/{}", outside.op),
        "ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), "etapa": "cabecalhos", "causa": error.code}));
    Err(Box::new(reply(Some(req.headers()), status, json!({"detail": error.detail()}))))
}

/// A recusa de cada rota para a sessão sem a interface dos mods no Rust: `press`/`close` pedem o Rust,
/// `show` deixa a aba com o app (que lê o 404 e troca sozinho), `input` não tem por onde digitar.
#[derive(Clone, Copy)]
struct Outside { op: &'static str, status: StatusCode, error: fn() -> ModsError }

const PRESS: Outside = Outside { op: "press", status: StatusCode::SERVICE_UNAVAILABLE, error: no_rust };
const CLOSE: Outside = Outside { op: "close", status: StatusCode::SERVICE_UNAVAILABLE, error: no_rust };
const SHOW: Outside = Outside { op: "show", status: StatusCode::NOT_FOUND, error: tab_in_app_error };
const INPUT: Outside = Outside { op: "input", status: StatusCode::CONFLICT, error: no_typing };

/// A sessão fora do `Mods` que é um Claude com terminal sem ator passa a ser do Rust.
async fn adopt(st: &Arc<AppState>, name: &str) -> Result<(), crate::runtime::protocol::RuntimeError> {
    if st.mods.owns(name) { return Ok(()); }
    match st.state.runtime.get() { Some(runtime) => runtime.adopt_claude_terminal(name).await, None => Ok(()) }
}

async fn body<T: DeserializeOwned>(headers: &HeaderMap, raw: Body) -> Result<T, Done> {
    let bytes = to_bytes(raw, BODY_LIMIT).await.map_err(|_| Box::new(invalid(Some(headers))))?;
    serde_json::from_slice(&bytes).map_err(|_| Box::new(invalid(Some(headers))))
}

/// A guarda da troca de agente: o passe da porta de entrada, que segura a troca até o fim da operação.
/// Porta retida pela troca recusa na hora (`session_transfer_busy`, como o Python); fechada por um
/// congelamento curto, espera reabrir dentro do orçamento, e o fim dele é a recusa do mod sem resposta.
async fn enter(st: &AppState, headers: &HeaderMap, name: &str, deadline: Instant) -> Result<IngressPass, Response> {
    let Some(runtime) = st.state.runtime.get() else {
        return Err(route_failed(st, headers, "rust.mods_failed", name, "erro_mod_guarda_indisponivel", TRANSFER_REASON));
    };
    runtime.ingress().enter(name, deadline.saturating_duration_since(Instant::now())).await.map_err(|_| {
        if Instant::now() >= deadline { refused(headers, &no_answer()) } else { reply(Some(headers), StatusCode::CONFLICT, busy_body()) }
    })
}

async fn run(st: &AppState, headers: &HeaderMap, name: &str, call: ModsCall, deadline: Instant) -> Response {
    let Some(Turn { link, lock }) = st.mods.link(name) else { return refused(headers, &missing()) };
    // Um pedido por vez por sessão, como a trava do `plugin_click.press`: dois aparelhos não se cruzam. A
    // vez que não sai no orçamento é recusa, sem chamar o mod: o app já teria desistido.
    let Ok(_turn) = tokio::time::timeout_at(deadline, lock.lock()).await else { return refused(headers, &no_answer()) };
    // A porta vem depois da vez: entrar antes faria a troca esperar também a fila da sessão.
    let _pass = match enter(st, headers, name, deadline).await { Ok(pass) => pass, Err(refusal) => return refusal };
    if Instant::now() >= deadline {
        return refused(headers, &no_answer());
    }
    let attempt = match &call { ModsCall::Press { site, plugin, key } => Some(st.mods.begin_click(name, site, plugin, key)), _ => None };
    // O que sobra do orçamento limita a chamada e vai junto até a superfície, que não leva ação ao mod sem
    // tempo para a resposta voltar antes dele. Cortada, a resposta que vier depois cai num canal fechado, e
    // o ator não leva à superfície um pedido que ainda estava na caixa dele.
    let result = tokio::time::timeout_at(deadline, link.call(call.clone(), deadline.into_std())).await.unwrap_or_else(|_| Err(no_answer()));
    let (copied, opened) = match &attempt {
        Some(attempt) => st.mods.finish_click(name, attempt, EFFECT_WAIT.min(deadline.saturating_duration_since(Instant::now()))).await,
        None => (None, None),
    };
    match result {
        Ok(value) => {
            let mut answer = json!({"ok": true});
            match call {
                ModsCall::Show { .. } => answer["shown_id"] = value["shown_id"].clone(),
                ModsCall::Input { .. } => answer["value"] = value["value"].clone(),
                ModsCall::Press { .. } | ModsCall::Close { .. } => {}
            }
            if let Some(text) = copied { answer["copied"] = json!(text); }
            if let Some(url) = opened { answer["opened"] = json!(url); }
            reply(Some(headers), StatusCode::OK, answer)
        }
        Err(error) => refused(headers, &error),
    }
}

/// Clique num botão de mod.
pub async fn press(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>, req: Request) -> Response {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let (name, headers, raw) = match owned(&st, peer, path, req, PRESS).await { Ok(parts) => parts, Err(response) => return *response };
    let request: PressBody = match body(&headers, raw).await { Ok(request) => request, Err(response) => return *response };
    if !fits(&request.site, 64) || !plugin_fits(request.plugin.as_deref()) || !fits(&request.key, 256) {
        return invalid(Some(&headers));
    }
    // O app de antes da rota `close` fechava o painel pelo `press` com a `key` reservada.
    if request.plugin.is_none() && request.key == CLOSE_KEY {
        return run(&st, &headers, &name, ModsCall::Close { site: request.site }, deadline).await;
    }
    let Some(plugin) = plugin_or_only(&st, &name, &request.site, request.plugin, &request.key, "Button")
        else { return refused(&headers, &missing()) };
    run(&st, &headers, &name, ModsCall::Press { site: request.site, plugin, key: request.key }, deadline).await
}

fn plugin_fits(plugin: Option<&str>) -> bool { plugin.is_none_or(|plugin| fits(plugin, PLUGIN_MAX)) }

/// O mod do pedido; sem ele (app de antes desta versão), o único mod com a `key` no lugar, como antes.
fn plugin_or_only(st: &AppState, name: &str, site: &str, plugin: Option<String>, key: &str, kind: &str) -> Option<String> {
    plugin.or_else(|| st.mods.plugin_of(name, site, key, &[kind]))
}

/// Fechar o painel `site` (o `✕` do cabeçalho).
pub async fn close(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>, req: Request) -> Response {
    pane_route(&st, peer, path, req, CLOSE, |site| ModsCall::Close { site }).await
}

/// Troca de aba: o painel `site` vai para a frente (`ui_pane_show`).
pub async fn show(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>, req: Request) -> Response {
    pane_route(&st, peer, path, req, SHOW, |site| ModsCall::Show { site }).await
}

/// Rota que só fala de um painel (`{site}`): `close` e `show`.
async fn pane_route(st: &Arc<AppState>, peer: SocketAddr, path: Result<Path<String>, PathRejection>, req: Request,
    outside: Outside, call: fn(String) -> ModsCall) -> Response {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let (name, headers, raw) = match owned(st, peer, path, req, outside).await { Ok(parts) => parts, Err(response) => return *response };
    let request: SiteBody = match body(&headers, raw).await { Ok(request) => request, Err(response) => return *response };
    if !fits(&request.site, 64) {
        return invalid(Some(&headers));
    }
    run(st, &headers, &name, call(request.site), deadline).await
}

/// Digitação num `Input` de mod: `change` a cada mudança, `submit` no Enter.
pub async fn input(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>, req: Request) -> Response {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let (name, headers, raw) = match owned(&st, peer, path, req, INPUT).await { Ok(parts) => parts, Err(response) => return *response };
    // Com terminal não há por onde digitar no campo do mod: a recusa não espera a vez da sessão (um clique
    // em curso a segura por segundos) nem entra na porta da troca de agente.
    if st.mods.is_terminal(&name) {
        return refused(&headers, &no_typing());
    }
    let request: InputBody = match body(&headers, raw).await { Ok(request) => request, Err(response) => return *response };
    if !fits(&request.site, 64) || !plugin_fits(request.plugin.as_deref()) || !fits(&request.key, 256) || !matches!(request.kind.as_str(), "change" | "submit")
        || request.value.chars().count() > VALUE_MAX {
        return invalid(Some(&headers));
    }
    let Some(plugin) = plugin_or_only(&st, &name, &request.site, request.plugin, &request.key, "Input")
        else { return refused(&headers, &missing()) };
    let call = ModsCall::Input { site: request.site, plugin, key: request.key, submit: request.kind == "submit", value: request.value };
    run(&st, &headers, &name, call, deadline).await
}
