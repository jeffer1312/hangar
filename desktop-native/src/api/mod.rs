pub mod dto;
pub mod device_install;
mod local;
mod recent;
pub use recent::RecentFailure;
pub mod route;
pub mod sse;
mod session_defaults;
mod tour_progress;

use std::time::Duration;
use reqwest::{Client, Method, Response, StatusCode, header};
use serde_json::{Value, json};
use url::Url;
use dto::{ChatEvent, CommandInfo, Delivery, PairResult, SessionInfo, UploadFile, Uploaded};

/// Só para medir a abertura de uma sessão: `HANGAR_NATIVE_OPEN_TRACE=1` escreve no stderr cada etapa, em ms desde o
/// clique (`open_trace_start`). Sem a variável, nada é formatado nem escrito.
pub fn open_trace(stage: impl FnOnce() -> String) {
    if !open_trace_on() { return; }
    let Some(start) = open_trace_clock().lock().ok().and_then(|clock| *clock) else { return };
    eprintln!("open_trace {:>8.1} {}", start.elapsed().as_secs_f64() * 1000., stage());
}

pub fn open_trace_start(name: &str) {
    if !open_trace_on() { return; }
    if let Ok(mut clock) = open_trace_clock().lock() { *clock = Some(std::time::Instant::now()); }
    open_trace(|| format!("select {name}"));
}

pub fn open_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HANGAR_NATIVE_OPEN_TRACE").is_some())
}

fn open_trace_clock() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static CLOCK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    &CLOCK
}

pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
const UPLOAD_SECONDS: u64 = 180;
/// Reabrir a sessão: o servidor espera até 5 min o Claude novo e, se falhar, mais 5 min a volta.
pub const REOPEN_SECONDS: u64 = 720;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// `Memory` é o anexo ainda no compositor (nome, bytes): não passa pela rede.
pub enum Source { Upload(String), Cited(String), Workspace(String), Transcript(String, usize), Remote(String), Memory(String, Shared) }

/// Bytes de um anexo na chave do cache do visor: iguais pelo endereço, não pelo conteúdo. Comparar e fazer hash de uma
/// imagem inteira a cada abertura custava milissegundos, e o `Debug` imprimia os bytes no rastro.
#[derive(Clone)]
pub struct Shared(pub std::sync::Arc<Vec<u8>>);

impl PartialEq for Shared { fn eq(&self, other: &Self) -> bool { std::sync::Arc::ptr_eq(&self.0, &other.0) } }
impl Eq for Shared {}
impl std::hash::Hash for Shared { fn hash<H: std::hash::Hasher>(&self, state: &mut H) { std::sync::Arc::as_ptr(&self.0).hash(state) } }
impl std::fmt::Debug for Shared { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{} bytes", self.0.len()) } }

/// Resposta da rota do Mermaid: a imagem com o tamanho de exibição (px CSS), o erro do diagrama ou a falha tentável.
#[derive(Clone, Debug, PartialEq)]
pub enum MermaidReply {
    Image { png: Vec<u8>, width: f32, height: f32 },
    /// 422: a mensagem do parser.
    Invalid(String),
    /// Vale tentar de novo: 503 (com o motivo do servidor), rede ou prazo.
    Unavailable(MermaidRetry),
    /// Outra tentativa daria o mesmo resultado.
    Refused(MermaidRefusal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MermaidRetry { Busy(String), Offline, Timeout }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MermaidRefusal {
    BadRequest,
    Unauthorized,
    Gone,
    /// 404 (servidor sem a rota ou convidado) e 403: a rota não é para este acesso; a tela mostra só o código.
    Hidden,
    /// Resposta fora do contrato (outro status, PNG sem tamanho).
    Other,
}

/// O status de uma resposta de erro do Mermaid; `message` é o motivo que o 503 traz.
pub fn mermaid_failure(status: u16, message: String) -> MermaidReply {
    match status {
        422 => MermaidReply::Invalid(message),
        // 502 e 504 vêm do proxy no caminho e passam: indisponível, como o 503.
        502..=504 => MermaidReply::Unavailable(MermaidRetry::Busy(message)),
        400 => MermaidReply::Refused(MermaidRefusal::BadRequest),
        401 => MermaidReply::Refused(MermaidRefusal::Unauthorized),
        410 => MermaidReply::Refused(MermaidRefusal::Gone),
        403 | 404 => MermaidReply::Refused(MermaidRefusal::Hidden),
        _ => MermaidReply::Refused(MermaidRefusal::Other),
    }
}

// `plain` busca mídia de terceiros: nunca leva o token do servidor.
#[derive(Clone)]
pub struct Api { client: Client, plain: Client, base: Url, token: String }

/// Escolha capturada antes do ditado; a resposta não troca seu modo, modelo ou destino.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationOptions {
    pub mode: String,
    pub model: Option<String>,
    pub generation: Option<String>,
    pub account: Option<String>,
    pub rust_capable: bool,
    pub include_recent_messages: bool,
}
impl Default for DictationOptions {
    fn default()->Self {Self{mode:"none".into(),model:None,generation:None,account:None,rust_capable:false,include_recent_messages:false}}
}

impl Api {
    pub fn server_address(&self) -> String {
        format!("{}{}", self.base.origin().ascii_serialization(), self.base.path().trim_end_matches('/'))
    }

    /// Falhas recentes deste servidor (até 50, mais recentes primeiro), pelo endereço salvo e pela rota de agora.
    // shortcut: a rede local de uma rota já esquecida não entra; guardar por identidade se isso fizer falta.
    pub fn recent_failures(&self) -> Vec<RecentFailure> {
        recent::recent(&[recent::origin(&self.base), recent::origin(&self.route())])
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub status: Option<u16>,
    pub detail: String,
    pub retry_after: Option<u64>,
    pub uncertain: bool,
    /// O código do envelope `{code, params, msg}` da resposta, quando veio: quem precisa decidir pela recusa
    /// (e não pelo status) olha aqui, porque o `detail` já é a frase traduzida.
    pub code: Option<String>,
}

impl Failure {
    pub fn local(detail: impl Into<String>) -> Self {
        Self { status: None, detail: detail.into(), retry_after: None, uncertain: false, code: None }
    }
    fn transport(post: bool) -> Self {
        Self { uncertain: post, ..Self::local(if post { "delivery_uncertain" } else { "network_error" }) }
    }
    /// Sem resposta: entra nas falhas recentes do servidor com status 0.
    fn lost(method: &str, error: &reqwest::Error, post: bool) -> Self {
        if let Some(url) = error.url() { recent::record(method, url, 0, Some("transport")); }
        Self::transport(post)
    }
}

/// O `code` do envelope `{"detail": {code, params, msg}}`; `detail` em texto ou em lista não tem código.
fn envelope_code(body: Option<&Value>) -> Option<String> {
    body?.get("detail")?.get("code")?.as_str().map(str::to_owned)
}

/// Recusa ou aviso de grupo `{code, params}`: a frase do web pelo código.
pub fn group_text(envelope: &Value) -> Option<String> {
    let code = envelope.get("code")?.as_str()?;
    if !(code.starts_with("erro_grupo_") || code.starts_with("erro_pareamento_") || matches!(code, "groups_need_rust_server"
        | "erro_contrato_mudou" | "erro_link_remoto" | "erro_sessao_nao_pareada" | "erro_sessao_sem_grupo" | "erro_peer_nao_avisado"
        | "erro_group_message_tempestade")) { return None; }
    envelope_text(envelope)
}

/// `{code, params}` pela frase do web; `None` = código sem frase.
fn envelope_text(envelope: &Value) -> Option<String> {
    let code = envelope.get("code")?.as_str()?;
    let params = envelope.get("params").and_then(Value::as_object)
        .map(|p| p.iter().map(|(k, v)| (k.clone(), param_text(v))).collect()).unwrap_or_default();
    crate::i18n::tr_web(code, &params)
}

/// Parâmetro como o `fmtParam` do web: lista com `; `, envelope traduzido (sem frase, o `msg`), `{sessao, erro}` como `sessao: erro`.
fn param_text(v: &Value) -> String {
    match v {
        Value::Array(items) => items.iter().map(param_text).collect::<Vec<_>>().join("; "),
        Value::String(s) => s.clone(),
        Value::Object(o) if o.get("code").is_some_and(Value::is_string) => envelope_text(v)
            .or_else(|| o.get("msg").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| o["code"].as_str().unwrap_or_default().to_owned()),
        Value::Object(o) if o.contains_key("sessao") && o.contains_key("erro") => format!("{}: {}", param_text(&o["sessao"]), param_text(&o["erro"])),
        other => other.to_string(),
    }
}

/// Aviso de uma resposta que deu certo: a frase traduzida; código sem frase fica com o texto do servidor.
pub fn warning_text(warning: &Value) -> String {
    match warning {
        Value::String(text) => text.clone(),
        _ => envelope_text(warning).or_else(|| warning.get("msg").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| warning.to_string()),
    }
}

/// A chave da frase de um código da abertura ou da troca de vida da sessão, e a da família dele.
fn session_life_key(code: &str) -> Option<(&str, Option<&'static str>)> {
    let family = [("lifecycle_", "session_lifecycle_failed"), ("relaunch_", "session_relaunch_failed"), ("heal_", "session_heal_failed")]
        .into_iter().find(|(prefix, _)| code.starts_with(prefix)).map(|(_, key)| key);
    match code {
        "session_patch_stale" | "session_patch_owner" | "session_patch_field" | "session_clear_cano_pid" => Some(("session_record_failed", None)),
        "codex_terminal_starting" | "runtime_unhealthy" | "erro_cliproxy_conta" | "erro_codex_catalogo_invalido" | "erro_codex_escolha_invalida"
        | "erro_codex_nao_reiniciou" | "erro_codex_reiniciando" | "erro_conta_criadora" | "erro_corpo_invalido" | "erro_criadora_ilegivel"
        | "erro_modo_desconhecido" | "erro_modo_padrao_ilegivel" | "erro_perfil_so_omp" | "erro_permissao_ocupada" | "erro_permissao_picker"
        | "erro_plano_fora_da_tela" | "erro_recarregar_so_sem_terminal" | "erro_nome_em_uso" | "erro_cwd_inexistente"
        | "erro_config_dir_invalido" | "erro_fora_da_pasta" | "erro_motor_invalido" | "erro_motor_sem_claude" | "erro_permissao_invalida"
        | "erro_permissao_so_claude" | "erro_provedor_offline" | "erro_provider_sessao_invalido" | "erro_conta_reconciliacao_falhou"
        | "erro_grupo_limpeza_falhou" | "erro_troca_modo" | "erro_recarregar" | "erro_retomar" | "erro_recarregar_sem_terminal" => Some((code, None)),
        _ => family.map(|key| (code, Some(key))),
    }
}

fn failure_detail(body: Option<Value>, status: u16) -> String {
    body.and_then(|value| value.get("detail").and_then(|detail| match detail {
        Value::String(message) => Some(message.clone()),
        Value::Object(fields) => {
            let msg = fields.get("msg").and_then(Value::as_str).filter(|message| !message.is_empty());
            if let Some(code @ ("session_transfer_source_changed" | "session_transfer_restore_failed")) = fields.get("code").and_then(Value::as_str) {
                return Some(msg.map_or_else(|| code.to_owned(), |msg| format!("{code}: {msg}")));
            }
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("session_transfer_")) {
                let params = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                    .filter(|(key, _)| matches!(key.as_str(), "model" | "effort" | "account" | "phase" | "limit" | "estimated"))
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            // Configurações do servidor e ocultar da lista usam a frase compartilhada, com os parâmetros dela.
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("config_sync_") || code.starts_with("update_channel_")
                || code.starts_with("session_hidden_") || code.starts_with("session_auto_hide_")) {
                let params = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            if let Some(message) = group_text(detail) { return Some(message); }
            // Par externo, botões dos mods, abertura de sessão e conta presa: a frase do web pelo código; o `detalhe`
            // de uma recusa e o rótulo do botão vêm nos parâmetros.
            if let Some(code) = fields.get("code").and_then(Value::as_str)
                .filter(|code| code.starts_with("erro_par_") || code.starts_with("erro_mod_") || code.starts_with("session_open_")
                    || matches!(*code, "erro_resume_so_claude" | "erro_session_id_invalido" | "erro_conta_sem_login" | "erro_sem_conta_destino"
                        | "erro_liberar_sessao" | "erro_liberar_conta")) {
                let params = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            // Abertura e troca de vida da sessão no Rust: a frase do web; etapa interna sem frase própria leva a da família.
            if let Some((key, family)) = fields.get("code").and_then(Value::as_str).and_then(session_life_key) {
                let params = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
                if let Some(message) = crate::i18n::tr_web(key, &params).or_else(|| family.and_then(|f| crate::i18n::tr_web(f, &params))) {
                    return Some(message);
                }
            }
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("erro_run_code_")) {
                let params = fields.get("params").and_then(Value::as_object).map(|p| p.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned))).collect()).unwrap_or_default();
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            // Comando de barra recusado: com mensagens esperando no painel, a frase própria (o motivo cru não explica
            // nada); os outros motivos vão dentro da frase compartilhada, como no web.
            if fields.get("code").and_then(Value::as_str) == Some("erro_comando_nao_executado") {
                let param = |name: &str| fields.get("params").and_then(|p| p.get(name)).and_then(Value::as_str).unwrap_or("").to_owned();
                if param("motivo") == "queue_waiting" { return Some(crate::i18n::tr_shared("command_refused_queue_waiting", &[])); }
                let params = std::collections::HashMap::from([("comando".to_owned(), param("comando")), ("motivo".to_owned(), param("motivo"))]);
                if let Some(message) = crate::i18n::tr_web("erro_comando_nao_executado", &params) { return Some(message); }
            }
            // Pergunta sem identidade, trocada ou ilegível: a frase do web no idioma do app, não o `msg` do servidor.
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("claude_") || code.starts_with("erro_interacao_")) {
                if let Some(message) = crate::i18n::tr_web(code, &std::collections::HashMap::new()) { return Some(message); }
            }
            // Custos e uso no Rust (503 do dono único): a frase do web pelo código, que já traz o código.
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("costs_") || *code == "internal_info") {
                // `internal_info` é o mesmo código do histórico, com a frase de lá.
                let key = if code == "internal_info" { "history_internal_info" } else { code };
                if let Some(message) = crate::i18n::tr_web(key, &std::collections::HashMap::new()) { return Some(message); }
            }
            // Mensagem direta ao subagente: a frase pelo código, com o `reason` da recusa do Claude Code dentro.
            if let Some(code) = fields.get("code").and_then(Value::as_str).filter(|code| code.starts_with("erro_subagente_")) {
                let reason = fields.get("reason").or_else(|| fields.get("params").and_then(|p| p.get("reason"))).and_then(Value::as_str).or(msg).unwrap_or("");
                let params = std::collections::HashMap::from([("reason".to_owned(), reason.to_owned())]);
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            // Atalhos do projeto: a frase do web pelo código, com o motivo (`params.detalhe`) dentro; sem a frase, o `msg`.
            if let Some(code @ ("erro_project_shortcuts" | "erro_project_shortcuts_projeto" | "erro_project_shortcuts_arquivo" | "erro_shortcut_pasta")) = fields.get("code").and_then(Value::as_str) {
                let reason = fields.get("params").and_then(|p| p.get("detalhe")).and_then(Value::as_str).or(msg).unwrap_or("");
                let params = std::collections::HashMap::from([("detalhe".to_owned(), reason.to_owned())]);
                if let Some(message) = crate::i18n::tr_web(code, &params) { return Some(message); }
            }
            fields.get("code").and_then(Value::as_str)
                // A busca depende de params.msg; sem transportar parâmetros, conserva a mensagem.
                .filter(|code| (code.starts_with("erro_arq_") && *code != "erro_arq_busca_falhou") || code.starts_with("erro_git_folder_")
                    || code.starts_with("erro_convite_") || *code == "erro_fora_do_convite")
                .or(msg)
                .or_else(|| fields.get("code").and_then(Value::as_str)).map(str::to_owned)
        }
        // Recusa de validação (422): uma lista de `{msg}`, uma por campo.
        Value::Array(items) => Some(items.iter().filter_map(|item| item.get("msg").and_then(Value::as_str)).collect::<Vec<_>>().join("; "))
            .filter(|message| !message.is_empty()),
        _ => None,
    })).unwrap_or_else(|| format!("HTTP {status}"))
}

impl Api {
    pub fn new(address: &str, token: &str) -> Result<Self, Failure> {
        let mut base = Url::parse(address.trim()).map_err(|_| Failure::local("invalid_url"))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none()
            || !base.username().is_empty() || base.password().is_some() || base.query().is_some() || base.fragment().is_some() {
            return Err(Failure::local("invalid_url"));
        }
        if !base.path().ends_with('/') { base.set_path(&format!("{}/", base.path())); }
        let mut value = header::HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| Failure::local("invalid_token"))?;
        value.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value);
        let client = Client::builder().default_headers(headers).connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| Failure::local("network_error"))?;
        let plain = Client::builder().connect_timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::limited(3))
            .build().map_err(|_| Failure::local("network_error"))?;
        Ok(Self { client, plain, base, token: token.trim().to_owned() })
    }

    /// O endereço salvo, mesmo quando os pedidos vão pela rede local (`route`).
    pub fn identity(&self) -> String { self.base.as_str().to_owned() }

    pub fn endpoint(&self, session: Option<&str>, action: Option<&str>) -> Url {
        let mut url = self.route();
        let mut parts = url.path_segments_mut().expect("validated HTTP base");
        parts.pop_if_empty().push("api").push("sessions");
        if let Some(name) = session { parts.push(name); }
        if let Some(action) = action { parts.push(action); }
        drop(parts);
        url
    }

    async fn checked(response: Response, post: bool) -> Result<Response, Failure> {
        Self::checked_as(response, post, if post { "POST" } else { "GET" }).await
    }

    /// `method` só rotula a falha guardada para o relatório de diagnóstico.
    async fn checked_as(response: Response, post: bool, method: &str) -> Result<Response, Failure> {
        if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED { return Ok(response); }
        let status = response.status().as_u16();
        let url = response.url().clone();
        let retry_after = response.headers().get(header::RETRY_AFTER).and_then(|s| s.to_str().ok()).and_then(|s| s.parse().ok());
        let body = response.json::<Value>().await.ok();
        let code = envelope_code(body.as_ref());
        recent::record(method, &url, status, code.as_deref());
        let detail = failure_detail(body, status);
        Err(Failure { status: Some(status), detail: detail.chars().take(500).collect(), retry_after, uncertain: post && status >= 500, code })
    }

    pub async fn sessions(&self) -> Result<Vec<SessionInfo>, Failure> {
        let r = self.client.get(self.endpoint(None, None)).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn history(&self, name: &str, limit: usize, etag: Option<&str>) -> Result<History, Failure> {
        let mut url = self.endpoint(Some(name), Some("history"));
        url.query_pairs_mut().append_pair("limit", &limit.to_string());
        let mut req = self.client.get(url).timeout(Duration::from_secs(30));
        if let Some(tag) = etag { req = req.header(header::IF_NONE_MATCH, tag); }
        let r = Self::checked(req.send().await.map_err(|_| Failure::transport(false))?, false).await?;
        open_trace(|| format!("history headers {}", r.status().as_u16()));
        if r.status() == StatusCode::NOT_MODIFIED { return Ok(History { events: None, etag: etag.map(str::to_owned) }); }
        let etag = r.headers().get(header::ETAG).and_then(|h| h.to_str().ok()).map(str::to_owned);
        let body = r.bytes().await.map_err(|_| Failure::local("invalid_response"))?;
        open_trace(|| format!("history body {} bytes", body.len()));
        let events: Vec<ChatEvent> = serde_json::from_slice(&body).map_err(|_| Failure::local("invalid_response"))?;
        open_trace(|| format!("history parsed {} events", events.len()));
        Ok(History { events: Some(events), etag })
    }

    pub async fn send(&self, name: &str, text: &str) -> Result<Delivery, Failure> {
        self.send_input(name, json!({"text": text, "steer": false})).await
    }

    /// A orientação pertence à entrada deste pedido, sem promover mensagens que já esperavam.
    pub async fn send_direct(&self, name: &str, text: &str) -> Result<Delivery, Failure> {
        self.send_input(name, json!({"text": text, "steer": true})).await
    }

    // Texto vai direto ao turno em voo (Codex, sem terminal); a resposta não traz `delivered`.
    pub async fn steer_text(&self, name: &str, text: &str) -> Result<Delivery, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("steer")))
            .json(&json!({"text": text})).timeout(Duration::from_secs(60))
            .send().await.map_err(|_| Failure::transport(true))?;
        let value: Value = Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) { return Err(Failure::transport(true)); }
        Ok(Delivery { ok: true, delivered: true, ..Default::default() })
    }

    pub async fn send_decided(&self, name: &str, text: &str, task: &str, message: &str) -> Result<Delivery, Failure> {
        self.send_input(name, json!({"text": text, "jev_task": task, "jev_message": message})).await
    }

    async fn send_input(&self, name: &str, body: Value) -> Result<Delivery, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("input")))
            .json(&body).timeout(Duration::from_secs(60))
            .send().await.map_err(|_| Failure::transport(true))?;
        let delivery: Delivery = Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))?;
        if !delivery.ok { return Err(Failure::transport(true)); }
        Ok(delivery)
    }

    pub async fn commands(&self, name: &str) -> Result<Vec<CommandInfo>, Failure> {
        let r = self.client.get(self.endpoint(Some(name), Some("commands"))).timeout(Duration::from_secs(30))
            .send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    // Corpo cru, sem multipart. Queda depois de mandar é incerteza: o arquivo pode ter sido salvo.
    pub async fn upload(&self, name: &str, filename: &str, mime: &str, bytes: Vec<u8>) -> Result<Uploaded, Failure> {
        let r = self.client.post(self.endpoint(Some(name), Some("upload")))
            .header(header::CONTENT_TYPE, mime)
            .header("X-Filename", crate::composer::encode_component(if filename.is_empty() { "arquivo" } else { filename }))
            .body(bytes).timeout(Duration::from_secs(UPLOAD_SECONDS))
            .send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Áudio do ditado nos anexos (`?audio_only=1`): o backend não trata como vídeo (quadros e transcrição de fala).
    pub async fn upload_audio(&self, name: &str, filename: &str, bytes: Vec<u8>) -> Result<Uploaded, Failure> {
        let mut url = self.endpoint(Some(name), Some("upload"));
        url.query_pairs_mut().append_pair("audio_only", "1");
        let r = self.client.post(url)
            .header(header::CONTENT_TYPE, crate::composer::mime_for(filename))
            .header("X-Filename", crate::composer::encode_component(filename))
            // 30 min de ditado dão ~58 MB: em 180 s só passariam acima de ~2,6 Mbit/s, abaixo de uma VPN em relay.
            .body(bytes).timeout(Duration::from_secs(600))
            .send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Sem `name` (nova conversa, antes de a sessão existir) só transcreve, sem guardar o áudio numa sessão.
    pub async fn transcribe(&self, name: Option<&str>, filename: &str, bytes: Vec<u8>, clean: bool, style: Option<&str>) -> Result<Value, Failure> {
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        let filename = if filename.is_empty() { "audio.wav" } else { filename };
        let request = self.client.post(self.transcribe_url(name, None, clean, style))
            .header(header::CONTENT_TYPE, crate::composer::mime_for(filename))
            .header("X-Filename", crate::composer::encode_component(filename))
            .body(bytes);
        Self::transcribed(request).await
    }

    /// Transcreve um áudio que já está nos anexos da sessão (`?arquivo=`, nome solto ou o caminho que o servidor
    /// devolveu): corpo vazio, nada é salvo de novo.
    pub async fn transcribe_saved(&self, name: &str, filename: &str, clean: bool, style: Option<&str>) -> Result<Value, Failure> {
        Self::transcribed(self.client.post(self.transcribe_url(Some(name), Some(filename), clean, style))).await
    }

    pub async fn dictate(&self,name:Option<&str>,filename:&str,bytes:Vec<u8>,style:Option<&str>,options:&DictationOptions)->Result<Value,Failure> {
        if bytes.len() as u64>MAX_BYTES{return Err(Failure::local("attach_too_big"));}
        let request=self.client.post(self.transcribe_url_with_options(name,None,true,style,options))
            .header(header::CONTENT_TYPE,crate::composer::mime_for(filename)).header("X-Filename",crate::composer::encode_component(filename)).body(bytes);
        Self::transcribed(request).await
    }

    pub async fn dictate_saved(&self,name:&str,filename:&str,style:Option<&str>,options:&DictationOptions)->Result<Value,Failure> {
        Self::transcribed(self.client.post(self.transcribe_url_with_options(Some(name),Some(filename),true,style,options))).await
    }

    pub async fn dictation_models(&self,session:&str,fresh:bool)->Result<Value,Failure> {
        let response=self.client.get(self.server_url(&["dictation","models"],&[("session",session),("fresh",if fresh{"true"}else{"false"})]))
            .timeout(Duration::from_secs(60)).send().await.map_err(|_|Failure::transport(false))?;
        Self::checked(response,false).await?.json().await.map_err(|_|Failure::local("invalid_response"))
    }

    pub async fn test_transcription_provider(&self, id: &str, filename: &str, bytes: Vec<u8>) -> Result<Value, Failure> {
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        Self::transcribed(self.client.post(self.server_url(&["transcription", "providers", id, "test"], &[]))
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header("X-Filename", crate::composer::encode_component(filename)).body(bytes)).await
    }

    fn transcribe_url(&self, name: Option<&str>, saved: Option<&str>, clean: bool, style: Option<&str>) -> Url {
        self.transcribe_url_with_options(name,saved,clean,style,&DictationOptions::default())
    }

    fn transcribe_url_with_options(&self,name:Option<&str>,saved:Option<&str>,clean:bool,style:Option<&str>,options:&DictationOptions)->Url {
        let mut url = match name {
            Some(name) => self.endpoint(Some(name), Some("transcribe")),
            None => self.server_url(&["dictation", "transcribe"], &[]),
        };
        let organize=clean&&options.rust_capable;
        url.query_pairs_mut().append_pair("limpar",if organize{"1"}else{"0"});
        if clean {
            url.query_pairs_mut().append_pair("organization_mode",&options.mode);
            if options.mode!="none" {
                url.query_pairs_mut().append_pair("include_recent_messages",if options.include_recent_messages{"true"}else{"false"});
                if let Some(style)=style.filter(|style|organize&&!style.is_empty()){url.query_pairs_mut().append_pair("estilo",style);}
                if let Some(model)=&options.model {url.query_pairs_mut().append_pair("organization_model",model);}
            }
            if let Some(generation)=&options.generation {url.query_pairs_mut().append_pair("generation",generation);}
            if let Some(account)=&options.account {url.query_pairs_mut().append_pair("organization_account",account);}
        }
        if let Some(file) = saved { url.query_pairs_mut().append_pair("arquivo", file); }
        url
    }

    async fn transcribed(request: reqwest::RequestBuilder) -> Result<Value, Failure> {
        // Cobre o ditado mais longo (30 min de áudio) mais a organização: o servidor responde só no fim.
        let r = request.timeout(Duration::from_secs(35 * 60)).send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn uploads(&self, name: &str) -> Result<Vec<UploadFile>, Failure> {
        #[derive(serde::Deserialize)]
        struct Listing { #[serde(default)] files: Vec<UploadFile> }
        let r = self.client.get(self.endpoint(Some(name), Some("uploads"))).timeout(Duration::from_secs(30))
            .send().await.map_err(|_| Failure::transport(false))?;
        let listing: Listing = Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))?;
        Ok(listing.files)
    }

    // Bytes de um anexo: do cofre, de caminho citado ou de imagem do transcript, autenticados; remoto, sem token.
    pub async fn fetch(&self, name: &str, source: &Source) -> Result<Vec<u8>, Failure> {
        let mut url = self.endpoint(Some(name), None);
        let mut client = &self.client;
        match source {
            Source::Upload(file) => { url.path_segments_mut().expect("validated HTTP base").extend(["uploads", file]); }
            Source::Cited(path) | Source::Workspace(path) => {
                url.path_segments_mut().expect("validated HTTP base").push("file");
                url.query_pairs_mut().append_pair("path", path);
                if matches!(source, Source::Workspace(_)) { url.query_pairs_mut().append_pair("root", "1"); }
            }
            Source::Transcript(id, index) => { url.path_segments_mut().expect("validated HTTP base").extend(["transcript-image", id, &index.to_string()]); }
            Source::Remote(address) => {
                url = Url::parse(address).ok().filter(|url| matches!(url.scheme(), "http" | "https")).ok_or_else(|| Failure::local("invalid_url"))?;
                client = &self.plain;
            }
            Source::Memory(_, bytes) => return Ok(bytes.0.to_vec()),
        }
        let r = client.get(url).timeout(Duration::from_secs(UPLOAD_SECONDS)).send().await.map_err(|_| Failure::transport(false))?;
        // Erro de terceiro não tem o corpo lido: nem memória, nem texto escolhido por ele na tela.
        if matches!(source, Source::Remote(_)) && !r.status().is_success() {
            let status = r.status().as_u16();
            return Err(Failure { status: Some(status), detail: failure_detail(None, status), retry_after: None, uncertain: false, code: None });
        }
        let r = Self::checked(r, false).await?;
        if r.content_length().is_some_and(|n| n > MAX_BYTES) { return Err(Failure::local("attach_too_big")); }
        // Resposta sem tamanho declarado para no teto enquanto chega.
        let mut body = Vec::new();
        let mut chunks = std::pin::pin!(r.bytes_stream());
        use futures::StreamExt;
        while let Some(chunk) = chunks.next().await {
            body.extend_from_slice(&chunk.map_err(|_| Failure::transport(false))?);
            if body.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        }
        Ok(body)
    }

    /// Página publicada: sem `extra` é a casca isolada (com `raw=1`, o HTML cru); `["shot"]` é a imagem. O 404 diz
    /// pelo `code` se a página expirou (`erro_pagina_expirou`) ou se o servidor não tem imagem (`erro_pagina_sem_imagem`).
    pub async fn page(&self, name: &str, id: &str, extra: &[&str], query: &[(&str, &str)]) -> Result<Vec<u8>, Failure> {
        let mut url = self.endpoint(Some(name), Some("pages"));
        url.path_segments_mut().expect("validated HTTP base").push(id).extend(extra);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        let r = self.client.get(url).timeout(Duration::from_secs(30)).send().await.map_err(|_| Failure::transport(false))?;
        Ok(Self::checked(r, false).await?.bytes().await.map_err(|_| Failure::transport(false))?.to_vec())
    }

    /// Diagrama desenhado pelo servidor (`POST /api/mermaid`). Só 503, rede e prazo podem passar numa nova tentativa.
    pub async fn mermaid(&self, source: &str, dark: bool, width: u32) -> MermaidReply {
        let url = self.server_url(&["mermaid"], &[]);
        let body = json!({"source": source, "theme": if dark { "dark" } else { "light" }, "width": width});
        let lost = |e: reqwest::Error| MermaidReply::Unavailable(if e.is_timeout() { MermaidRetry::Timeout } else { MermaidRetry::Offline });
        let r = match self.client.post(url).json(&body).timeout(Duration::from_secs(30)).send().await { Ok(r) => r, Err(e) => return lost(e) };
        let status = r.status();
        if status.is_success() {
            let size = |name: &str| r.headers().get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v > 0.);
            let (Some(width), Some(height)) = (size("x-hangar-css-width"), size("x-hangar-css-height")) else {
                return MermaidReply::Refused(MermaidRefusal::Other);
            };
            return match r.bytes().await {
                Ok(png) if !png.is_empty() => MermaidReply::Image { png: png.to_vec(), width, height },
                Ok(_) => MermaidReply::Refused(MermaidRefusal::Other),
                Err(e) => lost(e),
            };
        }
        let message = r.json::<Value>().await.ok().and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default().chars().take(500).collect::<String>();
        mermaid_failure(status.as_u16(), message)
    }

    pub async fn interrupt(&self, name: &str, clear: bool) -> Result<(), Failure> {
        let mut url = self.endpoint(Some(name), Some("interrupt"));
        url.query_pairs_mut().append_pair("clear", if clear { "true" } else { "false" });
        let r = self.client.post(url).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?;
        Ok(())
    }

    // Mutação sem retry: timeout ou queda depois de enviar vira incerteza, nunca repetição.
    pub async fn act(&self, name: &str, path: &[&str], body: Option<Value>, delete: bool, seconds: u64) -> Result<Value, Failure> {
        self.request(name, if delete { Method::DELETE } else { Method::POST }, path, None, body, seconds).await
    }

    /// Pedido na rota da sessão com método e consulta próprios; o `act` é o caso `POST`/`DELETE` sem consulta.
    pub async fn request(&self, name: &str, method: Method, path: &[&str], query: Option<(&str, &str)>, body: Option<Value>, seconds: u64)
        -> Result<Value, Failure> {
        let mut url = self.endpoint(Some(name), None);
        url.path_segments_mut().expect("validated HTTP base").extend(path);
        if let Some((key, value)) = query { url.query_pairs_mut().append_pair(key, value); }
        let label = method.clone();
        let mut req = self.client.request(method, url);
        if let Some(body) = body { req = req.json(&body); }
        let r = req.timeout(Duration::from_secs(seconds)).send().await.map_err(|e| Failure::lost(label.as_str(), &e, true))?;
        let r = Self::checked_as(r, true, label.as_str()).await?;
        r.json().await.map_err(|_| Failure::transport(true))
    }

    /// Junta `name` e `peers` num grupo (funde os grupos de todos). 409 = o grupo já tem outra tarefa; `replace_task` troca.
    pub async fn pair(&self, name: &str, peers: &[String], task: &str, replace_task: bool) -> Result<PairResult, Failure> {
        let body = json!({"peers": peers, "task": task, "replace_task": replace_task});
        self.act(name, &["pair"], Some(body), false, 60).await.map(|value| PairResult::from_value(&value))
    }

    /// `name` sai do grupo; os outros seguem juntos.
    pub async fn unpair(&self, name: &str) -> Result<PairResult, Failure> {
        self.act(name, &["pair"], None, true, 60).await.map(|value| PairResult::from_value(&value))
    }

    pub async fn share_create(&self, name: &str) -> Result<ShareCreated, ShareFailure> {
        self.post_with_prerequisites(name, "share", None).await
    }

    /// Mesmo túnel e mesmos pré-requisitos do compartilhamento: o 409 devolve o que falta.
    pub async fn create_pair_invite(&self, name: &str, level: PairLevel) -> Result<PairInvite, ShareFailure> {
        self.post_with_prerequisites(name, "pair-invite", Some(json!({"level": level.wire()}))).await
    }

    /// `level`: o que quem convidou terá aqui.
    pub async fn accept_pair(&self, name: &str, link: &str, level: PairLevel) -> Result<PairAccepted, ShareFailure> {
        self.post_with_prerequisites(name, "pair-accept", Some(json!({"link": link, "level": level.wire()}))).await
    }

    /// Nível que o par `alias` tem na sessão `name` (o "Liberar" e o pôr no grupo com acesso completo).
    pub async fn pair_access(&self, name: &str, alias: &str, level: PairLevel) -> Result<(), Failure> {
        self.act(name, &["pair-access"], Some(json!({"alias": alias, "level": level.wire()})), false, 30).await.map(|_| ())
    }

    /// Desfaz o par com `alias::session` dos dois lados: "Sair do pareamento" e "Desfazer par".
    pub async fn leave_external_pair(&self, alias: &str, session: &str) -> Result<(), Failure> {
        self.server_send_query(reqwest::Method::DELETE, &["external-pairs", alias], &[("session", session)], None, 30).await.map(|_| ())
    }

    // Rota que sobe o Funnel: as recusas de `share_refusal` têm tela própria, e o resto segue como falha comum. O
    // servidor só responde com o link do Funnel aberto de fora: até 20 s do `tailscale status` mais 3 min de conferência,
    // então o prazo daqui fica acima disso para não desistir na véspera da resposta.
    async fn post_with_prerequisites<T: serde::de::DeserializeOwned>(&self, name: &str, action: &str, body: Option<Value>) -> Result<T, ShareFailure> {
        let mut req = self.client.post(self.endpoint(Some(name), Some(action))).timeout(Duration::from_secs(220));
        if let Some(body) = body { req = req.json(&body); }
        let r = req.send().await.map_err(|_| ShareFailure::Other(Failure::transport(true)))?;
        let status = r.status().as_u16();
        if matches!(status, 404 | 405 | 409) {
            let body = r.json::<Value>().await.unwrap_or(Value::Null);
            if let Some(refusal) = share_refusal(status, &body) { return Err(refusal); }
            return Err(ShareFailure::Other(Failure { status: Some(status), detail: failure_detail(Some(body), status), retry_after: None, uncertain: false, code: None }));
        }
        Self::checked(r, true).await.map_err(ShareFailure::Other)?.json().await.map_err(|_| ShareFailure::Other(Failure::transport(true)))
    }

    /// Só consulta o que falta; não liga o Funnel.
    pub async fn share_prereqs(&self) -> Result<SharePrereqs, Failure> {
        let value = self.server_read(&["share", "prereqs"], &[], 15).await?;
        serde_json::from_value(value).map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn shares(&self, name: &str) -> Result<Vec<ShareEntry>, Failure> {
        let value = self.read(name, &["share"], &[], 15).await?;
        serde_json::from_value(value.get("shares").cloned().unwrap_or(Value::Null)).map_err(|_| Failure::local("invalid_response"))
    }

    /// `id` revoga um aparelho; `None` encerra todos os convites da sessão.
    pub async fn share_revoke(&self, name: &str, id: Option<&str>) -> Result<(), Failure> {
        let path: Vec<&str> = std::iter::once("share").chain(id).collect();
        self.act(name, &path, None, true, 20).await.map(|_| ())
    }

    /// Tarefa sugerida pelo fim da conversa das sessões (422 quando nenhuma tem conversa).
    pub async fn suggest_group_task(&self, sessions: &[String]) -> Result<String, Failure> {
        let value = self.server_send(reqwest::Method::POST, &["pair", "task-suggestion"], Some(json!({"sessions": sessions})), 120).await?;
        value.get("task").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| Failure::local("invalid_response"))
    }

    /// O mesmo prompt para várias sessões do servidor, pela esteira do `/input` de cada uma. Responde 200 com o resultado por
    /// sessão, na ordem pedida: `(nome, entrega, motivo da falha)`.
    pub async fn broadcast(&self, names: &[String], text: &str) -> Result<Vec<(String, Delivery, Option<String>)>, Failure> {
        let value = self.server_send(reqwest::Method::POST, &["broadcast"], Some(json!({"names": names, "text": text})), 120).await?;
        let results = value.get("results").and_then(Value::as_object).ok_or_else(|| Failure::transport(true))?;
        Ok(names.iter().map(|name| {
            let result = results.get(name);
            let flag = |key: &str| result.and_then(|r| r.get(key)).and_then(Value::as_bool) == Some(true);
            let error = result.and_then(|r| r.get("error")).filter(|e| !e.is_null()).map(|e| match e {
                Value::String(text) => text.clone(),
                _ => e.get("msg").or_else(|| e.get("code")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| e.to_string()),
            });
            (name.clone(), Delivery { ok: flag("ok"), delivered: flag("delivered"), ..Default::default() }, error)
        }).collect())
    }

    // Leitura sem efeito colateral: queda é rede, nunca incerteza.
    pub async fn read(&self, name: &str, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let mut url = self.endpoint(Some(name), None);
        url.path_segments_mut().expect("validated HTTP base").extend(path);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        let r = self.client.get(url).timeout(Duration::from_secs(seconds)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    pub async fn config(&self) -> Result<Value, Failure> {
        let mut url = self.route();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "config"]);
        let r = self.client.get(url).timeout(Duration::from_secs(15)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// `/api/<path>` do servidor (fora de uma sessão), com o método pedido.
    fn server_url(&self, path: &[&str], query: &[(&str, &str)]) -> Url {
        let mut url = self.route();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().push("api").extend(path);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        url
    }

    /// Leitura do servidor (cotação, diário, estado da atualização): queda é rede, nunca incerteza.
    pub async fn server_read(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.get(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|e| Failure::lost("GET", &e, false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// Arquivo inteiro do servidor (o diário), com o mesmo teto dos anexos.
    pub async fn server_bytes(&self, path: &[&str], seconds: u64) -> Result<Vec<u8>, Failure> {
        let r = self.client.get(self.server_url(path, &[])).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if r.content_length().is_some_and(|n| n > MAX_BYTES) { return Err(Failure::local("attach_too_big")); }
        let bytes = r.bytes().await.map_err(|_| Failure::transport(false))?;
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        Ok(bytes.to_vec())
    }

    /// Mutação no servidor, sem retry: queda depois de enviar é incerteza.
    pub async fn server_post(&self, path: &[&str], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.post(self.server_url(path, &[])).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    pub async fn server_post_query(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.post(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        Self::checked(r, true).await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Mutação (PUT, POST, DELETE), com ou sem corpo JSON, sem retry: queda depois de enviar é incerteza.
    pub async fn server_send(&self, method: reqwest::Method, path: &[&str], body: Option<Value>, seconds: u64) -> Result<Value, Failure> {
        self.server_send_query(method, path, &[], body, seconds).await
    }

    pub async fn server_send_query(&self, method: reqwest::Method, path: &[&str], query: &[(&str, &str)], body: Option<Value>,
        seconds: u64) -> Result<Value, Failure> {
        let label = method.clone();
        let mut req = self.client.request(method, self.server_url(path, query));
        if let Some(body) = body { req = req.json(&body); }
        let r = req.timeout(Duration::from_secs(seconds)).send().await.map_err(|e| Failure::lost(label.as_str(), &e, true))?;
        let r = Self::checked_as(r, true, label.as_str()).await?;
        // 204 não tem corpo para ler.
        if r.status() == StatusCode::NO_CONTENT { return Ok(Value::Null); }
        r.json().await.map_err(|_| Failure::transport(true))
    }

    /// DELETE de conta: o 409 de conta em uso com a lista do que a prende volta como `Removed::Held`.
    pub async fn remove_account(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Removed, Failure> {
        let r = self.client.delete(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        if r.status() == StatusCode::CONFLICT {
            let body = r.json::<Value>().await.ok();
            if let Some(holders) = body.as_ref().and_then(account_holders) { return Ok(Removed::Held(holders)); }
            let code = envelope_code(body.as_ref());
            return Err(Failure { status: Some(409), detail: failure_detail(body, 409).chars().take(500).collect(), retry_after: None, uncertain: false, code });
        }
        Self::checked_as(r, true, "DELETE").await?.json().await.map(Removed::Done).map_err(|_| Failure::transport(true))
    }

    /// Rota do hub de sincronização: a sessão ali é o cookie `cp_sync`, não o token. Devolve também o `cp_sync=…` que a
    /// resposta trouxer, pra quem chama guardar em memória e mandar de volta.
    pub async fn hub(&self, method: reqwest::Method, path: &[&str], query: &[(&str, &str)], body: Option<Value>, cookie: Option<&str>,
        seconds: u64) -> Result<(Value, Option<String>), Failure> {
        let (post, label) = (method != reqwest::Method::GET, method.clone());
        let mut req = self.client.request(method, self.server_url(path, query));
        if let Some(cookie) = cookie {
            let mut value = header::HeaderValue::from_str(cookie).map_err(|_| Failure::local("invalid_token"))?;
            value.set_sensitive(true);
            req = req.header(header::COOKIE, value);
        }
        if let Some(body) = body { req = req.json(&body); }
        let r = req.timeout(Duration::from_secs(seconds)).send().await.map_err(|_| Failure::transport(post))?;
        let r = Self::checked_as(r, post, label.as_str()).await?;
        let session = r.headers().get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next()).find(|v| v.starts_with("cp_sync=")).map(str::to_owned);
        Ok((r.json().await.map_err(|_| Failure::local("invalid_response"))?, session))
    }

    /// Retoma uma conversa do arquivo. Já aberta numa sessão viva, o servidor responde 409 `erro_conversa_viva` com o nome
    /// dela em `params.sessao`: volta como `Resumed::Live`, para quem chamou abrir essa sessão em vez de mostrar erro.
    pub async fn resume_archive(&self, project: &str, session_id: &str, body: Value) -> Result<Resumed, Failure> {
        let r = self.client.post(self.server_url(&["archive", project, session_id, "resume"], &[])).json(&body)
            .timeout(Duration::from_secs(120)).send().await.map_err(|_| Failure::transport(true))?;
        if r.status() == StatusCode::CONFLICT {
            let body = r.json::<Value>().await.ok();
            let live = body.as_ref().and_then(|b| b.get("detail")).filter(|d| d.get("code").and_then(Value::as_str) == Some("erro_conversa_viva"))
                .and_then(|d| d.pointer("/params/sessao")).and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned);
            if let Some(name) = live { return Ok(Resumed::Live(name)); }
            return Err(Failure { status: Some(409), detail: failure_detail(body, 409).chars().take(500).collect(), retry_after: None, uncertain: false, code: None });
        }
        let session = Self::checked(r, true).await?.json().await.map_err(|_| Failure::local("invalid_response"))?;
        Ok(Resumed::New(session))
    }

    /// DELETE com parâmetros na URL (cancelar o login do Codex leva a tentativa na query).
    pub async fn server_delete(&self, path: &[&str], query: &[(&str, &str)], seconds: u64) -> Result<Value, Failure> {
        let r = self.client.delete(self.server_url(path, query)).timeout(Duration::from_secs(seconds)).send().await
            .map_err(|_| Failure::transport(true))?;
        Self::checked_as(r, true, "DELETE").await?.json().await.map_err(|_| Failure::transport(true))
    }

    /// Manifesto da configuração compartilhada, com as etapas da leitura em `on` (`None` = Hangar sem etapas).
    pub async fn config_sync_manifest<F, Fut>(&self, on: F) -> Result<Value, Failure>
    where F: FnMut(Option<Value>) -> Fut, Fut: std::future::Future<Output = ()> {
        let req = self.client.get(self.server_url(&["config-sync", "manifest"], &[("stream", "1")]));
        Self::ndjson(req, false, 60, on).await
    }

    /// Pacote da origem (gzip). `keys` é o JSON `{item: [chaves]}` das entradas marcadas.
    pub async fn config_sync_bundle(&self, items: &str, keys: Option<&str>) -> Result<Vec<u8>, Failure> {
        let mut query = vec![("items", items)];
        if let Some(keys) = keys { query.push(("keys", keys)); }
        let r = self.client.get(self.server_url(&["config-sync", "bundle"], &query)).timeout(Duration::from_secs(180)).send().await
            .map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if r.content_length().is_some_and(|n| n > MAX_BYTES) { return Err(Failure::local("attach_too_big")); }
        let bytes = r.bytes().await.map_err(|_| Failure::transport(false))?;
        if bytes.len() as u64 > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
        Ok(bytes.to_vec())
    }

    /// Aplica o pacote no destino; o envio e a aplicação são o mesmo pedido, e as etapas chegam em `on`.
    pub async fn config_sync_apply<F, Fut>(&self, items: &str, bundle: Vec<u8>, on: F) -> Result<Value, Failure>
    where F: FnMut(Option<Value>) -> Fut, Fut: std::future::Future<Output = ()> {
        let req = self.client.post(self.server_url(&["config-sync", "apply"], &[("items", items), ("stream", "1")]))
            .header(header::CONTENT_TYPE, "application/gzip").body(bundle);
        Self::ndjson(req, true, 600, on).await
    }

    /// Resposta NDJSON: `progress` vai para `on`, `done` é o resultado e `error` a falha. Hangar antigo ignora `stream=1`
    /// e responde JSON inteiro: `on(None)` e o corpo é o resultado. `seconds` vale para a resposta e para cada silêncio.
    async fn ndjson<F, Fut>(req: reqwest::RequestBuilder, post: bool, seconds: u64, mut on: F) -> Result<Value, Failure>
    where F: FnMut(Option<Value>) -> Fut, Fut: std::future::Future<Output = ()> {
        use futures::StreamExt;
        let wait = Duration::from_secs(seconds);
        let cut = || if post { Failure { uncertain: true, ..Failure::local("shared_config_stream_cut") } } else { Failure::transport(false) };
        let r = tokio::time::timeout(wait, req.send()).await.map_err(|_| Failure::transport(post))?.map_err(|_| Failure::transport(post))?;
        let r = Self::checked(r, post).await?;
        let ndjson = r.headers().get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).is_some_and(|t| t.starts_with("application/x-ndjson"));
        if !ndjson {
            on(None).await;
            return tokio::time::timeout(wait, r.json()).await.ok().and_then(Result::ok).ok_or_else(cut);
        }
        let mut chunks = std::pin::pin!(r.bytes_stream());
        let mut buf: Vec<u8> = Vec::new();
        loop {
            while let Some(end) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=end).collect();
                let Ok(event) = serde_json::from_slice::<Value>(&line) else { continue };
                match event.get("type").and_then(Value::as_str) {
                    Some("progress") => on(Some(event)).await,
                    Some("done") => return Ok(event.get("result").cloned().unwrap_or(Value::Null)),
                    Some("error") => {
                        let status = event.get("status").and_then(Value::as_u64).unwrap_or(500) as u16;
                        let detail = failure_detail(Some(json!({"detail": event.get("detail")})), status);
                        return Err(Failure { status: Some(status), detail: detail.chars().take(500).collect(), retry_after: None, uncertain: false, code: None });
                    }
                    _ => {}
                }
            }
            match tokio::time::timeout(wait, chunks.next()).await {
                Ok(Some(Ok(chunk))) => buf.extend_from_slice(&chunk),
                Ok(None) => {
                    // Última linha sem `\n` ainda é resposta; o stream não é lido de novo depois do fim.
                    if let Ok(event) = serde_json::from_slice::<Value>(&buf)
                        && event.get("type").and_then(Value::as_str) == Some("done") {
                        return Ok(event.get("result").cloned().unwrap_or(Value::Null));
                    }
                    return Err(Failure { uncertain: post, ..Failure::local("shared_config_stream_cut") });
                }
                _ => return Err(cut()),
            }
        }
    }

    /// Paleta do papel de parede desta máquina. O backend só responde a pedidos locais: ligado a outro
    /// servidor volta 403, e 404 quer dizer que o desktop não gera paleta.
    pub async fn desktop_palette(&self) -> Result<Value, Failure> {
        let mut url = self.route();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "desktop", "palette"]);
        let r = self.client.get(url).timeout(Duration::from_secs(10)).send().await.map_err(|_| Failure::transport(false))?;
        Self::checked(r, false).await?.json().await.map_err(|_| Failure::local("invalid_response"))
    }

    /// Foto do papel de parede desta máquina, para o fundo Desktop em Vidro. Mesma regra da paleta: 403 fora
    /// do loopback, 404 sem papel de parede.
    pub async fn desktop_wallpaper(&self) -> Result<Vec<u8>, Failure> {
        let mut url = self.route();
        url.path_segments_mut().expect("validated HTTP base").pop_if_empty().extend(["api", "desktop", "wallpaper"]);
        let r = self.client.get(url).timeout(Duration::from_secs(20)).send().await.map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        let limit = crate::media::BACKDROP_MAX_BYTES;
        if r.content_length().is_some_and(|n| n > limit) { return Err(Failure::local("backdrop_too_big")); }
        // Resposta sem tamanho declarado para no teto enquanto chega, sem juntar tudo antes de conferir.
        let mut body = Vec::new();
        let mut chunks = std::pin::pin!(r.bytes_stream());
        use futures::StreamExt;
        while let Some(chunk) = chunks.next().await {
            body.extend_from_slice(&chunk.map_err(|_| Failure::transport(false))?);
            if body.len() as u64 > limit { return Err(Failure::local("backdrop_too_big")); }
        }
        Ok(body)
    }

    async fn stream(&self, name: Option<&str>, cursor: &str) -> Result<Response, Failure> {
        let mut url = self.endpoint(name, Some("events"));
        // O app junta a diferença da vista dos mods (`plugin_ui::apply_delta`).
        if name.is_some() { url.query_pairs_mut().append_pair("ui_delta", "1"); }
        let mut req = self.client.get(url).header(header::ACCEPT, "text/event-stream");
        if !cursor.is_empty() { req = req.header("Last-Event-ID", cursor); }
        let r = tokio::time::timeout(Duration::from_secs(15), req.send()).await
            .map_err(|_| Failure::transport(false))?.map_err(|_| Failure::transport(false))?;
        let r = Self::checked(r, false).await?;
        if !r.headers().get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("").starts_with("text/event-stream") {
            return Err(Failure::local("invalid_response"));
        }
        Ok(r)
    }
}

pub struct History { pub events: Option<Vec<ChatEvent>>, pub etag: Option<String> }

/// Resultado de retomar do arquivo: a sessão nova, ou o nome da viva que já tem a conversa aberta.
pub enum Resumed { New(SessionInfo), Live(String) }

/// O que prende uma conta Claude que o servidor recusou apagar: sessões do Hangar e processos presos.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct AccountHolders {
    #[serde(default, rename = "sessoes")] pub sessions: Vec<String>,
    #[serde(default, rename = "processos")] pub processes: Vec<HeldProcess>,
}

/// `session` é a sessão de terminal que hospeda o processo, quando ele mora numa.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct HeldProcess {
    pub pid: u64,
    #[serde(default, rename = "nome")] pub name: Option<String>,
    #[serde(default, rename = "sessao")] pub session: Option<String>,
}

/// Apagar conta: feito, ou recusado com o que a prende, para a tela perguntar se libera (`release=1`).
pub enum Removed { Done(Value), Held(AccountHolders) }

/// A lista da recusa por conta em uso; servidor antigo não a manda, e lista vazia não tem o que liberar.
pub fn account_holders(body: &Value) -> Option<AccountHolders> {
    let detail = body.get("detail")?;
    if !matches!(detail.get("code")?.as_str()?, "erro_sessao_usa_conta" | "erro_processos_usam_conta") { return None; }
    let holders: AccountHolders = serde_json::from_value(detail.get("params")?.clone()).ok()?;
    (!holders.sessions.is_empty() || !holders.processes.is_empty()).then_some(holders)
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ShareCreated { pub link: String, pub expires_at: f64 }

/// O convite de par tem a mesma forma do de compartilhamento: link e validade.
pub type PairInvite = ShareCreated;

/// Nível do par: `Full` escreve e tem o terminal; `View` lê e manda recado. Ausente (Hangar antigo) é `View`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairLevel { Full, #[default] View }

impl PairLevel {
    pub fn wire(self) -> &'static str { match self { PairLevel::Full => "full", PairLevel::View => "view" } }
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct PairAccepted { pub alias: String, pub owner: String, pub session: String, #[serde(default)] pub level: PairLevel }

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ShareEntry { pub id: String, pub device: Option<String>, pub created_at: f64, pub redeemed_at: Option<f64>, pub expires_at: f64, pub pending: bool }

/// Tudo o que falta na máquina para o Funnel subir (operador, liberação na tailnet) e como resolver. `enable_url` só vem
/// com `funnel` faltando.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SharePrereqs { pub missing: Vec<String>, pub fix: String, #[serde(deserialize_with = "tailscale_link")] pub enable_url: Option<String> }

/// O link vem do servidor e vai para `open_url`: só a página do Tailscale passa, qualquer outro esquema ou host vira `None`.
fn tailscale_link<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value = <Option<String> as serde::Deserialize>::deserialize(d).unwrap_or(None);
    Ok(value.filter(|url| url.starts_with("https://login.tailscale.com/")))
}

/// `Unreachable`: o link do Funnel não abriu de fora no prazo (o servidor já revogou o convite); `NeedsRust`: a rota não
/// existe (backend sem o servidor Rust).
pub enum ShareFailure { Blocked(SharePrereqs), Unreachable { stage: String, detail: String }, NeedsRust, Other(Failure) }

/// Recusa da criação que a tela trata à parte; o resto segue como falha comum.
pub fn share_refusal(status: u16, body: &Value) -> Option<ShareFailure> {
    match status {
        // Só a rota ausente (sem código) ou a recusa explícita do Python; o 404 com código do Rust é falha comum.
        404 | 405 => match body.get("detail").and_then(|d| d.get("code")).and_then(Value::as_str) {
            None | Some("share_needs_rust_server") => Some(ShareFailure::NeedsRust),
            Some(_) => None,
        },
        409 => share_blocked(body).map(ShareFailure::Blocked).or_else(|| {
            let detail = body.get("detail")?;
            if detail.get("code")?.as_str()? != "share_reach_failed" { return None; }
            let field = |k: &str| detail.get("params").and_then(|p| p.get(k)).and_then(Value::as_str).unwrap_or_default().to_owned();
            Some(ShareFailure::Unreachable { stage: field("stage"), detail: field("detail") })
        }),
        _ => None,
    }
}

/// Lista vazia ainda é pré-requisito: o `fix` diz o que fazer.
pub fn share_blocked(body: &Value) -> Option<SharePrereqs> {
    let detail = body.get("detail")?;
    if detail.get("code")?.as_str()? != "erro_compartilhar_pre_requisito" { return None; }
    Some(detail.get("params").cloned().and_then(|p| serde_json::from_value(p).ok()).unwrap_or_default())
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Redeemed { pub token: String, pub session: String, pub owner: String, pub address: String }

/// Resgate do convite, sem seguir redirecionamento. O `token` é o que o app já tem para a máquina: com ele o convite novo
/// entra no mesmo token em vez de trocar de sessão.
pub async fn redeem_invite(address: &str, code: &str, device: &str, token: Option<&str>) -> Result<Redeemed, Failure> {
    let mut url = Url::parse(address).map_err(|_| Failure::local("invalid_url"))?;
    url.path_segments_mut().map_err(|_| Failure::local("invalid_url"))?.pop_if_empty().extend(["api", "guest", "redeem"]);
    let client = Client::builder().connect_timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none())
        .build().map_err(|_| Failure::local("network_error"))?;
    let mut body = json!({"code": code, "device": device});
    if let Some(token) = token { body["token"] = json!(token); }
    let r = client.post(url).json(&body).timeout(Duration::from_secs(20)).send().await
        .map_err(|_| Failure::transport(true))?;
    Api::checked(r, true).await?.json().await.map_err(|_| Failure::local("invalid_response"))
}

#[derive(Clone, serde::Deserialize, PartialEq)]
pub struct ExternalPairDto { pub local_session: String, pub alias: String, pub owner: String, pub session: String, pub address: String, pub token: String }

// O token é credencial: fica fora de qualquer log.
impl std::fmt::Debug for ExternalPairDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalPairDto").field("alias", &self.alias).field("session", &self.session).field("address", &self.address)
            .field("token", &"***").finish_non_exhaustive()
    }
}

impl Api {
    /// Pares externos das sessões deste servidor (só o dono lê).
    pub async fn external_pairs(&self) -> Result<Vec<ExternalPairDto>, Failure> {
        serde_json::from_value(self.server_read(&["external-pairs"], &[], 15).await?).map_err(|_| Failure::local("invalid_response"))
    }
}

/// Liga a sessão pareada ao token de convite que este app já tem para a máquina dela.
pub async fn attach_guest(address: &str, holder: &str, other: &str) -> Result<(), Failure> {
    Api::new(address, holder)?.server_send(reqwest::Method::POST, &["guest", "attach"], Some(json!({"token": other})), 15).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn coded_not_found_is_not_needs_rust() {
        let missing = json!({"detail": {"code": "erro_sessao_inexistente", "msg": "x"}});
        assert!(share_refusal(404, &missing).is_none());
        let python = json!({"detail": {"code": "share_needs_rust_server", "msg": "x"}});
        assert!(matches!(share_refusal(404, &python), Some(ShareFailure::NeedsRust)));
        assert!(matches!(share_refusal(405, &json!({"detail": "Method Not Allowed"})), Some(ShareFailure::NeedsRust)));
    }

    #[test]
    fn pair_accept_without_level_is_view() {
        let old: PairAccepted = serde_json::from_value(json!({"ok": true, "alias": "a", "owner": "Ana", "session": "Y"})).unwrap();
        assert_eq!(old.level, PairLevel::View);
        let new: PairAccepted = serde_json::from_value(json!({"alias": "a", "owner": "Ana", "session": "Y", "level": "full"})).unwrap();
        assert_eq!((new.level, new.level.wire()), (PairLevel::Full, "full"));
    }

    #[test]
    fn dictation_client_defaults_to_no_organization_without_loading_any_model() {
        let api = Api::new("http://127.0.0.1:8765", "fixture-owner").unwrap();
        let url = api.transcribe_url(Some("destination"), None, true, Some("briefing"));
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("organization_mode").map(String::as_str), Some("none"));
        assert_eq!(query.get("limpar").map(String::as_str), Some("0"));
        assert!(!query.contains_key("estilo"));
        assert!(!query.contains_key("organization_model"));
    }

    #[test]
    fn update_channel_errors_translate_with_branch_parameter() {
        let message = failure_detail(Some(json!({"detail": {"code": "update_channel_missing",
            "params": {"branch": "test/channel"}, "msg": "server fallback"}})), 400);
        assert!(message.contains("test/channel"), "{message}");
        assert!(!message.contains("{branch}"));
        assert_ne!(message, "server fallback");
    }

    #[test]
    fn slash_command_refused_with_queued_messages_has_its_own_sentence() {
        let waiting = failure_detail(Some(json!({"detail": {"code": "erro_comando_nao_executado", "msg": "pt cru",
            "params": {"comando": "/clear", "motivo": "queue_waiting"}}})), 400);
        assert_eq!(waiting, crate::i18n::tr_shared("command_refused_queue_waiting", &[]));
        let other = failure_detail(Some(json!({"detail": {"code": "erro_comando_nao_executado", "msg": "pt cru",
            "params": {"comando": "/help", "motivo": "keyboard_loan"}}})), 400);
        assert!(other.contains("/help") && other.contains("keyboard_loan") && !other.contains("{comando}"), "{other}");
    }

    #[test]
    fn subagent_message_refusals_translate_by_code_with_the_reason() {
        for code in ["erro_subagente_mensagem_sem_terminal", "erro_subagente_mensagem_sem_plugin", "erro_subagente_inexistente",
            "erro_subagente_mensagem_sem_resposta"] {
            let message = failure_detail(Some(json!({"detail": {"code": code}})), 409);
            assert_eq!(message, crate::i18n::tr_web(code, &Default::default()).unwrap());
        }
        let refused = failure_detail(Some(json!({"detail": {"code": "erro_subagente_mensagem_recusada", "reason": "agente ocupado"}})), 409);
        assert!(refused.contains("agente ocupado") && !refused.contains("{reason}"), "{refused}");
        let nested = failure_detail(Some(json!({"detail": {"code": "erro_subagente_mensagem_recusada", "params": {"reason": "x1"}}})), 409);
        assert!(nested.contains("x1"), "{nested}");
    }

    #[test]
    fn costs_failure_shows_the_translated_reason_with_its_code() {
        let message = failure_detail(Some(json!({"ok": false, "error_code": "costs_no_disk", "message": "índice de custos indisponível",
            "detail": {"code": "costs_no_disk", "params": {"motivo": "índice de custos indisponível"},
                "msg": "índice de custos indisponível — costs_no_disk"}})), 503);
        assert_eq!(message, crate::i18n::tr_web("costs_no_disk", &Default::default()).unwrap());
        assert!(message.contains("(costs_no_disk)"), "{message}");
        let info = failure_detail(Some(json!({"detail": {"code": "internal_info", "params": {}, "msg": "x — internal_info"}})), 503);
        assert_eq!(info, crate::i18n::tr_web("history_internal_info", &Default::default()).unwrap());
    }

    #[test]
    fn group_errors_use_the_shared_phrase_with_lists_joined() {
        for code in ["groups_need_rust_server", "erro_grupo_peer_nao_registrado", "erro_grupo_fusao_entre_donos", "erro_contrato_mudou", "erro_link_remoto"] {
            let message = failure_detail(Some(json!({"detail": {"code": code, "params": {"servidor": "casa"}, "msg": "frase fixa do servidor"}})), 409);
            assert_ne!(message, "frase fixa do servidor", "{code}");
            assert!(!message.contains('{'), "{code}: {message}");
        }
        let warning = json!({"code": "erro_grupo_membro_nao_confirmado", "params": {"membros": ["casa::a", "casa::b"]}, "msg": "x"});
        let text = group_text(&warning).unwrap();
        assert!(text.contains("casa::a; casa::b"), "{text}");
    }

    #[test]
    fn pairing_and_session_group_errors_use_the_phrase_of_the_ui_language() {
        crate::i18n::english_in_this_thread(true);
        let detail = |code: &str| failure_detail(Some(json!({"detail": {"code": code, "params": {}, "msg": "frase fixa do servidor"}})), 409);
        assert_eq!(detail("erro_sessao_sem_grupo"), "session is not in a group");
        for code in ["erro_pareamento_desfeito", "erro_sessao_nao_pareada", "erro_peer_nao_avisado", "erro_group_message_tempestade"] {
            assert_ne!(detail(code), "frase fixa do servidor", "{code}");
        }
    }

    #[test]
    fn close_warning_translates_each_nested_reason_and_keeps_unknown_text() {
        let need = crate::i18n::tr_web("groups_need_rust_server", &Default::default()).unwrap();
        let warning = json!({"code": "erro_pareamento_saida_falhou", "msg": "aviso de saída falhou: s1: texto cru", "params": {"avisos": [
            {"sessao": "s1", "erro": {"code": "groups_need_rust_server", "params": {}, "msg": "texto cru"}},
            {"sessao": "s2", "erro": {"code": "codigo_sem_frase", "params": {}, "msg": "motivo do servidor"}}]}});
        let text = warning_text(&warning);
        assert!(text.contains(&format!("s1: {need}")), "{text}");
        assert!(text.contains("s2: motivo do servidor") && !text.contains("texto cru"), "{text}");
        assert_eq!(warning_text(&json!({"code": "codigo_sem_frase", "msg": "cru"})), "cru");
    }

    #[test]
    fn interaction_errors_use_the_shared_phrase_not_the_server_message() {
        for code in ["erro_interacao_sem_identidade", "erro_interacao_desatualizada", "erro_interacao_indisponivel"] {
            let message = failure_detail(Some(json!({"detail": {"code": code, "params": {}, "msg": "frase fixa do servidor"}})), 409);
            assert_eq!(message, crate::i18n::tr_web(code, &Default::default()).unwrap(), "{code}");
        }
    }

    #[test]
    fn backend_error_detail_keeps_string_and_object_messages() {
        assert_eq!(failure_detail(Some(json!({"detail": "plain rejection"})), 400), "plain rejection");
        assert_eq!(failure_detail(Some(json!({"detail": {"code": "turn_missing", "params": {}, "msg": "Nenhum turno ativo"}})), 409), "Nenhum turno ativo");
        assert_eq!(failure_detail(Some(json!({"detail": {"code": "turn_missing", "params": {}}})), 409), "turn_missing");
        assert_eq!(failure_detail(Some(json!({"detail": [{"msg": "at most 40 characters"}]})), 422), "at most 40 characters");
        for code in ["session_transfer_source_changed", "session_transfer_restore_failed"] {
            assert_eq!(failure_detail(Some(json!({"detail": {"code":code, "msg":"backend reason", "params":{}}})), 409),
                format!("{code}: backend reason"));
            assert_eq!(failure_detail(Some(json!({"detail": {"code":code, "params":{}}})), 409), code);
        }
        // Atalhos do projeto: a frase traduzida pelo código leva o motivo do backend, de `params.detalhe` ou do `msg`.
        let pasta = failure_detail(Some(json!({"detail": {"code": "erro_shortcut_pasta", "params": {"detalhe": "pasta nao existe: /x"}, "msg": "pasta nao existe: /x"}})), 400);
        assert!(pasta != "pasta nao existe: /x" && pasta.contains("pasta nao existe: /x"), "{pasta}");
        let items = failure_detail(Some(json!({"detail": {"code": "erro_project_shortcuts", "params": {}, "msg": "item 1 (shell) com pasta vazia"}})), 400);
        assert!(items != "item 1 (shell) com pasta vazia" && items.contains("item 1 (shell) com pasta vazia"), "{items}");
    }

    #[test]
    fn transfer_refusals_show_specific_localized_reasons_without_private_message() {
        for (code, pt, en) in [
            ("session_transfer_context_budget_exceeded", "excede a capacidade", "exceeds the selected"),
            ("session_transfer_model_capacity_unknown", "confirmar a capacidade", "capacity could not be verified"),
            ("session_transfer_model_media_unsupported", "não aceita as imagens", "does not support the images"),
            ("session_transfer_codex_version_unsupported", "versão instalada", "installed Codex version"),
            ("session_transfer_login_required", "Entre na conta", "Sign in"),
            ("session_transfer_account_full", "sem cota", "no quota"),
            ("session_transfer_invalid_model_choice", "catálogo", "catalog"),
            ("session_transfer_queue_pending", "mensagens na fila", "Messages are queued"),
            ("session_transfer_source_busy", "permissão pendente", "pending question or approval"),
            ("session_transfer_busy", "trocando de agente", "switching agents"),
            ("session_transfer_resume_unsupported", "transferência", "transfer"),
        ] {
            let text = failure_detail(Some(json!({"detail": {"code": code,
                "msg": "private-payload", "params": {"model": "chosen", "estimated": 123, "limit": 100}}})), 409);
            assert!(text.contains(pt) || text.contains(en), "{code}: {text}");
            assert!(!text.contains("private-payload") && !text.contains(code));
            for (source, expected) in [(include_str!("../../../messages/pt.json"), pt), (include_str!("../../../messages/en.json"), en)] {
                let catalog: Value = serde_json::from_str(source).unwrap();
                assert!(catalog[code].as_str().unwrap().contains(expected));
            }
        }
    }

    #[test]
    fn session_open_and_lifecycle_refusals_use_the_web_sentence() {
        for code in ["codex_terminal_starting", "runtime_unhealthy", "erro_conta_criadora", "erro_criadora_ilegivel",
            "erro_modo_padrao_ilegivel", "erro_cliproxy_conta", "session_patch_stale", "lifecycle_busy", "heal_not_needed",
            "relaunch_policy_forbids_bypass", "lifecycle_kill", "relaunch_ipc", "heal_backoff"] {
            let text = failure_detail(Some(json!({"detail": {"code": code, "params": {}, "msg": "texto-do-servidor"}})), 409);
            assert!(text != "texto-do-servidor" && text != code, "{code}: {text}");
        }
        let text = failure_detail(Some(json!({"detail": {"code": "erro_codex_escolha_invalida", "params": {"erro": "xhigh"}, "msg": "x"}})), 422);
        assert!(text != "x" && text.contains("xhigh"), "{text}");
        let text = failure_detail(Some(json!({"detail": {"code": "erro_cwd_inexistente", "params": {"cwd": "/sumiu"}, "msg": "x"}})), 400);
        assert!(text != "x" && text.contains("/sumiu"), "{text}");
    }

    #[test]
    fn claude_life_change_refusals_use_the_web_sentence() {
        for code in ["erro_troca_modo", "erro_recarregar", "erro_retomar", "erro_recarregar_sem_terminal"] {
            let text = failure_detail(Some(json!({"detail": {"code": code, "params": {"erro": "pane sumiu (lifecycle_kill)"}, "msg": "x"}})), 409);
            assert!(text != "x" && text.contains("pane sumiu (lifecycle_kill)"), "{code}: {text}");
        }
    }

    #[test]
    fn resume_refusals_use_the_web_sentence() {
        let text = failure_detail(Some(json!({"detail": {"code": "erro_resume_so_claude", "params": {"provider": "pi"}, "msg": "x"}})), 409);
        assert!(text != "x" && text.contains("pi") && text.contains("Claude"), "{text}");
        let text = failure_detail(Some(json!({"detail": {"code": "erro_session_id_invalido", "params": {}, "msg": "session_id invalido"}})), 422);
        assert!(text != "session_id invalido" && !text.contains("erro_session_id_invalido"), "{text}");
    }

    #[test]
    fn mod_press_refusals_use_the_web_sentence() {
        let text = failure_detail(Some(json!({"detail": {"code": "erro_mod_botao_ambiguo", "params": {"rotulo": "fechar"}, "msg": "x"}})), 409);
        assert!(text != "erro_mod_botao_ambiguo" && text.contains("fechar"), "{text}");
    }

    #[test]
    fn the_envelope_code_goes_into_the_failure() {
        let guard = json!({"ok": false, "detail": {"code": "erro_mod_guarda_indisponivel", "params": {"motivo": "x"}, "msg": "x"}});
        assert_eq!(envelope_code(Some(&guard)).as_deref(), Some("erro_mod_guarda_indisponivel"));
        assert_eq!(envelope_code(Some(&json!({"detail": "texto"}))), None);
        assert_eq!(envelope_code(Some(&json!({"detail": [{"msg": "campo"}]}))), None);
        assert_eq!(envelope_code(None), None);
    }

    #[test]
    fn new_mod_refusals_use_the_web_sentence() {
        for code in ["erro_mod_sem_digitacao", "erro_mod_desenho_vencido", "erro_mod_dialogo_aberto",
            "erro_mod_rascunho_no_prompt", "erro_mod_painel_nao_alcancavel", "erro_mod_fechar_recusado",
            "erro_mod_guarda_indisponivel", "erro_mod_painel_inexistente", "erro_mod_convidado"] {
            let text = failure_detail(Some(json!({"detail": {"code": code, "params": {}, "msg": "texto-do-servidor"}})), 409);
            assert!(text != code && text != "texto-do-servidor", "{code}: {text}");
        }
    }

    #[test]
    fn external_pair_errors_use_the_web_sentence_with_the_refusal_detail() {
        let refused = failure_detail(Some(json!({"detail": {"code": "erro_par_recusado", "params": {"detalhe": "convite vencido"}, "msg": "x"}})), 400);
        assert!(refused != "erro_par_recusado" && refused.contains("convite vencido"), "{refused}");
        let down = failure_detail(Some(json!({"detail": {"code": "erro_par_fora_do_ar", "params": {}, "msg": "raw"}})), 502);
        assert!(down != "raw" && down != "erro_par_fora_do_ar", "{down}");
    }

    #[test]
    fn share_prerequisite_is_read_from_the_409_body() {
        let body = json!({"detail": {"code": "erro_compartilhar_pre_requisito", "msg": "x",
            "params": {"missing": ["operator", "funnel"], "fix": "sudo tailscale set --operator=$USER"}}});
        assert_eq!(share_blocked(&body), Some(SharePrereqs { missing: vec!["operator".into(), "funnel".into()],
            fix: "sudo tailscale set --operator=$USER".into(), enable_url: None }));
        let empty = json!({"detail": {"code": "erro_compartilhar_pre_requisito", "params": {"missing": [], "fix": "https://login.tailscale.com/admin"}}});
        assert_eq!(share_blocked(&empty), Some(SharePrereqs { fix: "https://login.tailscale.com/admin".into(), ..Default::default() }));
        let funnel = json!({"detail": {"code": "erro_compartilhar_pre_requisito",
            "params": {"missing": ["funnel"], "fix": "libere", "enable_url": "https://login.tailscale.com/f/funnel?node=n1"}}});
        assert_eq!(share_blocked(&funnel).and_then(|p| p.enable_url).as_deref(), Some("https://login.tailscale.com/f/funnel?node=n1"));
        for bad in [json!("javascript:alert(1)"), json!("https://login.tailscale.com.evil.io/f"), json!(7)] {
            let body = json!({"detail": {"code": "erro_compartilhar_pre_requisito", "params": {"missing": ["funnel"], "fix": "libere", "enable_url": bad}}});
            let prereqs = share_blocked(&body).expect("still a prerequisite");
            assert_eq!((prereqs.missing, prereqs.enable_url), (vec!["funnel".to_owned()], None));
        }
        let direct: SharePrereqs = serde_json::from_value(json!({"missing": ["funnel"], "enable_url": "javascript:alert(1)"})).unwrap();
        assert_eq!(direct.enable_url, None);
        assert_eq!(share_blocked(&json!({"detail": {"code": "erro_outro", "params": {"missing": ["a"], "fix": "b"}}})), None);
        assert_eq!(share_blocked(&json!({"detail": "texto"})), None);
    }

    #[test]
    fn guest_out_of_scope_keeps_its_code() {
        let body = json!({"detail": {"code": "erro_fora_do_convite", "params": {}, "msg": "fora da sessao compartilhada"}});
        assert_eq!(failure_detail(Some(body), 403), "erro_fora_do_convite");
    }

    #[test]
    fn file_errors_keep_the_reason_but_search_keeps_its_message() {
        for (status, code) in [(409, "erro_arq_mudou_no_disco"), (409, "erro_arq_em_uso"),
            (409, "erro_arq_sumiu"), (409, "erro_arq_escrita_falhou"),
            (413, "erro_arq_grande_demais"), (415, "erro_arq_binario"), (403, "erro_arq_area_do_git")] {
            assert_eq!(failure_detail(Some(json!({"detail": {"code": code, "params": {"msg": "fixed"}, "msg": "fixed"}})), status), code);
        }
        assert_eq!(failure_detail(Some(json!({"detail": {"code": "erro_arq_busca_falhou",
            "params": {"msg": "search failed"}, "msg": "search failed"}})), 500), "search failed");
    }

    #[test]
    fn saved_audio_is_transcribed_by_name_and_fresh_audio_carries_no_name() {
        let api = Api::new("http://127.0.0.1:8765", "t").unwrap();
        let options=DictationOptions{mode:"external_api".into(),rust_capable:true,..Default::default()};
        let url = api.transcribe_url_with_options(Some("minha sessão"), Some("ditado 1.wav"), true, Some("prosa"),&options);
        assert!(url.path().ends_with("/api/sessions/minha%20sess%C3%A3o/transcribe"), "{url}");
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(query, [("limpar".to_owned(), "1".to_owned()), ("organization_mode".to_owned(),"external_api".to_owned()),("include_recent_messages".to_owned(),"false".to_owned()),("estilo".to_owned(), "prosa".to_owned()),
            ("arquivo".to_owned(), "ditado 1.wav".to_owned())]);
        let absolute = api.transcribe_url(Some("s"), Some("/home/u/.hangar/uploads/p-1a/s1/ditado.wav"), false, None);
        assert_eq!(absolute.query_pairs().find(|(k, _)| k == "arquivo").map(|(_, v)| v.into_owned()).as_deref(),
            Some("/home/u/.hangar/uploads/p-1a/s1/ditado.wav"), "o caminho absoluto vai inteiro, para valer depois do /clear");
        let fresh = api.transcribe_url(Some("s"), None, false, Some("prosa"));
        assert_eq!(fresh.query(), Some("limpar=0"), "sem limpar o estilo não vai, e áudio novo não leva nome");
    }

    #[tokio::test]
    async fn review_regression_reader_asset_keeps_root_and_citation_scopes_separate_from_external_auth() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for scope in ["root", "cited", "remote"] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    request.push_str(&line);
                    if line == "\r\n" { break; }
                }
                let uri = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                let url = url::Url::parse(&format!("http://fixture{uri}")).unwrap();
                let authenticated = request.lines().any(|line| line.eq_ignore_ascii_case("authorization: Bearer fixture-owner"));
                assert_eq!(authenticated, scope != "remote");
                if scope != "remote" {
                    assert_eq!(url.path(), "/api/sessions/fixture/file");
                    assert_eq!(url.query_pairs().find(|(key, _)| key == "path").map(|(_, value)| value.into_owned()).as_deref(), Some("docs/assets/example.png"));
                    assert_eq!(url.query_pairs().find(|(key, _)| key == "root").map(|(_, value)| value.into_owned()).as_deref(), (scope == "root").then_some("1"));
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG\0").await.unwrap();
            }
        });
        let api = super::Api::new(&format!("http://{address}"), "fixture-owner").unwrap();
        for source in [super::Source::Workspace("docs/assets/example.png".into()), super::Source::Cited("docs/assets/example.png".into()), super::Source::Remote(format!("http://{address}/external.png"))] {
            assert_eq!(api.fetch("fixture", &source).await.unwrap(), b"PNG\0");
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn saved_dictation_transports_the_frozen_options_and_preserves_raw_failures() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for mode in ["external_api", "harness"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let response = json!({"text":"  confirme o ditado salvo  ", "raw":"  confirme o ditado salvo  ",
                "estilo_aplicado":"cru", "organization_mode":mode,
                "organization_code":"dictation_model_unavailable", "aviso":"Modelo indisponível; a transcrição foi preservada."});
            let body = response.to_string();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    request.push_str(&line);
                    if line == "\r\n" { break; }
                }
                let uri = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                assert!(request.starts_with("POST "));
                let url = Url::parse(&format!("http://{address}{uri}")).unwrap();
                let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
                assert_eq!(url.path(), "/api/sessions/destination/transcribe");
                for (key, value) in [("arquivo", "saved-dictation.wav"), ("limpar", "1"),
                    ("organization_mode", mode), ("estilo", "briefing"),
                    ("generation", "k:frozen"),
                    ("organization_account", "fixture-account"), ("include_recent_messages", "true")] {
                    assert_eq!(query.get(key).map(String::as_str), Some(value));
                }
                assert_eq!(query.get("organization_model").map(String::as_str), if mode == "harness" { Some("organization-model") } else { None });
                assert!(request.to_ascii_lowercase().contains("authorization: bearer fixture-token\r\n"));
                let length = request.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|length| length.trim().parse::<usize>().unwrap())).unwrap_or(0);
                assert_eq!(length, 0, "áudio salvo não pode ser reenviado");
                assert!(!request.to_ascii_lowercase().contains("transfer-encoding:"));
                stream.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let api = Api::new(&format!("http://{address}"), "fixture-token").unwrap();
            let options = DictationOptions { mode:mode.into(), model:(mode == "harness").then(|| "organization-model".into()),
                generation:Some("k:frozen".into()), account:Some("fixture-account".into()),
                rust_capable:true, include_recent_messages:true };
            let result = api.dictate_saved("destination", "saved-dictation.wav", Some("briefing"), &options).await.unwrap();
            server.await.unwrap();
            assert_eq!(result, response, "a falha da organização conserva o cru e sua causa");
        }
    }

    #[tokio::test]
    async fn queue_panel_actions_send_method_path_and_body_of_the_owner_routes() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..4 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    head.push_str(&line);
                    if line == "\r\n" { break; }
                }
                assert!(head.to_ascii_lowercase().contains("authorization: bearer fixture-token\r\n"));
                let length = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap())).unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let line = head.lines().next().unwrap().to_owned();
                let body = (!body.is_empty()).then(|| serde_json::from_slice::<Value>(&body).unwrap());
                seen.push((line.split_whitespace().take(2).collect::<Vec<_>>().join(" "), body));
                let reply = r#"{"ok":true,"steered":true}"#;
                stream.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
            }
            seen
        });
        let api = Api::new(&format!("http://{address}"), "fixture-token").unwrap();
        use crate::interaction::Action;
        for action in [Action::QueueEdit("e 1".into(), "novo".into()), Action::QueueCancel("e 1".into()), Action::QueueSteer("e 1".into()),
            Action::QueueReorder("e 1".into(), None)] {
            let reply = api.request("a b", action.method(), &action.path(), action.query(), action.queue_body(), action.seconds()).await.unwrap();
            assert_eq!(reply["steered"], true);
        }
        assert_eq!(server.await.unwrap(), vec![
            ("PATCH /api/sessions/a%20b/queue/e%201".to_owned(), Some(json!({"text": "novo"}))),
            ("DELETE /api/sessions/a%20b/queue/e%201?only_waiting=true".to_owned(), None),
            ("POST /api/sessions/a%20b/queue/e%201/steer".to_owned(), None),
            ("POST /api/sessions/a%20b/queue/reorder".to_owned(), Some(json!({"id": "e 1", "before_id": null}))),
        ]);
    }

    /// A recusa de conta em uso com a lista vira o que perguntar; sem lista (servidor antigo) ou vazia, erro comum.
    #[test]
    fn account_in_use_refusal_carries_its_holders() {
        let held = json!({"detail": {"code": "erro_processos_usam_conta", "msg": "x", "params": {"pids": [1572, 42180],
            "sessoes": ["web"], "processos": [{"pid": 1572, "nome": "claude.exe", "sessao": null}, {"pid": 42180, "nome": null, "sessao": "term-login-1"}]}}});
        assert_eq!(account_holders(&held), Some(AccountHolders { sessions: vec!["web".into()], processes: vec![
            HeldProcess { pid: 1572, name: Some("claude.exe".into()), session: None },
            HeldProcess { pid: 42180, name: None, session: Some("term-login-1".into()) }] }));
        let old = json!({"detail": {"code": "erro_sessao_usa_conta", "msg": "x", "params": {"nome": "web"}}});
        assert_eq!(account_holders(&old), None);
        let empty = json!({"detail": {"code": "erro_sessao_usa_conta", "params": {"sessoes": [], "processos": []}}});
        assert_eq!(account_holders(&empty), None);
        let other = json!({"detail": {"code": "erro_conta_cheia", "params": {"sessoes": ["web"]}}});
        assert_eq!(account_holders(&other), None);
    }
}
