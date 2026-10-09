//! O Python fornece identidade/configuração; o Rust mantém o cofre e os bytes HTTP.
use super::{
    media,
    store::{UploadStore, project_key},
};
use crate::{accounts::quotas::now, routes::AppState};
use axum::{
    extract::{ConnectInfo, Path, Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

#[derive(Deserialize)]
pub struct Facts {
    pub cwd: PathBuf,
    pub session: String,
    pub root: PathBuf,
    pub retention: i64,
    pub media: String,
}

fn response(value: Value, status: u16) -> Response {
    crate::session_write::json_response(StatusCode::from_u16(status).unwrap(), value)
}
fn failure(status: u16, code: &'static str, message: &str) -> Response {
    crate::diag::coded(
        crate::session_write::detail(
            StatusCode::from_u16(status).unwrap(),
            code,
            message,
            json!({}),
        ),
        code,
    )
}
fn storage_error(error: std::io::Error, upload: bool) -> Response {
    match error.kind() {
        std::io::ErrorKind::FileTooLarge => {
            failure(413, "erro_arquivo_grande", "arquivo maior que 100 MiB")
        }
        std::io::ErrorKind::InvalidInput if upload => failure(
            400,
            "erro_arquivo_vazio",
            "arquivo vazio ou caminho inválido",
        ),
        std::io::ErrorKind::NotFound => {
            failure(404, "erro_upload_inexistente", "arquivo não encontrado")
        }
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied => {
            failure(400, "erro_arquivo_caminho", "caminho inválido")
        }
        _ => failure(
            503,
            "upload_storage_unavailable",
            "não foi possível acessar o cofre de anexos",
        ),
    }
}

pub(crate) fn tail(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("/api/sessions/")?.split_once('/')
}
pub fn matches(method: &Method, path: &str) -> bool {
    let Some((_, tail)) = tail(path) else {
        return false;
    };
    (*method == Method::POST && tail == "upload")
        || (matches!(*method, Method::GET | Method::HEAD)
            && (tail == "uploads"
                || tail
                    .strip_prefix("uploads/")
                    .is_some_and(|v| !v.is_empty() && !v.contains('/'))))
}

async fn facts(
    st: &AppState,
    name: &str,
    filename: &str,
    writing: bool,
) -> Result<Facts, Box<Response>> {
    let unavailable = || {
        Box::new(failure(
            503,
            "upload_facts_unavailable",
            "dados da sessão indisponíveis",
        ))
    };
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("filename", filename)
        .append_pair("writing", if writing { "true" } else { "false" })
        .finish();
    let request = axum::http::Request::get(format!(
        "http://{}/internal/sessions/{encoded}/uploads-facts?{query}",
        st.cfg.upstream
    ))
    .header("x-hangar-internal", &st.cfg.internal_secret)
    .body(axum::body::Body::empty())
    .map_err(|_| unavailable())?;
    // Cliente do servidor: os GET/Range de um anexo reaproveitam a conexão.
    let (status, bytes) = tokio::time::timeout(Duration::from_secs(15), async {
        let response = st.http.request(request).await.ok()?;
        let status = response.status().as_u16();
        let bytes = response.into_body().collect().await.ok()?.to_bytes();
        Some((status, bytes))
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(unavailable)?;
    if !(200..300).contains(&status) {
        return Err(Box::new(response(
            serde_json::from_slice(&bytes)
                .unwrap_or(json!({"detail":"dados da sessão indisponíveis"})),
            status,
        )));
    }
    let value: Facts = serde_json::from_slice(&bytes).map_err(|_| {
        Box::new(failure(
            503,
            "upload_facts_unavailable",
            "dados da sessão inválidos",
        ))
    })?;
    if !value.cwd.is_absolute()
        || !value.root.is_absolute()
        || axum::http::HeaderValue::from_str(&value.media).is_err()
    {
        return Err(Box::new(failure(
            503,
            "upload_facts_unavailable",
            "dados da sessão inválidos",
        )));
    }
    Ok(value)
}

pub async fn public(st: Arc<AppState>, req: Request) -> Response {
    let Some((name, tail)) = tail(req.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (name, tail) = (name.to_owned(), tail.to_owned());
    let Ok(name) = percent_encoding::percent_decode_str(&name).decode_utf8() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    handle(&st, &name, &tail, req).await
}

pub async fn private(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let query: HashMap<String, String> =
        form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let action = query.get("action").map(String::as_str).unwrap_or("");
    let tail = match action {
        "save" if *req.method() == Method::POST => "upload".to_owned(),
        "list" if *req.method() == Method::GET => "uploads".to_owned(),
        "download" if matches!(*req.method(), Method::GET | Method::HEAD) => format!(
            "uploads/{}",
            query.get("filename").map(String::as_str).unwrap_or("")
        ),
        "resolve" if *req.method() == Method::GET => "resolve".to_owned(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    handle(&st, &name, &tail, req).await
}

async fn handle(st: &AppState, name: &str, tail: &str, req: Request) -> Response {
    let query: HashMap<String, String> =
        form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let writing = tail == "upload";
    let filename = if writing {
        req.headers()
            .get("x-filename")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| query.get("name").cloned())
            .unwrap_or_default()
    } else {
        let raw = tail
            .strip_prefix("uploads/")
            .map(str::to_owned)
            .unwrap_or_else(|| query.get("filename").cloned().unwrap_or_default());
        if tail == "resolve" {
            raw
        } else {
            match percent_encoding::percent_decode_str(&raw).decode_utf8() {
                Ok(v) => v.into_owned(),
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            }
        }
    };
    let facts = match facts(st, name, &filename, writing).await {
        Ok(f) => f,
        Err(r) => return *r,
    };
    execute(st, name, tail, req, facts, &filename, query).await
}

/// A verificação de identidade do cofre faz várias chamadas de sistema por arquivo e espera o disco:
/// roda fora do worker async.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(std::io::Error::other)?
}

async fn execute(
    st: &AppState,
    name: &str,
    tail: &str,
    req: Request,
    facts: Facts,
    filename: &str,
    query: HashMap<String, String>,
) -> Response {
    let writing = tail == "upload";
    let project = match project_key(&facts.cwd) {
        Ok(p) => p,
        Err(e) => return storage_error(e, false),
    };
    let store = match UploadStore::new(&facts.root) {
        Ok(s) => s,
        Err(e) => return storage_error(e, false),
    };
    if writing {
        let audio_only = match boolean(query.get("audio_only")) {
            Ok(value) => value,
            Err(()) => return response(json!({"detail":"valor booleano inválido"}), 422),
        };
        let length = req
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());
        let stream = req
            .into_body()
            .into_data_stream()
            .map(|item| item.map_err(std::io::Error::other));
        let path = match store
            .publish(&project, &facts.session, filename, length, stream)
            .await
        {
            Ok(path) => path,
            Err(e) => return storage_error(e, true),
        };
        let prune = {
            let (store, project, retention) = (store.clone(), project.clone(), facts.retention);
            blocking(move || store.prune(&project, retention, now())).await
        };
        if prune.is_err() {
            tracing::warn!(
                code = "upload_prune_failed",
                "poda de anexos falhou após o upload"
            );
        }
        let (frames, audio) = if audio_only {
            (vec![], None)
        } else {
            media::extract(&store, &project, &facts.session, &path).await
        };
        let transcript = match audio {
            Some(audio) => media::transcribe(st, name, audio).await,
            None => String::new(),
        };
        return response(
            json!({"path":path,"frames":frames,"transcript":transcript.trim()}),
            200,
        );
    }
    if tail == "uploads" {
        let (session, retention) = (facts.session.clone(), facts.retention);
        return match blocking(move || store.list(&project, &session, retention, now())).await {
            Ok(files) => response(json!({"files":files}), 200),
            Err(e) => storage_error(e, false),
        };
    }
    if tail == "resolve" {
        let allow_absolute = query.get("allow_absolute").is_some_and(|s| s == "true");
        let (session, filename) = (facts.session.clone(), filename.to_owned());
        return match blocking(move || {
            store.resolve_audio(&project, &session, &filename, allow_absolute)
        })
        .await
        {
            Ok(path) => response(json!({"path":path}), 200),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => failure(
                403,
                "erro_arquivo_caminho_convidado",
                "convidado só transcreve áudio da pasta da sessão",
            ),
            Err(e) => storage_error(e, false),
        };
    }
    let download = match boolean(query.get("download")) {
        Ok(v) => v,
        Err(()) => return response(json!({"detail":"valor booleano inválido"}), 422),
    };
    let (session, name) = (facts.session.clone(), filename.to_owned());
    let (path, file) = match blocking(move || store.open(&project, &session, &name)).await {
        Ok(v) => v,
        Err(e) => return storage_error(e, false),
    };
    let head = *req.method() == Method::HEAD;
    let mut response = crate::workspace_routes::serve_open_file(
        tokio::fs::File::from_std(file),
        &path,
        req.headers(),
        download,
        Some(&facts.media),
    )
    .await;
    if head {
        *response.body_mut() = axum::body::Body::empty();
    }
    response
}

fn boolean(value: Option<&String>) -> Result<bool, ()> {
    value
        .map_or(Some(false), |v| crate::query::fastapi_bool(v))
        .ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::header,
    };
    fn state() -> AppState {
        AppState::new(crate::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream: "127.0.0.1:9".parse().unwrap(),
            internal_secret: "synthetic".into(),
            auth_token: "synthetic".into(),
            log_path: None,
            trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        })
    }
    fn facts(root: &std::path::Path) -> Facts {
        Facts {
            cwd: root.into(),
            session: "durable".into(),
            root: root.join("uploads"),
            retention: 0,
            media: "application/octet-stream".into(),
        }
    }
    #[tokio::test]
    async fn save_lists_same_bytes_and_rejects_empty_or_over_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let req = Request::builder().body(Body::from("binary-data")).unwrap();
        let saved = execute(
            &st,
            "test",
            "upload",
            req,
            facts(tmp.path()),
            "test.bin",
            Default::default(),
        )
        .await;
        assert_eq!(saved.status(), StatusCode::OK);
        let saved: Value =
            serde_json::from_slice(&to_bytes(saved.into_body(), 4096).await.unwrap()).unwrap();
        let path = PathBuf::from(saved["path"].as_str().unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"binary-data");
        let list = execute(
            &st,
            "test",
            "uploads",
            Request::new(Body::empty()),
            facts(tmp.path()),
            "",
            Default::default(),
        )
        .await;
        let list: Value =
            serde_json::from_slice(&to_bytes(list.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            list["files"][0]["filename"],
            path.file_name().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(list["files"][0]["size"], 11);
        assert!(list["files"][0]["expires_in_days"].is_null());
        for (req, expected) in [
            (Request::new(Body::empty()), StatusCode::BAD_REQUEST),
            (
                Request::builder()
                    .header(
                        header::CONTENT_LENGTH,
                        super::super::store::MAX_UPLOAD_BYTES + 1,
                    )
                    .body(Body::from("x"))
                    .unwrap(),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            assert_eq!(
                execute(
                    &st,
                    "test",
                    "upload",
                    req,
                    facts(tmp.path()),
                    "file.bin",
                    Default::default()
                )
                .await
                .status(),
                expected
            );
        }
        let directory = path.parent().unwrap();
        assert!(std::fs::read_dir(directory).unwrap().all(|item| {
            !item
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".upload-")
        }));
    }
    #[tokio::test]
    async fn file_response_keeps_range_download_and_html_isolation() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let info = facts(tmp.path());
        let project = project_key(&info.cwd).unwrap();
        let store = UploadStore::new(&info.root).unwrap();
        let content = b"0123456789";
        let path = store
            .publish(
                &project,
                "durable",
                "file.xml",
                None,
                futures_util::stream::iter([Ok(bytes::Bytes::from_static(content))]),
            )
            .await
            .unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        let req = Request::builder()
            .header(header::RANGE, "bytes=2-4")
            .body(Body::empty())
            .unwrap();
        let mut info = facts(tmp.path());
        info.media = "text/xml".into();
        let response = execute(
            &st,
            "test",
            "uploads/file.xml",
            req,
            info,
            filename,
            Default::default(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(
            response.headers()["content-security-policy"],
            "sandbox; script-src 'none'"
        );
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            &content[2..5]
        );
        let download = execute(
            &st,
            "test",
            "uploads/file.xml",
            Request::new(Body::empty()),
            facts(tmp.path()),
            filename,
            [("download".into(), "true".into())].into(),
        )
        .await;
        assert!(
            download.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        let html = vec![b'a'; 50003];
        let path = store
            .publish(
                &project,
                "durable",
                "test.html",
                None,
                futures_util::stream::iter([Ok(bytes::Bytes::from(html.clone()))]),
            )
            .await
            .unwrap();
        let mut info = facts(tmp.path());
        info.media = "text/html".into();
        let response = execute(
            &st,
            "test",
            "uploads/test.html",
            Request::new(Body::empty()),
            info,
            path.file_name().unwrap().to_str().unwrap(),
            Default::default(),
        )
        .await;
        let body = String::from_utf8(
            to_bytes(response.into_body(), 100000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("sandbox=\"allow-scripts allow-popups\""));
        let encoded = body
            .split("base64,")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            html
        );
    }
    #[tokio::test]
    async fn audio_resolution_refuses_guest_absolute_path_and_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let req = Request::new(Body::empty());
        let path = tmp
            .path()
            .join("uploads/outside.wav")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            execute(
                &st,
                "test",
                "resolve",
                req,
                facts(tmp.path()),
                &path,
                Default::default()
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            execute(
                &st,
                "test",
                "resolve",
                Request::new(Body::empty()),
                facts(tmp.path()),
                "../outside",
                Default::default()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
}
