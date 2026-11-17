//! O cofre de anexos e as rotas dele; a sessão vem da descoberta da lista do próprio Rust.
use super::{
    media,
    store::{UploadStore, project_key},
};
use crate::routes::AppState;
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

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
pub(crate) fn failure(status: u16, code: &'static str, message: &str) -> Response {
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

/// `runtime_config.get("upload_retention_days")`: o `runtime-config.json` vence, depois
/// `CP_UPLOAD_RETENTION_DAYS`, depois 30 dias.
pub fn retention_days(runtime: &Value, env: Option<&str>) -> i64 {
    let from_file = match &runtime["upload_retention_days"] {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    from_file.or_else(|| env?.trim().parse().ok()).unwrap_or(30)
}

/// Arquivo ausente vale o ambiente e o padrão; ilegível ou torto é erro, nunca os 30 dias: com a
/// retenção errada a poda apagaria anexos que a pessoa mandou guardar.
pub fn retention_from(read: std::io::Result<Vec<u8>>, env: Option<&str>) -> Result<i64, &'static str> {
    let runtime = match read {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "upload_retention_config_invalid")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(_) => return Err("upload_retention_config_unreadable"),
    };
    Ok(retention_days(&runtime, env))
}

/// Sem retenção legível, nada expira nem é podado; a causa vai ao diário.
async fn retention(st: &AppState) -> i64 {
    let path = st.accounts.env.claude_base.join("runtime-config.json");
    let env = std::env::var("CP_UPLOAD_RETENTION_DAYS").ok();
    retention_from(tokio::fs::read(&path).await, env.as_deref()).unwrap_or_else(|code| {
        st.diag.report("rust.uploads_failed", "", code, "a retenção de anexos não pôde ser lida; nada é podado");
        0
    })
}

/// Sessão, pasta e configuração do cofre, pela descoberta da lista; o envio confere antes a troca
/// de conversa e solta, como o `_transfer_check`.
pub(super) async fn facts(
    st: &AppState,
    name: &str,
    filename: &str,
    writing: bool,
) -> Result<Facts, Box<Response>> {
    if writing
        && let Some(runtime) = st.state.runtime.get()
        && runtime.ingress().enter(name, st.write_gate_wait).await.is_err()
    {
        return Err(Box::new(crate::diag::coded(
            crate::session_write::json_response(StatusCode::CONFLICT, crate::session_write::busy_body()),
            "session_transfer_busy",
        )));
    }
    let info = match tokio::time::timeout(crate::locate::LOCATE_TIMEOUT, st.side.locate.info(name)).await {
        Ok(Ok(info)) => info,
        _ => return Err(Box::new(failure(503, "upload_facts_unavailable", "dados da sessão indisponíveis"))),
    };
    let Some(info) = info else {
        return Err(Box::new(failure(404, "erro_sessao_inexistente", "sessão não encontrada")));
    };
    let Some(cwd) = info.cwd.filter(|c| c.is_absolute()) else {
        return Err(Box::new(failure(if writing { 409 } else { 404 }, "erro_cwd_indisponivel", "cwd da sessão indisponível")));
    };
    // Sessão sem transcript ainda cai no nome: anexo mandado nela fica para trás num rename.
    let session = if info.session_key.is_empty() { name.to_owned() } else { info.session_key };
    Ok(Facts {
        cwd,
        session,
        root: st.hangar_dir.join("uploads"),
        retention: retention(st).await,
        media: mime_guess::from_path(filename).first_or_octet_stream().to_string(),
    })
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
        let raw = tail.strip_prefix("uploads/").unwrap_or_default();
        match percent_encoding::percent_decode_str(raw).decode_utf8() {
            Ok(v) => v.into_owned(),
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    };
    let facts = match facts(st, name, &filename, writing).await {
        Ok(f) => f,
        Err(r) => return *r,
    };
    execute(st, tail, req, facts, &filename, query).await
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

/// O cofre e a chave do projeto da sessão.
pub(super) fn vault(st: &AppState, facts: &Facts) -> Result<(UploadStore, String), Response> {
    let project = project_key(&facts.cwd).map_err(|e| storage_error(e, false))?;
    let store = UploadStore::new(&facts.root).map_err(|e| storage_error(e, false))?;
    Ok((store.with_naming(st.upload_naming.clone()), project))
}

/// Publica o anexo e poda o projeto; falha da poda não custa o anexo que acabou de entrar.
pub(super) async fn publish<S>(st: &AppState, store: &UploadStore, project: &str, facts: &Facts, filename: &str, length: Option<u64>, stream: S)
    -> Result<PathBuf, Response>
where S: futures_util::Stream<Item = std::io::Result<bytes::Bytes>> + Send + Unpin {
    let path = store.publish(project, &facts.session, filename, length, stream).await.map_err(|e| storage_error(e, true))?;
    let (store, project, retention, now) = (store.clone(), project.to_owned(), facts.retention, (st.upload_naming.now)());
    if blocking(move || store.prune(&project, retention, now)).await.is_err() {
        tracing::warn!(code = "upload_prune_failed", "poda de anexos falhou após o upload");
    }
    Ok(path)
}

/// Áudio já enviado, por nome solto ou caminho absoluto do mesmo projeto (só o dono).
pub(super) async fn resolve_audio(store: &UploadStore, project: &str, facts: &Facts, filename: &str, allow_absolute: bool) -> Result<PathBuf, Response> {
    let (store, project, session, filename) = (store.clone(), project.to_owned(), facts.session.clone(), filename.to_owned());
    blocking(move || store.resolve_audio(&project, &session, &filename, allow_absolute)).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => failure(403, "erro_arquivo_caminho_convidado", "convidado só transcreve áudio da pasta da sessão"),
        _ => storage_error(e, false),
    })
}

async fn execute(
    st: &AppState,
    tail: &str,
    req: Request,
    facts: Facts,
    filename: &str,
    query: HashMap<String, String>,
) -> Response {
    let (store, project) = match vault(st, &facts) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let now = st.upload_naming.now.clone();
    if tail == "upload" {
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
            // O teto do corpo da borda (`edge_rules`) é o mesmo do arquivo: estourar é 413, não falha do disco.
            .map(|item| item.map_err(|error| if crate::http_body_limit::is_limit(&error) {
                std::io::ErrorKind::FileTooLarge.into()
            } else {
                std::io::Error::other(error)
            }));
        let path = match publish(st, &store, &project, &facts, filename, length, stream).await {
            Ok(path) => path,
            Err(r) => return r,
        };
        let (frames, audio) = if audio_only {
            (vec![], None)
        } else {
            media::extract(&store, &project, &facts.session, &path).await
        };
        let transcript = match audio {
            Some(audio) => media::transcribe(st, audio).await,
            None => String::new(),
        };
        return response(
            json!({"path":path,"frames":frames,"transcript":transcript.trim()}),
            200,
        );
    }
    if tail == "uploads" {
        let (session, retention, now) = (facts.session.clone(), facts.retention, now());
        return match blocking(move || store.list(&project, &session, retention, now)).await {
            Ok(files) => response(json!({"files":files}), 200),
            Err(e) => storage_error(e, false),
        };
    }
    let download = match boolean(query.get("download")) {
        Ok(v) => v,
        Err(()) => return response(json!({"detail":"valor booleano inválido"}), 422),
    };
    let width = match crate::thumbnail::parse_width(query.get("w")) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let (session, name) = (facts.session.clone(), filename.to_owned());
    let (path, file) = match blocking(move || store.open(&project, &session, &name)).await {
        Ok(v) => v,
        Err(e) => return storage_error(e, false),
    };
    let head = *req.method() == Method::HEAD;
    let mut response = crate::thumbnail::serve(
        tokio::fs::File::from_std(file),
        &path,
        req.headers(),
        download,
        Some(&facts.media),
        width,
    )
    .await;
    if head {
        *response.body_mut() = axum::body::Body::empty();
    }
    response
}

pub(super) fn boolean(value: Option<&String>) -> Result<bool, ()> {
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
    /// Corpo em partes acima do teto, com o mesmo `Limited` que a borda (`routes::edge_rules`) põe em todo
    /// pedido: 413 `erro_arquivo_grande`, nunca o 503 de disco. O corpo é gerado aos poucos, sem alocar 101 MiB.
    #[tokio::test]
    async fn chunked_upload_over_the_edge_cap_is_413() {
        let tmp = tempfile::tempdir().unwrap();
        let st = state();
        let chunks = futures_util::stream::iter((0..101).map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![0u8; 1 << 20]))));
        let body = Body::new(http_body_util::Limited::new(Body::from_stream(chunks), crate::http_body_limit::MAX_BYTES));
        let response = execute(&st, "upload", Request::new(body), facts(tmp.path()), "grande.bin", Default::default()).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["detail"]["code"], "erro_arquivo_grande", "{body}");
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
        let path = tmp
            .path()
            .join("uploads/outside.wav")
            .to_string_lossy()
            .into_owned();
        let facts = facts(tmp.path());
        let Ok((store, project)) = vault(&st, &facts) else { panic!("cofre de teste") };
        let status = |r: Result<PathBuf, Response>| r.unwrap_err().status();
        assert_eq!(status(resolve_audio(&store, &project, &facts, &path, false).await), StatusCode::FORBIDDEN);
        assert_eq!(status(resolve_audio(&store, &project, &facts, "../outside", false).await), StatusCode::BAD_REQUEST);
    }
}
