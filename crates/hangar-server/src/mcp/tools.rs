//! As 21 tools. Cada uma chama a rota pública que o `hangar-send`/`hangar-preview` chama, pelo
//! roteador deste processo e como o dono; o `html_render` publica direto na loja de páginas e os
//! verbos do navegador vão ao app desktop, como no CLI.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request};
use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tower_service::Service;

use super::identity::{self, Local, Origin};
use super::nav;
use crate::groups::exit::segment;
use crate::groups::model::Sidecar;
use crate::groups::peers::PeerClient;
use crate::routes::AppState;
use crate::transcript::pyjson::dumps_unicode;
use crate::vault::{VaultError, exec, files, remote::WaitStatus, wire::{AskReq, UseKind, UseReq}};
use zeroize::Zeroizing;

const VOICE_CLOSE_REFUSED: &str = "recusado: a voz fecha sessão pela própria ferramenta close_session, que pede o sim \
falado do usuário";
const VOICE_NEEDS_SESSION: &str = "a assistente de voz não é uma sessão: passe `sessao` com o nome da sessão em nome de \
quem agir (tool `sessions`)";
const JEV_OFF: &str = "o Jev não chegou ligado nesta chamada: ou a sessão nasceu sem ele (abra uma com \
`hangar-send --new <nome> <cwd> --jev`, ou marque o Jev em \"Mais opções\" ao criar), ou o MCP hangar desta \
conta foi registrado antes do cabeçalho X-Hangar-Jev (rode scripts/registrar-mcp.py e abra a sessão de novo). \
Sem ele, use browser e browser_batch.";
const JEV_UNCONFIGURED: &str = "o Jev não está configurado neste servidor: cadastre a chave em Configurações do servidor > Jev.";
const NAV_VERBS: [&str; 26] = ["snapshot", "click", "fill", "type", "press", "hover", "wait", "eval", "tema", "layout", "console",
    "network", "text", "url", "shot", "close", "goto", "back", "reload", "dialog", "login", "estado", "tab-list", "tab-new",
    "tab-switch", "tab-close"];
/// O `login` espera o usuário aprovar no painel (até 110 s no app).
const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);
const ROUTE_BODY_LIMIT: usize = 16 << 20;

type Outcome = Result<CallToolResult, String>;

pub struct Tools {
    st: Arc<AppState>,
    api: Router,
    http: reqwest::Client,
    server_id: String,
    /// Duas chamadas do mesmo turno não intercalam `click` e `snapshot` no navegador de uma sessão.
    nav_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Resultado de dicionário: o texto indentado e o mesmo valor estruturado, como o SDK do Python.
fn dict(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&value).unwrap_or_default())]);
    result.structured_content = Some(value);
    result
}

fn text(value: impl Into<String>) -> CallToolResult { CallToolResult::success(vec![ContentBlock::text(value)]) }

fn image(path: &Path) -> Option<ContentBlock> {
    let bytes = std::fs::read(path).ok()?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    Some(ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(bytes), mime))
}

fn args<T: DeserializeOwned>(tool: &str, raw: JsonObject) -> Result<T, String> {
    serde_json::from_value(Value::Object(raw)).map_err(|e| match super::expected_fields(tool) {
        Some(fields) => format!("argumentos inválidos para {tool}: {e}. Campos: {fields}"),
        None => format!("argumentos inválidos para {tool}: {e}"),
    })
}

/// A frase do erro de uma rota: `detail.msg` do `erro()`, o `detail` texto, ou o status.
fn detail(status: u16, body: &Value) -> String {
    let d = &body["detail"];
    d["msg"].as_str().or_else(|| d.as_str()).map(str::to_owned)
        .unwrap_or_else(|| if d.is_null() { format!("HTTP {status}") } else { d.to_string() })
}

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) }

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_owned())
}

#[derive(Deserialize)]
struct Send { alvo: String, texto: String, #[serde(default, rename = "tmux")] _tmux: bool }

#[derive(Deserialize)]
struct Group { texto: String, #[serde(default)] tmux: bool, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct Pair { #[serde(default)] alvo: String, #[serde(default)] tarefa: String, #[serde(default)] substituir_tarefa: bool,
    #[serde(default)] orq: bool, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct OnlySession { #[serde(default)] sessao: String }

/// Sem `conteudo` lê; com ele grava, e `mtime` é o da leitura (nulo = o contrato ainda não existe).
#[derive(Deserialize)]
struct Contract { conteudo: Option<String>, #[serde(default)] mtime: Option<String>, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct Close { alvo: String }

#[derive(Deserialize)]
struct RequestSecret { label: String, reason: String, #[serde(default)] placeholder: Option<String> }

#[derive(Deserialize)]
struct VaultExec { entry: String, env: String, command: String, reason: String, #[serde(default)] timeout_s: Option<u64> }

#[derive(Deserialize)]
struct VaultFile { entry: String, reason: String }

#[derive(Deserialize)]
struct VaultFill { entry: String, field: String, #[serde(rename = "ref")] target: String, reason: String }

const VAULT_NO_KEY: &str = "erro_cofre_sem_chave_sessao: o cofre só atende uma sessão identificada pela chave dela (X-Hangar-Key, \
a CP_SESSION_KEY); registre o MCP de novo (scripts/registrar-mcp.py) e reabra a sessão";
/// O alvo que o cartão mostra cabe no limite da dona.
const TARGET_CHARS: usize = 2000;

#[derive(Deserialize)]
struct NewSession {
    nome: String, cwd: Option<String>, provider: Option<String>, engine: Option<String>, model: Option<String>, effort: Option<String>,
    permissao: Option<String>, headless: Option<bool>, #[serde(default)] read_only: bool, conta: Option<String>, jev: Option<bool>,
    service_tier: Option<String>, branch: Option<String>, #[serde(default)] new_branch: bool, base: Option<String>,
    codex_account: Option<String>, subagent_model: Option<String>, initial_prompt: Option<String>, engine_account: Option<String>,
    omp_profile: Option<String>, claude_customizations: Option<Value>, inherit: Option<bool>, #[serde(default)] remember_provider: bool,
    resume: Option<String>, #[serde(default)] link: bool,
}

impl NewSession {
    /// O corpo da retomada; o que ela não aceita é recusado pelo nome.
    fn clone_for_resume(&self) -> Result<Value, String> {
        let unsupported = [("effort", self.effort.is_some()), ("permissao", self.permissao.is_some()), ("read_only", self.read_only),
            ("jev", self.jev.is_some()), ("service_tier", self.service_tier.is_some()), ("branch", self.branch.as_deref().is_some_and(|b| !b.is_empty())),
            ("new_branch", self.new_branch), ("subagent_model", self.subagent_model.is_some()), ("initial_prompt", self.initial_prompt.is_some()),
            ("omp_profile", self.omp_profile.is_some()), ("claude_customizations", self.claude_customizations.is_some()),
            ("inherit", self.inherit.is_some()), ("remember_provider", self.remember_provider), ("link", self.link)];
        if let Some((key, _)) = unsupported.iter().find(|(_, set)| *set) { return Err(format!("{key} não combina com resume")); }
        let mut body = json!({"name": self.nome});
        for (key, value) in [("provider", &self.provider), ("engine", &self.engine), ("model", &self.model), ("config_dir", &self.conta),
                             ("codex_account", &self.codex_account), ("engine_account", &self.engine_account)] {
            if let Some(value) = value { body[key] = json!(value); }
        }
        if let Some(headless) = self.headless { body["headless"] = json!(headless); }
        Ok(body)
    }

    /// O corpo do `POST /api/sessions` com o que veio explícito; `inherit`, `link` e `creator` são desta máquina e
    /// quem chama acrescenta.
    fn create_body(self, cwd: String) -> Value {
        let mut body = json!({"name": self.nome, "cwd": cwd});
        let optional = [("provider", self.provider.map(Value::from)), ("engine", self.engine.map(Value::from)), ("model", self.model.map(Value::from)),
            ("effort", self.effort.map(Value::from)), ("permission_mode", self.permissao.map(Value::from)), ("headless", self.headless.map(Value::from)),
            ("config_dir", self.conta.map(Value::from)), ("jev", self.jev.map(Value::from)), ("service_tier", self.service_tier.map(Value::from)),
            ("branch", self.branch.filter(|b| !b.is_empty()).map(Value::from)), ("base", self.base.filter(|b| !b.is_empty()).map(Value::from)),
            ("codex_account", self.codex_account.map(Value::from)), ("subagent_model", self.subagent_model.map(Value::from)),
            ("initial_prompt", self.initial_prompt.map(Value::from)), ("engine_account", self.engine_account.map(Value::from)),
            ("omp_profile", self.omp_profile.map(Value::from)), ("claude_customizations", self.claude_customizations),
            ("read_only", self.read_only.then_some(Value::Bool(true))), ("new_branch", self.new_branch.then_some(Value::Bool(true))),
            ("remember_provider", self.remember_provider.then_some(Value::Bool(true)))];
        for (key, value) in optional {
            if let Some(value) = value { body[key] = value; }
        }
        body
    }
}

/// `servidor::nome` → `Some((servidor, nome))`; nome sem `::` é desta máquina.
fn remote_target(nome: &str) -> Result<Option<(String, String)>, String> {
    let Some((server, name)) = nome.split_once("::") else { return Ok(None) };
    if server.is_empty() || name.is_empty() || name.contains("::") {
        return Err(format!("nome inválido '{nome}': use servidor::nome (um '::' só)"));
    }
    Ok(Some((server.to_owned(), name.to_owned())))
}

/// A conta de lá pelo caminho, rótulo ou nome da pasta (`work` acha `.claude-work`), como o `hangar-send`.
fn remote_account(rows: &[Value], conta: &str) -> Option<String> {
    rows.iter().filter_map(|c| Some((c["path"].as_str()?, c["label"].as_str().unwrap_or_default()))).find(|(path, label)| {
        let folder = path.replace('\\', "/").rsplit('/').next().unwrap_or_default().to_owned();
        [*path, *label, folder.as_str()].contains(&conta) || folder == format!(".claude-{conta}")
    }).map(|(path, _)| path.to_owned())
}

/// A resposta da criação, igual para esta e para outra máquina.
fn created(info: &Value, server: Option<&str>) -> Outcome {
    let mut out = json!({"name": info["name"], "cwd": info["cwd"], "provider": info["provider"], "headless": info["headless"],
        // Volta na resposta para que herdar errado nunca passe despercebido.
        "config_dir": info["config_dir"], "account_source": info["account_source"]});
    if let Some(server) = server { out["server"] = json!(server); }
    if info["avisos"].as_array().is_some_and(|a| !a.is_empty()) { out["avisos"] = info["avisos"].clone(); }
    // A sessão abriu: o erro diz isso e leva a resposta inteira, para nada se perder.
    if let Some(e) = info.get("link_error").filter(|e| !e.is_null()) {
        return Err(format!("sessão '{}' abriu SEM a ligação: {}: {} — {out}",
            info["name"].as_str().unwrap_or_default(), e["code"].as_str().unwrap_or_default(), e["detail"].as_str().unwrap_or_default()));
    }
    Ok(dict(out))
}

/// `new_session` com `servidor::nome`: abre lá, pela API de lá (`peers.json`), com as contas e os padrões de lá, como
/// `hangar-send --new servidor::nome`. Vai só o que veio explícito: nada herda de quem chama noutra máquina.
async fn remote_session(peers: &PeerClient, server: &str, mut a: NewSession) -> Outcome {
    if a.link { return Err("erro_link_remoto: a ligação só vale entre sessões desta máquina".into()); }
    if a.inherit == Some(true) { return Err("inherit não vale noutra máquina: lá nada herda de quem chama".into()); }
    a.inherit = None;
    let resume = a.resume.clone().filter(|s| !s.is_empty());
    // Sem pasta, a home de lá: o servidor de lá expande o `~`.
    let cwd = a.cwd.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "~".into());
    let call = move |method, path: String, body: Option<Value>| async move {
        peers.call(server, method, &path, body.as_ref()).await.map(|v| v.unwrap_or(Value::Null)).map_err(|e| e.text(server))
    };
    if let Some(conta) = a.conta.clone().filter(|c| !c.is_empty()) {
        let configs = call(reqwest::Method::GET, "/api/claude-configs".into(), None).await?;
        let rows = configs.as_array().map(Vec::as_slice).unwrap_or_default();
        let labels: Vec<&str> = rows.iter().filter_map(|c| c["label"].as_str()).collect();
        a.conta = Some(remote_account(rows, &conta)
            .ok_or_else(|| format!("conta '{conta}' não existe em '{server}' — lá há: {}", labels.join(", ")))?);
    }
    if let Some(sid) = resume {
        let info = call(reqwest::Method::POST, format!("/api/archive/-/{}/resume", segment(&sid)), Some(a.clone_for_resume()?)).await?;
        return Ok(resumed(&info, Some(server)));
    }
    let info = call(reqwest::Method::POST, "/api/sessions".into(), Some(a.create_body(cwd))).await?;
    created(&info, Some(server))
}

/// A resposta da retomada do Arquivo, igual para esta e para outra máquina.
fn resumed(info: &Value, server: Option<&str>) -> CallToolResult {
    let mut out = json!({"name": info["name"], "cwd": info["cwd"], "provider": info["provider"], "headless": info["headless"]});
    if let Some(server) = server { out["server"] = json!(server); }
    // O aviso de troca de conta por cota é a única notícia de que a sessão nasceu noutra conta.
    if info["avisos"].as_array().is_some_and(|a| !a.is_empty()) { out["avisos"] = info["avisos"].clone(); }
    dict(out)
}

#[derive(Deserialize)]
struct BrowserOpen { url: String, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct HtmlRender { title: String, html: Option<String>, url: Option<String>, height: Option<i64>, #[serde(default)] draft: bool,
    #[serde(default)] own_theme: bool, width: Option<i64>, theme: Option<String>, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct Browser { verbo: String, args: Option<Vec<Value>>, aba: Option<i64>, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct Batch { passos: Vec<Map<String, Value>>, #[serde(default)] sessao: String }

#[derive(Deserialize)]
struct Goal { objetivo: String, dados: Option<Map<String, Value>>, #[serde(default = "fifteen")] passos: i64,
    #[serde(default)] refazer: bool, #[serde(default)] pasta: String, #[serde(default)] sessao: String }

fn fifteen() -> i64 { 15 }

#[derive(Deserialize)]
struct Check { estado: String, #[serde(default)] sessao: String }

/// Argumento do verbo: texto ou número, como o `list[str | int]` do Python.
fn verb_args(raw: Option<&[Value]>) -> Result<Vec<String>, String> {
    raw.unwrap_or_default().iter().map(|a| match a {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        other => Err(format!("argumento de verbo inválido: {other}")),
    }).collect()
}

/// O verbo que `browser`/`browser_batch` não mandam. `fill-secret` sai só do `vault_fill`, com o valor fora dos args.
fn verb_refusal(verb: &str) -> Option<String> {
    if verb == "fill-secret" { return Some("erro_cofre_so_vault_fill: fill-secret só pela tool vault_fill".into()); }
    (!NAV_VERBS.contains(&verb)).then(|| format!("verbo desconhecido: {verb} (aceitos: {})", NAV_VERBS.join(", ")))
}

/// `interrupt`: a voz não abre filha ligada (nasce sem `creator`), e Esc na própria sessão mataria o turno de quem chama.
fn interrupt_caller_refusal(me: &str, voice: bool, alvo: &str) -> Option<String> {
    if voice { return Some("recusado: a voz não interrompe sessão; só quem abriu a filha com `link` interrompe".into()); }
    if alvo.contains("::") { return Some("interrupt só alcança sessão desta máquina".into()); }
    (alvo == me).then(|| format!("recusado: '{alvo}' é esta sessão"))
}

/// O vínculo só vale com o pai ainda no grupo da filha: sair do grupo desfaz o poder de interromper.
fn link_refusal(me: &str, alvo: &str, sidecar: Option<&Sidecar>) -> Option<String> {
    (sidecar.and_then(Sidecar::linked_parent) != Some(me)).then(|| format!("erro_interromper_nao_ligada: '{alvo}' não é filha ligada a você (aberta por você com new_session `link`)"))
}

impl Tools {
    pub fn new(st: Arc<AppState>, api: Router) -> Self {
        let http = crate::http_client().no_proxy().build().expect("cliente http do MCP");
        let server_id = st.server_id.clone();
        Self { st, api, http, server_id, nav_locks: Mutex::default() }
    }

    pub async fn call(&self, name: &str, raw: JsonObject, headers: &HeaderMap) -> CallToolResult {
        let outcome = match name {
            "who_am_i" => self.who_am_i(headers).await,
            "sessions" => self.sessions(headers).await,
            "send" => match args(name, raw) { Ok(a) => self.send(headers, a).await, Err(e) => Err(e) },
            "group" => match args(name, raw) { Ok(a) => self.group(headers, a).await, Err(e) => Err(e) },
            "pair" => match args(name, raw) { Ok(a) => self.pair(headers, a).await, Err(e) => Err(e) },
            "unpair" => match args::<OnlySession>(name, raw) { Ok(a) => self.unpair(headers, &a.sessao).await, Err(e) => Err(e) },
            "contract" => match args(name, raw) { Ok(a) => self.contract(headers, a).await, Err(e) => Err(e) },
            "new_session" => match args(name, raw) { Ok(a) => self.new_session(headers, a).await, Err(e) => Err(e) },
            "close_session" => match args::<Close>(name, raw) { Ok(a) => self.close_session(headers, &a.alvo).await, Err(e) => Err(e) },
            "interrupt" => match args::<Close>(name, raw) { Ok(a) => self.interrupt(headers, &a.alvo).await, Err(e) => Err(e) },
            "browser_open" => match args(name, raw) { Ok(a) => self.browser_open(headers, a).await, Err(e) => Err(e) },
            "html_render" => match args(name, raw) { Ok(a) => self.html_render(headers, a).await, Err(e) => Err(e) },
            "browser" => match args(name, raw) { Ok(a) => self.browser(headers, a).await, Err(e) => Err(e) },
            "browser_batch" => match args(name, raw) { Ok(a) => self.browser_batch(headers, a).await, Err(e) => Err(e) },
            "browser_goal" => match args(name, raw) { Ok(a) => self.browser_goal(headers, a).await, Err(e) => Err(e) },
            "browser_check" => match args(name, raw) { Ok(a) => self.browser_check(headers, a).await, Err(e) => Err(e) },
            "request_secret" => match args(name, raw) { Ok(a) => self.request_secret(headers, a).await, Err(e) => Err(e) },
            "vault_list" => self.vault_list(headers).await,
            "vault_exec" => match args(name, raw) { Ok(a) => self.vault_exec(headers, a).await, Err(e) => Err(e) },
            "vault_file" => match args(name, raw) { Ok(a) => self.vault_file(headers, a).await, Err(e) => Err(e) },
            "vault_fill" => match args(name, raw) { Ok(a) => self.vault_fill(headers, a).await, Err(e) => Err(e) },
            _ => Err(format!("Unknown tool: {name}")),
        };
        outcome.unwrap_or_else(|msg| CallToolResult::error(vec![ContentBlock::text(msg)]))
    }

    fn home(&self) -> PathBuf {
        self.st.list.env().dirs.as_ref().map(|d| d.home.clone()).or_else(std::env::home_dir).unwrap_or_default()
    }

    async fn resolve(&self, headers: &HeaderMap) -> Result<(String, Origin), String> {
        let home = self.home();
        let local = Local { home: &home, mux: &self.st.list.env().mux };
        identity::resolve(headers, &local, &self.st.cfg.internal_secret).await.map_err(|e| format!("sessao_desconhecida: {e}"))
    }

    /// `(nome, é a voz)`.
    async fn caller(&self, headers: &HeaderMap) -> Result<(String, bool), String> {
        self.resolve(headers).await.map(|(name, origin)| (name, origin == Origin::Voice))
    }

    /// A sessão em nome de quem a tool age. A voz não tem grupo, navegador nem conversa: diz qual.
    async fn me(&self, headers: &HeaderMap, sessao: &str) -> Result<String, String> {
        let (name, voice) = self.caller(headers).await?;
        if !voice { return Ok(name); }
        let sessao = sessao.trim();
        if sessao.is_empty() { return Err(VOICE_NEEDS_SESSION.into()); }
        Ok(sessao.to_owned())
    }

    /// A rota pública, como o dono desta máquina; `(status, corpo)`.
    async fn route(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = Request::builder().method(method).uri(path)
            .header("authorization", format!("Bearer {}", self.st.cfg.auth_token));
        if body.is_some() { request = request.header("content-type", "application/json"); }
        let Ok(mut request) = request.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))) else {
            return (500, json!({"detail": "caminho inválido"}));
        };
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let Ok(response) = self.api.clone().call(request).await;
        let status = response.status().as_u16();
        let raw = axum::body::to_bytes(response.into_body(), ROUTE_BODY_LIMIT).await.unwrap_or_default();
        (status, serde_json::from_slice(&raw).unwrap_or_else(|_| json!({"detail": String::from_utf8_lossy(&raw)})))
    }

    /// Corpo de sucesso da rota, ou a frase do erro.
    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let (status, value) = self.route(method, path, body).await;
        if (200..300).contains(&status) { Ok(value) } else { Err(detail(status, &value)) }
    }

    async fn who_am_i(&self, headers: &HeaderMap) -> Outcome {
        let (name, origin) = self.resolve(headers).await?;
        Ok(dict(if origin == Origin::Voice {
            json!({"name": name, "origem": origin.as_str(), "kind": "voice", "label": "assistente de voz do Hangar"})
        } else {
            json!({"name": name, "origem": origin.as_str()})
        }))
    }

    async fn sessions(&self, headers: &HeaderMap) -> Outcome {
        let me = self.caller(headers).await.ok().filter(|(_, voice)| !voice).map(|(name, _)| name);
        let rows = self.ok(Method::GET, "/api/sessions", None).await?;
        let rows: Vec<Value> = rows.as_array().into_iter().flatten().map(|s| json!({"name": s["name"], "state": s["state"],
            "cwd": s["cwd"], "provider": s["provider"], "headless": s["headless"],
            "voce": s["name"].as_str().is_some_and(|n| Some(n) == me.as_deref()), "grupo": s["pair_gid"],
            // Membro de outra máquina vem como `servidor::sessao`; `dona` só em grupo entre máquinas.
            "membros": s["pair_peers"], "dona": s["pair_owner"]})).collect();
        let content = rows.iter().map(|r| ContentBlock::text(serde_json::to_string_pretty(r).unwrap_or_default())).collect();
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(json!({"result": rows}));
        Ok(result)
    }

    async fn send(&self, headers: &HeaderMap, a: Send) -> Outcome {
        let (me, voice) = self.caller(headers).await?;
        let sid = &self.server_id;
        // A voz não é sessão: recado dela para uma sessão chamada "voz" não volta para ela mesma.
        if !voice && (a.alvo == me || (!sid.is_empty() && a.alvo == format!("{sid}::{me}"))) {
            return Err(format!("recusado: '{}' é esta sessão — o recado voltaria pra você. Quem é quem: tool `sessions` (campo `voce`).", a.alvo));
        }
        // Modelo que escreve "[de: eu] …" por conta própria não ganha o prefixo em dobro.
        let server = if sid.is_empty() { String::new() } else { format!("(?:{}::)?", regex::escape(sid)) };
        let prefix = regex::Regex::new(&format!(r"^\s*\[de:\s*{server}{}\]\s*", regex::escape(&me))).map_err(|e| e.to_string())?;
        let texto = prefix.replace(&a.texto, "").into_owned();
        let with_target = |resp: Value| {
            let mut out = Map::from_iter([("alvo".to_owned(), json!(a.alvo))]);
            if let Value::Object(fields) = resp { out.extend(fields); }
            dict(Value::Object(out))
        };
        let Some((server_id, session)) = a.alvo.split_once("::") else {
            let body = json!({"text": format!("[de: {me}] {texto}"), "steer": true});
            return self.ok(Method::POST, &format!("/api/sessions/{}/input", segment(&a.alvo)), Some(body)).await.map(with_target);
        };
        // Par externo primeiro (só o Rust sabe chamar a máquina de outra pessoa); apelido que não é
        // par desta sessão segue como máquina própria.
        if self.st.share.is_some() {
            let body = json!({"sender": me, "target": a.alvo, "text": texto});
            let (status, value) = self.route(Method::POST, "/api/external-pairs/send", Some(body)).await;
            if (200..300).contains(&status) { return Ok(with_target(value)); }
            let code = value["detail"]["code"].as_str().unwrap_or_default();
            if code != "erro_par_inexistente" {
                let msg = value["detail"]["msg"].as_str().filter(|m| !m.is_empty()).unwrap_or(code);
                return Err(format!("'{}': {}", a.alvo, if msg.is_empty() { detail(status, &value) } else { msg.to_owned() }));
            }
        }
        if sid.is_empty() { return Err("CP_SERVER_ID ausente no backend/.env — obrigatório pra envio cross-server".into()); }
        let body = json!({"text": format!("[de: {sid}::{me}] {texto}"), "steer": true});
        match self.st.peers.call(server_id, reqwest::Method::POST, &format!("/api/sessions/{session}/input"), Some(&body)).await {
            Ok(resp) => Ok(with_target(resp.unwrap_or_else(|| json!({})))),
            Err(e) => Err(e.text(server_id)),
        }
    }

    async fn group(&self, headers: &HeaderMap, a: Group) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let body = json!({"text": a.texto, "forcar_tmux": a.tmux});
        self.ok(Method::POST, &format!("/api/sessions/{}/group-message", segment(&me)), Some(body)).await.map(dict)
    }

    async fn pair(&self, headers: &HeaderMap, a: Pair) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let body = json!({"peer": a.alvo, "task": a.tarefa, "replace_task": a.substituir_tarefa, "orq": a.orq});
        self.ok(Method::POST, &format!("/api/sessions/{}/pair", segment(&me)), Some(body)).await.map(dict)
    }

    async fn unpair(&self, headers: &HeaderMap, sessao: &str) -> Outcome {
        let me = self.me(headers, sessao).await?;
        self.ok(Method::DELETE, &format!("/api/sessions/{}/pair", segment(&me)), None).await.map(dict)
    }

    async fn contract(&self, headers: &HeaderMap, a: Contract) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let path = format!("/api/sessions/{}/pair/contract", segment(&me));
        let (status, value) = match a.conteudo {
            None => self.route(Method::GET, &path, None).await,
            Some(content) => self.route(Method::PUT, &path, Some(json!({"content": content, "mtime": a.mtime}))).await,
        };
        if (200..300).contains(&status) { return Ok(dict(value)); }
        // O código vai na frente: `erro_contrato_mudou` é o sinal de ler de novo antes de gravar.
        Err(match value["detail"]["code"].as_str() {
            Some(code) => format!("{code}: {}", detail(status, &value)),
            None => detail(status, &value),
        })
    }

    async fn new_session(&self, headers: &HeaderMap, mut a: NewSession) -> Outcome {
        if a.headless == Some(true) && let Some(provider) = a.provider.as_deref().filter(|p| !matches!(*p, "claude" | "codex")) {
            return Err(format!("headless só vale com provider claude ou codex (veio: {provider})"));
        }
        // Quem chama precisa ser resolvido também para abrir noutra máquina, mesmo sem herdar nada.
        let (me, voice) = self.caller(headers).await?;
        if let Some((server, name)) = remote_target(&a.nome)? {
            a.nome = name;
            return remote_session(&self.st.peers, &server, a).await;
        }
        if let Some(sid) = a.resume.as_deref().filter(|s| !s.is_empty()) { return self.resume_session(sid, a.clone_for_resume()).await; }
        let (inherit, link) = (a.inherit, a.link);
        let cwd = a.cwd.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| self.home().display().to_string());
        let mut body = a.create_body(cwd);
        for (key, value) in [("inherit", inherit.map(Value::from)), ("link", link.then_some(Value::Bool(true))),
            // A voz não tem conta nem modo para herdar: o omitido fica com o padrão do servidor.
            ("creator", (!voice).then_some(Value::from(me)))] {
            if let Some(value) = value { body[key] = value; }
        }
        let info = self.ok(Method::POST, "/api/sessions", Some(body)).await?;
        created(&info, None)
    }

    /// `resume`: a conversa volta pela rota do Arquivo (`-` = a pasta que a guarda), com nome e superfície.
    async fn resume_session(&self, sid: &str, a: Result<Value, String>) -> Outcome {
        let body = a?;
        let info = self.ok(Method::POST, &format!("/api/archive/-/{}/resume", segment(sid)), Some(body)).await?;
        Ok(resumed(&info, None))
    }

    async fn close_session(&self, headers: &HeaderMap, alvo: &str) -> Outcome {
        let (me, voice) = self.caller(headers).await?;
        if voice { return Err(VOICE_CLOSE_REFUSED.into()); }
        if alvo.contains("::") { return Err("close_session só fecha sessão desta máquina".into()); }
        if alvo == me { return Err(format!("recusado: '{alvo}' é esta sessão — peça pra outra fechar")); }
        // O DELETE responde ok para nome que não existe: nome errado não pode virar "fechada".
        let home = self.home();
        if !identity::exists(&Local { home: &home, mux: &self.st.list.env().mux }, alvo).await {
            return Err(format!("sessão '{alvo}' não existe (tool `sessions`)"));
        }
        self.ok(Method::DELETE, &format!("/api/sessions/{}", segment(alvo)), None).await.map(dict)
    }

    async fn interrupt(&self, headers: &HeaderMap, alvo: &str) -> Outcome {
        let (me, voice) = self.caller(headers).await?;
        if let Some(refusal) = interrupt_caller_refusal(&me, voice, alvo) { return Err(refusal); }
        let home = self.home();
        if !identity::exists(&Local { home: &home, mux: &self.st.list.env().mux }, alvo).await {
            return Err(format!("sessão '{alvo}' não existe (tool `sessions`)"));
        }
        let groups = self.st.groups.as_ref().ok_or("grupos indisponíveis neste servidor: o vínculo da filha não pode ser conferido")?;
        let sidecar = groups.link(alvo).await.map_err(|e| format!("arquivo de grupo de '{alvo}' ilegível: {e}"))?;
        if let Some(refusal) = link_refusal(&me, alvo, sidecar.as_ref()) { return Err(refusal); }
        self.ok(Method::POST, &format!("/api/sessions/{}/interrupt", segment(alvo)), None).await.map(dict)
    }

    async fn browser_open(&self, headers: &HeaderMap, a: BrowserOpen) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let (since, home) = (now_ms(), self.home());
        let before = nav::url_before(&home, &me);
        self.ok(Method::POST, &format!("/api/sessions/{}/nav", segment(&me)), Some(json!({"url": a.url}))).await?;
        let ready = nav::wait_ready(&self.http, &home, &me, since, before.as_deref(), &a.url).await.map_err(|e| e.0)?;
        Ok(dict(json!({"ok": true, "url": ready})))
    }

    async fn html_render(&self, headers: &HeaderMap, a: HtmlRender) -> Outcome {
        if a.html.is_some() == a.url.is_some() { return Err("erro_pagina_html_ou_url: passe html ou url, nunca os dois".into()); }
        let me = self.me(headers, &a.sessao).await?;
        let mut payload = json!({"session": me, "title": a.title, "draft": a.draft});
        match (a.url, a.html) {
            (Some(url), _) => payload["url"] = json!(url),
            (None, html) => payload["html"] = json!(html),
        }
        if a.own_theme { payload["own_theme"] = json!(true); }
        for (key, value) in [("height", a.height.map(Value::from)), ("width", a.width.map(Value::from)), ("theme", a.theme.map(Value::from))] {
            if let Some(value) = value { payload[key] = value; }
        }
        let answer = crate::pages::routes::publish_value(&self.st, payload).await;
        if answer["ok"] != true {
            let (code, detail) = (answer["error"]["code"].as_str().unwrap_or("erro_paginas_indisponivel"), answer["error"]["detail"].as_str().unwrap_or(""));
            return Err(if detail.is_empty() { code.to_owned() } else { format!("{code}: {detail}") });
        }
        let result = answer["result"].clone();
        // O print do rascunho volta na resposta, como o `shot` do navegador; o estruturado fica igual.
        let shot = result["draft"]["shot"].as_str().map(PathBuf::from).filter(|p| p.is_file()).and_then(|p| image(&p));
        let Some(shot) = shot else { return Ok(dict(result)) };
        let mut out = CallToolResult::success(vec![ContentBlock::text(dumps_unicode(&result, false)), shot]);
        out.structured_content = Some(result);
        Ok(out)
    }

    fn nav_lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.nav_locks.lock().unwrap_or_else(|e| e.into_inner()).entry(name.to_owned()).or_default().clone()
    }

    async fn nav_verb(&self, name: &str, verb: &str, mut args: Vec<String>, tab: Option<i64>) -> Result<String, String> {
        if let Some(refusal) = verb_refusal(verb) { return Err(refusal); }
        if verb == "eval" && args.is_empty() { return Err("eval precisa de um trecho JS".into()); }
        let home = self.home();
        if verb == "shot" {
            // Caminho nosso e estável, que o app do celular consegue servir.
            let safe = regex::Regex::new(r"\W+").map_err(|e| e.to_string())?.replace_all(name, "-").into_owned();
            let dir = nav::nav_dir(&home).join("shots").join(safe);
            std::fs::create_dir_all(&dir).map_err(|e| format!("pasta dos prints: {e}"))?;
            args = vec![dir.join(format!("{}.png", now_ms())).to_string_lossy().into_owned()];
        }
        let timeout = if verb == "login" { LOGIN_TIMEOUT } else { nav::VERB_TIMEOUT };
        let answer = nav::verb(&self.http, &home, name, verb, &args, tab, timeout).await.map_err(|e| e.0)?;
        if answer.starts_with("erro:") { Err(answer) } else { Ok(answer) }
    }

    async fn browser(&self, headers: &HeaderMap, a: Browser) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let args = verb_args(a.args.as_deref())?;
        let answer = {
            let lock = self.nav_lock(&me);
            let _held = lock.lock().await;
            self.nav_verb(&me, &a.verbo, args, a.aba).await?
        };
        // O print volta na resposta: o agente vê sem outra ida ao disco.
        let shot = (a.verbo == "shot").then(|| PathBuf::from(answer.strip_prefix("ok: shot ").unwrap_or(&answer).trim()))
            .filter(|p| p.is_file()).and_then(|p| image(&p));
        Ok(match shot {
            Some(shot) => CallToolResult::success(vec![ContentBlock::text(answer), shot]),
            None => text(answer),
        })
    }

    async fn browser_batch(&self, headers: &HeaderMap, a: Batch) -> Outcome {
        let me = self.me(headers, &a.sessao).await?;
        let lock = self.nav_lock(&me);
        let _held = lock.lock().await;
        let mut done: Vec<Value> = Vec::new();
        for (i, step) in a.passos.iter().enumerate() {
            let verb = step.get("verbo").and_then(Value::as_str).unwrap_or_default();
            let ran = match verb_args(step.get("args").and_then(Value::as_array).map(Vec::as_slice)) {
                Ok(args) => self.nav_verb(&me, verb, args, step.get("aba").and_then(Value::as_i64)).await,
                Err(e) => Err(e),
            };
            match ran {
                Ok(answer) => done.push(Value::String(answer)),
                Err(e) => return Err(format!("parou no passo {i} ({verb}): {e}\nfeitos antes: {}", dumps_unicode(&Value::Array(done), false))),
            }
        }
        Ok(dict(json!({"feitos": done})))
    }

    /// `hangar-preview objetivo|confere` em nome da sessão, com a chave do servidor. Só para sessão que
    /// nasceu com o Jev (o `headersHelper` repassa o `HANGAR_JEV` dela); a voz usa a do servidor.
    async fn jev_cli(&self, headers: &HeaderMap, sessao: &str, args: Vec<String>, limit: Duration) -> Result<(i32, String), String> {
        let (_, voice) = self.caller(headers).await?;
        if !voice && header(headers, "x-hangar-jev").as_deref() != Some("on") { return Err(JEV_OFF.into()); }
        let target = self.me(headers, sessao).await?;
        // `runtime-config.json` da conta do servidor (`settings.projects_dir.parent`, a pasta `claude` da lista).
        let base = self.st.list.env().dirs.as_ref().map(|d| d.claude.clone())
            .unwrap_or_else(|| crate::accounts::environment::AccountEnvironment::capture().claude_base);
        let runtime = std::fs::read(base.join("runtime-config.json")).ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
        let mut env: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k != "TYPESAFE_API_KEY" && !k.starts_with("JEV_")
                && !matches!(k.as_str(), "CP_AUTH_TOKEN" | "HANGAR_INTERNAL_SECRET" | "HANGAR_RUNTIME_INSTANCE" | "HANGAR_AUTH_CONTEXT"))
            .collect();
        env.extend(crate::session_open::claude::jev_pairs(true, &runtime));
        if !env.iter().any(|(k, v)| k == "TYPESAFE_API_KEY" && !v.is_empty()) { return Err(JEV_UNCONFIGURED.into()); }
        let node = crate::runtime::process::resolve_program("node", env.iter().map(|(k, v)| (k, v)))
            .ok_or("node não encontrado no PATH do backend: o objetivo roda pelo hangar-preview")?;
        // A raiz da instalação; sem ela, o pai da pasta de trabalho (`backend/`), herdada do Python.
        let home = crate::accounts::environment::AccountEnvironment::capture().home;
        let script = crate::install_root::root(&home).unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(".."))
            .join("scripts").join("hangar-preview");
        // Mesma trava dos verbos: um `browser` no meio refaria o snapshot e trocaria as refs do Jev.
        let lock = self.nav_lock(&target);
        let _held = lock.lock().await;
        let mut command = tokio::process::Command::new(node);
        command.arg(script).args(&args).args(["--sessao", &target]).env_clear().envs(env)
            .stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = match tokio::time::timeout(limit, command.output()).await {
            Err(_) => return Err(format!("o Jev não terminou em {}s; a página ficou onde parou (veja com browser snapshot)", limit.as_secs())),
            Ok(Err(e)) => return Err(format!("hangar-preview não rodou: {e}")),
            Ok(Ok(out)) => out,
        };
        // ponytail: a saída e o erro vêm um depois do outro, não intercalados como no cano único do Python.
        let mut joined = String::from_utf8_lossy(&out.stdout).into_owned();
        joined.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.code().unwrap_or(-1), joined.replace("hangar-preview: ", "").trim().to_owned()))
    }

    async fn browser_goal(&self, headers: &HeaderMap, a: Goal) -> Outcome {
        let mut args = vec!["objetivo".to_owned(), a.objetivo, "--passos".into(), a.passos.clamp(1, 40).to_string()];
        if let Some(dados) = a.dados.filter(|d| !d.is_empty()) { args.extend(["--dados".into(), dumps_unicode(&Value::Object(dados), false)]); }
        if a.refazer { args.push("--refazer".into()); }
        if !a.pasta.trim().is_empty() { args.extend(["--pasta".into(), a.pasta.trim().to_owned()]); }
        // O script roda no cwd do servidor: a pasta do roteiro sai do git onde a sessão trabalha.
        let target = self.me(headers, &a.sessao).await?;
        let rows = self.ok(Method::GET, "/api/sessions", None).await.unwrap_or(Value::Null);
        let row = rows.as_array().into_iter().flatten().find(|r| r["name"] == target.as_str());
        let cwd = row.and_then(|r| r["git_cwd"].as_str().filter(|c| !c.is_empty()).or_else(|| r["cwd"].as_str().filter(|c| !c.is_empty())));
        let note = match cwd {
            Some(cwd) => { args.extend(["--cwd".into(), cwd.to_owned()]); String::new() }
            None => "\naviso: cwd da sessão desconhecido (sessao nao encontrada); roteiro relativo ao backend ou no cache do usuário".into(),
        };
        let (code, output) = self.jev_cli(headers, &a.sessao, args, Duration::from_secs(300)).await?;
        let output = output + &note;
        if code != 0 { return Err(output); }
        let mut result = text(output.clone());
        result.structured_content = Some(json!({"result": output}));
        Ok(result)
    }

    async fn browser_check(&self, headers: &HeaderMap, a: Check) -> Outcome {
        let (code, output) = self.jev_cli(headers, &a.sessao, vec!["confere".into(), a.estado], Duration::from_secs(60)).await?;
        let confidence = regex::Regex::new(r"confere: ([0-9.]+)").ok()
            .and_then(|r| r.captures(&output)).and_then(|c| c[1].parse::<f64>().ok());
        // Saída 1 também vem de app fechado ou navegador sumido: sem a linha do Jev é erro, não "ainda não".
        match confidence {
            Some(confidence) if code == 0 || code == 1 => Ok(dict(json!({"sim": code == 0, "confianca": confidence}))),
            _ => Err(output),
        }
    }
}

/// O cofre (`vault/exec.rs`): o valor nunca entra no resultado nem no erro.
impl Tools {
    /// A sessão pela chave dela, nunca pelo pane nem pelo nome: é a chave que a dona prende à
    /// entrada temporária e à liberação.
    async fn vault_caller(&self, headers: &HeaderMap) -> Result<(String, String), String> {
        let (name, origin) = self.resolve(headers).await?;
        match (origin, header(headers, "x-hangar-key").filter(|k| !k.is_empty())) {
            (Origin::Key, Some(key)) => Ok((name, key)),
            _ => Err(VAULT_NO_KEY.into()),
        }
    }

    /// Para a tool que espera o dono: sessão sem o prazo longo cairia antes da resposta.
    async fn vault_agent(&self, name: &str) -> Result<exec::Agent, String> {
        let agent = exec::agent(&self.st, name).await?;
        if agent.lacks_deadline { return Err(exec::NO_DEADLINE.into()); }
        Ok(agent)
    }

    /// A origem do cartão; sem `CP_SERVER_ID` a máquina é a única e o nome não decide nada.
    fn vault_origin(&self) -> String { if self.server_id.is_empty() { "local".into() } else { self.server_id.clone() } }

    async fn request_secret(&self, headers: &HeaderMap, a: RequestSecret) -> Outcome {
        let deadline = tokio::time::Instant::now() + exec::BUDGET;
        let (name, key) = self.vault_caller(headers).await?;
        self.vault_agent(&name).await?;
        let owner = exec::Owner::of(&self.st)?;
        let req = AskReq { origin_server: self.vault_origin(), session_key: key, session: name, label: a.label, reason: a.reason,
            placeholder: a.placeholder };
        let id = owner.start_ask(req).await?;
        let status = match exec::Pending::new(owner, id).settle(deadline).await? {
            WaitStatus::Saved { entry, temporary } => return Ok(dict(json!({"status": "saved", "entry": entry, "temporary": temporary}))),
            WaitStatus::Declined => "declined",
            WaitStatus::TimedOut => "timed_out",            _ => return Err(exec::off_contract()),
        };
        Ok(dict(json!({"status": status})))
    }

    async fn vault_list(&self, headers: &HeaderMap) -> Outcome {
        self.caller(headers).await?;
        let entries = exec::Owner::of(&self.st)?.entries().await?;
        let rows: Vec<Value> = entries.iter().map(|e| {
            let mut row = json!({"name": e.name, "policy": e.policy});
            if let Some(username) = &e.username { row["username"] = json!(username); }
            if let Some(url) = &e.url { row["url"] = json!(url); }
            row
        }).collect();
        let mut result = text(serde_json::to_string_pretty(&rows).unwrap_or_default());
        result.structured_content = Some(json!({"result": rows}));
        Ok(result)
    }

    async fn vault_exec(&self, headers: &HeaderMap, a: VaultExec) -> Outcome {
        let deadline = tokio::time::Instant::now() + exec::BUDGET;
        if !exec::env_name_ok(&a.env) {
            return Err("erro_cofre_argumento_invalido: `env` é o nome da variável (letras, números e _, sem começar por número)".into());
        }
        if a.command.trim().is_empty() { return Err("erro_cofre_argumento_invalido: `command` vazio".into()); }
        // O dono aprova o comando que vê no cartão: o que não cabe nele não roda.
        if a.command.chars().count() > TARGET_CHARS {
            return Err(format!("erro_cofre_argumento_invalido: `command` com mais de {TARGET_CHARS} caracteres; ponha o resto num script"));
        }
        let (name, key) = self.vault_caller(headers).await?;
        let agent = self.vault_agent(&name).await?;
        let cwd = agent.cwd.clone().ok_or("erro_cofre_sem_pasta: a pasta de trabalho da sessão é desconhecida")?;
        let env = agent.env.ok_or(exec::NO_ENV)?;
        let owner = exec::Owner::of(&self.st)?;
        let req = UseReq { origin_server: self.vault_origin(), session_key: key, session: name, entry: a.entry, kind: UseKind::Exec,
            reason: a.reason, target: a.command.clone() };
        let value = exec::secret(&owner, req, deadline).await?;
        let asked = a.timeout_s.map_or(exec::DEFAULT_TIMEOUT, |s| Duration::from_secs(s.max(1)));
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() { return Err(exec::NO_ANSWER.into()); }
        let (exit, output) = exec::run(&a.command, &cwd, env, &a.env, &value, asked.min(left)).await;
        Ok(dict(json!({"exit": exit, "output": output})))
    }

    async fn vault_file(&self, headers: &HeaderMap, a: VaultFile) -> Outcome {
        let deadline = tokio::time::Instant::now() + exec::BUDGET;
        let (name, key) = self.vault_caller(headers).await?;
        self.vault_agent(&name).await?;
        let dir = files::base().ok_or("erro_cofre_sem_pasta_privada: o servidor não tem XDG_RUNTIME_DIR (Linux) nem LOCALAPPDATA (Windows)")?;
        let owner = exec::Owner::of(&self.st)?;
        let req = UseReq { origin_server: self.vault_origin(), session_key: key.clone(), session: name.clone(), entry: a.entry,
            kind: UseKind::File, reason: a.reason, target: "arquivo".into() };
        let value = exec::secret(&owner, req, deadline).await?;
        files::sweep_live(&self.st).await;
        let path = tokio::task::spawn_blocking(move || files::write(&dir, &key, &name, value.as_bytes())).await
            .map_err(|_| "erro_cofre_arquivo: a gravação foi interrompida".to_owned())?
            .map_err(|e| format!("erro_cofre_arquivo: o arquivo não foi gravado: {e}"))?;
        files::expire_later(path.clone());
        Ok(dict(json!({"file": path})))
    }

    /// O host da página do navegador da sessão; só http(s) tem domínio para comparar com a entrada.
    async fn fill_host(&self, home: &Path, name: &str) -> Result<String, String> {
        let url = nav::page_url(&self.http, home, name).await.map_err(|e| format!("erro_cofre_sem_navegador: {}", e.0))?;
        exec::host_of(&url).filter(|_| url.starts_with("https://") || url.starts_with("http://"))
            .ok_or_else(|| format!("erro_cofre_dominio_diferente: a página ({url}) não tem domínio"))
    }

    /// O usuário sai do resumo da entrada (o `vault_list` já o mostra); a senha, pelo pedido de uso
    /// `fill` com o domínio da página como alvo.
    async fn vault_fill(&self, headers: &HeaderMap, a: VaultFill) -> Outcome {
        let deadline = tokio::time::Instant::now() + exec::BUDGET;
        let password = match a.field.as_str() {
            "password" => true,
            "username" => false,
            _ => return Err("erro_cofre_argumento_invalido: `field` é \"username\" ou \"password\"".into()),
        };
        if !exec::ref_ok(&a.target) { return Err("erro_cofre_argumento_invalido: `ref` é a ref @eN de um snapshot".into()); }
        let (name, key) = self.vault_caller(headers).await?;
        let owner = exec::Owner::of(&self.st)?;
        let home = self.home();
        let long_enough = async { if password { self.vault_agent(&name).await.map(drop) } else { Ok(()) } };
        let ((), host, entries) = tokio::try_join!(long_enough, self.fill_host(&home, &name), owner.entries())?;
        let entry = entries.iter().find(|e| crate::vault::same_name(&e.name, &a.entry)).ok_or_else(|| exec::vault_text(VaultError::UnknownEntry))?;
        if let Some(url) = entry.url.as_deref().filter(|u| !u.trim().is_empty()) && !exec::same_site(url, &host) {
            return Err(format!("erro_cofre_dominio_diferente: a página é de {host} e a senha é de {url}"));
        }
        let value = if password {
            let req = UseReq { origin_server: self.vault_origin(), session_key: key, session: name.clone(), entry: a.entry, kind: UseKind::Fill,
                reason: a.reason, target: host.clone() };
            exec::secret(&owner, req, deadline).await?
        } else {
            Zeroizing::new(entry.username.clone().ok_or("erro_cofre_sem_usuario: esta senha não tem usuário cadastrado")?)
        };
        let lock = self.nav_lock(&name);
        let _held = lock.lock().await;
        // A página pode ter navegado durante a aprovação: o valor só entra no domínio aprovado.
        let now = self.fill_host(&home, &name).await?;
        if now != host { return Err(format!("erro_cofre_dominio_diferente: a página saiu de {host} para {now} durante a aprovação")); }
        let answer = nav::fill_secret(&self.http, &home, &name, &[a.target, a.field], &value).await
            .map_err(|e| format!("erro_cofre_sem_navegador: {}", e.0))?;
        let answer = exec::mask(&answer, &value);
        if answer.starts_with("ok:") { return Ok(text("preenchido")); }
        let detail = answer.strip_prefix("erro:").map_or(answer.as_str(), str::trim);
        Err(if detail.starts_with("erro_cofre_") { detail.to_owned() } else { format!("erro_cofre_preenchimento: {detail}") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_argument_lists_every_field_of_the_tool() {
        let raw = json!({"to": "s", "message": "oi"}).as_object().unwrap().clone();
        assert_eq!(args::<Send>("send", raw).err().as_deref(),
            Some("argumentos inválidos para send: missing field `alvo`. Campos: alvo (string), texto (string), tmux (boolean, opcional)"));
    }

    fn child(linked_by: Option<&str>, peers: &[&str]) -> Sidecar {
        Sidecar { peers: peers.iter().map(|p| (*p).to_owned()).collect(), linked_by: linked_by.map(str::to_owned), ..Sidecar::default() }
    }

    #[test]
    fn only_the_linked_parent_still_in_the_group_interrupts() {
        assert_eq!(link_refusal("pai", "filha", Some(&child(Some("pai"), &["pai", "outra"]))), None);
        let refused = |s: Option<&Sidecar>| link_refusal("pai", "filha", s).is_some_and(|e| e.starts_with("erro_interromper_nao_ligada:"));
        assert!(refused(Some(&child(Some("outra"), &["pai", "outra"]))), "filha de outra sessão");
        assert!(refused(Some(&child(None, &["pai"]))), "par comum, sem vínculo");
        assert!(refused(Some(&child(Some("pai"), &["outra"]))), "pai saiu do grupo");
        assert!(refused(None), "sem arquivo de grupo");
    }

    #[test]
    fn the_browser_tool_cannot_send_fill_secret() {
        assert!(verb_refusal("fill-secret").is_some_and(|e| e.starts_with("erro_cofre_so_vault_fill:")));
        assert!(verb_refusal("fill").is_none());
        assert!(verb_refusal("inventado").is_some_and(|e| e.starts_with("verbo desconhecido")));
    }

    #[test]
    fn voice_self_and_remote_are_refused_before_the_link() {
        assert!(interrupt_caller_refusal("voz", true, "filha").is_some());
        assert!(interrupt_caller_refusal("pai", false, "pai").is_some());
        assert!(interrupt_caller_refusal("pai", false, "srv::filha").is_some());
        assert_eq!(interrupt_caller_refusal("pai", false, "filha"), None);
    }

    fn new_args(v: Value) -> NewSession { serde_json::from_value(v).unwrap() }

    fn peers_with_lab(dir: &Path, base_url: &str) -> PeerClient {
        let file = dir.join("peers.json");
        std::fs::write(&file, json!({"lab": {"base_url": base_url, "token": "tok-lab"}}).to_string()).unwrap();
        PeerClient::new(crate::groups::peers::PeerBook::new(Some(file)))
    }

    #[test]
    fn server_prefix_is_split_once() {
        assert_eq!(remote_target("lab::filha").unwrap(), Some(("lab".into(), "filha".into())));
        assert_eq!(remote_target("filha").unwrap(), None);
        for bad in ["::filha", "lab::", "a::b::c"] { assert!(remote_target(bad).is_err(), "{bad}"); }
    }

    /// Lá recebe só o explícito: nada de `creator`, `inherit` ou conta daqui; a conta e a pasta são resolvidas lá.
    #[tokio::test]
    async fn remote_session_posts_to_the_peer_only_what_was_passed() {
        use axum::routing::{get, post};
        // O axum do projeto não tem a feature `json`.
        let reply = |v: Value| ([(axum::http::header::CONTENT_TYPE, "application/json")], v.to_string());
        let seen: Arc<Mutex<Option<(String, Value)>>> = Arc::default();
        let sink = seen.clone();
        let app = Router::new()
            .route("/api/claude-configs", get(move || async move { reply(json!([
                {"path": "/home/r/.claude", "label": "default", "active": true},
                {"path": "/home/r/.claude-work", "label": "trabalho", "active": false}])) }))
            .route("/api/sessions", post(move |h: HeaderMap, b: String| {
                let sink = sink.clone();
                async move {
                    let auth = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
                    *sink.lock().unwrap() = Some((auth, serde_json::from_str(&b).unwrap()));
                    reply(json!({"name": "filha", "cwd": "/home/r", "provider": "claude", "headless": true,
                        "config_dir": "/home/r/.claude-work", "account_source": "explicit"}))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let peers = peers_with_lab(dir.path(), &format!("http://{addr}"));
        let a = new_args(json!({"nome": "filha", "conta": "work", "model": "opus", "headless": true, "inherit": false,
            "initial_prompt": "oi"}));
        let result = remote_session(&peers, "lab", a).await.unwrap();
        let (auth, body) = seen.lock().unwrap().take().expect("POST /api/sessions lá");
        assert_eq!(auth, "Bearer tok-lab");
        assert_eq!(body, json!({"name": "filha", "cwd": "~", "model": "opus", "headless": true,
            "config_dir": "/home/r/.claude-work", "initial_prompt": "oi"}));
        let out = result.structured_content.unwrap();
        assert_eq!((out["server"].clone(), out["name"].clone(), out["config_dir"].clone()),
            (json!("lab"), json!("filha"), json!("/home/r/.claude-work")));
    }

    #[tokio::test]
    async fn unknown_server_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let peers = peers_with_lab(dir.path(), "http://127.0.0.1:9");
        let err = remote_session(&peers, "vps", new_args(json!({"nome": "filha", "cwd": "/tmp"}))).await.unwrap_err();
        assert!(err.contains("'vps'") && err.contains("peers.json"), "{err}");
    }

    #[tokio::test]
    async fn link_and_inherit_are_refused_on_another_machine() {
        let peers = PeerClient::new(crate::groups::peers::PeerBook::new(None));
        let err = remote_session(&peers, "lab", new_args(json!({"nome": "filha", "cwd": "/tmp", "link": true}))).await.unwrap_err();
        assert!(err.starts_with("erro_link_remoto:"), "{err}");
        let err = remote_session(&peers, "lab", new_args(json!({"nome": "filha", "cwd": "/tmp", "inherit": true}))).await.unwrap_err();
        assert!(err.contains("inherit"), "{err}");
    }

    #[test]
    fn remote_account_comes_from_the_peer_configs() {
        let rows = [json!({"path": "C:\\Users\\r\\.claude", "label": "default"}), json!({"path": "C:\\Users\\r\\.claude-work", "label": "trabalho"})];
        for conta in ["work", "trabalho", ".claude-work", "C:\\Users\\r\\.claude-work"] {
            assert_eq!(remote_account(&rows, conta).as_deref(), Some("C:\\Users\\r\\.claude-work"), "{conta}");
        }
        assert_eq!(remote_account(&rows, "outra"), None);
    }

    /// A retomada desta máquina leva os avisos (troca de conta por cota) como a de outra.
    #[test]
    fn a_resume_carries_the_warnings_here_and_there() {
        let info = json!({"name": "s", "cwd": "/w", "provider": "codex", "headless": true, "avisos": ["conta Codex com 99% de uso; a sessão nasceu em b"]});
        for server in [None, Some("lab")] {
            let out = resumed(&info, server).structured_content.unwrap();
            assert_eq!(out["avisos"], info["avisos"], "{server:?}");
            assert_eq!(out.get("server").and_then(Value::as_str), server);
        }
        assert!(resumed(&json!({"name": "s", "avisos": []}), None).structured_content.unwrap().get("avisos").is_none());
    }
}
