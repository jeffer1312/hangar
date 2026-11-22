//! O padrão da abertura pertence ao servidor, para desktop e celular escolherem igual.
use axum::{body::to_bytes, extract::{ConnectInfo, Query, Request, State}, http::{Method, StatusCode}, response::Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, net::SocketAddr, path::Path, sync::Arc};
use crate::{routes::{AppState, gate}, session_open::PermissionMode, session_write::{detail_body, json_response}};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults { pub model: String, pub effort: String, pub permission: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    provider: String,
    #[serde(default)] engine: String,
    #[serde(default)] engine_account: String,
}

impl Scope {
    fn key(&self) -> Option<String> {
        if !["claude", "codex", "pi", "omp", "kimi"].contains(&self.provider.as_str())
            || (self.provider != "claude" && (!self.engine.is_empty() || !self.engine_account.is_empty()))
            || (self.engine.is_empty() && !self.engine_account.is_empty()) { return None; }
        Some(serde_json::to_string(&(&self.provider, &self.engine, &self.engine_account)).expect("strings"))
    }

    fn accepts(&self, value: &Defaults) -> bool {
        (value.model.is_empty() || super::session_open::resolve::valid_model(&value.model))
            && ["", "off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].contains(&value.effort.as_str())
            && (value.permission.is_empty() || match self.provider.as_str() {
                "claude" => PermissionMode::claude(&value.permission).is_some(),
                "codex" => PermissionMode::codex(&value.permission).is_some(),
                _ => false,
            })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    defaults: Option<Defaults>,
    // Importação do arquivo antigo do desktop não pisa numa escolha já publicada ou apagada.
    #[serde(default)] if_absent: bool,
}

#[derive(Serialize)]
struct Selection { configured: bool, defaults: Option<Defaults> }

fn path(home: &Path) -> std::path::PathBuf { home.join(".hangar").join("session-defaults.json") }

/// A chave gravada para provedor, motor e conta do motor; `None` = combinação que não existe.
pub(crate) fn scope_key(provider: &str, engine: &str, engine_account: &str) -> Option<String> {
    Scope { provider: provider.into(), engine: engine.into(), engine_account: engine_account.into() }.key()
}

/// O padrão marcado para o escopo, que o lançador aplica ao que o pedido não disse.
pub(crate) fn saved(home: &Path, provider: &str, engine: &str, engine_account: &str) -> io::Result<Option<Defaults>> {
    let Some(key) = scope_key(provider, engine, engine_account) else { return Ok(None) };
    Ok(read(&path(home))?.remove(&key).flatten())
}

/// O arquivo lido; `None` = nunca gravado.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read(path: &Path) -> io::Result<BTreeMap<String, Option<Defaults>>> { Ok(read_json(path)?.unwrap_or_default()) }

/// A trava de quem grava, para ler e reescrever o arquivo sem outra gravação no meio.
fn write_lock(path: &Path, saving: bool) -> io::Result<Option<std::fs::File>> {
    if !saving { return Ok(None); }
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    hangar_core::fs::lock_file(&path.with_extension("lock")).map(Some)
}

fn selection(path: &Path, key: &str, save: Option<Save>) -> io::Result<Selection> {
    let _lock = write_lock(path, save.is_some())?;
    let mut entries = read(path)?;
    if let Some(save) = save.filter(|save| !save.if_absent || !entries.contains_key(key)) {
        entries.insert(key.to_owned(), save.defaults);
        crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&entries)?)?;
    }
    Ok(Selection { configured: entries.contains_key(key), defaults: entries.remove(key).flatten() })
}

/// As opções da tela Beta que a abertura aplica ao que o pedido não disse. Arquivo próprio: o
/// `session-defaults.json` é lido estrito pela versão anterior, e uma chave nova nele a faria recusar
/// toda criação depois de voltar a atualização.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOptions { pub conversation_beta: bool, pub code_mode: bool }

fn options_path(home: &Path) -> std::path::PathBuf { home.join(".hangar").join("session-options.json") }

/// As opções gravadas; `None` = nunca gravadas.
pub(crate) fn options(home: &Path) -> io::Result<Option<SessionOptions>> { read_json(&options_path(home)) }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveOptions {
    options: SessionOptions,
    // A cópia do `appearance.json` antigo do desktop não pisa numa escolha já gravada.
    #[serde(default)] if_absent: bool,
}

#[derive(Serialize)]
struct OptionsSelection { configured: bool, options: SessionOptions }

fn options_selection(path: &Path, save: Option<SaveOptions>) -> io::Result<OptionsSelection> {
    let _lock = write_lock(path, save.is_some())?;
    let mut current = read_json(path)?;
    if let Some(save) = save.filter(|save| !save.if_absent || current.is_none()) {
        crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&save.options)?)?;
        current = Some(save.options);
    }
    Ok(OptionsSelection { configured: current.is_some(), options: current.unwrap_or_default() })
}

/// `GET`/`PUT /api/session-options`: só o dono.
pub async fn handle_options(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !(match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
        return crate::session_write::not_owner(&state, peer, request).await;
    }
    let save = if request.method() == Method::PUT {
        match to_bytes(request.into_body(), 16 * 1024).await.ok().and_then(|raw| serde_json::from_slice::<SaveOptions>(&raw).ok()) {
            Some(save) => Some(save),
            None => return failure(StatusCode::BAD_REQUEST, "session_options_invalid", "opções da abertura inválidas"),
        }
    } else { None };
    let path = options_path(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || options_selection(&path, save)).await {
        Ok(Ok(selection)) => json_response(StatusCode::OK, json!(selection)),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "session_options_unavailable", "não foi possível ler ou guardar as opções da abertura"),
    }
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !(match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
        // A recusa uniforme: convidado leva o 403 sem perder o login, o resto o 401 contado.
        return crate::session_write::not_owner(&state, peer, request).await;
    }
    serve(&state, request).await
}

pub(crate) async fn serve(state: &Arc<AppState>, request: Request) -> Response {
    let Ok(Query(scope)) = Query::<Scope>::try_from_uri(request.uri()) else {
        return failure(StatusCode::BAD_REQUEST, "session_defaults_invalid", "provedor, motor ou conta do motor inválidos");
    };
    let Some(key) = scope.key() else {
        return failure(StatusCode::BAD_REQUEST, "session_defaults_invalid", "provedor, motor ou conta do motor inválidos");
    };
    let save = if request.method() == Method::PUT {
        let body = to_bytes(request.into_body(), 16 * 1024).await.ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        let parsed = body.filter(|body| body.get("defaults").is_some())
            .and_then(|body| serde_json::from_value::<Save>(body).ok());
        let Some(save) = parsed.filter(|save| save.defaults.as_ref().is_none_or(|value| scope.accepts(value))) else {
            return failure(StatusCode::BAD_REQUEST, "session_defaults_invalid", "modelo, esforço ou permissão inválidos");
        };
        Some(save)
    } else { None };
    let path = path(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || selection(&path, &key, save)).await {
        Ok(Ok(selection)) => json_response(StatusCode::OK, json!(selection)),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "session_defaults_unavailable", "não foi possível ler ou guardar o padrão da abertura"),
    }
}
