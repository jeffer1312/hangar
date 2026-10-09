//! Outras máquinas do mesmo dono: `peers.json` (só leitura; quem grava é o Python) e a chamada
//! a elas com o token de dono, como o `peers.call` (`peers.py`).
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::transcript::py::py_str;

const READ_TIMEOUT: Duration = Duration::from_secs(8);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(16);
const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerCfg { pub base_url: String, pub token: String }

type Version = (i128, u64);

/// `peers.json` lido uma vez por versão do arquivo (mtime, tamanho).
pub struct PeerBook { path: Option<PathBuf>, cache: Mutex<Option<(Version, Arc<Map<String, Value>>)>> }

impl PeerBook {
    /// Sem caminho, nenhuma máquina conhecida.
    pub fn new(path: Option<PathBuf>) -> Self { Self { path, cache: Mutex::default() } }

    /// `peer_cfg`: só entrada com `base_url` e `token` (a que só tem `app` é máquina da lista do app,
    /// não peer). `enabled: false` só tira da varredura; continua endereçável.
    pub fn get(&self, server: &str) -> Option<PeerCfg> {
        let all = self.load();
        let entry = all.get(server)?.as_object()?;
        let field = |k: &str| entry.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
        Some(PeerCfg { base_url: field("base_url")?.trim_end_matches('/').to_owned(), token: field("token")?.to_owned() })
    }

    fn load(&self) -> Arc<Map<String, Value>> {
        let Some(path) = &self.path else { return Arc::default() };
        let Some(version) = version(path) else { return Arc::default() };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((v, map)) = &*cache && *v == version { return map.clone(); }
        let map = match std::fs::read(path).ok().map(|raw| serde_json::from_slice::<Value>(&raw)) {
            Some(Ok(Value::Object(map))) => Arc::new(map),
            // Arquivo que existe e não lê é erro de configuração, não "sem máquinas": vai ao log.
            _ => {
                if crate::warn_limit::allow(None, "groups_peers_unreadable") {
                    tracing::warn!(code = "groups_peers_unreadable", "groups: peers.json ilegível ou torto, nenhuma máquina conhecida");
                }
                Arc::default()
            }
        };
        *cache = Some((version, map.clone()));
        map
    }
}

fn version(path: &std::path::Path) -> Option<Version> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0);
    Some((mtime, meta.len()))
}

#[derive(Debug)]
pub enum PeerError {
    /// A máquina não está no `peers.json` (ou sem `base_url`/`token`): recusa limpa.
    Unknown,
    /// Respondeu fora de 2xx: não gravou nada.
    Refused { status: u16, detail: Value },
    /// Rede, prazo ou corpo ilegível: o outro lado pode ter gravado. O texto já vem completo.
    Transport(String),
}

impl PeerError {
    pub fn is_transport(&self) -> bool { matches!(self, PeerError::Transport(_)) }

    /// `str(PeerError)` do Python para a máquina `server`.
    pub fn text(&self, server: &str) -> String {
        match self {
            PeerError::Unknown => format!("servidor '{server}' não está em peers.json (ou sem base_url/token)"),
            PeerError::Refused { status, detail } => format!("{server} respondeu HTTP {status}: {}", py_str(detail)),
            PeerError::Transport(text) => text.clone(),
        }
    }
}

pub struct PeerClient { book: Arc<PeerBook>, http: OnceLock<reqwest::Client> }

impl PeerClient {
    pub fn new(book: PeerBook) -> Self { Self { book: Arc::new(book), http: OnceLock::new() } }

    /// `peers.call` (peers.py:401-411): prazo de 8 s por leitura e 16 s no total, corpo até 1 MiB,
    /// segue redirect como o Python. O `reqwest` tira o `Authorization` quando o redirect troca de
    /// host, porta ou esquema (`redirect::remove_sensitive_headers`), e o `urllib` não tirava.
    pub async fn call(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Option<Value>, PeerError> {
        let book = self.book.clone();
        let key = server.to_owned();
        let cfg = tokio::task::spawn_blocking(move || book.get(&key)).await.ok().flatten().ok_or(PeerError::Unknown)?;
        let transport = |e: &dyn std::error::Error| PeerError::Transport(format!("{server} inacessível: {}", chain(e)));
        let http = match self.http.get() {
            Some(http) => http,
            None => {
                let built = reqwest::Client::builder().connect_timeout(READ_TIMEOUT).read_timeout(READ_TIMEOUT).timeout(TOTAL_TIMEOUT)
                    .build().map_err(|e| transport(&e))?;
                self.http.get_or_init(|| built)
            }
        };
        let mut req = http.request(method, format!("{}{path}", cfg.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json").bearer_auth(&cfg.token);
        if let Some(body) = body { req = req.body(body.to_string()); }
        let mut resp = req.send().await.map_err(|e| transport(&e))?;
        let status = resp.status();
        let mut raw = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    raw.extend_from_slice(&chunk);
                    if raw.len() > MAX_BODY {
                        // A recusa ainda se lê pelo primeiro 1 MiB; o sucesso grande é incerto.
                        if !status.is_success() { raw.truncate(MAX_BODY); break; }
                        return Err(PeerError::Transport("resposta maior que 1 MiB".into()));
                    }
                }
                Ok(None) => break,
                Err(e) if status.is_success() => return Err(transport(&e)),
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&raw);
        if !status.is_success() {
            let detail = match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(mut map)) => map.remove("detail").unwrap_or_else(|| Value::String(text.clone().into_owned())),
                _ => Value::String(text.into_owned()),
            };
            return Err(PeerError::Refused { status: status.as_u16(), detail });
        }
        if text.trim().is_empty() { return Ok(None); }
        serde_json::from_str(&text).map(Some).map_err(|e| PeerError::Transport(format!("{server} respondeu corpo ilegível: {e}")))
    }
}

/// O erro e suas causas numa linha: o do `reqwest` sozinho não diz o que caiu.
fn chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
