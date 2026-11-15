//! Avisos da CLI que o dono mandou não mostrar mais nesta máquina: a rota do dono e a fonte do evento
//! `notice_ignores` da lista. O arquivo e as regras das chaves são do crate `hangar-notice-ignores`.
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use hangar_notice_ignores::Error;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex, OnceLock}};
use tokio::sync::Notify;
use crate::{routes::{AppState, gate}, session_write::{detail_body, json_response}};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change { key: String, ignored: bool }

fn body(keys: &BTreeSet<String>) -> Value { json!({"keys": keys}) }

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

/// Fonte do evento da lista. O arquivo tem um escritor só, a rota, que publica a lista nova aqui: sem vigia de
/// arquivo.
#[derive(Default)]
pub struct Feed {
    home: OnceLock<PathBuf>,
    json: Mutex<Option<Arc<str>>>,
    pub(crate) changed: Notify,
}

impl Feed {
    pub fn bind(&self, home: PathBuf) { let _ = self.home.set(home); }

    /// O JSON do evento; a primeira leitura vem do arquivo. `None`: ainda sem pasta ligada.
    pub(crate) async fn read(&self) -> Option<Result<Arc<str>, &'static str>> {
        if let Some(json) = self.json.lock().unwrap_or_else(|e| e.into_inner()).clone() { return Some(Ok(json)); }
        let home = self.home.get()?.clone();
        match tokio::task::spawn_blocking(move || hangar_notice_ignores::load(&home)).await {
            Ok(Ok(keys)) => Some(Ok(self.store(&keys))),
            _ => Some(Err("notice_ignores_unreadable")),
        }
    }

    fn store(&self, keys: &BTreeSet<String>) -> Arc<str> {
        let json: Arc<str> = body(keys).to_string().into();
        *self.json.lock().unwrap_or_else(|e| e.into_inner()) = Some(json.clone());
        json
    }

    /// Gravação aceita: o produtor da lista publica na hora.
    fn publish(&self, keys: &BTreeSet<String>) {
        self.store(keys);
        self.changed.notify_one();
    }
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !(match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
        return failure(StatusCode::FORBIDDEN, "notice_ignores_owner_only", "só o dono pode ler ou alterar os avisos ignorados");
    }
    let change = if request.method() == Method::PUT {
        let parsed = to_bytes(request.into_body(), 4 * 1024).await.ok()
            .and_then(|raw| serde_json::from_slice::<Change>(&raw).ok()).filter(|change| hangar_notice_ignores::valid_key(&change.key));
        let Some(change) = parsed else {
            return failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "aviso inválido");
        };
        Some(change)
    } else { None };
    let saving = change.is_some();
    let home = state.accounts.env.home.to_path_buf();
    let done = tokio::task::spawn_blocking(move || match change {
        Some(change) => hangar_notice_ignores::set(&home, &change.key, change.ignored),
        None => hangar_notice_ignores::load(&home).map_err(Error::Io),
    }).await;
    match done {
        Ok(Ok(keys)) => {
            if saving { state.list.notice_ignores.publish(&keys); }
            json_response(StatusCode::OK, body(&keys))
        }
        Ok(Err(Error::Full)) => failure(StatusCode::CONFLICT, "notice_ignores_full", "avisos ignorados demais; volte a mostrar algum antes"),
        Ok(Err(Error::InvalidKey)) => failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "aviso inválido"),
        // Arquivo ilegível nunca vira lista vazia: a gravação seguinte o apagaria.
        _ => failure(StatusCode::INTERNAL_SERVER_ERROR, "notice_ignores_unreadable", "não foi possível ler ou guardar os avisos ignorados"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn feed_reads_the_file_once_and_then_what_the_route_published() {
        let home = tempfile::tempdir().unwrap();
        let feed = Feed::default();
        assert!(feed.read().await.is_none(), "sem pasta ligada não há evento");
        feed.bind(home.path().to_path_buf());
        assert_eq!(feed.read().await.unwrap().unwrap().as_ref(), r#"{"keys":[]}"#);
        let keys = hangar_notice_ignores::set(home.path(), "hook:Stop", true).unwrap();
        assert_eq!(feed.read().await.unwrap().unwrap().as_ref(), r#"{"keys":[]}"#, "só a rota publica: sem vigia de arquivo");
        feed.publish(&keys);
        tokio::time::timeout(std::time::Duration::from_secs(1), feed.changed.notified()).await.expect("a gravação acorda o produtor");
        assert_eq!(feed.read().await.unwrap().unwrap().as_ref(), r#"{"keys":["hook:Stop"]}"#);
    }

    #[tokio::test]
    async fn feed_with_an_unreadable_file_is_an_error_not_an_empty_list() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
        std::fs::write(hangar_notice_ignores::path(home.path()), b"{").unwrap();
        let feed = Feed::default();
        feed.bind(home.path().to_path_buf());
        assert_eq!(feed.read().await, Some(Err("notice_ignores_unreadable")));
    }
}
