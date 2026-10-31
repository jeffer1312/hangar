//! O que a pessoa já viu do tour do nativo, guardado no servidor para outro computador ligado a
//! ele não mostrar de novo. A junção nunca desfaz o que algum aparelho já viu.
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeSet, io, net::SocketAddr, path::{Path, PathBuf}, sync::Arc};
use crate::{routes::{AppState, gate}, session_write::{detail_body, json_response}};

// A ordem das variantes é a da junção: `feito` vence `pulado` (e `None` perde para os dois).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
enum Tour { #[serde(rename = "pulado")] Skipped, #[serde(rename = "feito")] Done }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Switch { #[serde(rename = "valor")] on: bool, #[serde(rename = "em")] at: String }

impl Switch {
    fn instant(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> { chrono::DateTime::parse_from_rfc3339(&self.at).ok() }
}

/// Mesmo formato no arquivo e no corpo do `PUT`; no corpo, campo ausente não acrescenta nada. Os nomes no JSON são os
/// já gravados e os que o nativo manda: mudam só com os dois lados.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    #[serde(default)] tour: Option<Tour>,
    #[serde(default, rename = "dicas")] tips: BTreeSet<String>,
    #[serde(default, rename = "dicas_desligadas")] tips_switch: Option<Switch>,
}

impl Progress {
    fn valid(&self) -> bool { self.tips_switch.as_ref().is_none_or(|value| value.instant().is_some()) }

    fn merge(&mut self, other: Progress) {
        self.tour = self.tour.max(other.tour);
        self.tips.extend(other.tips);
        if let Some(theirs) = other.tips_switch {
            let newer = match &self.tips_switch {
                Some(ours) => theirs.instant() > ours.instant(),
                None => true,
            };
            if newer { self.tips_switch = Some(theirs); }
        }
    }
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("tour-progress.json") }

fn read(path: &Path) -> io::Result<Progress> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice::<Progress>(&bytes).map_err(io::Error::other)
            .and_then(|value| if value.valid() { Ok(value) } else { Err(io::Error::other("data inválida")) }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Progress::default()),
        Err(error) => Err(error),
    }
}

fn apply(path: &Path, patch: Option<Progress>) -> io::Result<Progress> {
    let Some(patch) = patch else { return read(path) };
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    // Dois aparelhos juntando ao mesmo tempo não podem perder a parte um do outro.
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut progress = read(path)?;
    let before = progress.clone();
    progress.merge(patch);
    if progress != before { crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&progress)?)?; }
    Ok(progress)
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !(match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
        return failure(StatusCode::FORBIDDEN, "tour_progress_owner_only", "só o dono pode ler ou alterar o que já foi visto do tour");
    }
    let patch = if request.method() == Method::PUT {
        let parsed = match to_bytes(request.into_body(), 64 * 1024).await {
            Ok(raw) => serde_json::from_slice::<Progress>(&raw).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }.and_then(|p| if p.valid() { Ok(p) } else { Err("data inválida".to_owned()) });
        let patch = match parsed {
            Ok(patch) => patch,
            // App mais novo com campo que este servidor não conhece cai aqui: o motivo fica no log.
            Err(cause) => {
                tracing::warn!(code = "erro_corpo_invalido", %cause, "tour-progress recusou o corpo");
                return failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo inválido");
            }
        };
        Some(patch)
    } else { None };
    let path = path(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || apply(&path, patch)).await {
        Ok(Ok(progress)) => json_response(StatusCode::OK, json!(progress)),
        Ok(Err(error)) => unreadable(&error.to_string()),
        Err(error) => unreadable(&error.to_string()),
    }
}

fn unreadable(cause: &str) -> Response {
    tracing::warn!(code = "tour_progress_unreadable", %cause, "tour-progress sem leitura ou gravação");
    failure(StatusCode::SERVICE_UNAVAILABLE, "tour_progress_unreadable", "não foi possível ler ou guardar o que já foi visto do tour")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch(on: bool, at: &str) -> Option<Switch> { Some(Switch { on, at: at.into() }) }
    fn set(items: &[&str]) -> BTreeSet<String> { items.iter().map(|item| item.to_string()).collect() }

    #[test]
    fn merge_never_downgrades_and_keeps_the_newest_switch() {
        let mut ours = Progress { tour: Some(Tour::Done), tips: set(&["b"]), tips_switch: switch(true, "2026-10-30T12:00:00Z") };
        ours.merge(Progress { tour: Some(Tour::Skipped), tips: set(&["a", "b"]), tips_switch: switch(false, "2026-10-30T11:00:00Z") });
        assert_eq!(ours.tour, Some(Tour::Done));
        assert_eq!(ours.tips.iter().map(String::as_str).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(ours.tips_switch, switch(true, "2026-10-30T12:00:00Z"));
        // Fuso diferente: vale o instante, não o texto.
        ours.merge(Progress { tips_switch: switch(false, "2026-10-30T10:00:01-02:00"), ..Progress::default() });
        assert_eq!(ours.tips_switch, switch(false, "2026-10-30T10:00:01-02:00"));
    }

    #[test]
    fn json_keeps_the_stored_names() {
        let progress = Progress { tour: Some(Tour::Skipped), tips: set(&["a"]), tips_switch: switch(true, "2026-10-30T12:00:00Z") };
        let json = json!({"tour": "pulado", "dicas": ["a"], "dicas_desligadas": {"valor": true, "em": "2026-10-30T12:00:00Z"}});
        assert_eq!(serde_json::to_value(&progress).unwrap(), json);
        assert_eq!(serde_json::from_value::<Progress>(json).unwrap(), progress);
        assert_eq!(serde_json::to_value(Tour::Done).unwrap(), "feito");
    }
}
