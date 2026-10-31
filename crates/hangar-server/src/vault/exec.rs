//! O uso de uma senha pelo agente (tools `request_secret` e `vault_*`): o pedido na dona (aqui ou
//! pelos peers), a espera da aprovação dentro do orçamento da chamada e o comando com a senha no
//! ambiente e a saída mascarada. O valor fica em `Zeroizing` e nunca entra em texto de erro.
use std::{ffi::OsString, future::Future, path::{Path, PathBuf}, sync::Arc, time::Duration};

use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::{io::AsyncReadExt, time::Instant};
use zeroize::Zeroizing;

use super::{Summary, VaultError, config::{self, Placement},
    remote::{Remote, RemoteError, UseStart, WaitStatus}, requests::{self, RequestError}, wire::{AskReq, UseReq, WAIT_HOLD}};
use crate::{groups::peers::PeerClient, routes::AppState};

/// Claude e Codex cortam a tool em 60 s sem o prazo do servidor `hangar`, que o `scripts/registrar-mcp.py`
/// grava na configuração da conta (`timeout` no `.claude.json`, `tool_timeout_sec` no `config.toml`).
const CLAUDE_TIMEOUT_MIN_MS: f64 = 600_000.0;
const CODEX_TOOL_TIMEOUT_MIN: i64 = 600;
/// Saída lida do comando: só o fim, antes da máscara e da cauda de 8 KB.
const READ_CAP: usize = 1 << 20;
/// Aprovação mais execução, por chamada.
pub const BUDGET: Duration = Duration::from_secs(600);
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Falha de rede na espera da outra máquina é tentada de novo até isso depois da última espera boa:
/// a dona só esquece o pedido depois de 60 s sem `wait`.
pub const RETRY_WINDOW: Duration = requests::IDLE;
const RETRY_PAUSE: Duration = Duration::from_secs(2);
const TAIL_CHARS: usize = 8 * 1024;
pub const MASK: &str = "••••";

pub const DENIED: &str = "erro_cofre_negado: o dono negou o uso desta senha";
pub const NO_ANSWER: &str = "erro_cofre_sem_resposta: o dono não respondeu dentro de 10 minutos";
pub const NO_DEADLINE: &str = "erro_cofre_sessao_sem_prazo: a conta desta sessão não tem o prazo longo do MCP hangar e a chamada \
cairia antes da resposta do dono; atualize o Hangar (ou rode scripts/registrar-mcp.py), reabra a sessão e tente de novo";
/// Onde mora o cofre visto desta máquina.
#[derive(Clone)]
pub enum Owner { Local(Arc<AppState>), Remote { peers: Arc<PeerClient>, id: String } }

/// O que uma espera devolveu de errado: rede (tentar de novo) ou o fim.
pub enum WaitFail { Retry(String), Stop(String) }

fn remote_text(error: &RemoteError) -> String { format!("{}: {}", error.code(), error.reason()) }

pub fn vault_text(error: VaultError) -> String {
    let (_, code, msg) = error.code_msg();
    format!("{code}: {msg}")
}

fn request_text(error: RequestError) -> String {
    match error {
        RequestError::Vault(error) => vault_text(error),
        RequestError::Lost => "erro_cofre_pedido_perdido: a dona do cofre não conhece mais este pedido".into(),
        RequestError::TooMany => "erro_cofre_pedidos_demais: pedidos demais desta sessão esperando resposta no cofre".into(),
        RequestError::Cooldown => "erro_cofre_pedidos_demais: o dono acabou de recusar um pedido desta sessão; espere 30 s".into(),
        RequestError::Stale | RequestError::Answered => "erro_cofre_pedido_perdido: o pedido mudou na dona".into(),
    }
}

/// `erro_cofre_dona_fora` com a frase de quando a resposta não é a que o pedido espera.
pub fn off_contract() -> String { "erro_cofre_dona_fora: a dona respondeu fora do contrato do cofre".into() }

impl Owner {
    pub fn of(st: &Arc<AppState>) -> Result<Self, String> {
        match config::placement(&st.hangar_dir) {
            Ok(Placement::Owner) => Ok(Self::Local(st.clone())),
            Ok(Placement::Remote(id)) => Ok(Self::Remote { peers: st.peers.clone(), id }),
            Ok(Placement::None) => Err("erro_cofre_ausente: nenhuma máquina guarda o cofre; o dono escolhe a dona em Configurações > Cofre".into()),
            Err(_) => Err("erro_cofre_config_ilegivel: o vault.json desta máquina não lê".into()),
        }
    }

    pub async fn entries(&self) -> Result<Vec<Summary>, String> {
        match self {
            Self::Local(st) => st.vault.list(&st.hangar_dir).await.map_err(vault_text),
            Self::Remote { peers, id } => Remote { peers, owner: id }.entries().await.map_err(|e| remote_text(&e)),
        }
    }

    pub async fn start_use(&self, req: UseReq) -> Result<UseStart, String> {
        match self {
            Self::Local(st) => requests::use_entry(st, req).await.map_err(request_text),
            Self::Remote { peers, id } => Remote { peers, owner: id }.use_(&req).await.map_err(|e| remote_text(&e)),
        }
    }

    pub async fn start_ask(&self, req: AskReq) -> Result<String, String> {
        match self {
            Self::Local(st) => requests::ask_entry(st, req).map_err(request_text),
            Self::Remote { peers, id } => Remote { peers, owner: id }.ask_entry(&req).await.map_err(|e| remote_text(&e)),
        }
    }

    async fn wait(&self, request: &str) -> Result<WaitStatus, WaitFail> {
        match self {
            Self::Local(st) => st.vault_requests.wait(request, WAIT_HOLD).await.map_err(|e| WaitFail::Stop(request_text(e))),
            Self::Remote { peers, id } => Remote { peers, owner: id }.wait(request).await.map_err(|e| match e {
                RemoteError::OwnerDown(_) => WaitFail::Retry(remote_text(&e)),
                other => WaitFail::Stop(remote_text(&other)),
            }),
        }
    }
}

/// As consultas longas até o desfecho. Orçamento estourado é `TimedOut`, como o prazo da dona.
pub async fn settle<F, Fut>(deadline: Instant, mut wait: F) -> Result<WaitStatus, String>
where F: FnMut() -> Fut, Fut: Future<Output = Result<WaitStatus, WaitFail>> {
    let mut last_ok = Instant::now();
    loop {
        match tokio::time::timeout_at(deadline, wait()).await {
            Err(_) => return Ok(WaitStatus::TimedOut),
            Ok(Ok(WaitStatus::Pending)) => last_ok = Instant::now(),
            Ok(Ok(done)) => return Ok(done),
            Ok(Err(WaitFail::Stop(text))) => return Err(text),
            Ok(Err(WaitFail::Retry(text))) => {
                if last_ok.elapsed() >= RETRY_WINDOW { return Err(text); }
                if Instant::now() + RETRY_PAUSE >= deadline { return Ok(WaitStatus::TimedOut); }
                tokio::time::sleep(RETRY_PAUSE).await;
            }
        }
    }
}

/// Um pedido aberto na dona. Solto sem desfecho (orçamento estourado, chamada cancelada, futuro
/// largado) cancela o pedido, e o cartão some da tela do dono.
pub struct Pending { owner: Owner, id: String, armed: bool }

impl Pending {
    pub fn new(owner: Owner, id: String) -> Self { Self { owner, id, armed: true } }

    pub async fn settle(mut self, deadline: Instant) -> Result<WaitStatus, String> {
        let result = { let (owner, id) = (&self.owner, self.id.as_str()); settle(deadline, move || owner.wait(id)).await };
        // O desfecho entregue fecha o pedido na dona.
        if matches!(&result, Ok(status) if !matches!(status, WaitStatus::TimedOut)) { self.armed = false; }
        result
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.armed { return; }
        match &self.owner {
            Owner::Local(st) => { let _ = st.vault_requests.cancel(&self.id); }
            Owner::Remote { peers, id } => {
                let (peers, owner, request) = (peers.clone(), id.clone(), std::mem::take(&mut self.id));
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        // Pedido já fechado na dona é o fim esperado; o resto deixa o cartão na tela até o prazo dela.
                        match (Remote { peers: &peers, owner: &owner }).cancel(&request).await {
                            Ok(()) | Err(RemoteError::Lost) => {}
                            Err(error) => tracing::warn!(code = "vault_cancel_failed", owner = %owner, reason = error.code(), "o pedido não foi cancelado na dona"),
                        }
                    });
                }
            }
        }
    }
}

/// A senha liberada para um uso: na hora (`free`, entrada temporária) ou depois da aprovação.
pub async fn secret(owner: &Owner, req: UseReq, deadline: Instant) -> Result<Zeroizing<String>, String> {
    let id = match owner.start_use(req).await? {
        UseStart::Value(value) => return Ok(value),
        UseStart::Request(id) => id,
    };
    match Pending::new(owner.clone(), id).settle(deadline).await? {
        WaitStatus::Allowed(value) => Ok(value),
        WaitStatus::Denied => Err(DENIED.into()),
        WaitStatus::TimedOut => Err(NO_ANSWER.into()),
        _ => Err(off_contract()),
    }
}

pub const NO_ENV: &str = "erro_cofre_sem_ambiente: o ambiente desta sessão não foi lido nem refeito pelo lançador; \
o comando não roda com o ambiente do servidor";

type Env = Vec<(OsString, OsString)>;

/// O agente da sessão: pasta de trabalho, o ambiente dele (lido do processo ou refeito pelo lançador)
/// e se a sessão nasceu sem o prazo longo das tools que esperam o dono.
pub struct Agent { pub cwd: Option<PathBuf>, pub env: Option<Env>, pub lacks_deadline: bool }

pub async fn agent(st: &Arc<AppState>, name: &str) -> Result<Agent, String> {
    let found = st.list.fresh_session(name).await
        .map_err(|_| "erro_cofre_sessao_indisponivel: a lista de sessões não respondeu".to_owned())?;
    let Some((row, pid)) = found else { return Err(format!("sessao_desconhecida: '{name}' não está na lista de sessões")) };
    let text = |v: Option<String>| v.filter(|v| !v.is_empty());
    let cwd = text(row.git_cwd).or(text(row.cwd)).map(PathBuf::from);
    let key = row.lifecycle_id.as_deref().and_then(|l| l.strip_prefix("k:")).map(str::to_owned);
    let codex_home = text(row.codex_home).map(PathBuf::from);
    let (st, name, provider, headless) = (st.clone(), name.to_owned(), row.provider, row.headless);
    tokio::task::spawn_blocking(move || {
        let env = session_env(&st, &name, &provider, headless, pid, key.as_deref());
        let config = match provider.as_str() {
            "codex" => codex_home.map(|home| home.join("config.toml")),
            "claude" => env.as_deref().map(|env| claude_config(env, &st.accounts.env.home)),
            _ => None,
        };
        let config = config.and_then(|path| std::fs::read_to_string(path).ok());
        let lacks_deadline = lacks_deadline(&provider, config.as_deref());
        Agent { cwd, env, lacks_deadline }
    }).await.map_err(|_| "erro_cofre_sessao_indisponivel: a leitura da sessão foi interrompida".to_owned())
}

/// O `.claude.json` que a sessão leu ao nascer: o da conta dela (`CLAUDE_CONFIG_DIR`), senão o do usuário.
fn claude_config(env: &[(OsString, OsString)], home: &Path) -> PathBuf {
    let dir = env.iter().find(|(k, v)| k == "CLAUDE_CONFIG_DIR" && !v.is_empty()).map(|(_, v)| PathBuf::from(v));
    dir.as_deref().unwrap_or(home).join(".claude.json")
}

/// Com terminal, o ambiente do agente pelo `/proc`; sem terminal, o do cano (que nasceu com o ambiente
/// da subida) com a chave conferida, senão o que o lançador monta agora.
fn session_env(st: &AppState, name: &str, provider: &str, headless: bool, pid: Option<u32>, key: Option<&str>) -> Option<Env> {
    if !headless { return pid.and_then(environ); }
    let base = &st.accounts.env;
    let dir = base.home.join(".hangar").join(if provider == "codex" { "codex-sessions" } else { "claude-headless" });
    let meta = crate::runtime::codex_sidecar::load(&crate::runtime::codex_sidecar::path(&dir, name))?;
    let cano = meta.get("cano").and_then(|c| c.get("pid")).and_then(serde_json::Value::as_u64).and_then(|p| u32::try_from(p).ok());
    let mine = |env: &Env| key.is_some_and(|key| env.iter().any(|(k, v)| k == "CP_SESSION_KEY" && v == key));
    cano.and_then(environ).filter(mine).or_else(|| launch_env(base, provider, name, &serde_json::Value::Object(meta)))
}

/// O ambiente que o lançador daria a esta sessão sem terminal agora (Windows, cano ilegível).
fn launch_env(base: &crate::accounts::environment::AccountEnvironment, provider: &str, name: &str, meta: &serde_json::Value) -> Option<Env> {
    let map = match provider {
        "codex" => crate::runtime::codex_launch::launch_env(base, &crate::runtime::codex_sidecar::dir(&base.home), meta).ok()?.env,
        "claude" => {
            let spec = crate::session_open::record::claude_live(&base.home, meta, name).ok()?;
            let facts = crate::session_open::facts::LocalFacts::new(base.clone(), tokio::runtime::Handle::try_current().ok()?);
            crate::runtime::claude_launch::plan_for(base, None, &spec, meta, &facts).ok()?.env
        }
        _ => return None,
    };
    Some(map.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
}

#[cfg(target_os = "linux")]
fn environ(pid: u32) -> Option<Vec<(OsString, OsString)>> {
    use std::os::unix::ffi::OsStringExt;
    let raw = Zeroizing::new(std::fs::read(format!("/proc/{pid}/environ")).ok()?);
    if raw.is_empty() { return None; }
    Some(raw.split(|b| *b == 0).filter_map(|pair| {
        let at = pair.iter().position(|b| *b == b'=')?;
        Some((OsString::from_vec(pair[..at].to_vec()), OsString::from_vec(pair[at + 1..].to_vec())))
    }).collect())
}

#[cfg(not(target_os = "linux"))]
fn environ(_pid: u32) -> Option<Vec<(OsString, OsString)>> { None }

/// A conta da sessão não tem o prazo longo do servidor `hangar`: `config` é o `.claude.json` (Claude) ou o
/// `config.toml` (Codex) dela. Sem leitura a chamada segue.
// shortcut: o agente lê a configuração ao subir; sessão aberta antes de a conta ser atualizada passa aqui e cai em 60 s.
pub fn lacks_deadline(provider: &str, config: Option<&str>) -> bool {
    match provider {
        "claude" => config.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()).is_some_and(|config| {
            !config["mcpServers"]["hangar"]["timeout"].as_f64().is_some_and(|t| t >= CLAUDE_TIMEOUT_MIN_MS)
        }),
        "codex" => config.and_then(|raw| toml::from_str::<toml::Table>(raw).ok()).is_some_and(|config| {
            let timeout = config.get("mcp_servers").and_then(|m| m.get("hangar")).and_then(|h| h.get("tool_timeout_sec"));
            !timeout.and_then(|t| t.as_integer().or_else(|| t.as_float().map(|f| f as i64))).is_some_and(|t| t >= CODEX_TOOL_TIMEOUT_MIN)
        }),
        _ => false,
    }
}

/// O que o comando nunca herda: o que é só da sessão ou do motor (`NOT_INHERITED`) e os segredos do servidor.
fn stripped(name: &OsString) -> bool {
    crate::session_open::terminal::NOT_INHERITED.iter().chain(crate::terminal_process::PRIVATE_ENV_KEYS.iter()).any(|k| name == k)
}

/// Nome que um shell aceita como variável e que não é segredo do servidor.
pub fn env_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128 && !crate::terminal_process::PRIVATE_ENV_KEYS.contains(&name)
}

/// O `encodeURIComponent`: deixa `-_.!~*'()` como estão.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*')
    .remove(b'\'').remove(b'(').remove(b')');

/// `application/x-www-form-urlencoded`: deixa `*-._`, e o espaço vira `+`.
const FORM: &AsciiSet = &NON_ALPHANUMERIC.remove(b'*').remove(b'-').remove(b'.').remove(b'_');
/// Abaixo disso o pedaço de base64 do meio casaria com texto comum.
const MIN_CORE: usize = 6;

/// `%2F` → `%2f`.
fn lower_hex(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut left = 0;
    for c in encoded.chars() {
        if c == '%' { left = 2; out.push(c); continue; }
        out.push(if left > 0 { left -= 1; c.to_ascii_lowercase() } else { c });
    }
    out
}

/// O valor como string JSON, sem as aspas; `ascii` escapa todo não-ASCII em `\uXXXX`.
fn json_escaped(value: &str, ascii: bool) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (ascii && !c.is_ascii()) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) { out.push_str(&format!("\\u{unit:04x}")); }
            }
            c => out.push(c),
        }
    }
    out
}

/// O miolo do base64 do valor quando ele começa a `skip` bytes de um bloco (0, 1 ou 2): só os
/// caracteres que dependem apenas dele, para achá-lo dentro de outro conteúdo (`user:senha` do Basic auth).
fn base64_core(engine: &impl Engine, value: &[u8], skip: usize) -> Option<String> {
    let mut padded = vec![0u8; skip];
    padded.extend_from_slice(value);
    let encoded = Zeroizing::new(engine.encode(&padded));
    let (start, end) = ((skip * 8).div_ceil(6), padded.len() * 8 / 6);
    let core: String = encoded.get(start..end)?.to_owned();
    (core.len() >= MIN_CORE).then_some(core)
}

/// O valor trocado por `••••` em texto, base64 (padrão e de URL, com e sem `=`, também no meio de
/// outro conteúdo), percent-encoded (maiúsculo e minúsculo, de URL e de formulário) e escapado em JSON.
pub fn mask(text: &str, value: &str) -> Zeroizing<String> { mask_with(text, &forms(value)) }

fn mask_with(text: &str, forms: &[Zeroizing<String>]) -> Zeroizing<String> {
    let mut out = Zeroizing::new(text.to_owned());
    for form in forms { out = Zeroizing::new(out.replace(form.as_str(), MASK)); }
    out
}

/// As formas do valor que a saída pode trazer, a mais longa antes.
fn forms(value: &str) -> Vec<Zeroizing<String>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let mut forms: Vec<Zeroizing<String>> = vec![Zeroizing::new(value.to_owned())];
    for engine in [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD] { forms.push(Zeroizing::new(engine.encode(value))); }
    for engine in [STANDARD, URL_SAFE] {
        for skip in 0..3 { forms.extend(base64_core(&engine, value.as_bytes(), skip).map(Zeroizing::new)); }
    }
    for set in [NON_ALPHANUMERIC, COMPONENT] {
        let encoded = Zeroizing::new(utf8_percent_encode(value, set).to_string());
        forms.push(Zeroizing::new(lower_hex(&encoded)));
        forms.push(encoded);
    }
    let form = Zeroizing::new(utf8_percent_encode(value, FORM).to_string().replace("%20", "+"));
    forms.push(Zeroizing::new(lower_hex(&form)));
    forms.push(form);
    for ascii in [false, true] { forms.push(Zeroizing::new(json_escaped(value, ascii))); }
    forms.retain(|f| !f.is_empty());
    // O mais longo antes: o base64 sem `=` é prefixo do com `=`.
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    forms
}

/// A cauda mascarada sem mascarar a saída inteira: corta antes com folga da forma mais longa e, depois
/// da máscara, tira a folga, onde pode ter ficado o fim de uma forma partida no corte.
fn masked_tail(text: &str, value: &str) -> String {
    let forms = forms(value);
    // Em bytes: nenhuma forma tem mais caracteres que isso.
    let slack = forms.first().map_or(0, |f| f.len());
    let count = text.chars().count();
    if count <= TAIL_CHARS + slack { return tail(&mask_with(text, &forms)); }
    let cut = Zeroizing::new(text.chars().skip(count - TAIL_CHARS - slack).collect::<String>());
    let masked = mask_with(&cut, &forms);
    tail(&masked.chars().skip(slack).collect::<String>())
}

/// O host de um endereço (com ou sem esquema), em minúsculas, sem usuário, porta nem ponto final.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') { Some(v6) => v6.split(']').next()?, None => host.split(':').next()? };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// A página é do site da `url` da entrada: o mesmo host ou um subdomínio dele (`www.` da entrada não conta).
pub fn same_site(entry_url: &str, page_host: &str) -> bool {
    let Some(host) = host_of(entry_url) else { return false };
    let host = host.strip_prefix("www.").filter(|h| h.contains('.')).unwrap_or(&host);
    page_host == host || page_host.ends_with(&format!(".{host}"))
}

/// Ref de snapshot (`@eN`): o único alvo que o `fill-secret` aceita.
pub fn ref_ok(target: &str) -> bool {
    target.len() > 2 && target.starts_with("@e") && target[2..].bytes().all(|b| b.is_ascii_digit())
}

fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= TAIL_CHARS { return text.to_owned(); }
    text.chars().skip(count - TAIL_CHARS).collect()
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status.code().map(i64::from).or_else(|| status.signal().map(|s| -i64::from(s))).unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i64 { status.code().map_or(-1, i64::from) }

fn shell(command: &str) -> tokio::process::Command {
    if cfg!(windows) {
        let mut shell = tokio::process::Command::new("powershell");
        shell.args(["-NoProfile", "-Command", command]);
        shell
    } else {
        let mut shell = tokio::process::Command::new("/bin/sh");
        shell.args(["-c", command]);
        shell
    }
}

/// Os processos do comando morrem junto com a chamada largada (cancelada no meio): os netos levam o valor.
struct KillTree(Option<crate::terminal_process::CommandTree>);

impl Drop for KillTree {
    fn drop(&mut self) {
        use crate::terminal_process::OwnedTree;
        if let Some(tree) = self.0.as_mut() { let _ = tree.terminate(); }
    }
}

/// Só os últimos `READ_CAP` bytes: um comando que despeja gigabytes não enche a memória.
async fn read_tail(mut pipe: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Zeroizing<Vec<u8>>> {
    let (mut kept, mut chunk) = (Zeroizing::new(Vec::new()), Zeroizing::new(vec![0u8; 64 * 1024]));
    loop {
        let n = pipe.read(&mut chunk).await?;
        if n == 0 { return Ok(kept); }
        kept.extend_from_slice(&chunk[..n]);
        if kept.len() > 2 * READ_CAP {
            let cut = kept.len() - READ_CAP;
            let rest = Zeroizing::new(kept[cut..].to_vec());
            kept = rest;
        }
    }
}

/// `(código, cauda mascarada)`. O ambiente é o da sessão sem o que ela nunca passa adiante, mais
/// `name=valor`. No prazo o grupo inteiro morre e volta `(124, "timeout Ns")`.
pub async fn run(command: &str, cwd: &Path, base_env: Env, name: &str, value: &Zeroizing<String>, timeout: Duration)
    -> (i64, String) {
    if !cwd.is_dir() { return (-1, format!("a pasta da sessão não existe: {}", cwd.display())); }
    let mut child_cmd = shell(command);
    child_cmd.current_dir(cwd).env_clear()
        .envs(base_env.into_iter().filter(|(k, _)| !stripped(k)))
        .env(name, value.as_str())
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let tree = match crate::terminal_process::CommandTree::configure(&mut child_cmd) {
        Ok(tree) => tree,
        Err(e) => return (-1, format!("o comando não rodou: {e}")),
    };
    let mut child = match child_cmd.spawn() {
        Ok(child) => child,
        Err(e) => return (-1, format!("o shell não rodou: {e}")),
    };
    drop(child_cmd);
    let mut guard = KillTree(Some(tree));
    let tree = guard.0.as_mut().expect("árvore recém-posta");
    if let Err(e) = tree.attach(&child) {
        crate::terminal_process::finish(tree).await;
        let _ = child.wait().await;
        guard.0 = None;
        return (-1, format!("o comando não rodou: {e}"));
    }
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let done = tokio::time::timeout(timeout, async {
        let (o, e) = tokio::join!(read_tail(out), read_tail(err));
        let status = child.wait().await;
        match (o, e, status) {
            (Ok(o), Ok(e), Ok(status)) => Ok((status, o, e)),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
        }
    }).await;
    crate::terminal_process::finish(tree).await;
    guard.0 = None;
    match done {
        Err(_) => {
            let _ = child.wait().await;
            (124, format!("timeout {}s", timeout.as_secs()))
        }
        Ok(Ok((status, o, e))) => {
            let text = Zeroizing::new(format!("{}\n{}", String::from_utf8_lossy(&o), String::from_utf8_lossy(&e)));
            (exit_code(status), masked_tail(text.trim(), value))
        }
        Ok(Err(e)) => (-1, format!("a saída do comando não leu: {e}")),
    }
}

#[cfg(test)]
#[path = "tests_fill.rs"]
mod tests_fill;
