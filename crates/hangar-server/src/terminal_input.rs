//! Escritor Claude terminal; a fila e a política pertencem ao executor do runtime.
use regex::Regex;
use crate::interactions::{ExpectedInteraction, InteractionError, InteractionKind, InteractionStore};
use serde_json::{Value, json};
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
    /// A tela lida antes da prova da vida destes fatos: a guarda do menu a compara, sem outra captura.
    #[serde(skip)]
    pub screen: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// `Focus` não entrega texto: o plugin devolve o teclado ao prompt, de onde estiver nos mods, sem mexer no
/// rascunho nem na aba que a pessoa vê.
pub enum PluginMode { Fill, User, Focus }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRequest { pub id: String, pub text: String, pub mode: PluginMode }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginReply { Unavailable, NotWritten, Filled, Accepted, Unknown }
pub trait TerminalServices: Send + Sync {
    fn facts<'a>(&'a self, binding: &'a TerminalBinding) -> ServiceFuture<'a, InputFacts>;
    fn publish<'a>(&'a self, binding: &'a TerminalBinding, request: PluginRequest) -> ServiceFuture<'a, PluginReply>;
    /// Recusa painel real ou reserva incompatível; não recusa só porque o turno está trabalhando.
    fn permission_guard<'a>(&'a self, binding: &'a TerminalBinding) -> ServiceFuture<'a, ()>;
    /// Grava no diário que a escrita vai começar: antes disso, cair não deixa a entrega incerta.
    fn writing<'a>(&'a self) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
    /// Os fatos terminam conferindo o mesmo pane do `verify_pane`, depois da tela: o driver não o repete.
    fn facts_check_pane(&self) -> bool { false }
    /// Mesmo pane e mesmo processo do agente, sem provar a conversa: só para tirar o próprio texto do
    /// composer quando a prova da conversa sumiu depois da escrita. Sem implementação, não limpa.
    fn life_check<'a>(&'a self, _binding: &'a TerminalBinding) -> ServiceFuture<'a, ()> { Box::pin(async { Err(ServiceError("life_check_unavailable")) }) }
    /// Tamanho do transcript antes do Enter; `None` sem transcript legível.
    fn transcript_mark<'a>(&'a self) -> ServiceFuture<'a, Option<u64>> { Box::pin(async { Ok(None) }) }
    /// O Claude pôs `text` na fila dele depois de `mark`, ou seja, aceitou o Enter.
    fn transcript_enqueued<'a>(&'a self, _mark: u64, _text: &'a str) -> ServiceFuture<'a, bool> { Box::pin(async { Ok(false) }) }
    /// Os hooks `UserPromptSubmit` começaram para `text` depois de `since` (epoch): o Claude passou da
    /// recusa dele e só um hook ainda pode barrar, o que o transcript mostra como erro.
    fn prompt_started<'a>(&'a self, _since: f64, _text: &'a str) -> ServiceFuture<'a, bool> { Box::pin(async { Ok(false) }) }
    /// Tira a publicação do plugin já provada pelo transcript, sem esperar o aviso dele.
    fn abandon_publication<'a>(&'a self, _publication_id: &'a str) -> ServiceFuture<'a, ()> { Box::pin(async { Ok(()) }) }
}

/// `<config_dir>/.hangar-prompt/<conversa>.json` do `state_hook`: o SHA-1 do texto aparado cujos hooks
/// `UserPromptSubmit` começaram e quando. Arquivo ausente ou torto não prova nada.
pub(crate) fn prompt_started_in(config_dir: &std::path::Path, conversation: &str, since: f64, text: &str) -> bool {
    prompt_mark(config_dir, conversation).is_some_and(|(sha1, ts)| ts >= since && sha1 == prompt_sha1(text))
}

/// O marcador lido: (SHA-1, instante).
pub(crate) fn prompt_mark(config_dir: &std::path::Path, conversation: &str) -> Option<(String, f64)> {
    let bytes = std::fs::read(config_dir.join(".hangar-prompt").join(format!("{conversation}.json"))).ok()?;
    let mark = serde_json::from_slice::<Value>(&bytes).ok()?;
    Some((mark["sha1"].as_str()?.to_owned(), mark["ts"].as_f64()?))
}

pub(crate) fn prompt_sha1(text: &str) -> String { sha1_smol::Sha1::from(text.trim()).digest().to_string() }

/// O Claude grava a entrada ao aceitar o Enter, antes dos hooks `UserPromptSubmit` da conta; o composer
/// só esvazia depois deles. Com turno em curso é o `enqueue` com `content`; parado, o `enqueue` vem sem
/// texto e a prova é a linha `user` com o texto, gravada logo em seguida.
pub(crate) fn enqueued_since(path: &std::path::Path, mark: u64, text: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    if file.seek(SeekFrom::Start(mark)).is_err() { return false; }
    let mut tail = Vec::new();
    if file.take(4 << 20).read_to_end(&mut tail).is_err() { return false; }
    let want = compact(text);
    String::from_utf8_lossy(&tail).lines().any(|line| {
        if !line.contains("\"enqueue\"") && !line.contains("\"user\"") { return false; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
        let written = match v["type"].as_str() {
            Some("queue-operation") if v["operation"] == "enqueue" => v["content"].as_str().map(str::to_owned),
            Some("user") => match &v["message"]["content"] {
                Value::String(content) => Some(content.clone()),
                Value::Array(blocks) => Some(blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()),
                _ => None,
            },
            _ => None,
        };
        written.is_some_and(|content| !content.is_empty() && compact(&content) == want)
    })
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
    /// Aceita com a sessão parada: o turno começou com ela, não ficou na fila interna do Claude.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub consumed: bool,
}
impl DeliveryResult {
    fn new(disposition: Disposition, stage: DeliveryStage, code: &str) -> Self {
        Self { disposition, stage, cleanup: Cleanup::NotNeeded, native: false, message_id: None, code: code.into(), draft: None, consumed: false }
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
/// Observador tmux somente de leitura herdado pelo `send-keys`: a recusa vem antes de qualquer tecla,
/// e o mesmo pane com cliente vazio a contorna. Cliente explícito, outra falha e psmux ficam fora.
fn read_only_retry(request: &CommandRequest, output: &CommandOutput, stderr: &[u8]) -> Option<CommandRequest> {
    if cfg!(windows) || output.success || !request.stdin.is_empty() || String::from_utf8_lossy(stderr).trim() != "client is read-only" { return None; }
    let at = request.args.iter().position(|arg| arg == "send-keys")?;
    let options = &request.args[at + 1..];
    if options.iter().take_while(|arg| *arg != "--").any(|arg| arg == "-c") { return None; }
    let mut args = request.args.clone();
    args.splice(at + 1..at + 1, ["-c".to_string(), String::new()]);
    Some(CommandRequest { program: request.program.clone(), args, stdin: Vec::new() })
}
impl TerminalIo for ProcessIo {
    fn command<'a>(&'a self, request: CommandRequest) -> IoFuture<'a, CommandOutput> {
        Box::pin(async move {
            let (output, stderr) = self.run(request.clone()).await?;
            let Some(retry) = read_only_retry(&request, &output, &stderr) else { return Ok(output) };
            tracing::warn!(code = "mux_readonly_client", "send-keys recusado pelo cliente somente de leitura; repete com cliente vazio");
            Ok(self.run(retry).await?.0)
        })
    }
    fn socket<'a>(&'a self, descriptor: &'a NativeMessage, envelope: Vec<u8>) -> IoFuture<'a, WriteOutcome> { self.socket_inner(descriptor, envelope) }
}
impl ProcessIo {
    async fn run(&self, request: CommandRequest) -> Result<(CommandOutput, Vec<u8>), IoFailure> {
        {
            use std::process::Stdio;
            let mut command=child_command(request.program);
            command.args(request.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
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
            let mut stderr = child.stderr.take().unwrap();
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
            // Só na falha, e só o que já está no cano: um neto que herdou o stderr não segura o comando.
            let mut errors = Vec::new();
            if !status.success() {
                let mut buf = vec![0u8; 4096];
                if let Ok(Ok(n)) = timeout(Duration::from_millis(100), stderr.read(&mut buf)).await { buf.truncate(n); errors = buf; }
            }
            match result {Ok(Ok(stdout))=>Ok((CommandOutput {success:status.success(),stdout},errors)),_=>Err(IoFailure {code:"command_uncertain",may_have_written:true})}
        }
    }
    fn socket_inner<'a>(&'a self, descriptor: &'a NativeMessage, envelope: Vec<u8>) -> IoFuture<'a, WriteOutcome> {
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
// Dica de argumento que o Claude desenha depois de um comando com barra completo (`/clear │ [name]`,
// `/compact <optional custom summarization instructions>`). Em alguns temas ela não vem esmaecida.
// Sem `#`: anexo (`[Image #1]`) nunca passa por dica.
static ARG_HINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\[[^\[\]#]*\]|<[^<>#]*>)$").unwrap());
static NEXT_BUFFER: AtomicU64 = AtomicU64::new(0);
fn compact(text: &str) -> String { text.chars().filter(|c| !c.is_whitespace() && !"│┃║".contains(*c)).collect() }
/// O Claude marca `› stashed` na linha de dicas acima do composer enquanto guarda um rascunho.
fn stash_held(screen: &str) -> bool {
    let lines: Vec<_> = screen.split('\n').collect();
    let rules: Vec<_> = lines.iter().enumerate().filter(|(_, s)| s.matches('─').count() >= 20).map(|(n, _)| n).collect();
    rules.len().checked_sub(2).and_then(|n| rules[n].checked_sub(1)).is_some_and(|n| lines[n].contains("› stashed"))
}
impl ComposerSnapshot {
    /// As linhas da tela e as duas últimas réguas (topo e base do composer), sem conferir tamanho.
    fn frame(screen: &str) -> Option<(Vec<&str>, usize, usize)> {
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
        Some((lines, top, bottom))
    }
    /// Composer no pé da tela e mais alto do que a leitura aceita: mensagem longa colada, não outra tela.
    pub fn too_tall(screen: &str) -> bool {
        Self::frame(screen).is_some_and(|(lines, top, bottom)| lines.len() - bottom <= 8 && bottom - top > 40)
    }
    pub fn parse(screen: &str) -> Option<Self> {
        let (lines, top, bottom) = Self::frame(screen)?;
        // Mensagem longa colada cresce o composer; acima disto as réguas já não são as dele.
        if lines.len() - bottom > 8 || bottom - top > 40 { return None; }
        let content = lines[top+1..bottom].iter().map(|s| s.trim().strip_prefix('❯').unwrap_or(s.trim()).trim()).collect::<Vec<_>>().join("\n");
        let placeholders = PLACEHOLDER.captures_iter(&content).map(|c| format!("{}:{}", &c[1], &c[2])).collect();
        Some(Self { content, placeholders, stashed: stash_held(screen) })
    }
    pub fn is_empty(&self) -> bool { self.content.trim().is_empty() }
    /// O composer compactado, sem a dica de argumento que segue o comando com barra digitado. Só
    /// com o composer vazio antes: um rascunho `/clear [nota]` do dono não vira dica.
    fn typed(&self, text: &str, before: &Self) -> String {
        let (visible, expected) = (compact(&self.content), compact(text));
        let hinted = before.is_empty() && expected.starts_with('/')
            && visible.strip_prefix(expected.as_str()).is_some_and(|rest| ARG_HINT.is_match(rest));
        if hinted { expected } else { visible }
    }
    pub fn proves(&self, text: &str, before: &Self) -> Proof {
        if self.placeholders.difference(&before.placeholders).next().is_some() { return Proof::Present; }
        let visible = self.typed(text, before);
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
        if self.typed(text, before) == compact(text) { return true; }
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
/// Picker vivo: o cursor e, abaixo dele, o rodapé de navegação, sem composer depois. Em qualquer linha,
/// como o Python: a lista de tarefas abaixo do picker empurra o rodapé para fora das últimas linhas.
fn live_picker(screen: &str) -> bool {
    if cursor(screen).is_none() { return false; }
    if crate::terminal_state::analyze(screen).overlay { return true; }
    let lines: Vec<_> = screen.lines().collect();
    let Some(last) = lines.iter().rposition(|l| CURSOR.is_match(l)) else { return false };
    lines[last..].iter().rposition(|l| l.contains("Esc to cancel") || l.contains("to navigate"))
        .is_some_and(|footer| !lines[last + footer..].iter().any(|l| crate::terminal_state::is_rule(l)))
}
/// `_CAIXA_OPCAO`: múltipla escolha só com a caixinha numa linha de opção, nunca um `- [ ]` da conversa.
fn multiple_choice(screen: &str) -> bool {
    static BOX_OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*[❯›]?\s*\d+\.\s*\[.?\]\s").unwrap());
    BOX_OPTION.is_match(screen)
}
fn overlay(screen: &str) -> bool {
    let analysis = crate::terminal_state::analyze(screen);
    analysis.overlay || analysis.state == "awaiting_input"
}

const SUBAGENT_COMPOSER: &str = "composer_targets_subagent";
struct AgentRow { name: String, cursor: bool, viewed: bool }
/// O painel de agentes abaixo do composer: `●` é o agente que o composer vê, `❯` o cursor do painel.
fn agent_rows(screen: &str) -> Vec<AgentRow> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(❯\s+)?([●◯])\s+(\S+)").unwrap());
    let screen = unstyle(screen, true);
    let lines: Vec<_> = screen.lines().collect();
    let below = lines.iter().rposition(|l| crate::terminal_state::is_rule(l)).map_or(&lines[..0], |rule| &lines[rule + 1..]);
    below.iter().filter_map(|l| ROW.captures(l)).map(|c| AgentRow { name: c[3].into(), cursor: c.get(1).is_some(), viewed: &c[2] == "●" }).collect()
}
/// O texto digitado iria a um subagente: o painel marca outro agente que não o `main`, ou o composer
/// vazio mostra a dica esmaecida `Message @<agente>…`.
fn subagent_composer(screen: &str, typed: &str) -> bool {
    let rows = agent_rows(screen);
    let panel = rows.iter().any(|row| row.name == "main" && !row.viewed) && rows.iter().any(|row| row.viewed && row.name != "main");
    let hint = |s: &str| ComposerSnapshot::parse(s).is_some_and(|c| c.content.starts_with("Message @"));
    panel || hint(screen) && !hint(typed)
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

/// Retrato comparável do menu: mover cursor e marcar caixas não troca a pergunta.
pub fn interaction_pane(screen: &str) -> Result<Value, InteractionError> {
    let analysis = crate::terminal_state::analyze(screen);
    let options = analysis.options.filter(|options| !options.is_empty()).ok_or(InteractionError::Unavailable)?;
    if analysis.state != "awaiting_input" { return Err(InteractionError::Changed); }
    Ok(json!({"question": analysis.question, "options": options.iter().map(|label| {
        let label = label.trim();
        if let Some(rest) = label.strip_prefix('[').and_then(|s| s.split_once(']')).map(|(_, rest)| rest) { rest.trim() } else { label }
    }).collect::<Vec<_>>()}))
}

/// O que a guarda de uma tecla compara: o menu exibido ou, na aba de envio do formulário (que não é
/// menu), as respostas listadas com o cursor em "Submit answers".
fn guard_pane(screen: &str) -> Result<Value, InteractionError> {
    if crate::state::ask::submit_tab(screen) {
        return Ok(json!({"review": screen.lines().filter_map(|line| line.trim().strip_prefix('→')).map(str::trim).collect::<Vec<_>>()}));
    }
    interaction_pane(screen)
}

pub fn interaction_semantic(screen: &str, question: Option<&hangar_api::ask::AskQuestion>) -> Result<Value, InteractionError> {
    Ok(json!({"source":"terminal", "pane":interaction_pane(screen)?, "questions":question.map(|q| &q.questions)}))
}

#[derive(Clone, Copy)]
struct GuardedText<'a> {
    expected: &'a ExpectedInteraction,
    closed: bool,
    pane: Option<&'a Value>,
    input: Option<(&'a str, &'a ComposerSnapshot)>,
    allow_owned: bool,
}
type TextGuard<'a> = Option<GuardedText<'a>>;

pub struct TerminalDriver {
    interactions: Option<Arc<InteractionStore>>,
    interaction_source: Option<PathBuf>,
    binding: TerminalBinding,
    services: Arc<dyn TerminalServices>,
    io: Arc<dyn TerminalIo>,
    limits: InputLimits,
    /// Compartilhável: drivers de vida curta do mesmo pane (Codex com terminal) se enfileiram aqui.
    serial: Arc<Mutex<()>>,
    /// A identidade do pane já foi conferida por quem criou o driver: as operações de mod não a refazem.
    pane_checked: bool,
    /// Sem o Esc que devolve o foco do rodapé ao composer: a linha já tentou o bastante.
    footer_kept: bool,
    /// Controles do dono sem guarda de interação: conferem só o pane (`verify_light`).
    controls: bool,
}
/// Sequência inteira na mesma trava da entrada, sem reentrar pelos métodos públicos do driver.
pub(crate) struct PermissionSequence<'a> {
    driver: &'a TerminalDriver,
    _serial: tokio::sync::MutexGuard<'a, ()>,
}
impl PermissionSequence<'_> {
    /// Guarda e prova de vida, sem a política de menu: o seletor do Codex é ele mesmo um menu.
    pub(crate) async fn screen(&self) -> Result<String, IoFailure> {
        let driver = self.driver;
        driver.services.permission_guard(&driver.binding).await
            .map_err(|e| IoFailure { code: e.0, may_have_written: false })?;
        // Uma reserva de mod não pode dispensar a prova de vida nesta sequência.
        if driver.pane_checked { return Err(IoFailure { code: "permission_unverified_driver", may_have_written: false }); }
        driver.capture_inner().await
    }
    pub(crate) async fn copy_mode(&self) -> Result<bool, IoFailure> { Ok(self.driver.mods_formats().await?.in_mode) }
    /// A tela sem o texto esmaecido (a sugestão do composer vazio), para conferir o rascunho.
    pub(crate) async fn typed_screen(&self) -> Result<String, IoFailure> { Ok(self.driver.composer_capture().await?.1) }
    pub(crate) async fn press(&self, key: &str) -> Result<(), IoFailure> {
        let driver = self.driver;
        driver.services.permission_guard(&driver.binding).await
            .map_err(|e| IoFailure { code: e.0, may_have_written: false })?;
        driver.key_inner(key).await
    }
    pub(crate) async fn type_text(&self, text: &str) -> Result<(), IoFailure> {
        let driver = self.driver;
        driver.services.permission_guard(&driver.binding).await
            .map_err(|e| IoFailure { code: e.0, may_have_written: false })?;
        driver.literal(text).await
    }
    /// Sem guarda: fechar o que esta sequência abriu não pode depender do painel continuar fechado.
    pub(crate) async fn escape(&self) -> Result<(), IoFailure> { self.driver.key_inner("Escape").await }
    pub(crate) async fn capture(&self) -> Result<String, IoFailure> {
        let screen = self.screen().await?;
        let analysis = crate::terminal_state::analyze(&screen);
        if analysis.overlay || analysis.options.is_some() {
            return Err(IoFailure { code: "permission_menu_open", may_have_written: false });
        }
        if self.copy_mode().await? {
            return Err(IoFailure { code: "permission_copy_mode", may_have_written: false });
        }
        Ok(screen)
    }
    pub(crate) async fn next(&self, expected: &str, deadline: tokio::time::Instant) -> Result<(), IoFailure> {
        let screen = tokio::time::timeout_at(deadline, self.capture()).await
            .map_err(|_| IoFailure { code: "permission_probe_timeout", may_have_written: false })??;
        if crate::state::permission::parse_permission_mode(&screen) != Some(expected) {
            return Err(IoFailure { code: "permission_mode_changed", may_have_written: false });
        }
        let driver = self.driver;
        tokio::time::timeout_at(deadline, driver.services.permission_guard(&driver.binding)).await
            .map_err(|_| IoFailure { code: "permission_probe_timeout", may_have_written: false })?
            .map_err(|e| IoFailure { code: e.0, may_have_written: false })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(IoFailure { code: "permission_probe_timeout", may_have_written: false });
        }
        tokio::time::timeout_at(deadline, driver.pane_effect(vec!["send-keys".into(), "-t".into(), driver.binding.pane.clone(), "BTab".into()])).await
            .map_err(|_| IoFailure { code: "permission_key_timeout", may_have_written: true })?
    }
}
impl TerminalDriver {
    pub(crate) async fn permission_sequence(&self) -> PermissionSequence<'_> {
        PermissionSequence { driver: self, _serial: self.serial.lock().await }
    }
    pub fn new(binding: TerminalBinding, services: Arc<dyn TerminalServices>, io: Arc<dyn TerminalIo>, limits: InputLimits) -> Self { Self { interactions: None, interaction_source: None, binding, services, io, limits, serial: Arc::new(Mutex::new(())), pane_checked: false, footer_kept: false, controls: false } }
    pub fn controls(mut self) -> Self { self.controls = true; self }
    pub fn with_serial(mut self, serial: Arc<Mutex<()>>) -> Self { self.serial = serial; self }
    pub fn with_interactions(mut self, store: Arc<InteractionStore>) -> Self { self.interactions = Some(store); self }
    pub fn with_interaction_source(mut self, transcript: PathBuf) -> Self { self.interaction_source = Some(transcript); self }
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
        // Falha da pergunta conserva a causa; o resto continua sendo "terminal indisponível".
        let facts = self.services.facts(&self.binding).await.map_err(|error| IoFailure {
            code: if error.0.starts_with("askq_") || InteractionError::from_code(error.0).is_some() { error.0 } else { "facts_unavailable" },
            may_have_written: false,
        })?;
        if facts.binding != self.binding || self.binding.name.is_empty() || self.binding.conversation.is_empty()
            || self.binding.mux_argv.is_empty() || (self.binding.windows && !self.binding.pane.starts_with(&format!("={}:", self.binding.name))) {
            return Err(IoFailure { code: "stale_binding", may_have_written: false });
        }
        Ok(facts)
    }
    async fn verify(&self) -> Result<InputFacts, IoFailure> {
        let facts = self.facts().await?;
        if !self.services.facts_check_pane() { self.verify_pane().await?; }
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
        self.effect_guarded(args, stdin, None).await
    }
    /// `seen`: a tela que a conferência final acabou de ler, antes de provar a vida.
    async fn check_text_guard(&self, guard: TextGuard<'_>, seen: Option<&str>) -> Result<(), IoFailure> {
        let Some(guard) = guard else { return Ok(()); };
        let failure = |error: InteractionError| IoFailure { code: error.code(), may_have_written: false };
        self.check_source(guard.expected, guard.closed).map_err(failure)?;
        if guard.closed {
            let facts = self.verify().await?;
            let (screen, typed) = self.composer_capture().await?;
            let composer = Self::composer(&screen, &typed)?;
            let owned = guard.allow_owned && guard.input.is_some_and(|(text,before)| self.ours(&composer,text,before));
            if !facts.ready || facts.open_question || crate::terminal_state::analyze(&screen).state == "awaiting_input"
                || crate::terminal_state::footer_visible(&screen) || !composer.is_empty() && !owned {
                return Err(failure(InteractionError::Changed));
            }
        } else if let Some(pane) = guard.pane {
            let screen = match seen { Some(screen) => screen.to_owned(), None => self.capture_inner().await? };
            if guard_pane(&screen).map_err(failure)? != *pane { return Err(failure(InteractionError::Changed)); }
        }
        self.check_source(guard.expected, guard.closed).map_err(failure)
    }
    async fn effect_guarded(&self, args: Vec<String>, stdin: Vec<u8>, guard: TextGuard<'_>) -> Result<(), IoFailure> {
        if self.controls && guard.is_none() { self.verify_pane().await?; } else {
            let facts = self.verify().await?;
            self.check_text_guard(guard, facts.screen.as_deref()).await?;
        }
        let output = self.raw(args, stdin).await?;
        if !output.success { return Err(IoFailure { code: "mux_effect_failed", may_have_written: true }); }
        Ok(())
    }
    /// Os controles do dono (Parar, teclas, texto do espelho) conferem só o pane: com o Claude caído e o
    /// pane de volta ao shell, o espelho ainda digita `claude -c`.
    async fn verify_light(&self) -> Result<(), IoFailure> {
        if self.controls { self.verify_pane().await } else { self.verify().await.map(|_| ()) }
    }
    async fn capture_inner(&self) -> Result<String, IoFailure> {
        self.verify_light().await?;
        let output = self.raw(vec!["capture-pane".into(), "-p".into(), "-t".into(), self.binding.pane.clone(), "-S".into(), "-200".into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "capture_failed", may_have_written: false }); }
        String::from_utf8(output.stdout).map_err(|_| IoFailure { code: "capture_utf8", may_have_written: false })
    }
    /// Uma captura para ler o composer: a tela inteira e só o que foi digitado.
    async fn composer_capture(&self) -> Result<(String, String), IoFailure> {
        self.verify_light().await?;
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
    /// Uma captura sem chamar os serviços que ela própria alimenta. Sem conferir o pane: quem chama
    /// prova a mesma vida depois dela.
    pub(crate) async fn facts_screen(&self) -> Result<String, IoFailure> {
        let (screen, _) = self.composer_capture_unverified().await?;
        if self.binding.windows && screen.contains('\u{fffd}') { return Err(IoFailure { code:"capture_utf8",may_have_written:false }); }
        Ok(screen)
    }
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
    pub async fn composer_free(&self) -> bool { self.composer_empty().await == Some(true) }
    /// O campo vazio ou não, conferindo só o pane; `None` quando não se lê.
    pub async fn composer_empty(&self) -> Option<bool> {
        let _serial = self.serial.lock().await;
        self.verify_pane().await.ok()?;
        let (screen, typed) = self.composer_capture_unverified().await.ok()?;
        Self::composer(&screen, &typed).ok().map(|draft| draft.is_empty())
    }
    async fn settle(&self) { tokio::time::sleep(self.limits.settle).await; }
    async fn key_inner(&self, key: &str) -> Result<(), IoFailure> { self.key_guarded(key, None).await }
    /// Tecla num menu respondido: a tela da conferência final, logo antes dela, ainda mostra `pane`.
    async fn menu_key(&self, key: &str, expected: Option<&ExpectedInteraction>, pane: Option<&Value>) -> Result<(), IoFailure> {
        if expected.is_some() && pane.is_none() { return Err(IoFailure { code: InteractionError::Unavailable.code(), may_have_written: false }); }
        self.key_guarded(key, expected.map(|expected| GuardedText { expected, closed: false, pane, input: None, allow_owned: false })).await
    }
    async fn key_guarded(&self, key: &str, guard: TextGuard<'_>) -> Result<(), IoFailure> {
        let mut args = vec!["send-keys".into(), "-t".into(), self.binding.pane.clone()];
        if key == "Enter" && !self.binding.windows { args.extend(["-l".into(), "--".into(), "\r".into()]); }
        else if key == "Escape" && self.binding.windows { args.extend(["-l".into(), "--".into(), "\u{1b}[27;1;27;1;0;1_\u{1b}[27;1;27;0;0;1_".into()]); }
        else { args.push(key.into()); }
        self.effect_guarded(args, vec![], guard).await
    }
    async fn literal(&self, text: &str) -> Result<(), IoFailure> { self.literal_guarded(text, None).await }
    async fn literal_guarded(&self, text: &str, mut guard: TextGuard<'_>) -> Result<(), IoFailure> {
        if text.is_empty() { return Ok(()); }
        if !self.binding.windows {
            // O tmux lê o ponto e vírgula final como separador mesmo depois de --.
            let text = text.strip_suffix(';').map_or_else(|| text.to_string(), |s| format!("{s}\\;"));
            return self.effect_guarded(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(), text], vec![], guard).await;
        }
        let placeholder = text.starts_with('-');
        let text = if placeholder { format!("x{text}") } else { text.into() };
        let chars: Vec<_> = text.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            let mut end = (start + 512).min(chars.len());
            while end < chars.len() && chars[end] == '-' && end-start < 700 { end += 1; }
            let part: String = chars[start..end].iter().collect();
            self.effect_guarded(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), "-l".into(), "--".into(), part], vec![], guard).await
                .map_err(|mut e| { e.may_have_written |= start > 0; e })?;
            start = end;
            if let Some(guard) = guard.as_mut() { guard.allow_owned = true; }
            if start < chars.len() { tokio::time::sleep(self.limits.slash_settle).await; }
        }
        if placeholder {
            self.key_guarded("Home", guard).await.map_err(|mut e| { e.may_have_written = true; e })?;
            self.key_guarded("DC", guard).await.map_err(|mut e| { e.may_have_written = true; e })?;
            // O `x` que sobrou mudaria o texto: falha com o código, sem o Enter.
            if self.capture_inner().await.map_err(|mut e| { e.may_have_written = true; e })?.contains(&text) {
                return Err(IoFailure { code: "placeholder_unremoved", may_have_written: true });
            }
        }
        Ok(())
    }
    async fn paste(&self, text: &str, id: &str) -> Result<(), IoFailure> { self.paste_guarded(text, id, None).await }
    async fn paste_guarded(&self, text: &str, id: &str, guard: TextGuard<'_>) -> Result<(), IoFailure> {
        let buffer = format!("hangar-{}-{}-{}", std::process::id(), NEXT_BUFFER.fetch_add(1, Ordering::Relaxed),
            id.chars().filter(|c| c.is_ascii_alphanumeric()).take(64).collect::<String>());
        self.verify_light().await?;
        let load = self.raw(vec!["load-buffer".into(), "-b".into(), buffer.clone(), "-".into()], text.as_bytes().to_vec()).await?;
        if !load.success { return Err(IoFailure { code: "buffer_failed", may_have_written: false }); }
        self.effect_guarded(vec!["paste-buffer".into(), "-t".into(), self.binding.pane.clone(), "-b".into(), buffer, "-p".into(), "-d".into()], vec![], guard).await
    }
    async fn clipboard_guarded(&self, text: &str, guard: TextGuard<'_>) -> Result<(), IoFailure> {
        if !self.verify().await?.clipboard_available { return Err(IoFailure { code: "clipboard_unavailable", may_have_written: false }); }
        let output = self.io.command(CommandRequest { program: "powershell.exe".into(), args: vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $text=[Console]::In.ReadToEnd(); Set-Clipboard -Value $text; if ((Get-Clipboard -Raw) -cne $text) { exit 1 }".into()], stdin: text.as_bytes().to_vec() }).await?;
        if !output.success { return Err(IoFailure { code: "clipboard_failed", may_have_written: false }); }
        self.key_guarded("M-v", guard).await
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
        if subagent_composer(screen, typed) { return Err(IoFailure { code: SUBAGENT_COMPOSER, may_have_written: false }); }
        let mut snapshot = ComposerSnapshot::parse(typed).ok_or(IoFailure { code: "composer_unreadable", may_have_written: false })?;
        snapshot.stashed = stash_held(screen);
        Ok(snapshot)
    }
    /// Composer vendo um subagente: o painel de agentes do próprio Claude Code volta ao `main` (↓ até o
    /// cursor nele, Enter, Esc para o foco voltar ao composer). Só com o composer vazio: com rascunho o ↓
    /// anda dentro dele. Sem prova de volta, adia com o código, sem digitar.
    async fn return_to_main(&self) -> Result<ComposerSnapshot, IoFailure> {
        let refused = IoFailure { code: SUBAGENT_COMPOSER, may_have_written: false };
        let (_, typed) = self.composer_capture().await?;
        if !ComposerSnapshot::parse(&typed).is_some_and(|draft| draft.is_empty()) { return Err(refused); }
        let mut screen = String::new();
        for _ in 0..4 {
            self.key_inner("Down").await.map_err(|e| IoFailure { may_have_written: false, ..e })?;
            self.settle().await;
            screen = self.capture_inner().await?;
            if agent_rows(&screen).iter().any(|row| row.cursor && row.name == "main") { break; }
        }
        // O Enter só vai com o cursor no `main`: noutra linha ele abriria outro agente.
        if !agent_rows(&screen).iter().any(|row| row.cursor && row.name == "main") { return Err(refused); }
        self.key_inner("Enter").await.map_err(|e| IoFailure { may_have_written: false, ..e })?;
        self.settle().await;
        let viewing = self.capture_inner().await?;
        if !agent_rows(&viewing).iter().any(|row| row.viewed && row.name == "main") { return Err(refused); }
        // O Esc só devolve o foco ao composer se ele ainda está no painel: no composer, interromperia o turno.
        if crate::terminal_state::footer_focus(&unstyle(&viewing, true)) {
            self.key_inner("Escape").await.map_err(|e| IoFailure { may_have_written: false, ..e })?;
        }
        for _ in 0..4 {
            self.settle().await;
            match self.snapshot().await { Err(e) if e.code == "footer_focus" => continue, Err(e) if e.code == SUBAGENT_COMPOSER => return Err(refused), other => return other }
        }
        Err(refused)
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
        // A primeira leitura vem logo depois do `literal_settle` que a escrita já esperou; a espera fica
        // entre as tentativas. A prova é a mesma: só o texto presente passa.
        let mut last_unread = None;
        for attempt in 0..self.limits.proof_attempts.max(1) {
            if attempt > 0 { self.settle().await; }
            match self.snapshot().await {
                Ok(now) if now.proves(text, before) == Proof::Present => return true,
                Ok(_) => last_unread = None,
                // Tela ainda redesenhando (o TUI recém-retomado): a leitura volta dentro da mesma janela.
                Err(error) if matches!(error.code, "composer_unreadable" | "overlay" | "footer_focus") => last_unread = Some(error.code),
                Err(error) => {
                    tracing::warn!(code = "input_proof_read_failed", reason = error.code, "prova da entrada encerrada por leitura que falhou");
                    return false;
                }
            }
        }
        if let Some(reason) = last_unread {
            tracing::warn!(code = "input_proof_read_failed", reason, "prova da entrada sem leitura do composer até o fim do prazo");
        }
        false
    }
    /// Depois do `Filled`: o plugin já confirmou o que pôs no campo. A tela só desmente quando dá para
    /// lê-la; composer ilegível (mensagem longa demais para o desenho) não segura o Enter.
    async fn prove_filled(&self, text: &str, before: &ComposerSnapshot) -> bool {
        // O plugin já respondeu `Filled`: lê a tela na hora e só espera entre as tentativas.
        for attempt in 0..self.limits.proof_attempts.max(1) {
            if attempt > 0 { self.settle().await; }
            let Ok((screen, typed)) = self.composer_capture().await else { return false };
            match Self::composer(&screen, &typed) {
                Ok(now) if now.proves(text, before) == Proof::Present => return true,
                Ok(_) => {}
                // Só o composer comprovadamente alto: tela sem as réguas dele pode ser um diálogo, e o Enter o confirmaria.
                Err(e) if e.code == "composer_unreadable" && ComposerSnapshot::too_tall(&typed) => return true,
                Err(_) => return false,
            }
        }
        false
    }
    /// O composer conferindo só o pane e o agente (`life_check`), sem os fatos da conversa.
    async fn snapshot_agent(&self) -> Result<ComposerSnapshot, IoFailure> {
        self.services.life_check(&self.binding).await.map_err(|error| IoFailure { code: error.0, may_have_written: false })?;
        if !self.services.facts_check_pane() { self.verify_pane().await?; }
        let (screen, typed) = self.composer_capture_unverified().await?;
        Self::composer(&screen, &typed)
    }
    async fn key_agent(&self, key: &str) -> Result<(), IoFailure> {
        self.services.life_check(&self.binding).await.map_err(|error| IoFailure { code: error.0, may_have_written: false })?;
        if !self.services.facts_check_pane() { self.verify_pane().await?; }
        let output = self.raw(vec!["send-keys".into(), "-t".into(), self.binding.pane.clone(), key.into()], vec![]).await?;
        if !output.success { return Err(IoFailure { code: "mux_effect_failed", may_have_written: true }); }
        Ok(())
    }
    async fn cleanup_owned(&self, text: &str, before: &ComposerSnapshot) -> bool {
        // A prova da conversa sumiu depois da escrita: o pane e o agente conferidos bastam para
        // reconhecer o próprio texto e tirá-lo, sem apagar o que não é nosso.
        let (mut now, agent) = match self.snapshot().await {
            Ok(now) => (now, false),
            Err(error) if error.code == "facts_unavailable" => match self.snapshot_agent().await { Ok(now) => (now, true), Err(_) => return false },
            Err(_) => return false,
        };
        // A limpeza só começa vendo o nosso texto inteiro: um começo dele pode ter sido digitado pela pessoa.
        if !now.owned(text, before) { return false; }
        // O Ctrl+U apaga só a linha do cursor (numa linha vazia, só a quebra): o texto de várias linhas sai aos
        // pedaços, e cada pedaço que sobra tem de ser o começo do nosso. Qualquer outra coisa é da pessoa. O
        // orçamento é o configurado: texto longo demais fica como entrega incerta, sem prender a entrada.
        for _ in 0..self.limits.cleanup_attempts {
            if if agent { self.key_agent("C-u").await } else { self.key_inner("C-u").await }.is_err() { return false; }
            self.settle().await;
            let Ok(after) = (if agent { self.snapshot_agent().await } else { self.snapshot().await }) else { return false; };
            if after.is_empty() { return true; }
            if !self.ours(&after, text, before) || after.content.chars().count() >= now.content.chars().count() { return false; }
            now = after;
        }
        false
    }
    /// O campo tem o nosso texto inteiro ou o começo dele (a parte que a limpeza ainda não tirou). Com texto
    /// alheio antes do nosso, um resto que pareça nosso começo pode ser o dele: só o texto inteiro vale.
    fn ours(&self, now: &ComposerSnapshot, text: &str, before: &ComposerSnapshot) -> bool {
        now.owned(text, before) || before.content.is_empty() && !now.content.is_empty() && (compact(text).starts_with(&compact(&now.content))
            || self.binding.windows && text.starts_with('-') && format!("x{}", compact(text)).starts_with(&compact(&now.content)))
    }
    fn failed(error: IoFailure, stage: DeliveryStage) -> DeliveryResult {
        if let Some(interaction) = InteractionError::from_code(error.code) { return Self::interaction_failure(interaction, error.may_have_written); }
        DeliveryResult::new(if error.may_have_written { Disposition::Unknown } else { Disposition::Deferred }, stage, error.code)
    }
    async fn partial(&self, text: &str, before: &ComposerSnapshot, stage: DeliveryStage) -> DeliveryResult {
        let cleaned = self.cleanup_owned(text, before).await;
        let mut result = DeliveryResult::new(if cleaned { Disposition::Deferred } else { Disposition::Unknown }, stage, "input_unproved");
        result.cleanup = if cleaned { Cleanup::Proved } else { Cleanup::Unproved };
        result
    }
    async fn submit(&self, text: &str, before: &ComposerSnapshot, draft: Option<&ComposerSnapshot>, expected: Option<&ExpectedInteraction>) -> DeliveryResult {
        match self.verify().await {
            Ok(facts) if !facts.open_question => {}
            // Sem os fatos da conversa o Enter não sai; o texto é nosso e sai do composer, como na prova.
            Err(error) if error.code == "facts_unavailable" && expected.is_none() => return self.partial(text, before, DeliveryStage::Submit).await,
            _ => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, "submission_blocked"),
        }
        let mark = if expected.is_none() && !text.trim_start().starts_with('/') { self.services.transcript_mark().await.ok().flatten() } else { None };
        let since = crate::state::monitor::wall_now();
        if let Err(error) = self.key_guarded("Enter", expected.map(|expected| GuardedText { expected,closed:true,pane:None,input:Some((text,before)),allow_owned:true })).await {
            // A conferência antes da tecla falhou: o Enter não saiu, e o texto nosso sai do campo como no ramo acima.
            if expected.is_none() && !error.may_have_written { return self.partial(text, before, DeliveryStage::Submit).await; }
            return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, if expected.is_some() { "erro_sem_confirmacao_resposta" } else { error.code });
        }
        let mut slash_selected = false;
        let clear = text.split_whitespace().next() == Some("/clear");
        let mut last_error = None;
        // Composer ainda com o texto logo após o Enter só reprova esta leitura: a seguinte espera o `settle`.
        // Barra espera antes: o menu de sugestões ainda na tela pediria um segundo Enter antes do primeiro valer.
        let slash = text.trim_start().starts_with('/');
        for attempt in 0..self.limits.proof_attempts.max(1) {
            if attempt > 0 || slash { self.settle().await; }
            // Antes do segundo Enter da barra: com os hooks rodando o composer ainda mostra o comando.
            if expected.is_none() && self.services.prompt_started(since, text).await.unwrap_or(false) {
                return DeliveryResult::new(Disposition::Accepted, DeliveryStage::SubmitProof, "prompt_started");
            }
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
                    // Só o mesmo comando parado no composer ganha outro Enter (o autocomplete engoliu o
                    // primeiro), uma vez: o que rodou pode ter trocado a conversa.
                    if !slash_selected && ComposerSnapshot::parse(&typed).is_some_and(|now| now.owned(text, before)) {
                        slash_selected = true;
                        if let Err(e) = self.key_inner("Enter").await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Submit, e.code); }
                    }
                }
            }
            if let Some(mark) = mark && self.services.transcript_enqueued(mark, text).await.unwrap_or(false) {
                return DeliveryResult::new(Disposition::Accepted, DeliveryStage::SubmitProof, "submitted_enqueued");
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
        let draft = match draft {
            Err(e) if e.code == SUBAGENT_COMPOSER => self.return_to_main().await,
            other => other,
        };
        let draft = match draft { Ok(d) => d, Err(e) => return Self::failed(e, DeliveryStage::Composer) };
        let idle = facts.idle;
        if draft.is_empty() {
            let mut result = self.deliver(text, id, draft, None, None).await;
            result.consumed = idle && result.disposition == Disposition::Accepted;
            return result;
        }
        // O guardado tem uma vaga só: ocupado, o Ctrl+S jogaria fora o que já estava nele.
        if draft.stashed { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Composer, "composer_busy"); }
        let mut result = match self.stash_draft(&draft).await {
            Ok(before) => self.deliver(text, id, before, Some(&draft), None).await,
            Err(e) => Self::failed(e, DeliveryStage::Composer),
        };
        result.consumed = idle && result.disposition == Disposition::Accepted;
        result.draft = Some(self.settle_draft(&draft, result.disposition).await);
        result
    }
    async fn deliver(&self, text: &str, id: &str, mut before: ComposerSnapshot, draft: Option<&ComposerSnapshot>, expected: Option<&ExpectedInteraction>) -> DeliveryResult {
        let facts = match self.verify().await { Ok(f) => f, Err(e) => return Self::failed(e, DeliveryStage::Identity) };
        if facts.open_question || !facts.ready { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Ready, "input_unavailable"); }
        let mut refresh_guard = false;
        // Conversar guardado fica no composer: a publicação geral não carrega a interação.
        if expected.is_none() && facts.plugin_live && !text.trim_start().starts_with('/') {
            // Com algo guardado, o envio vai pelo composer: só ele devolve o guardado.
            let mode = if user_mode(&facts, text, before.stashed) { PluginMode::User } else { PluginMode::Fill };
            let request = PluginRequest { id: id.into(), text: text.into(), mode: mode.clone() };
            if self.services.writing().await.is_err() { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Plugin, "write_journal"); }
            // Modo `user`: o aviso do plugin só sai depois dos hooks da conta. Vale o que vier antes,
            // ele ou o `enqueue` no transcript, como no Python.
            let user = mode == PluginMode::User;
            let mark = if user { self.services.transcript_mark().await.ok().flatten() } else { None };
            let since = crate::state::monitor::wall_now();
            let publish = self.services.publish(&self.binding, request);
            tokio::pin!(publish);
            let reply = if !user { (&mut publish).await } else { loop {
                tokio::select! {
                    reply = &mut publish => break reply,
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {
                        let code = if let Some(mark) = mark && self.services.transcript_enqueued(mark, text).await.unwrap_or(false) { "plugin_enqueued" }
                            else if self.services.prompt_started(since, text).await.unwrap_or(false) { "prompt_started" }
                            else { continue };
                        drop(publish);
                        // Falhar aqui deixa a publicação no lugar: a próxima entrega sai incerta até o prazo dela.
                        if let Err(error) = self.services.abandon_publication(id).await {
                            tracing::warn!(code = error.0, "publicação do plugin não saiu depois da prova pelo transcript");
                        }
                        return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Plugin, code);
                    }
                }
            } };
            match reply {
                Ok(PluginReply::Unavailable | PluginReply::NotWritten) => refresh_guard = true,
                Ok(PluginReply::Accepted) if mode == PluginMode::User => return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Plugin, "plugin_accepted"),
                Ok(PluginReply::Filled) if mode == PluginMode::Fill => {
                    if self.prove_filled(text, &before).await { return self.submit(text, &before, draft, expected).await; }
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
        let guard = expected.map(|expected| GuardedText { expected,closed:true,pane:None,input:Some((text,&before)),allow_owned:false });
        let write = if use_clipboard { self.clipboard_guarded(text, guard).await }
            else if text.contains('\n') { self.paste_guarded(text, id, guard).await } else { self.literal_guarded(text, guard).await };
        if let Err(e) = write {
            if !e.may_have_written { return Self::failed(e, DeliveryStage::Write); }
            if expected.is_some() { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Write, "erro_sem_confirmacao_resposta"); }
            return self.partial(text, &before, DeliveryStage::Write).await;
        }
        let delay = if text.trim_start().starts_with('/') { self.limits.slash_settle }
            else if text.contains('\n') { self.limits.multiline_settle } else { self.limits.literal_settle };
        tokio::time::sleep(delay).await;
        if self.prove_input(text, &before).await { drop(clipboard); return self.submit(text, &before, draft, expected).await; }
        if expected.is_some() { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::InputProof, "erro_sem_confirmacao_resposta"); }
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
        // Sem a marca não se sabe se a TUI nunca teve fila ou já a consumiu: adiado com o código, nunca sucesso.
        if !screen.contains("ctrl+x ctrl+s to send now") { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Steer, "no_queued_input"); }
        for key in ["C-x", "C-s"] { if let Err(e) = self.key_inner(key).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Steer, e.code); } }
        DeliveryResult::new(Disposition::Accepted, DeliveryStage::Steer, "steered")
    }
    async fn navigate(&self, target: usize, mut screen: String, require_cursor: bool, expected: Option<&ExpectedInteraction>) -> Result<(String, bool), IoFailure> {
        let mut effects = false;
        let original = expected.map(|_| interaction_pane(&screen)).transpose().map_err(|error| IoFailure { code:error.code(), may_have_written:false })?;
        let mut row = cursor(&screen).or_else(|| unnumbered_cursor(&screen));
        if row.is_none() && require_cursor { return Err(IoFailure { code: "cursor_unreadable", may_have_written: false }); }
        if row.is_none() {
            for _ in 1..target {
                self.menu_key("Down", expected, original.as_ref()).await.map_err(|mut e| { e.may_have_written |= effects; e })?;
                effects = true;
                self.settle().await;
            }
            return self.capture_inner().await.map(|screen| (screen, effects)).map_err(|mut e| { e.may_have_written |= effects; e });
        }
        for _ in 0..4 {
            let current = row.ok_or(IoFailure { code: "cursor_lost", may_have_written: effects })?;
            if current == target { return Ok((screen, effects)); }
            for _ in 0..target.abs_diff(current) {
                self.menu_key(if current < target { "Down" } else { "Up" }, expected, original.as_ref()).await.map_err(|mut e| { e.may_have_written |= effects; e })?;
                effects = true;
                self.settle().await;
            }
            screen = self.capture_inner().await.map_err(|mut e| { e.may_have_written |= effects; e })?;
            row = cursor(&screen).or_else(|| unnumbered_cursor(&screen));
        }
        Err(IoFailure { code: "cursor_drift", may_have_written: effects })
    }
    /// Tecla que falhou: a guarda do menu recusa como interação; o resto deixa a entrega incerta.
    fn key_failed(error: IoFailure, stage: DeliveryStage, written: bool) -> DeliveryResult {
        match InteractionError::from_code(error.code) {
            Some(interaction) => Self::interaction_failure(interaction, written || error.may_have_written),
            None => DeliveryResult::new(Disposition::Unknown, stage, error.code),
        }
    }
    fn interaction_failure(error: InteractionError, written: bool) -> DeliveryResult {
        DeliveryResult::new(if written { Disposition::Unknown } else { Disposition::Rejected }, DeliveryStage::Identity, error.code())
    }

    fn check_source(&self, expected: &ExpectedInteraction, closed: bool) -> Result<(), InteractionError> {
        let store = self.interactions.as_ref().ok_or(InteractionError::Unavailable)?;
        if closed { store.check_after_close(expected)?; }
        else if store.expected(&expected.binding, Some(&expected.interaction_id))? != *expected { return Err(InteractionError::Changed); }
        if expected.binding.conversation != self.binding.conversation || expected.binding.generation != self.binding.generation {
            return Err(InteractionError::Changed);
        }
        if expected.semantic["source"] != "terminal" { return Err(InteractionError::Unavailable); }
        if let Some(revision) = &expected.source_revision {
            let path = self.interaction_source.as_ref().ok_or(InteractionError::Unavailable)?;
            match crate::state::ask::read_revision(path).map_err(|_| InteractionError::Unavailable)? {
                Some((question, current)) if current == *revision && json!(question.questions) == expected.semantic["questions"] => {},
                None if closed => {},
                _ => return Err(InteractionError::Changed),
            }
        } else {
            if expected.target.kind != InteractionKind::Menu { return Err(InteractionError::Unavailable); }
            // Só o sidecar que é a pergunta deste menu o tira dos botões do pane.
            if let Some(path) = &self.interaction_source {
                if crate::state::ask::holds_pane(store, &expected.binding, path, &expected.semantic["pane"]).map_err(|_| InteractionError::Unavailable)? { return Err(InteractionError::Changed); }
            }
        }
        Ok(())
    }

    async fn check_interaction(&self, expected: &ExpectedInteraction) -> Result<String, InteractionError> {
        self.check_source(expected, false)?;
        let screen = self.capture_inner().await.map_err(|_| InteractionError::Unavailable)?;
        if interaction_pane(&screen)? != expected.semantic["pane"] { return Err(InteractionError::Changed); }
        self.check_source(expected, false)?;
        Ok(screen)
    }

    pub async fn guarded_select(&self, expected: &ExpectedInteraction, option: usize, require_cursor: bool) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        if option == 0 || expected.semantic["pane"]["options"].as_array().is_none_or(|options| option > options.len()) {
            return Self::interaction_failure(InteractionError::Changed, false);
        }
        if let Some(refused) = self.plan_gone(expected).await { return refused; }
        if let Err(error) = self.check_interaction(expected).await { return Self::interaction_failure(error, false); }
        self.select_inner(option, require_cursor, Some(expected)).await
    }

    /// Cartão do plano cujo diálogo saiu da tela: recusa própria, antes de qualquer tecla.
    async fn plan_gone(&self, expected: &ExpectedInteraction) -> Option<DeliveryResult> {
        if !expected.target.request_key.starts_with("plugin:plan:") { return None; }
        match self.capture_inner().await {
            Ok(screen) if crate::terminal_state::plan_dialog(&crate::terminal_state::analyze(&screen)) => None,
            Ok(_) => Some(DeliveryResult::new(Disposition::Rejected, DeliveryStage::Identity, "plan_dialog_missing")),
            Err(error) => Some(Self::failed(error, DeliveryStage::Select)),
        }
    }

    /// Terceira opção do plano com o retorno: o diálogo abre o texto na própria linha da opção, e o
    /// Enter recusa o plano levando o texto a Claude.
    pub async fn guarded_plan_feedback(&self, expected: &ExpectedInteraction, option: usize, text: &str) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        if !expected.target.request_key.starts_with("plugin:plan:") || option != 3 || !valid_text(text) || text.trim().is_empty() || text.contains('\n') {
            return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_option");
        }
        if expected.semantic["pane"]["options"].as_array().is_none_or(|options| options.len() != 3) {
            return Self::interaction_failure(InteractionError::Changed, false);
        }
        if let Some(refused) = self.plan_gone(expected).await { return refused; }
        let screen = match self.check_interaction(expected).await { Ok(screen) => screen, Err(error) => return Self::interaction_failure(error, false) };
        if let Err(e) = self.navigate(option, screen, true, Some(expected)).await { return Self::not_converged(e, DeliveryStage::Select); }
        match self.check_interaction(expected).await {
            Ok(current) if cursor(&current) == Some(option) => {},
            Ok(_) => return Self::interaction_failure(InteractionError::Changed, false),
            Err(error) => return Self::interaction_failure(error, false),
        }
        let guard = GuardedText { expected, closed: false, pane: Some(&expected.semantic["pane"]), input: None, allow_owned: false };
        if let Err(e) = self.literal_guarded(text, Some(guard)).await { return Self::key_failed(e, DeliveryStage::Select, false); }
        self.settle().await;
        // O rótulo da opção virou o texto: a guarda do menu não serve mais, a prova é a própria linha.
        let typed = match self.capture_inner().await { Ok(s) => s, Err(e) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, e.code) };
        let analysis = crate::terminal_state::analyze(&typed);
        let label = analysis.options.as_ref().and_then(|o| o.get(2)).map(|l| compact(l)).unwrap_or_default();
        if !crate::terminal_state::plan_dialog(&analysis) || cursor(&typed) != Some(option) || label.is_empty() || !compact(text).starts_with(&label) {
            return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "plan_feedback_unproved");
        }
        if let Err(e) = self.key_inner("Enter").await { return Self::key_failed(e, DeliveryStage::Select, true); }
        self.settle().await;
        match self.capture_inner().await {
            Ok(after) if !after.trim().is_empty() && !crate::terminal_state::plan_dialog(&crate::terminal_state::analyze(&after)) =>
                DeliveryResult::new(Disposition::Accepted, DeliveryStage::Select, "selected"),
            _ => DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "plan_still_open"),
        }
    }

    pub async fn select(&self, option: usize, require_cursor: bool) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        self.select_inner(option, require_cursor, None).await
    }

    async fn select_inner(&self, option: usize, require_cursor: bool, expected: Option<&ExpectedInteraction>) -> DeliveryResult {
        if option == 0 || option > 100 { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_option"); }
        let screen = match expected {
            Some(expected) => match self.check_interaction(expected).await { Ok(screen) => screen, Err(error) => return Self::interaction_failure(error, false) },
            None => match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Select) },
        };
        // Até o Enter só saíram setas: falha aqui não respondeu nada (`erro_opcao_nao_convergiu`).
        let (screen, _) = match self.navigate(option, screen, require_cursor, expected).await { Ok(s) => s, Err(e) => return Self::not_converged(e, DeliveryStage::Select) };
        let screen = if let Some(expected) = expected {
            match self.check_interaction(expected).await {
                Ok(current) if cursor(&current).or_else(|| unnumbered_cursor(&current)) == Some(option) => current,
                Ok(_) => return Self::interaction_failure(InteractionError::Changed, false),
                Err(error) => return Self::interaction_failure(error, false),
            }
        } else { screen };
        let multiple = multiple_choice(&screen);
        if let Err(e) = self.menu_key(if multiple { "Space" } else { "Enter" }, expected, expected.map(|e| &e.semantic["pane"])).await { return Self::key_failed(e, DeliveryStage::Select, false); }
        // TUI lenta: algumas leituras antes de decidir que o picker não andou.
        let mut code = "selection_unproved";
        for attempt in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            let Ok(after) = self.capture_inner().await else { continue };
            if after.trim().is_empty() { continue; }
            code = "picker_still_open";
            if multiple {
                // O Space saiu e a marca não mudou: incerta com o código, nunca sucesso.
                if option_mark(&screen, option) == option_mark(&after, option) {
                    if attempt + 1 < self.limits.proof_attempts.max(1) { continue; }
                    return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, "selection_unchanged");
                }
                return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Select, "selected");
            }
            // Outra pergunta com os mesmos rótulos (duas permissões seguidas) não é o picker parado.
            let still = (live_picker(&after) || unnumbered_cursor(&after).is_some()) && option_labels(&screen) == option_labels(&after)
                && interaction_pane(&screen).ok() == interaction_pane(&after).ok();
            if !still { return DeliveryResult::new(Disposition::Accepted, DeliveryStage::Select, "selected"); }
        }
        DeliveryResult::new(Disposition::Unknown, DeliveryStage::Select, code)
    }
    /// Falha antes do Enter: a resposta não saiu. Recusa com o código, nunca incerta.
    fn not_converged(error: IoFailure, stage: DeliveryStage) -> DeliveryResult {
        match InteractionError::from_code(error.code) {
            Some(interaction) => Self::interaction_failure(interaction, false),
            None => DeliveryResult::new(Disposition::Rejected, stage, error.code),
        }
    }
    pub async fn guarded_submit_selected(&self, expected: &ExpectedInteraction) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        // Contrato da rota: só o menu do pane, que nasce sem sidecar e sem revisão; o `check_source`
        // prova que o sidecar continua ausente. Pergunta com sidecar responde por `answer_questions`.
        if expected.target.kind != InteractionKind::Menu || expected.source_revision.is_some() {
            return Self::interaction_failure(InteractionError::Unavailable, false);
        }
        if let Err(error) = self.check_interaction(expected).await { return Self::interaction_failure(error, false); }
        self.submit_selected_inner(Some(expected)).await
    }

    pub async fn submit_selected(&self) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        self.submit_selected_inner(None).await
    }

    async fn submit_selected_inner(&self, expected: Option<&ExpectedInteraction>) -> DeliveryResult {
        let screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Select) };
        if let Some(expected) = expected {
            if interaction_pane(&screen).ok().as_ref() != Some(&expected.semantic["pane"]) { return Self::interaction_failure(InteractionError::Changed, false); }
            if let Err(error) = self.check_source(expected, false) { return Self::interaction_failure(error, false); }
        }
        if !live_picker(&screen) || !multiple_choice(&screen) { return DeliveryResult::new(Disposition::Deferred, DeliveryStage::Select, "multiple_picker_missing"); }
        // Sem o Enter da aba de envio nada foi enviado: o `Right` só troca de aba.
        if let Err(e) = self.menu_key("Right", expected, expected.map(|e| &e.semantic["pane"])).await { return Self::not_converged(e, DeliveryStage::Select); }
        self.settle().await;
        let review = match self.capture_inner().await { Ok(s) if s.contains("Submit answers") => s, _ => return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Select, "submit_tab_missing") };
        // O Enter só vai para a mesma aba de envio que o `Right` abriu.
        let review = match expected.map(|_| guard_pane(&review)).transpose() { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, false) };
        if let Err(e) = self.menu_key("Enter", expected, review.as_ref()).await { return Self::key_failed(e, DeliveryStage::Select, true); }
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
    pub async fn guarded_answer_chat(&self, expected: &ExpectedInteraction, answers: &[QuestionAnswer], operation_id: &str) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        if expected.target.kind != InteractionKind::Question || expected.source_revision.is_none() {
            return Self::interaction_failure(InteractionError::Unavailable, false);
        }
        if answers.is_empty() || answers.iter().any(|a| !a.valid()) { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_answer"); }
        let questions: Vec<String> = expected.semantic["questions"].as_array().into_iter().flatten()
            .map(|q| q["question"].as_str().unwrap_or("").to_string()).collect();
        let values: Vec<Value> = answers.iter().map(|a| json!(a)).collect();
        let text = crate::session_write::answer::chat_text(&values, &questions);
        if text.is_empty() { return self.answer_inner(answers, Some(expected), None).await; }
        if let Err(error) = self.check_interaction(expected).await { return Self::interaction_failure(error, false); }
        self.close_and_send(expected, &text, operation_id).await
    }

    /// Esc no picker conferido, espera ele fechar e entrega `text` como mensagem, conferida contra a
    /// interação fechada. Quem chama já provou que o picker na tela é o do `expected`.
    async fn close_and_send(&self, expected: &ExpectedInteraction, text: &str, operation_id: &str) -> DeliveryResult {
        if let Err(error) = self.key_inner("Escape").await { return Self::failed(error, DeliveryStage::Answer); }
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            if let Err(error) = self.check_source(expected, true) { return Self::interaction_failure(error, true); }
            let (screen, typed) = match self.composer_capture().await {
                Ok(capture) => capture,
                Err(error) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, error.code),
            };
            if crate::terminal_state::footer_visible(&screen) || overlay(&screen) { continue; }
            let before = match Self::composer(&screen, &typed) {
                Ok(before) if before.is_empty() => before,
                _ => continue,
            };
            if !self.verify().await.is_ok_and(|facts| facts.ready && !facts.open_question) { continue; }
            if let Err(error) = self.check_source(expected, true) { return Self::interaction_failure(error, true); }
            let mut result = self.deliver(text, operation_id, before, None, Some(expected)).await;
            if result.disposition != Disposition::Accepted {
                result.disposition = Disposition::Unknown;
                result.code = "erro_sem_confirmacao_resposta".into();
            }
            return result;
        }
        DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, "answer_chat_close_unproved")
    }

    /// Reserva do Python quando as teclas da resposta falham: Esc e as respostas como mensagem. Só com o
    /// picker vivo mostrando o mesmo quadro publicado (nunca com o composer ou o turno na tela); sem essa
    /// prova, ou sem texto para mandar, `None` e fica a recusa com o motivo.
    pub async fn guarded_answer_fallback(&self, expected: &ExpectedInteraction, answers: &[QuestionAnswer], operation_id: &str) -> Option<DeliveryResult> {
        let _serial = self.serial.lock().await;
        if expected.target.kind != InteractionKind::Question || expected.source_revision.is_none() { return None; }
        let questions: Vec<String> = expected.semantic["questions"].as_array().into_iter().flatten()
            .map(|q| q["question"].as_str().unwrap_or("").to_string()).collect();
        let values: Vec<Value> = answers.iter().map(|a| json!(a)).collect();
        let text = crate::session_write::answer::fallback_text(&values, &questions);
        if text.is_empty() { return None; }
        let screen = self.check_interaction(expected).await.ok()?;
        if !live_picker(&screen) { return None; }
        let mut result = self.close_and_send(expected, &text, operation_id).await;
        result.code = match result.disposition {
            Disposition::Accepted => "answer_fallback_sent",
            _ if result.code == "answer_chat_close_unproved" => "answer_fallback_not_delivered",
            _ => "answer_fallback_unproved",
        }.into();
        if result.disposition != Disposition::Accepted { result.disposition = Disposition::Unknown; }
        Some(result)
    }

    pub async fn guarded_answer(&self, expected: &ExpectedInteraction, answers: &[QuestionAnswer]) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        if expected.source_revision.is_none() || expected.target.kind != InteractionKind::Question { return Self::interaction_failure(InteractionError::Unavailable, false); }
        if expected.semantic["questions"].as_array().is_none_or(|questions| questions.len() != answers.len()) { return Self::interaction_failure(InteractionError::Changed, false); }
        if let Err(error) = self.check_interaction(expected).await { return Self::interaction_failure(error, false); }
        self.answer_inner(answers, Some(expected), None).await
    }

    /// Pergunta do plugin respondida pelas teclas, quando ele não pode receber a resposta: só com o
    /// picker vivo mostrando a primeira pergunta dela.
    pub async fn plugin_ask_keys(&self, expected: &ExpectedInteraction, answers: &[QuestionAnswer]) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        if expected.target.kind != InteractionKind::Question || expected.semantic["questions"].as_array().is_none_or(|q| q.len() != answers.len()) {
            return Self::interaction_failure(InteractionError::Changed, false);
        }
        let screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return Self::failed(e, DeliveryStage::Answer) };
        let shown = live_picker(&screen) && interaction_pane(&screen).is_ok_and(|pane| crate::state::ask::pane_is_step(&expected.semantic["questions"], 0, &pane));
        if !shown { return Self::interaction_failure(InteractionError::Changed, false); }
        // Cada etapa conferida contra as perguntas do plugin: as teclas nunca caem noutra etapa ou formulário.
        self.answer_inner(answers, None, Some(&expected.semantic["questions"])).await
    }

    pub async fn answer(&self, answers: &[QuestionAnswer]) -> DeliveryResult {
        let _serial = self.serial.lock().await;
        self.answer_inner(answers, None, None).await
    }

    /// `form`: as perguntas de um formulário sem interação do terminal (a do plugin), conferidas etapa
    /// por etapa contra o pane como as do `expected`.
    async fn answer_inner(&self, answers: &[QuestionAnswer], expected: Option<&ExpectedInteraction>, form: Option<&Value>) -> DeliveryResult {
        if answers.is_empty() || answers.iter().any(|a| !a.valid()) { return DeliveryResult::new(Disposition::Rejected, DeliveryStage::Validate, "invalid_answer"); }
        let questions = expected.map(|expected| &expected.semantic["questions"]).or(form);
        let mut effects = false;
        for (index, answer) in answers.iter().enumerate() {
            if let Some(expected) = expected {
                if let Err(error) = self.check_source(expected, false) { return Self::interaction_failure(error, effects); }
            }
            if let Some(questions) = questions {
                let Some(question) = questions.get(index) else { return Self::interaction_failure(InteractionError::Changed, effects); };
                let labels: Vec<_> = question["options"].as_array().into_iter().flatten().filter_map(|o| o["label"].as_str()).collect();
                if answer.kind == AnswerKind::Option && (answer.indices.iter().any(|i| *i >= labels.len())
                    || answer.indices.iter().map(|i| labels[*i]).collect::<Vec<_>>() != answer.labels.iter().map(String::as_str).collect::<Vec<_>>()) {
                    return Self::interaction_failure(InteractionError::Changed, effects);
                }
            }
            let screen = match self.capture_inner().await { Ok(s) if live_picker(&s) => s, _ => return DeliveryResult::new(if effects { Disposition::Unknown } else { Disposition::Deferred }, DeliveryStage::Answer, "picker_missing") };
            if let Some(questions) = questions {
                let pane = match interaction_pane(&screen) { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, effects) };
                // A primeira etapa é o próprio quadro publicado; as seguintes, o mesmo casamento da publicação.
                let same = match expected {
                    Some(expected) if index == 0 => pane == expected.semantic["pane"],
                    _ => crate::state::ask::pane_is_step(questions, index, &pane),
                };
                if !same { return Self::interaction_failure(InteractionError::Changed, effects); }
            }
            let mut screen = screen;
            for target in answer.targets() {
                let (selected, moved) = match self.navigate(target, screen, true, expected).await {
                    Ok(selected) => selected,
                    // Antes de qualquer Enter só setas saíram: recusa, como no `/select`.
                    Err(error) if !effects => return Self::not_converged(error, DeliveryStage::Answer),
                    Err(mut error) => { error.may_have_written = true; return Self::failed(error, DeliveryStage::Answer); },
                };
                effects |= moved;
                if let Some(expected) = expected {
                    if let Err(error) = self.check_source(expected, false) { return Self::interaction_failure(error, effects); }
                    let fresh = match self.capture_inner().await { Ok(fresh) => fresh, Err(error) => return Self::failed(IoFailure { may_have_written:effects, ..error }, DeliveryStage::Answer) };
                    if interaction_pane(&fresh).ok() != interaction_pane(&selected).ok() || cursor(&fresh) != Some(target) {
                        return Self::interaction_failure(InteractionError::Changed, effects);
                    }
                }
                let text_pane = if answer.kind == AnswerKind::Text && expected.is_some() {
                    let pane = match interaction_pane(&selected) { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, effects) };
                    if pane["options"].get(target - 1).and_then(Value::as_str).is_none_or(|label|label.trim_end_matches('.') != "Type something") {
                        return Self::interaction_failure(InteractionError::Changed, effects);
                    }
                    Some(pane)
                } else { None };
                let selected_pane = expected.and_then(|_| interaction_pane(&selected).ok());
                if let Err(e) = self.menu_key(if answer.kind == AnswerKind::Option && answer.multi { "Space" } else { "Enter" }, expected, selected_pane.as_ref()).await { return Self::key_failed(e, DeliveryStage::Answer, effects); }
                effects = true;
                self.settle().await;
                if answer.kind == AnswerKind::Text {
                    let pane = if let Some(expected) = expected {
                        if let Err(error) = self.check_source(expected, false) { return Self::interaction_failure(error, true); }
                        let fresh = match self.capture_inner().await { Ok(screen) => screen, Err(error) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, error.code) };
                        let pane = match interaction_pane(&fresh) { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, true) };
                        if Some(&pane) != text_pane.as_ref() {
                            return Self::interaction_failure(InteractionError::Changed, true);
                        }
                        text_pane
                    } else { None };
                    let guard = expected.map(|expected| GuardedText { expected,closed:false,pane:pane.as_ref(),input:None,allow_owned:false });
                    if let Err(e) = self.literal_guarded(answer.value.as_deref().unwrap(), guard).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                    self.settle().await;
                    if let Err(e) = self.key_guarded("Enter", guard).await { return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code); }
                }
                screen = match self.capture_inner().await { Ok(s) => s, Err(e) => return DeliveryResult::new(Disposition::Unknown, DeliveryStage::Answer, e.code) };
            }
            if answer.multi {
                let pane = match expected.map(|_| interaction_pane(&screen)).transpose() { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, effects) };
                if let Err(e) = self.menu_key("Right", expected, pane.as_ref()).await { return Self::key_failed(e, DeliveryStage::Answer, effects); }
                self.settle().await;
            }
        }
        for _ in 0..self.limits.proof_attempts.max(1) {
            self.settle().await;
            let Ok(screen) = self.capture_inner().await else { continue; };
            if screen.contains("Submit answers") {
                if !review_matches(&screen, answers) { continue; }
                let review = match expected.map(|_| guard_pane(&screen)).transpose() { Ok(pane) => pane, Err(error) => return Self::interaction_failure(error, true) };
                if let Err(e) = self.menu_key("Enter", expected, review.as_ref()).await { return Self::key_failed(e, DeliveryStage::Answer, true); }
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
    if native_message(facts, text) { return false; }
    let slash = text.trim_start().starts_with('/');
    !(facts.plugin_live && !slash && user_mode(facts, text, false))
}

pub(crate) fn message_prefix(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('[')?;
    let (prefix, body) = rest.split_once(']')?;
    let sender = ["de:", "grupo:", "painel:"].iter().find_map(|kind| prefix.strip_prefix(kind))?.trim();
    if sender.is_empty() { return None; }
    Some((sender, body.trim_start()))
}
/// Recado de outra sessão com o socket nativo do agente: vai pelo canal do próprio Claude Code, não pelo teclado.
pub fn native_message(facts: &InputFacts, text: &str) -> bool { facts.native.is_some() && recognized_message(text).is_some() }
fn recognized_message(text: &str) -> Option<&str> { message_prefix(text).map(|(_, body)| body) }
/// Nem menu nem diálogo, e o composer não se lê: espera que não é do Claude, é da leitura.
pub(crate) fn composer_unreadable(screen: &str) -> bool {
    let state = crate::terminal_state::analyze(screen);
    !state.overlay && state.state != "awaiting_input" && ComposerSnapshot::parse(screen).is_none()
}
pub(crate) fn input_state(screen: &str) -> (bool, bool) {
    let state = crate::terminal_state::analyze(screen);
    (ComposerSnapshot::parse(screen).is_some() && !state.overlay && state.state != "awaiting_input", state.state == "idle")
}
/// Corpo completo do hook, sem envolver outra vez as respostas.
pub(crate) fn plugin_answer_body(questions: &Value, answers: &Value) -> Result<Value, ServiceError> {
    let invalid = || ServiceError("terminal_answer_invalid");
    let questions = questions.as_array().ok_or_else(invalid)?;
    let answers = answers.as_array().filter(|answers| answers.len() == questions.len()).ok_or_else(invalid)?;
    let mut response = serde_json::Map::new();let mut answered = Vec::new();let mut chat = Vec::new();
    for (question, answer) in questions.iter().zip(answers) {
        let title = question["question"].as_str().ok_or_else(invalid)?;
        let text = match answer["kind"].as_str() {
            Some("chat") => { chat.push(title);continue; }
            Some("text") => answer["value"].as_str().unwrap_or("").trim().to_owned(),
            Some("option") => {
                // Os índices escolhem a opção; rótulos enviados só confirmam, nunca substituem.
                let options = question["options"].as_array().ok_or_else(invalid)?;
                let chosen = answer["indices"].as_array().ok_or_else(invalid)?.iter()
                    .map(|index| index.as_u64().and_then(|index| options.get(usize::try_from(index).ok()?)).and_then(|option| option["label"].as_str()).ok_or_else(invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                // Espaço a mais ou a menos no rótulo enviado não troca a opção.
                let same = |sent: Option<&str>, label: &str| sent.is_some_and(|sent| sent.split_whitespace().eq(label.split_whitespace()));
                if let Some(labels) = answer["labels"].as_array().filter(|labels| !labels.is_empty())
                    && (labels.len() != chosen.len() || !labels.iter().zip(&chosen).all(|(sent, label)| same(sent.as_str(), label))) {
                    return Err(invalid());
                }
                chosen.join(", ")
            }
            _ => return Err(invalid()),
        };
        if text.is_empty() { return Err(invalid()); }
        answered.push(format!("- {title} → {text}"));response.insert(title.into(), json!(text));
    }
    if chat.is_empty() { return Ok(json!({"answers":response})); }
    let mut lines = if chat.len() == 1 { vec![format!("Sobre «{}» ele prefere conversar antes de responder.", chat[0])] }
        else { let mut lines = vec!["Sobre estas perguntas ele prefere conversar antes de responder:".into()];lines.extend(chat.iter().map(|title| format!("- {title}")));lines };
    if !answered.is_empty() { lines.push("Ele já respondeu:".into());lines.extend(answered); }
    lines.push("Não repita a pergunta: responda em texto e aguarde a mensagem dele.".into());Ok(json!({"deny":lines.join("\n")}))
}
fn escape_attribute(text: &str) -> String { text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;") }
pub(crate) fn native_envelope(descriptor: &NativeMessage, text: &str, id: &str) -> Vec<u8> {
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
/// O Review lista uma linha `→` por pergunta, as últimas da tela (acima pode haver `→` da conversa).
/// Só os rótulos de opção se conferem, como no Python: texto livre pode ter vírgula ou quebrar linha.
pub fn review_matches(screen: &str, answers: &[QuestionAnswer]) -> bool {
    let rows: Vec<&str> = screen.lines().filter_map(|l| l.trim().strip_prefix('→')).collect();
    let Some(rows) = rows.len().checked_sub(answers.len()).map(|skip| &rows[skip..]) else { return false };
    answers.iter().zip(rows).all(|(a, row)| {
        // Texto livre: a linha mostra o começo dele (o resto quebra sem `→`), com vírgula e tudo.
        if a.kind == AnswerKind::Text {
            let shown = compact(row.trim().trim_end_matches('…'));
            return !shown.is_empty() && a.value.as_deref().is_some_and(|value| compact(value).starts_with(&shown));
        }
        if a.kind != AnswerKind::Option { return true; }
        let expected: BTreeSet<_> = a.labels.iter().map(String::as_str).collect();
        !expected.is_empty() && expected == row.split(',').map(str::trim).collect()
    })
}

#[cfg(test)]
mod enqueued_tests {
    use super::{enqueued_since, prompt_started_in};

    #[test]
    fn prompt_started_reads_the_state_hook_mark() {
        // O mesmo marcador do `state_hook.py` para o prompt "abc" (SHA-1 conhecido).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".hangar-prompt")).unwrap();
        std::fs::write(dir.path().join(".hangar-prompt/sid.json"), r#"{"sha1":"a9993e364706816aba3e25717850c26c9cd0d89d","ts":100.5}"#).unwrap();
        assert!(prompt_started_in(dir.path(), "sid", 100.0, " abc\n"));
        assert!(!prompt_started_in(dir.path(), "sid", 101.0, "abc"), "marca de antes do envio");
        assert!(!prompt_started_in(dir.path(), "sid", 100.0, "abd"), "outro texto");
        assert!(!prompt_started_in(dir.path(), "outra", 100.0, "abc"), "outra conversa");
    }

    fn transcript(lines: &[&str]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        file
    }

    #[test]
    fn idle_prompt_is_proved_by_its_user_line_before_the_hooks() {
        let file = transcript(&[
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-14T22:13:09.577Z"}"#,
            r#"{"type":"queue-operation","operation":"dequeue","timestamp":"2026-10-14T22:13:09.589Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"tem um espaço ai — 📎 imagem: /tmp/a.png"}}"#,
        ]);
        assert!(enqueued_since(file.path(), 0, "tem um espaço ai — 📎 imagem: /tmp/a.png"));
        assert!(!enqueued_since(file.path(), 0, "outra mensagem"));
    }

    #[test]
    fn mid_turn_prompt_is_proved_by_the_enqueue_content() {
        let file = transcript(&[r#"{"type":"queue-operation","operation":"enqueue","content":"nao e a faixa"}"#]);
        assert!(enqueued_since(file.path(), 0, "nao e a faixa"));
    }

    #[test]
    fn a_tool_result_with_the_same_text_proves_nothing() {
        let file = transcript(&[r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"oi"}]}}"#]);
        assert!(!enqueued_since(file.path(), 0, "oi"));
    }
}
