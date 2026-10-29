//! Tela web (`frontend/dist`) servida pelo Rust na porta do dono, a chave que a liga e a busca da
//! tela pela chave das fontes na release `dist-commits`.
//!
//! Pasta servida: `~/.hangar/ui/<dist_key>/` quando o `manifest.json` do pacote deste processo (ou,
//! fora de pacote, o da raiz da instalação) diz a chave; senão `<raiz>/frontend/dist` (checkout e
//! máquina ainda não migrada).
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

static SETTING_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// A ausência é o padrão; a trava impede confundi-la com a troca feita pela rota de configuração.
fn enabled_for_request(home: &Path) -> io::Result<bool> {
    let _guard = SETTING_WRITE.lock().map_err(|_| io::Error::other("trava da configuração indisponível"))?;
    match std::fs::read(setting_path(home)) {
        Ok(bytes) => serde_json::from_slice::<Setting>(&bytes).map(|s| s.web_ui).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied => enabled(home),
        Err(error) => Err(error),
    }
}

/// A pasta da tela desta instalação, exista ou não; `None` sem raiz conhecida.
pub fn dist_dir(home: &Path) -> Option<PathBuf> { dist_dir_in(home, crate::update::running_package()) }

/// A chave do pacote deste processo vence a do `current`: depois da troca, o `current` já é a versão
/// nova enquanto o servidor de pé ainda é o de antes.
fn dist_dir_in(home: &Path, running: Option<&crate::update::PackageId>) -> Option<PathBuf> {
    if let Some(key) = running.and_then(|p| p.dist_key.as_deref()).filter(|k| valid_key(k)) {
        return Some(home.join(".hangar").join("ui").join(key));
    }
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

/// Caminhos da tela, inclusive recusas de método e arquivo ausente, pertencem ao Rust.
pub fn matches(_method: &Method, path: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    let path = decoded.as_ref();
    !["/api", "/internal", "/__hangar_server", "/mcp", "/convite", "/par", "/docs", "/redoc", "/openapi.json"].iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/')))
}

pub async fn fallback(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    if matches(req.method(), req.uri().path()) {
        req.headers_mut().remove("x-hangar-static-origin");
        let (forward, _) = match gate(&st, peer, &req).await {
            Ok(value) => value,
            Err(e) => {
                let mut response = crate::auth::unavailable(e);
                cors(req.headers(), response.headers_mut());
                return response;
            }
        };
        let scheme = if forward.https { "https" } else { "http" };
        if let Some(origin) = req.headers().get(header::HOST).and_then(|v|v.to_str().ok())
            .and_then(|host|format!("{scheme}://{host}").parse::<HeaderValue>().ok()) {
            req.headers_mut().insert("x-hangar-static-origin",origin);
        }
        return serve_request(&st, req).await;
    }
    crate::routes::pass_any(State(st), ConnectInfo(peer), req).await
}

/// O repasse privado chega aqui diretamente; nenhuma falha devolve o pedido ao Python.
pub(crate) async fn serve_request(st: &AppState, req: Request) -> Response {
    if let Some(response) = crate::share::gate::preflight(&req) { return response; }
    let (home, method, uri, headers) = (st.accounts.env.home.clone(), req.method().clone(), req.uri().clone(), req.headers().clone());
    let cors_headers = headers.clone();
    let head = method == Method::HEAD;
    let mut response = match tokio::task::spawn_blocking(move || serve(&home, &method, &uri, &headers)).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(code="web_ui_read_failed", "leitura da tela web falhou: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    };
    cors(&cors_headers, response.headers_mut());
    if head { *response.body_mut() = Body::empty(); }
    response
}

fn static_error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({"detail":message}))
}

fn filesystem_error(error: io::Error) -> Response {
    #[cfg(unix)]
    let too_long = error.raw_os_error() == Some(libc::ENAMETOOLONG);
    #[cfg(windows)]
    let too_long = error.raw_os_error().is_some_and(|code| [
        windows_sys::Win32::Foundation::ERROR_FILENAME_EXCED_RANGE as i32,
        windows_sys::Win32::Foundation::ERROR_INVALID_NAME as i32,
    ].contains(&code));
    #[cfg(not(any(unix, windows)))]
    let too_long = false;
    if too_long { return static_error(StatusCode::NOT_FOUND, "Not Found"); }
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory => static_error(StatusCode::NOT_FOUND,"Not Found"),
        io::ErrorKind::PermissionDenied => static_error(StatusCode::UNAUTHORIZED,"Unauthorized"),
        _ => {
            tracing::warn!(code="web_ui_read_failed", "arquivo da tela não pôde ser lido: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}

fn relative(path: &str) -> Option<Vec<String>> {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8().ok()?;
    let mut parts = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {},
            ".." => { parts.pop()?; },
            other if other.contains(['\\', ':', '\0']) => return None,
            other => parts.push(other.to_owned()),
        }
    }
    Some(parts)
}

fn contained(root: &Path, file: &Path) -> io::Result<PathBuf> {
    let real = file.canonicalize()?;
    if real.starts_with(root) { Ok(real) } else { Err(io::ErrorKind::NotFound.into()) }
}

fn serve(home: &Path, method: &Method, uri: &axum::http::Uri, headers: &HeaderMap) -> Response {
    if !matches!(*method, Method::GET | Method::HEAD) { return static_error(StatusCode::METHOD_NOT_ALLOWED,"Method Not Allowed"); }
    let Some(parts) = relative(uri.path()) else { return static_error(StatusCode::NOT_FOUND,"Not Found"); };
    if !enabled_for_request(home).unwrap_or_else(|error| { tracing::warn!("web-ui.json ilegível: {error}"); true }) {
        return detail(StatusCode::NOT_FOUND,"web_ui_disabled","a tela web está desligada nesta máquina",json!({}));
    }
    let Some(dir) = dist_dir(home) else { return static_error(StatusCode::NOT_FOUND,"Not Found"); };
    let root = match dir.canonicalize() { Ok(root) => root, Err(error) => return filesystem_error(error) };
    let requested = parts.iter().fold(root.clone(), |p,c| p.join(c));
    let selected = contained(&root, &requested).and_then(|file| {
        if file.is_dir() { contained(&root,&file.join("index.html")).map(|index|(index,true)) }
        else { Ok((file,false)) }
    });
    let (file, directory) = match selected {
        Ok(found) => found,
        Err(error) if matches!(error.kind(),io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
            if let Ok(file) = contained(&root,&root.join("404.html")) {
                return file_response(&file, headers, true).unwrap_or_else(filesystem_error);
            }
            return static_error(StatusCode::NOT_FOUND,"Not Found");
        }
        Err(error) => return filesystem_error(error),
    };
    if directory && !uri.path().ends_with('/') {
        let origin = headers.get("x-hangar-static-origin").and_then(|v|v.to_str().ok()).map(str::to_owned)
            .or_else(|| headers.get(header::HOST).and_then(|v|v.to_str().ok()).map(|host|format!("http://{host}"))).unwrap_or_default();
        let query = uri.query().map(|q|format!("?{q}")).unwrap_or_default();
        let location = format!("{origin}{}/{query}",uri.path());
        return (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION,location)]).into_response();
    }
    file_response(&file,headers,false).unwrap_or_else(filesystem_error)
}

fn file_response(path: &Path, headers: &HeaderMap, missing: bool) -> io::Result<Response> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() { return Err(io::ErrorKind::NotFound.into()); }
    let modified = meta.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let last = chrono::DateTime::<chrono::Utc>::from_timestamp(modified.as_secs() as i64,0)
        .map(|t|t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()).unwrap_or_default();
    let guessed = mime_guess::from_path(path).first_or_octet_stream().to_string();
    let media = match path.extension().and_then(|e| e.to_str()) {
        Some("js" | "mjs") => "text/javascript",
        Some("ico") => "image/vnd.microsoft.icon",
        _ => &guessed,
    };
    let media = if media.starts_with("text/") { format!("{media}; charset=utf-8") } else { media.into() };
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(header::CONTENT_TYPE,media.parse().unwrap());
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let size = bytes.len() as u64;
    // Range usa a representação original; o gzip conserva o validador próprio já publicado.
    if missing || !headers.contains_key(header::RANGE) { bytes = maybe_gzip(headers,response.headers_mut(),bytes); }
    let gz = if response.headers().contains_key(header::CONTENT_ENCODING) { "-gz" } else { "" };
    let etag = format!("\"{:x}-{:x}{gz}\"",modified.as_nanos(),size);
    response.headers_mut().insert(header::ETAG,etag.parse().unwrap());
    response.headers_mut().insert(header::LAST_MODIFIED,last.parse().unwrap());
    response.headers_mut().insert(header::ACCEPT_RANGES,HeaderValue::from_static("bytes"));
    response.headers_mut().insert(header::VARY,HeaderValue::from_static("Accept-Encoding"));
    if path.extension().is_some_and(|e|e=="html") { response.headers_mut().insert(header::CACHE_CONTROL,HeaderValue::from_static("no-cache")); }
    else if !missing && path.parent().and_then(Path::file_name).is_some_and(|n|n=="assets") { response.headers_mut().insert(header::CACHE_CONTROL,HeaderValue::from_static("public, max-age=31536000, immutable")); }
    if !missing {
        let fresh = if headers.contains_key(header::IF_NONE_MATCH) { not_modified(headers,&etag) }
            else { headers.get(header::IF_MODIFIED_SINCE).and_then(|v|v.to_str().ok()).and_then(http_date_timestamp)
                .is_some_and(|timestamp| timestamp>=modified.as_secs() as i64) };
        if fresh {
            *response.status_mut()=StatusCode::NOT_MODIFIED;
            response.headers_mut().remove(header::CONTENT_TYPE);
            response.headers_mut().remove(header::CONTENT_ENCODING);
            response.headers_mut().remove(header::ACCEPT_RANGES);
            response.headers_mut().remove(header::LAST_MODIFIED);
            return Ok(response);
        }
    }
    if missing { *response.status_mut()=StatusCode::NOT_FOUND; }
    if let Some(range) = headers.get(header::RANGE).and_then(|v|v.to_str().ok()).filter(|_| !missing)
        .filter(|_|headers.get(header::IF_RANGE).is_none_or(|v|v==etag.as_str() || v==last.as_str())) {
        use crate::workspace_routes::RangeError;
        let ranges = match crate::workspace_routes::ranges(&bounded_range_numbers(range),size) {
            Ok(ranges) => ranges,
            Err(RangeError::Malformed(message)) => return Ok((StatusCode::BAD_REQUEST,[(header::CONTENT_TYPE,"text/plain; charset=utf-8")],message).into_response()),
            Err(RangeError::Unsatisfiable) => return Ok((StatusCode::RANGE_NOT_SATISFIABLE,[(header::CONTENT_RANGE,format!("bytes */{size}")),(header::CONTENT_TYPE,"text/plain; charset=utf-8".into())],"").into_response()),
        };
        *response.status_mut()=StatusCode::PARTIAL_CONTENT;
        if ranges.len()==1 {
            let (start,end)=ranges[0];
            response.headers_mut().insert(header::CONTENT_RANGE,format!("bytes {start}-{}/{}",i128::from(end)-1,size).parse().unwrap());
            bytes=bytes[start as usize..end as usize].to_vec();
        } else {
            let boundary="hangar-static-range";
            let mut multipart=Vec::new();
            for (start,end) in ranges {
                multipart.extend_from_slice(format!("--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{}\r\n\r\n",i128::from(end)-1,size).as_bytes());
                multipart.extend_from_slice(&bytes[start as usize..end as usize]);
                multipart.extend_from_slice(b"\r\n");
            }
            multipart.extend_from_slice(format!("--{boundary}--").as_bytes());
            bytes=multipart;
            response.headers_mut().insert(header::CONTENT_TYPE,format!("multipart/byteranges; boundary={boundary}").parse().unwrap());
        }
    }
    response.headers_mut().insert(header::CONTENT_LENGTH,bytes.len().to_string().parse().unwrap());
    *response.body_mut()=Body::from(bytes);
    Ok(response)
}

/// Clientes antigos também enviam as formas RFC 850 e asctime de data HTTP.
fn http_date_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value).ok().map(|date| date.timestamp())
        .or_else(|| ["%A, %d-%b-%y %H:%M:%S GMT", "%a %b %e %H:%M:%S %Y"].iter()
            .find_map(|format| chrono::NaiveDateTime::parse_from_str(value, format).ok())
            .map(|date| date.and_utc().timestamp()))
}

/// O Python aceita inteiros arbitrários; acima de u64 todos têm a mesma relação com o arquivo.
fn bounded_range_numbers(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut number = u64::from(c as u8 - b'0');
            while chars.peek().is_some_and(char::is_ascii_digit) {
                let digit = chars.next().unwrap() as u8 - b'0';
                number = number.saturating_mul(10).saturating_add(u64::from(digit));
            }
            result.push_str(&number.to_string());
        } else {
            result.push(c);
        }
    }
    result
}

/// `If-None-Match` em comparação fraca: lista separada por vírgula, `W/` e `*`.
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers.get_all(header::IF_NONE_MATCH).iter().filter_map(|v|v.to_str().ok()).flat_map(|v|v.split(','))
        .any(|v|v.trim()=="*" || v.trim().trim_start_matches("W/")==etag)
}

/// `GET`/`PUT /api/web-ui` com `{"web_ui": bool}`: só o dono.
pub async fn setting(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !(match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
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
            let _guard = SETTING_WRITE.lock().map_err(|_| io::Error::other("trava da configuração indisponível"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ON_DISK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Depois da troca sem reinício (instalação na mão, a janela até o reinício, volta que falhou) o
    /// `current` já é a versão nova; o servidor de pé serve a tela da versão dele.
    #[test]
    fn ui_serves_the_key_of_the_running_package_not_of_current() {
        let home = tempfile::tempdir().unwrap();
        let current = home.path().join(".hangar/current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("manifest.json"), json!({"version": "0.1.0.2", "commit": "c2", "channel": "main", "dist_key": ON_DISK}).to_string()).unwrap();
        let ui = home.path().join(".hangar/ui");
        let running = crate::update::PackageId { version: "0.1.0.1".into(), commit: "c1".into(), dist_key: Some(RUNNING.into()) };

        assert_eq!(dist_dir_in(home.path(), Some(&running)), Some(ui.join(RUNNING)));
        assert_eq!(dist_dir_in(home.path(), None), Some(ui.join(ON_DISK)), "fora de pacote vale o current");
    }
}
