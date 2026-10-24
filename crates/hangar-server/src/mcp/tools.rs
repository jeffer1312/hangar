//! As 16 tools. Cada uma chama a rota pública que o `hangar-send`/`hangar-preview` chama, pelo
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
use crate::routes::AppState;
use crate::transcript::pyjson::dumps_unicode;

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
    serde_json::from_value(Value::Object(raw)).map_err(|e| format!("argumentos inválidos para {tool}: {e}"))
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
struct NewSession {
    nome: String, cwd: String, provider: Option<String>, engine: Option<String>, model: Option<String>, effort: Option<String>,
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

    async fn new_session(&self, headers: &HeaderMap, a: NewSession) -> Outcome {
        let (me, voice) = self.caller(headers).await?;
        if a.headless == Some(true) && let Some(provider) = a.provider.as_deref().filter(|p| !matches!(*p, "claude" | "codex")) {
            return Err(format!("headless só vale com provider claude ou codex (veio: {provider})"));
        }
        if let Some(sid) = a.resume.as_deref().filter(|s| !s.is_empty()) { return self.resume_session(sid, a.clone_for_resume()).await; }
        let mut body = json!({"name": a.nome, "cwd": a.cwd});
        let optional = [("provider", a.provider.map(Value::from)), ("engine", a.engine.map(Value::from)), ("model", a.model.map(Value::from)),
            ("effort", a.effort.map(Value::from)), ("permission_mode", a.permissao.map(Value::from)), ("headless", a.headless.map(Value::from)),
            ("config_dir", a.conta.map(Value::from)), ("jev", a.jev.map(Value::from)), ("service_tier", a.service_tier.map(Value::from)),
            ("branch", a.branch.filter(|b| !b.is_empty()).map(Value::from)), ("base", a.base.filter(|b| !b.is_empty()).map(Value::from)),
            ("codex_account", a.codex_account.map(Value::from)), ("subagent_model", a.subagent_model.map(Value::from)),
            ("initial_prompt", a.initial_prompt.map(Value::from)), ("engine_account", a.engine_account.map(Value::from)),
            ("omp_profile", a.omp_profile.map(Value::from)), ("claude_customizations", a.claude_customizations),
            ("inherit", a.inherit.map(Value::from)), ("read_only", a.read_only.then_some(Value::Bool(true))),
            ("new_branch", a.new_branch.then_some(Value::Bool(true))), ("remember_provider", a.remember_provider.then_some(Value::Bool(true))),
            ("link", a.link.then_some(Value::Bool(true))),
            // A voz não tem conta nem modo para herdar: o omitido fica com o padrão do servidor.
            ("creator", (!voice).then_some(Value::from(me)))];
        for (key, value) in optional {
            if let Some(value) = value { body[key] = value; }
        }
        let info = self.ok(Method::POST, "/api/sessions", Some(body)).await?;
        let mut out = json!({"name": info["name"], "cwd": info["cwd"], "provider": info["provider"], "headless": info["headless"],
            // Volta na resposta para que herdar errado nunca passe despercebido.
            "config_dir": info["config_dir"], "account_source": info["account_source"]});
        if info["avisos"].as_array().is_some_and(|a| !a.is_empty()) { out["avisos"] = info["avisos"].clone(); }
        // A sessão abriu: o erro diz isso e leva a resposta inteira, para nada se perder.
        if let Some(e) = info.get("link_error").filter(|e| !e.is_null()) {
            return Err(format!("sessão '{}' abriu SEM a ligação: {}: {} — {out}",
                info["name"].as_str().unwrap_or_default(), e["code"].as_str().unwrap_or_default(), e["detail"].as_str().unwrap_or_default()));
        }
        Ok(dict(out))
    }

    /// `resume`: a conversa volta pela rota do Arquivo (`-` = a pasta que a guarda), com nome e superfície.
    async fn resume_session(&self, sid: &str, a: Result<Value, String>) -> Outcome {
        let body = a?;
        let info = self.ok(Method::POST, &format!("/api/archive/-/{}/resume", segment(sid)), Some(body)).await?;
        Ok(dict(json!({"name": info["name"], "cwd": info["cwd"], "provider": info["provider"], "headless": info["headless"]})))
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
        if !NAV_VERBS.contains(&verb) { return Err(format!("verbo desconhecido: {verb} (aceitos: {})", NAV_VERBS.join(", "))); }
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
                && !matches!(k.as_str(), "CP_AUTH_TOKEN" | "HANGAR_INTERNAL_SECRET" | "HANGAR_RUNTIME_INSTANCE"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn voice_self_and_remote_are_refused_before_the_link() {
        assert!(interrupt_caller_refusal("voz", true, "filha").is_some());
        assert!(interrupt_caller_refusal("pai", false, "pai").is_some());
        assert!(interrupt_caller_refusal("pai", false, "srv::filha").is_some());
        assert_eq!(interrupt_caller_refusal("pai", false, "filha"), None);
    }
}
