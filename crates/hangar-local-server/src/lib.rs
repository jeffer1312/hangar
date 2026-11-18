//! O servidor desta máquina sem a API dele: estado do serviço, motivo da falha, versões instaladas,
//! o `~/.hangar/current` e as ações que consertam (subir, liberar a porta, apontar para outra
//! versão). Sem `hangar-core`: o app nativo usa este crate quando o servidor está parado, e o
//! servidor e o core usam as mesmas primitivas de link, de caminho e de leitura. Nenhuma ação tem
//! rota HTTP.
//!
//! Tudo o que sai do processo passa por `host::Host`; tudo é bloqueante.
pub mod config;
pub mod diary;
pub mod host;
pub mod link;
pub mod lock;
pub mod port;
pub mod service;
pub mod versions;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use host::{Host, RealHost};
use host::ProcessInfo;
pub use lock::{LockError, UpdateLock};
pub use port::{Listeners, PortHolder};
pub use service::{Os, Service, ServiceState};
pub use versions::Versions;

pub const DEFAULT_PORT: u16 = 8765;
const OUTPUT_LINES: usize = 40;
/// Processo de pé há mais que isso sem a porta responder não está subindo, está preso. O servidor
/// abre a porta antes de qualquer preparo, em segundos; 180 s é a carência do vigia do servidor
/// (`watchdog::GRACE`), em que o `WATCHDOG=1` sai sempre. Depois dela o systemd (`WatchdogSec=60`)
/// já mataria um servidor travado; no Windows não há quem mate, e é aqui que ele vira parado.
pub const STARTING_LIMIT: Duration = Duration::from_secs(180);
/// Quanto `free_port` espera a porta soltar depois do `SIGTERM` (ou do `taskkill`)...
const RELEASE_WAIT: Duration = Duration::from_secs(5);
/// ...e depois do `SIGKILL`.
const KILL_WAIT: Duration = Duration::from_secs(3);
const STEP: Duration = Duration::from_millis(100);
/// Quanto `use_version` espera alguém responder na porta para conferir a versão.
const VERSION_WAIT: Duration = Duration::from_secs(10);
const VERSION_STEP: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalState {
    Running,
    /// A saúde responde e o resto ainda devolve 503 `server_starting`, ou o gerenciador diz que
    /// está subindo e a porta ainda não atende (até `STARTING_LIMIT`).
    Starting,
    Stopped { failure: Option<Failure> },
    /// Sem unit, tarefa ou agente: o servidor não é instalado como serviço nesta máquina.
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Código estável: `port_unavailable`, `launch_no_current`, `launch_no_server`, `server_exited`,
    /// `service_exit`, `service_failed`, `service_unreadable` ou o `codigo` do diário. O texto da
    /// tela sai no cliente.
    pub code: String,
    /// Dado da falha (nome do processo, caminho, código de saída), nunca frase.
    pub detail: String,
    pub port_holder: Option<PortHolder>,
    /// As últimas linhas do serviço: journal no Linux, `hangar-backend*.log` no Windows,
    /// `backend-launchd.log` no macOS.
    pub output: Vec<String>,
}

/// O que o pedido de subir conseguiu dizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    /// O gerenciador não respondeu no prazo do comando (o `systemctl start` espera a unit ficar
    /// pronta): o painel segue pelo `probe`.
    StillStarting,
}

#[derive(Debug)]
pub enum ActionError {
    /// Sem gerenciador conhecido, ou troca de versão no macOS (sem pacote).
    Unsupported,
    DevMode,
    /// Sem unit, tarefa ou agente: não há quem suba a versão trocada.
    NotInstalled,
    /// O gerenciador não respondeu: não dá para saber se o servidor está parado.
    ServiceUnreadable(String),
    /// Outra atualização ou troca segura a trava.
    Locked,
    ServerRunning,
    UnknownVersion(String),
    NoBinary(String),
    /// O processo da porta não é mais o do diagnóstico (saiu, outro pid, outro nascimento).
    HolderChanged,
    OtherUser,
    TerminateRefused,
    PortStillHeld,
    /// Depois da troca e da subida, o `current` aponta para outra versão.
    CurrentChanged(String),
    /// Depois da troca e da subida, responde outra versão (subiu a antiga entre a conferência e a troca).
    VersionMismatch(String),
    /// A troca entrou, o passo seguinte falhou e o `current` não voltou: segue em `version`.
    CurrentLeftOn { version: String, cause: String },
    Command { code: Option<i32>, output: String },
    Io(std::io::Error),
}

impl ActionError {
    pub fn code(&self) -> &'static str {
        match self {
            ActionError::Unsupported => "local_server_unsupported",
            ActionError::DevMode => "local_server_dev_mode",
            ActionError::NotInstalled => "local_server_not_installed",
            ActionError::ServiceUnreadable(_) => "local_server_service_unreadable",
            ActionError::Locked => "update_locked",
            ActionError::ServerRunning => "local_server_running",
            ActionError::UnknownVersion(_) => "local_server_unknown_version",
            ActionError::NoBinary(_) => "local_server_no_binary",
            ActionError::HolderChanged => "local_server_holder_changed",
            ActionError::OtherUser => "local_server_other_user",
            ActionError::TerminateRefused => "local_server_terminate_refused",
            ActionError::PortStillHeld => "local_server_port_still_held",
            ActionError::CurrentChanged(_) => "local_server_current_changed",
            ActionError::VersionMismatch(_) => "local_server_version_mismatch",
            ActionError::CurrentLeftOn { .. } => "local_server_current_left_on",
            ActionError::Command { .. } => "local_server_command_failed",
            ActionError::Io(_) => "local_server_io",
        }
    }

    /// O dado para o detalhe da tela e do diário.
    pub fn detail(&self) -> String {
        match self {
            ActionError::UnknownVersion(v) | ActionError::NoBinary(v) | ActionError::ServiceUnreadable(v)
            | ActionError::CurrentChanged(v) | ActionError::VersionMismatch(v) => v.clone(),
            ActionError::CurrentLeftOn { version, cause } => format!("{version}: {cause}"),
            ActionError::Command { code, output } => format!("{code:?}: {output}"),
            ActionError::Io(e) => e.to_string(),
            _ => String::new(),
        }
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = self.detail();
        if detail.is_empty() { f.write_str(self.code()) } else { write!(f, "{}: {detail}", self.code()) }
    }
}

impl std::error::Error for ActionError {}

/// O servidor desta máquina: `home`, a porta pública e onde ficam os logs.
pub struct LocalServer<H: Host = RealHost> {
    pub home: PathBuf,
    pub port: u16,
    pub log_base: PathBuf,
    pub os: Os,
    pub host: H,
}

impl LocalServer<RealHost> {
    pub fn new(home: &Path, port: u16) -> LocalServer<RealHost> {
        LocalServer { home: home.to_path_buf(), port, log_base: diary::log_base(home), os: Os::current(), host: RealHost }
    }

    /// Na porta em que o serviço sobe (`config::resolve`), com a ressalva da leitura dela para o log de
    /// quem chama. Bloqueia: lê a unit e o `.env`.
    pub fn configured(home: &Path) -> (LocalServer<RealHost>, Option<config::Issue>) {
        let resolved = config::resolve(&RealHost, Os::current(), home);
        (LocalServer::new(home, resolved.port), resolved.issue)
    }
}

/// O que respondeu na porta.
enum Answer {
    /// `version`: o `package_version` da saúde; ausente fora de pacote.
    Hangar { starting: bool, version: Option<String> },
    Other,
    Silent,
}

impl<H: Host> LocalServer<H> {
    pub fn hangar(&self) -> PathBuf { self.home.join(".hangar") }

    pub fn probe(&self) -> LocalState {
        match self.answer() {
            Answer::Hangar { starting: true, .. } => return LocalState::Starting,
            Answer::Hangar { starting: false, .. } => return LocalState::Running,
            Answer::Other | Answer::Silent => {}
        }
        let service = self.service_now();
        match service.state {
            ServiceState::Missing => LocalState::NotInstalled,
            // De pé no gerenciador e a porta ainda sem resposta: a subida antes de abrir a porta.
            ServiceState::Starting | ServiceState::Running if !self.stuck(&service) => LocalState::Starting,
            ServiceState::Starting | ServiceState::Running => LocalState::Stopped { failure: Some(Failure {
                code: "service_failed".to_owned(), detail: service.summary.clone(), port_holder: None, output: self.output() }) },
            _ => LocalState::Stopped { failure: self.failure(&service) },
        }
    }

    /// O processo da vida atual está de pé há mais que `STARTING_LIMIT`. Sem pid conhecido, não.
    fn stuck(&self, service: &Service) -> bool {
        service.main_pid.and_then(|pid| self.host.process(pid))
            .is_some_and(|process| self.host.now().saturating_sub(process.start_time) > STARTING_LIMIT.as_secs())
    }

    /// A saúde só diz "o hangar-server está de pé"; quem diz se a subida acabou é uma rota comum,
    /// que responde 503 `server_starting` até lá (a trava da subida vem antes da autenticação).
    fn answer(&self) -> Answer {
        let json = |body: &str| serde_json::from_str::<serde_json::Value>(body).ok();
        match self.host.get(self.port, "/__hangar_server/health") {
            None => Answer::Silent,
            Some(reply) if reply.status == 200 && json(&reply.body).is_some_and(|v| v.get("protocol").is_some()) => {
                let version = json(&reply.body).and_then(|v| v.get("package_version").and_then(serde_json::Value::as_str).map(str::to_owned));
                let starting = self.host.get(self.port, "/api/client-api").is_some_and(|reply| reply.status == 503
                    && json(&reply.body).is_some_and(|v| v.get("detail").and_then(serde_json::Value::as_str) == Some("server_starting")));
                Answer::Hangar { starting, version }
            }
            Some(_) => Answer::Other,
        }
    }

    /// A leitura crua do gerenciador.
    pub fn service(&self) -> Service { service::read(&self.host, self.os) }

    /// A leitura corrigida pela lista de processos no Windows: a tarefa segue `Running` com o
    /// lançador esperando para religar, e o processo da vida é o `hangar-server` filho dele.
    fn service_now(&self) -> Service {
        let mut service = self.service();
        if self.os != Os::Windows || service.state != ServiceState::Running { return service; }
        let processes = self.host.processes();
        let launchers: Vec<u32> = processes.iter().filter(|p| is_backend_launcher(&p.name, &p.cmd)).map(|p| p.pid).collect();
        let server = processes.iter().find(|p| p.parent.is_some_and(|parent| launchers.contains(&parent))
            && p.name.to_ascii_lowercase().starts_with("hangar-server"));
        match server {
            Some(server) => service.main_pid = Some(server.pid),
            None if !launchers.is_empty() => service.state = ServiceState::Restarting,
            None => {}
        }
        service
    }

    pub fn listeners(&self) -> Listeners { port::listeners(&self.host, self.os, self.port) }

    fn failure(&self, service: &Service) -> Option<Failure> {
        let listeners = self.listeners();
        let diary = diary::last_failure(&diary::server_dir(&self.log_base));
        // O código de saída só explica uma parada que o gerenciador conta como falha: `systemctl
        // stop` de um servidor que demorou a sair (SIGKILL no fim do prazo) também deixa código.
        let failed = matches!(service.state, ServiceState::Failed | ServiceState::Restarting);
        let (code, detail, port_holder) = if let Some(holder) = listeners.holders.first() {
            ("port_unavailable".to_owned(), holder.name.clone(), Some(holder.clone()))
        } else if listeners.hidden {
            ("port_unavailable".to_owned(), String::new(), None)
        } else if let Some(failure) = diary {
            (failure.code, failure.detail, None)
        } else if service.state == ServiceState::Unreadable {
            ("service_unreadable".to_owned(), service.summary.clone(), None)
        } else if let Some(status) = service.exit_status.filter(|_| failed) {
            ("service_exit".to_owned(), status.to_string(), None)
        } else if failed {
            ("service_failed".to_owned(), service.summary.clone(), None)
        } else {
            // Parado limpo (alguém parou o serviço): nada a explicar.
            return None;
        };
        Some(Failure { code, detail, port_holder, output: self.output() })
    }

    /// As últimas linhas do que o serviço escreveu.
    pub fn output(&self) -> Vec<String> {
        let text = match self.os {
            Os::Linux => match self.host.run("journalctl", &["--user", "-u", service::UNIT, "-n", &OUTPUT_LINES.to_string(), "--no-pager", "-o", "cat"]) {
                Ok(out) if out.code == Some(0) => out.stdout,
                Ok(out) => return vec![format!("journalctl saiu com {:?}: {}", out.code, out.stderr.trim())],
                Err(e) => return vec![format!("journalctl não rodou: {e}")],
            },
            Os::Windows => match newest_backend_log(&self.log_base.join("privado")) {
                Some(path) => read_tail(&path),
                None => return vec![format!("nenhum hangar-backend*.log em {}", self.log_base.join("privado").display())],
            },
            Os::Macos => read_tail(&self.log_base.join("backend-launchd.log")),
            Os::Other => String::new(),
        };
        let lines: Vec<String> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).map(str::to_owned).collect();
        lines[lines.len().saturating_sub(OUTPUT_LINES)..].to_vec()
    }

    pub fn versions(&self) -> Versions { versions::read(&self.hangar()) }

    /// Sobe o serviço pelo gerenciador.
    pub fn start(&self) -> Result<StartOutcome, ActionError> {
        let result = self.start_service();
        let outcome = match &result { Ok(StartOutcome::StillStarting) => "ainda_subindo", _ => "" };
        self.record("start", result.as_ref().err(), &[("resultado", outcome)]);
        result
    }

    fn start_service(&self) -> Result<StartOutcome, ActionError> {
        let out = match self.os {
            Os::Linux => self.host.run("systemctl", &["--user", "start", service::UNIT]),
            Os::Windows => service::powershell(&self.host, &service::guarded(&format!("Start-ScheduledTask -TaskName '{}' -ErrorAction Stop", service::TASK))),
            Os::Macos => {
                let target = service::launchd_target(&self.host).ok_or(ActionError::Unsupported)?;
                self.host.run("launchctl", &["kickstart", &target])
            }
            Os::Other => return Err(ActionError::Unsupported),
        }.map_err(ActionError::Io)?;
        if out.timed_out { return Ok(StartOutcome::StillStarting); }
        match out.code {
            Some(0) => Ok(StartOutcome::Started),
            code => Err(ActionError::Command { code, output: format!("{}{}", out.stdout, out.stderr).trim().to_owned() }),
        }
    }

    /// Encerra quem segura a porta e os filhos dele, só se no momento ele ainda escuta nela, é do
    /// mesmo usuário e tem o nascimento lido no diagnóstico (o pid pode ter sido reusado). O
    /// nascimento é reconferido logo antes de cada sinal.
    pub fn free_port(&self, holder: &PortHolder) -> Result<(), ActionError> {
        let result = self.free_port_checked(holder);
        self.record("free_port", result.as_ref().err(), &[("processo", &holder.name), ("alvo_pid", &holder.pid.to_string())]);
        result
    }

    fn free_port_checked(&self, holder: &PortHolder) -> Result<(), ActionError> {
        if !self.holds(holder.pid) { return Err(ActionError::HolderChanged); }
        let me = self.host.user();
        let same = || -> Result<String, ActionError> {
            let process = self.host.process(holder.pid).ok_or(ActionError::HolderChanged)?;
            if process.start_time != holder.start_time { return Err(ActionError::HolderChanged); }
            if me.is_none() || process.user != me { return Err(ActionError::OtherUser); }
            Ok(process.name)
        };
        // Um servidor do Hangar preso na porta sai sozinho: os filhos dele são as sessões (os
        // servidores do psmux no Windows, os canos das sessões sem terminal), e a árvore as levaria.
        let whole_tree = !same()?.to_ascii_lowercase().starts_with("hangar-");
        if self.os == Os::Windows {
            // À força: no Windows não há pedido de saída para um processo sem janela.
            same()?;
            let pid = holder.pid.to_string();
            let args: &[&str] = if whole_tree { &["/PID", pid.as_str(), "/T", "/F"] } else { &["/PID", pid.as_str(), "/F"] };
            let out = self.host.run("taskkill", args).map_err(ActionError::Io)?;
            // O código do `taskkill` não diz o que aconteceu (128 sem o processo): quem decide é a porta.
            return if self.released(holder.pid, RELEASE_WAIT) { self.vacated() } else if out.code == Some(0) {
                Err(ActionError::PortStillHeld)
            } else {
                Err(ActionError::Command { code: out.code, output: format!("{}{}", out.stdout, out.stderr).trim().to_owned() })
            };
        }
        let tree = if whole_tree { self.tree(holder.pid, me.as_deref()) } else { Vec::new() };
        same()?;
        if !self.host.signal(holder.pid, false) { return Err(ActionError::TerminateRefused); }
        for child in &tree { if self.still(child, me.as_deref()) { self.host.signal(child.pid, false); } }
        if self.released(holder.pid, RELEASE_WAIT) { return self.vacated(); }
        // Não saiu com o pedido: à força, se ainda é o mesmo processo.
        match same() {
            Ok(_) => { self.host.signal(holder.pid, true); }
            // Saiu (ou o pid já é outro) e a porta continua presa: quem segura agora é outro.
            Err(ActionError::HolderChanged) => return Err(ActionError::PortStillHeld),
            Err(e) => return Err(e),
        }
        for child in &tree { if self.still(child, me.as_deref()) { self.host.signal(child.pid, true); } }
        if self.released(holder.pid, KILL_WAIT) { self.vacated() } else { Err(ActionError::PortStillHeld) }
    }

    /// Quem segurava saiu; a porta só está livre quando ninguém mais escuta nela: um filho que
    /// herdou o socket, ou outro processo que entrou no lugar. Consulta que falha não é porta livre.
    fn vacated(&self) -> Result<(), ActionError> {
        let mut waited = Duration::ZERO;
        loop {
            let now = self.listeners();
            if now.error.is_none() && now.is_empty() { return Ok(()); }
            if waited >= KILL_WAIT {
                return Err(match now.error {
                    Some(error) => ActionError::Io(std::io::Error::other(error)),
                    None => ActionError::PortStillHeld,
                });
            }
            self.host.pause(STEP);
            waited += STEP;
        }
    }

    fn holds(&self, pid: u32) -> bool { self.listeners().holders.iter().any(|h| h.pid == pid) }

    fn released(&self, pid: u32, wait: Duration) -> bool {
        let mut waited = Duration::ZERO;
        loop {
            if !self.holds(pid) { return true; }
            if waited >= wait { return false; }
            self.host.pause(STEP);
            waited += STEP;
        }
    }

    /// Os descendentes de `root` do mesmo usuário: os filhos herdam o socket e seguram a porta.
    fn tree(&self, root: u32, me: Option<&str>) -> Vec<ProcessInfo> {
        let processes = self.host.processes();
        let mut found: Vec<ProcessInfo> = Vec::new();
        let mut pending = vec![root];
        while let Some(parent) = pending.pop() {
            for child in processes.iter().filter(|p| p.parent == Some(parent) && me.is_some() && p.user.as_deref() == me) {
                if child.pid != root && !found.iter().any(|f| f.pid == child.pid) {
                    found.push(child.clone());
                    pending.push(child.pid);
                }
            }
        }
        found
    }

    /// O filho ainda é o lido em `tree`: na espera o pid pode ter ido para outro processo.
    fn still(&self, child: &ProcessInfo, me: Option<&str>) -> bool {
        self.host.process(child.pid).is_some_and(|now| now.start_time == child.start_time && me.is_some() && now.user.as_deref() == me)
    }

    /// Aponta o `current` para `version` e sobe. Segura a trava da atualização e exige o servidor
    /// parado com serviço instalado; a troca é a do atualizador. Depois da subida confere que o
    /// `current` e a versão que responde são `version`. Não existe no macOS, que não roda pacote.
    pub fn use_version(&self, version: &str) -> Result<StartOutcome, ActionError> {
        let from = self.versions().current.unwrap_or_default();
        let result = self.use_version_checked(version);
        self.record("use_version", result.as_ref().err(), &[("de", &from), ("para", version)]);
        result?;
        let started = self.start()?;
        let confirmed = self.confirm_version(version);
        if let Err(e) = &confirmed { self.record("use_version_confirm", Some(e), &[("para", version)]); }
        confirmed.map(|()| started)
    }

    fn use_version_checked(&self, version: &str) -> Result<(), ActionError> {
        if !matches!(self.os, Os::Linux | Os::Windows) { return Err(ActionError::Unsupported); }
        if self.versions().dev_mode { return Err(ActionError::DevMode); }
        let hangar = self.hangar();
        let dir = versions::versions_dir(&hangar).join(version);
        if versions::parse(version).is_none() || !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(ActionError::UnknownVersion(version.to_owned()));
        }
        if !versions::runnable(&hangar, version) { return Err(ActionError::NoBinary(version.to_owned())); }
        let _lock = UpdateLock::try_acquire(&hangar).map_err(|e| match e {
            LockError::Locked => ActionError::Locked,
            LockError::Io(e) => ActionError::Io(e),
        })?;
        if !matches!(self.answer(), Answer::Silent) { return Err(ActionError::ServerRunning); }
        let service = self.service_now();
        match service.state {
            // Sem quem suba, a troca deixaria o `current` noutra versão e o servidor ainda parado.
            ServiceState::Missing => return Err(ActionError::NotInstalled),
            ServiceState::Unreadable => return Err(ActionError::ServiceUnreadable(service.summary)),
            ServiceState::Running | ServiceState::Starting => return Err(ActionError::ServerRunning),
            // Com o serviço religando, a troca vale na próxima tentativa; `confirm_version` pega a
            // subida da versão antiga entre esta conferência e a troca.
            ServiceState::Restarting | ServiceState::Stopped | ServiceState::Failed => {}
        }
        let previous = link::swap(&hangar, &dir).map_err(ActionError::Io)?;
        drop_unproven_or_undo(&hangar, version, previous, link::remove_unproven, link::swap)
    }

    /// O `current` segue em `version` e quem responde na porta é ela. Ninguém respondendo no prazo
    /// é a subida ainda em curso: o painel segue pelo `probe`.
    fn confirm_version(&self, version: &str) -> Result<(), ActionError> {
        let now = self.versions().current;
        if now.as_deref() != Some(version) { return Err(ActionError::CurrentChanged(now.unwrap_or_default())); }
        let mut waited = Duration::ZERO;
        loop {
            match self.answer() {
                Answer::Hangar { version: Some(running), .. } if running == version => return Ok(()),
                Answer::Hangar { version: running, .. } => return Err(ActionError::VersionMismatch(running.unwrap_or_default())),
                Answer::Other | Answer::Silent => {}
            }
            if waited >= VERSION_WAIT { return Ok(()); }
            self.host.pause(VERSION_STEP);
            waited += VERSION_STEP;
        }
    }

    /// A falha de gravar não muda o resultado da ação: vai ao stderr do app.
    fn record(&self, action: &str, error: Option<&ActionError>, fields: &[(&str, &str)]) {
        let detail = error.map(ActionError::detail).unwrap_or_default();
        let fields: Vec<(&str, &str)> = fields.iter().copied().chain([("detalhe", detail.as_str())]).collect();
        if let Err(e) = diary::record_action(&self.log_base, action, error.map(ActionError::code), &fields) {
            eprintln!("hangar-local-server: local_server.acao {action} não gravado no diário ({:?})", e.kind());
        }
    }
}

/// O registro da troca sem prova é da atualização que não se provou. Com a versão escolhida à mão
/// (e a trava conosco, nenhuma atualização no meio), a próxima recuperação o usaria para desfazer a
/// escolha quando ela é a própria versão do registro; nas outras ela só o apagaria. Sem apagá-lo, o
/// `current` volta para `previous`; se não voltar, o erro diz que ficou em `version`.
fn drop_unproven_or_undo(hangar: &Path, version: &str, previous: Option<PathBuf>,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
    swap: impl FnOnce(&Path, &Path) -> std::io::Result<Option<PathBuf>>) -> Result<(), ActionError> {
    let Err(e) = remove(hangar) else { return Ok(()) };
    let left = |cause: String| ActionError::CurrentLeftOn { version: version.to_owned(), cause };
    match previous {
        Some(previous) => match swap(hangar, &previous) {
            Ok(_) => Err(ActionError::Io(e)),
            Err(undo) => Err(left(format!("{e}; a volta para {} falhou: {undo}", previous.display()))),
        },
        // Não havia `current` antes: não há para onde voltar.
        None => Err(left(e.to_string())),
    }
}

/// `hangar-server.exe` ou `hangar-launcher.exe` com `launch backend` (o lançador da tarefa).
fn is_backend_launcher(name: &str, cmd: &[String]) -> bool {
    let name = name.to_ascii_lowercase();
    (name.starts_with("hangar-server") || name.starts_with("hangar-launcher"))
        && cmd.windows(2).any(|pair| pair[0] == "launch" && pair[1] == "backend")
}

fn newest_backend_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("hangar-backend") && n.ends_with(".log")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(modified, _)| *modified).map(|(_, path)| path)
}

/// O fim do arquivo (64 KiB bastam para 40 linhas).
fn read_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map_or(0, |m| m.len());
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(len.saturating_sub(64 * 1024))).is_err() || file.read_to_end(&mut bytes).is_err() { return String::new(); }
    String::from_utf8_lossy(&bytes).into_owned()
}
