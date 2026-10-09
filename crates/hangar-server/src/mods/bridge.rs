//! Rotas da ponte do plugin do Hangar para as sessões que o Rust atende. Na sessão sem terminal o plugin
//! entra no `claude -p` pelo `--plugin-dir` que o Python põe no lançamento (S7) e usa só `press-start` e
//! `opened`, no clique do app pela superfície `desktop`. Na sessão com terminal do Rust (fase 3) o plugin
//! roda no terminal e usa todas: a faixa e os painéis (`ui`), os avisos, o press, a cópia, a URL, o foco da
//! reserva por teclado e a rolagem. Sessão que o Rust não atende segue ao Python com o mesmo corpo.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use subtle::ConstantTimeEq;

use super::http::{fits, invalid, reply};
use super::model::PLUGIN_MAX;
use super::state::{SurfaceExtra, TerminalPane, TerminalView};
use crate::routes::{AppState, gate, pass};

const BODY_LIMIT: usize = 16 * 1024;
const URL_MAX: usize = 8192;
/// Prazo da pergunta ao Python sobre o nome antigo: o plugin desiste do `press-start` em 3 s, e a resposta,
/// com o repasse ao Python incluído, tem de chegar antes; senão o clique roda depois de ele desistir.
const NAME_CHECK: std::time::Duration = std::time::Duration::from_secs(1);
/// O `/ui` leva a faixa e os painéis inteiros: o mesmo teto do Python (`MAX_BAND_BYTES`).
const UI_BODY_LIMIT: usize = 300 * 1024;
/// A cópia aceita o que o Python aceita (`CopiedBody.text`, até 65536 caracteres); no JSON um caractere
/// chega a 6 bytes (`\u0001`).
const COPIED_BODY_LIMIT: usize = 400 * 1024;
const COPIED_TEXT_MAX: usize = 65536;
/// O `ToastBody` do Python não tem teto: texto longo é cortado (`TOAST_MAX_CHARS`), não recusado, para o
/// aviso não sumir do app. O teto do corpo é o da faixa, folgado para qualquer aviso de verdade.
const TOAST_BODY_LIMIT: usize = UI_BODY_LIMIT;
const ID_MAX: usize = 64;
const ELEMENT_MAX: usize = 256;
const CAPS_MAX: usize = 16;
/// Prazo da cópia do `/ui` ao Python: o plugin espera a resposta, e a cópia não decide nada.
const PYTHON_COPY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// O que toda chamada da ponte traz: a sessão (o nome com que o processo nasceu) e o token dela.
#[derive(Deserialize)]
struct Envelope<T> { sessao: String, token: String, #[serde(flatten)] body: T }

/// Corpo do `press-start` e do `pressed`. `plugin`: o mod do press, que o plugin do Hangar carregado antes
/// desta versão não manda. O Python, que atende a sessão fora do Rust, ignora o campo.
#[derive(Deserialize)]
struct PressBody { #[serde(rename = "requestId")] request_id: String, element: String, #[serde(default)] plugin: Option<String> }

#[derive(Deserialize)]
struct Opened { attempt: String, url: String }

/// O token da ponte de uma sessão, igual ao `plugin_bridge.mint` do Python: HMAC-SHA256 do token do
/// dono (ou "hangar", sem token) sobre `plugin:<nome>`, em 32 dígitos hexadecimais.
pub fn mint(secret: &str, name: &str) -> String {
    let secret = if secret.is_empty() { "hangar" } else { secret };
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    ring::hmac::sign(&key, format!("plugin:{name}").as_bytes()).as_ref()[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Corpo lido uma vez: sessão que não é do Rust volta ao Python com o mesmo corpo. Devolve o corpo e o
/// nome atual da sessão, que pode não ser o `sessao` do plugin (sessão renomeada sem relançar o processo:
/// o plugin manda o nome com que nasceu, e o token é o desse nome). Corpo que não serve ao tipo, de sessão
/// do Rust, é 422, como o Pydantic do Python.
async fn owned_sized<T: DeserializeOwned>(st: &Arc<AppState>, peer: SocketAddr, req: Request, limit: usize) -> Result<(Envelope<T>, String), Box<Response>> {
    let (fwd, _) = gate(st, peer, &req);
    let (parts, raw) = req.into_parts();
    let Ok(bytes) = to_bytes(raw, limit).await else { return Err(Box::new(StatusCode::PAYLOAD_TOO_LARGE.into_response())) };
    let parsed = serde_json::from_slice::<Envelope<T>>(&bytes);
    let sessao = match &parsed {
        Ok(envelope) => Some(envelope.sessao.clone()),
        Err(_) => serde_json::from_slice::<Value>(&bytes).ok().and_then(|body| body["sessao"].as_str().map(str::to_owned)),
    };
    let name = sessao.as_deref().and_then(|sessao| st.mods.bridge_session(sessao));
    let elsewhere = match (&name, &sessao) {
        (Some(name), Some(sessao)) if name != sessao => named_elsewhere(st, sessao).await,
        _ => false,
    };
    match (parsed, name) {
        (Ok(envelope), Some(name)) if !elsewhere => Ok((envelope, name)),
        (Err(_), Some(_)) if !elsewhere => Err(Box::new(invalid(None))),
        _ => Err(Box::new(pass(st, Request::from_parts(parts, Body::from(bytes)), &fwd).await)),
    }
}

async fn owned<T: DeserializeOwned>(st: &Arc<AppState>, peer: SocketAddr, req: Request) -> Result<(Envelope<T>, String), Box<Response>> {
    owned_sized(st, peer, req, BODY_LIMIT).await
}

/// O nome antigo de uma sessão renomeada pode ser o nome atual de outra, fora do Rust (com terminal, ou
/// criada depois no Python), cujo plugin manda o mesmo `sessao` com um token que vale. Pergunta ao Python
/// pelo status do `info`, sem o cache do `/events` (a resposta de um segundo atrás pode ser de antes do
/// renomear) e sem ler o corpo: 404 é sessão inexistente. Existindo a sessão, ou sem resposta em
/// `NAME_CHECK`, o pedido é dela e vai ao Python.
async fn named_elsewhere(st: &AppState, sessao: &str) -> bool {
    let url = format!("http://{}/internal/sessions/{}/info", st.cfg.upstream, utf8_percent_encode(sessao, NON_ALPHANUMERIC));
    let Ok(request) = axum::http::Request::get(url).header("x-hangar-internal", &st.cfg.internal_secret).body(Body::empty()) else { return true };
    !matches!(tokio::time::timeout(NAME_CHECK, st.http.request(request)).await,
        Ok(Ok(response)) if response.status() == StatusCode::NOT_FOUND)
}

/// Comparação em tempo constante, como o `secrets.compare_digest` do Python.
fn token_ok(st: &AppState, name: &str, token: &str) -> bool {
    mint(&st.cfg.auth_token, name).as_bytes().ct_eq(token.as_bytes()).into()
}

pub async fn press_start(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<PressBody>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    // Os limites do Pydantic do Python (`PressBody`) vêm antes do token, como lá.
    if !fits(&body.request_id, ID_MAX) || !fits(&body.element, ELEMENT_MAX) || !fits_opt(&body.plugin, PLUGIN_MAX) {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return reply(None, StatusCode::FORBIDDEN, json!({"detail": "token do plugin inválido"}));
    }
    let attempt = st.mods.match_click(&name, &body.request_id, body.plugin.as_deref(), &body.element);
    reply(None, StatusCode::OK, json!({"fromApp": attempt.is_some(), "attempt": attempt}))
}

pub async fn opened(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<Opened>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    // Os limites do `OpenedBody`. A URL não tem mínimo no Pydantic: vazia passa daqui e cai no 400 do esquema.
    if !fits(&body.attempt, 64) || body.url.chars().count() > URL_MAX {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return reply(None, StatusCode::FORBIDDEN, json!({"detail": "token do plugin inválido"}));
    }
    let lower = body.url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return reply(None, StatusCode::BAD_REQUEST, json!({"detail": "só http(s)"}));
    }
    if st.mods.opened(&name, &body.attempt, &body.url) {
        reply(None, StatusCode::OK, json!({"ok": true}))
    } else {
        // 409: o plugin segue com o `next` e o mod abre a URL na máquina do servidor, como no Python.
        reply(None, StatusCode::CONFLICT, json!({"detail": "clique do app já respondido"}))
    }
}

#[derive(Deserialize)]
struct UiBody {
    #[serde(default)] above: Value,
    /// O de hoje: `bodyColumns` mais as 5 colunas do `[-]`, que o Python lê para cortar a prévia.
    #[serde(default)] columns: Option<u64>,
    /// O que o mod recebeu e o evento publica.
    #[serde(default, rename = "bodyColumns")] body_columns: Option<u64>,
    #[serde(default)] panes: Vec<TerminalPane>,
    #[serde(default)] shown: Option<String>,
    /// O que o plugin desta sessão atende (`btw`): o app libera só o que funciona aqui.
    #[serde(default)] caps: Vec<String>,
}
#[derive(Deserialize)]
struct ToastBody { text: String, #[serde(rename = "timeoutMs", default)] timeout_ms: Option<f64>, #[serde(default)] plugin: Option<String> }
#[derive(Deserialize)]
struct CopiedBody { attempt: String, text: String }
#[derive(Deserialize)]
struct FocusTargetBody { #[serde(rename = "requestId")] request_id: String, #[serde(default)] plugin: Option<String>, #[serde(default)] element: Option<String> }
#[derive(Deserialize)]
struct FocusedBody { attempt: String, #[serde(rename = "requestId")] request_id: String, #[serde(default)] plugin: Option<String>,
    #[serde(default)] element: Option<String>, denied: bool }
#[derive(Deserialize)]
struct ScrollBody { #[serde(rename = "requestId")] request_id: String, offset: u64,
    #[serde(rename = "bodyRows")] _body_rows: u64, #[serde(rename = "contentRows")] _content_rows: u64 }

fn ok() -> Response { reply(None, StatusCode::OK, json!({"ok": true})) }
fn forbidden() -> Response { reply(None, StatusCode::FORBIDDEN, json!({"detail": "token do plugin inválido"})) }
/// Campo opcional: ausente, ou dentro do limite.
fn fits_opt(text: &Option<String>, max: usize) -> bool { text.as_deref().is_none_or(|text| fits(text, max)) }

/// A cópia do `/ui` para o Python, no formato de hoje: a prévia (`preview.py`, `transcript_columns` e
/// `band_anchor`) e o campo `faixa` do `/pull` continuam lendo o cache dele, sem código novo no Python. O
/// `plugin_ui` que ele publica com ela é descartado no hub (a sessão é do Rust). Falha só vai ao log, sem
/// URL, token nem conteúdo: a prévia fica sem o corte até o próximo `/ui`, e o app não perde nada.
async fn copy_to_python(st: &AppState, body: &Value) {
    let url = format!("http://{}/api/plugin/ui", st.cfg.upstream);
    let Ok(request) = axum::http::Request::post(url).header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())) else { return };
    match tokio::time::timeout(PYTHON_COPY_TIMEOUT, st.http.request(request)).await {
        Ok(Ok(response)) if response.status().is_success() => {}
        Ok(Ok(response)) => tracing::debug!(status = response.status().as_u16(), "cópia da faixa ao Python recusada"),
        _ => tracing::debug!("cópia da faixa ao Python sem resposta"),
    }
}

/// A faixa e os painéis da sessão com terminal, como o plugin do Hangar os viu; dispara a leitura do
/// painel na frente pela tela (a fonte, risco "shown_id na troca de aba sem redesenho").
pub async fn ui(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned_sized::<UiBody>(&st, peer, req, UI_BODY_LIMIT).await { Ok(found) => found, Err(response) => return *response };
    let Envelope { sessao, token, body } = envelope;
    // Os limites do `BandPane` do Python vêm antes do token, como lá.
    if !body.panes.iter().all(|pane| fits(&pane.id, ID_MAX)) || !fits_opt(&body.shown, ID_MAX)
        || body.caps.len() > CAPS_MAX || !body.caps.iter().all(|cap| fits(cap, ID_MAX)) {
        return invalid(None);
    }
    if !token_ok(&st, &sessao, &token) {
        return forbidden();
    }
    // Sessão do Rust sem terminal: a faixa dela vem da superfície; do plugin entram só o que ele atende e o
    // estado estruturado dos painéis, juntados à vista da superfície.
    if !st.mods.is_terminal(&name) {
        let data = body.panes.into_iter().filter_map(|pane| pane.data.map(|data| (pane.id, data))).collect();
        st.mods.surface_extra(&name, SurfaceExtra { caps: body.caps, data });
        return ok();
    }
    // Antes de responder e em ordem: dois `/ui` seguidos não chegam trocados ao cache do Python. Com o nome
    // atual e o token dele: numa sessão renomeada, o cache do Python é pelo nome de agora.
    let copy = json!({"sessao": name, "token": mint(&st.cfg.auth_token, &name), "above": body.above, "columns": body.columns, "panes": body.panes});
    copy_to_python(&st, &copy).await;
    st.mods.terminal_ui(&name, TerminalView { above: body.above, columns: body.body_columns, panes: body.panes, shown: body.shown, caps: body.caps });
    st.mods.schedule_shown(&name);
    ok()
}

pub async fn toast(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned_sized::<ToastBody>(&st, peer, req, TOAST_BODY_LIMIT).await { Ok(found) => found, Err(response) => return *response };
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    let body = &envelope.body;
    if let Some(life) = st.mods.life(&name) {
        let ms = body.timeout_ms.filter(|ms| ms.is_finite() && *ms > 0.0).map_or(0, |ms| ms as u64);
        st.mods.toast(&name, life, body.plugin.as_deref().unwrap_or(""), &body.text, ms);
    }
    ok()
}

pub async fn pressed(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<PressBody>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    if !fits(&body.request_id, ID_MAX) || !fits(&body.element, ELEMENT_MAX) || !fits_opt(&body.plugin, PLUGIN_MAX) {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    st.mods.pressed(&name, &body.request_id, body.plugin.as_deref(), &body.element);
    ok()
}

pub async fn copied(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned_sized::<CopiedBody>(&st, peer, req, COPIED_BODY_LIMIT).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    // O `CopiedBody` do Python: `attempt` de 1 a 64, texto (vazio vale) até 65536 caracteres.
    if !fits(&body.attempt, ID_MAX) || body.text.chars().count() > COPIED_TEXT_MAX {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    if st.mods.click_copied(&name, &body.attempt, &body.text) {
        ok()
    } else {
        // 409: o plugin segue com o `next` e a cópia acontece no terminal.
        reply(None, StatusCode::CONFLICT, json!({"detail": "clique do app já respondido"}))
    }
}

pub async fn focus_target(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<FocusTargetBody>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    if !fits(&body.request_id, ID_MAX) || !fits_opt(&body.plugin, ID_MAX) || !fits_opt(&body.element, ELEMENT_MAX) {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    let target = st.mods.focus_target(&name, &body.request_id, body.plugin.as_deref(), body.element.as_deref());
    reply(None, StatusCode::OK, serde_json::to_value(target).unwrap_or(Value::Null))
}

pub async fn focused(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<FocusedBody>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    if !fits(&body.attempt, ID_MAX) || !fits(&body.request_id, ID_MAX) || !fits_opt(&body.plugin, PLUGIN_MAX) || !fits_opt(&body.element, ELEMENT_MAX) {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    if st.mods.focused(&name, &body.attempt, &body.request_id, body.plugin.as_deref(), body.element.as_deref(), body.denied) {
        ok()
    } else {
        reply(None, StatusCode::CONFLICT, json!({"detail": "alvo do foco já desarmado"}))
    }
}

pub async fn scroll(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (envelope, name) = match owned::<ScrollBody>(&st, peer, req).await { Ok(found) => found, Err(response) => return *response };
    let body = &envelope.body;
    if !fits(&body.request_id, ID_MAX) {
        return invalid(None);
    }
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    st.mods.scrolled(&name, &body.request_id, i64::try_from(body.offset).unwrap_or(i64::MAX));
    ok()
}
