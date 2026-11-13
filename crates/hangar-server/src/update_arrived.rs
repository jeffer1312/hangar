//! O que cada atualização trouxe, gravado na subida, e as rotas com que cada aparelho do dono lê e
//! marca visto. Quem grava é o servidor novo: o executor da atualização roda o binário antigo.
use crate::{routes::{AppState, gate}, session_write::{detail_body, json_response}};
use crate::update::http::Mode;
use axum::{body::to_bytes, extract::{ConnectInfo, Query, Request, State}, http::StatusCode, response::Response};
use serde_json::{Value, json};
use std::{collections::HashMap, io, net::SocketAddr, path::{Path, PathBuf}, sync::Arc};

/// Grava a chegada quando a versão em uso não é a última gravada; falha vai ao log e a subida segue.
pub async fn record_startup(home: &Path) {
    let recorded = match crate::update::http::mode(home) {
        Mode::Package(_) => blocking(home, package).await,
        Mode::Developer(root) => checkout(home, &root).await,
        Mode::Unmigrated => checkout(home, &crate::update_notes::server_root()).await,
    };
    if let Err(cause) = recorded {
        tracing::warn!(code = "update_arrived_record_failed", %cause, "o que esta atualização trouxe não foi gravado");
    }
}

fn package(home: &Path) -> io::Result<()> {
    let layout = crate::update::Layout::new(home);
    let current = crate::update::installed(&layout).ok_or_else(|| io::Error::other("manifest.json da versão em uso ilegível"))?.version;
    let known = hangar_update_news::known_version(home)?;
    if known.as_deref() == Some(current.as_str()) { return Ok(()); }
    let (from, news) = match previous(&layout, known.as_deref()) {
        Some((dir, version)) => (version, package_news(&layout.current(), &dir)?),
        None => (known.unwrap_or_default(), Vec::new()),
    };
    hangar_update_news::record(home, &from, &current, news).map(drop)
}

/// A pasta da versão de antes: a da última gravada, senão a maior que sobrou ao lado da em uso (a
/// poda só guarda as duas).
fn previous(layout: &crate::update::Layout, known: Option<&str>) -> Option<(PathBuf, String)> {
    let current = std::fs::canonicalize(layout.current()).ok();
    let version = |dir: &Path| (std::fs::canonicalize(dir).ok() != current).then(|| crate::update::package_manifest(dir))
        .flatten().and_then(|manifest| manifest["version"].as_str().map(str::to_owned));
    if let Some(dir) = known.map(|known| layout.versions().join(known)) && let Some(found) = version(&dir) { return Some((dir, found)); }
    std::fs::read_dir(layout.versions()).ok()?.flatten()
        .filter_map(|entry| Some((entry.file_name().to_str()?.parse::<hangar_package::Version>().ok()?, entry.path())))
        .filter_map(|(order, dir)| version(&dir).map(|found| (order, dir, found)))
        .max_by_key(|(order, ..)| *order)
        .map(|(_, dir, found)| (dir, found))
}

/// `*.md` de `docs/novidades` da versão em uso que a anterior não tem.
fn package_news(current: &Path, previous: &Path) -> io::Result<Vec<Value>> {
    let dir = current.join("docs").join("novidades");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let old = previous.join("docs").join("novidades");
    let mut news = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if !hangar_docs::is_note_file(&name) || old.join(&name).exists() { continue; }
        let id = name.trim_end_matches(".md");
        if let Some((rank, item)) = hangar_docs::news_item(id, &std::fs::read_to_string(entry.path())?, std::env::consts::OS) {
            news.push((rank, id.to_owned(), item));
        }
    }
    Ok(crate::update_notes::order_news(news))
}

async fn checkout(home: &Path, root: &Path) -> io::Result<()> {
    let failed = || io::Error::other(format!("git rev-parse --show-toplevel HEAD falhou em {}", root.display()));
    let out = crate::update_notes::git(root, &["rev-parse", "--show-toplevel", "HEAD"]).await.map_err(|()| failed())?;
    let mut lines = out.lines().map(str::trim);
    let (Some(repo), Some(head)) = (lines.next().map(PathBuf::from), lines.next().map(str::to_owned)) else { return Err(failed()) };
    let known = blocking(home, hangar_update_news::known_version).await?;
    if known.as_deref() == Some(head.as_str()) { return Ok(()); }
    let news = match known.as_deref() {
        // `from..HEAD`, nunca `HEAD..alvo`: depois do pull o HEAD já é o alvo.
        Some(from) if crate::update_notes::git(&repo, &["rev-parse", "--verify", "--quiet", &format!("{from}^{{commit}}")]).await.is_ok() =>
            crate::update_notes::added_news(&repo, from, &head).await.map_err(|()| io::Error::other(format!("novidades de {from} a {head} ilegíveis")))?,
        // Versão de pacote ou commit sumido: não há de onde comparar.
        _ => Vec::new(),
    };
    let from = known.unwrap_or_default();
    blocking(home, move |home| hangar_update_news::record(home, &from, &head, news).map(drop)).await
}

pub(crate) async fn blocking<T: Send + 'static>(home: &Path, work: impl FnOnce(&Path) -> io::Result<T> + Send + 'static) -> io::Result<T> {
    let home = home.to_owned();
    tokio::task::spawn_blocking(move || work(&home)).await.map_err(io::Error::other)?
}

/// `GET /api/update/arrived?device=<id>`: o que o aparelho ainda não viu. A primeira leitura grava o
/// aparelho, por isso o convidado é recusado antes.
pub async fn unseen(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(refused) = owner_gate(&state, peer, &request, owner_only).await { return refused; }
    let device = Query::<HashMap<String, String>>::try_from_uri(request.uri()).ok()
        .and_then(|Query(mut query)| query.remove("device")).filter(|device| hangar_update_news::valid_device(device));
    let Some(device) = device else { return bad_device() };
    match blocking(&state.accounts.env.home, move |home| hangar_update_news::unseen(home, &device)).await {
        Ok(found) => json_response(StatusCode::OK, json!({"news": by_kind(found.news), "through": found.through})),
        Err(error) => unavailable(&error),
    }
}

#[derive(serde::Deserialize)]
struct Seen { device: String, through: u64 }

/// `POST /api/update/arrived/seen` com `{device, through}`: o cursor do aparelho avança, nunca volta.
pub async fn seen(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(refused) = owner_gate(&state, peer, &request, owner_only).await { return refused; }
    let body = to_bytes(request.into_body(), 4 * 1024).await.ok()
        .and_then(|raw| serde_json::from_slice::<Seen>(&raw).ok()).filter(|body| hangar_update_news::valid_device(&body.device));
    let Some(Seen { device, through }) = body else { return bad_device() };
    match blocking(&state.accounts.env.home, move |home| hangar_update_news::mark_seen(home, &device, through)).await {
        Ok(cursor) => json_response(StatusCode::OK, json!({"through": cursor})),
        Err(error) => unavailable(&error),
    }
}

/// O crate devolve na ordem das chegadas; a tela mostra pela de `/api/update/notes`.
fn by_kind(news: Vec<Value>) -> Vec<Value> {
    let rank = |item: &Value| match item["kind"].as_str() { Some("new") => 0, Some("improvement") => 1, _ => 2 };
    crate::update_notes::order_news(news.into_iter().map(|item| (rank(&item), item["id"].as_str().unwrap_or_default().to_owned(), item)).collect())
}

/// Só o dono: o convidado recebe `refused`, com o código da rota, não o 401 genérico do `routes::owner_gate`.
/// Sem `async fn`: o `&Request` atravessando o `await` tiraria o `Send` do handler.
pub(crate) fn owner_gate(state: &AppState, peer: SocketAddr, request: &Request, refused: fn() -> Response)
    -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let admission = gate(state, peer, request);
    async move {
        match admission.await {
            Ok((_, true)) => Ok(()),
            Ok(_) => Err(refused()),
            Err(error) => Err(crate::auth::unavailable(error)),
        }
    }
}

fn owner_only() -> Response {
    json_response(StatusCode::FORBIDDEN, detail_body("update_arrived_owner_only", "só o dono pode ler ou marcar as novidades que chegaram", json!({})))
}

fn bad_device() -> Response {
    json_response(StatusCode::UNPROCESSABLE_ENTITY, detail_body("update_arrived_bad_device", "id de aparelho ou corpo inválido", json!({})))
}

fn unavailable(error: &io::Error) -> Response {
    tracing::warn!(code = "update_arrived_unavailable", %error, "novidades que chegaram sem leitura ou gravação");
    json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body("update_arrived_unavailable", "não deu para ler as novidades que chegaram", json!({})))
}
