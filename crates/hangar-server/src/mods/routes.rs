//! Rotas dos apps para a interface dos mods de uma sessão sem terminal que o Rust atende como
//! superfície remota: clique (`press`), fechar painel (`close`), troca de aba (`show`) e digitação
//! (`input`). Sessão que o Rust não atende assim, ou pedido de convidado, segue para o Python, que é o
//! dono, como no `/events`; a digitação, que o Python não tem, é recusada aqui.
//!
//! A sessão com terminal que o Rust atende (fase 3) usa as mesmas rotas, com o clique pela tela. O pedido
//! que não é do dono segue ao Python como nas outras: ele autentica o convidado (inclusive o convite da porta
//! 8766, que nunca passa por aqui) e devolve o pedido pela ponte privada (`bridge`). A digitação é
//! recusada logo na entrada, sem esperar a vez da sessão nem consultar a guarda da troca de agente.
//!
//! Antes de cada operação a rota pergunta ao Python se a troca de agente está em curso
//! (`GET /internal/sessions/{name}/transfer`): a coordenação da troca mora lá, e o 409 volta ao app como
//! o Python o deu. Limite aceito (ruling A12): a rota do Python segura o ingresso (`session_ingress`)
//! até o fim, e esta pergunta uma vez, já com a vez da sessão; uma troca que comece entre a resposta e o
//! `ui_*` não é vista. Cada `change` do `input` também paga essa ida ao Python.
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
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::http::{fits, invalid, reply};
use super::model::*;
use super::state::Turn;
use crate::routes::{AppState, gate, pass, route_failed};

const BODY_LIMIT: usize = 64 * 1024;
/// Orçamento do pedido inteiro, desde a entrada: abaixo dos 8 s em que o app desiste, com folga para a
/// volta da resposta.
pub(crate) const REQUEST_BUDGET: Duration = Duration::from_millis(7500);
/// Prazo da guarda da troca de agente. Curto: é uma consulta local ao Python, e o silêncio dele não pode
/// comer o tempo do mod.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);
/// O `onPress` do mod costuma abrir ou copiar sem `await`: o efeito pode chegar logo depois do press
/// (mesma espera do `plugin_click.EFFECT_S`).
const EFFECT_WAIT: Duration = Duration::from_millis(300);
const VALUE_MAX: usize = 16384;
const TRANSFER_REASON: &str = "o backend não confirmou que a sessão está livre da troca de agente";

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

/// O Rust atende quando o pedido é do dono e a sessão é dele; senão, o Python, que autentica e recusa o
/// convidado (nenhum token tem tratamento pelo formato aqui). `outside`: a recusa do dono numa sessão sem
/// superfície no Rust, para a rota que o Python não tem (`input`).
async fn owned(st: &Arc<AppState>, peer: SocketAddr, path: Result<Path<String>, PathRejection>, req: Request,
    outside: Option<fn() -> ModsError>) -> Result<(String, HeaderMap, Body), Done> {
    let (fwd, owner) = gate(st, peer, &req);
    let bridged = req.extensions().get::<Bridged>().is_some();
    let owner = owner || bridged;
    match (path, outside) {
        (Ok(Path(name)), _) if owner && st.mods.owns(&name) => {
            let headers = req.headers().clone();
            Ok((name, headers, req.into_body()))
        }
        (Ok(Path(_)), Some(refusal)) if owner => Err(Box::new(refused(req.headers(), &refusal()))),
        // Da ponte nunca volta ao Python: ele trata a sessão que não é do Rust.
        _ if bridged => Err(Box::new(StatusCode::NOT_FOUND.into_response())),
        _ => Err(Box::new(pass(st, req, &fwd).await)),
    }
}

/// Pedido que o Python devolveu pela ponte privada depois de autenticar quem não é o dono (convidado,
/// Connect). Extensão do pedido: nenhum cliente a põe pela rede.
#[derive(Clone, Copy)]
struct Bridged;

/// `POST /__hangar_server/mods/{name}/{op}` na porta privada: a operação de mod de quem o Python já
/// autenticou, com o mesmo efeito da do dono. Cada recusa tem status próprio, porque só o 404 deixa o
/// Python clicar ele mesmo: 403 segredo ou origem errados, 400 operação desconhecida, 404 sessão que o
/// Rust não atende.
pub async fn bridge(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((name, op)): Path<(String, String)>, mut req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !matches!(op.as_str(), "press" | "close" | "show" | "input") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !st.mods.owns(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    req.extensions_mut().insert(Bridged);
    let path = Ok(Path(name));
    match op.as_str() {
        "press" => press(State(st), ConnectInfo(peer), path, req).await,
        "close" => close(State(st), ConnectInfo(peer), path, req).await,
        "show" => show(State(st), ConnectInfo(peer), path, req).await,
        _ => input(State(st), ConnectInfo(peer), path, req).await,
    }
}

async fn body<T: DeserializeOwned>(headers: &HeaderMap, raw: Body) -> Result<T, Done> {
    let bytes = to_bytes(raw, BODY_LIMIT).await.map_err(|_| Box::new(invalid(Some(headers))))?;
    serde_json::from_slice(&bytes).map_err(|_| Box::new(invalid(Some(headers))))
}

/// A guarda da troca de agente das rotas do Python (`_transfer_check`), perguntada a ele, que coordena
/// a troca. `None`: livre. 409: a recusa dele, com o `detail` como veio. Sem resposta no prazo dela, ou
/// com outro status, recusa com código e motivo, sem repassar (dono único). Se o que a cortou foi o fim
/// do orçamento do pedido (a vez da sessão demorou), a recusa é a do mod sem resposta.
async fn transfer(st: &AppState, headers: &HeaderMap, name: &str, deadline: Instant) -> Option<Response> {
    let until = deadline.min(Instant::now() + TRANSFER_TIMEOUT);
    let failed = || {
        if Instant::now() >= deadline {
            return Some(refused(headers, &no_answer()));
        }
        Some(route_failed(st, headers, "rust.mods_failed", name, "erro_mod_guarda_indisponivel", TRANSFER_REASON))
    };
    let url = format!("http://{}/internal/sessions/{}/transfer", st.cfg.upstream, utf8_percent_encode(name, NON_ALPHANUMERIC));
    let Ok(request) = axum::http::Request::get(url).header("x-hangar-internal", &st.cfg.internal_secret).body(Body::empty()) else { return failed() };
    let Ok(Ok(response)) = tokio::time::timeout_at(until, st.http.request(request)).await else { return failed() };
    if response.status().is_success() {
        return None;
    }
    if response.status() != StatusCode::CONFLICT {
        return failed();
    }
    let Ok(Ok(collected)) = tokio::time::timeout_at(until, response.into_body().collect()).await else { return failed() };
    // Um 409 sem o `detail` do Python (corpo vazio, outro formato) não diz o que recusar: é falha da guarda.
    let Ok(answer) = serde_json::from_slice::<Value>(&collected.to_bytes()) else { return failed() };
    if !answer["detail"].is_object() {
        return failed();
    }
    Some(reply(Some(headers), StatusCode::CONFLICT, json!({"detail": answer["detail"]})))
}

async fn run(st: &AppState, headers: &HeaderMap, name: &str, call: ModsCall, deadline: Instant) -> Response {
    let Some(Turn { link, lock }) = st.mods.link(name) else { return refused(headers, &missing()) };
    // Um pedido por vez por sessão, como a trava do `plugin_click.press`: dois aparelhos não se cruzam. A
    // vez que não sai no orçamento é recusa, sem chamar o mod: o app já teria desistido.
    let Ok(_turn) = tokio::time::timeout_at(deadline, lock.lock()).await else { return refused(headers, &no_answer()) };
    // A guarda vem depois da vez: perguntada antes, a resposta envelheceria enquanto o pedido espera a
    // trava. Vale para toda operação, inclusive cada `change` do `input` (A12).
    if let Some(busy) = transfer(st, headers, name, deadline).await {
        return busy;
    }
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
    let (name, headers, raw) = match owned(&st, peer, path, req, None).await { Ok(parts) => parts, Err(response) => return *response };
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
    pane_route(&st, peer, path, req, |site| ModsCall::Close { site }).await
}

/// Troca de aba: o painel `site` vai para a frente (`ui_pane_show`).
pub async fn show(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>, req: Request) -> Response {
    pane_route(&st, peer, path, req, |site| ModsCall::Show { site }).await
}

/// Rota que só fala de um painel (`{site}`): `close` e `show`.
async fn pane_route(st: &Arc<AppState>, peer: SocketAddr, path: Result<Path<String>, PathRejection>, req: Request,
    call: fn(String) -> ModsCall) -> Response {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let (name, headers, raw) = match owned(st, peer, path, req, None).await { Ok(parts) => parts, Err(response) => return *response };
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
    let (name, headers, raw) = match owned(&st, peer, path, req, Some(no_typing)).await { Ok(parts) => parts, Err(response) => return *response };
    // Com terminal não há por onde digitar no campo do mod: a recusa não espera a vez da sessão (um clique
    // em curso a segura por segundos) nem pergunta ao Python pela troca de agente.
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
