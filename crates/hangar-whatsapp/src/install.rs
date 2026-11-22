//! Instalar, atualizar e remover o serviço `hangar-whatsapp`, uma tarefa por vez, com o andamento
//! consultável pela tela.
//!
//! O programa, o registro da instalação e o aceite moram em `~/.hangar/whatsapp-service/`; os dados do
//! binário (`--data`), em `~/.hangar/whatsapp/`. Remover o programa nunca apaga os dados.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hangar_managed_binary::service::{self, Arg, Service, ServiceId};
use serde_json::{Value, json};

use crate::client::{CallError, Client};
use crate::release::{self, Github, Platform, Release};

const START_TIMEOUT: Duration = Duration::from_secs(30);

pub const SERVICE: ServiceId = ServiceId {
    unit: "hangar-whatsapp.service",
    launchd_label: "dev.hangar.whatsapp",
    task: "hangar-whatsapp",
    description: "WhatsApp do Hangar (somente leitura)",
};

/// Versão do texto de risco que a pessoa aceitou.
pub const ACCEPT_TEXT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `--data` do binário: bancos, sessão do aparelho, mídia, `control.json` e `control.key`.
    pub data: PathBuf,
    /// Programa (`bin/<versão>/`), registro da instalação e aceite.
    pub service: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Context {
    pub paths: Paths,
    pub platform: Platform,
    pub github: Github,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tag: String,
    pub binary: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Download,
    Service,
    Start,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed { tag: String },
    Updated { tag: String },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

/// Uma instalação, atualização ou remoção por vez; o andamento fica em memória para o `GET` da tela.
#[derive(Default)]
pub struct Installer {
    state: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    job: Option<Job>,
    /// Uma operação síncrona (remover, apagar com o serviço parado) em curso.
    inline: bool,
}

#[derive(Clone, Debug)]
struct Job {
    kind: &'static str,
    step: Step,
    result: Option<Result<Outcome, Failure>>,
}

/// Enquanto viver, nenhuma tarefa começa.
pub struct InlineGuard<'a>(&'a Installer);

impl Paths {
    pub fn new(home: &Path) -> Self {
        let hangar = home.join(".hangar");
        Self { home: home.to_path_buf(), data: hangar.join("whatsapp"), service: hangar.join("whatsapp-service") }
    }

    pub fn record(&self) -> PathBuf {
        self.service.join("install.json")
    }

    pub fn accepted(&self) -> PathBuf {
        self.service.join("accepted.json")
    }
}

impl Context {
    pub fn new(home: &Path) -> Self {
        Self { paths: Paths::new(home), platform: Platform::local(), github: Github::default() }
    }

    fn service(&self, binary: &Path) -> Service {
        Service {
            id: SERVICE,
            executable: binary.to_path_buf(),
            args: vec![Arg::Plain("--data".into()), Arg::Path(self.paths.data.clone())],
        }
    }
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Service => "service",
            Self::Start => "start",
        }
    }
}

impl Failure {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self { code, detail: detail.into() }
    }
}

impl From<hangar_managed_binary::release::Error> for Failure {
    fn from(e: hangar_managed_binary::release::Error) -> Self {
        let detail = format!("{e:?}");
        Self::new(e.code(&release::CODES), detail)
    }
}

impl From<service::Error> for Failure {
    fn from(e: service::Error) -> Self {
        Self::new(e.code(&release::CODES), format!("{}: {}", e.step, e.detail))
    }
}

impl Installer {
    /// Começa `install` ou `update` em segundo plano.
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn start(self: &Arc<Self>, kind: &'static str, ctx: Context) -> Result<(), Busy> {
        {
            let mut slot = self.slot();
            if slot.inline || slot.job.as_ref().is_some_and(|j| j.result.is_none()) {
                return Err(Busy);
            }
            slot.job = Some(Job { kind, step: Step::Download, result: None });
        }
        let this = self.clone();
        tokio::spawn(async move {
            let progress_owner = this.clone();
            let work = tokio::spawn(async move {
                let progress = move |step: Step| {
                    if let Some(job) = progress_owner.slot().job.as_mut() {
                        job.step = step;
                    }
                };
                match kind {
                    "update" => update(&ctx, &progress).await,
                    _ => install(&ctx, &progress).await,
                }
            });
            // Tarefa que cai no meio também termina: sem resultado, toda tentativa seguinte veria "ocupado".
            let result = work.await.unwrap_or_else(|error| {
                tracing::error!(kind, %error, "whatsapp: a tarefa de instalação parou no meio");
                Err(Failure::new("whatsapp_job_crashed", format!("a tarefa parou no meio: {error}")))
            });
            if let Some(job) = this.slot().job.as_mut() {
                job.result = Some(result);
            }
        });
        Ok(())
    }

    /// Reserva a vez para uma operação que a rota faz na hora (remover, apagar com o serviço parado).
    ///
    /// # Errors
    ///
    /// [`Busy`] com outra tarefa ou operação em curso.
    pub fn inline(&self) -> Result<InlineGuard<'_>, Busy> {
        let mut slot = self.slot();
        if slot.inline || slot.job.as_ref().is_some_and(|j| j.result.is_none()) {
            return Err(Busy);
        }
        slot.inline = true;
        Ok(InlineGuard(self))
    }

    /// Há tarefa ou operação em curso agora; quem grava algo antes de [`Installer::start`] pergunta aqui.
    pub fn busy(&self) -> bool {
        let slot = self.slot();
        slot.inline || slot.job.as_ref().is_some_and(|j| j.result.is_none())
    }

    /// Esquece a última tarefa: depois de remover, o resultado da instalação não vale mais.
    pub fn forget(&self) {
        let mut slot = self.slot();
        if slot.job.as_ref().is_some_and(|j| j.result.is_some()) {
            slot.job = None;
        }
    }

    /// `{kind,step,done,error,outcome}` ou `null`.
    pub fn job(&self) -> Value {
        let slot = self.slot();
        let Some(job) = slot.job.as_ref() else { return Value::Null };
        let (done, error, outcome) = match &job.result {
            None => (false, Value::Null, Value::Null),
            Some(Ok(outcome)) => (true, Value::Null, json!(match outcome {
                Outcome::Installed { .. } => "installed",
                Outcome::Updated { .. } => "updated",
            })),
            Some(Err(f)) => (true, json!({"code": f.code, "detail": f.detail}), Value::Null),
        };
        json!({"kind": job.kind, "step": job.step.name(), "done": done, "error": error, "outcome": outcome})
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for InlineGuard<'_> {
    fn drop(&mut self) {
        self.0.slot().inline = false;
    }
}

/// A instalação que o registro aponta, com o programa no disco.
pub fn current(paths: &Paths) -> Option<Installed> {
    let raw = std::fs::read(paths.record()).ok()?;
    let record: Value = serde_json::from_slice(&raw).ok()?;
    let tag = record["tag"].as_str().filter(|t| !t.is_empty())?.to_owned();
    let binary = PathBuf::from(record["binary"].as_str()?);
    binary.is_file().then_some(Installed { tag, binary })
}

/// Quando a pessoa aceitou o aviso de risco, ou `None`.
pub fn accepted_at(paths: &Paths) -> Option<String> {
    let raw = std::fs::read(paths.accepted()).ok()?;
    let value: Value = serde_json::from_slice(&raw).ok()?;
    value["at"].as_str().map(str::to_owned)
}

/// Grava `accepted.json` `{"at","text_version"}` com a hora de agora.
///
/// # Errors
///
/// `whatsapp_accept_write_failed` quando o arquivo não grava.
pub fn accept(paths: &Paths) -> Result<(), Failure> {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let body = json!({"at": at, "text_version": ACCEPT_TEXT_VERSION}).to_string();
    write_file(&paths.accepted(), body.as_bytes()).map_err(|e| Failure::new("whatsapp_accept_write_failed", e))
}

/// Instala (ou religa) e espera o serviço responder com a assinatura certa.
///
/// Já instalado e parado: registra o serviço de novo e o sobe, sem baixar nada.
///
/// # Errors
///
/// Falha do download, da conferência do `--version`, do registro do serviço ou da subida.
pub async fn install(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let client = Client::new(ctx.paths.data.clone());
    // De pé só com um status assinado: pid vivo no `control.json` pode ser outro processo.
    if let Some(found) = current(&ctx.paths) && client.status().await.is_ok() {
        return Ok(Outcome::Installed { tag: found.tag });
    }
    let installed = install_stopped(ctx, progress).await?;
    progress(Step::Start);
    let version = release::check(&installed.binary).await?;
    service::start(&ctx.paths.home, &SERVICE).await?;
    wait_up(&client, &version, None).await?;
    Ok(Outcome::Installed { tag: installed.tag })
}

/// Baixa (se preciso), confere a API e registra o serviço sem subir o programa. A migração usa este passo
/// antes de importar os dados; depois chama [`start`].
///
/// No Windows, registrar a tarefa interrompe o programa que já roda da pasta do serviço, e nada sobe de
/// novo: quem chama sobe o serviço depois, por [`start`].
///
/// # Errors
///
/// Falha do download, da conferência do `--version`, do registro da instalação ou do serviço.
pub async fn install_stopped(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Installed, Failure> {
    let installed = match current(&ctx.paths) {
        Some(found) => found,
        None => {
            progress(Step::Download);
            let latest = release::latest(&ctx.github).await?;
            let binary = release::BINARY.fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.service).await?;
            release::check(&binary).await?;
            Installed { tag: latest.tag, binary }
        }
    };
    progress(Step::Service);
    // O registro vem antes do serviço: se o serviço falhar, a próxima tentativa já acha o programa baixado.
    write_record(&ctx.paths, &installed)?;
    service::install(&ctx.paths.home, &ctx.paths.service, &ctx.service(&installed.binary)).await?;
    Ok(installed)
}

/// Sobe o serviço já instalado e espera ele responder.
///
/// # Errors
///
/// `whatsapp_not_installed` sem instalação; falha do gerenciador de serviços ou da subida.
pub async fn start(ctx: &Context) -> Result<(), Failure> {
    let installed = current(&ctx.paths).ok_or_else(not_installed)?;
    let version = release::check(&installed.binary).await?;
    service::start(&ctx.paths.home, &SERVICE).await?;
    wait_up(&Client::new(ctx.paths.data.clone()), &version, None).await
}

/// Troca pela release mais nova da mesma versão maior; se ela não responder, volta para a anterior.
///
/// # Errors
///
/// `whatsapp_not_installed`; falha do download, da conferência ou da subida (com a volta tentada).
pub async fn update(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let old = current(&ctx.paths).ok_or_else(not_installed)?;
    progress(Step::Download);
    let latest: Release = release::latest(&ctx.github).await?;
    if !release::newer(&latest.tag, &old.tag) {
        return Ok(Outcome::Updated { tag: old.tag });
    }
    let binary = release::BINARY.fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.service).await?;
    let version = release::check(&binary).await?;
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    progress(Step::Service);
    service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&binary)).await?;
    progress(Step::Start);
    // O registro só passa para a versão nova depois de ela responder: senão a próxima tentativa diria
    // "atualizado" sem nada rodando.
    if let Err(failure) = wait_up(&client, &version, before).await {
        let back = service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await;
        return Err(match back {
            Ok(()) => failure,
            Err(e) => Failure::new("whatsapp_update_no_rollback",
                format!("{}; e o serviço não voltou para {}: {}: {}", failure.detail, old.tag, e.step, e.detail)),
        });
    }
    let new = Installed { tag: latest.tag.clone(), binary: binary.clone() };
    write_record(&ctx.paths, &new)?;
    if let Some(dir) = old.binary.parent().filter(|d| Some(*d) != binary.parent())
        && let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "whatsapp: atualizado, mas a pasta da versão anterior ficou");
    }
    Ok(Outcome::Updated { tag: latest.tag })
}

/// Tira o serviço e apaga o programa e o registro. Os dados (`--data`) e o aceite ficam.
///
/// # Errors
///
/// Falha do gerenciador de serviços ou ao apagar a pasta do programa.
pub async fn uninstall(ctx: &Context) -> Result<(), Failure> {
    service::unregister(&ctx.paths.home, &ctx.paths.service, &SERVICE).await?;
    let gone = |path: &Path, e: std::io::Error| Failure::new("whatsapp_remove_failed", format!("{}: {e}", path.display()));
    let bin = ctx.paths.service.join("bin");
    match std::fs::remove_dir_all(&bin) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(gone(&bin, e)),
        _ => {}
    }
    match std::fs::remove_file(ctx.paths.record()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(gone(&ctx.paths.record(), e)),
        _ => Ok(()),
    }
}

/// Algum processo do serviço existe pelo gerenciador, ou vai existir logo (o `Restart=always` esperando
/// o `RestartSec`), mesmo sem `control.json`. Sem como perguntar (Windows), `false`: lá o arquivo aberto
/// não se apaga, e quem apaga diz o que já saiu.
pub async fn service_process(paths: &Paths) -> bool {
    if cfg!(target_os = "macos") {
        let output = tokio::process::Command::new("launchctl").args(["list", SERVICE.launchd_label])
            .stdin(std::process::Stdio::null()).output().await;
        return output.is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("\"PID\" = "));
    }
    if cfg!(windows) || !service::systemd_unit_path(&paths.home, &SERVICE).is_file() {
        return false;
    }
    let output = tokio::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "MainPID", "-p", "ActiveState", "-p", "SubState", SERVICE.unit])
        .stdin(std::process::Stdio::null()).output().await;
    output.is_ok_and(|o| o.status.success() && systemd_busy(&String::from_utf8_lossy(&o.stdout)))
}

/// Saída `Chave=valor` do `systemctl show`: processo vivo, ou unit subindo, parando ou à espera de religar.
fn systemd_busy(show: &str) -> bool {
    let field = |key: &str| show.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).unwrap_or_default();
    !matches!(field("MainPID"), "" | "0")
        || matches!(field("ActiveState"), "active" | "activating" | "reloading" | "deactivating" | "refreshing")
        || field("SubState") == "auto-restart"
}

/// Espera o processo novo responder ao `status` com assinatura válida e a versão esperada.
async fn wait_up(client: &Client, version: &str, before: Option<u32>) -> Result<(), Failure> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut last = String::from("sem control.json");
    while Instant::now() < deadline {
        match client.claimed() {
            Some(control) if Some(control.pid) == before => last = "ainda o processo anterior".into(),
            Some(_) => match client.status().await {
                Ok(status) => match status["version"].as_str() {
                    Some(running) if running != version => last = format!("respondeu a versão {running}"),
                    _ => return Ok(()),
                },
                Err(CallError::Off(detail) | CallError::Untrusted(detail) | CallError::Transport(detail)) => last = detail,
            },
            None => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(Failure::new("whatsapp_not_started", format!("o serviço não respondeu em {}s ({last})", START_TIMEOUT.as_secs())))
}

fn not_installed() -> Failure {
    Failure::new("whatsapp_not_installed", "o hangar-whatsapp não está instalado")
}

fn write_record(paths: &Paths, installed: &Installed) -> Result<(), Failure> {
    let body = json!({"tag": installed.tag, "binary": installed.binary.to_string_lossy()}).to_string();
    write_file(&paths.record(), body.as_bytes()).map_err(|e| Failure::new("whatsapp_record_failed", e))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("{}: {e}", path.display());
    std::fs::create_dir_all(path.parent().expect("arquivo tem pasta")).map_err(failed)?;
    hangar_core::fs::atomic_write(path, bytes).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_and_data_live_in_separate_folders() {
        let paths = Paths::new(Path::new("/home/u"));
        assert_eq!(paths.data, Path::new("/home/u/.hangar/whatsapp"));
        assert_eq!(paths.service, Path::new("/home/u/.hangar/whatsapp-service"));
        assert!(!paths.record().starts_with(&paths.data) && !paths.accepted().starts_with(&paths.data));
        let unit = service::systemd_unit(&Context::new(Path::new("/home/u")).service(Path::new("/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp")));
        assert!(unit.contains("ExecStart=\"/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp\" --data \"/home/u/.hangar/whatsapp\"\n"), "{unit}");
    }

    #[test]
    fn the_record_counts_only_with_the_program_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        assert_eq!(current(&paths), None);
        let binary = paths.service.join("bin/0.1.0/hangar-whatsapp");
        write_record(&paths, &Installed { tag: "v0.1.0".into(), binary: binary.clone() }).unwrap();
        assert_eq!(current(&paths), None, "programa ausente");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        assert_eq!(current(&paths), Some(Installed { tag: "v0.1.0".into(), binary }));
    }

    #[test]
    fn accepting_writes_the_time_and_the_text_version() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        assert_eq!(accepted_at(&paths), None);
        accept(&paths).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(paths.accepted()).unwrap()).unwrap();
        assert_eq!(saved["text_version"], 1);
        assert!(saved["at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(accepted_at(&paths).as_deref(), saved["at"].as_str());
    }

    #[test]
    fn a_unit_waiting_to_restart_counts_as_busy() {
        let show = |pid: &str, active: &str, sub: &str| format!("MainPID={pid}\nActiveState={active}\nSubState={sub}\n");
        assert!(systemd_busy(&show("4321", "active", "running")));
        assert!(systemd_busy(&show("0", "activating", "auto-restart")), "na janela do RestartSec o MainPID é 0");
        assert!(systemd_busy(&show("0", "deactivating", "stop-sigterm")));
        assert!(!systemd_busy(&show("0", "inactive", "dead")));
        assert!(!systemd_busy(&show("0", "failed", "failed")));
        assert!(!systemd_busy(""), "sem resposta do systemd");
    }

    #[test]
    fn one_job_or_inline_operation_at_a_time() {
        let installer = Installer::default();
        assert!(!installer.busy());
        let guard = installer.inline().unwrap();
        assert_eq!(installer.inline().err(), Some(Busy));
        assert!(installer.busy());
        drop(guard);
        installer.slot().job = Some(Job { kind: "install", step: Step::Service, result: None });
        assert_eq!(installer.inline().err(), Some(Busy));
        assert_eq!(installer.job(), json!({"kind": "install", "step": "service", "done": false, "error": null, "outcome": null}));
        installer.slot().job.as_mut().unwrap().result = Some(Err(Failure::new("whatsapp_hash", "x")));
        assert_eq!(installer.job()["error"], json!({"code": "whatsapp_hash", "detail": "x"}));
        assert!(installer.inline().is_ok());
        installer.forget();
        assert_eq!(installer.job(), Value::Null);
    }
}
