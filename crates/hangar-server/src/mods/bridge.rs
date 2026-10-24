//! Rotas da ponte do plugin do Hangar para as sessões que o Rust atende. Na sessão sem terminal o plugin
//! entra no `claude -p` pelo `--plugin-dir` que o Python põe no lançamento (S7) e usa só `press-start` e
//! `opened`, no clique do app pela superfície `desktop`. Na sessão com terminal do Rust (fase 3) o plugin
//! roda no terminal e usa todas: a faixa e os painéis (`ui`), os avisos, o press, a cópia, a URL, o foco da
//! reserva por teclado e a rolagem. A faixa e os avisos da sessão Claude que o Rust não atende (aberta à
//! mão) também ficam aqui (`Mods::loose_ui`); as demais rotas dela seguem ao Python com o mesmo corpo.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use super::http::{fits, invalid, reply};
use super::model::PLUGIN_MAX;
use super::state::{SurfaceExtra, TerminalPane, TerminalView};
use crate::routes::{AppState, gate, pass};

const BODY_LIMIT: usize = 16 * 1024;
const URL_MAX: usize = 8192;
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

/// O que toda chamada da ponte traz: a sessão (o nome com que o processo nasceu) e o token dela.
#[derive(Deserialize)]
struct Envelope<T> { sessao: String, token: String, #[serde(flatten)] body: T }

/// Corpo do `press-start` e do `pressed`. `plugin`: o mod do press, que o plugin do Hangar carregado antes
/// desta versão não manda. O Python, que atende a sessão fora do Rust, ignora o campo.
#[derive(Deserialize)]
struct PressBody { #[serde(rename = "requestId")] request_id: String, element: String, #[serde(default)] plugin: Option<String> }

#[derive(Deserialize)]
struct Opened { attempt: String, url: String }

/// O token da ponte só do nome, igual ao `plugin_bridge.mint(nome)` do Python: HMAC-SHA256 do token do
/// dono (ou "hangar", sem token) sobre `plugin:<nome>`, em 32 dígitos hexadecimais. É o dos processos
/// lançados antes da chave.
pub fn mint(secret: &str, name: &str) -> String {
    hmac_hex(secret, &format!("plugin:{name}"))
}

/// O token da ponte de um processo, igual ao `plugin_bridge.mint(nome, chave)` do Python:
/// `<chave>.<HMAC de plugin:<nome>:<chave>>`. A chave é a da sessão no Rust (`target.key`, ou a do
/// ambiente do processo com terminal): a ponte acha a sessão por ela, e o nome entra só na conferência.
pub fn mint_keyed(secret: &str, name: &str, key: &str) -> String {
    format!("{key}.{}", hmac_hex(secret, &format!("plugin:{name}:{key}")))
}

fn hmac_hex(secret: &str, message: &str) -> String {
    let secret = if secret.is_empty() { "hangar" } else { secret };
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    ring::hmac::sign(&key, message.as_bytes()).as_ref()[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Corpo lido uma vez: sessão que não é do Rust volta ao Python com o mesmo corpo. Devolve o corpo e o
/// nome atual da sessão, que pode não ser o `sessao` do plugin (sessão renomeada sem relançar o processo:
/// o plugin manda o nome com que nasceu). Corpo que não serve ao tipo, de sessão do Rust, é 422, como o
/// Pydantic do Python.
async fn owned_sized<T: DeserializeOwned>(st: &Arc<AppState>, peer: SocketAddr, req: Request, limit: usize) -> Result<(Envelope<T>, String), Box<Response>> {
    let (fwd, _) = gate(st, peer, &req);
    let (parts, raw) = req.into_parts();
    let Ok(bytes) = to_bytes(raw, limit).await else { return Err(Box::new(StatusCode::PAYLOAD_TOO_LARGE.into_response())) };
    let parsed = serde_json::from_slice::<Envelope<T>>(&bytes);
    let found = match &parsed {
        Ok(envelope) => st.mods.bridge_session(&envelope.sessao, &envelope.token),
        // Sem token, a sessão é a do nome: o corpo dela é recusado aqui (422), como antes.
        Err(_) => serde_json::from_slice::<Value>(&bytes).ok().and_then(|body| body["sessao"].as_str()
            .and_then(|sessao| st.mods.bridge_session(sessao, body["token"].as_str().unwrap_or_default()))),
    };
    if found.is_none() && matches!(&parsed, Ok(envelope) if envelope.token.contains('.')) {
        // Normal para sessão do Python; numa do Rust é a chave que faltou no vínculo.
        tracing::debug!("ponte do plugin: chave sem sessão no Rust; segue ao Python");
    }
    match (parsed, found) {
        (Ok(envelope), Some(name)) => Ok((envelope, name)),
        (Err(_), Some(_)) => Err(Box::new(invalid(None))),
        _ => Err(Box::new(pass(st, Request::from_parts(parts, Body::from(bytes)), &fwd).await)),
    }
}

/// Corpo do `/ui` e do `/toast`, que não seguem ao Python: com a sessão do Rust (nome atual), ou `None`
/// para a sessão sem entrada aqui, guardada pelo `sessao` do plugin depois de conferido o token.
async fn session_or_loose<T: DeserializeOwned>(st: &Arc<AppState>, req: Request, limit: usize) -> Result<(Envelope<T>, Option<String>), Box<Response>> {
    let Ok(bytes) = to_bytes(req.into_body(), limit).await else { return Err(Box::new(StatusCode::PAYLOAD_TOO_LARGE.into_response())) };
    let Ok(envelope) = serde_json::from_slice::<Envelope<T>>(&bytes) else { return Err(Box::new(invalid(None))) };
    let name = st.mods.bridge_session(&envelope.sessao, &envelope.token);
    Ok((envelope, name))
}

/// O prazo do aviso como o Python o tratava: ausente ou zero é o padrão (`0` aqui), o resto vai a 1 s até
/// 5 min, sem fração.
fn toast_ms(ms: Option<f64>) -> u64 {
    match ms {
        Some(ms) if ms.is_finite() && ms != 0.0 => ms.clamp(1000.0, crate::side::TOAST_MAX_MS) as u64,
        _ => 0,
    }
}

async fn owned<T: DeserializeOwned>(st: &Arc<AppState>, peer: SocketAddr, req: Request) -> Result<(Envelope<T>, String), Box<Response>> {
    owned_sized(st, peer, req, BODY_LIMIT).await
}

/// Comparação em tempo constante, como o `secrets.compare_digest` do Python. `name` é o `sessao` do
/// plugin: o nome com que o token foi cunhado.
fn token_ok(st: &AppState, name: &str, token: &str) -> bool {
    session_token_ok(&st.cfg.auth_token, name, token)
}

pub fn session_token_ok(secret: &str, name: &str, token: &str) -> bool {
    let expected = match token.split_once('.') {
        Some((key, _)) => mint_keyed(secret, name, key),
        None => mint(secret, name),
    };
    expected.as_bytes().ct_eq(token.as_bytes()).into()
}

pub fn machine_key(secret: &str) -> String { hmac_hex(secret, "plugin:machine") }

pub fn machine_key_ok(secret: &str, key: &str) -> bool {
    machine_key(secret).as_bytes().ct_eq(key.as_bytes()).into()
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
    /// `bodyColumns` mais as 5 colunas do `[-]`: onde a prévia corta as linhas do pane (`crop_columns`).
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

/// A faixa e os painéis da sessão com terminal, como o plugin do Hangar os viu; dispara a leitura do
/// painel na frente pela tela (a fonte, risco "shown_id na troca de aba sem redesenho").
pub async fn ui(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let (envelope, name) = match session_or_loose::<UiBody>(&st, req, UI_BODY_LIMIT).await { Ok(found) => found, Err(response) => return *response };
    let Envelope { sessao, token, body } = envelope;
    // Os limites do `BandBody` do Python vêm antes do token, como lá: `above` é objeto ou nulo.
    if !(body.above.is_object() || body.above.is_null()) || !body.panes.iter().all(|pane| fits(&pane.id, ID_MAX))
        || !fits_opt(&body.shown, ID_MAX) || body.caps.len() > CAPS_MAX || !body.caps.iter().all(|cap| fits(cap, ID_MAX)) {
        return invalid(None);
    }
    if !token_ok(&st, &sessao, &token) {
        return forbidden();
    }
    let Some(name) = name else {
        st.mods.loose_ui(&sessao, body.above, body.panes);
        return ok();
    };
    // Sessão do Rust sem terminal: a faixa dela vem da superfície; do plugin entram só o que ele atende e o
    // estado estruturado dos painéis, juntados à vista da superfície.
    if !st.mods.is_terminal(&name) {
        let data = body.panes.into_iter().filter_map(|pane| pane.data.map(|data| (pane.id, data))).collect();
        st.mods.surface_extra(&name, SurfaceExtra { caps: body.caps, data });
        return ok();
    }
    st.mods.terminal_ui(&name, TerminalView { above: body.above, columns: body.body_columns, panes: body.panes, shown: body.shown, caps: body.caps, crop_columns: body.columns });
    st.mods.schedule_shown(&name);
    ok()
}

pub async fn toast(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let (envelope, name) = match session_or_loose::<ToastBody>(&st, req, TOAST_BODY_LIMIT).await { Ok(found) => found, Err(response) => return *response };
    if !token_ok(&st, &envelope.sessao, &envelope.token) {
        return forbidden();
    }
    let body = &envelope.body;
    let (plugin, ms) = (body.plugin.as_deref().unwrap_or(""), toast_ms(body.timeout_ms));
    match name {
        Some(name) => if let Some(life) = st.mods.life(&name) { st.mods.toast(&name, life, plugin, &body.text, ms) },
        None => st.mods.loose_toast(&envelope.sessao, plugin, &body.text, ms),
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Prazo e texto do plugin, quaisquer: o prazo fica no do Python (padrão ou 1 s a 5 min) e o aviso da
        /// sessão sem entrada nunca derruba o servidor.
        #[test]
        fn any_toast_from_the_plugin_is_kept_without_panicking(ms in proptest::option::of(any::<f64>()), text in "(?s).{0,300}", plugin in ".{0,80}") {
            let got = toast_ms(ms);
            prop_assert!(got == 0 || (1000..=300_000).contains(&got));
            let mods = crate::mods::state::Mods::default();
            mods.loose_toast("solta", &plugin, &text, got);
            mods.loose_ui("solta", Value::Null, Vec::new());
        }
    }
}
