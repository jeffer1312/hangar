//! Instalar, atualizar e remover o serviço `hangar-whatsapp`, uma tarefa por vez, com o andamento
//! consultável pela tela.
//!
//! O programa, o registro da instalação e o aceite moram em `~/.hangar/whatsapp-service/`; os dados do
//! binário (`--data`), em `~/.hangar/whatsapp/`. Remover o programa nunca apaga os dados.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hangar_managed_binary::jobs::{self, Jobs};
use hangar_managed_binary::record;
use hangar_managed_binary::service::{self, Arg, Service, ServiceId};
use serde_json::{Value, json};

use crate::client::{CallError, Client};
use crate::release::{self, Github, Platform};

pub use hangar_managed_binary::jobs::Busy;
pub use hangar_managed_binary::record::Installed;

const START_TIMEOUT: Duration = Duration::from_secs(30);

pub const SERVICE: ServiceId = ServiceId {
    unit: "hangar-whatsapp.service",
    launchd_label: "dev.hangar.whatsapp",
    task: "hangar-whatsapp",
    description: "WhatsApp do Hangar",
    no_restart_exit: Some(SCHEMA_NEWER_EXIT),
};

/// O binário sai com este código quando o `messages.db` é de um esquema mais novo que o dele: só um binário
/// mais novo abre esses dados, e religar o mesmo não adianta.
pub const SCHEMA_NEWER_EXIT: u8 = 3;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    Updated,
    /// Atualizar não achou release mais nova que a instalada: nada mudou.
    AlreadyLatest,
}

/// Uma instalação, atualização ou remoção por vez; o andamento fica em memória para o `GET` da tela.
pub type Installer = Jobs<Step, Outcome, Failure>;

/// A vez guardada para uma instalação ou atualização que ainda vai começar ([`begin`]).
pub type Reservation = jobs::Reservation<Step, Outcome, Failure>;

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

impl Failure {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self { code, detail: detail.into() }
    }
}

impl jobs::Step for Step {
    fn name(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Service => "service",
            Self::Start => "start",
        }
    }
}

impl jobs::Outcome for Outcome {
    fn name(&self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::AlreadyLatest => "already_latest",
        }
    }
}

impl jobs::Failure for Failure {
    fn code(&self) -> &str {
        self.code
    }

    fn detail(&self) -> &str {
        &self.detail
    }

    fn crashed(kind: &'static str, error: &tokio::task::JoinError) -> Self {
        tracing::error!(kind, %error, "whatsapp: a tarefa de instalação parou no meio");
        Self::new("whatsapp_job_crashed", format!("a tarefa parou no meio: {error}"))
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

/// Começa `install` ou `update` em segundo plano.
///
/// # Errors
///
/// [`Busy`] com outra tarefa ou operação em curso.
pub fn spawn(installer: &Installer, kind: &'static str, ctx: Context) -> Result<(), Busy> {
    installer.reserve(kind, Step::Download).map(|reservation| begin(reservation, ctx))
}

/// Começa em segundo plano a tarefa da vez guardada (`install` ou `update`).
pub fn begin(reservation: Reservation, ctx: Context) {
    let kind = reservation.kind();
    drop(reservation.start(move |progress| async move {
        match kind {
            "update" => update(&ctx, &*progress).await,
            _ => install(&ctx, &*progress).await,
        }
    }));
}

/// A instalação que o registro aponta, com o programa no disco.
pub fn current(paths: &Paths) -> Option<Installed> {
    record::read(&paths.record())
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
    if current(&ctx.paths).is_some() && client.status().await.is_ok() {
        return Ok(Outcome::Installed);
    }
    let (installed, checked) = prepare(ctx, progress).await?;
    progress(Step::Start);
    let version = match checked {
        Some(version) => version,
        None => release::check(&installed.binary).await?,
    };
    service::start(&ctx.paths.home, &SERVICE).await?;
    wait_up(&client, &version, None).await?;
    Ok(Outcome::Installed)
}

/// Baixa (se preciso), confere a API e registra o serviço sem subir o programa. É a etapa da migração de
/// dados, feita à mão pelo operador: instala sem subir, importa os dados e então sobe por [`start`].
/// Nenhuma rota do Hangar chama esta etapa.
///
/// No Windows, registrar a tarefa interrompe o programa que já roda da pasta do serviço, e nada sobe de
/// novo: o serviço só sobe depois, por [`start`].
///
/// # Errors
///
/// Falha do download, da conferência do `--version`, do registro da instalação ou do serviço.
pub async fn install_stopped(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Installed, Failure> {
    prepare(ctx, progress).await.map(|(installed, _)| installed)
}

/// [`install_stopped`] com a versão do `--version`, quando o programa acabou de ser baixado e conferido.
async fn prepare(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<(Installed, Option<String>), Failure> {
    let (installed, checked) = match current(&ctx.paths) {
        Some(found) => (found, None),
        None => {
            progress(Step::Download);
            let latest = release::latest(&ctx.github).await?;
            let binary = release::BINARY.fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.service).await?;
            let version = release::check(&binary).await?;
            (Installed { tag: latest.tag, binary }, Some(version))
        }
    };
    progress(Step::Service);
    // O registro vem antes do serviço: se o serviço falhar, a próxima tentativa já acha o programa baixado.
    write_record(&ctx.paths, &installed)?;
    service::install(&ctx.paths.home, &ctx.paths.service, &ctx.service(&installed.binary)).await?;
    Ok((installed, checked))
}

/// Sobe o serviço já instalado e espera ele responder: o fim da migração de dados feita à mão pelo
/// operador, depois de [`install_stopped`] e da importação. Nenhuma rota do Hangar chama esta etapa.
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

/// Troca pela release mais nova da mesma versão maior; se ela não responder, volta para a anterior, salvo se
/// ela já abriu os dados.
///
/// # Errors
///
/// `whatsapp_not_installed`; falha do download, da conferência ou da subida (com a volta tentada);
/// `whatsapp_update_data_migrated` quando a nova já abriu os dados e ficou registrada.
pub async fn update(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let old = current(&ctx.paths).ok_or_else(not_installed)?;
    progress(Step::Download);
    let latest = release::latest(&ctx.github).await?;
    if !release::newer(&latest.tag, &old.tag) {
        return Ok(Outcome::AlreadyLatest);
    }
    let binary = release::BINARY.fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.service).await?;
    let version = release::check(&binary).await?;
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    progress(Step::Service);
    // No Windows o registro para o processo antigo antes de poder falhar ao ligar o novo.
    let registered = service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&binary)).await.map_err(Failure::from);
    let switched = registered.is_ok();
    progress(Step::Start);
    // O registro só passa para a versão nova depois de ela responder: senão a próxima tentativa diria
    // "atualizado" sem nada rodando. Sem o registro gravado, o serviço também volta: ele e o registro
    // apontam sempre para a mesma versão.
    let up = match registered {
        Ok(()) => wait_up(&client, &version, before).await,
        Err(failure) => Err(failure),
    };
    let new = Installed { tag: latest.tag, binary };
    if let Err(failure) = up.and_then(|()| write_record(&ctx.paths, &new)) {
        if opened_by_new(switched, before, client.claimed().map(|c| c.pid)) {
            return Err(keep_new(&ctx.paths, &new, failure));
        }
        let back = service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await;
        return Err(match back {
            Ok(()) => failure,
            Err(e) => Failure::new("whatsapp_update_no_rollback",
                format!("{}; e o serviço não voltou para {}: {}: {}", failure.detail, old.tag, e.step, e.detail)),
        });
    }
    if let Some(dir) = old.binary.parent().filter(|d| Some(*d) != new.binary.parent())
        && let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "whatsapp: atualizado, mas a pasta da versão anterior ficou");
    }
    Ok(Outcome::Updated)
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

/// Espera o processo novo responder ao `status` com assinatura válida e a versão esperada.
async fn wait_up(client: &Client, version: &str, before: Option<u32>) -> Result<(), Failure> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut last = String::from("sem control.json");
    while Instant::now() < deadline {
        match client.claimed() {
            Some(control) if Some(control.pid) == before => last = "ainda o processo anterior".into(),
            Some(_) => match client.status().await {
                Ok(status) => match same_version(&status, version) {
                    Ok(()) => return Ok(()),
                    Err(why) => last = why,
                },
                Err(CallError::Off(detail) | CallError::Untrusted(detail) | CallError::Transport(detail)) => last = detail,
            },
            None => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(Failure::new("whatsapp_not_started", format!("o serviço não respondeu em {}s ({last})", START_TIMEOUT.as_secs())))
}

/// O `status` assinado só prova a subida com a `version` esperada; sem o campo, não prova.
fn same_version(status: &Value, version: &str) -> Result<(), String> {
    match status["version"].as_str() {
        Some(running) if running == version => Ok(()),
        Some(running) => Err(format!("respondeu a versão {running}")),
        None => Err("respondeu sem a versão".into()),
    }
}

/// A versão nova abriu os dados quando o `control.json` passou a outro processo depois de ela ser registrada:
/// o binário só o grava depois de abrir (e migrar) o `messages.db`.
fn opened_by_new(registered: bool, before: Option<u32>, now: Option<u32>) -> bool {
    registered && now.is_some() && now != before
}

/// Com os dados no formato da versão nova, a anterior recusaria abri-los (sai com 3) e o WhatsApp ficaria
/// parado: a nova fica registrada e a falha pede outra tentativa.
fn keep_new(paths: &Paths, new: &Installed, failure: Failure) -> Failure {
    let record = write_record(paths, new).err().map(|e| format!("; {}", e.detail)).unwrap_or_default();
    Failure::new("whatsapp_update_data_migrated",
        format!("{}{record}; os dados já estão no formato novo; tente atualizar de novo", failure.detail))
}

fn not_installed() -> Failure {
    Failure::new("whatsapp_not_installed", "o hangar-whatsapp não está instalado")
}

fn write_record(paths: &Paths, installed: &Installed) -> Result<(), Failure> {
    let path = paths.record();
    record::write(&path, &installed.tag, &installed.binary)
        .map_err(|e| Failure::new("whatsapp_record_failed", format!("{}: {e}", path.display())))
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
    }

    // A unit é do systemd: os caminhos esperados usam `/`, que no Windows o `Path::join` troca por `\`.
    #[cfg(unix)]
    #[test]
    fn the_unit_points_the_program_at_the_data_folder() {
        let unit = service::systemd_unit(&Context::new(Path::new("/home/u")).service(Path::new("/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp")));
        assert!(unit.contains("ExecStart=\"/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp\" --data \"/home/u/.hangar/whatsapp\"\n"), "{unit}");
    }

    /// Dados de esquema mais novo: o systemd para de religar a cada 10 s e a unit fica `failed` com a saída 3.
    #[test]
    fn the_unit_does_not_restart_after_the_schema_newer_exit() {
        let ctx = Context::new(Path::new("/home/u"));
        let unit = service::systemd_unit(&ctx.service(Path::new("/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp")));
        assert!(unit.contains("Restart=always\nRestartSec=10\nRestartPreventExitStatus=3\n\n[Install]\n"), "{unit}");
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
    fn only_the_expected_version_proves_the_service_is_up() {
        assert_eq!(same_version(&json!({"version": "0.1.1", "state": "connected"}), "0.1.1"), Ok(()));
        assert_eq!(same_version(&json!({"version": "0.1.0"}), "0.1.1"), Err("respondeu a versão 0.1.0".into()));
        assert_eq!(same_version(&json!({"state": "connected"}), "0.1.1"), Err("respondeu sem a versão".into()));
        assert_eq!(same_version(&json!({"version": 1}), "0.1.1"), Err("respondeu sem a versão".into()), "versão que não é texto");
    }

    #[tokio::test]
    async fn the_job_json_uses_the_whatsapp_names() {
        let installer = Installer::default();
        installer.start("install", Step::Download, |progress| async move {
            progress(Step::Service);
            Err(Failure::new("whatsapp_hash", "x"))
        }).unwrap().await;
        assert_eq!(installer.job(), json!({"kind": "install", "step": "service", "done": true,
            "error": {"code": "whatsapp_hash", "detail": "x"}, "outcome": null}));
        installer.forget();
        installer.start("update", Step::Download, |_| async { Ok(Outcome::Updated) }).unwrap().await;
        assert_eq!(installer.job()["outcome"], "updated");
        installer.forget();
        installer.start("update", Step::Download, |_| async { Ok(Outcome::AlreadyLatest) }).unwrap().await;
        assert_eq!(installer.job()["outcome"], "already_latest");
    }

    #[test]
    fn the_new_version_owns_the_data_once_it_claims_the_control_file() {
        assert!(opened_by_new(true, Some(10), Some(11)));
        assert!(opened_by_new(true, None, Some(11)), "nada rodava antes");
        assert!(!opened_by_new(true, Some(10), Some(10)), "ainda o processo anterior");
        assert!(!opened_by_new(true, Some(10), None), "ninguém pegou o control.json");
        assert!(!opened_by_new(false, Some(10), Some(11)), "o registro falhou: a versão nova não subiu");
    }

    /// Voltar com os dados já no formato novo deixaria o WhatsApp parado (a anterior sai com 3).
    #[test]
    fn a_failure_after_the_new_version_opened_the_data_keeps_it_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let binary = paths.service.join("bin/0.2.0/hangar-whatsapp");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        let new = Installed { tag: "v0.2.0".into(), binary };
        let failure = keep_new(&paths, &new, Failure::new("whatsapp_not_started", "o serviço não respondeu em 60s"));
        assert_eq!(failure.code, "whatsapp_update_data_migrated");
        assert!(failure.detail.ends_with("os dados já estão no formato novo; tente atualizar de novo"), "{}", failure.detail);
        assert_eq!(current(&paths), Some(new));
    }

    /// Sem release mais nova que a instalada, atualizar não baixa nem religa nada e diz isso.
    #[tokio::test]
    async fn updating_to_the_installed_release_says_it_is_already_the_latest() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let releases = axum::Router::new().route("/repos/{owner}/{repo}/releases",
            axum::routing::get(|| async {
                ([(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!([{"tag_name": "v0.1.0"}, {"tag_name": "v0.0.9"}]).to_string())
            }));
        tokio::spawn(async move { axum::serve(listener, releases).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context { paths: Paths::new(dir.path()), platform: Platform::local(),
            github: Github { api, download: "http://127.0.0.1:9/nada".into() } };
        let binary = ctx.paths.service.join("bin/0.1.0/hangar-whatsapp");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        write_record(&ctx.paths, &Installed { tag: "v0.1.0".into(), binary: binary.clone() }).unwrap();
        assert_eq!(update(&ctx, &|_| {}).await, Ok(Outcome::AlreadyLatest));
        assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }), "o registro não mudou");
    }
}
