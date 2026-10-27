//! O executor destacado: `hangar-server update --run|--restart|--dev`. Roda num processo só dele, com
//! runtime próprio, fora do serviço que ele reinicia: Linux num escopo transiente do systemd e em
//! sessão própria (o `systemctl --user restart` mata o cgroup inteiro da unit); Windows fora do Job
//! do lançador (`CREATE_BREAKAWAY_FROM_JOB`), em grupo próprio e sem janela. O binário é o do
//! servidor que o lançou, pelo caminho real da versão: a troca do `current` não o tira de baixo.
//!
//!   hangar-server update --run [--channel <canal>] [--listen <ip:porta>]
//!   hangar-server update --restart --request-id <id> [--listen <ip:porta>]
//!   hangar-server update --dev [--listen <ip:porta>]
//!   hangar-server update --dev-mode-on <checkout> | --dev-mode-off [--listen <ip:porta>]
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;

use super::dev_mode::Request as DevRequest;
use super::restart::{SystemRestarter, Topology};
use super::{HealthProbe, Layout, Outcome, Reading, Release, Restarter, Settings, StateFile, UpdateLock};
use crate::accounts::environment::AccountEnvironment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// Pacote: índice, download, passos, troca, reinício e prova.
    Update { channel: Option<String> },
    /// Só reinicia; `request_id` liga o desfecho ao pedido da tela.
    Restart { request_id: String },
    /// Modo desenvolvedor: `git pull --ff-only` no checkout e reinício.
    Dev,
    /// Liga ou desliga o modo desenvolvedor, como o `hangar-server dev-mode`.
    DevMode(DevRequest),
    /// A migração para o pacote depois do aviso aceito na tela, como o `hangar-server migrate-package`.
    Migrate { checkout: Option<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub task: Task,
    /// O endereço público do servidor (`0.0.0.0` vira loopback na prova de vida).
    pub listen: SocketAddr,
}

const DEFAULT_LISTEN: &str = "127.0.0.1:8765";

impl Args {
    /// `args` sem o nome do programa nem o `update`.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let (mut task, mut channel, mut request_id, mut listen) = (None, None, None, DEFAULT_LISTEN.parse().expect("endereço padrão"));
        let mut checkout = None;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} sem valor"));
            match arg.as_str() {
                "--run" | "--restart" | "--dev" | "--dev-mode-off" | "--migrate" if task.is_none() => task = Some(arg.clone()),
                "--checkout" => checkout = Some(value("--checkout")?),
                "--dev-mode-on" if task.is_none() => { task = Some(arg.clone()); checkout = Some(value("--dev-mode-on")?); }
                "--channel" => channel = Some(value("--channel")?),
                "--request-id" => request_id = Some(value("--request-id")?),
                "--listen" => listen = value("--listen")?.parse().map_err(|e| format!("--listen inválido: {e}"))?,
                other => return Err(format!("argumento inesperado: {other}")),
            }
        }
        let task = match task.as_deref() {
            Some("--run") => Task::Update { channel },
            Some("--restart") => Task::Restart { request_id: request_id.ok_or("--restart pede --request-id")? },
            Some("--dev") => Task::Dev,
            Some("--dev-mode-on") => Task::DevMode(DevRequest::On(checkout.unwrap_or_default().into())),
            Some("--dev-mode-off") => Task::DevMode(DevRequest::Off),
            Some("--migrate") => Task::Migrate { checkout: checkout.map(PathBuf::from) },
            _ => return Err("falta --run, --restart, --dev, --dev-mode-on, --dev-mode-off ou --migrate".into()),
        };
        Ok(Args { task, listen })
    }

    /// Os argumentos do executor, a partir do `update`.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = vec!["update".to_owned()];
        match &self.task {
            Task::Update { channel } => {
                argv.push("--run".into());
                if let Some(channel) = channel { argv.extend(["--channel".into(), channel.clone()]); }
            }
            Task::Restart { request_id } => argv.extend(["--restart".into(), "--request-id".into(), request_id.clone()]),
            Task::Dev => argv.push("--dev".into()),
            Task::DevMode(DevRequest::On(dir)) => argv.extend(["--dev-mode-on".into(), dir.display().to_string()]),
            Task::DevMode(DevRequest::Off) => argv.push("--dev-mode-off".into()),
            Task::Migrate { checkout } => {
                argv.push("--migrate".into());
                if let Some(dir) = checkout { argv.extend(["--checkout".into(), dir.display().to_string()]); }
            }
        }
        argv.extend(["--listen".into(), self.listen.to_string()]);
        argv
    }
}

/// O que a rodada precisa saber, tirado do ambiente do processo.
#[derive(Debug, Clone)]
pub struct Parts {
    pub home: PathBuf,
    pub layout: Layout,
    pub release: Release,
    /// `<config do Claude>/.hangar-update`, o mesmo do Python (`CLAUDE_CONFIG_DIR` ou `~/.claude`).
    pub state_dir: PathBuf,
    pub diary_dir: Option<PathBuf>,
    pub health_url: String,
    pub port: u16,
    /// Avisos do lançamento ([`WARNING_ENV`]), gravados depois que a rodada zera os avisos.
    pub notes: Vec<String>,
}

/// A trava da rodada. Ocupada: o estado é de quem a segura, nada se grava. Outra falha vai ao estado.
fn lock(parts: &Parts, state: &StateFile) -> Option<UpdateLock> {
    match UpdateLock::try_acquire(&parts.layout) {
        Ok(lock) => Some(lock),
        Err(super::UpdateError::Locked) => None,
        Err(e) => {
            let erro = format!("não consegui abrir a trava da atualização: {e}");
            tracing::warn!("update: {erro}");
            state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "reinicio_erro": erro, "voltou": false, "no_ar": true}));
            None
        }
    }
}

impl Parts {
    /// Canal: o pedido; senão o escolhido no Canal de testes ou o do pacote instalado (`super::channel`).
    pub fn new(env: &AccountEnvironment, args: &Args, release_base: &str) -> Parts {
        let layout = Layout::new(&env.home);
        let channel = match &args.task {
            Task::Update { channel: Some(channel) } => channel.clone(),
            _ => super::channel(&layout),
        };
        let host = if args.listen.ip().is_unspecified() { std::net::Ipv4Addr::LOCALHOST.into() } else { args.listen.ip() };
        Parts {
            home: env.home.clone(),
            release: Release { base_url: release_base.to_owned(), channel },
            state_dir: super::state_dir(&env.claude_base),
            diary_dir: crate::diag::diary_dir_in(env),
            health_url: format!("http://{}", SocketAddr::new(host, args.listen.port())),
            port: args.listen.port(),
            notes: env.base.get(WARNING_ENV).filter(|w| !w.is_empty()).cloned().into_iter().collect(),
            layout,
        }
    }
}

/// Aviso do lançamento que o executor leva ao estado depois de zerá-lo (o Windows sem breakaway).
pub const WARNING_ENV: &str = "HANGAR_UPDATE_WARNING";

/// O log do executor; também recebe o stdout/stderr dele (erro antes do log abrir).
pub fn log_path() -> Option<PathBuf> {
    crate::runtime::local_policy::log_base().map(|base| base.join("privado").join("atualizacao-rust.log"))
}

/// O caminho real do binário. No Linux, binário trocado com o servidor de pé (`cargo build` no modo
/// desenvolvedor) aparece como `<caminho> (deleted)`: vale o arquivo novo no mesmo caminho.
pub fn executor_path(exe: &Path) -> std::io::Result<PathBuf> {
    let real = |p: &Path| std::fs::canonicalize(p).map(crate::list::links::without_verbatim);
    if exe.exists() { return real(exe); }
    let replaced = exe.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(" (deleted)")).map(|n| exe.with_file_name(n));
    match replaced {
        Some(path) if path.exists() => real(&path),
        _ => Err(std::io::Error::other(format!("o binário do servidor ({}) não existe mais; reinicie o servidor e tente de novo", exe.display()))),
    }
}

/// Com a unit do systemd, sem escopo próprio o `systemctl restart` mataria o executor no meio.
fn scope_refusal(prefix_empty: bool, require: bool) -> Option<String> {
    (prefix_empty && require).then(|| "o systemd-run --user --scope não está disponível: o reinício do serviço mataria a atualização \
        no meio, sem volta; atualize reiniciando o serviço à mão".to_owned())
}

/// O executor saiu com erro antes de gravar o pid dele: o estado de "começando" vira a falha, com o
/// código de saída, o fim do log e o `pid` que o `start` devolveu (a tela casa o fim por ele). Quem
/// já gravou o pid é dono do estado e escreve o próprio desfecho.
pub fn conclude_early_exit(state_dir: &Path, pid: u32, code: Option<i32>, log: Option<&Path>) {
    let state = StateFile::new(state_dir);
    let current = state.read();
    if current.get("fase").and_then(serde_json::Value::as_str) != Some("rodando") || current.get("pid").is_some_and(|p| !p.is_null()) { return; }
    let text = log.and_then(|l| std::fs::read_to_string(l).ok()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" | ");
    let erro = format!("o processo da atualização saiu com {} antes de começar: {tail}", code.map_or("sinal".to_owned(), |c| c.to_string()));
    tracing::warn!("update: {erro}");
    state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": true, "pid": pid}));
}

/// Quem lança o executor; o real é [`Detached`], o teste anota.
pub trait Spawner: Sync {
    /// `args` a partir do `update`; devolve o pid.
    fn spawn(&self, args: &[String]) -> std::io::Result<u32>;
}

/// O executor destacado, pelo binário real deste servidor.
pub struct Detached {
    pub exe: PathBuf,
    /// A máquina roda pela unit do systemd: sem escopo próprio, recusa.
    pub require_scope: bool,
    /// Onde o estado mora, para concluir um executor que morreu antes de gravar o pid.
    pub state_dir: PathBuf,
}

impl Detached {
    pub fn current(require_scope: bool, state_dir: PathBuf) -> std::io::Result<Detached> {
        Ok(Detached { exe: executor_path(&std::env::current_exe()?)?, require_scope, state_dir })
    }
}

impl Spawner for Detached {
    /// Bloqueante (a sondagem do escopo do systemd roda processo).
    fn spawn(&self, args: &[String]) -> std::io::Result<u32> {
        use std::process::{Command, Stdio};
        #[cfg(unix)]
        let mut command = {
            use std::os::unix::process::CommandExt;
            let prefix = crate::runtime::process::scope_prefix();
            if let Some(why) = scope_refusal(prefix.is_empty(), self.require_scope) { return Err(std::io::Error::other(why)); }
            let mut command = match prefix.split_first() {
                Some((first, rest)) => { let mut c = Command::new(first); c.args(rest).arg(&self.exe); c }
                None => Command::new(&self.exe),
            };
            // SAFETY: entre o fork e o exec só roda `setsid`, que é async-signal-safe e não aloca.
            // Sessão própria: fora do grupo de processos do servidor.
            unsafe { command.pre_exec(|| { libc::setsid(); Ok(()) }); }
            command
        };
        #[cfg(not(unix))]
        let mut command = Command::new(&self.exe);
        let log = log_path();
        let out = log.as_ref().and_then(|l| {
            let _ = std::fs::create_dir_all(l.parent()?);
            std::fs::OpenOptions::new().create(true).append(true).open(l).ok()
        });
        let (stdout, stderr) = match out.as_ref().map(|f| (f.try_clone(), f.try_clone())) {
            Some((Ok(a), Ok(b))) => (Stdio::from(a), Stdio::from(b)),
            _ => (Stdio::null(), Stdio::null()),
        };
        command.args(args).stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        let child = spawn_detached(&mut command)?;
        let pid = child.id();
        // Recolhe o processo quando ele sai (zumbi contaria como vivo para a tela) e conclui o
        // estado se ele morreu antes de assumi-lo.
        let state_dir = self.state_dir.clone();
        std::thread::spawn(move || {
            let mut child = child;
            if let Ok(status) = child.wait() && !status.success() {
                conclude_early_exit(&state_dir, pid, status.code(), log.as_deref());
            }
        });
        Ok(pid)
    }
}

#[cfg(unix)]
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> { command.spawn() }

/// Fora do Job do lançador (senão o reinício que o executor pede o mata), grupo próprio e sem
/// janela. Sem `DETACHED_PROCESS`: com ele o PowerShell do reinício abriria um console visível.
/// Job sem `BREAKAWAY_OK` recusa (acesso negado): aí sobe dentro dele, e o aviso vai ao estado pelo
/// próprio executor ([`WARNING_ENV`]).
#[cfg(windows)]
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const BREAKAWAY: u32 = 0x0100_0000;
    const NEW_GROUP_NO_WINDOW: u32 = 0x0000_0200 | 0x0800_0000;
    match command.creation_flags(BREAKAWAY | NEW_GROUP_NO_WINDOW).spawn() {
        Err(e) if e.raw_os_error() == Some(5) => {
            let warning = "o Job do servidor recusou a saída do processo da atualização: se o reinício derrubar o servidor, ela pode parar no meio";
            tracing::warn!("update: {warning}");
            command.env(WARNING_ENV, warning).creation_flags(NEW_GROUP_NO_WINDOW).spawn()
        }
        other => other,
    }
}

/// Lançado, com o pid do executor e o pedido do reinício.
#[derive(Debug, PartialEq, Eq)]
pub struct Started {
    pub pid: u32,
    pub request_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// Já há atualização ou reinício rodando.
    Busy,
    /// O executor não subiu; o motivo também vai ao estado.
    Launch(String),
}

/// Uma decisão de lançar por vez neste processo (rota e verificação periódica).
static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lança o executor. O estado de "começando" é gravado antes, sem pid: quem grava o pid é o próprio
/// executor, e um pid escrito por aqui poderia correr com a primeira escrita dele. Bloqueante.
pub fn start(home: &Path, claude_base: &Path, args: &Args, spawner: &dyn Spawner) -> Result<Started, StartError> {
    let _one = STARTING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let layout = Layout::new(home);
    let state_dir = super::state_dir(claude_base);
    // Com a versão no ar: executor que morreu no reinício que deu certo conta como concluído.
    let running = super::running_package().map(|p| p.version.as_str());
    if super::for_screen(&state_dir, &layout, running).get("fase").and_then(serde_json::Value::as_str) == Some("rodando") {
        return Err(StartError::Busy);
    }
    // A trava do executor de pé.
    if matches!(UpdateLock::try_acquire(&layout), Err(super::UpdateError::Locked)) { return Err(StartError::Busy); }
    let state = StateFile::new(&state_dir);
    let mut fresh = json!({"fase": "rodando", "ok": null, "erro": null, "pid": null, "passo": 0, "total": super::state::STEPS.len(),
        "texto": "Começando", "voltou": null, "no_ar": null, "reiniciar_manual": false, "avisos": [], "log": []});
    let request_id = match &args.task {
        Task::Restart { request_id } => {
            fresh["texto"] = "Começando o reinício".into();
            fresh["request_id"] = request_id.clone().into();
            fresh["reinicio_erro"] = serde_json::Value::Null;
            Some(request_id.clone())
        }
        _ => None,
    };
    state.write(fresh);
    match spawner.spawn(&args.argv()) {
        Ok(pid) => Ok(Started { pid, request_id }),
        Err(e) => {
            let erro = format!("não consegui lançar o processo da atualização: {e}");
            tracing::warn!("update: {erro}");
            state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": true}));
            Err(StartError::Launch(erro))
        }
    }
}

/// O binário não achado também passa pelo `start`: a recusa chega ao estado como qualquer outra.
struct Missing(String);

impl Spawner for Missing {
    fn spawn(&self, _args: &[String]) -> std::io::Result<u32> { Err(std::io::Error::other(self.0.clone())) }
}

/// Lança o executor destacado deste servidor. `require_scope`: a máquina roda pela unit do systemd.
/// Bloqueante.
pub fn launch(home: &Path, claude_base: &Path, args: &Args, require_scope: bool) -> Result<Started, StartError> {
    match Detached::current(require_scope, super::state_dir(claude_base)) {
        Ok(spawner) => start(home, claude_base, args, &spawner),
        Err(e) => start(home, claude_base, args, &Missing(e.to_string())),
    }
}

/// Reinicia sem atualizar: o mesmo estado e a mesma prova (o Rust de pé com outro pid).
pub async fn restart_only<R: Restarter, H: HealthProbe>(parts: &Parts, request_id: &str, restarter: &R, health: &H, settings: &Settings) -> bool {
    let state = StateFile::new(&parts.state_dir);
    let Some(_lock) = lock(parts, &state) else { return false };
    state.write(json!({"fase": "rodando", "ok": null, "erro": null, "voltou": null, "no_ar": null, "resgate": null,
        "pid": std::process::id(), "reinicio_erro": null, "avisos": [], "log": [], "commit_de": "", "commit_para": null,
        "reiniciar_manual": false, "shell_mudou": false, "waiting_for_server_build": false, "request_id": request_id}));
    for note in &parts.notes { state.warn(note); }
    state.step("reiniciar");
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let result = match restarter.restart().await {
        Err(e) => Err(format!("o servidor não reiniciou: {e}")),
        Ok(()) => super::wait_new(health, before, settings).await,
    };
    match result {
        Ok(()) => { state.write(json!({"fase": "pronto", "ok": true, "texto": "Reiniciado"})); true }
        Err(erro) => {
            tracing::warn!("update: reinício falhou: {erro}");
            state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "reinicio_erro": erro, "voltou": false}));
            false
        }
    }
}

/// Modo desenvolvedor: confere, `git pull --ff-only`, reinicia e prova com qualquer versão do Rust.
/// Sem volta: o pull só avança a branch, e o checkout é de quem desenvolve.
pub async fn dev_update<R: Restarter, H: HealthProbe>(parts: &Parts, root: &Path, restarter: &R, health: &H, settings: &Settings,
    runner: Arc<dyn super::StepRunner + Send + Sync>) -> bool {
    let state = StateFile::new(&parts.state_dir);
    let Some(_lock) = lock(parts, &state) else { return false };
    let fail = |erro: String, no_ar: bool| {
        tracing::warn!("update: modo desenvolvedor: {erro}");
        state.write(json!({"fase": "pronto", "ok": false, "erro": erro, "voltou": false, "no_ar": no_ar}));
        false
    };
    state.write(json!({"fase": "rodando", "ok": null, "erro": null, "resgate": null, "voltou": null, "no_ar": null, "pid": std::process::id(),
        "commit_de": "", "commit_para": null, "reiniciar_manual": false, "shell_mudou": false, "waiting_for_server_build": false,
        "request_id": null, "log": [], "avisos": [], "passos_invalidos": []}));
    for note in &parts.notes { state.warn(note); }
    state.step("baixar");
    let root_owned = root.to_path_buf();
    let pulled = tokio::task::spawn_blocking(move || {
        let checkout = super::dev::read(&root_owned, false)?;
        if let Some(why) = super::dev::refusal(&checkout) { return Err(why); }
        let output = super::dev::pull(&root_owned)?;
        let after = super::dev::read(&root_owned, false)?;
        Ok((checkout.head, after.head, output))
    }).await.unwrap_or_else(|e| Err(e.to_string()));
    let (from, to, output) = match pulled {
        Ok(done) => done,
        Err(why) => return fail(why, true),
    };
    state.log(&output);
    state.write(json!({"commit_de": from, "commit_para": to}));
    if let Some(warning) = super::dev_mode::fetch_rust(runner, root).await { state.warn(&warning); }
    if restarter.manual() {
        state.write(json!({"fase": "pronto", "ok": true, "texto": "Atualizado", "reiniciar_manual": true}));
        return true;
    }
    state.step("reiniciar");
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let result = match restarter.restart().await {
        Err(e) => Err(format!("o servidor não reiniciou: {e}")),
        Ok(()) => super::wait_new(health, before, settings).await,
    };
    match result {
        Ok(()) => { state.write(json!({"fase": "pronto", "ok": true, "texto": "Atualizado"})); true }
        Err(why) => { let no_ar = !matches!(health.read().await, Reading::Down(_)); fail(why, no_ar) }
    }
}

/// O `dev-mode` pedido pela tela: a mesma troca, com o desfecho no estado que ela acompanha.
pub async fn dev_mode<R: Restarter, H: HealthProbe>(parts: &Parts, request: &DevRequest, in_service_group: bool, restarter: &R, health: &H,
    settings: &Settings, runner: Arc<dyn super::StepRunner + Send + Sync>) -> bool {
    use super::dev_mode::{Outcome as DevOutcome, BUSY, ROLLED_BACK};
    let state = StateFile::new(&parts.state_dir);
    // O pid só entra depois das recusas: antes, o `other_engine` da troca o tomaria pelo motor git.
    let on_step = |key: &str| {
        if key == "trocar" {
            state.write(json!({"fase": "rodando", "ok": null, "erro": null, "codigo": null, "voltou": null, "no_ar": null, "resgate": null,
                "pid": std::process::id(), "reinicio_erro": null, "request_id": null, "versao_para": null, "reiniciar_manual": false,
                "log": [], "avisos": []}));
            for note in &parts.notes { state.warn(note); }
        }
        state.step(key);
    };
    // Desligar numa máquina que nunca teve pacote (migrou direto para o modo desenvolvedor): baixa o do canal antes.
    // O pid já no download: sem ele a tela daria a rodada por abandonada depois da janela de nascimento.
    let mut ui_warning = None;
    if *request == DevRequest::Off && matches!(super::dev_mode::check(&parts.home, request), Ok(super::dev_mode::Checked::Download)) {
        state.write(json!({"fase": "rodando", "ok": null, "erro": null, "codigo": null, "voltou": null, "no_ar": null, "resgate": null,
            "pid": std::process::id(), "reinicio_erro": null, "request_id": null, "versao_para": null, "reiniciar_manual": false,
            "log": [], "avisos": []}));
        state.step("baixar");
        match super::download_published(&parts.release, &parts.layout, &parts.home, settings).await {
            Ok(warning) => ui_warning = warning,
            Err(e) => {
                state.write(json!({"fase": "pronto", "ok": false, "erro": format!("o pacote do canal {} não baixou: {e}", parts.release.channel),
                    "codigo": "erro_dev_mode_sem_pacote", "voltou": false, "no_ar": true, "pid": std::process::id()}));
                return false;
            }
        }
    }
    let front_runner = runner.clone();
    // O checkout de onde o vite roda: o do modo desenvolvedor, antes da troca.
    let dev_root = match super::http::mode(&parts.home) { super::http::Mode::Developer(root) => Some(root), _ => None };
    let result = super::dev_mode::switch(&parts.home, &parts.state_dir, request, in_service_group, restarter, health, settings, runner,
        &on_step, &|warning| state.warn(warning)).await;
    if let Some(warning) = ui_warning { state.warn(&warning); }
    // No pacote a porta do front é do servidor: o `vite` do checkout para de segurá-la (a tela velha).
    if *request == DevRequest::Off && matches!(result, Ok(DevOutcome::Switched(_) | DevOutcome::SwitchedManual(_))) && let Some(root) = dev_root {
        let (command, home) = (super::migrate::front_command(cfg!(windows), false, &root), parts.home.clone());
        let run = tokio::task::spawn_blocking(move || front_runner.run(&command, &home)).await;
        match run {
            Ok(run) if run.code == Some(0) => {}
            Ok(run) => state.warn(&format!("o serviço do front (vite) não desligou e segue na porta dele: {}", run.output.trim())),
            Err(e) => state.warn(&format!("o serviço do front (vite) não desligou: {e}")),
        }
    }
    let label = if *request == DevRequest::Off { "Modo desenvolvedor desligado" } else { "Modo desenvolvedor ligado" };
    // O pid vai em todo desfecho: a tela casa o fim pelo pid que o pedido devolveu, e recusa não passa pelo `trocar`.
    let pid = std::process::id();
    match result {
        Ok(DevOutcome::Switched(_)) => { state.write(json!({"fase": "pronto", "ok": true, "texto": label, "pid": pid})); true }
        Ok(DevOutcome::SwitchedManual(_)) => {
            state.write(json!({"fase": "pronto", "ok": true, "texto": label, "reiniciar_manual": true, "pid": pid}));
            true
        }
        Ok(DevOutcome::Already(text)) => { state.write(json!({"fase": "pronto", "ok": true, "texto": text, "pid": pid})); true }
        Err(failure) if failure.code == BUSY => {
            tracing::warn!("update: dev-mode: {failure}");
            // Estado com o pid de outra rodada é dela; sem pid é o "começando" deste pedido.
            if state.read().get("pid").is_none_or(|p| p.is_null() || *p == json!(pid)) {
                state.write(json!({"fase": "pronto", "ok": false, "erro": failure.message, "codigo": failure.code,
                    "voltou": false, "no_ar": true, "pid": pid}));
            }
            false
        }
        Err(failure) => {
            tracing::warn!(code = failure.code, "update: dev-mode: {failure}");
            let no_ar = !matches!(health.read().await, Reading::Down(_));
            state.write(json!({"fase": "pronto", "ok": false, "erro": failure.message, "codigo": failure.code,
                "voltou": failure.code == ROLLED_BACK, "no_ar": no_ar, "pid": pid}));
            false
        }
    }
}

/// A migração aceita na tela. Ela mesma para, sobe, prova e desfaz; aqui só o estado que a tela lê.
/// Numa thread comum: o `migrate::run` cria o próprio runtime.
async fn migrate(parts: &Parts, checkout: Option<PathBuf>) -> bool {
    let state = StateFile::new(&parts.state_dir);
    state.write(json!({"fase": "rodando", "ok": null, "erro": null, "codigo": null, "voltou": null, "no_ar": null, "resgate": null,
        "pid": std::process::id(), "reinicio_erro": null, "request_id": null, "versao_para": null, "reiniciar_manual": false,
        "log": [], "avisos": []}));
    state.step("migrar");
    let args: Vec<String> = checkout.map(|dir| vec!["--checkout".to_owned(), dir.display().to_string()]).unwrap_or_default();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || { let _ = sender.send(super::migrate::run(&args)); });
    let result = receiver.await.unwrap_or_else(|_| Err((1, "a migração morreu sem resposta".to_owned())));
    let pid = std::process::id();
    // Só vale migrada de fato: "não migrada" (sem pacote para o sistema) sai 0 na linha de comando,
    // mas aqui a pessoa pediu a migração e ela não aconteceu.
    let migrated = super::migrate::read_record(&parts.layout.hangar.join(super::migrate::MARKER))
        .is_some_and(|r| r.mode != super::migrate::Mode::NotMigrated);
    let result = result.and_then(|lines| if migrated { Ok(lines) } else { Err((1, lines.join("; "))) });
    match result {
        Ok(lines) => {
            for line in lines.iter().filter_map(|l| l.strip_prefix("aviso: ")) { state.warn(line); }
            state.write(json!({"fase": "pronto", "ok": true, "texto": lines.first().cloned().unwrap_or_default(), "pid": pid}));
            true
        }
        Err((_, why)) => {
            let why = why.trim_start_matches("##HANGAR-FALHA##").trim().to_owned();
            tracing::warn!("update: migração: {why}");
            // O motivo do `migrate::roll_back` diz o que da volta não se completou.
            let (back, up) = (!(why.contains(", menos:") || why.contains("não parou")), !why.contains("não subiu"));
            state.write(json!({"fase": "pronto", "ok": false, "erro": why, "codigo": "erro_migracao", "voltou": back, "no_ar": up, "pid": pid}));
            false
        }
    }
}

/// `hangar-server update …`: `args` sem o nome do programa nem o `update`.
pub fn cli(args: &[String]) -> ExitCode {
    let args = match Args::parse(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("hangar-server update: {e}\nuso: hangar-server update --run [--channel C] | --restart --request-id ID | --dev \
                | --dev-mode-on PASTA | --dev-mode-off | --migrate [--checkout PASTA]  [--listen IP:PORTA]");
            return ExitCode::from(2);
        }
    };
    let log = log_path();
    if let Some(dir) = log.as_ref().and_then(|l| l.parent()) { let _ = std::fs::create_dir_all(dir); }
    crate::init_log(log.as_deref());
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => { eprintln!("hangar-server update: runtime: {e}"); return ExitCode::from(1); }
    };
    let ok = runtime.block_on(execute(&AccountEnvironment::capture(), &args));
    if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

async fn execute(env: &AccountEnvironment, args: &Args) -> bool {
    let parts = Parts::new(env, args, super::RELEASE_BASE);
    let runner: Arc<dyn super::StepRunner + Send + Sync> = Arc::new(super::ShellRunner { timeout: super::steps::STEP_TIMEOUT });
    let topology = {
        let (runner, home) = (runner.clone(), parts.home.clone());
        tokio::task::spawn_blocking(move || Topology::detect(&*runner, &home, cfg!(windows))).await.unwrap_or(Topology::Manual)
    };
    let restarter = SystemRestarter { topology, runner: runner.clone(), home: parts.home.clone(), port: parts.port };
    let health = super::HttpHealth::new(&parts.health_url);
    let settings = Settings::default();
    tracing::info!(task = ?args.task, topology = topology.name(), "update: executor de pé");
    let fetch_runner = Arc::new(super::ShellRunner { timeout: super::dev_mode::FETCH_TIMEOUT });
    match &args.task {
        Task::Restart { request_id } => restart_only(&parts, request_id, &restarter, &health, &settings).await,
        Task::Dev => match super::http::mode(&parts.home) {
            super::http::Mode::Developer(root) => dev_update(&parts, &root, &restarter, &health, &settings, fetch_runner).await,
            _ => { tracing::warn!("update: --dev sem modo desenvolvedor"); false }
        },
        Task::DevMode(request) => dev_mode(&parts, request, super::migrate::in_service_group(), &restarter, &health, &settings, fetch_runner).await,
        Task::Migrate { checkout } => migrate(&parts, checkout.clone()).await,
        Task::Update { .. } => {
            let client = match crate::http_client().build() {
                Ok(client) => client,
                Err(e) => { tracing::warn!("update: cliente http: {e}"); return false; }
            };
            let platform = super::platform();
            let job = super::Job { client: &client, release: &parts.release, platform: &platform, layout: &parts.layout, home: &parts.home,
                state_dir: &parts.state_dir, diary_dir: parts.diary_dir.clone(), ui: Some(&super::ReleaseUi), notes: parts.notes.clone(), settings };
            let outcome = super::run(&job, runner, &restarter, &health, Arc::new(super::ProcInUse)).await;
            tracing::info!(?outcome, "update: fim");
            matches!(outcome, Outcome::Updated { .. } | Outcome::UpdatedManual { .. } | Outcome::UpToDate)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn strings(args: &[&str]) -> Vec<String> { args.iter().map(|a| a.to_string()).collect() }

    #[test]
    fn update_executor_args_parse_and_round_trip() {
        let run = Args::parse(&strings(&["--run", "--channel", "beta", "--listen", "0.0.0.0:9000"])).unwrap();
        assert_eq!(run, Args { task: Task::Update { channel: Some("beta".into()) }, listen: "0.0.0.0:9000".parse().unwrap() });
        assert_eq!(Args::parse(&run.argv()[1..]).unwrap(), run, "o que o servidor lança é o que o executor lê");
        assert_eq!(run.argv()[0], "update");
        let restart = Args::parse(&strings(&["--restart", "--request-id", "abc"])).unwrap();
        assert_eq!(restart, Args { task: Task::Restart { request_id: "abc".into() }, listen: DEFAULT_LISTEN.parse().unwrap() });
        assert_eq!(Args::parse(&strings(&["--dev"])).unwrap().task, Task::Dev);
        for task in [Task::DevMode(DevRequest::On("/x/hangar".into())), Task::DevMode(DevRequest::Off),
                     Task::Migrate { checkout: None }, Task::Migrate { checkout: Some("/x/hangar".into()) }] {
            let args = Args { task, listen: DEFAULT_LISTEN.parse().unwrap() };
            assert_eq!(Args::parse(&args.argv()[1..]).unwrap(), args);
        }
        for bad in [&[][..], &["--restart"], &["--run", "--listen"], &["--run", "--dev"], &["--run", "--x"], &["--run", "--listen", "nada"],
                    &["--dev-mode-on"], &["--dev-mode-off", "--dev"]] {
            assert!(Args::parse(&strings(bad)).is_err(), "{bad:?}");
        }
    }

    fn env(home: &Path, claude: Option<&Path>) -> AccountEnvironment {
        let mut map = std::collections::BTreeMap::from([("HOME".to_owned(), home.display().to_string()),
            ("USERPROFILE".to_owned(), home.display().to_string())]);
        if let Some(claude) = claude { map.insert("CLAUDE_CONFIG_DIR".into(), claude.display().to_string()); }
        AccountEnvironment::from_map(map)
    }

    #[test]
    fn update_executor_job_parts_follow_the_python_paths_and_the_installed_channel() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let args = Args::parse(&strings(&["--run", "--listen", "0.0.0.0:9000"])).unwrap();
        let parts = Parts::new(&env(home, None), &args, "http://base");
        assert_eq!(parts.state_dir, home.join(".claude").join(".hangar-update"));
        assert_eq!((parts.release.channel.as_str(), parts.release.base_url.as_str()), ("latest", "http://base"), "sem pacote, a main");
        assert_eq!((parts.health_url.as_str(), parts.port), ("http://127.0.0.1:9000", 9000));
        assert_eq!(parts.layout.hangar, home.join(".hangar"));

        let claude = home.join("outra-conta");
        assert_eq!(Parts::new(&env(home, Some(&claude)), &args, "x").state_dir, claude.join(".hangar-update"), "CLAUDE_CONFIG_DIR");

        let version = home.join(".hangar/versions/0.1.0.5");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("manifest.json"), json!({"version": "0.1.0.5", "commit": "c", "channel": "beta", "protocol": 1,
            "api": 1, "os": "linux-x86_64", "files": {}}).to_string()).unwrap();
        super::super::swap(&Layout::new(home), &version).unwrap();
        assert_eq!(Parts::new(&env(home, None), &args, "x").release.channel, "beta", "o canal do pacote instalado");
        let asked = Args::parse(&strings(&["--run", "--channel", "outro"])).unwrap();
        assert_eq!(Parts::new(&env(home, None), &asked, "x").release.channel, "outro", "o pedido vence");
        assert_eq!(Parts::new(&env(home, None), &asked, "x").health_url, "http://127.0.0.1:8765");
    }

    #[derive(Default)]
    struct FakeSpawner {
        spawned: Mutex<Vec<Vec<String>>>,
        fail: bool,
    }

    impl Spawner for FakeSpawner {
        fn spawn(&self, args: &[String]) -> std::io::Result<u32> {
            if self.fail { return Err(std::io::Error::other("sem permissão")); }
            self.spawned.lock().unwrap().push(args.to_vec());
            Ok(4242)
        }
    }

    #[test]
    fn update_start_writes_starting_spawns_once_and_refuses_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let (home, claude) = (dir.path(), dir.path().join(".claude"));
        let spawner = FakeSpawner::default();
        let args = Args { task: Task::Update { channel: None }, listen: DEFAULT_LISTEN.parse().unwrap() };
        assert_eq!(start(home, &claude, &args, &spawner), Ok(Started { pid: 4242, request_id: None }));
        let state = StateFile::new(&super::super::state_dir(&claude)).read();
        assert_eq!((state["fase"].clone(), state["pid"].clone(), state["texto"].clone()), (json!("rodando"), json!(null), json!("Começando")));
        assert_eq!(spawner.spawned.lock().unwrap().as_slice(), [args.argv()]);
        assert_eq!(start(home, &claude, &args, &spawner), Err(StartError::Busy), "o executor ainda sobe");
        assert_eq!(spawner.spawned.lock().unwrap().len(), 1);

        StateFile::new(&super::super::state_dir(&claude)).write(json!({"fase": "pronto", "ok": true}));
        let restart = Args { task: Task::Restart { request_id: "pedido-1".into() }, listen: DEFAULT_LISTEN.parse().unwrap() };
        assert_eq!(start(home, &claude, &restart, &spawner), Ok(Started { pid: 4242, request_id: Some("pedido-1".into()) }));
        assert_eq!(StateFile::new(&super::super::state_dir(&claude)).read()["request_id"], "pedido-1");

        // A trava de um executor de pé recusa também.
        StateFile::new(&super::super::state_dir(&claude)).write(json!({"fase": "pronto", "ok": true}));
        let _held = UpdateLock::try_acquire(&Layout::new(home)).unwrap();
        assert_eq!(start(home, &claude, &args, &spawner), Err(StartError::Busy));
    }

    #[test]
    fn update_start_that_cannot_launch_says_why_in_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        let args = Args { task: Task::Update { channel: None }, listen: DEFAULT_LISTEN.parse().unwrap() };
        let Err(StartError::Launch(why)) = start(dir.path(), &claude, &args, &FakeSpawner { fail: true, ..FakeSpawner::default() }) else { panic!() };
        assert!(why.contains("sem permissão"));
        let state = StateFile::new(&super::super::state_dir(&claude)).read();
        assert_eq!((state["fase"].clone(), state["ok"].clone()), (json!("pronto"), json!(false)));
    }

    struct Restart(AtomicUsize);
    impl Restarter for Restart {
        async fn restart(&self) -> Result<(), String> { self.0.fetch_add(1, Ordering::SeqCst); Ok(()) }
    }

    /// Antes do reinício o pid 10; depois, o 11 (ou o mesmo, com `same`).
    struct Health<'a>(&'a AtomicUsize, bool);
    impl HealthProbe for Health<'_> {
        async fn read(&self) -> Reading {
            let restarted = self.0.load(Ordering::SeqCst) > 0 && !self.1;
            Reading::Rust { protocol: 1, pid: Some(if restarted { 11 } else { 10 }), commit: None }
        }
    }

    fn fast() -> Settings { Settings { health_deadline: Duration::from_millis(100), health_interval: Duration::from_millis(5), ..Settings::default() } }

    #[tokio::test]
    async fn update_restart_only_proves_a_new_process_and_keeps_the_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args::parse(&strings(&["--restart", "--request-id", "r1"])).unwrap();
        let parts = Parts::new(&env(dir.path(), None), &args, "x");
        let restart = Restart(AtomicUsize::new(0));
        assert!(restart_only(&parts, "r1", &restart, &Health(&restart.0, false), &fast()).await);
        let state = StateFile::new(&parts.state_dir).read();
        assert_eq!((state["fase"].clone(), state["ok"].clone(), state["request_id"].clone(), state["pid"].clone()),
            (json!("pronto"), json!(true), json!("r1"), json!(std::process::id())));

        // Outra pasta, outra trava: um filho que outro teste lança herda a descrição do arquivo da
        // trava até o exec, e a segunda chamada na mesma pasta podia achá-la ocupada.
        let dir = tempfile::tempdir().unwrap();
        let parts = Parts::new(&env(dir.path(), None), &args, "x");
        let same = Restart(AtomicUsize::new(0));
        assert!(!restart_only(&parts, "r2", &same, &Health(&same.0, true), &fast()).await);
        let state = StateFile::new(&parts.state_dir).read();
        assert_eq!(state["ok"], false);
        assert!(state["reinicio_erro"].as_str().unwrap().contains("mesmo de antes"));
    }

    /// Uma máquina em pacote e um checkout ao lado.
    fn packaged(dir: &Path) -> (Parts, PathBuf) {
        let home = dir.join("home");
        let version = home.join(".hangar/versions/0.1.0.5");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("manifest.json"), json!({"version": "0.1.0.5", "commit": "c5", "channel": "latest", "protocol": 1,
            "api": 1, "os": super::super::platform(), "files": {}}).to_string()).unwrap();
        super::super::swap(&Layout::new(&home), &version).unwrap();
        let checkout = dir.join("hangar");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(checkout.join("backend/app")).unwrap();
        let args = Args { task: Task::DevMode(DevRequest::Off), listen: DEFAULT_LISTEN.parse().unwrap() };
        (Parts::new(&env(&home, None), &args, "x"), checkout)
    }

    #[tokio::test]
    async fn update_dev_mode_writes_the_outcome_to_the_state_the_screen_reads() {
        let dir = tempfile::tempdir().unwrap();
        let (parts, checkout) = packaged(dir.path());
        let state = StateFile::new(&parts.state_dir);
        let restart = Restart(AtomicUsize::new(0));
        let health = Health(&restart.0, false);

        assert!(!dev_mode(&parts, &DevRequest::On(checkout.join("nada")), false, &restart, &health, &fast(), silent()).await);
        let read = state.read();
        assert_eq!((read["fase"].clone(), read["ok"].clone(), read["codigo"].clone(), read["pid"].clone()),
            (json!("pronto"), json!(false), json!("erro_dev_mode_pasta_inexistente"), json!(std::process::id())), "a tela casa o fim pelo pid");
        assert!(read["erro"].as_str().unwrap().contains("não existe"));
        assert_eq!(restart.0.load(Ordering::SeqCst), 0);

        assert!(dev_mode(&parts, &DevRequest::On(checkout.clone()), false, &restart, &health, &fast(), silent()).await);
        let read = state.read();
        assert_eq!((read["fase"].clone(), read["ok"].clone(), read["etapa"].clone(), read["pid"].clone(), read["codigo"].clone()),
            (json!("pronto"), json!(true), json!("reiniciar"), json!(std::process::id()), json!(null)));
        // Checkout sem venv: o Rust dele não foi atualizado, e a tela mostra o aviso.
        assert!(read["avisos"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Python")), "{read:?}");
        assert_eq!(restart.0.load(Ordering::SeqCst), 1);

        // Trava ocupada por quem não gravou o estado (o `start` acabou de gravar o "começando"): a
        // recusa vai ao estado com o pid deste processo.
        state.write(json!({"fase": "rodando", "pid": null}));
        let _held = UpdateLock::try_acquire(&parts.layout).unwrap();
        assert!(!dev_mode(&parts, &DevRequest::Off, false, &restart, &health, &fast(), silent()).await);
        let read = state.read();
        assert_eq!((read["fase"].clone(), read["ok"].clone(), read["codigo"].clone(), read["pid"].clone()),
            (json!("pronto"), json!(false), json!(super::super::dev_mode::BUSY), json!(std::process::id())));
        assert!(read["erro"].as_str().unwrap().contains("rodando"));

        // Estado com o pid de outra rodada: é dela.
        state.write(json!({"fase": "rodando", "pid": 77}));
        assert!(!dev_mode(&parts, &DevRequest::Off, false, &restart, &health, &fast(), silent()).await);
        assert_eq!((state.read()["fase"].clone(), state.read()["pid"].clone()), (json!("rodando"), json!(77)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn update_dev_refuses_a_dirty_checkout_without_touching_it_and_pulls_a_clean_one() {
        let dir = tempfile::tempdir().unwrap();
        let clone = super::super::dev::tests::behind_clone(dir.path());
        let parts = Parts::new(&env(dir.path(), None), &Args::parse(&strings(&["--dev"])).unwrap(), "x");
        std::fs::write(clone.join("a"), "editado").unwrap();
        let restart = Restart(AtomicUsize::new(0));
        assert!(!dev_update(&parts, &clone, &restart, &Health(&restart.0, false), &fast(), silent()).await);
        assert_eq!(restart.0.load(Ordering::SeqCst), 0);
        assert!(!clone.join("b").exists(), "nada foi puxado");
        assert!(StateFile::new(&parts.state_dir).read()["erro"].as_str().unwrap().contains("com edição"));

        super::super::dev::tests::sh(&clone, "git checkout -q -- a");
        assert!(dev_update(&parts, &clone, &restart, &Health(&restart.0, false), &fast(), silent()).await);
        assert!(clone.join("b").exists());
        assert_eq!(restart.0.load(Ordering::SeqCst), 1);
        assert_eq!(StateFile::new(&parts.state_dir).read()["ok"], true);
    }

    /// Runner que não roda nada.
    fn silent() -> Arc<dyn super::super::StepRunner + Send + Sync> { Arc::new(Seen::default()) }

    /// Grava o comando, a pasta, quantos reinícios já houve e se o pull já tinha trazido o `b`.
    #[derive(Default)]
    struct Seen { restarts: Option<Arc<Restart>>, calls: Mutex<Vec<(String, PathBuf, usize, bool)>> }
    impl super::super::StepRunner for Seen {
        fn run(&self, command: &str, cwd: &Path) -> super::super::StepRun {
            let restarts = self.restarts.as_ref().map_or(0, |r| r.0.load(Ordering::SeqCst));
            let pulled = cwd.parent().is_some_and(|c| c.join("b").exists());
            self.calls.lock().unwrap().push((command.to_owned(), cwd.to_owned(), restarts, pulled));
            super::super::StepRun { code: Some(1), output: "sem rede".into() }
        }
    }

    /// Cada atualização do modo desenvolvedor baixa o Rust da branch do checkout depois do pull e
    /// antes do reinício; a falha é aviso, e a atualização segue.
    #[cfg(unix)]
    #[tokio::test]
    async fn update_dev_fetches_the_checkout_rust_after_the_pull_and_before_the_restart() {
        let dir = tempfile::tempdir().unwrap();
        let clone = super::super::dev::tests::behind_clone(dir.path());
        let python = crate::install_root::python_exe(&clone, false);
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&python, "").unwrap();
        let parts = Parts::new(&env(dir.path(), None), &Args::parse(&strings(&["--dev"])).unwrap(), "x");
        let restart = Arc::new(Restart(AtomicUsize::new(0)));
        let runner = Arc::new(Seen { restarts: Some(restart.clone()), ..Seen::default() });
        assert!(dev_update(&parts, &clone, &*restart, &Health(&restart.0, false), &fast(), runner.clone()).await);
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (command, cwd, restarts, pulled) = &calls[0];
        assert!(command.contains(&python.display().to_string()) && command.ends_with("-m app.rust_release"), "{command}");
        assert_eq!((cwd, *restarts, *pulled), (&clone.join("backend"), 0, true));
        assert_eq!(restart.0.load(Ordering::SeqCst), 1);
        let state = StateFile::new(&parts.state_dir).read();
        assert_eq!(state["ok"], true);
        assert!(state["avisos"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("sem rede")), "{state:?}");
    }
}

#[cfg(test)]
mod review_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn update_executor_path_drops_the_deleted_suffix_and_says_when_nothing_exists() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("hangar-server");
        std::fs::write(&exe, "").unwrap();
        let real = crate::list::links::without_verbatim(std::fs::canonicalize(&exe).unwrap());
        assert_eq!(executor_path(&exe).unwrap(), real);
        assert_eq!(executor_path(&dir.path().join("hangar-server (deleted)")).unwrap(), real, "binário trocado com o servidor de pé");
        let error = executor_path(&dir.path().join("sumiu (deleted)")).unwrap_err().to_string();
        assert!(error.contains("não existe mais"), "{error}");
    }

    #[test]
    fn update_systemd_without_a_scope_refuses_to_launch() {
        assert!(scope_refusal(true, true).unwrap().contains("systemd-run"));
        assert_eq!(scope_refusal(true, false), None, "sem a unit, o reinício não mata o executor");
        assert_eq!(scope_refusal(false, true), None);
    }

    #[test]
    fn update_child_that_dies_before_writing_its_pid_is_concluded_with_the_code_and_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateFile::new(dir.path());
        let log = dir.path().join("executor.log");
        std::fs::write(&log, "linha velha\nhangar-server update: argumento inesperado: --x\n").unwrap();
        state.write(json!({"fase": "rodando", "pid": null}));
        conclude_early_exit(dir.path(), 4242, Some(2), Some(&log));
        let read = state.read();
        assert_eq!((read["fase"].clone(), read["ok"].clone(), read["pid"].clone()), (json!("pronto"), json!(false), json!(4242)),
            "a tela casa o fim pelo pid que o pedido devolveu");
        let erro = read["erro"].as_str().unwrap();
        assert!(erro.contains('2') && erro.contains("argumento inesperado"), "{erro}");

        // Quem já gravou o pid é dono do estado.
        state.write(json!({"fase": "rodando", "pid": 77, "ok": null, "erro": null}));
        conclude_early_exit(dir.path(), 4242, Some(1), Some(&log));
        assert_eq!(state.read()["fase"], "rodando");
    }

    struct Ok1(AtomicUsize);
    impl Restarter for Ok1 {
        async fn restart(&self) -> Result<(), String> { self.0.fetch_add(1, Ordering::SeqCst); Ok(()) }
    }
    struct NewPid<'a>(&'a AtomicUsize);
    impl HealthProbe for NewPid<'_> {
        async fn read(&self) -> Reading { Reading::Rust { protocol: 1, pid: Some(10 + self.0.load(Ordering::SeqCst) as u32), commit: None } }
    }

    fn parts(home: &Path, warning: Option<&str>) -> Parts {
        let mut map = std::collections::BTreeMap::from([("HOME".to_owned(), home.display().to_string()),
            ("USERPROFILE".to_owned(), home.display().to_string())]);
        if let Some(w) = warning { map.insert(WARNING_ENV.into(), w.into()); }
        Parts::new(&AccountEnvironment::from_map(map), &Args::parse(&["--restart".into(), "--request-id".into(), "r".into()]).unwrap(), "x")
    }

    fn fast() -> Settings { Settings { health_deadline: Duration::from_millis(100), health_interval: Duration::from_millis(5), ..Settings::default() } }

    #[tokio::test]
    async fn update_launch_warning_reaches_the_state_after_the_executor_resets_it() {
        let dir = tempfile::tempdir().unwrap();
        let parts = parts(dir.path(), Some("o Job do servidor recusou a saída"));
        let restart = Ok1(AtomicUsize::new(0));
        assert!(restart_only(&parts, "r", &restart, &NewPid(&restart.0), &fast()).await);
        assert_eq!(StateFile::new(&parts.state_dir).read()["avisos"], json!(["o Job do servidor recusou a saída"]));
    }

    #[tokio::test]
    async fn update_lock_that_cannot_be_opened_is_written_to_the_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hangar"), "não é pasta").unwrap();
        let parts = parts(dir.path(), None);
        let restart = Ok1(AtomicUsize::new(0));
        assert!(!restart_only(&parts, "r", &restart, &NewPid(&restart.0), &fast()).await);
        let state = StateFile::new(&parts.state_dir).read();
        assert_eq!(state["ok"], false);
        assert!(state["erro"].as_str().unwrap().contains("trava"), "{state:?}");
        assert_eq!(restart.0.load(Ordering::SeqCst), 0);
    }
}
