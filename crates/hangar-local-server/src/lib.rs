//! O servidor desta máquina sem a API dele: estado do serviço, motivo da falha, versões instaladas,
//! o `~/.hangar/current` e as ações que consertam (subir, liberar a porta, apontar para outra
//! versão). Sem `hangar-core`: o app nativo usa este crate quando o servidor está parado, e o
//! servidor usa as mesmas primitivas de link e de leitura. Nenhuma ação tem rota HTTP.
//!
//! Tudo o que sai do processo passa por `host::Host`; tudo é bloqueante.
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
pub use lock::{LockError, UpdateLock};
pub use port::{Listeners, PortHolder};
pub use service::{Os, Service, ServiceState};
pub use versions::Versions;

pub const DEFAULT_PORT: u16 = 8765;
const OUTPUT_LINES: usize = 40;
/// Quanto `free_port` espera a porta soltar depois de encerrar o processo.
const RELEASE_WAIT: Duration = Duration::from_secs(5);
const RELEASE_STEP: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalState {
    Running,
    /// A saúde responde e o resto ainda devolve 503 `server_starting`, ou o gerenciador diz que
    /// está subindo e a porta ainda não atende.
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

#[derive(Debug)]
pub enum ActionError {
    /// Sem gerenciador conhecido, ou troca de versão no macOS (sem pacote).
    Unsupported,
    DevMode,
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
    Command { code: Option<i32>, output: String },
    Io(std::io::Error),
}

impl ActionError {
    pub fn code(&self) -> &'static str {
        match self {
            ActionError::Unsupported => "local_server_unsupported",
            ActionError::DevMode => "local_server_dev_mode",
            ActionError::Locked => "update_locked",
            ActionError::ServerRunning => "local_server_running",
            ActionError::UnknownVersion(_) => "local_server_unknown_version",
            ActionError::NoBinary(_) => "local_server_no_binary",
            ActionError::HolderChanged => "local_server_holder_changed",
            ActionError::OtherUser => "local_server_other_user",
            ActionError::TerminateRefused => "local_server_terminate_refused",
            ActionError::PortStillHeld => "local_server_port_still_held",
            ActionError::Command { .. } => "local_server_command_failed",
            ActionError::Io(_) => "local_server_io",
        }
    }

    /// O dado para o detalhe da tela e do diário.
    pub fn detail(&self) -> String {
        match self {
            ActionError::UnknownVersion(v) | ActionError::NoBinary(v) => v.clone(),
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
}

/// O que respondeu na porta.
enum Answer {
    Hangar { starting: bool },
    Other,
    Silent,
}

impl<H: Host> LocalServer<H> {
    pub fn hangar(&self) -> PathBuf { self.home.join(".hangar") }

    pub fn probe(&self) -> LocalState {
        match self.answer() {
            Answer::Hangar { starting: true } => return LocalState::Starting,
            Answer::Hangar { starting: false } => return LocalState::Running,
            Answer::Other | Answer::Silent => {}
        }
        let service = self.service_now();
        match service.state {
            ServiceState::Missing => LocalState::NotInstalled,
            // De pé no gerenciador e a porta ainda sem resposta: a subida antes de abrir a porta.
            ServiceState::Starting | ServiceState::Running => LocalState::Starting,
            _ => LocalState::Stopped { failure: self.failure(&service) },
        }
    }

    /// A saúde só diz "o hangar-server está de pé"; quem diz se a subida acabou é uma rota comum,
    /// que responde 503 `server_starting` até lá (a trava da subida vem antes da autenticação).
    fn answer(&self) -> Answer {
        match self.host.get(self.port, "/__hangar_server/health") {
            None => Answer::Silent,
            Some(reply) if reply.status == 200 && serde_json::from_str::<serde_json::Value>(&reply.body).is_ok_and(|v| v.get("protocol").is_some()) => {
                let starting = self.host.get(self.port, "/api/client-api").is_some_and(|reply| reply.status == 503
                    && serde_json::from_str::<serde_json::Value>(&reply.body).is_ok_and(|v| v.get("detail").and_then(serde_json::Value::as_str) == Some("server_starting")));
                Answer::Hangar { starting }
            }
            Some(_) => Answer::Other,
        }
    }

    /// A leitura crua do gerenciador.
    pub fn service(&self) -> Service { service::read(&self.host, self.os) }

    /// A leitura corrigida: no Windows a tarefa segue `Running` com o lançador esperando para religar.
    fn service_now(&self) -> Service {
        let mut service = self.service();
        if service.state == ServiceState::Running && self.launcher_waiting() { service.state = ServiceState::Restarting; }
        service
    }

    pub fn listeners(&self) -> Listeners { port::listeners(&self.host, self.os, self.port) }

    /// Windows: o lançador de pé sem um `hangar-server` filho está na espera entre uma queda e a
    /// religada. Nos outros sistemas o gerenciador já diz isso (`auto-restart`, `spawn scheduled`).
    fn launcher_waiting(&self) -> bool {
        if self.os != Os::Windows { return false; }
        let processes = self.host.processes();
        let launchers: Vec<u32> = processes.iter().filter(|p| is_backend_launcher(&p.name, &p.cmd)).map(|p| p.pid).collect();
        !launchers.is_empty() && !processes.iter().any(|p| p.parent.is_some_and(|parent| launchers.contains(&parent))
            && p.name.to_ascii_lowercase().starts_with("hangar-server"))
    }

    fn failure(&self, service: &Service) -> Option<Failure> {
        let listeners = self.listeners();
        let diary = diary::last_failure(&diary::server_dir(&self.log_base));
        let (code, detail, port_holder) = if let Some(holder) = listeners.holders.first() {
            ("port_unavailable".to_owned(), holder.name.clone(), Some(holder.clone()))
        } else if listeners.hidden {
            ("port_unavailable".to_owned(), String::new(), None)
        } else if let Some(failure) = diary {
            (failure.code, failure.detail, None)
        } else if service.state == ServiceState::Unreadable {
            ("service_unreadable".to_owned(), service.summary.clone(), None)
        } else if let Some(status) = service.exit_status {
            ("service_exit".to_owned(), status.to_string(), None)
        } else if matches!(service.state, ServiceState::Failed | ServiceState::Restarting) {
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
    pub fn start(&self) -> Result<(), ActionError> {
        let result = self.start_service();
        self.record("start", &result, &[]);
        result
    }

    fn start_service(&self) -> Result<(), ActionError> {
        let out = match self.os {
            Os::Linux => self.host.run("systemctl", &["--user", "start", service::UNIT]),
            Os::Windows => service::powershell(&self.host, &service::guarded(&format!("Start-ScheduledTask -TaskName '{}' -ErrorAction Stop", service::TASK))),
            Os::Macos => {
                let target = service::launchd_target(&self.host).ok_or(ActionError::Unsupported)?;
                self.host.run("launchctl", &["kickstart", &target])
            }
            Os::Other => return Err(ActionError::Unsupported),
        }.map_err(ActionError::Io)?;
        match out.code {
            Some(0) => Ok(()),
            code => Err(ActionError::Command { code, output: format!("{}{}", out.stdout, out.stderr).trim().to_owned() }),
        }
    }

    /// Encerra quem segura a porta, só se no momento ele ainda escuta nela, é do mesmo usuário e tem
    /// o nascimento lido no diagnóstico (o pid pode ter sido reusado).
    pub fn free_port(&self, holder: &PortHolder) -> Result<(), ActionError> {
        let result = self.free_port_checked(holder);
        self.record("free_port", &result, &[("processo", &holder.name), ("alvo_pid", &holder.pid.to_string())]);
        result
    }

    fn free_port_checked(&self, holder: &PortHolder) -> Result<(), ActionError> {
        let now = self.listeners();
        if !now.holders.iter().any(|h| h.pid == holder.pid) { return Err(ActionError::HolderChanged); }
        let process = self.host.process(holder.pid).ok_or(ActionError::HolderChanged)?;
        if process.start_time != holder.start_time { return Err(ActionError::HolderChanged); }
        let me = self.host.user();
        if me.is_none() || process.user != me { return Err(ActionError::OtherUser); }
        if !self.host.terminate(holder.pid) { return Err(ActionError::TerminateRefused); }
        let mut waited = Duration::ZERO;
        loop {
            if !self.listeners().holders.iter().any(|h| h.pid == holder.pid) { return Ok(()); }
            if waited >= RELEASE_WAIT { return Err(ActionError::PortStillHeld); }
            self.host.pause(RELEASE_STEP);
            waited += RELEASE_STEP;
        }
    }

    /// Aponta o `current` para `version` e sobe. Segura a trava da atualização e exige o servidor
    /// parado; a troca é a do atualizador. Não existe no macOS, que não roda pacote.
    pub fn use_version(&self, version: &str) -> Result<(), ActionError> {
        let from = self.versions().current.unwrap_or_default();
        let result = self.use_version_checked(version);
        self.record("use_version", &result, &[("de", &from), ("para", version)]);
        result?;
        self.start()
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
        // Com o lançador religando, a troca vale na próxima tentativa dele: só a vida de pé recusa.
        if !matches!(self.answer(), Answer::Silent) || matches!(self.service_now().state, ServiceState::Running | ServiceState::Starting) {
            return Err(ActionError::ServerRunning);
        }
        link::swap(&hangar, &dir).map(|_| ()).map_err(ActionError::Io)
    }

    /// A falha de gravar não muda o resultado da ação: vai ao stderr do app.
    fn record(&self, action: &str, result: &Result<(), ActionError>, fields: &[(&str, &str)]) {
        let error = result.as_ref().err();
        let detail = error.map(ActionError::detail).unwrap_or_default();
        let fields: Vec<(&str, &str)> = fields.iter().copied().chain([("detalhe", detail.as_str())]).collect();
        if let Err(e) = diary::record_action(&self.log_base, action, error.map(ActionError::code), &fields) {
            eprintln!("hangar-local-server: local_server.acao {action} não gravado no diário ({:?})", e.kind());
        }
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
