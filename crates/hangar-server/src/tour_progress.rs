//! O que a pessoa já viu do tour do nativo, guardado no servidor para outro computador ligado a
//! ele não mostrar de novo. A junção nunca desfaz o que algum aparelho já viu.
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeSet, io, net::SocketAddr, path::{Path, PathBuf}, sync::Arc};
use crate::{routes::{AppState, gate}, session_write::{detail_body, json_response}};

// A ordem das variantes é a da junção: `feito` vence `pulado` (e `None` perde para os dois).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Tour { Pulado, Feito }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Switch { valor: bool, em: String }

impl Switch {
    fn instant(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> { chrono::DateTime::parse_from_rfc3339(&self.em).ok() }
}

/// Mesmo formato no arquivo e no corpo do `PUT`; no corpo, campo ausente não acrescenta nada.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    #[serde(default)] tour: Option<Tour>,
    #[serde(default)] dicas: BTreeSet<String>,
    #[serde(default)] dicas_desligadas: Option<Switch>,
}

impl Progress {
    fn valid(&self) -> bool { self.dicas_desligadas.as_ref().is_none_or(|value| value.instant().is_some()) }

    fn merge(&mut self, other: Progress) {
        self.tour = self.tour.max(other.tour);
        self.dicas.extend(other.dicas);
        if let Some(theirs) = other.dicas_desligadas {
            let newer = match &self.dicas_desligadas {
                Some(ours) => theirs.instant() > ours.instant(),
                None => true,
            };
            if newer { self.dicas_desligadas = Some(theirs); }
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
        let parsed = to_bytes(request.into_body(), 64 * 1024).await.ok()
            .and_then(|raw| serde_json::from_slice::<Progress>(&raw).ok())
            .filter(Progress::valid);
        let Some(patch) = parsed else {
            return failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo inválido");
        };
        Some(patch)
    } else { None };
    let path = path(&state.accounts.env.home);
    match tokio::task::spawn_blocking(move || apply(&path, patch)).await {
        Ok(Ok(progress)) => json_response(StatusCode::OK, json!(progress)),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "tour_progress_unreadable", "não foi possível ler ou guardar o que já foi visto do tour"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch(valor: bool, em: &str) -> Option<Switch> { Some(Switch { valor, em: em.into() }) }
    fn set(items: &[&str]) -> BTreeSet<String> { items.iter().map(|item| item.to_string()).collect() }

    #[test]
    fn merge_never_downgrades_and_keeps_the_newest_switch() {
        let mut ours = Progress { tour: Some(Tour::Feito), dicas: set(&["b"]), dicas_desligadas: switch(true, "2026-10-30T12:00:00Z") };
        ours.merge(Progress { tour: Some(Tour::Pulado), dicas: set(&["a", "b"]), dicas_desligadas: switch(false, "2026-10-30T11:00:00Z") });
        assert_eq!(ours.tour, Some(Tour::Feito));
        assert_eq!(ours.dicas.iter().map(String::as_str).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(ours.dicas_desligadas, switch(true, "2026-10-30T12:00:00Z"));
        // Fuso diferente: vale o instante, não o texto.
        ours.merge(Progress { dicas_desligadas: switch(false, "2026-10-30T10:00:01-02:00"), ..Progress::default() });
        assert_eq!(ours.dicas_desligadas, switch(false, "2026-10-30T10:00:01-02:00"));
    }
}
