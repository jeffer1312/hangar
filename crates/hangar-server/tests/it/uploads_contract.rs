//! Cada pedido de `uploads_contract_cases` vai às rotas públicas reais, e status, cabeçalhos, corpo e
//! árvore do cofre são comparados depois de normalizar raiz (`<root>`) e chave de projeto (`<project>`,
//! `<other-project>`). Fora disso, o corpo JSON é comparado byte a byte; a tabela foi medida no Python,
//! e o que o Rust faz diferente são as exceções nomeadas abaixo.
use super::fake::{OWNER, spawn_fake};
use base64::Engine;
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    locate::{BoxFuture, InfoFailed, Locate},
    routes::{AppState, router},
    transcript::InternalInfo,
    uploads::store::{Naming, UploadStore, project_key},
};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "uploads_contract_cases.rs"]
pub mod cases;

const EPOCH: f64 = 1_700_000_000.0;
const SECRET: &str = "contrato";
const CHILD: &str = "HANGAR_UPLOAD_CONTRACT_CHILD";

/// A tabela do cenário, com o `mode` deste sistema.
fn table(text: &str) -> Value {
    let mut scenario: Value = serde_json::from_str(text).unwrap();
    if cfg!(windows) {
        for row in scenario["normalized_tree"].as_array_mut().unwrap() {
            row["mode"] = match row["mode"].as_u64() {
                Some(0o40755) => json!(0o40777),
                Some(0o100644) => json!(0o100666),
                other => json!(other),
            };
        }
    }
    scenario
}

#[derive(Default)]
struct Sessions(Mutex<HashMap<String, InternalInfo>>);
impl Locate for Sessions {
    fn info<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<InternalInfo>, InfoFailed>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(name).cloned()) })
    }
}

struct Response {
    status: u16,
    headers: Map<String, Value>,
    body: Vec<u8>,
}

struct Replay {
    _dir: tempfile::TempDir,
    root: PathBuf,
    vault: PathBuf,
    claude: PathBuf,
    sessions: Arc<Sessions>,
    projects: Vec<(String, &'static str)>,
    public: SocketAddr,
    names: Arc<AtomicU64>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Replay {
    fn drop(&mut self) {
        for task in &self._tasks {
            task.abort();
        }
    }
}

impl Replay {
    /// As sessões e pastas do `UploadReference.environment`: `fixture` grava em `transcript` (o id do
    /// transcript), `neighbor` no mesmo projeto, `other` noutro, `no-cwd` sem pasta.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // O cofre devolve caminhos sem o prefixo `\\?\` (o `plain_path` do store.rs).
        #[cfg(windows)]
        let root = PathBuf::from(root.to_str().unwrap().strip_prefix(r"\\?\").unwrap_or(root.to_str().unwrap()));
        let (project, other) = (root.join("project"), root.join("other-project"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let home = root.join("home");
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let sessions = Arc::new(Sessions::default());
        for (name, key, cwd) in [
            ("fixture", "transcript", Some(&project)),
            ("neighbor", "neighbor", Some(&project)),
            ("other", "other", Some(&other)),
            ("no-cwd", "", None),
        ] {
            sessions.0.lock().unwrap().insert(name.into(), info(&root, key, cwd));
        }
        let (_python, upstream) = spawn_fake().await;
        let mut state = AppState::new(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            auth_token: OWNER.into(),
            internal_secret: SECRET.into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.side.locate = sessions.clone();
        state.hangar_dir = root.join(".hangar");
        state.accounts = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
            ]
            .into(),
        ));
        // Relógio fixo e contador hexadecimal reiniciado por fixture, como o `token_hex` da captura.
        let counter = Arc::new(AtomicU64::new(0));
        let names = counter.clone();
        state.upload_naming = Naming {
            now: Arc::new(|| EPOCH),
            suffix: Arc::new(move || Ok(format!("{:06x}", counter.fetch_add(1, Ordering::SeqCst) + 1))),
        };
        let state = Arc::new(state);
        let (public, public_task) = serve(router(state.clone())).await;
        let projects = vec![(project_key(&project).unwrap(), "<project>"), (project_key(&other).unwrap(), "<other-project>")];
        let vault = root.join(".hangar").join("uploads");
        let replay = Self { _dir: dir, root, vault, claude, sessions, projects, public, names, _tasks: vec![public_task] };
        replay.retention(0);
        replay
    }

    fn retention(&self, days: i64) {
        std::fs::write(self.claude.join("runtime-config.json"), json!({"upload_retention_days": days}).to_string()).unwrap();
    }

    /// `normalize` da captura. O cofre do Rust é `<raiz>/.hangar/uploads`, o da captura era
    /// `<root>/vault`: só a localização difere, e ela vira o mesmo marcador.
    fn normalize(&self, value: &str) -> String {
        let mut value = value.to_owned();
        for (prefix, label) in [(&self.vault, "<root>/vault"), (&self.root, "<root>")] {
            if let Some(rest) = value.strip_prefix(prefix.to_str().unwrap()) {
                value = format!("{label}{}", rest.replace(std::path::MAIN_SEPARATOR, "/"));
                break;
            }
        }
        for (key, label) in &self.projects {
            value = value.replace(key.as_str(), label);
        }
        value
    }

    fn normalize_value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.normalize(&s)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.normalize_value(v)).collect()),
            Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, self.normalize_value(v))).collect()),
            other => other,
        }
    }

    /// `_normalize_json_body`: só os tokens de string que são valores mudam; o resto fica byte a byte.
    fn normalize_json_body(&self, body: &[u8]) -> Vec<u8> {
        let text = String::from_utf8(body.to_vec()).unwrap();
        let token = regex::Regex::new(r#""(?:[^"\\]|\\.)*""#).unwrap();
        let mut out = String::new();
        let mut cursor = 0;
        for found in token.find_iter(&text) {
            out.push_str(&text[cursor..found.start()]);
            cursor = found.end();
            let raw = found.as_str();
            if text[found.end()..].trim_start().starts_with(':') {
                out.push_str(raw);
                continue;
            }
            let value: String = serde_json::from_str(raw).unwrap();
            let normalized = self.normalize(&value);
            out.push_str(&if normalized == value { raw.to_owned() } else { serde_json::to_string(&normalized).unwrap() });
        }
        out.push_str(&text[cursor..]);
        out.into_bytes()
    }

    async fn send(&self, address: SocketAddr, method: &str, url: &str, headers: &Map<String, Value>, body: &[u8]) -> Response {
        let mut head = format!("{method} {url} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {OWNER}\r\naccept-encoding: identity\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {}\r\n", value.as_str().unwrap()));
        }
        // O cliente da captura manda o tamanho em todo POST; o gravado, se houver, vence.
        if method == "POST" && !headers.contains_key("content-length") {
            head.push_str(&format!("content-length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        let _ = stream.write_all(body).await;
        // Sem `connection: close`, que o servidor ecoaria: lê até o fim que a própria resposta anuncia.
        let mut raw = Vec::new();
        let mut chunk = [0; 64 * 1024];
        loop {
            let n = match tokio::time::timeout(Duration::from_secs(60), stream.read(&mut chunk)).await.expect("resposta sem fim") {
                Ok(n) => n,
                Err(error) => panic!("leitura da resposta falhou antes do fim: {error}"),
            };
            raw.extend_from_slice(&chunk[..n]);
            if n == 0 || complete(&raw) {
                return parse(&raw);
            }
        }
    }

    async fn upload(&self, session: &str, filename: &str, body: &[u8]) -> Response {
        let headers = Map::from_iter([("x-filename".to_owned(), json!(filename))]);
        self.send(self.public, "POST", &format!("/api/sessions/{session}/upload"), &headers, body).await
    }

    /// A vista de uma resposta HTTP como a captura grava: JSON normalizado e tamanho ajustado.
    fn record(&self, response: Response) -> (u16, Map<String, Value>, Vec<u8>, Value) {
        let Response { status, mut headers, body } = response;
        let is_json = headers.get("content-type").and_then(Value::as_str).is_some_and(|v| v.starts_with("application/json"));
        if !is_json {
            return (status, headers, body, Value::Null);
        }
        let parsed = self.normalize_value(serde_json::from_slice(&body).unwrap());
        let normalized = self.normalize_json_body(&body);
        if let Some(length) = headers.get("content-length").and_then(Value::as_str).map(|v| v.parse::<i64>().unwrap()) {
            headers.insert("content-length".into(), json!((length - body.len() as i64 + normalized.len() as i64).to_string()));
        }
        (status, headers, normalized, parsed)
    }

    fn snapshot_tree(&self) -> Vec<Value> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                out.push(path.clone());
                if path.is_dir() {
                    walk(&path, out);
                }
            }
        }
        let mut paths = Vec::new();
        if self.vault.exists() {
            walk(&self.vault, &mut paths);
        }
        paths.sort();
        paths
            .iter()
            .map(|path| {
                let meta = std::fs::metadata(path).unwrap();
                let mut item = Map::new();
                item.insert("path".into(), json!(self.normalize(path.to_str().unwrap())));
                item.insert("kind".into(), json!(if meta.is_dir() { "directory" } else { "file" }));
                item.insert("mtime".into(), if meta.is_dir() { json!("<directory-mtime>") } else { json!(seconds(&meta)) });
                item.insert("mode".into(), json!(mode(&meta)));
                if meta.is_file() {
                    item.insert("size".into(), json!(meta.len()));
                    item.insert("content_base64".into(), json!(base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).unwrap())));
                }
                Value::Object(item)
            })
            .collect()
    }
}

fn info(root: &Path, key: &str, cwd: Option<&PathBuf>) -> InternalInfo {
    let jsonl = root.join(format!("{}.jsonl", if key.is_empty() { "sem" } else { key }));
    serde_json::from_value(json!({"provider": "claude", "jsonl": jsonl, "cwd": cwd, "session_key": key, "lifecycle_id": "life", "history": {}})).unwrap()
}

async fn serve(router: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    (address, task)
}

fn complete(raw: &[u8]) -> bool {
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let body = &raw[split + 4..];
    if let Some(length) = head.lines().find_map(|l| l.strip_prefix("content-length:")) {
        return body.len() >= length.trim().parse::<usize>().unwrap();
    }
    if head.lines().any(|l| l.starts_with("transfer-encoding:") && l.contains("chunked")) {
        return body == b"0\r\n\r\n" || body.ends_with(b"\r\n0\r\n\r\n");
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).position(|w| w == needle).expect("resposta HTTP malformada")
}

fn parse(raw: &[u8]) -> Response {
    let split = find(raw, b"\r\n\r\n");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Map::new();
    for line in lines {
        let (name, value) = line.split_once(':').unwrap();
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_owned());
        // Repetidos se juntam com ", ", como no `dict(response.headers)` do httpx.
        let joined = match headers.get(&name) {
            Some(Value::String(old)) => format!("{old}, {value}"),
            _ => value,
        };
        headers.insert(name, json!(joined));
    }
    let mut body = raw[split + 4..].to_vec();
    if headers.get("transfer-encoding").and_then(Value::as_str) == Some("chunked") {
        let mut rest = &body[..];
        let mut out = Vec::new();
        loop {
            let end = find(rest, b"\r\n");
            let size = usize::from_str_radix(std::str::from_utf8(&rest[..end]).unwrap().split(';').next().unwrap().trim(), 16).unwrap();
            rest = &rest[end + 2..];
            if size == 0 {
                break;
            }
            out.extend_from_slice(&rest[..size]);
            rest = &rest[size + 2..];
        }
        body = out;
    }
    Response { status, headers, body }
}

fn seconds(meta: &std::fs::Metadata) -> f64 {
    match meta.modified().unwrap().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

#[cfg(unix)]
fn mode(meta: &std::fs::Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::mode(meta)
}

/// O `st_mode` que o Python inventa no Windows: pasta 0o40777, arquivo 0o100666 (0o100444 se só leitura).
#[cfg(windows)]
fn mode(meta: &std::fs::Metadata) -> u32 {
    if meta.is_dir() {
        0o40777
    } else if meta.permissions().readonly() {
        0o100444
    } else {
        0o100666
    }
}

fn set_mtime(path: &Path, seconds: f64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs_f64(seconds)))
        .unwrap();
}

fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.chars().take(300).collect()
}

fn decode(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(value.as_str().unwrap()).unwrap()
}

/// Exceção aceita, comportamento do Rust já no ar: erro em `{code, params, msg}` com o texto
/// acentuado do Rust (e o 413 também em JSON). O status continua sendo o da referência.
const RUST_ERRORS: &[(&str, usize, &str, &str)] = &[
    ("errors-and-isolation", 0, "erro_sessao_inexistente", "sessão não encontrada"),
    ("errors-and-isolation", 1, "erro_cwd_indisponivel", "cwd da sessão indisponível"),
    ("errors-and-isolation", 2, "erro_arquivo_vazio", "arquivo vazio ou caminho inválido"),
    ("errors-and-isolation", 3, "erro_arquivo_grande", "arquivo maior que 100 MiB"),
    ("errors-and-isolation", 7, "erro_upload_inexistente", "arquivo não encontrado"),
    ("errors-and-isolation", 8, "erro_upload_inexistente", "arquivo não encontrado"),
    ("errors-and-isolation", 9, "erro_sessao_inexistente", "sessão não encontrada"),
    ("errors-and-isolation", 10, "erro_upload_inexistente", "arquivo não encontrado"),
    ("errors-and-isolation", 11, "erro_arquivo_caminho", "caminho inválido"),
];

/// Exceção aceita, comportamento do Rust já no ar: o resolvedor de áudio responde com o código
/// do Rust no lugar do texto do Python; o status continua sendo o da referência.
const RUST_RESOLVER: &[(&str, &str)] = &[
    ("guest-absolute", "erro_arquivo_caminho_convidado"),
    ("other-project", "erro_arquivo_caminho"),
    ("network", "erro_arquivo_caminho"),
    ("nul", "erro_arquivo_caminho"),
    ("traversal", "erro_arquivo_caminho"),
    ("missing", "erro_upload_inexistente"),
];

/// Exceção aceita, Rust já no ar: nome de sessão hostil recusado no envio vira o erro do Rust.
/// Só o Windows grava esse caso: `create_dir` de `..-..` cria `..-` (o Win32 tira pontos finais),
/// `Directory::child` recusa com InvalidInput (store.rs, ramo `cfg(windows)`: `real_path(&path)? != path`)
/// e `storage_error(e, true)` responde 400 `erro_arquivo_vazio` (http.rs, `InvalidInput if upload`).
const RUST_SAVE_ERRORS: &[(&str, &str, &str)] = &[("../..", "erro_arquivo_vazio", "arquivo vazio ou caminho inválido")];

/// O mapeamento do ramo "resolve" de `uploads::http::execute`, que a transcrição por arquivo usa.
fn resolver_error(kind: std::io::ErrorKind) -> (u16, &'static str) {
    match kind {
        std::io::ErrorKind::PermissionDenied => (403, "erro_arquivo_caminho_convidado"),
        std::io::ErrorKind::FileTooLarge => (413, "erro_arquivo_grande"),
        std::io::ErrorKind::NotFound => (404, "erro_upload_inexistente"),
        std::io::ErrorKind::InvalidInput => (400, "erro_arquivo_caminho"),
        _ => (503, "upload_storage_unavailable"),
    }
}

fn quoted(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|v| v.len() > 2 && v.starts_with('"') && v.ends_with('"'))
}

/// Repete o cenário inteiro e devolve as divergências; as diferenças de MIME aceitas vão ao stderr.
async fn replay(name: &str, text: &str) -> Vec<String> {
    let golden = table(text);
    let records = golden["normalized_response"].as_array().unwrap();
    let total = match name {
        "active-content-range" => 20,
        "errors-and-isolation" | "retention-and-audio" => 16,
        "video-without-ffmpeg" => 2,
        other => panic!("cenário sem total: {other}"),
    };
    assert_eq!(records.len(), total, "{name}");
    let r = Replay::new().await;
    let mut diffs = Vec::new();
    let mut uploads: Vec<PathBuf> = Vec::new();
    let mut hostile = 0;
    for (index, expected) in records.iter().enumerate() {
        let at = |field: &str| format!("{name}#{index} {field}");
        // Passos da captura que não viram registro, presos ao pedido gravado logo depois deles.
        if name == "retention-and-audio" && index == 11 {
            assert_eq!((&expected["method"], &expected["url"]), (&json!("GET"), &json!("/api/sessions/neighbor/uploads")), "a fixture mudou de forma");
            r.retention(7);
            set_mtime(&uploads[1], EPOCH - 8.0 * 86400.0);
            set_mtime(&uploads[2], EPOCH - 8.0 * 86400.0);
        }
        if name == "retention-and-audio" && index == 15 {
            assert_eq!((&expected["method"], &expected["url"]), (&json!("GET"), &json!("/api/sessions/fixture/uploads")), "a fixture mudou de forma");
            r.retention(0);
        }
        match expected["kind"].as_str() {
            Some("http") => {
                let method = expected["method"].as_str().unwrap();
                let url = expected["url"].as_str().unwrap();
                let request = &expected["request"];
                let response = r.send(r.public, method, url, request["headers"].as_object().unwrap(), &decode(&request["body_base64"])).await;
                let (status, mut headers, body, parsed) = r.record(response);
                // O mtime é entrada explícita da captura, aplicada logo após cada envio aceito.
                if method == "POST" && url.contains("/upload") && status == 200 {
                    let path = PathBuf::from(raw_path(&r, &parsed));
                    set_mtime(&path, EPOCH);
                    uploads.push(path);
                }
                if status != expected["status"] {
                    diffs.push(format!("{} esperado={} recebido={status}", at("status"), expected["status"]));
                }
                // Cabeçalho de transporte do hyper; o cliente em processo da captura não tinha servidor.
                headers.remove("date");
                if let Some(&(_, _, code, msg)) = RUST_ERRORS.iter().find(|(f, i, _, _)| *f == name && *i == index) {
                    let rust_body = json!({"detail": {"code": code, "params": {}, "msg": msg}});
                    let rust_headers = Map::from_iter([
                        ("content-length".to_owned(), json!(rust_body.to_string().len().to_string())),
                        ("content-type".to_owned(), json!("application/json")),
                    ]);
                    if headers != rust_headers {
                        diffs.push(format!("{} esperado={rust_headers:?} rust={headers:?}", at("headers(erro Rust)")));
                    }
                    if body != rust_body.to_string().into_bytes() || parsed != rust_body {
                        diffs.push(format!("{} esperado={rust_body} rust={:?}", at("body(erro Rust)"), excerpt(&body)));
                    }
                    continue;
                }
                let want = expected["headers"].as_object().unwrap();
                let download = url.contains("/uploads/") && matches!(status, 200 | 206);
                let names: std::collections::BTreeSet<&String> = want.keys().chain(headers.keys()).collect();
                for header in names {
                    let (wanted, got) = (want.get(header), headers.get(header));
                    if wanted == got {
                        continue;
                    }
                    // Exceção aceita: o MIME do download vem do `mime_guess`, não da tabela.
                    if download && header == "content-type" && wanted.is_some() && got.is_some() {
                        eprintln!("MIME aceito {}: esperado={} recebido={}", at("headers.content-type"), wanted.unwrap(), got.unwrap());
                        continue;
                    }
                    // Exceção aceita: sem compressão, o Rust não manda `vary`.
                    if header == "vary" && wanted == Some(&json!("Accept-Encoding")) && got.is_none() {
                        continue;
                    }
                    // Exceção aceita: ETag em formato próprio, presente e entre aspas.
                    if download && header == "etag" && quoted(got) {
                        continue;
                    }
                    // Exceção aceita: o download leva `cache-control: max-age=60`.
                    if download && header == "cache-control" && wanted.is_none() && got == Some(&json!("max-age=60")) {
                        continue;
                    }
                    // Exceção aceita: a página que embrulha o HTML sai em chunked, sem tamanho dos dois lados.
                    if download
                        && header == "transfer-encoding"
                        && wanted.is_none()
                        && got == Some(&json!("chunked"))
                        && !want.contains_key("content-length")
                        && !headers.contains_key("content-length")
                    {
                        continue;
                    }
                    diffs.push(format!("{} esperado={wanted:?} recebido={got:?}", at(&format!("headers.{header}"))));
                }
                let want_body = decode(&expected["body_base64"]);
                if body != want_body {
                    diffs.push(format!("{} esperado={:?} recebido={:?}", at("body"), excerpt(&want_body), excerpt(&body)));
                }
                if parsed != expected["json"] {
                    diffs.push(format!("{} esperado={} recebido={parsed}", at("json"), expected["json"]));
                }
            }
            Some("platform-save") => {
                let session = expected["session"].as_str().unwrap();
                hostile += 1;
                let alias = format!("hostile-{hostile}");
                let project = r.root.join("project");
                r.sessions.0.lock().unwrap().insert(alias.clone(), info(&r.root, session, Some(&project)));
                let (status, _, _, parsed) = r.record(r.upload(&alias, "x.png", b"x").await);
                let result = if status == 200 {
                    let path = PathBuf::from(raw_path(&r, &parsed));
                    set_mtime(&path, EPOCH);
                    json!({"status": 200, "path": parsed["path"]})
                } else if let Some(&(_, code, msg)) = RUST_SAVE_ERRORS.iter().find(|(s, _, _)| *s == session) {
                    // Na tabela, o contador foi gasto antes da recusa.
                    r.names.fetch_add(1, Ordering::SeqCst);
                    let rust_body = json!({"detail": {"code": code, "params": {}, "msg": msg}});
                    if parsed != rust_body {
                        diffs.push(format!("{} esperado={rust_body} rust={parsed}", at(&format!("platform-save[{session:?}](erro Rust)"))));
                    }
                    json!({"status": status, "detail": expected["result"]["detail"]})
                } else {
                    json!({"status": status, "detail": detail(&parsed)})
                };
                if result != expected["result"] {
                    diffs.push(format!("{} esperado={} recebido={result}", at(&format!("platform-save[{session:?}]")), expected["result"]));
                }
            }
            Some("audio-resolver") => {
                let case = expected["case"].as_str().unwrap();
                let path = |i: usize| uploads[i].to_str().unwrap().to_owned();
                let (reference, owner) = match case {
                    "own-name" => (uploads[0].file_name().unwrap().to_str().unwrap().to_owned(), true),
                    "previous-transcript" => (path(1), true),
                    "guest-absolute" => (path(0), false),
                    "other-project" => (path(2), true),
                    "network" => ("//outside.invalid/share/audio.webm".to_owned(), true),
                    "nul" => ("bad\0.webm".to_owned(), true),
                    "traversal" => ("../audio.webm".to_owned(), true),
                    "missing" => ("missing.webm".to_owned(), true),
                    other => panic!("caso do resolvedor de áudio sem repetição no Rust: {other}"),
                };
                // Como o `resolve_session_audio(cwd, "transcript", …)` da captura: projeto e sessão de `fixture`.
                let store = UploadStore::new(&r.vault).unwrap();
                let project = project_key(&r.root.join("project")).unwrap();
                let result = match store.resolve_audio(&project, "transcript", &reference, owner) {
                    Ok(path) => json!({"status": 200, "path": r.normalize(path.to_str().unwrap())}),
                    Err(error) => {
                        let (status, rust_code) = resolver_error(error.kind());
                        match RUST_RESOLVER.iter().find(|(c, _)| *c == case) {
                            // Exceção aceita, Rust já no ar: status da referência e código do Rust.
                            Some(&(_, code)) => {
                                if rust_code != code {
                                    diffs.push(format!("{} esperado={code} rust={rust_code}", at(&format!("audio-resolver[{case}].code"))));
                                }
                                json!({"status": status, "detail": expected["result"]["detail"]})
                            }
                            None => json!({"status": status, "code": rust_code}),
                        }
                    }
                };
                if result != expected["result"] {
                    diffs.push(format!("{} esperado={} recebido={result}", at(&format!("audio-resolver[{case}]")), expected["result"]));
                }
            }
            other => diffs.push(format!("{} registro de tipo {other:?} não foi repetido", at("kind"))),
        }
    }
    let tree = r.snapshot_tree();
    let want = golden["normalized_tree"].as_array().unwrap();
    let paths = |rows: &[Value]| rows.iter().map(|row| row["path"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    if paths(&tree) != paths(want) {
        diffs.push(format!("{name} tree.paths esperado={:?} recebido={:?}", paths(want), paths(&tree)));
    }
    for row in want {
        if let Some(actual) = tree.iter().find(|t| t["path"] == row["path"])
            && actual != row
        {
            diffs.push(format!("{name} tree[{}] esperado={row} recebido={actual}", row["path"]));
        }
    }
    diffs
}

/// Desfaz a normalização de `<root>/vault` e `<project>` para achar o arquivo no disco, com o
/// separador nativo (no Windows o caminho volta a ser referência do resolvedor de áudio).
fn raw_path(r: &Replay, parsed: &Value) -> String {
    let rest = parsed["path"].as_str().unwrap().strip_prefix("<root>/vault/").expect("envio fora do cofre");
    let mut path = r.vault.clone();
    for part in rest.split('/') {
        let mut part = part.to_owned();
        for (key, label) in &r.projects {
            part = part.replace(label, key);
        }
        path.push(part);
    }
    path.to_str().unwrap().to_owned()
}

/// O texto do erro: a captura grava `detail` como frase; o Rust responde `{code, params, msg}`.
fn detail(parsed: &Value) -> Value {
    match &parsed["detail"] {
        Value::Object(detail) => detail.get("msg").cloned().unwrap_or(Value::Null),
        other => other.clone(),
    }
}

#[track_caller]
fn assert_same(name: &str, diffs: Vec<String>) {
    assert!(diffs.is_empty(), "{name}: {} divergência(s) da tabela:\n{}", diffs.len(), diffs.join("\n"));
}

#[tokio::test]
#[cfg_attr(target_os = "macos", ignore = "a tabela só tem Linux e Windows")]
async fn errors_and_isolation_sequence() {
    assert_same("errors-and-isolation", replay("errors-and-isolation", cases::ERRORS_AND_ISOLATION).await);
}

#[tokio::test]
#[cfg_attr(target_os = "macos", ignore = "a tabela só tem Linux e Windows")]
async fn retention_and_audio_sequence() {
    assert_same("retention-and-audio", replay("retention-and-audio", cases::RETENTION_AND_AUDIO).await);
}

#[tokio::test]
#[cfg_attr(target_os = "macos", ignore = "a tabela só tem Linux e Windows")]
async fn active_content_range_sequence() {
    assert_same("active-content-range", replay("active-content-range", cases::ACTIVE_CONTENT_RANGE).await);
}

/// Sem decodificador de verdade: o próprio executável roda de novo, só este teste, com o PATH vazio.
#[tokio::test]
#[cfg_attr(target_os = "macos", ignore = "a tabela só tem Linux e Windows")]
async fn video_without_ffmpeg_sequence() {
    if std::env::var_os(CHILD).is_some() {
        let missing = std::process::Command::new("ffmpeg").arg("-version").output();
        assert!(missing.is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound), "o ffmpeg ainda está ao alcance; o cenário sem decodificador não rodaria");
        return assert_same("video-without-ffmpeg", replay("video-without-ffmpeg", cases::VIDEO_WITHOUT_FFMPEG).await);
    }
    let empty = tempfile::tempdir().unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &crate::exact_name(module_path!(), "video_without_ffmpeg_sequence"), "--nocapture"])
        .env("PATH", empty.path())
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    crate::assert_ran_one(&result.stdout);
}
