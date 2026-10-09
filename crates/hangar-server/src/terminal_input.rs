//! Escritor Claude terminal; a fila e a política pertencem ao executor do runtime.
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, future::Future, path::PathBuf, pin::Pin, sync::{Arc, LazyLock, atomic::{AtomicU64, Ordering}}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::Command, sync::Mutex, time::timeout};

pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'a>>;
pub type IoFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IoFailure>> + Send + 'a>>;
#[derive(Clone, Copy, Debug)]
pub struct ServiceError(pub &'static str);
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalBinding {
    pub name: String,
    pub pane: String,
    pub conversation: String,
    pub generation: u64,
    pub created: u64,
    pub mux_argv: Vec<String>,
    pub windows: bool,
    pub clipboard_lock_path: Option<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMessage {
    pub socket: PathBuf,
    pub origin: String,
    pub sender: String,
    pub mode: String,
    pub message_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputFacts {
    pub binding: TerminalBinding,
    pub ready: bool,
    pub idle: bool,
    pub open_question: bool,
    pub plugin_live: bool,
    pub plugin_user: bool,
    #[serde(default)]
    pub clipboard_available: bool,
    pub native: Option<NativeMessage>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginMode { Fill, User }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRequest { pub id: String, pub text: String, pub mode: PluginMode }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginReply { Unavailable, NotWritten, Filled, Accepted, Unknown }
pub trait TerminalServices: Send + Sync {
    fn facts<'a>(&'a self, binding: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts>;
    fn publish<'a>(&'a self, binding: &'a TerminalBinding, request: PluginRequest) -> ServiceFuture<'a, PluginReply>;
    /// Grava no diário que a escrita vai começar: antes disso, cair não deixa a entrega incerta.
    fn writing<'a>(&'a self) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Disposition { Accepted, Deferred, Rejected, Unknown }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage { Validate, Identity, Ready, Composer, Native, Plugin, Write, InputProof, Submit, SubmitProof, Cleanup, Control, Select, Answer, Steer }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Cleanup { NotNeeded, Proved, Unproved }
/// Onde ficou o rascunho do dono que o escritor guardou para entregar.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftOutcome { Returned, Stashed, Unverified }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliveryResult {
    pub disposition: Disposition,
    pub stage: DeliveryStage,
    pub cleanup: Cleanup,
    pub native: bool,
    pub message_id: Option<String>,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftOutcome>,
}
impl DeliveryResult {
    fn new(disposition: Disposition, stage: DeliveryStage, code: &str) -> Self {
        Self { disposition, stage, cleanup: Cleanup::NotNeeded, native: false, message_id: None, code: code.into(), draft: None }
    }
}
#[derive(Clone, Debug)]
pub struct CommandRequest { pub program: String, pub args: Vec<String>, pub stdin: Vec<u8> }
#[derive(Clone, Debug)]
pub struct CommandOutput { pub success: bool, pub stdout: Vec<u8> }
#[derive(Clone, Copy, Debug)]
pub struct IoFailure { pub code: &'static str, pub may_have_written: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOutcome { Written, NotWritten, Unknown }
pub trait TerminalIo: Send + Sync {
    fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput>;
    fn socket<'a>(&'a self, descriptor: &'a NativeMessage, envelope: Vec<u8>) -> IoFuture<'a, WriteOutcome>;
}
#[derive(Clone)]
pub struct ProcessIo { pub command_timeout: Duration, pub socket_timeout: Duration }
impl Default for ProcessIo { fn default() -> Self { Self { command_timeout: Duration::from_secs(3), socket_timeout: Duration::from_secs(3) } } }
pub(crate) fn child_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    // O multiplexador e o clipboard não recebem a autoridade privada do servidor.
    for name in ["HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE", "CP_AUTH_TOKEN"] { command.env_remove(name); }
    command
}
impl TerminalIo for ProcessIo {
    fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput> {
        Box::pin(async move {
            use std::process::Stdio;
            let mut command=child_command(request.program);
            command.args(request.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
            let mut tree=crate::terminal_process::CommandTree::configure(&mut command)
                .map_err(|_|IoFailure {code:"command_containment",may_have_written:false})?;
            let mut child = command.spawn()
                .map_err(|_| IoFailure { code: "spawn_failed", may_have_written: false })?;
            if tree.attach(&child).is_err() {
                crate::terminal_process::finish(&mut tree).await;
                child.wait().await.map_err(|_|IoFailure {code:"command_wait_failed",may_have_written:true})?;
                return Err(IoFailure {code:"command_containment",may_have_written:true});
            }
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let work = async {
                let input = async move {
                    stdin.write_all(&request.stdin).await?;
                    drop(stdin);
                    Ok::<_, std::io::Error>(())
                };
                let output = async {
                    let mut bytes = Vec::new();
                    (&mut stdout).take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).await?;
                    if bytes.len() > 8 * 1024 * 1024 { return Err(std::io::Error::other("output limit")); }
                    Ok(bytes)
                };
                let (_, bytes) = tokio::try_join!(input, output)?;
                // Líder recolhido antes do `finish` soltaria o número do grupo para reuso.
                if let Err(error) = crate::terminal_process::leader_exited(&mut child).await {
                    if crate::warn_limit::allow(None, "command_wait_proof_failed") {
                        tracing::warn!(code="command_wait_proof_failed", io_kind=?error.kind(), os_error=?error.raw_os_error(),
                            reason="fim do comando não comprovado", "comando do multiplexador");
                    }
                    return Err(error);
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let result=timeout(self.command_timeout,work).await;
            crate::terminal_process::finish(&mut tree).await;
            let status=child.wait().await.map_err(|_|IoFailure {code:"command_wait_failed",may_have_written:true})?;
            match result {Ok(Ok(stdout))=>Ok(CommandOutput {success:status.success(),stdout}),_=>Err(IoFailure {code:"command_uncertain",may_have_written:true})}
        })
    }
    fn socket<'a>(&'a self, descriptor: &'a NativeMessage, envelope: Vec<u8>) -> IoFuture<'a, WriteOutcome> {
        Box::pin(async move {
            #[cfg(unix)] {
                let Ok(Ok(mut stream)) = timeout(self.socket_timeout, tokio::net::UnixStream::connect(&descriptor.socket)).await else { return Ok(WriteOutcome::NotWritten); };
                // Depois de começar a escrita, nenhum erro autoriza outro transporte.
                if !matches!(timeout(self.socket_timeout, stream.write_all(&envelope)).await, Ok(Ok(()))) { return Ok(WriteOutcome::Unknown); }
                let _ = stream.shutdown().await;
                let mut reply = [0u8; 4096];
                let _ = timeout(Duration::from_secs(1), stream.read(&mut reply)).await;
                Ok(WriteOutcome::Written)
            }
            #[cfg(not(unix))] { let _ = (descriptor, envelope); Ok(WriteOutcome::NotWritten) }
        })
    }
}
#[derive(Clone)]
pub struct InputLimits {
    pub settle: Duration,
    pub literal_settle: Duration,
    pub multiline_settle: Duration,
    pub slash_settle: Duration,
    pub proof_attempts: usize,
    pub ready_attempts: usize,
    pub cleanup_attempts: usize,
}
impl Default for InputLimits {
    fn default() -> Self {
        Self { settle: Duration::from_millis(150), literal_settle: Duration::from_millis(200),
            multiline_settle: Duration::from_millis(500), slash_settle: Duration::from_millis(300),
            proof_attempts: 40, ready_attempts: 80, cleanup_attempts: 20 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof { Present, Absent, Unreadable }
#[derive(Clone, Debug)]
pub struct ComposerSnapshot { pub content: String, pub placeholders: BTreeSet<String>, pub stashed: bool }
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(Pasted text|Image) #(\d+)").unwrap());
static CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*[❯›]\s*(\d+)\.\s").unwrap());
static AGENT_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:❯\s+)?[●◯]\s+(\S+)").unwrap());
static NEXT_BUFFER: AtomicU64 = AtomicU64::new(0);
fn compact(text: &str) -> String { text.chars().filter(|c| !c.is_whitespace() && !"│┃║".contains(*c)).collect() }
/// O Claude marca `› stashed` na linha de dicas acima do composer enquanto guarda um rascunho.
fn stash_held(screen: &str) -> bool {
    let lines: Vec<_> = screen.split('\n').collect();
    let rules: Vec<_> = lines.iter().enumerate().filter(|(_, s)| s.matches('─').count() >= 20).map(|(n, _)| n).collect();
    rules.len().checked_sub(2).and_then(|n| rules[n].checked_sub(1)).is_some_and(|n| lines[n].contains("› stashed"))
}
impl ComposerSnapshot {
    pub fn parse(screen: &str) -> Option<Self> {
        let mut lines: Vec<_> = screen.split('\n').collect();
        while lines.last().is_some_and(|s| s.trim().is_empty()) { lines.pop(); }
        // O painel de agentes ganha uma linha por subagente abaixo do rodapé: fora da conta da distância.
        // Só corta com a linha `main` e um `◯`: bloco só de `●` é conversa, e opção `◯` sem `main` é diálogo.
        let panel = lines.iter().rev().take_while(|s| AGENT_ROW.is_match(s)).count();
        let rows = &lines[lines.len() - panel..];
        if rows.iter().any(|s| s.contains('◯')) && rows.iter().any(|s| AGENT_ROW.captures(s).is_some_and(|c| &c[1] == "main")) {
            lines.truncate(lines.len() - panel);
            while lines.last().is_some_and(|s| s.trim().is_empty()) { lines.pop(); }
        }
        let rules: Vec<_> = lines.iter().enumerate().filter(|(_, s)| s.matches('─').count() >= 20).map(|(n, _)| n).collect();
        let bottom = *rules.last()?;
        let top = *rules.get(rules.len().checked_sub(2)?)?;
        if lines.len() - bottom > 8 || bottom - top > 15 { return None; }
        let content = lines[top+1..bottom].iter().map(|s| s.trim().strip_prefix('❯').unwrap_or(s.trim()).trim()).collect::<Vec<_>>().join("\n");
        let placeholders = PLACEHOLDER.captures_iter(&content).map(|c| format!("{}:{}", &c[1], &c[2])).collect();
        Some(Self { content, placeholders, stashed: stash_held(screen) })
    }
    pub fn is_empty(&self) -> bool { self.content.trim().is_empty() }
    pub fn proves(&self, text: &str, before: &Self) -> Proof {
        if self.placeholders.difference(&before.placeholders).next().is_some() { return Proof::Present; }
        let visible = compact(&self.content);
        let expected = compact(text);
        if expected.is_empty() { return Proof::Unreadable; }
        if expected.chars().count() < 12 {
            return if visible == expected { Proof::Present } else if self.is_empty() { Proof::Absent } else { Proof::Unreadable };
        }
        let head: String = text.trim().chars().take(40).collect();
        let tail: String = text.trim().rsplit('\n').next().unwrap_or("").trim().chars().rev().take(40).collect::<String>().chars().rev().collect();
        if [head, tail].iter().map(|s| compact(s)).any(|s| s.chars().count() >= 12 && visible.contains(&s)) { Proof::Present } else { Proof::Absent }
    }
    fn owned(&self, text: &str, before: &Self) -> bool {
        if compact(&self.content) == compact(text) { return true; }
        let mut residual = self.content.clone();
        for hit in PLACEHOLDER.find_iter(&self.content) {
            let end = self.content[hit.start()..].find(']').map(|n| hit.start()+n+1);
            if let Some(end) = end { residual = residual.replace(&self.content[hit.start()..end], ""); }
        }
        !self.placeholders.is_empty() && self.placeholders.is_disjoint(&before.placeholders) && residual.trim().is_empty()
    }
}
/// Tira as sequências SGR da captura `-e`. Sem `keep_dim`, some também o texto esmaecido: é a
/// sugestão que o Claude desenha no composer vazio, e ninguém a digitou.
pub fn unstyle(styled: &str, keep_dim: bool) -> String {
    let mut out = String::with_capacity(styled.len());
    let mut dim = false;
    let mut chars = styled.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if keep_dim || !dim || c == '\n' { out.push(c); }
            continue;
        }
        if chars.next_if_eq(&'[').is_none() { continue; }
        let mut params = String::new();
        let mut last = None;
        for n in chars.by_ref() {
            if ('@'..='~').contains(&n) { last = Some(n); break; }
            params.push(n);
        }
        if last != Some('m') { continue; }
        let mut codes = params.split([';', ':']).map(|p| p.parse::<u32>().unwrap_or(0));
        while let Some(code) = codes.next() {
            match code {
                0 | 22 => dim = false,
                2 => dim = true,
                // O `2` de `38;2;r;g;b` é truecolor, não esmaecido.
                38 | 48 | 58 => match codes.next() { Some(5) => { codes.next(); } Some(2) => { codes.nth(2); } _ => {} },
                _ => {}
            }
        }
    }
    out
}
fn valid_text(text: &str) -> bool { !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') }
fn cursor(screen: &str) -> Option<usize> { CURSOR.captures_iter(screen).last()?.get(1)?.as_str().parse().ok() }
fn live_picker(screen: &str) -> bool { cursor(screen).is_some() && crate::terminal_state::analyze(screen).overlay }
fn overlay(screen: &str) -> bool {
    let analysis = crate::terminal_state::analyze(screen);
    analysis.overlay || analysis.state == "awaiting_input"
}

/// O que o clique de mod precisa saber do pane antes de cada passo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneFormats { pub mouse: bool, pub in_mode: bool, pub columns: u16, pub rows: u16 }
/// `|` e não tab: o psmux devolve vazio para formato que não conhece, e o campo vazio precisa sobrar.
pub const MODS_FORMATS: &str = "#{mouse_sgr_flag}|#{alternate_on}|#{pane_in_mode}|#{window_width}|#{window_height}";
impl PaneFormats {
    /// Valor vazio nunca vale 0: sem `mouse_sgr_flag` (psmux) o sinal do mouse é a tela alternativa, que o
    /// Claude Code só usa em tela cheia; `pane_in_mode` vazio é ilegível, não "fora de modo".
    pub fn parse(text: &str) -> Option<Self> {
        let fields: Vec<&str> = text.trim_end_matches(['\n', '\r']).split('|').collect();
        let [sgr, alternate, mode, columns, rows] = fields.as_slice() else { return None };
        if !matches!(*mode, "0" | "1") { return None; }
        let mouse = if sgr.is_empty() { *alternate == "1" } else { *sgr == "1" };
        Some(Self { mouse, in_mode: *mode == "1", columns: columns.parse().ok()?, rows: rows.parse().ok()? })
    }
}
/// As teclas da reserva por teclado (T5): uma por chamada, e só os dois acordes medidos juntos.
const MODS_KEYS: [&[&str]; 4] = [&["C-x", "Tab"], &["Tab"], &["Enter"], &["C-x", "x"]];

pub struct TerminalDriver {
    binding: TerminalBinding,
    services: Arc<dyn TerminalServices>,
    io: Arc<dyn TerminalIo>,
    limits: InputLimits,
    serial: Mutex<()>,
    /// A identidade do pane já foi conferida por quem criou o driver: as operações de mod não a refazem.
    pane_checked: bool,
    /// Sem o Esc que devolve o foco do rodapé ao composer: a linha já tentou o bastante.
    footer_kept: bool,
}
impl TerminalDriver {
    pub fn new(binding: TerminalBinding, services: Arc<dyn TerminalServices>, io: Arc<dyn TerminalIo>, limits: InputLimits) -> Self { Self { binding, services, io, limits, serial: Mutex::new(()), pane_checked: false, footer_kept: false } }
    /// Driver para as operações de um clique de mod cujo pane já teve a identidade conferida na mesma
    /// reserva (`PaneOp::Hold`): sem a conferência, cada operação é um processo do multiplexador a menos.
    pub fn pane_checked(mut self) -> Self { self.pane_checked = true; self }
    pub fn footer_kept(mut self) -> Self { self.footer_kept = true; self }
    pub fn binding(&self) -> &TerminalBinding { &self.binding }
    fn request(&self, args: Vec<String>, stdin: Vec<u8>) -> Result<CommandRequest, IoFailure> {
        let (program, prefix) = self.binding.mux_argv.split_first().ok_or(IoFailure { code: "mux_missing", may_have_written: false })?;
        Ok(CommandRequest { program: program.clone(), args: prefix.iter().cloned().chain(args).collect(), stdin })
    }
    async fn raw(&self, args: Vec<String>, stdin: Vec<u8>) -> Result<CommandOutput, IoFailure> { self.io.command(self.request(args, stdin)?).await }
    async fn facts(&self) -> Result<InputFacts, IoFailure> {
        let facts = self.services.facts(&self.binding).await.map_err(|_| IoFailure { code: "facts_unavailable", may_have_written: false })?;
        if facts.binding != self.binding || self.binding.name.is_empty() || self.binding.conversation.is_empty()
            || self.binding.mux_argv.is_empty() || (self.binding.windows && !self.binding.pane.starts_with(&format!("={}:", self.binding.name))) {
            return Err(IoFailure { code: "stale_binding", may_have_written: false });
        }
        Ok(facts)
    }
    async fn verify(&self) -> Result<InputFacts, IoFailure> {
        let facts = self.facts().await?;
        self.verify_pane().await?;
        Ok(facts)
    }
    /// Mesmo pane e mesma vida do multiplexador, sem perguntar pela conversa.
    async fn verify_pane(&self) -> Result<(), IoFailure> {
        if self.pane_checked { return Ok(()); }
        let output = self.raw(vec!["display-message".into(), "-p".into(), "-t".into(), self.binding.pane.clone(), "#{session_name}\t#{pane_id}\t#{session_created}".into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "identity_failed", may_have_written: false }); }
        let output = std::str::from_utf8(&output.stdout).map_err(|_| IoFailure { code: "identity_utf8", may_have_written: false })?;
        let fields: Vec<_> = output.trim().split('\t').collect();
        // No Windows o pane é sempre %1; só o alvo completo distingue as sessões.
        if fields.len() != 3 || fields[0] != self.binding.name || (!self.binding.windows && fields[1] != self.binding.pane)
            || fields[2].parse::<u64>().ok() != Some(self.binding.created) {
            return Err(IoFailure { code: "stale_mux", may_have_written: false });
        }
        Ok(())
    }
    async fn effect(&self, args: Vec<String>, stdin: Vec<u8>) -> Result<(), IoFailure> {
        self.verify().await?;
        let output = self.raw(args, stdin).await?;
        if !output.success { return Err(IoFailure { code: "mux_effect_failed", may_have_written: true }); }
        Ok(())
    }
    async fn capture_inner(&self) -> Result<String, IoFailure> {
        self.verify().await?;
        let output = self.raw(vec!["capture-pane".into(), "-p".into(), "-t".into(), self.binding.pane.clone(), "-S".into(), "-200".into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "capture_failed", may_have_written: false }); }
        String::from_utf8(output.stdout).map_err(|_| IoFailure { code: "capture_utf8", may_have_written: false })
    }
    /// Uma captura para ler o composer: a tela inteira e só o que foi digitado.
    async fn composer_capture(&self) -> Result<(String, String), IoFailure> {
        self.verify().await?;
        self.composer_capture_unverified().await
    }
    /// Leitura sem conferir a conversa: só para provar o próprio `/clear`, que a troca.
    async fn composer_capture_unverified(&self) -> Result<(String, String), IoFailure> {
        // Também no psmux: sem estilo, a sugestão esmaecida do composer vazio parece texto digitado.
        let args = vec!["capture-pane".into(), "-p".into(), "-e".into(), "-t".into(), self.binding.pane.clone(), "-S".into(), "-200".into()];
        let output = self.raw(args, vec![]).await?;
        if !output.success { return Err(IoFailure { code: "capture_failed", may_have_written: false }); }
        let styled = String::from_utf8(output.stdout).map_err(|_| IoFailure { code: "capture_utf8", may_have_written: false })?;
        Ok((unstyle(&styled, true), unstyle(&styled, false)))
    }
    pub async fn capture(&self) -> Result<String, IoFailure> { let _serial = self.serial.lock().await; self.capture_inner().await }
    /// Efeito de mod no pane: confere só a identidade do pane (sem os fatos do Python, que pesam e não
    /// mudam um clique de mouse) e não entra no diário da fila.
    async fn pane_effect(&self, args: Vec<String>) -> Result<(), IoFailure> {
        self.verify_pane().await?;
        let output = self.raw(args, vec![]).await?;
        if !output.success { return Err(IoFailure { code: "mux_effect_failed", may_have_written: true }); }
        Ok(())
    }
    fn window(&self) -> String { format!("={}:", self.binding.name) }
    pub async fn mods_formats(&self) -> Result<PaneFormats, IoFailure> {
        self.verify_pane().await?;
        let output = self.raw(vec!["display-message".into(), "-p".into(), "-t".into(), self.binding.pane.clone(), MODS_FORMATS.into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "formats_failed", may_have_written: false }); }
        PaneFormats::parse(&String::from_utf8_lossy(&output.stdout)).ok_or(IoFailure { code: "formats_unreadable", may_have_written: false })
    }
    /// Terminais de verdade ligados à sessão. No tmux, o observador da prévia e o vigia de tamanho do
    /// Hangar são clientes de controle e entram no `#{session_attached}` (medido no tmux 3.7c): contam só os
    /// outros. No psmux vale o `#{session_attached}`, e vazio nunca vale 0: o ramo depende de nenhum cliente
    /// de controle do Hangar se ligar à sessão no Windows, o que o `TerminalPool` (observador da prévia) e o
    /// `watch_notices` (vigia) garantem recusando lá. Quem fizer um deles rodar no Windows troca este ramo
    /// pelo do tmux, conferido antes com o `list-clients -F` do psmux (Task 20).
    pub async fn mods_clients(&self) -> Result<usize, IoFailure> {
        self.verify_pane().await?;
        if self.binding.windows {
            let output = self.raw(vec!["display-message".into(), "-p".into(), "-t".into(), self.binding.pane.clone(), "#{session_attached}".into()], vec![]).await?;
            if !output.success { return Err(IoFailure { code: "clients_failed", may_have_written: false }); }
            return String::from_utf8_lossy(&output.stdout).trim().parse().map_err(|_| IoFailure { code: "clients_unreadable", may_have_written: false });
        }
        let output = self.raw(vec!["list-clients".into(), "-t".into(), format!("={}", self.binding.name), "-F".into(), "#{client_flags}".into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "clients_failed", may_have_written: false }); }
        Ok(String::from_utf8_lossy(&output.stdout).lines().filter(|l| !l.trim().is_empty() && !l.split(',').any(|f| f == "control-mode")).count())
    }
    /// Só a parte visível, com atributos: é nela que as coordenadas do mouse valem.
    pub async fn mods_screen(&self) -> Result<String, IoFailure> {
        self.verify_pane().await?;
        let output = self.raw(vec!["capture-pane".into(), "-p".into(), "-e".into(), "-t".into(), self.binding.pane.clone()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "capture_failed", may_have_written: false }); }
        String::from_utf8(output.stdout).map_err(|_| IoFailure { code: "capture_utf8", may_have_written: false })
    }
    /// Clique SGR (botão esquerdo, apertar e soltar), linha e coluna a partir de 0; não tira o teclado
    /// do prompt. A sequência é montada só aqui: mal formada, ela vira texto no prompt (psmux).
    pub async fn mouse(&self, row: u16, col: u16) -> Result<(), IoFailure> {
        let (c, r) = (u32::from(col) + 1, u32::from(row) + 1);
        self.pane_effect(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(),
            format!("\u{1b}[<0;{c};{r}M\u{1b}[<0;{c};{r}m")]).await
    }
    /// Um evento de roda, com o ponteiro sobre o corpo do painel.
    pub async fn wheel(&self, row: u16, col: u16, down: bool) -> Result<(), IoFailure> {
        let (c, r) = (u32::from(col) + 1, u32::from(row) + 1);
        self.pane_effect(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(),
            format!("\u{1b}[<{};{c};{r}M", if down { 65 } else { 64 })]).await
    }
    pub async fn mods_keys(&self, keys: &[&str]) -> Result<(), IoFailure> {
        if !MODS_KEYS.contains(&keys) { return Err(IoFailure { code: "key_not_allowed", may_have_written: false }); }
        let mut args = vec!["send-keys".into(), "-t".into(), self.binding.pane.clone()];
        // Enter como CR cru fora do Windows, como no `key_inner`: com `extended-keys` o nome sai codificado.
        if keys == ["Enter"] && !self.binding.windows { args.extend(["-l".into(), "--".into(), "\r".into()]); }
        else { args.extend(keys.iter().map(|k| k.to_string())); }
        self.pane_effect(args).await
    }
    /// Redimensiona e devolve a janela ao `window-size latest`: sozinho, o `resize-window` a deixa em
    /// `manual`, e o terminal de quem se ligar depois não manda mais no tamanho.
    pub async fn resize(&self, columns: u16, rows: u16) -> Result<(), IoFailure> {
        self.pane_effect(vec!["resize-window".into(), "-t".into(), self.window(), "-x".into(), columns.to_string(), "-y".into(), rows.to_string()]).await?;
        self.pane_effect(vec!["set-window-option".into(), "-t".into(), self.window(), "window-size".into(), "latest".into()]).await
    }
    /// Só leitura: composer legível e vazio, então uma entrega adiada pode tentar já. Quem chama
    /// acabou de conferir a conversa pelos fatos; aqui basta o mesmo pane.
    pub async fn composer_free(&self) -> bool {
        let _serial = self.serial.lock().await;
        if self.verify_pane().await.is_err() { return false; }
        let Ok((screen, typed)) = self.composer_capture_unverified().await else { return false };
        Self::composer(&screen, &typed).is_ok_and(|draft| draft.is_empty())
    }
    async fn settle(&self) { tokio::time::sleep(self.limits.settle).await; }
    async fn key_inner(&self, key: &str) -> Result<(), IoFailure> {
        let mut args = vec!["send-keys".into(), "-t".into(), self.binding.pane.clone()];
        if key == "Enter" && !self.binding.windows { args.extend(["-l".into(), "--".into(), "\r".into()]); }
        else if key == "Escape" && self.binding.windows { args.extend(["-l".into(), "--".into(), "\u{1b}[27;1;27;1;0;1_\u{1b}[27;1;27;0;0;1_".into()]); }
        else { args.push(key.into()); }
        self.effect(args, vec![]).await
    }
    async fn literal(&self, text: &str) -> Result<(), IoFailure> {
        if text.is_empty() { return Ok(()); }
        if !self.binding.windows {
            // O tmux lê o ponto e vírgula final como separador mesmo depois de --.
            let text = text.strip_suffix(';').map_or_else(|| text.to_string(), |s| format!("{s}\\;"));
            return self.effect(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(), text], vec![]).await;
        }
        let placeholder = text.starts_with('-');
        let text = if placeholder { format!("x{text}") } else { text.into() };
        let chars: Vec<_> = text.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            let mut end = (start + 512).min(chars.len());
            while end < chars.len() && chars[end] == '-' && end-start < 700 { end += 1; }
            let part: String = chars[start..end].iter().collect();
            self.effect(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(), part], vec![]).await
                .map_err(|mut e| { e.may_have_written |= start > 0; e })?;
            start = end;
            if start < chars.len() { tokio::time::sleep(self.limits.slash_settle).await; }
        }
        if placeholder {
            self.key_inner("Home").await.map_err(|mut e| { e.may_have_written = true; e })?;
            self.key_inner("DC").await.map_err(|mut e| { e.may_have_written = true; e })?;
            if self.capture_inner().await.map_err(|mut e| { e.may_have_written = true; e })?.contains(&text) {
                return Err(IoFailure { code: "placeholder_unremoved", may_have_written: true });
            }
        }
        Ok(())
    }
    async fn paste(&self, text: &str, id: &str) -> Result<(), IoFailure> {
        let buffer = format!("hangar-{}-{}-{}", std::process::id(), NEXT_BUFFER.fetch_add(1, Ordering::Relaxed),
            id.chars().filter(|c| c.is_ascii_alphanumeric()).take(64).collect::<String>());
        self.verify().await?;
        let load = self.raw(vec!["load-buffer".into(), "-b".into(), buffer.clone(), "-".into()], text.as_bytes().to_vec()).await?;
        if !load.success { return Err(IoFailure { code: "buffer_failed", may_have_written: false }); }
        self.effect(vec!["paste-buffer".into(), "-t".into(), self.binding.pane.clone(), "-b".into(), buffer, "-p".into(), "-d".into()], vec![]).await
    }
    async fn clipboard(&self, text: &str) -> Result<(), IoFailure> {
        if !self.verify().await?.clipboard_available { return Err(IoFailure { code: "clipboard_unavailable", may_have_written: false }); }
        let output = self.io.command(CommandRequest { program: "powershell.exe".into(), args: vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $text=[Console]::In.ReadToEnd(); Set-Clipboard -Value $text; if ((Get-Clipboard -Raw) -cne $text) { exit 1 }".into()], stdin: text.as_bytes().to_vec() }).await?;
        if !output.success { return Err(IoFailure { code: "clipboard_failed", may_have_written: false }); }
        self.key_inner("M-v").await
    }
    async fn clipboard_lock(&self) -> Result<Option<std::fs::File>, IoFailure> {
        if !self.binding.windows { return Ok(None); }
        let path = self.binding.clipboard_lock_path.as_ref().ok_or(IoFailure { code: "clipboard_lock_missing", may_have_written: false })?;
        let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path)
            .map_err(|_| IoFailure { code: "clipboard_lock_open", may_have_written: false })?;
        for _ in 0..self.limits.ready_attempts.max(1) {
            if file.try_lock().is_ok() { self.verify().await?; return Ok(Some(file)); }
            self.settle().await;
        }
        Err(IoFailure { code: "clipboard_lock_busy", may_have_written: false })
    }
    async fn snapshot(&self) -> Result<ComposerSnapshot, IoFailure> {
        let (screen, typed) = self.composer_capture().await?;
        Self::composer(&screen, &typed)
    }
    fn composer(screen: &str, typed: &str) -> Result<ComposerSnapshot, IoFailure> {
        if overlay(screen) { return Err(IoFailure { code: "overlay", may_have_written: false }); }
        if crate::terminal_state::footer_focus(screen) { return Err(IoFailure { code: "footer_focus", may_have_written: false }); }
        let mut snapshot = ComposerSnapshot::parse(typed).ok_or(IoFailure { code: "composer_unreadable", may_have_written: false })?;
        snapshot.stashed = stash_held(screen);
        Ok(snapshot)
    }
    /// Com o foco no rodapé do Claude Code (painel de agentes, pílula de tarefas) o texto digitado some
    /// e o `x` para um subagente. Um Esc lá só devolve o foco ao composer, sem interromper o turno; o
    /// foco que não volta adia a entrada sem digitar.
    async fn return_footer_focus(&self) -> Result<ComposerSnapshot, IoFailure> {
        self.key_inner("Escape").await.map_err(|e| IoFailure { may_have_written: false, ..e })?;
        let mut last = IoFailure { code: "footer_focus", may_have_written: false };
        for _ in 0..4 {
            self.settle().await;
            match self.snapshot().await { Err(e) if e.code == "footer_focus" => last = e, other => return other }
        }
        Err(last)
    }
    async fn refresh_input_guard(&self) -> Result<ComposerSnapshot, IoFailure> {
        let facts = self.verify().await?;
        if facts.open_question || !facts.ready {
            return Err(IoFailure { code: "input_unavailable", may_have_written: false });
        }
        let before = self.snapshot().await?;
        // O rascunho surgido durante a espera pertence ao dono.
        if !before.is_empty() { return Err(IoFailure { code: "composer_busy", may_have_written: false }); }
        Ok(before)
    }
    /// Põe o rascunho do dono no guardado do Claude (Ctrl+S), que o devolve sozinho no envio.
    async fn stash_draft(&self, draft: &ComposerSnapshot) -> Result<ComposerSnapshot, IoFailure> {
        let busy = IoFailure { code: "composer_busy", may_have_written: false };
        // Nada da mensagem saiu: o Ctrl+S incerto é do rascunho, e o `settle_draft` o confere.
        self.key_inner("C-s").await.map_err(|e| IoFailure { may_have_written: false, ..e })?;
        for _ in 0..self.limits.cleanup_attempts.max(1) {
            self.settle().await;
            let now = self.snapshot().await?;
            if now.is_empty() && now.stashed { return Ok(now); }
            // Tecla sem efeito é CLI sem guardado; texto diferente é o dono digitando.
            if !now.is_empty() && now.content != draft.content { break; }
        }
        Err(busy)
    }
    /// Sem envio, o escritor devolve o guardado; com envio incerto, nenhum Ctrl+S às cegas. Só
    /// sobre composer vazio: com texto novo do dono, o Ctrl+S guardaria esse texto por cima.
    async fn settle_draft(&self, draft: &ComposerSnapshot, disposition: Disposition) -> DraftOutcome {
        let mut pressed = false;
        for _ in 0..self.limits.cleanup_attempts.max(1) {
            // Só o pane: o `/clear` enviado já trocou a conversa.
            let read = match self.verify_pane().await { Ok(()) => self.composer_capture_unverified().await, Err(e) => Err(e) };
            let Ok(now) = read.and_then(|(screen, typed)| Self::composer(&screen, &typed)) else { return DraftOutcome::Unverified };
            if !now.stashed {
                return if compact(&now.content) == compact(&draft.content) { DraftOutcome::Returned } else { DraftOutcome::Unverified };
            }
            if disposition == Disposition::Deferred && !pressed && now.is_empty() {
                if self.key_inner("C-s").await.is_err() { return DraftOutcome::Unverified; }
                pressed = true;
            } else if disposition == Disposition::Deferred && !pressed { return DraftOutcome::Stashed; }
            self.settle().await;
        }
        DraftOutcome::Stashed
    }
    async fn prove_input(&self, text: &str, before: &ComposerSnapshot) -> bool {
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            let Ok(now) = self.snapshot().await else { return false; };
            if now.proves(text, before) == Proof::Present { return true; }
        }
        false
    }
    async fn cleanup_owned(&self, text: &str, before: &ComposerSnapshot) -> bool {
        let Ok(mut now) = self.snapshot().await else { return false; };
        if !now.owned(text, before) { return false; }
        for _ in 0..self.limits.cleanup_attempts {
            if self.key_inner("C-u").await.is_err() { return false; }
            self.settle().await;
            let Ok(after) = self.snapshot().await else { return false; };
            if after.is_empty() { return true; }
            if !after.owned(text, before) || after.content.chars().count() >= now.content.chars().count() { return false; }
            now = after;
        }
        false
    }
    fn failed(error: IoFailure, stage: DeliveryStage) -> DeliveryResult {
        DeliveryResult::new(if error.may_have_written { Disposition::Unknown } else { Disposition::Deferred }, stage, error.code)
    }
    async fn partial(&self, text: &str, before: &ComposerSnapshot, stage: DeliveryStage) -> DeliveryResult {
        let cleaned = self.cleanup_owned(text, before).await;
        let mut result = DeliveryResult::new(if cleaned { Disposition::Deferred } else { Disposition::Unknown }, stage, "input_unproved");
        result.cleanup = if cleaned { Cleanup::Proved } else { Cleanup::Unproved };
        result
    }
    async fn submit(&self, text: &str, before: &ComposerSnapshot, draft: Option<&ComposerSnapshot>) -> DeliveryResult {
        if !self.verify().await.is_ok_and(|facts| !facts.open_question) {
            return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, "submission_blocked");
        }
        if let Err(error) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, error.code); }
        let mut slash_selected = false;
        let clear = text.split_whitespace().next() == Some("/clear");
        let mut last_error = None;
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            let capture = if clear {
                match self.verify_pane().await { Ok(()) => self.composer_capture_unverified().await, Err(e) => Err(e) }
            } else { self.composer_capture().await };
            last_error = capture.as_ref().err().map(|error| error.code);
            if let Ok((screen, typed)) = capture {
                // Só aceita se o texto também sumiu da tela com estilo: esmaecido nunca prova envio.
                if ComposerSnapshot::parse(&typed).is_some_and(|now| now.is_empty())
                    && !ComposerSnapshot::parse(&screen).is_some_and(|now| now.proves(text, before) == Proof::Present) {
                    return DeliveryResult::new(Disposition::Accepted, DeliveryStage::SubmitProof, "submitted");
                }
                // O guardado só sai do Ctrl+S sozinho no envio: ele volta ao composer e a marca some.
                // Rascunho nosso tem que voltar igual; o guardado alheio, desconhecido, só sem o nosso texto.
                let restored = |s: &str| ComposerSnapshot::parse(s).is_some_and(|now| match draft {
                    Some(draft) => compact(&now.content) == compact(&draft.content),
                    None => now.proves(text, before) != Proof::Present,
                });
                if before.stashed && !stash_held(&screen) && restored(&typed) && (draft.is_some() || restored(&screen)) {
                    return DeliveryResult::new(Disposition::Accepted, DeliveryStage::SubmitProof, "submitted");
                }
                if text.trim_start().starts_with('/') {
                    if overlay(&screen) && ComposerSnapshot::parse(&typed).is_none() {
                        return DeliveryResult::new(Disposition::Accepted, DeliveryStage::SubmitProof, "slash_overlay");
                    }
                    // Só o mesmo comando parado permite confirmar a sugestão do menu.
                    if !slash_selected && (screen.contains("to navigate") || screen.contains("Tab to accept"))
                        && ComposerSnapshot::parse(&typed).is_some_and(|now| now.owned(text, before)) {
                        slash_selected = true;
                        if let Err(e) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, e.code); }
                    }
                }
            }
        }
        // Limpar depois do Enter nunca demonstra que a mensagem não foi consumida.
        let mut result = DeliveryResult::new(Disposition::Unknown, DeliveryStage::SubmitProof, "submit_unproved");
        if let Some(code) = last_error { result.code = format!("submit_unproved:{code}"); }
        result
    }
    pub async fn prompt(&self, text: &str, id: &str) -> DeliveryResult {
        if !valid_text(text) || text.trim().is_empty() { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_text"); }
        let _serial = self.serial.lock().await;
        let mut facts = match self.verify().await { Ok(f) => f, Err(e) => return Self::failed(e, DeliveryStage::Identity) };
        if let Some(native) = facts.native.as_ref().filter(|_| recognized_message(text).is_some()) {
            let mid = native.message_id.as_deref().unwrap_or(id);
            let envelope = native_envelope(native, text, mid);
            if let Err(e) = self.verify().await { return Self::failed(e, DeliveryStage::Identity); }
            if self.services.writing().await.is_err() { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Native, "write_journal"); }
            let outcome = self.io.socket(native, envelope).await.unwrap_or(WriteOutcome::Unknown);
            if outcome != WriteOutcome::NotWritten {
                let mut result = DeliveryResult::new(if outcome == WriteOutcome::Written { Disposition::Accepted } else { Disposition::Unknown }, DeliveryStage::Native, "native_write");
                result.native = true; result.message_id = Some(mid.into()); return result;
            }
        }
        for _ in 0..self.limits.ready_attempts.max(1) {
            if facts.open_question { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Ready, "question_open"); }
            if facts.ready { break; }
            self.settle().await;
            facts = match self.verify().await { Ok(f) => f, Err(e) => return Self::failed(e, DeliveryStage::Identity) };
        }
        if !facts.ready { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Ready, "not_ready"); }
        let draft = match self.snapshot().await {
            Err(e) if e.code == "footer_focus" && !self.footer_kept => self.return_footer_focus().await,
            other => other,
        };
        let draft = match draft { Ok(d) => d, Err(e) => return Self::failed(e, DeliveryStage::Composer) };
        if draft.is_empty() { return self.deliver(text, id, draft, None).await; }
        // O guardado tem uma vaga só: ocupado, o Ctrl+S jogaria fora o que já estava nele.
        if draft.stashed { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Composer, "composer_busy"); }
        let mut result = match self.stash_draft(&draft).await {
            Ok(before) => self.deliver(text, id, before, Some(&draft)).await,
            Err(e) => Self::failed(e, DeliveryStage::Composer),
        };
        result.draft = Some(self.settle_draft(&draft, result.disposition).await);
        result
    }
    async fn deliver(&self, text: &str, id: &str, mut before: ComposerSnapshot, draft: Option<&ComposerSnapshot>) -> DeliveryResult {
        let facts = match self.verify().await { Ok(f) => f, Err(e) => return Self::failed(e, DeliveryStage::Identity) };
        let mut refresh_guard = false;
        if facts.plugin_live && !text.trim_start().starts_with('/') {
            // Com algo guardado, o envio vai pelo composer: só ele devolve o guardado.
            let mode = if user_mode(&facts, text, before.stashed) { PluginMode::User } else { PluginMode::Fill };
            let request = PluginRequest { id: id.into(), text: text.into(), mode: mode.clone() };
            if self.services.writing().await.is_err() { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Plugin, "write_journal"); }
            match self.services.publish(&self.binding, request).await {
                Ok(PluginReply::Unavailable | PluginReply::NotWritten) => refresh_guard = true,
                Ok(PluginReply::Accepted) if mode == PluginMode::User => return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Plugin, "plugin_accepted"),
                Ok(PluginReply::Filled) if mode == PluginMode::Fill => {
                    if self.prove_input(text, &before).await { return self.submit(text, &before, draft).await; }
                    return self.partial(text, &before, DeliveryStage::InputProof).await;
                }
                _ => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Plugin, "plugin_uncertain"),
            }
        }
        let use_clipboard = self.binding.windows && (text.contains('\n') || text.contains('\\'));
        let clipboard = if use_clipboard { match self.clipboard_lock().await { Ok(lock) => lock, Err(e) => return Self::failed(e, DeliveryStage::Write) } } else { None };
        if refresh_guard || use_clipboard {
            before = match self.refresh_input_guard().await { Ok(b) => b, Err(e) => return Self::failed(e, DeliveryStage::Composer) };
        }
        if self.services.writing().await.is_err() { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Write, "write_journal"); }
        let write = if use_clipboard { self.clipboard(text).await }
            else if text.contains('\n') { self.paste(text, id).await } else { self.literal(text).await };
        if let Err(e) = write {
            if !e.may_have_written { return Self::failed(e, DeliveryStage::Write); }
            return self.partial(text, &before, DeliveryStage::Write).await;
        }
        let delay = if text.trim_start().starts_with('/') { self.limits.slash_settle }
            else if text.contains('\n') { self.limits.multiline_settle } else { self.limits.literal_settle };
        tokio::time::sleep(delay).await;
        if self.prove_input(text, &before).await { drop(clipboard); return self.submit(text, &before, draft).await; }
        self.partial(text, &before, DeliveryStage::InputProof).await
    }
    pub async fn key(&self, key: &str, interactive: bool) -> DeliveryResult {
        let Some(mapped) = allowed_key(key, interactive) else { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "key_not_allowed"); };
        let _serial = self.serial.lock().await;
        match self.key_inner(mapped).await { Ok(()) => DeliveryResult::new(Disposition::Accepted, DeliveryStage::Control, "key_written"), Err(e) => Self::failed(e, DeliveryStage::Control) }
    }
    pub async fn text(&self, text: &str) -> DeliveryResult {
        if !valid_text(text) { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_text"); }
        let _serial = self.serial.lock().await;
        let result = if text.contains('\n') && !self.binding.windows { self.paste(text, "interactive").await } else { self.literal(text).await };
        match result { Ok(()) => DeliveryResult::new(Disposition::Accepted, DeliveryStage::Control, "text_written"), Err(e) => Self::failed(e, DeliveryStage::Control) }
    }
    pub async fn interrupt(&self, clear: bool) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        // Com o foco no rodapé o primeiro Esc só o devolve ao composer: o segundo interrompe.
        let focused = match self.composer_capture().await {
            Ok((screen, _)) => crate::terminal_state::footer_focus(&screen),
            Err(e) => { tracing::warn!(pane=%self.binding.pane, code=e.code, "tela ilegível antes da interrupção; o foco do rodapé não foi conferido"); false }
        };
        if focused {
            if let Err(e) = self.key_inner("Escape").await { return Self::failed(e, DeliveryStage::Control); }
            self.settle().await;
        }
        if let Err(e) = self.key_inner("Escape").await { return Self::failed(e, DeliveryStage::Control); }
        if clear {
            self.settle().await;
            match self.snapshot().await {
                Ok(s) if !s.is_empty() => if let Err(e) = self.key_inner("Escape").await { return Self::failed(e, DeliveryStage::Control); },
                Ok(_) => (),
                Err(e) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Control, e.code),
            }
        }
        DeliveryResult::new(Disposition::Accepted, DeliveryStage::Control, "interrupted")
    }
    pub async fn steer(&self) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        let screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Steer) };
        if !screen.contains("ctrl+x ctrl+s to send now") { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Steer, "no_queued_input"); }
        for key in ["C-x", "C-s"] { if let Err(e) = self.key_inner(key).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Steer, e.code); } }
        DeliveryResult::new(Disposition::Accepted, DeliveryStage::Steer, "steered")
    }
    async fn navigate(&self, target: usize, mut screen: String, require_cursor: bool) -> Result<String, IoFailure> {
        let mut effects = false;
        let mut row = cursor(&screen).or_else(|| unnumbered_cursor(&screen));
        if row.is_none() && require_cursor { return Err(IoFailure { code: "cursor_unreadable", may_have_written: false }); }
        if row.is_none() {
            for _ in 1..target {
                self.key_inner("Down").await.map_err(|mut e| { e.may_have_written |= effects; e })?;
                effects = true;
                self.settle().await;
            }
            return self.capture_inner().await.map_err(|mut e| { e.may_have_written |= effects; e });
        }
        for _ in 0..4 {
            let current = row.ok_or(IoFailure { code: "cursor_lost", may_have_written: effects })?;
            if current == target { return Ok(screen); }
            for _ in 0..target.abs_diff(current) {
                self.key_inner(if current < target { "Down" } else { "Up" }).await.map_err(|mut e| { e.may_have_written |= effects; e })?;
                effects = true;
                self.settle().await;
            }
            screen = self.capture_inner().await.map_err(|mut e| { e.may_have_written |= effects; e })?;
            row = cursor(&screen).or_else(|| unnumbered_cursor(&screen));
        }
        Err(IoFailure { code: "cursor_drift", may_have_written: effects })
    }
    pub async fn select(&self, option: usize, require_cursor: bool) -> DeliveryResult {
        if option == 0 || option > 100 { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_option"); }
        let _serial = self.serial.lock().await;
        let screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Select) };
        let screen = match self.navigate(option, screen, require_cursor).await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Select) };
        let multiple = screen.lines().any(|line| line.contains("[ ]") || line.contains("[✔]"));
        if let Err(e) = self.key_inner(if multiple { "Space" } else { "Enter" }).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, e.code); }
        self.settle().await;
        let after = match self.capture_inner().await { Ok(s) if !s.trim().is_empty() => s, _ => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "selection_unproved") };
        if multiple {
            if option_mark(&screen, option) == option_mark(&after, option) { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "selection_unchanged"); }
        } else if (live_picker(&after) || unnumbered_cursor(&after).is_some()) && option_labels(&screen) == option_labels(&after) { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "picker_still_open"); }
        DeliveryResult::new(Disposition::Accepted, DeliveryStage::Select, "selected")
    }
    pub async fn submit_selected(&self) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        let screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Select) };
        if !live_picker(&screen) || !screen.lines().any(|l| l.contains("[ ]") || l.contains("[✔]")) { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Select, "multiple_picker_missing"); }
        if let Err(e) = self.key_inner("Right").await { return Self::failed(e, DeliveryStage::Select); }
        self.settle().await;
        if !self.capture_inner().await.is_ok_and(|s| s.contains("Submit answers")) { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "submit_tab_missing"); }
        if let Err(e) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, e.code); }
        self.prove_control_submission(DeliveryStage::Select).await
    }
    async fn prove_control_submission(&self, stage: DeliveryStage) -> DeliveryResult {
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            if let Ok((screen, typed)) = self.composer_capture().await {
                if !screen.trim().is_empty() && (ComposerSnapshot::parse(&typed).is_some_and(|s| s.is_empty())
                    || (!overlay(&screen) && !screen.contains("Submit answers"))) {
                    return DeliveryResult::new(Disposition::Accepted, stage, "control_submitted");
                }
            }
        }
        DeliveryResult::new(Disposition::Unknown, stage, "control_submission_unproved")
    }
    pub async fn answer(&self, answers: &[QuestionAnswer]) -> DeliveryResult {
        if answers.is_empty() || answers.iter().any(|a| !a.valid()) { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_answer"); }
        let _serial = self.serial.lock().await;
        let mut effects = false;
        for answer in answers {
            let screen = match self.capture_inner().await { Ok(s) if live_picker(&s) => s, _ => return DeliveryResult::new(if effects { Disposition::Unknown } else { Disposition::Deferred }, DeliveryStage::Answer, "picker_missing") };
            let mut screen = screen;
            for target in answer.targets() {
                if let Err(e) = self.navigate(target, screen, true).await {
                    return DeliveryResult::new(if effects || e.may_have_written { Disposition::Unknown } else { Disposition::Deferred }, DeliveryStage::Answer, e.code);
                }
                effects = true;
                if let Err(e) = self.key_inner(if answer.kind == AnswerKind::Option && answer.multi { "Space" } else { "Enter" }).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                self.settle().await;
                if answer.kind == AnswerKind::Text {
                    if let Err(e) = self.literal(answer.value.as_deref().unwrap()).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                    self.settle().await;
                    if let Err(e) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                }
                screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code) };
            }
            if answer.multi {
                if let Err(e) = self.key_inner("Right").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                self.settle().await;
            }
        }
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            let Ok(screen) = self.capture_inner().await else { continue; };
            if screen.contains("Submit answers") {
                if !review_matches(&screen, answers) { continue; }
                if let Err(e) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                return self.prove_control_submission(DeliveryStage::Answer).await;
            }
            if !screen.trim().is_empty() && !live_picker(&screen) { return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Answer, "answered"); }
        }
        DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, "answer_unproved")
    }
}
/// O plugin entrega sem tecla nenhuma (`PluginMode::User`): sessão parada, sem rascunho guardado, sem `@`
/// nem `!`. Com o plugin vivo e fora disso, `Fill`, que aperta `Enter`.
fn user_mode(facts: &InputFacts, text: &str, stashed: bool) -> bool {
    facts.plugin_user && facts.idle && !stashed && !text.contains('@') && !text.trim_start().starts_with('!')
}

/// A entrega de `text` com estes fatos vai apertar tecla no pane? Não no modo `User` do plugin nem na
/// entrega nativa (socket), que não passam pelo teclado. Antes do rascunho ser lido: com um rascunho
/// guardado o modo vira `Fill`, e a entrega nativa que não escreve cai na digitação.
pub fn presses_keys(facts: &InputFacts, text: &str) -> bool {
    if facts.native.is_some() && recognized_message(text).is_some() { return false; }
    let slash = text.trim_start().starts_with('/');
    !(facts.plugin_live && !slash && user_mode(facts, text, false))
}

fn recognized_message(text: &str) -> Option<&str> {
    let closing = text.find(']')?;
    let prefix = text.get(1..closing)?;
    if !text.starts_with('[') || !["de:", "grupo:", "painel:"].iter().any(|p| prefix.starts_with(p)) { return None; }
    Some(text[closing+1..].trim_start())
}
fn escape_attribute(text: &str) -> String { text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;") }
fn native_envelope(descriptor: &NativeMessage, text: &str, id: &str) -> Vec<u8> {
    let content = format!("<cross-session-message from=\"{}\" from-name=\"{}\" from-mode=\"{}\">\n{}\n</cross-session-message>", escape_attribute(&descriptor.origin), escape_attribute(&descriptor.sender), escape_attribute(&descriptor.mode), text.trim_matches('\n'));
    let mut bytes = serde_json::to_vec(&serde_json::json!({"msgV":1,"msg_id":id,"type":"user","message":{"role":"user","content":content},"priority":"next","from":descriptor.origin})).unwrap();
    bytes.push(b'\n'); bytes
}
fn allowed_key(key: &str, interactive: bool) -> Option<&str> {
    match key {
        "Up"|"Down"|"Left"|"Right"|"Enter"|"Escape"|"Tab"|"BTab"|"Space" => Some(key),
        "PageUp" => Some("PPage"), "PageDown" => Some("NPage"),
        "Backspace" if interactive => Some("BSpace"), "Delete" if interactive => Some("DC"),
        "Home"|"End"|"C-c"|"C-d"|"C-r"|"C-u"|"C-k"|"C-w"|"C-a"|"C-e"|"C-l"|"C-z"|"C-p"|"C-n"|"C-b"|"C-f"|"C-g" if interactive => Some(key),
        _ => None,
    }
}
fn unnumbered_cursor(screen: &str) -> Option<usize> {
    if !overlay(screen) { return None; }
    let lines: Vec<_> = screen.lines().collect();
    let selected = lines.iter().rposition(|s| s.trim_start().starts_with("❯ ") && !CURSOR.is_match(s))?;
    if lines[selected+1..].iter().any(|s| s.matches('─').count() >= 10) { return None; }
    let line = lines[selected];
    let column = line.chars().take_while(|c| c.is_whitespace()).count() + 2;
    let aligned = |s: &str| {
        let chars: Vec<_> = s.chars().collect();
        chars.len() > column && chars[..column].iter().all(|c| c.is_whitespace()) && !chars[column].is_whitespace()
    };
    let mut top = selected;
    while top > 0 && aligned(lines[top-1]) { top -= 1; }
    let mut bottom = selected+1;
    while bottom < lines.len() && aligned(lines[bottom]) { bottom += 1; }
    if bottom-top < 2 { return None; }
    Some(selected-top+1)
}
fn option_labels(screen: &str) -> Vec<String> {
    static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[❯›]?\s*(\d+)\.\s*(.*)").unwrap());
    screen.lines().filter_map(|l| OPTION.captures(l)).map(|c| format!("{}. {}", &c[1], c[2].split(|c:char| ('─'..='╿').contains(&c)).next().unwrap_or("").trim())).collect()
}
fn option_mark(screen: &str, option: usize) -> Option<String> {
    let pattern = Regex::new(&format!(r"(?m)^\s*[❯›]?\s*{}\.\s*\[(.*?)\]", option)).unwrap();
    Some(pattern.captures(screen)?[1].into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnswerKind { Option, Text, Chat }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionAnswer {
    pub kind: AnswerKind,
    pub question_id: Option<String>,
    #[serde(default)] pub indices: Vec<usize>,
    #[serde(default)] pub labels: Vec<String>,
    #[serde(default)] pub multi: bool,
    pub value: Option<String>,
    pub type_index: Option<usize>,
    pub chat_index: Option<usize>,
}
impl QuestionAnswer {
    fn valid(&self) -> bool {
        match self.kind {
            AnswerKind::Option => !self.indices.is_empty() && self.indices.iter().all(|i| *i < 100) && (self.multi || self.indices.len()==1) && !self.labels.is_empty(),
            AnswerKind::Text => self.type_index.is_some_and(|i| i < 100) && self.value.as_ref().is_some_and(|v| !v.trim().is_empty() && valid_text(v) && !v.contains('\n')),
            AnswerKind::Chat => self.chat_index.is_some_and(|i| i < 100),
        }
    }
    fn targets(&self) -> Vec<usize> {
        let mut indices = match self.kind { AnswerKind::Option => self.indices.clone(), AnswerKind::Text => vec![self.type_index.unwrap()], AnswerKind::Chat => vec![self.chat_index.unwrap()] };
        indices.sort_unstable(); indices.dedup(); indices.into_iter().map(|i| i+1).collect()
    }
}
pub fn review_matches(screen: &str, answers: &[QuestionAnswer]) -> bool {
    let tokens: Vec<BTreeSet<_>> = screen.lines().filter_map(|l| l.trim().strip_prefix('→')).map(|s| s.split(',').map(str::trim).collect()).collect();
    if tokens.len() != answers.len() { return false; }
    answers.iter().zip(tokens).all(|(a, row)| {
        let expected: BTreeSet<_> = if a.kind == AnswerKind::Text { a.value.iter().map(String::as_str).collect() } else { a.labels.iter().map(String::as_str).collect() };
        !expected.is_empty() && expected == row
    })
}
