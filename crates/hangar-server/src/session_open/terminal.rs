//! O lançador de terminal de todo provedor: `tmux new-session` (o psmux no Windows) com o `-e`
//! completo, a conta, o escopo do systemd e o comando no dialeto do multiplexador. Port do
//! `tmux.new_session` do Python; quem monta o comando do agente é o plano de cada provedor.
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::future::BoxFuture;

use super::AgentProvider;

/// Saída de um comando do multiplexador ou de um programa da abertura; `code: None` = não terminou no prazo ou não rodou.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MuxOutput { pub code: Option<i32>, pub stdout: String, pub stderr: String }

impl MuxOutput {
    fn ok(&self) -> bool { self.code == Some(0) }
}

/// O pane pedido pode estar de pé, mas o multiplexador não respondeu: quem chama não desfaz nada em volta dele.
pub const UNCERTAIN: &str = "terminal_incerto";

/// O que o multiplexador disse de uma sessão. Sem resposta (`code: None`) não é ausência.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence { Present, Absent, Unknown }

/// `has-session` em três estados, com a última linha do stderr.
pub async fn presence(mux: &dyn Mux, env: &TerminalEnv, name: &str) -> (Presence, String) {
    let out = mux.run(vec![env.program.clone(), "has-session".into(), "-t".into(), format!("={name}")]).await;
    let presence = match out.code { Some(0) => Presence::Present, Some(_) => Presence::Absent, None => Presence::Unknown };
    (presence, tail(&out.stderr))
}

/// Quem roda o multiplexador: o processo de verdade ou o falso dos testes e do harness.
pub trait Mux: Send + Sync {
    /// Prefixo do escopo transiente do systemd (`systemd-run … --`); vazio sem ele.
    fn scope(&self) -> BoxFuture<'_, Vec<String>>;
    fn run(&self, argv: Vec<String>) -> BoxFuture<'_, MuxOutput>;
    /// Uma linha do diário exportável sobre o próprio multiplexador.
    fn diary(&self, line: serde_json::Value);
}

/// O que vem da máquina e não do pedido.
#[derive(Clone, Debug)]
pub struct TerminalEnv {
    pub windows: bool,
    /// O `tmux` (psmux no Windows).
    pub program: String,
    /// As variáveis de tela (`display_env::KEYS`): as do servidor e as que faltam nele, achadas.
    pub display: Vec<(String, String)>,
    /// `CLAUDE_CONFIG_DIR` do servidor, quando ele tem um.
    pub server_config_dir: Option<String>,
    /// `~/.claude`: a conta padrão, que para o Claude Code é a ausência da variável.
    pub default_config_dir: PathBuf,
    /// Quanto o pane precisa ficar de pé depois de criado para não contar como morto ao nascer.
    pub birth: Duration,
}

impl TerminalEnv {
    /// A máquina de quem chama: o servidor ou o comando de terminal.
    pub fn local(home: &Path, env: &std::collections::BTreeMap<String, String>) -> Self {
        let var = |name: &str| env.get(name).filter(|v| !v.is_empty()).cloned();
        let display = crate::display_env::for_launch(var);
        // ponytail: janela fixa; um CLI que cai depois dela passa como vivo. Prova por provedor (o
        // agente com o id da conversa no pane) quando a reabertura vier para cá.
        let birth = Duration::from_millis(if cfg!(windows) { 1500 } else { 500 });
        Self { windows: cfg!(windows), program: "tmux".into(), display, server_config_dir: var("CLAUDE_CONFIG_DIR"),
            default_config_dir: home.join(".claude"), birth }
    }
}

/// O pane que se pede: a identidade, a conta e o que roda nele.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSpec {
    pub name: String,
    pub cwd: String,
    pub provider: AgentProvider,
    /// `CP_SESSION_KEY`: a chave do spec, a mesma em toda vida da sessão.
    pub key: String,
    /// `-e` da sessão, depois dos de todo provedor, na ordem.
    pub env: Vec<(String, String)>,
    /// Conta Claude que o pane lê; `None` = a padrão.
    pub config_dir: Option<String>,
    /// argv do que roda no pane.
    pub command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnError {
    /// O `detalhe` do diário.
    pub code: &'static str,
    /// Última linha do stderr do multiplexador, cortada.
    pub stderr: String,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.stderr) }
}

/// `_mesmo_dir`: mesma pasta escrita de jeitos diferentes (barra, maiúscula no Windows).
fn same_dir(a: &str, b: &Path, windows: bool) -> bool {
    let norm = |p: &str| {
        let p = p.trim_end_matches(['/', '\\']).to_owned();
        if windows { p.replace('/', "\\").to_lowercase() } else { p }
    };
    norm(a) == norm(&b.to_string_lossy())
}

/// O `-e CLAUDE_CONFIG_DIR` vai quando a conta não é a padrão ou o servidor tem a variável (omitir
/// não apagaria a herança). A padrão é a ausência: exportar `~/.claude` faz o CLI ler outro `.claude.json`.
pub fn exports_config_dir(env: &TerminalEnv, config_dir: Option<&str>) -> Option<String> {
    let chosen = config_dir.map(str::to_owned).or_else(|| env.server_config_dir.clone())
        .unwrap_or_else(|| env.default_config_dir.to_string_lossy().into_owned());
    (env.server_config_dir.is_some() || !same_dir(&chosen, &env.default_config_dir, env.windows)).then_some(chosen)
}

/// `join_cmd`: o argv numa string só, no dialeto do multiplexador (POSIX: `shlex.join`; psmux:
/// aspas simples com o apóstrofo dobrado).
pub fn join(argv: &[String], windows: bool) -> String {
    argv.iter().map(|arg| crate::runtime::claude_relaunch::quote(arg, windows)).collect::<Vec<_>>().join(" ")
}

/// O agente dentro do envelope do só leitura (`bwrap …`), por um `/bin/sh -c` lá dentro. O `exec` troca o
/// shell pelo agente: no dash ele ficaria de pai, e a restauração veria o shell em vez do agente.
pub fn sandboxed(prefix: &[String], command: &[String], windows: bool) -> Vec<String> {
    let inner = if windows { join(command, windows) } else { format!("exec {}", join(command, windows)) };
    prefix.iter().cloned().chain(["/bin/sh".into(), "-c".into(), inner]).collect()
}

/// O que só a sessão põe (ou a configuração do servidor, relida na abertura) e o que é
/// segredo do servidor. Servidor tmux subido de dentro de outra sessão teria os dela; o pane sai sem
/// os que esta não pôs por `-e`. As do motor são as que o `hangar-engine` exporta (`engines.env_de`).
pub const NOT_INHERITED: [&str; 46] = [
    "CLAUDE_CODE_SUBAGENT_MODEL", "CLAUDE_CODE_ENABLE_FUNCTION_HOOKS", "HANGAR_PLUGIN_URL", "HANGAR_PLUGIN_TOKEN",
    "HANGAR_CLAUDE_SETTINGS", "HANGAR_JEV", "TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL",
    "JEV_TEXTO_BASE_URL", "JEV_TEXTO_API_KEY", "JEV_TEXTO_MODELO", "JEV_TEXTO_CMD",
    "JEV_AUX_BASE_URL", "JEV_AUX_API_KEY", "JEV_AUX_MODELO", "JEV_AUX_ESFORCO",
    "CP_ENGINE", "CP_ENGINE_ACCOUNT", "CP_ENGINE_CREDENTIAL_ID", "CP_ENGINE_ACCOUNT_BASE_URL", "CP_ENGINE_SERVICE_TIER",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "HANGAR_ORQ_READ_ONLY", "HANGAR_CANO_KEY", "HANGAR_CANO_OWNER",
    "CP_PI_SESSION", "OMP_PROFILE",
    "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_CUSTOM_HEADERS", "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "CLAUDE_CODE_EXTRA_BODY", "CLAUDE_CODE_AUTO_COMPACT_WINDOW", "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS", "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS", "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
    "ENABLE_TOOL_SEARCH", "DISABLE_PROMPT_CACHING",
];
/// Do servidor: nunca para o pane, nem para o cliente que pode subir o servidor tmux.
const SERVER_SECRETS: [&str; 3] = ["CP_AUTH_TOKEN", "HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE"];

/// O argv do `new-session`, sem o escopo.
pub fn new_session_argv(env: &TerminalEnv, pane: &PaneSpec) -> Vec<String> {
    let mut argv: Vec<String> = [env.program.as_str(), "new-session", "-d", "-s", &pane.name, "-c", &pane.cwd, "-x", "200", "-y", "50"]
        .map(str::to_owned).into();
    let mut e = |key: &str, value: &str| argv.extend(["-e".to_owned(), format!("{key}={value}")]);
    e("COLORTERM", "truecolor");
    e("CLAUDE_CODE_TMUX_TRUECOLOR", "1");
    e("KIMI_CODE_TUI_FULL_SCREEN", "1");
    e("CP_SESSION_NAME", &pane.name);
    e("CP_PROVIDER", pane.provider.name());
    e("CP_SESSION_KEY", &pane.key);
    for (key, value) in &env.display { e(key, value); }
    for (key, value) in &pane.env { e(key, value); }
    let exported = exports_config_dir(env, pane.config_dir.as_deref());
    if let Some(dir) = &exported { e("CLAUDE_CONFIG_DIR", dir); }
    let command = join(&pane.command, env.windows);
    // `exec`: o agente vira o processo do pane e recebe o teclado. No psmux o comando roda sem shell
    // no meio, e `exec` mataria o pane ao nascer; lá o pane herda de quem chama (`client_command`).
    if env.windows {
        argv.push(command);
        return argv;
    }
    // O pane herda o ambiente do servidor tmux, de quem o subiu: a conta padrão é a ausência.
    let own = |var: &&str| pane.env.iter().any(|(key, _)| key == var);
    let unset: Vec<&str> = exported.is_none().then_some("CLAUDE_CONFIG_DIR").into_iter()
        .chain(NOT_INHERITED.iter().copied().filter(|var| !own(var))).chain(SERVER_SECRETS).collect();
    // Em argumentos separados o tmux roda direto, sem o `default-shell`: o fish carrega o `conf.d` até
    // com `-c`, e o que trava lá segura a sessão sem prazo.
    argv.extend(["/bin/sh".to_owned(), "-c".to_owned()]);
    argv.push(format!("exec env {} {command}", unset.iter().map(|var| format!("-u {var}")).collect::<Vec<_>>().join(" ")));
    argv
}

/// Última linha não vazia, cortada: vai ao diário sem trazer a saída inteira.
fn tail(stderr: &str) -> String {
    stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("").chars().take(200).collect()
}

/// Cria o pane. Escopo que o systemd recusa (o gerenciador parou de criar escopo depois da sonda)
/// repete uma vez sem ele, como o `_scope_lost` do Python.
pub async fn spawn(mux: &dyn Mux, env: &TerminalEnv, pane: &PaneSpec) -> Result<(), SpawnError> {
    let argv = new_session_argv(env, pane);
    let scope = if env.windows { Vec::new() } else { mux.scope().await };
    let mut scoped = scope.clone();
    scoped.extend(argv.iter().cloned());
    let mut output = mux.run(scoped.clone()).await;
    // Escopos criados juntos (o `orq` cria em rajada) recebem do systemd uma recusa passageira;
    // ela se repete, com o escopo e com espera; esgotada, é erro com o código dela.
    let mut waits = crate::runtime::process::SCOPE_RETRY.iter();
    while !scope.is_empty() && !output.ok() && crate::runtime::process::scope_unit_missing(output.stderr.as_bytes()).is_some() {
        let Some(wait) = waits.next() else {
            return Err(SpawnError { code: crate::runtime::process::SCOPE_UNIT_MISSING, stderr: tail(&output.stderr) });
        };
        tokio::time::sleep(crate::runtime::process::scope_wait(*wait)).await;
        output = mux.run(scoped.clone()).await;
    }
    if !scope.is_empty() && !output.ok() && output.stderr.trim_start().starts_with("Failed to ") {
        tracing::warn!(code = "systemd_scope_refused", cause = %tail(&output.stderr), "escopo do systemd recusado; o terminal sobe sem ele");
        mux.diary(serde_json::json!({"evento": "mux.scope_perdido", "nivel": "aviso", "retorno": output.code, "comando": "new-session"}));
        output = mux.run(argv).await;
    }
    // Sem resposta, a sessão pode ter nascido (e o psmux devolve diferente de zero com ela de
    // pé). Só vale se for esta: outra de mesmo nome ganhou a corrida e o pedido é recusado.
    if !output.ok() {
        match is_this_one(mux, env, pane).await {
            Presence::Present => {}
            Presence::Absent => return Err(SpawnError { code: "terminal_nao_criado", stderr: tail(&output.stderr) }),
            Presence::Unknown => return Err(SpawnError { code: UNCERTAIN, stderr: tail(&output.stderr) }),
        }
    }
    born(mux, env, &pane.name).await
}

/// O `new-session` devolve 0 com o comando morto ao nascer (binário ausente, flag recusada).
/// A janela é de relógio. Com o pid do pane (Linux), a saída do processo chega como sinal pelo
/// `pidfd`; sem ele (psmux), o `has-session` é conferido a cada 100 ms até o prazo. Sem resposta até
/// o fim é incerto, nunca "morreu".
async fn born(mux: &dyn Mux, env: &TerminalEnv, name: &str) -> Result<(), SpawnError> {
    let deadline = tokio::time::Instant::now() + env.birth;
    let died = |stderr: String| SpawnError { code: "terminal_morreu_ao_nascer", stderr };
    if let Some(pid) = pane_pid(mux, env, name).await && let Some(exited) = wait_exit(pid, deadline).await {
        return if exited { Err(died("o comando do terminal saiu logo depois de abrir".into())) } else { Ok(()) };
    }
    let mut unanswered = None;
    loop {
        tokio::time::sleep_until((tokio::time::Instant::now() + Duration::from_millis(100)).min(deadline)).await;
        match presence(mux, env, name).await {
            (Presence::Absent, stderr) => return Err(died(stderr)),
            (Presence::Unknown, stderr) => unanswered = Some(stderr),
            (Presence::Present, _) => unanswered = None,
        }
        if tokio::time::Instant::now() >= deadline { break; }
    }
    unanswered.map_or(Ok(()), |stderr| Err(SpawnError { code: UNCERTAIN, stderr }))
}

/// O pid do processo do pane; `None` fora do Linux (sem `pidfd` para esperar por ele) ou sem resposta.
async fn pane_pid(mux: &dyn Mux, env: &TerminalEnv, name: &str) -> Option<i32> {
    if !cfg!(target_os = "linux") || env.windows { return None; }
    let out = mux.run(vec![env.program.clone(), "display-message".into(), "-p".into(), "-t".into(), format!("={name}"), "#{pane_pid}".into()]).await;
    out.ok().then(|| out.stdout.trim().parse().ok()).flatten().filter(|pid| *pid > 0)
}

/// Espera o processo sair até o prazo: `Some(true)` saiu, `Some(false)` vivo no prazo, `None` sem como esperar.
#[cfg(target_os = "linux")]
async fn wait_exit(pid: i32, deadline: tokio::time::Instant) -> Option<bool> {
    use std::os::fd::FromRawFd;
    // SAFETY: syscall sem ponteiros; o descritor devolvido passa a ser só deste `OwnedFd`.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return (std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)).then_some(true);
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) };
    let fd = tokio::io::unix::AsyncFd::with_interest(fd, tokio::io::Interest::READABLE).ok()?;
    match tokio::time::timeout_at(deadline, fd.readable()).await {
        Ok(Ok(_)) => Some(true),
        Ok(Err(_)) => None,
        Err(_) => Some(false),
    }
}

#[cfg(not(target_os = "linux"))]
async fn wait_exit(_pid: i32, _deadline: tokio::time::Instant) -> Option<bool> { None }

/// O alvo da sessão: `=nome` casa exato (sem ele o tmux cai em prefixo).
pub(crate) fn target(windows: bool, name: &str) -> String { if windows { name.into() } else { format!("={name}") } }

/// A sessão `pane.name` existe e carrega a chave deste pedido. `Absent` também quando a de mesmo nome é outra.
async fn is_this_one(mux: &dyn Mux, env: &TerminalEnv, pane: &PaneSpec) -> Presence {
    let (exists, _) = presence(mux, env, &pane.name).await;
    if exists != Presence::Present { return exists; }
    let key = mux.run(vec![env.program.clone(), "show-environment".into(), "-t".into(), target(env.windows, &pane.name), "CP_SESSION_KEY".into()]).await;
    match key.code {
        None => Presence::Unknown,
        Some(0) if key.stdout.trim() == format!("CP_SESSION_KEY={}", pane.key) => Presence::Present,
        Some(_) => Presence::Absent,
    }
}

/// O cliente do multiplexador: no psmux o pane herda o ambiente dele, e no tmux ele vira o do
/// servidor quando é ele quem o sobe. A conta vai só pelo `-e`.
pub fn client_command(program: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::from(crate::terminal_input::child_command(program));
    for var in ["CLAUDE_CONFIG_DIR"].into_iter().chain(NOT_INHERITED).chain(SERVER_SECRETS) { command.env_remove(var); }
    command
}

/// O argv que vai ao processo de verdade. O systemd grava a linha de comando do escopo no journal:
/// o escopo leva descrição própria, e o `-e` de chave secreta vira um arquivo que o shell do pane lê
/// e apaga antes do agente (fora também do `/proc/<pid>/cmdline`). Devolve o conteúdo do arquivo.
#[cfg_attr(not(unix), allow(dead_code))]
fn private_launch(mut argv: Vec<String>, file: &Path) -> (Vec<String>, Option<String>) {
    let Some(at) = argv.iter().position(|a| a == "new-session") else { return (argv, None) };
    let name = argv[at..].windows(2).find(|w| w[0] == "-s").map(|w| w[1].clone()).unwrap_or_default();
    let scoped = argv.first().is_some_and(|p| Path::new(p).file_name().is_some_and(|f| f == "systemd-run"));
    let mut at = at;
    if scoped && let Some(end) = argv[..at].iter().position(|a| a == "--") {
        argv.insert(end, format!("--description=hangar:{name}"));
        at += 1;
    }
    let secret = |pair: &str| pair.split_once('=').is_some_and(|(k, _)| ["_API_KEY", "_TOKEN", "_SECRET"].iter().any(|s| k.ends_with(s)));
    let shell = argv.len() >= 3 && argv[argv.len() - 3] == "/bin/sh" && argv[argv.len() - 2] == "-c";
    if !shell { return (argv, None) }
    let mut contents = String::new();
    let mut i = at;
    while i + 1 < argv.len() {
        if argv[i] == "-e" && secret(&argv[i + 1]) {
            let (key, value) = argv[i + 1].split_once('=').unwrap();
            contents.push_str(&format!("export {key}={}\n", quote(value)));
            argv.drain(i..i + 2);
        } else { i += 1; }
    }
    if contents.is_empty() { return (argv, None) }
    let path = quote(&file.to_string_lossy());
    let command = argv.pop().unwrap();
    argv.push(format!(". {path} && rm -f {path} && {command}"));
    (argv, Some(contents))
}

fn quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }

/// Grava o arquivo de segredos do pane: só o dono lê, nome novo por subida.
#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // O pane lê e apaga em instantes; o que passou de 10 min é de uma subida que não respondeu.
        let stale = std::time::SystemTime::now() - Duration::from_secs(600);
        for entry in std::fs::read_dir(dir)?.flatten() {
            if entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at < stale) {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    tracing::warn!(kind = ?error.kind(), "arquivo de segredos antigo do pane não foi apagado");
                }
            }
        }
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(contents.as_bytes())
}

/// O processo de verdade, sem os segredos do servidor no ambiente do cliente (que vira o do
/// servidor tmux quando é ele quem o sobe).
/// `probe`: a sondagem do escopo (a do systemd; os testes trocam por uma falsa).
/// `diary`: a pasta do diário (sem ela, nada é gravado: os testes não escrevem no da máquina).
pub struct ProcessMux { pub timeout: Duration, pub probe: fn() -> Vec<String>, pub diary: Option<PathBuf> }

impl Default for ProcessMux { fn default() -> Self { Self { timeout: Duration::from_secs(5), probe: systemd_scope, diary: None } } }

/// Bloqueante: a primeira chamada roda o `systemd-run` da sondagem.
pub(crate) fn systemd_scope() -> Vec<String> {
    #[cfg(unix)]
    { crate::runtime::process::scope_prefix().iter().map(|a| a.to_string_lossy().into_owned()).collect() }
    #[cfg(not(unix))]
    { Vec::new() }
}

impl Mux for ProcessMux {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> {
        let probe = self.probe;
        Box::pin(async move {
            tokio::task::spawn_blocking(probe).await.unwrap_or_else(|error| {
                tracing::warn!(%error, "sondagem do escopo systemd falhou; o terminal sobe sem o escopo");
                Vec::new()
            })
        })
    }

    fn diary(&self, line: serde_json::Value) {
        if self.diary.is_some() { crate::diag::record(self.diary.clone(), vec![line], "multiplexador"); }
    }

    fn run(&self, argv: Vec<String>) -> BoxFuture<'_, MuxOutput> {
        Box::pin(async move {
            #[cfg(unix)]
            let (argv, secrets) = match std::env::var_os("XDG_RUNTIME_DIR") {
                Some(dir) => {
                    let path = Path::new(&dir).join("hangar/env").join(format!("{}.env", crate::mods::state::random_hex(8)));
                    match private_launch(argv.clone(), &path) {
                        (private, Some(contents)) => match write_private(&path, &contents) {
                            Ok(()) => (private, Some(path)),
                            Err(error) => {
                                tracing::warn!(kind = ?error.kind(), "arquivo de segredos do pane não foi gravado; as chaves seguem pelo -e");
                                if self.diary.is_some() {
                                    let line = serde_json::json!({"evento": "mux.segredos_no_argv", "nivel": "aviso", "erro_tipo": format!("{:?}", error.kind())});
                                    let _ = crate::diag::record(self.diary.clone(), vec![line], "multiplexador").await;
                                }
                                (argv, None)
                            }
                        },
                        (private, None) => (private, None),
                    }
                }
                None => (argv, None),
            };
            let Some((program, args)) = argv.split_first() else { return MuxOutput::default() };
            let simultaneous = RUNNING.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let started = std::time::Instant::now();
            let mut command = client_command(program);
            command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
            #[cfg(windows)]
            if creation_flags(args) != 0 { command.creation_flags(creation_flags(args)); }
            let (out, failure) = match tokio::time::timeout(self.timeout, command.output()).await {
                Ok(Ok(out)) => (MuxOutput { code: out.status.code(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned() }, None),
                Ok(Err(error)) => (MuxOutput { code: None, stdout: String::new(), stderr: error.to_string() }, Some(("OSError", error.raw_os_error()))),
                Err(_) => (MuxOutput { code: None, stdout: String::new(), stderr: "o multiplexador não respondeu no prazo".into() }, Some(("TimeoutExpired", None))),
            };
            RUNNING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            // Recusado, o pane não nasceu e ninguém vai ler o arquivo; sem resposta, ele pode ter nascido.
            #[cfg(unix)]
            if let Some(path) = &secrets && out.code.is_some_and(|code| code != 0) { let _ = std::fs::remove_file(path); }
            if self.diary.is_some()
                && let Some(line) = command_line(&argv, &out, failure, started.elapsed(), self.timeout, simultaneous) {
                let _ = crate::diag::record(self.diary.clone(), vec![line], "multiplexador").await;
            }
            out
        })
    }
}

/// O `new-session` sai do Job do servidor (`CREATE_BREAKAWAY_FROM_JOB`): o psmux sobe dele o servidor
/// da sessão, e dentro do Job a sessão morreria quando o backend reinicia.
#[cfg_attr(not(windows), allow(dead_code))]
fn creation_flags(args: &[String]) -> u32 {
    if args.first().is_some_and(|cmd| cmd == "new-session") { 0x0100_0000 } else { 0 }
}

static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Os subcomandos que o diário nomeia; o resto vira "outro" (nunca o argv, que pode levar texto da sessão).
const DIARY_COMMANDS: [&str; 19] = ["list-sessions", "list-panes", "has-session", "show-options", "display-message", "display",
    "new-session", "new-window", "set-option", "kill-session", "rename-session", "send-keys", "capture-pane", "set-buffer",
    "show-buffer", "delete-buffer", "load-buffer", "paste-buffer", "resize-window"];
/// Consultas cujo "não existe" (rc 1) é resposta, não falha.
const ABSENCE_OK: [&str; 4] = ["list-sessions", "list-panes", "has-session", "show-options"];

/// A linha `mux.comando` do `tmux._run` do Python: comando falho (fora a ausência esperada), sem
/// resposta ou de 1 s ou mais. Nem argv nem stderr.
fn command_line(argv: &[String], out: &MuxOutput, failure: Option<(&str, Option<i32>)>, elapsed: Duration, limit: Duration,
                simultaneous: usize) -> Option<serde_json::Value> {
    let mux = Path::new(&argv[0]).file_stem().is_some_and(|s| s == "tmux" || s == "psmux");
    let command = if argv[0] == "systemd-run" { "systemd-scope" }
        else { argv.get(1).filter(|c| mux && DIARY_COMMANDS.contains(&c.as_str())).map_or("outro", String::as_str) };
    let failed = out.code.is_some_and(|code| code != 0);
    let stderr = out.stderr.as_str();
    let absent = ["no server", "no sessions", "can't find session", "no such session", "invalid option: @cp_hidden"].iter().any(|p| stderr.starts_with(p))
        || (stderr.starts_with("error connecting to ") && stderr.trim_end().ends_with("(No such file or directory)"));
    let expected = failure.is_none() && out.code == Some(1) && ABSENCE_OK.contains(&command) && absent;
    let ms = elapsed.as_millis() as u64;
    if failure.is_none() && !(failed && !expected) && ms < 1000 { return None; }
    let mut line = serde_json::json!({"evento": "mux.comando", "nivel": if failed && !expected { "erro" } else { "aviso" }, "comando": command,
        "ms": ms, "limite_ms": limit.as_millis() as u64, "simultaneos": simultaneous, "retorno": out.code,
        "erro_tipo": failure.map_or(if failed { "retorno" } else { "" }, |(kind, _)| kind), "errno": failure.and_then(|(_, errno)| errno)});
    #[cfg(target_os = "linux")]
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        let kb = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.trim().split_whitespace().next()?.parse::<u64>().ok());
        if let (Some(total), Some(free)) = (kb("MemTotal:"), kb("MemAvailable:")) {
            line["memoria_total_mb"] = (total / 1024).into();
            line["memoria_disponivel_mb"] = (free / 1024).into();
        }
    }
    Some(line)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::Mutex;

    /// Multiplexador falso: responde na ordem as saídas dadas, cada uma depois de `delay`, e guarda cada argv.
    #[derive(Default)]
    pub(crate) struct FakeMux { pub scope: Vec<String>, pub replies: Mutex<Vec<MuxOutput>>, pub seen: Mutex<Vec<Vec<String>>>, pub delay: Duration,
        pub notes: Mutex<Vec<Value>> }

    impl FakeMux {
        pub(crate) fn answering(replies: Vec<MuxOutput>) -> Self { Self { replies: Mutex::new(replies), ..Default::default() } }
        pub(crate) fn runs(&self) -> Vec<Vec<String>> { self.seen.lock().unwrap().clone() }
        /// Só os `new-session`: a prova de vida acrescenta consultas.
        pub(crate) fn creations(&self) -> Vec<Vec<String>> { self.runs().into_iter().filter(|run| run.iter().any(|a| a == "new-session")).collect() }
    }

    /// No dash (Ubuntu, Debian) o `sh -c` fica de pai do comando: sem `exec` a restauração via o shell, não o
    /// agente. O envelope falso entrega o `sh -c` a um dash falso, que só sai do caminho com `exec`.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_read_only_agent_replaces_the_shell_inside_the_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let (dash, bwrap) = (dir.path().join("dash"), dir.path().join("bwrap"));
        crate::write_test_executable(&dash, "#!/bin/bash\nshift\neval \"$1\"\n");
        crate::write_test_executable(&bwrap, format!("#!/bin/bash\nshift\nexec '{}' \"$@\"\n", dash.display()));
        let argv = sandboxed(&[bwrap.to_string_lossy().into_owned()], &["sleep".into(), "30".into()], false);
        let mut child = std::process::Command::new(&argv[0]).args(&argv[1..]).spawn().unwrap();
        let comm = || std::fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default().trim().to_owned();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while comm() != "sleep" && std::time::Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        let seen = comm();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(seen, "sleep", "o processo da sessão é o agente, não o shell");
    }

    impl Mux for FakeMux {
        fn scope(&self) -> BoxFuture<'_, Vec<String>> { Box::pin(async { self.scope.clone() }) }
        fn run(&self, argv: Vec<String>) -> BoxFuture<'_, MuxOutput> {
            self.seen.lock().unwrap().push(argv);
            let mut replies = self.replies.lock().unwrap();
            let reply = if replies.is_empty() { ok() } else { replies.remove(0) };
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                reply
            })
        }
        fn diary(&self, line: Value) { self.notes.lock().unwrap().push(line); }
    }

    pub(crate) fn ok() -> MuxOutput { MuxOutput { code: Some(0), ..Default::default() } }
    pub(crate) fn failed(stderr: &str) -> MuxOutput { MuxOutput { code: Some(1), stdout: String::new(), stderr: stderr.into() } }

    pub(crate) fn machine_at(default_config_dir: &str) -> TerminalEnv { TerminalEnv { default_config_dir: default_config_dir.into(), ..machine() } }

    pub(crate) fn machine() -> TerminalEnv {
        TerminalEnv { windows: false, program: "tmux".into(), display: vec![("WAYLAND_DISPLAY".into(), "wayland-1".into())], server_config_dir: None,
            default_config_dir: "/r/home/.claude".into(), birth: Duration::from_millis(500) }
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/session-open").join(format!("{name}.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// O argv que o Python passou ao `new-session` no cenário e o `-e` da sessão (depois de `CP_PROVIDER`
    /// e do `WAYLAND_DISPLAY`, antes da conta).
    fn python_argv(name: &str) -> (Vec<String>, Vec<(String, String)>) {
        let fixture = fixture(name);
        let tmux = fixture["result"]["subprocesses"].as_array().unwrap().iter().find(|p| p["prog"] == "tmux").unwrap();
        let concrete = |a: &str| a.replace("<ROOT>", "/r").replace("<REPO>", "/repo").replace("<uuid:1>", "11111111-1111-4111-8111-111111111111");
        let argv: Vec<String> = tmux["argv"].as_array().unwrap().iter().map(|a| concrete(a.as_str().unwrap())).collect();
        let env: Vec<(String, String)> = argv.windows(2).filter(|w| w[0] == "-e").filter_map(|w| w[1].split_once('='))
            .filter(|(k, _)| !["COLORTERM", "CLAUDE_CODE_TMUX_TRUECOLOR", "KIMI_CODE_TUI_FULL_SCREEN", "CP_SESSION_NAME",
                "CP_PROVIDER", "WAYLAND_DISPLAY", "CLAUDE_CONFIG_DIR"].contains(k))
            .map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        (argv, env)
    }

    fn claude(env: Vec<(String, String)>, config_dir: Option<&str>) -> PaneSpec {
        PaneSpec { name: "s1".into(), cwd: "/r/work/proj".into(), provider: AgentProvider::Claude, key: "k".repeat(32), env,
            config_dir: config_dir.map(str::to_owned),
            command: ["claude", "--session-id", "11111111-1111-4111-8111-111111111111", "--plugin-dir", "/repo/plugins/hangar", "--plugin-dir",
                "/repo/plugins/github-actions", "--permission-mode", "bypassPermissions"].map(str::to_owned).into() }
    }

    /// O mesmo argv do Python, mais o `CP_SESSION_KEY` (matriz, item 2) e o `/bin/sh -c` que tira o
    /// `default-shell` do caminho: conta padrão com `env -u`, conta escolhida por `-e`.
    #[test]
    fn the_new_session_argv_is_the_python_one_plus_the_session_key() {
        for (fixture, config_dir) in [("create_claude_terminal_default", None), ("create_claude_terminal_account_b", Some("/r/home/.claude-b"))] {
            let (mut python, env) = python_argv(fixture);
            let at = python.iter().position(|a| a == "CP_PROVIDER=claude").unwrap() + 1;
            python.splice(at..at, ["-e".to_owned(), format!("CP_SESSION_KEY={}", "k".repeat(32))]);
            python.insert(0, "tmux".into());
            let last = python.len() - 1;
            python.splice(last..last, ["/bin/sh".to_owned(), "-c".to_owned()]);
            let mut ours = new_session_argv(&machine(), &claude(env, config_dir));
            let command = ours.pop().unwrap();
            ours.push(undo_not_inherited(&command));
            assert_eq!(ours, python, "{fixture}");
        }
    }

    /// O comando sem o `env -u` das variáveis não herdadas, como o Python o montava.
    pub(crate) fn undo_not_inherited(command: &str) -> String {
        let words: Vec<&str> = command.split(' ').collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < words.len() {
            if words[i] == "-u" && words.get(i + 1).is_some_and(|var| NOT_INHERITED.contains(var) || SERVER_SECRETS.contains(var)) {
                i += 2;
                continue;
            }
            out.push(words[i]);
            i += 1;
        }
        let out = out.join(" ");
        out.strip_prefix("exec env ").filter(|rest| !rest.starts_with("-u ")).map_or(out.clone(), |rest| format!("exec {rest}"))
    }

    /// O comando do agente não passa pelo shell do usuário: o `config.fish` que trava (keyring preso)
    /// seguraria a sessão sem prazo.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_agent_command_does_not_go_through_the_user_default_shell() {
        let dir = tempfile::tempdir().unwrap();
        let (marker, ran) = (dir.path().join("shell-do-usuario"), dir.path().join("rodou"));
        let shell = dir.path().join("fakeshell");
        crate::write_test_executable(&shell, format!("#!/bin/sh\ntouch '{}'\nexec /bin/sh \"$@\"\n", marker.display()));
        let conf = dir.path().join("tmux.conf");
        std::fs::write(&conf, format!("set -g default-shell '{}'\n", shell.display())).unwrap();
        let program = dir.path().join("tmux");
        let socket = format!("hangar-shell-{}", std::process::id());
        crate::write_test_executable(&program, format!("#!/bin/sh\nexec tmux -L {socket} -f '{}' \"$@\"\n", conf.display()));
        let env = TerminalEnv { program: program.to_string_lossy().into_owned(), ..machine() };
        let mut pane = claude(Vec::new(), None);
        pane.cwd = dir.path().to_string_lossy().into_owned();
        pane.command = vec!["touch".into(), ran.to_string_lossy().into_owned()];
        let argv = new_session_argv(&env, &pane);
        let status = std::process::Command::new(&argv[0]).args(&argv[1..]).status().unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while !ran.exists() && std::time::Instant::now() < end { std::thread::sleep(Duration::from_millis(50)); }
        let _ = std::process::Command::new(&program).arg("kill-server").status();
        assert!(status.success() && ran.exists(), "o comando rodou: {argv:?}");
        assert!(!marker.exists(), "o default-shell do usuário não entra: {argv:?}");
    }

    /// O pane nasce com as variáveis de tela do servidor e as que faltam nele (o Computer Use recusa
    /// sem `HYPRLAND_INSTANCE_SIGNATURE`).
    #[test]
    fn the_pane_is_born_with_every_display_variable_the_server_has_or_found() {
        let run = tempfile::tempdir().unwrap();
        std::fs::write(run.path().join("wayland-1"), "").unwrap();
        std::fs::create_dir_all(run.path().join("hypr").join("inst_1")).unwrap();
        let dir = run.path().to_string_lossy().into_owned();
        let current = |key: &str| match key { "DISPLAY" => Some(":0".to_owned()), "XDG_RUNTIME_DIR" => Some(dir.clone()), _ => None };
        let display = crate::display_env::complete(current, "HYPRLAND_INSTANCE_SIGNATURE=inst_1\n", Some(run.path()));
        let pairs = [("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":0"), ("HYPRLAND_INSTANCE_SIGNATURE", "inst_1"), ("XDG_RUNTIME_DIR", dir.as_str())];
        assert_eq!(display, pairs.map(|(k, v)| (k.to_owned(), v.to_owned())));
        let argv = new_session_argv(&TerminalEnv { display, ..machine() }, &claude(Vec::new(), None));
        for (key, value) in pairs {
            assert!(argv.windows(2).any(|w| w[0] == "-e" && w[1] == format!("{key}={value}")), "{key}: {argv:?}");
        }
    }

    /// Servidor com `CLAUDE_CONFIG_DIR` próprio exporta sempre, até a padrão; no psmux a padrão é a
    /// ausência sem `env -u` (o pane herda de quem chama) e o comando vai sem `exec`, citado à moda dele.
    #[test]
    fn the_account_and_the_dialect_follow_the_multiplexer() {
        let mut env = machine();
        env.server_config_dir = Some("/r/home/.claude".into());
        let argv = new_session_argv(&env, &claude(Vec::new(), None));
        assert!(argv.contains(&"CLAUDE_CONFIG_DIR=/r/home/.claude".to_owned()));
        assert!(undo_not_inherited(argv.last().unwrap()).starts_with("exec claude "), "{argv:?}");
        let windows = TerminalEnv { windows: true, display: Vec::new(), default_config_dir: "C:\\Users\\j\\.claude".into(), ..machine() };
        let mut pane = claude(Vec::new(), Some("C:/Users/j/.claude/"));
        pane.command = vec!["claude".into(), "--settings".into(), "{\"a\":\"it's\"}".into()];
        let argv = new_session_argv(&windows, &pane);
        assert!(!argv.iter().any(|a| a.starts_with("CLAUDE_CONFIG_DIR=")), "a padrão escrita de outro jeito: {argv:?}");
        assert_eq!(argv.last().unwrap(), "claude --settings '{\"a\":\"it''s\"}'");
    }

    /// A chave secreta sai do argv (o systemd grava a linha de comando do escopo no journal) e vai a um
    /// arquivo que o shell do pane lê e apaga antes do agente; o escopo ganha descrição própria.
    #[cfg(unix)]
    #[test]
    fn secrets_leave_the_command_line_for_a_file_the_pane_reads_and_deletes() {
        let pane = claude(vec![("TYPESAFE_API_KEY".into(), "sk-a'b".into()), ("HANGAR_PLUGIN_TOKEN".into(), "k.t".into()),
            ("HANGAR_JEV".into(), "on".into())], None);
        let mut argv: Vec<String> = ["/usr/bin/systemd-run", "--user", "--scope", "--collect", "-q", "--"].map(str::to_owned).into();
        argv.extend(new_session_argv(&machine(), &pane));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s1.env");
        let (private, contents) = private_launch(argv, &file);
        let joined = private.join(" ");
        assert!(!joined.contains("sk-a") && !joined.contains("k.t"), "{joined}");
        assert!(private.windows(2).any(|w| w[0] == "-e" && w[1] == "HANGAR_JEV=on"), "{joined}");
        assert!(private.windows(2).any(|w| w[0] == "-e" && w[1] == format!("CP_SESSION_KEY={}", "k".repeat(32))), "{joined}");
        assert_eq!(private[..6].join(" "), "/usr/bin/systemd-run --user --scope --collect -q --description=hangar:s1");
        std::fs::write(&file, contents.unwrap()).unwrap();
        // O comando do pane, com `printenv` no lugar do agente.
        let shell = private.last().unwrap().rsplit_once(" exec env ").map(|(head, _)| format!("{head} printenv TYPESAFE_API_KEY HANGAR_PLUGIN_TOKEN")).unwrap();
        let out = std::process::Command::new("/bin/sh").args(["-c", &shell]).env_clear().output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "sk-a'b\nk.t\n", "{shell}");
        assert!(!file.exists());
    }

    /// Prova na máquina: tmux isolado (`-L`) e escopo de verdade do systemd. O agente recebe a chave,
    /// o arquivo some e o journal só tem a descrição. `cargo test … -- --ignored real_scope`.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "usa o systemd e o journal do usuário"]
    async fn real_scope_keeps_the_key_out_of_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let socket = format!("kr-{}", crate::mods::state::random_hex(4));
        let tmux = dir.path().join("tmux");
        std::fs::write(&tmux, format!("#!/bin/sh\nexec tmux -L {socket} -f /dev/null \"$@\"\n")).unwrap();
        std::fs::set_permissions(&tmux, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let key = format!("sk-{}", crate::mods::state::random_hex(8));
        let out = dir.path().join("out");
        let env = TerminalEnv { program: tmux.to_string_lossy().into_owned(), display: Vec::new(), ..machine() };
        let pane = PaneSpec { name: socket.clone(), cwd: dir.path().to_string_lossy().into_owned(), provider: AgentProvider::Claude,
            key: "k".repeat(32), env: vec![("TYPESAFE_API_KEY".into(), key.clone())], config_dir: None,
            command: vec!["sh".into(), "-c".into(), format!("printenv TYPESAFE_API_KEY > {}; sleep 30", out.display())] };
        let mux = ProcessMux::default();
        let spawned = spawn(&mux, &env, &pane).await;
        let seen = (0..50).find_map(|_| { std::thread::sleep(Duration::from_millis(100)); std::fs::read_to_string(&out).ok().filter(|s| !s.is_empty()) });
        let _ = std::process::Command::new(&tmux).arg("kill-server").status();
        spawned.unwrap();
        assert_eq!(seen.as_deref(), Some(format!("{key}\n").as_str()));
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap();
        let left: Vec<_> = std::fs::read_dir(Path::new(&runtime).join("hangar/env")).unwrap().flatten()
            .filter(|e| std::fs::read_to_string(e.path()).is_ok_and(|s| s.contains(&key))).collect();
        assert!(left.is_empty(), "arquivo ficou: {left:?}");
        std::thread::sleep(Duration::from_secs(1));
        let journal = std::process::Command::new("journalctl").args(["--user", "--since", "-2min", "--no-pager"]).output().unwrap();
        let journal = String::from_utf8_lossy(&journal.stdout);
        assert!(journal.contains(&format!("Started hangar:{socket}")), "sem a descrição no journal");
        assert!(!journal.contains(&key), "a chave foi ao journal");
    }

    /// Subida que não respondeu deixa o arquivo para trás; a próxima gravação o apaga.
    #[cfg(unix)]
    #[test]
    fn an_old_secrets_file_is_swept_by_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.env");
        std::fs::write(&old, "export K='v'\n").unwrap();
        std::fs::File::options().write(true).open(&old).unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
        let fresh = dir.path().join("fresh.env");
        write_private(&fresh, "export K='w'\n").unwrap();
        assert!(!old.exists());
        assert!(fresh.exists());
    }

    /// Sem segredo, sem arquivo; sem escopo, nada de descrição.
    #[cfg(unix)]
    #[test]
    fn a_launch_without_secrets_stays_as_it_was() {
        let argv = new_session_argv(&machine(), &claude(vec![("HANGAR_JEV".into(), "on".into())], None));
        let (private, contents) = private_launch(argv.clone(), Path::new("/nope"));
        assert_eq!((private, contents), (argv, None));
    }

    /// Nada que o servidor tmux herdou de outra sessão (subagente, Jev, ponte, portão de hooks,
    /// motor, cano) nem segredo do servidor chega ao pane; o que a sessão põe por `-e` fica.
    #[test]
    fn the_pane_does_not_inherit_what_belongs_to_another_session() {
        let pane = claude(vec![("HANGAR_JEV".into(), "on".into()), ("TYPESAFE_API_KEY".into(), "k".into()),
            ("CLAUDE_CODE_SUBAGENT_MODEL".into(), "sonnet".into())], None);
        let command = new_session_argv(&machine(), &pane).pop().unwrap();
        let unset: Vec<&str> = command.split(" -u ").skip(1).map(|rest| rest.split(' ').next().unwrap()).collect();
        for var in ["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_ENABLE_FUNCTION_HOOKS", "HANGAR_PLUGIN_URL", "HANGAR_PLUGIN_TOKEN",
            "JEV_ENDPOINT", "JEV_MODEL", "JEV_AUX_API_KEY", "JEV_TEXTO_CMD", "CP_ENGINE", "CP_ENGINE_SERVICE_TIER",
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS", "HANGAR_CANO_KEY", "CP_PI_SESSION", "CP_AUTH_TOKEN", "HANGAR_INTERNAL_SECRET"] {
            assert!(unset.contains(&var), "{var} não sai: {command}");
        }
        for var in ["HANGAR_JEV", "TYPESAFE_API_KEY", "CLAUDE_CODE_SUBAGENT_MODEL", "CP_SESSION_KEY"] {
            assert!(!unset.contains(&var), "{var} é da sessão: {command}");
        }
        assert!(command.starts_with("exec env -u CLAUDE_CONFIG_DIR -u "), "{command}");
        // No psmux o pane herda de quem chama: o cliente sai sem eles.
        let client = client_command("tmux");
        let removed: Vec<String> = client.as_std().get_envs().filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        for var in ["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_SUBAGENT_MODEL", "HANGAR_PLUGIN_TOKEN", "TYPESAFE_API_KEY", "CP_AUTH_TOKEN"] {
            assert!(removed.iter().any(|r| r == var), "{var} fica no cliente: {removed:?}");
        }
    }

    /// O que o `hangar-engine` exporta (`engines.py`) é da sessão de motor que o pôs; o pane novo
    /// não herda de um servidor tmux subido de dentro dela.
    #[test]
    fn the_pane_does_not_inherit_what_an_engine_exports() {
        let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/app/engines.py")).unwrap();
        let exported: std::collections::BTreeSet<&str> = regex::Regex::new(r#""((?:ANTHROPIC|CLAUDE_CODE|CP_ENGINE|ENABLE|DISABLE)_[A-Z0-9_]+|CP_ENGINE)""#)
            .unwrap().captures_iter(&source).map(|c| c.get(1).unwrap().as_str()).collect();
        assert!(exported.len() >= 20, "o padrão não achou as variáveis do motor: {exported:?}");
        let missing: Vec<&&str> = exported.iter().filter(|var| !NOT_INHERITED.contains(var)).collect();
        assert!(missing.is_empty(), "o pane herdaria do motor: {missing:?}");
    }

    /// Só o `new-session` sai do Job do servidor: o psmux sobe o servidor da sessão a partir dele, e
    /// dentro do Job a sessão morreria no reinício do backend.
    #[test]
    fn only_new_session_breaks_away_from_the_server_job() {
        let args = |cmd: &str| vec![cmd.to_owned(), "-d".to_owned()];
        assert_eq!(creation_flags(&args("new-session")), 0x0100_0000);
        for cmd in ["has-session", "send-keys", "kill-session", "list-sessions"] {
            assert_eq!(creation_flags(&args(cmd)), 0, "{cmd}");
        }
    }

    #[tokio::test]
    async fn the_scope_wraps_the_command_and_a_refused_scope_runs_once_without_it() {
        let scope: Vec<String> = ["systemd-run", "--user", "--scope", "--collect", "-q", "--"].map(str::to_owned).into();
        let mux = FakeMux { scope: scope.clone(), ..FakeMux::answering(vec![failed("Failed to start transient scope unit: Access denied\n")]) };
        spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap();
        let runs = mux.creations();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0][..6], scope[..]);
        assert_eq!(runs[1][0], "tmux", "a segunda vez sem o escopo");
        // O escopo perdido vai ao diário, como o `mux.scope_perdido` do Python.
        assert_eq!(*mux.notes.lock().unwrap(), [serde_json::json!({"evento": "mux.scope_perdido", "nivel": "aviso", "retorno": 1, "comando": "new-session"})]);
    }

    /// Comando do multiplexador que falha (fora a ausência esperada) ou passa de 1 s vai ao
    /// diário como `mux.comando`, sem argv nem stderr.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_mux_command_goes_to_the_diary() {
        let dir = tempfile::tempdir().unwrap();
        let tmux = dir.path().join("tmux");
        crate::write_test_executable(&tmux, "#!/bin/sh\ncase \"$1\" in new-session) echo segredo >&2; exit 1;; has-session) echo \"can't find session: x\" >&2; exit 1;; esac\n");
        let diary = dir.path().join("diario");
        let mux = ProcessMux { diary: Some(diary.clone()), ..ProcessMux::default() };
        let tmux = tmux.to_string_lossy().into_owned();
        mux.run(vec![tmux.clone(), "has-session".into(), "-t".into(), "=x".into()]).await;
        mux.run(vec![tmux, "new-session".into(), "-d".into(), "-s".into(), "s1".into()]).await;
        let text: String = std::fs::read_dir(&diary).unwrap().flatten().map(|f| std::fs::read_to_string(f.path()).unwrap()).collect();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 1, "a ausência esperada do has-session não entra: {text}");
        assert_eq!((&lines[0]["evento"], &lines[0]["nivel"], &lines[0]["comando"], &lines[0]["retorno"], &lines[0]["erro_tipo"], &lines[0]["limite_ms"]),
            (&serde_json::json!("mux.comando"), &serde_json::json!("erro"), &serde_json::json!("new-session"), &serde_json::json!(1), &serde_json::json!("retorno"), &serde_json::json!(5000)));
        assert!(!text.contains("segredo") && !text.contains("s1"), "{text}");
    }

    /// A recusa passageira do systemd a escopos criados juntos se repete, com espera, até três
    /// vezes, sempre com o escopo; esgotada, é erro com o código dela.
    #[tokio::test(start_paused = true)]
    async fn the_transient_scope_refusal_is_retried_with_the_scope() {
        let scope: Vec<String> = ["systemd-run", "--user", "--scope", "--collect", "-q", "--"].map(str::to_owned).into();
        let missing = || failed("Failed to start transient scope unit: Unit run-p4242.scope not found.\n");
        let mux = FakeMux { scope: scope.clone(), ..FakeMux::answering(vec![missing(), missing()]) };
        spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap();
        assert_eq!(mux.creations().len(), 3);
        assert!(mux.creations().iter().all(|run| run[..6] == scope[..]), "todas com o escopo");
        let mux = FakeMux { scope: scope.clone(), ..FakeMux::answering(vec![missing(), missing(), missing(), missing()]) };
        let error = spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap_err();
        assert_eq!((error.code, mux.creations().len()), ("systemd_scope_unit_missing", 4));
        assert_eq!(error.stderr, "Failed to start transient scope unit: Unit run-p4242.scope not found.");
    }

    /// Sem resposta no prazo (ou o psmux com código diferente de zero), a sessão pode ter nascido;
    /// ela só vale se for esta (a chave dela), e uma de mesmo nome que não é esta é recusa.
    #[tokio::test]
    async fn an_unanswered_new_session_counts_only_when_the_session_is_this_one() {
        let key = format!("CP_SESSION_KEY={}\n", "k".repeat(32));
        let timeout = MuxOutput { code: None, stdout: String::new(), stderr: "o multiplexador não respondeu no prazo".into() };
        let answered = |stdout: &str| MuxOutput { code: Some(0), stdout: stdout.into(), stderr: String::new() };
        let mux = FakeMux::answering(vec![timeout.clone(), ok(), answered(&key)]);
        spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap();
        assert_eq!(mux.runs()[1], ["tmux", "has-session", "-t", "=s1"]);
        assert_eq!(mux.runs()[2], ["tmux", "show-environment", "-t", "=s1", "CP_SESSION_KEY"]);
        let other = FakeMux::answering(vec![failed("duplicate session: s1\n"), ok(), answered("CP_SESSION_KEY=outra\n")]);
        assert_eq!(spawn(&other, &machine(), &claude(Vec::new(), None)).await.unwrap_err().stderr, "duplicate session: s1");
        let gone = FakeMux::answering(vec![timeout, failed("can't find session: s1\n")]);
        assert_eq!(spawn(&gone, &machine(), &claude(Vec::new(), None)).await.unwrap_err().code, "terminal_nao_criado");
    }

    /// O `new-session` devolve 0 com o comando morto ao nascer (binário ausente, flag que o CLI
    /// recusa); o pane é conferido até o fim da janela de nascimento e a morte é erro com código.
    #[tokio::test(start_paused = true)]
    async fn a_pane_that_dies_at_birth_is_an_error() {
        let mux = FakeMux::answering(vec![ok(), ok(), failed("can't find session: s1\n")]);
        let error = spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap_err();
        assert_eq!(error, SpawnError { code: "terminal_morreu_ao_nascer", stderr: "can't find session: s1".into() });
        let alive = FakeMux::answering(Vec::new());
        spawn(&alive, &machine(), &claude(Vec::new(), None)).await.unwrap();
        let polls = alive.runs().iter().filter(|run| run[1] == "has-session").count();
        assert_eq!(polls, 5, "conferido a cada 100 ms durante a janela");
    }

    /// A janela de nascimento é de relógio: um multiplexador lento não a estica pela soma das consultas.
    #[tokio::test(start_paused = true)]
    async fn the_birth_window_is_wall_clock_not_the_sum_of_sleeps() {
        let slow = FakeMux { delay: Duration::from_millis(300), ..FakeMux::default() };
        let start = tokio::time::Instant::now();
        spawn(&slow, &machine(), &claude(Vec::new(), None)).await.unwrap();
        assert!(start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
    }

    /// Com o pid do pane, a morte do comando é sinal: chega na hora, sem esperar a janela; vivo, a
    /// janela termina no prazo.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_pane_process_exit_is_a_signal_not_a_poll() {
        let pid_of = |child: &std::process::Child| MuxOutput { code: Some(0), stdout: format!("{}\n", child.id()), stderr: String::new() };
        let long = TerminalEnv { birth: Duration::from_secs(5), ..machine() };
        let mut dying = std::process::Command::new("sh").args(["-c", "sleep 0.1"]).spawn().unwrap();
        let mux = FakeMux::answering(vec![ok(), pid_of(&dying)]);
        let start = std::time::Instant::now();
        let error = spawn(&mux, &long, &claude(Vec::new(), None)).await.unwrap_err();
        assert_eq!(error.code, "terminal_morreu_ao_nascer");
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
        dying.wait().unwrap();
        let mut living = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let short = TerminalEnv { birth: Duration::from_millis(300), ..machine() };
        let start = std::time::Instant::now();
        spawn(&FakeMux::answering(vec![ok(), pid_of(&living)]), &short, &claude(Vec::new(), None)).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(300) && start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
        living.kill().unwrap();
        living.wait().unwrap();
    }

    /// Todo pane nasce com o provedor dele em `CP_PROVIDER` (o `/resume` do Python o deixava
    /// vazio); o tipo não tem o vazio, e o `-e` vem antes de qualquer outro da sessão.
    #[test]
    fn every_pane_carries_its_provider() {
        for provider in AgentProvider::ALL {
            let argv = new_session_argv(&machine(), &PaneSpec { provider, ..claude(Vec::new(), None) });
            let at = argv.iter().position(|a| a == &format!("CP_PROVIDER={}", provider.name())).unwrap();
            assert_eq!((argv[at - 1].as_str(), argv[at - 2].as_str()), ("-e", "CP_SESSION_NAME=s1"));
        }
    }

    /// Sem resposta do multiplexador não é "não existe": a sessão pode estar de pé, e a falha volta
    /// incerta (quem chama não desfaz a worktree debaixo dela).
    #[tokio::test(start_paused = true)]
    async fn a_silent_multiplexer_is_uncertain_not_absent() {
        let silent = || MuxOutput { code: None, stdout: String::new(), stderr: "o multiplexador não respondeu no prazo".into() };
        let mux = FakeMux::answering(vec![silent(), silent()]);
        assert_eq!(spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap_err().code, UNCERTAIN);
        let mux = FakeMux::answering(std::iter::once(ok()).chain(std::iter::repeat_n(silent(), 7)).collect());
        assert_eq!(spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap_err().code, UNCERTAIN, "a janela inteira sem resposta");
        let mux = FakeMux::answering(vec![ok(), ok(), silent(), ok()]);
        spawn(&mux, &machine(), &claude(Vec::new(), None)).await.expect("uma resposta perdida com o pane de pé no fim é nascimento");
    }

    /// A sondagem do escopo (o `systemd-run` e a espera dele) roda fora do runtime: as outras tarefas
    /// seguem enquanto ela demora.
    #[tokio::test(flavor = "current_thread")]
    async fn the_scope_probe_does_not_hold_the_runtime() {
        let mux = ProcessMux { probe: || { std::thread::sleep(Duration::from_millis(300)); Vec::new() }, ..ProcessMux::default() };
        let env = TerminalEnv { program: "false".into(), ..machine() };
        let start = std::time::Instant::now();
        let ticker = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            start.elapsed()
        });
        let _ = spawn(&mux, &env, &claude(Vec::new(), None)).await;
        let ticked = ticker.await.unwrap();
        assert!(ticked < Duration::from_millis(250), "a outra tarefa esperou a sondagem: {ticked:?}");
    }

    #[tokio::test]
    async fn a_refusal_is_an_error_with_the_stderr_tail() {
        let mux = FakeMux::answering(vec![failed("aviso\nduplicate session: s1\n\n")]);
        let error = spawn(&mux, &machine(), &claude(Vec::new(), None)).await.unwrap_err();
        assert_eq!(error, SpawnError { code: "terminal_nao_criado", stderr: "duplicate session: s1".into() });
    }
}
