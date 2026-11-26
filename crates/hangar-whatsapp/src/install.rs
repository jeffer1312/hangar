//! Instalar, atualizar e remover o serviço `hangar-whatsapp`, uma tarefa por vez, com o andamento
//! consultável pela tela.
//!
//! O programa, o registro da instalação e o aceite moram em `~/.hangar/whatsapp-service/`; os dados do
//! binário (`--data`), em `~/.hangar/whatsapp/`. Remover o programa nunca apaga os dados.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use hangar_managed_binary::jobs::{self, Jobs};
use hangar_managed_binary::record;
use hangar_managed_binary::service::{self, Arg, Service, ServiceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{CallError, Client};
use crate::control;
use crate::release::{self, Github, Platform, Release};

pub use hangar_managed_binary::jobs::Busy;
pub use hangar_managed_binary::record::Installed;

const START_TIMEOUT: Duration = Duration::from_secs(30);
/// O launchd não espera o programa sair para responder ao `unload`.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// A release nova muda o formato dos dados, ou não diz se muda, e ninguém confirmou: nada mudou.
    NeedsConfirm,
    /// O relógio já sabe que esta tag espera confirmação ou falhou: nem baixou.
    Skipped,
}

/// Quem pediu a atualização, e o que ela pode fazer sem perguntar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// O relógio do servidor: só troca de versão, e só quando o formato dos dados fica igual.
    Auto,
    /// O botão da tela: religa a instalada que não responde, mas para antes de mudar o formato dos dados.
    Manual,
    /// O dono confirmou a mudança de formato dos dados.
    Confirmed,
}

/// A última procura de atualização, em `update-check.json`, para a tela dizer se há versão nova sem ir ao GitHub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheck {
    /// A release mais nova da versão maior, igual à instalada quando não há outra.
    pub latest: String,
    /// Trocar para `latest` muda o formato dos dados, ou não se sabe.
    pub migration: bool,
    /// RFC 3339 em UTC.
    pub checked_at: String,
    /// A última tentativa de chegar a `latest` que falhou; some quando uma dá certo ou a tag muda.
    pub failed: Option<CheckFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub tag: String,
    pub code: String,
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

    pub fn update_check(&self) -> PathBuf {
        self.service.join("update-check.json")
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
            Self::NeedsConfirm => "needs_confirm",
            Self::Skipped => "skipped",
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

/// Começa `install` ou `update` em segundo plano; `mode` só vale para `update`.
///
/// # Errors
///
/// [`Busy`] com outra tarefa ou operação em curso.
pub fn spawn(installer: &Installer, kind: &'static str, ctx: Context, mode: Mode) -> Result<(), Busy> {
    installer.reserve(kind, Step::Download).map(|reservation| begin(reservation, ctx, mode))
}

/// Começa em segundo plano a tarefa da vez guardada (`install` ou `update`); `mode` só vale para `update`.
pub fn begin(reservation: Reservation, ctx: Context, mode: Mode) {
    let kind = reservation.kind();
    drop(reservation.start(move |progress| async move {
        match kind {
            "update" => update(&ctx, mode, &*progress).await,
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
    let body = json!({"at": now(), "text_version": ACCEPT_TEXT_VERSION}).to_string();
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

/// A última procura de atualização gravada, ou `None` sem arquivo legível.
pub fn read_update_check(paths: &Paths) -> Option<UpdateCheck> {
    serde_json::from_slice(&std::fs::read(paths.update_check()).ok()?).ok()
}

/// Grava `update-check.json`.
///
/// # Errors
///
/// `whatsapp_update_check_write_failed` quando o arquivo não grava.
pub fn write_update_check(paths: &Paths, check: &UpdateCheck) -> Result<(), Failure> {
    let body = serde_json::to_vec(check).map_err(|e| Failure::new("whatsapp_update_check_write_failed", e.to_string()))?;
    write_file(&paths.update_check(), &body).map_err(|e| Failure::new("whatsapp_update_check_write_failed", e))
}

/// Troca pela release mais nova da mesma versão maior; se ela não responder, volta para a anterior, salvo se
/// ela já abriu os dados. Release que muda o formato dos dados, ou não diz, só entra com [`Mode::Confirmed`].
/// Sem release mais nova, religa a instalação registrada que não responde, salvo no [`Mode::Auto`], que também
/// pula a tag que a última procura já viu parar na confirmação ou falhar ([`Outcome::Skipped`]). Quando a
/// procura no GitHub responde, o resultado fica em `update-check.json`.
///
/// # Errors
///
/// `whatsapp_not_installed`; falha do download, da conferência ou da subida (com a volta tentada);
/// `whatsapp_update_restored` quando a nova não respondeu e a cópia dos dados que ela gravou nesta tentativa e a
/// anterior voltaram ([`Mode::Confirmed`]); `whatsapp_update_data_migrated` quando a nova já abriu os dados e ficou
/// registrada.
pub async fn update(ctx: &Context, mode: Mode, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let old = current(&ctx.paths).ok_or_else(not_installed)?;
    progress(Step::Download);
    let latest = release::latest(&ctx.github).await?;
    // Sem isto o relógio baixaria a mesma release toda hora para parar no mesmo ponto; tentar de novo é da tela.
    if mode == Mode::Auto && release::newer(&latest.tag, &old.tag)
        && let Some(check) = read_update_check(&ctx.paths)
            .filter(|c| c.latest == latest.tag && (c.migration || c.failed.is_some())) {
        let check = UpdateCheck { checked_at: now(), ..check };
        if let Err(failure) = write_update_check(&ctx.paths, &check) {
            tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: a procura de atualização não ficou gravada");
        }
        return Ok(Outcome::Skipped);
    }
    let mut migration = None;
    let result = switch(ctx, mode, &old, &latest, &mut migration, progress).await;
    record_check(&ctx.paths, &latest.tag, migration, &result);
    result
}

/// [`update`] depois de achar a release mais nova; `migration` recebe a resposta assim que se sabe.
async fn switch(ctx: &Context, mode: Mode, old: &Installed, latest: &Release, migration: &mut Option<bool>,
    progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    if !release::newer(&latest.tag, &old.tag) {
        *migration = Some(false);
        // O relógio não mexe no serviço: religar a instalada é da tela, onde a pessoa vê o resultado.
        return match mode {
            Mode::Auto => Ok(Outcome::AlreadyLatest),
            Mode::Manual | Mode::Confirmed => ensure_up(ctx, old, progress).await,
        };
    }
    // O fetch reaproveita o programa que já está na pasta: ele pode ser o que o serviço roda, e não sai daqui.
    let fetched = !ctx.paths.service.join("bin").join(latest.version()).join(release::BINARY.program_file(ctx.platform)).exists();
    let binary = release::BINARY.fetch(&ctx.github, latest, ctx.platform, &ctx.paths.service).await?;
    let version = release::check(&binary).await?;
    let changes = schema_changes(&old.binary, &binary).await;
    *migration = Some(changes);
    if stops_for_confirmation(mode, changes) {
        tracing::info!(from = %old.tag, to = %latest.tag, "whatsapp: a versão nova pode mudar o formato dos dados; esperando confirmação");
        discard_download(old, &binary, fetched);
        return Ok(Outcome::NeedsConfirm);
    }
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    let recorded_before = recorded_pid(&ctx.paths.data);
    // A cópia dos dados que a volta pode usar é só a que a nova gravou nesta tentativa.
    let since = SystemTime::now();
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
    let new = Installed { tag: latest.tag.clone(), binary };
    if let Err(failure) = up {
        let source = restore_source(mode, &ctx.paths.data, release::schema_of(&old.binary).await, since);
        return Err(match after_failure(opened_after(switched, recorded_before, &ctx.paths.data), source) {
            AfterFailure::Restore(schema) => restore(ctx, old, &new, schema, fetched, failure).await,
            AfterFailure::KeepNew => keep_new(&ctx.paths, &new, failure),
            AfterFailure::Rollback => rollback(ctx, old, failure).await,
        });
    }
    // A nova respondeu e só o registro não gravou: os dados são dela, e a cópia não volta.
    if let Err(failure) = write_record(&ctx.paths, &new) {
        return Err(if opened_after(switched, recorded_before, &ctx.paths.data) {
            keep_new(&ctx.paths, &new, failure)
        } else {
            rollback(ctx, old, failure).await
        });
    }
    if let Some(dir) = old.binary.parent().filter(|d| Some(*d) != new.binary.parent())
        && let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "whatsapp: atualizado, mas a pasta da versão anterior ficou");
    }
    Ok(Outcome::Updated)
}

/// O formato dos dados só fica igual quando os dois programas dizem o mesmo esquema; quem não diz conta como
/// mudança, porque subir o `user_version` não tem volta.
async fn schema_changes(old: &Path, new: &Path) -> bool {
    let (old, new) = tokio::join!(release::schema_of(old), release::schema_of(new));
    tracing::info!(?old, ?new, "whatsapp: esquema dos dados antes e depois da troca");
    !matches!((old, new), (Some(a), Some(b)) if a == b)
}

fn stops_for_confirmation(mode: Mode, migration: bool) -> bool {
    migration && mode != Mode::Confirmed
}

/// Um programa baixado e não confirmado não fica na pasta: a próxima tentativa o confere de novo. Só sai o que
/// esta chamada baixou (`fetched`): o que já estava na pasta pode ser o que o serviço roda, com o registro atrás.
fn discard_download(old: &Installed, binary: &Path, fetched: bool) {
    if fetched && let Some(dir) = binary.parent().filter(|d| Some(*d) != old.binary.parent())
        && let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "whatsapp: a versão que espera confirmação ficou na pasta");
    }
}

/// A falha fica com a tag até uma tentativa dar certo; a espera por confirmação não a apaga, para a tela
/// continuar dizendo que a última tentativa falhou.
fn record_check(paths: &Paths, latest: &str, migration: Option<bool>, result: &Result<Outcome, Failure>) {
    let previous = read_update_check(paths).filter(|c| c.latest == latest);
    let failed = match result {
        Ok(Outcome::NeedsConfirm) => previous.as_ref().and_then(|c| c.failed.clone()),
        Ok(_) => None,
        Err(failure) => Some(CheckFailure { tag: latest.to_owned(), code: failure.code.to_owned() }),
    };
    // Sem a resposta dos dois programas (o download falhou), a mudança de formato vale como certa.
    let migration = migration.or(previous.map(|c| c.migration)).unwrap_or(true);
    let check = UpdateCheck { latest: latest.to_owned(), migration, checked_at: now(), failed };
    if let Err(failure) = write_update_check(paths, &check) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: a procura de atualização não ficou gravada");
    }
}

/// Sem release mais nova, a instalação registrada só está em dia se responde assinada com a versão dela: uma
/// atualização que manteve a nova registrada depois de falhar deixa o serviço parado, e repetir religa.
async fn ensure_up(ctx: &Context, installed: &Installed, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let version = release::check(&installed.binary).await?;
    let client = Client::new(ctx.paths.data.clone());
    let healthy = || async { client.status().await.is_ok_and(|status| same_version(&status, &version).is_ok()) };
    // Uma segunda leitura antes de religar: um status que falhou de passagem não derruba um serviço saudável.
    if healthy().await || healthy().await {
        return Ok(Outcome::AlreadyLatest);
    }
    tracing::warn!(tag = %installed.tag, "whatsapp: a versão registrada não respondeu; religando");
    let before = client.claimed().map(|c| c.pid);
    progress(Step::Service);
    service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&installed.binary)).await?;
    progress(Step::Start);
    wait_up(&client, &version, before).await?;
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

/// A versão nova abriu os dados quando o `control.json` passou a outro pid depois de ela ser registrada: o
/// binário só o grava depois de abrir (e migrar) o `messages.db`. Os pids são os gravados no arquivo, vivos ou não.
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

/// O que fazer depois que a nova não subiu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterFailure {
    /// Devolver a cópia `messages.db.schema<v>.bak` e a anterior ([`restore`]).
    Restore(u32),
    /// Os dados podem estar no formato da nova: ela fica registrada ([`keep_new`]).
    KeepNew,
    /// A nova não abriu os dados: a anterior volta sobre eles ([`rollback`]).
    Rollback,
}

/// A cópia desta tentativa vale mais que a prova de abertura: o binário sobe o `user_version` antes de gravar o
/// `control.json`, e voltar a anterior sobre o banco migrado a deixaria saindo com 3. Sem cópia, quem abriu os
/// dados fica com eles.
fn after_failure(opened: bool, source: Option<u32>) -> AfterFailure {
    match (source, opened) {
        (Some(schema), _) => AfterFailure::Restore(schema),
        (None, true) => AfterFailure::KeepNew,
        (None, false) => AfterFailure::Rollback,
    }
}

/// Registra a anterior de novo, sobre dados que a nova não abriu.
async fn rollback(ctx: &Context, old: &Installed, failure: Failure) -> Failure {
    match service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await {
        Ok(()) => failure,
        Err(e) => Failure::new("whatsapp_update_no_rollback",
            format!("{}; e o serviço não voltou para {}: {}: {}", failure.detail, old.tag, e.step, e.detail)),
    }
}

/// Devolve a cópia que a nova gravou antes de migrar e religa a anterior: o serviço para sem sair do gerenciador
/// (registrar a anterior com a nova de pé deixaria ela abrir o banco migrado e sair com 3), a cópia vai para o
/// lugar do `messages.db` e a anterior é registrada. Sem a parada confirmada ou com a troca dos arquivos falhando,
/// fica o [`keep_new`], com o serviço parado. `fetched`: a nova foi baixada nesta chamada.
async fn restore(ctx: &Context, old: &Installed, new: &Installed, schema: u32, fetched: bool, failure: Failure) -> Failure {
    let not_restored = |why: String| Failure { detail: format!("{}; a cópia dos dados não voltou: {why}", failure.detail), ..failure.clone() };
    // Conferido antes de parar: sem a anterior rodando, devolver a cópia não adianta.
    let version = match release::check(&old.binary).await {
        Ok(version) => version,
        Err(e) => return keep_new(&ctx.paths, new, not_restored(format!("{} não respondeu ao --version ({})", old.tag, e.detail))),
    };
    if let Err(why) = stop_confirmed(&ctx.paths.home, &ctx.paths.service, &ctx.paths.data).await {
        return keep_new(&ctx.paths, new, not_restored(why));
    }
    let data = ctx.paths.data.clone();
    let swapped = tokio::task::spawn_blocking(move || restore_backup(&data, schema)).await
        .unwrap_or_else(|e| Err(format!("a troca dos arquivos parou no meio: {e}")));
    if let Err(why) = swapped {
        return keep_new(&ctx.paths, new, not_restored(format!("{why}; o serviço ficou parado")));
    }
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    let back = match service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await {
        Ok(()) => wait_up(&client, &version, before).await,
        Err(e) => Err(Failure::from(e)),
    };
    match back {
        Ok(()) => {
            tracing::warn!(from = %old.tag, to = %new.tag, schema, "whatsapp: a versão nova não respondeu; os dados e a anterior voltaram");
            discard_download(old, &new.binary, fetched);
            Failure::new("whatsapp_update_restored", format!(
                "{}; os dados voltaram para a cópia de antes da troca e {} voltou a rodar; mensagens que chegaram durante \
                 a tentativa (até uns {}s) podem não voltar; o banco da tentativa ficou em messages.db.migrated",
                failure.detail, old.tag, START_TIMEOUT.as_secs()))
        }
        Err(e) => Failure::new("whatsapp_update_no_rollback", format!(
            "{}; os dados voltaram para a cópia de antes da troca (o banco da tentativa ficou em messages.db.migrated), \
             mas {} não voltou a rodar: {}", failure.detail, old.tag, e.detail)),
    }
}

/// Só com a confirmação do dono, o esquema `v` da cópia `messages.db.schema<v>.bak` que a nova gravou desde
/// `since`: a do esquema da anterior, ou, quando a anterior não diz o esquema, a única gravada nesta tentativa.
/// Cópia de outra tentativa não serve: devolvê-la apagaria o que chegou depois dela.
fn restore_source(mode: Mode, data: &Path, old_schema: Option<u32>, since: SystemTime) -> Option<u32> {
    if mode != Mode::Confirmed {
        return None;
    }
    let fresh = |schema: &u32| std::fs::metadata(backup_path(data, *schema)).and_then(|m| m.modified()).is_ok_and(|at| at >= since);
    if let Some(schema) = old_schema {
        return Some(schema).filter(fresh);
    }
    let mut found = std::fs::read_dir(data).ok()?.filter_map(|entry| {
        let name = entry.ok()?.file_name();
        name.to_str()?.strip_prefix("messages.db.schema")?.strip_suffix(".bak")?.parse::<u32>().ok()
    }).filter(fresh);
    let schema = found.next()?;
    found.next().is_none().then_some(schema)
}

fn backup_path(data: &Path, schema: u32) -> PathBuf {
    data.join(format!("messages.db.schema{schema}.bak"))
}

/// Para o serviço e espera o gerenciador dizer que nenhum processo ficou e o pid do `control.json` sair: o
/// gerenciador sozinho não vê um programa que escapou dele.
async fn stop_confirmed(home: &Path, managed_dir: &Path, data: &Path) -> Result<(), String> {
    service::stop(home, managed_dir, &SERVICE).await.map_err(|e| format!("o serviço não parou: {}: {}", e.step, e.detail))?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        let running = service::process_running(home, &SERVICE).await
            .map_err(|e| format!("não consegui confirmar que o serviço parou: {}: {}", e.step, e.detail))?;
        let pid = live_pid(data)?;
        if !running && pid.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(match pid {
                Some(pid) => format!("o processo {pid} do control.json não saiu em {}s", STOP_TIMEOUT.as_secs()),
                None => format!("o serviço não parou em {}s", STOP_TIMEOUT.as_secs()),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// O pid do `control.json` que ainda vive. Arquivo ilegível não confirma nada: o processo pode estar de pé.
fn live_pid(data: &Path) -> Result<Option<u32>, String> {
    match control::read_control(data) {
        Ok(control) => Ok(control::alive(&control).then_some(control.pid)),
        Err(control::ReadError::Missing) => Ok(None),
        Err(control::ReadError::Invalid(why)) => Err(format!("não consegui ler o pid do control.json: {why}")),
    }
}

/// O banco e os arquivos de lado dele: um `-wal` ou `-journal` que ficasse seria aplicado sobre a cópia.
const DB_FILES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

/// Põe a cópia `messages.db.schema<schema>.bak` no lugar do `messages.db`, com o programa parado. Nada se apaga:
/// o conjunto migrado sai do caminho para `messages.db.migrated*`, onde fica até a próxima volta, e a cópia
/// continua para uma segunda tentativa. A cópia entra por um arquivo à parte gravado no disco antes da troca.
///
/// # Errors
///
/// O que falhou e o estado em que os arquivos ficaram: o que tinha saído do caminho volta.
fn restore_backup(data: &Path, schema: u32) -> Result<(), String> {
    let backup = backup_path(data, schema);
    let staged = data.join("messages.db.restoring");
    let copied = std::fs::copy(&backup, &staged)
        .and_then(|_| std::fs::OpenOptions::new().write(true).open(&staged)?.sync_all());
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("{}: {e}; nada mudou", backup.display()));
    }
    let mut moved = Vec::new();
    for suffix in DB_FILES {
        let (from, to) = (data.join(format!("messages.db{suffix}")), data.join(format!("messages.db.migrated{suffix}")));
        // Sem o arquivo agora, o da volta anterior com o mesmo nome iria junto com o banco desta.
        let (result, path) = if from.exists() {
            (std::fs::rename(&from, &to).map(|()| true), &from)
        } else {
            (std::fs::remove_file(&to).map(|()| false).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(false) } else { Err(e) }), &to)
        };
        match result {
            Ok(true) => moved.push((from.clone(), to.clone())),
            Ok(false) => {}
            Err(e) => return Err(undo(&staged, &moved, format!("{}: {e}", path.display()))),
        }
    }
    std::fs::rename(&staged, data.join("messages.db")).map_err(|e| undo(&staged, &moved, format!("{}: {e}", staged.display())))
}

/// Põe de volta o que saiu do caminho e diz como os arquivos ficaram.
fn undo(staged: &Path, moved: &[(PathBuf, PathBuf)], why: String) -> String {
    let _ = std::fs::remove_file(staged);
    let stuck: Vec<String> = moved.iter().rev().filter_map(|(from, to)| std::fs::rename(to, from).err()
        .map(|e| format!("{} ficou como {}: {e}", from.display(), to.display()))).collect();
    if stuck.is_empty() {
        format!("{why}; os arquivos do banco voltaram como estavam")
    } else {
        format!("{why}; e não voltaram: {}", stuck.join("; "))
    }
}

/// O pid do `control.json` sem conferir se o processo vive: a nova pode gravar o arquivo e morrer antes da
/// conferência, e a prova de que ela abriu os dados tem que sobreviver a isso.
fn recorded_pid(data: &Path) -> Option<u32> {
    control::read_control(data).ok().map(|c| c.pid)
}

/// [`opened_by_new`] pelo arquivo depois da falha; arquivo que não se lê conta como aberto, porque voltar sobre
/// dados migrados deixa o WhatsApp parado.
fn opened_after(registered: bool, before: Option<u32>, data: &Path) -> bool {
    match control::read_control(data) {
        Ok(control) => opened_by_new(registered, before, Some(control.pid)),
        Err(control::ReadError::Missing) => false,
        Err(control::ReadError::Invalid(why)) => {
            tracing::warn!(%why, "whatsapp: control.json ilegível depois da troca de versão");
            registered
        }
    }
}

fn not_installed() -> Failure {
    Failure::new("whatsapp_not_installed", "o hangar-whatsapp não está instalado")
}

fn write_record(paths: &Paths, installed: &Installed) -> Result<(), Failure> {
    let path = paths.record();
    record::write(&path, &installed.tag, &installed.binary)
        .map_err(|e| Failure::new("whatsapp_record_failed", format!("{}: {e}", path.display())))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
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
        assert_eq!(paths.update_check(), Path::new("/home/u/.hangar/whatsapp-service/update-check.json"));
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
        installer.forget();
        installer.start("update", Step::Download, |_| async { Ok(Outcome::NeedsConfirm) }).unwrap().await;
        assert_eq!(installer.job()["outcome"], "needs_confirm");
        installer.forget();
        installer.start("update", Step::Download, |_| async { Ok(Outcome::Skipped) }).unwrap().await;
        assert_eq!(installer.job()["outcome"], "skipped");
    }

    #[test]
    fn only_a_confirmed_update_crosses_a_change_of_the_data_format() {
        for mode in [Mode::Auto, Mode::Manual] {
            assert!(stops_for_confirmation(mode, true), "{mode:?}");
            assert!(!stops_for_confirmation(mode, false), "{mode:?}");
        }
        assert!(!stops_for_confirmation(Mode::Confirmed, true));
        assert!(!stops_for_confirmation(Mode::Confirmed, false));
    }

    /// Programa de mentira: `--version` diz `version`; `--schema-version` diz `schema` ou sai 2, como o binário
    /// anterior ao C1 diante de uma flag que não conhece.
    #[cfg(unix)]
    fn fake_program(path: &Path, version: &str, schema: Option<u32>) {
        let schema = schema.map_or_else(|| "exit 2".to_owned(), |n| format!("echo {n}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\ncase \"$1\" in\n--version) echo 'hangar-whatsapp {version} (api 1)' ;;\n\
            --schema-version) {schema} ;;\n*) exit 1 ;;\nesac\n")).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_data_format_stays_only_when_both_programs_say_the_same_schema() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old/hangar-whatsapp"), dir.path().join("new/hangar-whatsapp"));
        for (before, after, changes) in [(Some(2), Some(2), false), (None, Some(2), true), (Some(1), Some(2), true), (Some(2), None, true)] {
            fake_program(&old, "0.1.0", before);
            fake_program(&new, "0.2.0", after);
            assert_eq!(schema_changes(&old, &new).await, changes, "{before:?} -> {after:?}");
        }
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

    /// A nova grava o `control.json` e morre antes da conferência: a prova de que ela abriu os dados é o pid
    /// gravado, não o processo vivo. Um `control.json` antigo, de processo morto, não é abertura da nova.
    #[test]
    fn the_new_version_owns_the_data_even_after_dying_once_it_wrote_the_control_file() {
        // Acima de qualquer pid que o sistema dê: nunca é um processo vivo.
        const DEAD: u32 = i32::MAX.unsigned_abs();
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let write = |pid: u32| std::fs::write(data.join("control.json"),
            json!({"version": 1, "api": 1, "port": 40123, "pid": pid}).to_string()).unwrap();
        assert!(opened_by_new(true, recorded_pid(data), Some(DEAD)), "nada gravado antes");
        write(DEAD);
        let before = recorded_pid(data);
        assert_eq!(before, Some(DEAD));
        assert_eq!(Client::new(data.to_path_buf()).claimed(), None, "nada vivo antes da troca");
        assert!(!opened_by_new(true, before, recorded_pid(data)), "o arquivo de uma execução anterior ficou igual");
        write(DEAD - 1);
        assert_eq!(Client::new(data.to_path_buf()).claimed(), None, "a nova já morreu");
        assert!(opened_by_new(true, before, recorded_pid(data)), "a nova gravou o arquivo e morreu");
    }

    /// Um `control.json` que não se lê depois da troca não prova que a nova deixou de abrir os dados.
    #[test]
    fn an_unreadable_control_file_after_the_switch_keeps_the_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert!(!opened_after(true, None, data), "sem arquivo, a nova não abriu");
        std::fs::write(data.join("control.json"), "{").unwrap();
        assert!(opened_after(true, None, data));
        assert!(!opened_after(false, None, data), "sem o registro, a nova nem subiu");
    }

    fn write_db_file(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// A cópia entra no lugar do banco migrado; o conjunto migrado sai do caminho inteiro, sem nada apagado, e a
    /// cópia fica para outra tentativa.
    #[test]
    fn restoring_puts_the_backup_in_place_and_keeps_the_migrated_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        for (name, body) in [("messages.db", "migrated"), ("messages.db-wal", "wal"), ("messages.db-shm", "shm"),
            ("messages.db.schema1.bak", "before"), ("session.db", "session"), ("messages.db.migrated-journal", "stale")] {
            write_db_file(&data.join(name), body);
        }
        restore_backup(data, 1).unwrap();
        assert_eq!(read(&data.join("messages.db")), "before");
        for gone in ["messages.db-wal", "messages.db-shm", "messages.db-journal", "messages.db.restoring"] {
            assert!(!data.join(gone).exists(), "{gone} ficou");
        }
        assert_eq!(read(&data.join("messages.db.migrated")), "migrated");
        assert_eq!(read(&data.join("messages.db.migrated-wal")), "wal");
        assert_eq!(read(&data.join("messages.db.migrated-shm")), "shm");
        assert!(!data.join("messages.db.migrated-journal").exists(), "o arquivo de lado da volta anterior iria junto com o banco desta");
        assert_eq!(read(&data.join("messages.db.schema1.bak")), "before", "a cópia saiu");
        assert_eq!(read(&data.join("session.db")), "session", "a sessão do aparelho mudou");
    }

    /// Sem a cópia, nada muda: o banco migrado e o `-wal` dele seguem no lugar.
    #[test]
    fn restoring_without_the_backup_leaves_the_migrated_database_whole() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        write_db_file(&data.join("messages.db"), "migrated");
        write_db_file(&data.join("messages.db-wal"), "wal");
        write_db_file(&data.join("messages.db.schema2.bak"), "other");
        let why = restore_backup(data, 1).unwrap_err();
        assert!(why.contains("messages.db.schema1.bak") && why.ends_with("nada mudou"), "{why}");
        assert_eq!(read(&data.join("messages.db")), "migrated");
        assert_eq!(read(&data.join("messages.db-wal")), "wal");
        assert!(!data.join("messages.db.restoring").exists());
        assert!(!data.join("messages.db.migrated").exists());
    }

    /// Uma troca que falha no meio põe de volta o que já tinha saído do caminho e diz isso.
    #[test]
    fn a_restore_that_fails_halfway_puts_the_migrated_set_back() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        for (name, body) in [("messages.db", "migrated"), ("messages.db-wal", "wal"), ("messages.db-shm", "shm"),
            ("messages.db.schema1.bak", "before")] {
            write_db_file(&data.join(name), body);
        }
        // Uma pasta com conteúdo no nome de destino do `-shm`: o rename dele falha depois do banco e do `-wal`.
        std::fs::create_dir_all(data.join("messages.db.migrated-shm/x")).unwrap();
        let why = restore_backup(data, 1).unwrap_err();
        assert!(why.contains("messages.db-shm") && why.ends_with("os arquivos do banco voltaram como estavam"), "{why}");
        assert_eq!(read(&data.join("messages.db")), "migrated");
        assert_eq!(read(&data.join("messages.db-wal")), "wal");
        assert_eq!(read(&data.join("messages.db-shm")), "shm");
        for gone in ["messages.db.migrated", "messages.db.migrated-wal", "messages.db.restoring"] {
            assert!(!data.join(gone).exists(), "{gone} ficou");
        }
        assert_eq!(read(&data.join("messages.db.schema1.bak")), "before");
    }

    /// A cópia desta tentativa leva à volta mesmo sem a prova de que a nova abriu os dados: o binário migra antes
    /// de gravar o `control.json`.
    #[test]
    fn a_backup_of_this_attempt_restores_even_without_proof_of_opening() {
        assert_eq!(after_failure(false, Some(1)), AfterFailure::Restore(1));
        assert_eq!(after_failure(true, Some(1)), AfterFailure::Restore(1));
        assert_eq!(after_failure(true, None), AfterFailure::KeepNew);
        assert_eq!(after_failure(false, None), AfterFailure::Rollback);
    }

    #[test]
    fn the_stop_waits_for_the_pid_of_the_control_file() {
        // Acima de qualquer pid que o sistema dê: nunca é um processo vivo.
        const DEAD: u32 = i32::MAX.unsigned_abs();
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let write = |pid: u32| std::fs::write(data.join("control.json"),
            json!({"version": 1, "api": 1, "port": 40123, "pid": pid}).to_string()).unwrap();
        assert_eq!(live_pid(data), Ok(None), "sem control.json");
        write(DEAD);
        assert_eq!(live_pid(data), Ok(None), "o processo já saiu");
        write(std::process::id());
        assert_eq!(live_pid(data), Ok(Some(std::process::id())));
        std::fs::write(data.join("control.json"), "{").unwrap();
        assert!(live_pid(data).is_err(), "control.json ilegível não confirma a parada");
    }

    /// O programa que já estava na pasta antes do download pode ser o que o serviço roda: não sai.
    #[test]
    fn discarding_removes_only_what_this_call_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let old = Installed { tag: "v0.1.0".into(), binary: dir.path().join("bin/0.1.0/hangar-whatsapp") };
        let new = dir.path().join("bin/0.2.0/hangar-whatsapp");
        for binary in [&old.binary, &new] {
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(binary, b"x").unwrap();
        }
        discard_download(&old, &new, false);
        assert!(new.exists(), "o que já estava na pasta saiu");
        discard_download(&old, &old.binary, true);
        assert!(old.binary.exists(), "a pasta da instalada saiu");
        discard_download(&old, &new, true);
        assert!(!new.parent().unwrap().exists(), "o baixado nesta chamada ficou");
    }

    #[test]
    fn only_a_confirmed_update_with_a_backup_of_this_attempt_restores() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        // Horas fixas: a cópia tem a hora que o teste dá a ela.
        let since = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let (fresh, stale) = (since + Duration::from_secs(5), since - Duration::from_secs(3600));
        let backup = |schema: u32, at: SystemTime| {
            let path = backup_path(data, schema);
            write_db_file(&path, "before");
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
        };
        assert_eq!(restore_source(Mode::Confirmed, data, Some(1), since), None, "sem cópia");
        assert_eq!(restore_source(Mode::Confirmed, data, None, since), None, "sem cópia nem esquema");
        backup(1, fresh);
        assert_eq!(restore_source(Mode::Confirmed, data, Some(1), since), Some(1));
        assert_eq!(restore_source(Mode::Confirmed, data, None, since), Some(1), "a anterior sem --schema-version");
        assert_eq!(restore_source(Mode::Confirmed, data, Some(2), since), None, "a cópia é de outro esquema");
        for mode in [Mode::Auto, Mode::Manual] {
            assert_eq!(restore_source(mode, data, Some(1), since), None, "{mode:?}");
        }
        backup(2, stale);
        assert_eq!(restore_source(Mode::Confirmed, data, Some(2), since), None, "a cópia é de outra tentativa");
        assert_eq!(restore_source(Mode::Confirmed, data, None, since), Some(1), "só a desta tentativa conta");
        backup(2, fresh);
        assert_eq!(restore_source(Mode::Confirmed, data, None, since), None, "duas desta tentativa: não se sabe qual");
        write_db_file(&data.join("messages.db.schemaX.bak"), "x");
        assert_eq!(restore_source(Mode::Confirmed, data, Some(1), since), Some(1));
    }

    /// Releases no GitHub de mentira com `v0.1.0` como a mais nova, e a instalação `v0.1.0` registrada.
    async fn installed_latest(routes: axum::Router) -> (tempfile::TempDir, Context, PathBuf, u16) {
        installed_with(&["v0.1.0", "v0.0.9"], routes).await
    }

    /// Releases `tags` no GitHub de mentira, que não serve download, e a instalação `v0.1.0` registrada.
    async fn installed_with(tags: &[&str], routes: axum::Router) -> (tempfile::TempDir, Context, PathBuf, u16) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = Value::from(tags.iter().map(|tag| json!({"tag_name": tag})).collect::<Vec<_>>()).to_string();
        let releases = routes.route("/repos/{owner}/{repo}/releases",
            axum::routing::get(move || {
                let body = body.clone();
                async move { ([(axum::http::header::CONTENT_TYPE, "application/json")], body) }
            }));
        tokio::spawn(async move { axum::serve(listener, releases).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context { paths: Paths::new(dir.path()), platform: Platform::local(),
            github: Github { api: format!("http://127.0.0.1:{port}"), download: "http://127.0.0.1:9/nada".into() } };
        let binary = ctx.paths.service.join("bin/0.1.0/hangar-whatsapp");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        write_record(&ctx.paths, &Installed { tag: "v0.1.0".into(), binary: binary.clone() }).unwrap();
        (dir, ctx, binary, port)
    }

    /// Sem release mais nova e com o serviço respondendo assinado na versão instalada, atualizar não baixa nem
    /// religa nada e diz isso.
    #[cfg(unix)]
    #[tokio::test]
    async fn updating_to_the_installed_release_says_it_is_already_the_latest() {
        let key: Vec<u8> = (0u8..32).collect();
        let signing = key.clone();
        let status = axum::Router::new().route("/v1/status", axum::routing::get(move |headers: axum::http::HeaderMap| {
            let key = signing.clone();
            async move {
                let header = headers.get(control::SIGNATURE_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();
                let (ts, _) = header.split_once('.').unwrap_or_default();
                let body = json!({"api": 1, "version": "0.1.0", "state": "connected"}).to_string();
                let signature = control::sign_response(&key, ts, 200, body.as_bytes());
                ([(control::SIGNATURE_HEADER, signature)], body)
            }
        }));
        let (_dir, ctx, binary, port) = installed_latest(status).await;
        std::fs::write(&binary, "#!/bin/sh\necho 'hangar-whatsapp 0.1.0 (api 1)'\n").unwrap();
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(&ctx.paths.data).unwrap();
        std::fs::write(ctx.paths.data.join("control.key"), hangar_managed_binary::hex::encode(&key)).unwrap();
        std::fs::write(ctx.paths.data.join("control.json"),
            json!({"version": 1, "api": 1, "port": port, "pid": std::process::id()}).to_string()).unwrap();
        assert_eq!(update(&ctx, Mode::Manual, &|_| {}).await, Ok(Outcome::AlreadyLatest));
        assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }), "o registro não mudou");
    }

    /// A tentativa seguinte a uma atualização que deixou a nova registrada e parada acha a mesma tag: sem o
    /// serviço de pé, ela não é "já é a mais nova". Aqui o programa nem roda, e a falha aparece antes de religar.
    #[tokio::test]
    async fn the_installed_release_that_does_not_answer_is_not_already_the_latest() {
        let (_dir, ctx, binary, _) = installed_latest(axum::Router::new()).await;
        let got = update(&ctx, Mode::Manual, &|_| {}).await;
        assert_eq!(got.map_err(|f| f.code), Err("whatsapp_version_unreadable"));
        assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }), "o registro não mudou");
    }

    /// O relógio não religa: o mesmo programa que não roda, no modo automático, só diz que não há versão nova, e
    /// a procura que deu certo apaga a falha anterior da mesma tag.
    #[tokio::test]
    async fn the_clock_does_not_restart_the_installed_release() {
        let (_dir, ctx, binary, _) = installed_latest(axum::Router::new()).await;
        let failed = Some(CheckFailure { tag: "v0.1.0".into(), code: "whatsapp_not_started".into() });
        write_update_check(&ctx.paths, &UpdateCheck { latest: "v0.1.0".into(), migration: true, checked_at: "x".into(), failed }).unwrap();
        assert_eq!(update(&ctx, Mode::Auto, &|_| {}).await, Ok(Outcome::AlreadyLatest));
        assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }), "o registro não mudou");
        let check = read_update_check(&ctx.paths).unwrap();
        assert_eq!((check.latest.as_str(), check.migration, check.failed), ("v0.1.0", false, None));
        assert!(check.checked_at.ends_with('Z'), "{}", check.checked_at);
    }

    /// Binário instalado sem `--schema-version` (anterior ao C1): a troca pode subir o `user_version`, e sem
    /// confirmação nada é registrado, o programa baixado sai da pasta e a procura fica gravada.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_release_that_may_change_the_data_format_waits_for_confirmation() {
        let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        fake_program(&binary, "0.1.0", None);
        let downloaded = ctx.paths.service.join("bin/0.2.0/hangar-whatsapp");
        for mode in [Mode::Auto, Mode::Manual] {
            fake_program(&downloaded, "0.2.0", Some(2));
            assert_eq!(update(&ctx, mode, &|_| {}).await, Ok(Outcome::NeedsConfirm), "{mode:?}");
            assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary: binary.clone() }), "{mode:?}");
            // O programa já estava na pasta antes da procura (o GitHub de mentira não serve download): não é desta chamada.
            assert!(downloaded.exists(), "{mode:?}: o programa que já estava na pasta saiu");
            assert!(binary.exists(), "{mode:?}: a versão instalada continua");
            let check = read_update_check(&ctx.paths).unwrap();
            assert_eq!((check.latest.as_str(), check.migration, check.failed), ("v0.2.0", true, None), "{mode:?}");
        }
    }

    /// A falha fica gravada com a tag; a passada seguinte que para na confirmação não a apaga.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_attempt_stays_with_its_tag() {
        let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        fake_program(&binary, "0.1.0", None);
        // Sem o programa na pasta, o download vai ao GitHub de mentira, que não serve nada.
        let got = update(&ctx, Mode::Auto, &|_| {}).await;
        assert_eq!(got.map_err(|f| f.code), Err(release::CODES.download_failed));
        let failed = Some(CheckFailure { tag: "v0.2.0".into(), code: release::CODES.download_failed.into() });
        let check = read_update_check(&ctx.paths).unwrap();
        assert_eq!((check.latest.as_str(), check.migration, &check.failed), ("v0.2.0", true, &failed), "sem resposta conta como mudança");
        fake_program(&ctx.paths.service.join("bin/0.2.0/hangar-whatsapp"), "0.2.0", Some(2));
        assert_eq!(update(&ctx, Mode::Manual, &|_| {}).await, Ok(Outcome::NeedsConfirm));
        assert_eq!(read_update_check(&ctx.paths).unwrap().failed, failed);
    }

    /// O relógio não baixa de novo a tag que já parou na confirmação ou falhou; a tela, sim. A procura gravada
    /// só ganha a hora nova.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_clock_skips_a_tag_it_already_knows_will_stop() {
        let failed = Some(CheckFailure { tag: "v0.2.0".into(), code: "whatsapp_not_started".into() });
        for (migration, failed) in [(true, None), (false, failed)] {
            let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
            fake_program(&binary, "0.1.0", None);
            let known = UpdateCheck { latest: "v0.2.0".into(), migration, checked_at: "x".into(), failed: failed.clone() };
            write_update_check(&ctx.paths, &known).unwrap();
            let downloaded = ctx.paths.service.join("bin/0.2.0/hangar-whatsapp");
            assert_eq!(update(&ctx, Mode::Auto, &|_| {}).await, Ok(Outcome::Skipped), "{migration} {failed:?}");
            assert!(!downloaded.parent().unwrap().exists(), "o relógio baixou a tag que ia parar");
            let check = read_update_check(&ctx.paths).unwrap();
            assert_eq!((check.latest.as_str(), check.migration, &check.failed), ("v0.2.0", migration, &failed));
            assert_ne!(check.checked_at, "x", "a hora da procura não andou");
            // O antigo sem `--schema-version` faz a tela parar na confirmação, depois de baixar.
            fake_program(&downloaded, "0.2.0", Some(2));
            assert_eq!(update(&ctx, Mode::Manual, &|_| {}).await, Ok(Outcome::NeedsConfirm), "{migration} {failed:?}");
            assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }));
        }
    }

    /// Com o mesmo esquema dos dois lados a troca segue sem confirmação: sem ela, o programa baixado ficaria
    /// para o registro do serviço. O registro em si não roda aqui (mexeria no gerenciador de serviços da máquina).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_same_schema_or_a_confirmation_does_not_stop_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old/hangar-whatsapp"), dir.path().join("new/hangar-whatsapp"));
        fake_program(&old, "0.2.0", Some(2));
        fake_program(&new, "0.3.0", Some(2));
        let same = schema_changes(&old, &new).await;
        for mode in [Mode::Auto, Mode::Manual, Mode::Confirmed] {
            assert!(!stops_for_confirmation(mode, same), "{mode:?}");
        }
        fake_program(&old, "0.1.0", None);
        assert!(!stops_for_confirmation(Mode::Confirmed, schema_changes(&old, &new).await));
    }
}
