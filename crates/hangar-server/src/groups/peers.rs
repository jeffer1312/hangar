//! Outras máquinas do mesmo dono: `peers.json` (só leitura; quem grava é o Python) e a chamada
//! a elas com o token de dono, como o `peers.call` (`peers.py`).
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::transcript::py::py_str;

const READ_TIMEOUT: Duration = Duration::from_secs(8);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(16);
/// Leitura e prazo total do pedido à dona que espera outras máquinas antes de responder (`op: join`
/// espera até `federation::JOIN_WAIT`, `op: message` até `fed_notice::DELIVER_WAIT`): maior que as
/// duas, senão quem pede desiste antes do resultado e relata o contrário do que a dona fez.
pub const OWNER_WAIT_TIMEOUT: Duration = Duration::from_secs(20);
/// Leitura e prazo total da consulta longa do cofre (`/api/vault/requests/{id}/wait`): acima do que
/// a dona segura (`vault::wire::WAIT_HOLD`), senão quem espera desiste antes do `pending`.
pub const LONG_POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// Corpo com senha sem `Content-Length`: cobre a maior senha do cofre (16 KiB) com todo byte
/// escapado no JSON, para o vetor não crescer.
const SECRET_RESERVE: usize = 128 * 1024;
const MAX_BODY: usize = 1 << 20;
/// Texto do erro de sucesso acima de `MAX_BODY`; quem lê pode pedir menos.
pub const TOO_LARGE: &str = "resposta maior que 1 MiB";

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
    pub fn get(&self, server: &str) -> Option<PeerCfg> { cfg(self.load().get(server)?) }

    /// Todos os tokens do `peers.json`, para tirá-los do que volta ao modelo.
    pub fn tokens(&self) -> Vec<String> {
        self.load().values().filter_map(|v| v["token"].as_str()).filter(|t| !t.is_empty()).map(str::to_owned).collect()
    }

    /// Peers da varredura: com endereço e token, sem `enabled: false`.
    pub fn enabled_ids(&self) -> Vec<String> {
        let all = self.load();
        let mut ids: Vec<String> = all.iter()
            .filter(|(_, v)| v["enabled"] != Value::Bool(false)
                && v["base_url"].as_str().is_some_and(|s| !s.is_empty())
                && v["token"].as_str().is_some_and(|s| !s.is_empty()))
            .map(|(k, _)| k.clone())
            .collect();
        ids.sort();
        ids
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

fn cfg(entry: &Value) -> Option<PeerCfg> {
    let entry = entry.as_object()?;
    let field = |k: &str| entry.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
    Some(PeerCfg { base_url: field("base_url")?.trim_end_matches('/').to_owned(), token: field("token")?.to_owned() })
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
    /// A máquina não tem grupo entre máquinas (rota `/api/groups/*` inexistente ou sem `protocol`).
    OldVersion,
    /// Quem atendeu pela chave `key` do `peers.json` se diz `reported` (vazio = não disse).
    IdentityMismatch { key: String, reported: String },
}

impl PeerError {
    pub fn is_transport(&self) -> bool { matches!(self, PeerError::Transport(_)) }

    /// `str(PeerError)` do Python para a máquina `server`.
    pub fn text(&self, server: &str) -> String {
        match self {
            PeerError::Unknown => format!("servidor '{server}' não está em peers.json (ou sem base_url/token)"),
            PeerError::Refused { status, detail } => format!("{server} respondeu HTTP {status}: {}", py_str(detail)),
            PeerError::Transport(text) => text.clone(),
            PeerError::OldVersion => format!("{server} está numa versão do Hangar sem grupo entre máquinas"),
            PeerError::IdentityMismatch { key, reported } =>
                format!("a máquina '{key}' do peers.json responde como '{reported}': confira o CP_SERVER_ID dela"),
        }
    }
}

pub struct PeerClient {
    book: Arc<PeerBook>,
    http: OnceLock<reqwest::Client>,
    waiting: OnceLock<reqwest::Client>,
    /// `call_secret` e `call_long`: sem redirect.
    secret: OnceLock<reqwest::Client>,
    long: OnceLock<reqwest::Client>,
}

impl PeerClient {
    pub fn new(book: PeerBook) -> Self {
        Self { book: Arc::new(book), http: OnceLock::new(), waiting: OnceLock::new(), secret: OnceLock::new(), long: OnceLock::new() }
    }

    pub fn enabled_ids(&self) -> Vec<String> { self.book.enabled_ids() }

    /// Todo nome do `peers.json`, com ou sem endereço: apelido de par externo não pode repetir nenhum.
    pub fn ids(&self) -> std::collections::HashSet<String> { self.book.load().keys().cloned().collect() }

    /// Os tokens de peer conhecidos (leitura de arquivo, fora do executor).
    pub async fn tokens(&self) -> Vec<String> {
        let book = self.book.clone();
        tokio::task::spawn_blocking(move || book.tokens()).await.unwrap_or_default()
    }

    /// Endereço do peer no `peers.json` (o aparelho abre a sessão de lá por ele); nunca o token.
    pub fn base_url(&self, server: &str) -> Option<String> { self.book.get(server).map(|c| c.base_url) }

    /// Os ids com `base_url` e `token` (os que `base_url` acha), lidos uma vez e fora do executor.
    pub async fn addressable(&self) -> std::collections::HashSet<String> {
        let book = self.book.clone();
        tokio::task::spawn_blocking(move || book.load().iter().filter(|(_, v)| cfg(v).is_some()).map(|(k, _)| k.clone()).collect())
            .await.unwrap_or_default()
    }

    /// `peers.call` (peers.py:401-411): prazo de 8 s por leitura e 16 s no total, corpo até 1 MiB,
    /// segue redirect como o Python. O `reqwest` tira o `Authorization` quando o redirect troca de
    /// host, porta ou esquema (`redirect::remove_sensitive_headers`), e o `urllib` não tirava.
    pub async fn call(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Option<Value>, PeerError> {
        self.call_status(server, method, path, body).await.map(|(_, v)| v)
    }

    /// Como `call`, com o status 2xx exato.
    pub async fn call_status(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>)
        -> Result<(u16, Option<Value>), PeerError> {
        self.send(false, server, method, path, body).await
    }

    async fn send(&self, waiting: bool, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>)
        -> Result<(u16, Option<Value>), PeerError> {
        let book = self.book.clone();
        let key = server.to_owned();
        let cfg = tokio::task::spawn_blocking(move || book.get(&key)).await.ok().flatten().ok_or(PeerError::Unknown)?;
        let (slot, read, total) = if waiting { (&self.waiting, OWNER_WAIT_TIMEOUT, OWNER_WAIT_TIMEOUT) } else { (&self.http, READ_TIMEOUT, TOTAL_TIMEOUT) };
        let http = match slot.get() {
            Some(http) => http,
            None => {
                let built = crate::http_client().connect_timeout(READ_TIMEOUT).read_timeout(read).timeout(total).build()
                    .map_err(|e| transport(server, &e))?;
                slot.get_or_init(|| built)
            }
        };
        request(http, server, &format!("{}{path}", cfg.base_url), Some(&cfg.token), method, body).await
    }

    /// Como `call`, com o corpo 2xx cru em `Zeroizing`: a resposta pode levar senha e nunca vira
    /// `Value`, que não zera. Recusa (fora de 2xx) volta como no `call`.
    pub async fn call_secret(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>)
        -> Result<(u16, Zeroizing<Vec<u8>>), PeerError> {
        self.send_secret(false, server, method, path, body).await
    }

    /// `call_secret` da consulta longa do cofre: sem o `READ_TIMEOUT` de 8 s; leitura e total de
    /// `LONG_POLL_TIMEOUT`.
    pub async fn call_long(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>)
        -> Result<(u16, Zeroizing<Vec<u8>>), PeerError> {
        self.send_secret(true, server, method, path, body).await
    }

    async fn send_secret(&self, long: bool, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>)
        -> Result<(u16, Zeroizing<Vec<u8>>), PeerError> {
        let book = self.book.clone();
        let key = server.to_owned();
        let cfg = tokio::task::spawn_blocking(move || book.get(&key)).await.ok().flatten().ok_or(PeerError::Unknown)?;
        let (slot, read, total) = if long { (&self.long, LONG_POLL_TIMEOUT, LONG_POLL_TIMEOUT) } else { (&self.secret, READ_TIMEOUT, TOTAL_TIMEOUT) };
        let http = match slot.get() {
            Some(http) => http,
            None => {
                // Sem redirect: o 3xx volta como recusa, e nada com senha vai a outro endereço.
                let built = crate::http_client().redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(READ_TIMEOUT).read_timeout(read).timeout(total).build()
                    .map_err(|e| transport(server, &e))?;
                slot.get_or_init(|| built)
            }
        };
        let mut req = http.request(method, format!("{}{path}", cfg.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json").bearer_auth(&cfg.token);
        if let Some(body) = body { req = req.body(body.to_string()); }
        let mut resp = req.send().await.map_err(|e| transport(server, &e))?;
        let status = resp.status();
        // Capacidade reservada antes: o vetor que cresce deixa cópias do corpo sem zerar.
        let mut raw = Zeroizing::new(Vec::with_capacity(resp.content_length().map_or(SECRET_RESERVE, |n| (n as usize).min(MAX_BODY))));
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) if raw.len() + chunk.len() > MAX_BODY => {
                    if status.is_success() { return Err(PeerError::Transport(TOO_LARGE.into())); }
                    break;
                }
                Ok(Some(chunk)) => raw.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) if status.is_success() => return Err(transport(server, &e)),
                Err(_) => break,
            }
        }
        if !status.is_success() {
            let text = String::from_utf8_lossy(&raw);
            let detail = match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(mut map)) => map.remove("detail").unwrap_or_else(|| Value::String(text.clone().into_owned())),
                _ => Value::String(text.into_owned()),
            };
            return Err(PeerError::Refused { status: status.as_u16(), detail });
        }
        Ok((status.as_u16(), raw))
    }

    /// `call_group` de um pedido à dona que espera outras máquinas (`op: join`, `op: message`), com
    /// `OWNER_WAIT_TIMEOUT`.
    pub async fn call_group_waiting(&self, server: &str, path: &str, body: &Value) -> Result<Value, PeerError> {
        self.group(true, server, reqwest::Method::POST, path, Some(body)).await
    }

    /// Chamada a `/api/groups/*`: além do `call`, confere quem atendeu e se a máquina conhece
    /// grupos entre máquinas. Versão antiga só por sinal próprio: 404 sem `code` (a rota não
    /// existe lá) ou `GET /api/groups/{gid}` sem `protocol`; 404 com código é recusa comum.
    pub async fn call_group(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, PeerError> {
        self.group(false, server, method, path, body).await
    }

    async fn group(&self, waiting: bool, server: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, PeerError> {
        let group_get = method == reqwest::Method::GET
            && path.strip_prefix("/api/groups/").is_some_and(|rest| !rest.is_empty() && !rest.contains('/'));
        let answer = match self.send(waiting, server, method, path, body).await.map(|(_, v)| v) {
            Err(PeerError::Refused { status: 404, detail }) if path.starts_with("/api/groups/") && detail.get("code").is_none() => {
                return Err(PeerError::OldVersion);
            }
            other => other?.unwrap_or(Value::Null),
        };
        if group_get && answer.get("protocol").and_then(Value::as_u64).is_none_or(|p| p < u64::from(super::model::PROTOCOL)) {
            return Err(PeerError::OldVersion);
        }
        // Só a sondagem (o GET do grupo) exige o `server_id`; nas operações, ausente é aceito e
        // presente tem de bater (204 ou corpo vazio não dizem quem atendeu).
        match answer.get("server_id").and_then(Value::as_str) {
            Some(reported) if reported != server => Err(PeerError::IdentityMismatch { key: server.to_owned(), reported: reported.to_owned() }),
            None if group_get => Err(PeerError::IdentityMismatch { key: server.to_owned(), reported: String::new() }),
            _ => Ok(answer),
        }
    }
}

/// O cliente com os prazos do `peers.call`: 8 s por leitura e 16 s no total.
pub(crate) fn client(builder: reqwest::ClientBuilder) -> reqwest::Result<reqwest::Client> {
    builder.connect_timeout(READ_TIMEOUT).read_timeout(READ_TIMEOUT).timeout(TOTAL_TIMEOUT).build()
}

pub(crate) fn transport(label: &str, e: &dyn std::error::Error) -> PeerError {
    PeerError::Transport(format!("{label} inacessível: {}", chain(e)))
}

/// Pedido JSON a outra máquina, com o corpo até 1 MiB; `label` nomeia a máquina nos erros e o
/// token vazio não manda `Authorization`.
pub(crate) async fn request(http: &reqwest::Client, label: &str, url: &str, token: Option<&str>, method: reqwest::Method,
                            body: Option<&Value>) -> Result<(u16, Option<Value>), PeerError> {
    let mut req = http.request(method, url).header(reqwest::header::CONTENT_TYPE, "application/json");
    if let Some(token) = token.filter(|t| !t.is_empty()) { req = req.bearer_auth(token); }
    if let Some(body) = body { req = req.body(body.to_string()); }
    let mut resp = req.send().await.map_err(|e| transport(label, &e))?;
    let status = resp.status();
    let mut raw = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                raw.extend_from_slice(&chunk);
                if raw.len() > MAX_BODY {
                    // A recusa ainda se lê pelo primeiro 1 MiB; o sucesso grande é incerto.
                    if !status.is_success() { raw.truncate(MAX_BODY); break; }
                    return Err(PeerError::Transport(TOO_LARGE.into()));
                }
            }
            Ok(None) => break,
            Err(e) if status.is_success() => return Err(transport(label, &e)),
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
    if text.trim().is_empty() { return Ok((status.as_u16(), None)); }
    serde_json::from_str(&text).map(|v| (status.as_u16(), Some(v))).map_err(|e| PeerError::Transport(format!("{label} respondeu corpo ilegível: {e}")))
}

/// O erro e suas causas numa linha: o do `reqwest` sozinho não diz o que caiu.
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
