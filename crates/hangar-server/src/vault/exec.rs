//! O uso de uma senha pelo agente (tools `request_secret` e `vault_*`): o pedido na dona (aqui ou
//! pelos peers), a espera da aprovação dentro do orçamento da chamada e o comando com a senha no
//! ambiente e a saída mascarada. O valor fica em `Zeroizing` e nunca entra em texto de erro.
use std::{ffi::OsString, future::Future, path::{Path, PathBuf}, sync::Arc, time::Duration};

use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::{io::AsyncReadExt, time::Instant};
use zeroize::Zeroizing;

use super::{Summary, VaultError, KeyError, config::{self, Placement},
    remote::{Remote, RemoteError, UseStart, WaitStatus}, requests::{self, RequestError}, wire::{AskReq, UseReq, WAIT_HOLD}};
use crate::{groups::peers::PeerClient, routes::AppState};

/// A ociosidade do MCP por HTTP no Claude Code é de 5 min sem resposta nem progresso: a tool que
/// espera o dono responder precisa de mais que o orçamento de 10 min.
pub const CLAUDE_IDLE_VAR: &str = "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT";
pub const CLAUDE_IDLE_MS: &str = "660000";
/// O Codex corta a tool em 60 s sem isso.
pub const CODEX_TOOL_TIMEOUT: &str = "mcp_servers.hangar.tool_timeout_sec=660";
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
pub const NO_DEADLINE: &str = "erro_cofre_sessao_sem_prazo: esta sessão nasceu sem o prazo longo das tools do cofre e a chamada \
cairia antes da resposta do dono; reabra a sessão (recarregar) e tente de novo";
const CANCELLED: &str = "erro_cofre_cancelado: o pedido foi cancelado antes da resposta";

/// Onde mora o cofre visto desta máquina.
#[derive(Clone)]
pub enum Owner { Local(Arc<AppState>), Remote { peers: Arc<PeerClient>, id: String } }

/// O que uma espera devolveu de errado: rede (tentar de novo) ou o fim.
pub enum WaitFail { Retry(String), Stop(String) }

fn remote_text(error: &RemoteError) -> String { format!("{}: {}", error.code(), error.reason()) }

pub fn vault_text(error: VaultError) -> String {
    let (code, msg) = match error {
        VaultError::Key(KeyError::Locked) => ("erro_cofre_trancado", "o chaveiro da dona está trancado"),
        VaultError::Key(KeyError::Lost | KeyError::Absent) => ("erro_cofre_chave_perdida", "o cofre existe e a chave dele sumiu do chaveiro"),
        VaultError::Key(KeyError::Unsupported) => ("erro_cofre_sem_chaveiro", "esta máquina não tem chaveiro do sistema para o cofre"),
        VaultError::Key(KeyError::Bus) => ("erro_cofre_chaveiro_indisponivel", "o chaveiro não respondeu"),
        VaultError::Absent => ("erro_cofre_ausente", "esta máquina não tem cofre"),
        VaultError::Unreadable => ("erro_cofre_ilegivel", "o arquivo do cofre não abriu com a chave do chaveiro"),
        VaultError::Io => ("erro_cofre_indisponivel", "não foi possível ler ou gravar o cofre"),
        VaultError::UnknownEntry => ("erro_cofre_entrada_desconhecida", "senha não encontrada no cofre (veja vault_list)"),
        VaultError::DuplicateName => ("erro_cofre_nome_repetido", "já existe uma senha com esse nome"),
        VaultError::Invalid => ("erro_cofre_argumento_invalido", "nome, motivo ou alvo vazio ou longo demais"),
    };
    format!("{code}: {msg}")
}

fn request_text(error: RequestError) -> String {
    match error {
        RequestError::Vault(error) => vault_text(error),
        RequestError::Lost => "erro_cofre_pedido_perdido: a dona do cofre não conhece mais este pedido".into(),
        RequestError::TooMany => "erro_cofre_pedidos_demais: pedidos demais esperando resposta no cofre".into(),
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
                    runtime.spawn(async move { let _ = Remote { peers: &peers, owner: &owner }.cancel(&request).await; });
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
        WaitStatus::Cancelled => Err(CANCELLED.into()),
        _ => Err(off_contract()),
    }
}

/// O agente da sessão como a lista o vê: pasta de trabalho e, onde dá para ler, ambiente e argv.
pub struct Agent { pub provider: String, pub cwd: Option<PathBuf>, pub env: Option<Vec<(OsString, OsString)>>, pub argv: Vec<String> }

pub async fn agent(st: &AppState, name: &str) -> Result<Agent, String> {
    let found = st.list.fresh_session(name).await
        .map_err(|_| "erro_cofre_sessao_indisponivel: a lista de sessões não respondeu".to_owned())?;
    let Some((row, pid)) = found else { return Err(format!("sessao_desconhecida: '{name}' não está na lista de sessões")) };
    let text = |v: Option<String>| v.filter(|v| !v.is_empty());
    let cwd = text(row.git_cwd).or(text(row.cwd)).map(PathBuf::from);
    let (env, argv) = match pid {
        Some(pid) => tokio::task::spawn_blocking(move || (environ(pid), argv(pid))).await.unwrap_or_default(),
        None => (None, Vec::new()),
    };
    Ok(Agent { provider: row.provider, cwd, env, argv })
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

#[cfg(target_os = "linux")]
fn argv(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline")).map(|raw| raw.split(|b| *b == 0).filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned()).collect()).unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
fn argv(_pid: u32) -> Vec<String> { Vec::new() }

/// A sessão nasceu sem o prazo longo, quando dá para saber: Claude pelo ambiente do agente, Codex pelo
/// argv do app-server. Sem leitura (Windows, processo de outro jeito) a chamada segue.
pub fn lacks_deadline(agent: &Agent) -> bool {
    match agent.provider.as_str() {
        "claude" => agent.env.as_ref().is_some_and(|env| !env.iter().any(|(k, _)| k == CLAUDE_IDLE_VAR)),
        "codex" => agent.argv.iter().any(|a| a == "app-server") && !agent.argv.iter().any(|a| a.contains("mcp_servers.hangar.tool_timeout_sec")),
        _ => false,
    }
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

/// O valor trocado por `••••` em texto, base64 (padrão e de URL, com e sem `=`) e percent-encoded.
// shortcut: base64 só alinhado no começo do valor; dentro de outro blob (Basic auth "user:senha") passa.
pub fn mask(text: &str, value: &str) -> Zeroizing<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let mut forms: Vec<Zeroizing<String>> = vec![Zeroizing::new(value.to_owned())];
    for engine in [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD] { forms.push(Zeroizing::new(engine.encode(value))); }
    for set in [NON_ALPHANUMERIC, COMPONENT] { forms.push(Zeroizing::new(utf8_percent_encode(value, set).to_string())); }
    forms.retain(|f| !f.is_empty());
    // O mais longo antes: o base64 sem `=` é prefixo do com `=`.
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    let mut out = Zeroizing::new(text.to_owned());
    for form in &forms { out = Zeroizing::new(out.replace(form.as_str(), MASK)); }
    out
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

/// `(código, cauda mascarada)`. O ambiente é o do agente (ou o do servidor) sem os segredos do
/// servidor, mais `name=valor`. No prazo o grupo inteiro morre e volta `(124, "timeout Ns")`.
pub async fn run(command: &str, cwd: &Path, base_env: Vec<(OsString, OsString)>, name: &str, value: &Zeroizing<String>, timeout: Duration)
    -> (i64, String) {
    if !cwd.is_dir() { return (-1, format!("a pasta da sessão não existe: {}", cwd.display())); }
    let mut child_cmd = shell(command);
    child_cmd.current_dir(cwd).env_clear()
        .envs(base_env.into_iter().filter(|(k, _)| !crate::terminal_process::PRIVATE_ENV_KEYS.iter().any(|p| k == p)))
        .env(name, value.as_str())
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let mut tree = match crate::terminal_process::CommandTree::configure(&mut child_cmd) {
        Ok(tree) => tree,
        Err(e) => return (-1, format!("o comando não rodou: {e}")),
    };
    let mut child = match child_cmd.spawn() {
        Ok(child) => child,
        Err(e) => return (-1, format!("o shell não rodou: {e}")),
    };
    drop(child_cmd);
    if let Err(e) = tree.attach(&child) {
        crate::terminal_process::finish(&mut tree).await;
        let _ = child.wait().await;
        return (-1, format!("o comando não rodou: {e}"));
    }
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let done = tokio::time::timeout(timeout, async {
        let (mut o, mut e) = (Zeroizing::new(Vec::new()), Zeroizing::new(Vec::new()));
        let (ro, re) = tokio::join!(out.read_to_end(&mut o), err.read_to_end(&mut e));
        let status = child.wait().await;
        (ro.and(re).and(status), o, e)
    }).await;
    crate::terminal_process::finish(&mut tree).await;
    match done {
        Err(_) => {
            let _ = child.wait().await;
            (124, format!("timeout {}s", timeout.as_secs()))
        }
        Ok((Ok(status), o, e)) => {
            let text = Zeroizing::new(format!("{}\n{}", String::from_utf8_lossy(&o), String::from_utf8_lossy(&e)));
            let masked = mask(text.trim(), value);
            (exit_code(status), tail(&masked))
        }
        Ok((Err(e), _, _)) => (-1, format!("a saída do comando não leu: {e}")),
    }
}
