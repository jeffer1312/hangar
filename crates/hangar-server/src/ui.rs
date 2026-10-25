//! Tela web (`frontend/dist`) servida pelo Rust na porta do dono, a chave que a liga e a busca da
//! tela pela chave das fontes na release `dist-commits`.
//!
//! Pasta servida: `~/.hangar/ui/<dist_key>/` quando o `manifest.json` da raiz da instalação diz a
//! chave (modo pacote); senão `<raiz>/frontend/dist` (checkout e máquina ainda não migrada).
use std::io;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::routes::{AppState, cors, gate, maybe_gzip};
use crate::session_write::{detail, json_response};

/// De onde `fetch` baixa: os mesmos nomes que o `ci.yml` publica.
pub const DIST_COMMITS_URL: &str = "https://github.com/jeffer1312/hangar/releases/download/dist-commits";

#[derive(Debug)]
pub struct FetchError {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.detail) }
}

impl std::error::Error for FetchError {}

fn fail(code: &'static str, detail: impl std::fmt::Display) -> FetchError { FetchError { code, detail: detail.to_string() } }

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Setting { web_ui: bool }

fn setting_path(home: &Path) -> PathBuf { home.join(".hangar").join("web-ui.json") }

/// A chave é um sha256 em hexadecimal minúsculo: vira nome de pasta, nada mais passa.
fn valid_key(key: &str) -> bool { key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }

/// Tela web ligada nesta máquina; sem o arquivo, ligada.
pub fn enabled(home: &Path) -> io::Result<bool> {
    match crate::runtime::queue::read_replaced(&setting_path(home)) {
        Ok(bytes) => serde_json::from_slice::<Setting>(&bytes).map(|s| s.web_ui).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

/// A pasta da tela desta instalação, exista ou não; `None` sem raiz conhecida.
pub fn dist_dir(home: &Path) -> Option<PathBuf> {
    let root = crate::install_root::root(home)?;
    Some(match manifest_key(&root) {
        Some(key) => home.join(".hangar").join("ui").join(key),
        None => root.join("frontend").join("dist"),
    })
}

/// O `dist_key` do `manifest.json` da raiz (o campo que `diag.install_manifest` lê no Python).
fn manifest_key(root: &Path) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("manifest.json")).ok()?).ok()?;
    manifest.get("dist_key")?.as_str().filter(|k| valid_key(k)).map(str::to_owned)
}

/// Fallback da 8765: serve o que é da tela e repassa o resto ao Python.
pub async fn fallback(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD)
        && let Some(rel) = relative(req.uri().path())
    {
        let (home, headers) = (st.accounts.env.home.clone(), req.headers().clone());
        if let Ok(Some(resp)) = tokio::task::spawn_blocking(move || serve(&home, &rel, &headers)).await {
            return resp;
        }
    }
    crate::routes::pass_any(State(st), ConnectInfo(peer), req).await
}

/// Componentes do caminho pedido; `None` para a API e para caminho que sairia da pasta.
fn relative(path: &str) -> Option<Vec<String>> {
    let api = ["/api", "/internal", "/__hangar_server", "/mcp"].iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/')));
    if api { return None; }
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8().ok()?;
    let parts: Vec<String> = decoded.split('/').filter(|c| !c.is_empty()).map(str::to_owned).collect();
    let safe = parts.iter().all(|c| c != "." && c != ".." && !c.contains(['\\', ':', '\0']));
    safe.then_some(parts)
}

/// `None` = não é da tela (o Python responde). Desligada, a raiz diz o motivo; sem a pasta, a raiz
/// segue ao Python como antes e o motivo vai só ao log.
fn serve(home: &Path, rel: &[String], headers: &HeaderMap) -> Option<Response> {
    let dir = dist_dir(home)?;
    let mut file = rel.iter().fold(dir, |p, c| p.join(c));
    if file.is_dir() {
        file = file.join("index.html");
    }
    let exists = file.is_file();
    if !exists && !rel.is_empty() {
        return None;
    }
    let on = enabled(home).unwrap_or_else(|e| {
        tracing::warn!("web-ui.json ilegível, tela web segue ligada: {e}");
        true
    });
    if !on {
        let mut resp = detail(StatusCode::NOT_FOUND, "web_ui_disabled", "a tela web está desligada nesta máquina", json!({}));
        cors(headers, resp.headers_mut());
        return Some(resp);
    }
    if !exists {
        tracing::warn!(code = "web_ui_missing", file = %file.display(), "tela web ausente, a raiz segue ao Python");
        return None;
    }
    file_response(&file, headers)
}

/// Como o `_UIStatic` do Python: HTML revalida sempre (`no-cache` com ETag), `assets/` tem hash no
/// nome e é imutável; o resto sem diretiva.
fn file_response(file: &Path, headers: &HeaderMap) -> Option<Response> {
    let meta = std::fs::metadata(file).ok()?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    let cache = if file.extension().is_some_and(|e| e == "html") {
        Some("no-cache")
    } else if file.parent().and_then(Path::file_name).is_some_and(|n| n == "assets") {
        Some("public, max-age=31536000, immutable")
    } else {
        None
    };
    let media = mime_guess::from_path(file).first_or_octet_stream().to_string();
    let media = if media.starts_with("text/") { format!("{media}; charset=utf-8") } else { media };
    let mut resp = Response::new(Body::empty());
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&media).ok()?);
    // O corpo sai antes da tag: a versão gzip é outra representação e tem tag própria.
    let body = maybe_gzip(headers, resp.headers_mut(), std::fs::read(file).ok()?);
    let gz = if resp.headers().contains_key(header::CONTENT_ENCODING) { "-gz" } else { "" };
    let etag = format!("\"{:x}-{:x}{gz}\"", modified.as_nanos(), meta.len());
    if not_modified(headers, &etag) {
        let kept = std::mem::take(resp.headers_mut());
        resp = StatusCode::NOT_MODIFIED.into_response();
        *resp.headers_mut() = kept;
        resp.headers_mut().remove(header::CONTENT_TYPE);
        resp.headers_mut().remove(header::CONTENT_ENCODING);
    } else {
        *resp.body_mut() = Body::from(body);
    }
    let out = resp.headers_mut();
    out.insert(header::ETAG, HeaderValue::from_str(&etag).ok()?);
    out.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    if let Some(cache) = cache {
        out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    }
    cors(headers, out);
    Some(resp)
}

/// `If-None-Match` em comparação fraca: lista separada por vírgula, `W/` e `*`.
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers.get_all(header::IF_NONE_MATCH).iter().filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| t == "*" || t.trim_start_matches("W/") == etag)
}

/// `GET`/`PUT /api/web-ui` com `{"web_ui": bool}`: só o dono.
pub async fn setting(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !gate(&st, peer, &req).1 {
        return detail(StatusCode::FORBIDDEN, "web_ui_owner_only", "só o dono pode ler ou mudar a tela web", json!({}));
    }
    let save = if req.method() == Method::PUT {
        let parsed = to_bytes(req.into_body(), 1024).await.ok().and_then(|raw| serde_json::from_slice::<Setting>(&raw).ok());
        let Some(save) = parsed else {
            return detail(StatusCode::BAD_REQUEST, "web_ui_invalid", "esperado {\"web_ui\": true ou false}", json!({}));
        };
        Some(save)
    } else {
        None
    };
    let home = st.accounts.env.home.clone();
    let result = tokio::task::spawn_blocking(move || {
        if let Some(save) = save {
            let path = setting_path(&home);
            std::fs::create_dir_all(path.parent().expect("pasta do arquivo"))?;
            crate::runtime::queue::atomic_write(&path, &serde_json::to_vec(&save)?)?;
        }
        enabled(&home)
    }).await;
    let cause = match result {
        Ok(Ok(on)) => return json_response(StatusCode::OK, json!({"web_ui": on})),
        Ok(Err(e)) => e.to_string(),
        Err(e) => format!("tarefa morreu: {e}"),
    };
    tracing::warn!(code = "web_ui_unavailable", "tela web: escolha não lida ou não guardada: {cause}");
    detail(StatusCode::SERVICE_UNAVAILABLE, "web_ui_unavailable", "não foi possível ler ou guardar a escolha da tela web", json!({}))
}

/// Baixa `frontend-dist-<chave>.tar.gz` de `dist-commits`, confere o `.sha256`, extrai em
/// `<ui_dir>/<chave>.tmp` e renomeia para `<ui_dir>/<chave>`; ficam as 2 chaves mais novas.
/// Chave já instalada não baixa nada. Quem chama decide pela chave `web_ui` ([`enabled`]).
pub async fn fetch(dist_key: &str, ui_dir: &Path) -> Result<PathBuf, FetchError> {
    fetch_from(&reqwest::Client::new(), DIST_COMMITS_URL, dist_key, ui_dir).await
}

pub async fn fetch_from(http: &reqwest::Client, base: &str, dist_key: &str, ui_dir: &Path) -> Result<PathBuf, FetchError> {
    if !valid_key(dist_key) {
        return Err(fail("ui_key_invalid", dist_key));
    }
    let final_dir = ui_dir.join(dist_key);
    if final_dir.join("index.html").is_file() {
        return Ok(final_dir);
    }
    let name = format!("frontend-dist-{dist_key}.tar.gz");
    let sha = download(http, &format!("{base}/{name}.sha256"), Duration::from_secs(15)).await?;
    let expected = String::from_utf8_lossy(&sha).split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let archive = download(http, &format!("{base}/{name}"), Duration::from_secs(180)).await?;
    let got: String = ring::digest::digest(&ring::digest::SHA256, &archive).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if got != expected {
        return Err(fail("ui_sha_mismatch", name));
    }
    let (ui_dir, key) = (ui_dir.to_owned(), dist_key.to_owned());
    tokio::task::spawn_blocking(move || install(&archive, &ui_dir, &key)).await.map_err(|e| fail("ui_io", e))?
}

async fn download(http: &reqwest::Client, url: &str, timeout: Duration) -> Result<bytes::Bytes, FetchError> {
    let resp = http.get(url).timeout(timeout).send().await.map_err(|e| fail("ui_download_failed", e))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(fail("ui_key_not_published", url));
    }
    let resp = resp.error_for_status().map_err(|e| fail("ui_download_failed", e))?;
    resp.bytes().await.map_err(|e| fail("ui_download_failed", e))
}

fn install(archive: &[u8], ui_dir: &Path, key: &str) -> Result<PathBuf, FetchError> {
    let io_fail = |e: io::Error| fail("ui_io", e);
    let (tmp, final_dir) = (ui_dir.join(format!("{key}.tmp")), ui_dir.join(key));
    // Sobra de uma busca interrompida.
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(io_fail)?;
    let result = unpack(archive, &tmp).and_then(|()| {
        if !tmp.join("index.html").is_file() {
            return Err(fail("ui_archive_invalid", "index.html ausente"));
        }
        // Pasta final sem index.html não serve: a nova toma o lugar.
        if std::fs::symlink_metadata(&final_dir).is_ok() {
            std::fs::remove_dir_all(&final_dir).map_err(io_fail)?;
        }
        hangar_package::rename_with_retry(&tmp, &final_dir, &mut |a, b| std::fs::rename(a, b), &mut std::thread::sleep)
            .map_err(|e| fail("ui_rename_failed", e))
    });
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    prune(ui_dir, key);
    Ok(final_dir)
}

/// Só arquivo e pasta, com caminho relativo dentro do destino.
fn unpack(archive: &[u8], dest: &Path) -> Result<(), FetchError> {
    let bad = |e: io::Error| fail("ui_archive_invalid", e);
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in ar.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        let kind = entry.header().entry_type();
        if matches!(kind, tar::EntryType::XGlobalHeader | tar::EntryType::XHeader) {
            continue;
        }
        let rel = entry.path().map_err(bad)?.into_owned();
        if !rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
            return Err(fail("ui_archive_invalid", format!("caminho inseguro {}", rel.display())));
        }
        let target = dest.join(&rel);
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(bad)?;
        } else if kind.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(bad)?;
            }
            entry.unpack(&target).map_err(bad)?;
        } else {
            return Err(fail("ui_archive_invalid", format!("{}: tipo {kind:?}", rel.display())));
        }
    }
    Ok(())
}

/// A chave que o servidor serve até o reinício: a do manifesto da raiz atual. `ui_dir` é
/// `<home>/.hangar/ui`.
fn in_use(ui_dir: &Path) -> Option<String> {
    let home = ui_dir.parent()?.parent()?;
    manifest_key(&crate::install_root::root(home)?)
}

/// Fica a chave nova e a em uso; sem chave em uso distinta, a mais recente das outras.
fn prune(ui_dir: &Path, keep: &str) {
    let Ok(read) = std::fs::read_dir(ui_dir) else { return };
    let serving = in_use(ui_dir).filter(|k| k != keep);
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = read.flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n != keep && Some(n) != serving.as_deref() && valid_key(n)))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    others.sort();
    if serving.is_none() {
        others.pop();
    }
    for (_, dir) in others {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), "tela web antiga não saiu: {e}");
        }
    }
}
