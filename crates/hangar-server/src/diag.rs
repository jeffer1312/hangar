//! O diário mora no `hangar-core` (linhas do servidor) e no `hangar-usage-diary` (linhas da tela,
//! resumo e download); aqui ficam as rotas `/api/diag` e `/api/diag/arquivo`, que leem o estado do servidor.
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};

pub use hangar_core::diag::*;

use crate::routes::AppState;

const ROUTES: &[(&str, &str)] = &[("/api/diag", "GET"), ("/api/diag", "POST"), ("/api/diag/arquivo", "GET")];
const SERVER_OS: &str = if cfg!(windows) { "nt/win32" } else if cfg!(target_os = "macos") { "posix/darwin" } else { "posix/linux" };
static MUX_VERSION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(?:psmux|tmux|pmux) [0-9][0-9A-Za-z.+_-]*$").unwrap());

/// O diário da tela é do Rust: nenhum destes caminhos vai ao Python.
pub fn owns_path(path: &str) -> bool {
    matches!(crate::workspace_http::decoded(path).as_ref(), "/api/diag" | "/api/diag/" | "/api/diag/arquivo" | "/api/diag/arquivo/")
}

pub async fn route(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(req.method(), req.headers()) {
        return response;
    }
    let (method, headers) = (req.method().clone(), req.headers().clone());
    let mut response = reply(&st, peer, req).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    hangar_core::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn reply(st: &AppState, peer: SocketAddr, req: Request) -> Response {
    if let Some(response) = hangar_core::http_body_limit::precheck(req.headers()) {
        return response;
    }
    let path = crate::workspace_http::decoded(req.uri().path()).into_owned();
    let (_, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let decision = hangar_core::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    if let Some(response) = hangar_core::http_route_response::response(
        &decision, req.method(), req.uri(), req.headers(), https, &st.cfg.listen.to_string(),
    ) {
        return response;
    }
    // Convidado que um porteiro deixou chegar aqui grava como na rota antiga (o login só alcança o POST).
    let guest = matches!(crate::share::caller::caller(&req), Some(crate::share::caller::Caller::Guest(_)));
    if !guest && let Err(response) = crate::routes::owner_gate(st, peer, &req).await {
        return response;
    }
    let base = diary_base(st);
    if req.method() == Method::POST {
        return record(base, req).await;
    }
    // O pedido não atravessa a espera: `&Request` tiraria o `Send` do handler.
    let (headers, query) = (req.headers().clone(), req.uri().query().map(str::to_owned));
    drop(req);
    if path == "/api/diag" {
        summary(base, query.as_deref(), &headers).await
    } else {
        download(st, base, &headers).await
    }
}

/// `diario/` nos logs do Hangar da conta-base. Prova com o ambiente nos logs reais não grava.
fn diary_base(st: &AppState) -> Option<PathBuf> {
    let base = crate::runtime::local_policy::log_base_in(&st.accounts.env)?;
    if hangar_core::process::cargo_test_binary() && hangar_core::paths::log_base().is_some_and(|real| real == base) {
        return None;
    }
    Some(base.join("diario"))
}

async fn record(base: Option<PathBuf>, req: Request) -> Response {
    let body = match hangar_core::http_body_limit::read(req.into_body()).await {
        Ok(body) => body,
        Err(error) if hangar_core::http_body_limit::is_limit(&error) => return hangar_core::http_body_limit::rejected(),
        // Cliente que desistiu no meio do corpo: como corpo ilegível, nunca erro.
        Err(_) => return hangar_core::http_cors::json_guarded(StatusCode::OK, json!({"gravadas": 0})),
    };
    let now = chrono::Local::now().fixed_offset();
    let written = tokio::task::spawn_blocking(move || match base {
        Some(base) => hangar_usage_diary::record_screen(&base, &body, now),
        None => Ok(0),
    }).await;
    match written {
        Ok(Ok(count)) => hangar_core::http_cors::json_guarded(StatusCode::OK, json!({"gravadas": count})),
        _ => hangar_core::http_cors::internal_error(),
    }
}

async fn summary(base: Option<PathBuf>, query: Option<&str>, headers: &HeaderMap) -> Response {
    let count = match hangar_usage_diary::latest_count(query) {
        Ok(count) => count,
        Err(input) => return hangar_core::http::json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{
            "type": "int_parsing", "loc": ["query", "ultimas"],
            "msg": "Input should be a valid integer, unable to parse string as an integer", "input": input}]})),
    };
    // Pasta vazia não existe: o resumo sai zerado.
    let base = base.unwrap_or_default();
    let value = tokio::task::spawn_blocking(move || {
        let mut value = hangar_usage_diary::summary(&base);
        value["ultimas"] = Value::Array(hangar_usage_diary::latest(&base, count));
        value
    }).await;
    let Ok(value) = value else { return hangar_core::http_cors::internal_error() };
    gzipped(headers, hangar_core::http_cors::json_guarded(StatusCode::OK, value)).await
}

/// O diário sai em fluxo: lido arquivo a arquivo e comprimido enquanto sai, sem montar a semana em memória.
async fn download(st: &AppState, base: Option<PathBuf>, headers: &HeaderMap) -> Response {
    let header = header_line(st).await;
    let base = base.unwrap_or_default();
    let wants_gzip = headers.get(header::ACCEPT_ENCODING).and_then(|value| value.to_str().ok()).is_some_and(|value| value.contains("gzip"));
    let (decided, decision) = tokio::sync::oneshot::channel();
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    tokio::task::spawn_blocking(move || {
        let mut sink = DownloadSink { start: Vec::new(), decided: Some(decided), wants_gzip, encoder: None, sender };
        let result = hangar_usage_diary::download_to(&base, &header, &mut |part| sink.write(part));
        // Cliente que desistiu: nada mais a entregar.
        if result.is_ok() {
            let _ = sink.finish();
        }
    });
    let Ok(gzip) = decision.await else { return hangar_core::http_cors::internal_error() };
    let body = Body::from_stream(futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|part: bytes::Bytes| (Ok::<_, std::io::Error>(part), receiver))
    }));
    let mut response = Response::builder().status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"hangar-uso.jsonl\"");
    if gzip {
        response = response.header(header::CONTENT_ENCODING, "gzip").header(header::VARY, "Accept-Encoding");
    }
    response.body(body).expect("cabeçalhos estáticos válidos")
}

/// Segura o primeiro 1 KB para decidir o gzip como a rota antiga (só com 1 KB ou mais), e depois
/// entrega cada pedaço, comprimido se for o caso. A compressão sem descarga intermediária dá os
/// mesmos bytes de comprimir o corpo inteiro de uma vez.
struct DownloadSink {
    start: Vec<u8>,
    decided: Option<tokio::sync::oneshot::Sender<bool>>,
    wants_gzip: bool,
    encoder: Option<flate2::write::GzEncoder<Vec<u8>>>,
    sender: tokio::sync::mpsc::Sender<bytes::Bytes>,
}

impl DownloadSink {
    const GZIP_MINIMUM: usize = 1024;

    fn write(&mut self, part: &[u8]) -> std::io::Result<()> {
        if self.decided.is_none() {
            return self.emit(part);
        }
        self.start.extend_from_slice(part);
        if self.start.len() < Self::GZIP_MINIMUM { Ok(()) } else { self.decide() }
    }

    fn decide(&mut self) -> std::io::Result<()> {
        let gzip = self.wants_gzip && self.start.len() >= Self::GZIP_MINIMUM;
        if let Some(decided) = self.decided.take() {
            decided.send(gzip).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        }
        if gzip {
            self.encoder = Some(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5)));
        }
        let start = std::mem::take(&mut self.start);
        self.emit(&start)
    }

    fn emit(&mut self, part: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let out = match &mut self.encoder {
            Some(encoder) => {
                encoder.write_all(part)?;
                std::mem::take(encoder.get_mut())
            }
            None => part.to_vec(),
        };
        self.send(out)
    }

    fn send(&self, out: Vec<u8>) -> std::io::Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        self.sender.blocking_send(bytes::Bytes::from(out)).map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }

    fn finish(mut self) -> std::io::Result<()> {
        if self.decided.is_some() {
            self.decide()?;
        }
        match self.encoder.take() {
            Some(encoder) => self.send(encoder.finish()?),
            None => Ok(()),
        }
    }
}

/// O gzip da rota antiga: só com `Accept-Encoding: gzip` e corpo de 1 KB ou mais.
async fn gzipped(headers: &HeaderMap, response: Response) -> Response {
    if !response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else { return hangar_core::http_cors::internal_error() };
    let bytes = hangar_core::http::maybe_gzip(headers, &mut parts.headers, bytes.to_vec());
    parts.headers.insert(header::CONTENT_LENGTH, bytes.len().into());
    Response::from_parts(parts, Body::from(bytes))
}

/// Primeira linha do download: o que o repositório não pode contar sobre a máquina.
async fn header_line(st: &AppState) -> String {
    let now = chrono::Local::now();
    let mux = mux_version(&st.list.env().capture_program).await;
    let root = crate::install_root::root(&st.accounts.env.home).unwrap_or_else(|| st.accounts.env.home.join(".hangar/current"));
    // Logo depois de subir o cache ainda está frio: espera a versão em vez de mandar "indisponivel".
    let backend = match crate::boot_version::ensure(root, st.accounts.env.base.clone()).await {
        Ok(version) => version.describe,
        Err(_) => "indisponivel".to_owned(),
    };
    let started = crate::terminal_facts::agent_birth(std::process::id())
        .and_then(|at| chrono::DateTime::from_timestamp(at.floor() as i64, (at.fract() * 1e9) as u32))
        .map_or(now, |at| at.with_timezone(&chrono::Local));
    let (system, machine) = platform();
    let value = json!({
        "evento": "diag.formato",
        "ts": iso(now),
        "so_servidor": SERVER_OS,
        "formato": 2,
        "sistema": system,
        "arquitetura": machine,
        "cpus": std::thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        "mux": mux,
        "mux_fonte": "tmux -V no momento do download (versão do CLI)",
        "inicio_backend": iso(started),
        "pid_backend": std::process::id(),
        "recursos_no_download": resources(),
        "dias_guardados": hangar_usage_diary::DAYS_KEPT,
        "ordem": "por dia; use ts e seq para ordenar eventos de origens distintas",
        "campos_documentados_em": "crates/hangar-usage-diary/src/lib.rs (FIELDS) e frontend/src/lib/diag.ts",
        "nao_contem": "conversa, prompt, resposta do agente, chave de API, conteúdo de arquivo",
        "backend": backend,
    });
    hangar_core::pycompat::pyjson::dumps_unicode(&value, false)
}

fn iso(at: chrono::DateTime<chrono::Local>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

/// `tmux -V` (o psmux responde ao mesmo), saneado: versão reconhecida, `retorno:N` ou o tipo da falha.
async fn mux_version(program: &std::ffi::OsStr) -> String {
    let mut command = crate::terminal_input::child_command(program);
    command.arg("-V").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(2), command.output()).await {
        Err(_) => "TimeoutExpired".into(),
        Ok(Err(error)) => python_error(&error).into(),
        Ok(Ok(output)) => match exit_code(&output.status) {
            0 => {
                let version = hangar_core::pycompat::py::strip(&String::from_utf8_lossy(&output.stdout)).to_owned();
                if MUX_VERSION.is_match(&version) { version } else { "resposta_nao_reconhecida".into() }
            }
            code => format!("retorno:{code}"),
        },
    }
}

/// O `returncode` do Python: sinal sai negativo.
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(status);
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    signal.map_or_else(|| status.code().unwrap_or(-1), |signal| -signal)
}

/// O nome da exceção que o diário já mostrava para cada falha de sistema.
fn python_error(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

/// `platform.system() release() version()` e `platform.machine()`.
#[cfg(unix)]
fn platform() -> (String, String) {
    // SAFETY: `uname` só preenche a estrutura recebida; cada campo é uma string C terminada em zero.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return (String::new(), String::new());
    }
    // SAFETY: depois do `uname` bem-sucedido cada campo é uma string C terminada em zero dentro do
    // próprio arranjo, que vive até o fim desta função.
    let text = |field: &[libc::c_char]| unsafe { std::ffi::CStr::from_ptr(field.as_ptr()) }.to_string_lossy().into_owned();
    (format!("{} {} {}", text(&name.sysname), text(&name.release), text(&name.version)), text(&name.machine))
}

// shortcut: no Windows a versão vem do sysinfo ("Windows 11 26100"), não do formato exato do Python
// ("Windows 11 10.0.26100"); trocar se alguém filtrar o diário por esse texto.
#[cfg(windows)]
fn platform() -> (String, String) {
    let release = sysinfo::System::os_version().unwrap_or_default();
    let release = release.split_whitespace().next().unwrap_or_default().to_owned();
    let build = sysinfo::System::kernel_version().unwrap_or_default();
    (format!("Windows {release} {build}"), std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default())
}

/// Subida sem o Python: a queda da vida anterior (marca de pé que ficou) e o `backend.inicio`, os
/// nomes que o relatório já colhe. Bloqueante; roda antes do log desta vida.
pub fn record_boot(hangar_dir: &std::path::Path, log: Option<&std::path::Path>) {
    let pid = std::process::id();
    let mut lines = Vec::new();
    match hangar_boot::life::begin(hangar_dir, &iso(chrono::Local::now())) {
        Ok(begin) => {
            if begin.reset {
                lines.push(json!({"evento": "hangar_server.vida_ilegivel", "nivel": "aviso", "codigo": "server_life_reset"}));
            }
            if begin.unclean {
                let saida = log.map(|log| hangar_boot::life::last_errors(log, 3)).unwrap_or_default();
                lines.push(json!({"evento": "hangar_server.caiu", "nivel": "aviso", "saidas_nao_limpas": begin.unclean_exits, "saida": saida}));
            }
        }
        Err(error) => lines.push(json!({"evento": "hangar_server.vida_ilegivel", "nivel": "aviso",
            "codigo": "server_life_unwritable", "tipo": format!("{:?}", error.kind())})),
    }
    let mut start = json!({"evento": "backend.inicio", "nivel": "ok", "pid": pid});
    if let (Some(start), Value::Object(found)) = (start.as_object_mut(), resources()) { start.extend(found); }
    lines.push(start);
    record_blocking(diary_dir().as_deref(), &lines, "subida");
}

/// Pronto para atender: a marca de pé desta vida. Sem ela gravada, a queda desta vida não é contada.
pub fn record_running(hangar_dir: &std::path::Path) {
    if let Err(error) = hangar_boot::life::mark_running(hangar_dir, std::process::id(), &iso(chrono::Local::now())) {
        record_blocking(diary_dir().as_deref(), &[json!({"evento": "hangar_server.vida_ilegivel", "nivel": "aviso",
            "codigo": "server_life_unwritable", "tipo": format!("{:?}", error.kind())})], "subida");
    }
}

/// Saída deliberada (sinal, falha do servidor): a marca de pé desta vida sai, e a próxima subida
/// não conta queda. `nivel` erro quando a saída foi por falha.
pub fn record_stop(hangar_dir: &std::path::Path, cause: &str, failed: bool) {
    let mut line = json!({"evento": "backend.encerrando", "nivel": if failed { "erro" } else { "ok" }, "causa": cause});
    match hangar_boot::life::end(hangar_dir, std::process::id()) {
        Ok(hangar_boot::life::Ended::Reset) => line["codigo"] = json!("server_life_reset"),
        Ok(_) => {},
        Err(error) => {
            line["codigo"] = json!("server_life_unwritable");
            line["tipo"] = json!(format!("{:?}", error.kind()));
        }
    }
    record_blocking(diary_dir().as_deref(), &[line], "parada");
}

/// Foto barata no download; não espera amostra de CPU.
fn resources() -> Value {
    let mut out = serde_json::Map::new();
    if let Some(threads) = threads() {
        out.insert("threads_backend".into(), json!(threads));
    }
    match memory() {
        Ok((total, available)) => {
            out.insert("memoria_total_mb".into(), json!(total));
            out.insert("memoria_disponivel_mb".into(), json!(available));
        }
        Err(kind) => {
            out.insert("recursos_erro".into(), json!(kind));
        }
    }
    Value::Object(out)
}

#[cfg(target_os = "linux")]
fn threads() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status").ok()?.lines()
        .find_map(|line| line.strip_prefix("Threads:")?.trim().parse().ok())
}

#[cfg(not(target_os = "linux"))]
fn threads() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn memory() -> Result<(u64, u64), &'static str> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|error| python_error(&error))?;
    let kb = |key: &str| text.lines().find_map(|line| line.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok());
    match (kb("MemTotal:"), kb("MemAvailable:")) {
        (Some(total), Some(available)) => Ok((total / 1024, available / 1024)),
        _ => Err("KeyError"),
    }
}

#[cfg(not(target_os = "linux"))]
fn memory() -> Result<(u64, u64), &'static str> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    Ok((system.total_memory() / (1024 * 1024), system.available_memory() / (1024 * 1024)))
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;
    use std::path::Path;
    pub fn rows(dir: &Path) -> Vec<Value> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .flat_map(|path| {
                let raw = std::fs::read_to_string(path).unwrap();
                let complete = raw.rsplit_once('\n').map_or("", |(complete, _)| complete);
                complete
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    pub async fn wait_for(dir: &Path, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        use notify::Watcher;
        let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let _ = tx.send(event);
            })
            .unwrap();
        watcher
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let found = rows(dir);
            if found.iter().any(&predicate) {
                return found;
            }
            tokio::time::timeout_at(deadline, changes.recv())
                .await
                .expect("o evento não chegou ao diário local")
                .expect("a observação encerrou")
                .expect("a observação falhou");
        }
    }
}
