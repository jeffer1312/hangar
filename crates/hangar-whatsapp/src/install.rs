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
/// Teto da espera quando a nova está migrando os dados: um `messages.db` grande leva minutos.
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(600);
/// Passado o [`START_TIMEOUT`], cada volta pergunta ao gerenciador; no Windows isso abre um PowerShell.
const MIGRATION_POLL: Duration = Duration::from_secs(5);
/// O launchd não espera o programa sair para responder ao `unload`.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Teto de cada chamada ao gerenciador de serviços: uma que não volta seguraria a vez do instalador.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

pub const SERVICE: ServiceId = ServiceId {
    unit: "hangar-whatsapp.service",
    launchd_label: "dev.hangar.whatsapp",
    task: "hangar-whatsapp",
    description: "WhatsApp do Hangar",
    no_restart_exit: &[SCHEMA_NEWER_EXIT, BACKUP_FAILED_EXIT],
};

/// O binário sai com este código quando o `messages.db` é de um esquema mais novo que o dele: só um binário
/// mais novo abre esses dados, e religar o mesmo não adianta.
pub const SCHEMA_NEWER_EXIT: u8 = 3;

/// O binário sai com este código quando não grava a cópia de antes de migrar: o `messages.db` fica como estava, e
/// religar o mesmo só repetiria a falha.
pub const BACKUP_FAILED_EXIT: u8 = 4;

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
    /// Os arquivos que uma volta deixou fora do lugar voltaram e o serviço subiu ([`recover`]); a troca fica para a
    /// próxima passada.
    Restored,
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
    /// A tentativa de chegar a `latest` que falhou depois de registrar o serviço da nova; prende a tag para o
    /// relógio. Some quando uma dá certo ou a tag muda.
    #[serde(default)]
    pub failed: Option<CheckFailure>,
    /// A última tentativa parou antes de registrar a nova (download, conferência): não prende a tag, e some quando
    /// uma tentativa passa dessas etapas ou a tag muda.
    #[serde(default)]
    pub last_error: Option<CheckFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub tag: String,
    pub code: String,
}

/// Uma passada da atualização automática: `from` é a tag instalada antes; `to`, a release mais nova achada (a
/// instalada depois quando trocou), ou `from` quando a procura no GitHub falhou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoUpdate {
    pub from: String,
    pub to: String,
    pub result: Result<Outcome, Failure>,
}

/// O que a passada do relógio tem a fazer, decidido sem a vez do instalador ([`auto_wants`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoPlan {
    NotInstalled,
    /// Nada a trocar, ou a procura no GitHub falhou.
    Done(AutoUpdate),
    /// Há release mais nova a tentar: [`update_to`] no [`Mode::Auto`], na vez.
    Switch { from: String, latest: Release },
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

    /// Os arquivos do banco que uma volta da cópia deixou fora do lugar ([`recover`]).
    pub fn restore_pending(&self) -> PathBuf {
        self.service.join("restore-pending.json")
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
            Self::Restored => "restored",
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
pub fn spawn(installer: &Installer, kind: &'static str, ctx: Context, mode: Mode, expected: Option<String>) -> Result<(), Busy> {
    installer.reserve(kind, Step::Download).map(|reservation| begin(reservation, ctx, mode, expected))
}

/// Começa em segundo plano a tarefa da vez guardada (`install` ou `update`); `mode` e `expected` (a tag que o dono
/// confirmou, [`update_expecting`]) só valem para `update`.
pub fn begin(reservation: Reservation, ctx: Context, mode: Mode, expected: Option<String>) {
    let kind = reservation.kind();
    drop(reservation.start(move |progress| async move {
        match kind {
            "update" => update_expecting(&ctx, mode, expected.as_deref(), &*progress).await,
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
    recover(ctx).await?;
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
    wait_up(&client, &version, None, None).await?;
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
    wait_up(&Client::new(ctx.paths.data.clone()), &version, None, None).await
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

/// O que a passada do relógio tem a fazer, sem segurar a vez: procura a release mais nova e aplica as regras do
/// [`Mode::Auto`]. Sem release nova não mexe no serviço; tag que a última procura já viu parar na confirmação ou
/// falhar depois de registrar a nova é pulada ([`Outcome::Skipped`]). Só a troca a fazer volta como
/// [`AutoPlan::Switch`]. A procura que responde fica em `update-check.json`.
pub async fn auto_wants(ctx: &Context) -> AutoPlan {
    let Some(old) = current(&ctx.paths) else { return AutoPlan::NotInstalled };
    let latest = match release::latest(&ctx.github).await {
        Ok(latest) => latest,
        Err(failure) => return AutoPlan::Done(AutoUpdate { to: old.tag.clone(), from: old.tag, result: Err(failure) }),
    };
    let skipped = |old: Installed, latest: Release| AutoPlan::Done(AutoUpdate { from: old.tag, to: latest.tag, result: Ok(Outcome::Skipped) });
    // Sem a vez (um envio a segura): a procura gravada é só da instalação que a pediu. Trocada no meio da procura,
    // nada se grava, e a próxima passada procura de novo.
    if current(&ctx.paths).as_ref().map(|now| &now.tag) != Some(&old.tag) {
        return skipped(old, latest);
    }
    if !release::newer(&latest.tag, &old.tag) {
        let result = Ok(Outcome::AlreadyLatest);
        record_check(&ctx.paths, &latest.tag, &Attempt { migration: Some(false), registered: false, schema_unread: false }, &result);
        return AutoPlan::Done(AutoUpdate { from: old.tag, to: latest.tag, result });
    }
    if skip_known(&ctx.paths, &latest.tag) {
        return skipped(old, latest);
    }
    AutoPlan::Switch { from: old.tag, latest }
}

/// Troca pela release mais nova da mesma versão maior; se ela não responder, volta para a anterior, salvo se ela
/// já abriu os dados. Release que muda o formato dos dados, ou não diz, só entra com [`Mode::Confirmed`].
/// Sem release mais nova, religa a instalação registrada que não responde, salvo no [`Mode::Auto`], que também
/// pula a tag que a última procura já viu parar na confirmação ou falhar ([`Outcome::Skipped`]). Quando a
/// procura no GitHub responde, o resultado fica em `update-check.json`.
///
/// # Errors
///
/// `whatsapp_not_installed`; falha do download, da conferência ou da subida (com a volta tentada);
/// `whatsapp_backup_failed` quando a nova não gravou a cópia de antes de migrar e a anterior voltou;
/// `whatsapp_update_restored` quando a nova não respondeu e a cópia dos dados que ela gravou nesta tentativa e a
/// anterior voltaram ([`Mode::Confirmed`]); `whatsapp_update_data_migrated` quando a nova já abriu os dados e ficou
/// registrada.
pub async fn update(ctx: &Context, mode: Mode, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    update_expecting(ctx, mode, None, progress).await
}

/// [`update`] com a tag que o dono viu ao confirmar: a confirmação vale só para ela, e uma release mais nova que
/// apareceu no meio volta a perguntar ([`Mode::Manual`]).
///
/// # Errors
///
/// As de [`update`].
pub async fn update_expecting(ctx: &Context, mode: Mode, expected: Option<&str>, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    current(&ctx.paths).ok_or_else(not_installed)?;
    recover(ctx).await?;
    progress(Step::Download);
    let latest = release::latest(&ctx.github).await?;
    let mode = if mode == Mode::Confirmed && expected.is_some_and(|tag| tag != latest.tag) { Mode::Manual } else { mode };
    update_to(ctx, mode, &latest, progress).await
}

/// [`update`] para a release `latest` já achada: a passada do relógio procura fora da vez ([`auto_wants`]) e só
/// a troca roda na vez. A instalação e a última procura são relidas aqui, porque podem ter mudado no meio.
///
/// # Errors
///
/// As de [`update`].
pub async fn update_to(ctx: &Context, mode: Mode, latest: &Release, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let old = current(&ctx.paths).ok_or_else(not_installed)?;
    // Sem isto o relógio baixaria a mesma release toda hora para parar no mesmo ponto; tentar de novo é da tela.
    if mode == Mode::Auto && release::newer(&latest.tag, &old.tag) && skip_known(&ctx.paths, &latest.tag) {
        return Ok(Outcome::Skipped);
    }
    let mut attempt = Attempt::default();
    let result = switch(ctx, mode, &old, latest, &mut attempt, progress).await;
    record_check(&ctx.paths, &latest.tag, &attempt, &result);
    result
}

/// O que uma tentativa ficou sabendo antes de terminar.
#[derive(Clone, Copy, Debug, Default)]
struct Attempt {
    /// Trocar muda o formato dos dados; `None` quando ela parou antes de perguntar.
    migration: Option<bool>,
    /// O serviço já foi registrado: a falha daqui em diante prende a tag.
    registered: bool,
    /// Um dos esquemas não se leu (prazo, arquivo ocupado): a confirmação pedida não prende a tag, e a próxima
    /// passada pergunta de novo.
    schema_unread: bool,
}

/// O `last_error` da confirmação pedida sem um dos esquemas lidos.
const SCHEMA_UNREAD: &str = "whatsapp_schema_unread";

/// A última procura viu `tag` esperando confirmação, ou falhando depois de registrar a nova: o relógio não a tenta,
/// e a procura gravada só ganha a hora nova. Uma `migration` que a tentativa não chegou a ler (`last_error`) não
/// prende a tag: ela foi herdada ou suposta.
fn skip_known(paths: &Paths, tag: &str) -> bool {
    let Some(check) = read_update_check(paths)
        .filter(|c| c.latest == tag && (c.failed.is_some() || (c.migration && c.last_error.is_none()))) else { return false };
    save_check(paths, &UpdateCheck { checked_at: now(), ..check });
    true
}

/// [`write_update_check`] de quem não tem a quem devolver a falha: ela vai ao log.
fn save_check(paths: &Paths, check: &UpdateCheck) {
    if let Err(failure) = write_update_check(paths, check) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "whatsapp: a procura de atualização não ficou gravada");
    }
}

/// [`update`] depois de achar a release mais nova; `attempt` recebe o que se sabe assim que se sabe.
async fn switch(ctx: &Context, mode: Mode, old: &Installed, latest: &Release, attempt: &mut Attempt,
    progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    if !release::newer(&latest.tag, &old.tag) {
        attempt.migration = Some(false);
        // O relógio não mexe no serviço: religar a instalada é da tela, onde a pessoa vê o resultado.
        return match mode {
            Mode::Auto => Ok(Outcome::AlreadyLatest),
            Mode::Manual | Mode::Confirmed => ensure_up(ctx, old, &mut attempt.registered, progress).await,
        };
    }
    // O fetch reaproveita o programa que já está na pasta: ele pode ser o que o serviço roda, e não sai daqui.
    let (binary, fetched) = release::BINARY.fetch_new(&ctx.github, latest, ctx.platform, &ctx.paths.service).await?;
    let (version, new_schema, file_schema) = tokio::join!(release::check(&binary), release::schema_of(&binary),
        release::data_schema_of(&binary, &ctx.paths.data));
    let version = match version {
        Ok(version) => version,
        Err(failure) => {
            discard_download(old, &binary, fetched);
            return Err(failure);
        }
    };
    let changes = format_changes(new_schema, file_schema);
    tracing::info!(?new_schema, ?file_schema, changes, "whatsapp: esquema do arquivo e da versão nova");
    attempt.migration = Some(changes);
    attempt.schema_unread = new_schema.is_none() || file_schema.is_none();
    if changes && mode != Mode::Confirmed {
        tracing::info!(from = %old.tag, to = %latest.tag, "whatsapp: a versão nova pode mudar o formato dos dados; esperando confirmação");
        discard_download(old, &binary, fetched);
        return Ok(Outcome::NeedsConfirm);
    }
    let client = Client::new(ctx.paths.data.clone());
    // Trocar religa o programa e derruba o pareamento em curso: o relógio deixa para a próxima passada.
    if mode == Mode::Auto && client.status().await.is_ok_and(|status| pairing(&status)) {
        tracing::info!(from = %old.tag, to = %latest.tag, "whatsapp: pareamento em curso; a troca fica para a próxima passada");
        discard_download(old, &binary, fetched);
        return Ok(Outcome::Skipped);
    }
    wait_sends(&client).await;
    let before = client.claimed().map(|c| c.pid);
    let recorded_before = recorded_pid(&ctx.paths.data);
    // A cópia dos dados que a volta e a espera usam é só a que a nova gravou nesta tentativa.
    let since = SystemTime::now();
    progress(Step::Service);
    attempt.registered = true;
    // No Windows o registro para o processo antigo antes de poder falhar ao ligar o novo.
    let registered = service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&binary)).await.map_err(Failure::from);
    let switched = registered.is_ok();
    progress(Step::Start);
    // O registro só passa para a versão nova depois de ela responder: senão a próxima tentativa diria
    // "atualizado" sem nada rodando. Sem o registro gravado, o serviço também volta: ele e o registro
    // apontam sempre para a mesma versão.
    let migrating = Migrating { home: &ctx.paths.home, data: &ctx.paths.data, since };
    let up = match registered {
        Ok(()) => wait_up(&client, &version, before, Some(migrating)).await,
        Err(failure) => Err(failure),
    };
    let new = Installed { tag: latest.tag.clone(), binary };
    if let Err(failure) = up {
        // Sem a cópia a nova não migrou e deixou o arquivo como estava: a anterior volta sobre ele.
        if switched && backup_failed(&ctx.paths.home, &new.binary).await {
            tracing::warn!(from = %old.tag, to = %new.tag, "whatsapp: a versão nova não gravou a cópia dos dados; voltando para a anterior");
            let back = rollback(ctx, old, Failure::new("whatsapp_backup_failed", format!(
                "{}; {} não conseguiu gravar a cópia dos dados antes de migrar (saída {BACKUP_FAILED_EXIT}) e não mexeu no messages.db",
                failure.detail, new.tag))).await;
            discard_unless_kept(old, &new.binary, fetched, &back);
            return Err(back);
        }
        let source = restore_source(mode, &ctx.paths.data, file_schema, since);
        return Err(match after_failure(opened_after(switched, recorded_before, &ctx.paths.data, since), source,
            same_format(new_schema, file_schema)) {
            AfterFailure::Restore(schema) => restore(ctx, old, &new, schema, fetched, since, failure).await,
            AfterFailure::KeepNew => keep_new(&ctx.paths, &new, failure),
            AfterFailure::Rollback => {
                let back = rollback(ctx, old, failure).await;
                discard_unless_kept(old, &new.binary, fetched, &back);
                back
            }
        });
    }
    // A nova respondeu e só o registro não gravou: os dados são dela, e a cópia não volta.
    if let Err(failure) = write_record(&ctx.paths, &new) {
        return Err(if opened_after(switched, recorded_before, &ctx.paths.data, since) {
            keep_new(&ctx.paths, &new, failure)
        } else {
            rollback(ctx, old, failure).await
        });
    }
    if let Some(dir) = old.binary.parent().filter(|d| Some(*d) != new.binary.parent()) {
        remove_version_dir(dir, "whatsapp: atualizado, mas a pasta da versão anterior ficou");
    }
    Ok(Outcome::Updated)
}

/// O formato só fica igual quando a nova diz o esquema que grava e o arquivo já está nele (ou adiante), ou não
/// existe (0: nada a converter); quem não diz conta como mudança, porque subir o `user_version` não tem volta. O
/// programa instalado não é consultado: o arquivo é a verdade, e a versão anterior pode nem ter `--schema-version`.
fn format_changes(new: Option<u32>, file: Option<u32>) -> bool {
    !matches!((new, file), (Some(new), Some(file)) if file == 0 || file >= new)
}

/// O arquivo existia e já estava no esquema da nova, que só acrescenta colunas sem subir o `user_version`: a
/// anterior o abre. Com arquivo 0 a nova o criou no esquema dela.
fn same_format(new: Option<u32>, file: Option<u32>) -> bool {
    !format_changes(new, file) && file.is_some_and(|file| file > 0)
}

/// O último processo do serviço saiu com [`BACKUP_FAILED_EXIT`]. Gerenciador que não responde não prova nada.
async fn backup_failed(home: &Path, binary: &Path) -> bool {
    service::process_state(home, &SERVICE, binary).await
        .is_ok_and(|process| process.last_exit == Some(i32::from(BACKUP_FAILED_EXIT)))
}

/// [`discard_download`] depois de um [`rollback`]: sem a anterior de volta, o serviço pode seguir apontando para a nova.
fn discard_unless_kept(old: &Installed, binary: &Path, fetched: bool, back: &Failure) {
    if back.code != "whatsapp_update_no_rollback" {
        discard_download(old, binary, fetched);
    }
}

/// Um programa baixado e não confirmado não fica na pasta: a próxima tentativa o confere de novo. Só sai o que
/// esta chamada baixou (`fetched`): o que já estava na pasta pode ser o que o serviço roda, com o registro atrás.
fn discard_download(old: &Installed, binary: &Path, fetched: bool) {
    if fetched && let Some(dir) = binary.parent().filter(|d| Some(*d) != old.binary.parent()) {
        remove_version_dir(dir, "whatsapp: a versão baixada e não usada ficou na pasta");
    }
}

/// A pasta de uma versão do programa; a que não sai só ocupa disco, e vai ao log com `why`.
fn remove_version_dir(dir: &Path, why: &'static str) {
    if let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "{why}");
    }
}

/// A falha depois de registrar a nova (`failed`) fica com a tag até uma tentativa dar certo; a espera por
/// confirmação não a apaga, para a tela continuar dizendo que a última troca falhou. A que veio antes
/// (`last_error`) só descreve a última tentativa.
fn record_check(paths: &Paths, latest: &str, attempt: &Attempt, result: &Result<Outcome, Failure>) {
    let previous = read_update_check(paths).filter(|c| c.latest == latest);
    let kept = || previous.as_ref().and_then(|c| c.failed.clone());
    let (failed, last_error) = match result {
        Ok(Outcome::NeedsConfirm) => (kept(), attempt.schema_unread.then(|| CheckFailure { tag: latest.to_owned(), code: SCHEMA_UNREAD.to_owned() })),
        Ok(_) => (None, None),
        Err(failure) => {
            let failure = CheckFailure { tag: latest.to_owned(), code: failure.code.to_owned() };
            if attempt.registered { (Some(failure), None) } else { (kept(), Some(failure)) }
        }
    };
    // Sem a resposta do programa novo (o download falhou), a mudança de formato vale como certa.
    let migration = attempt.migration.or(previous.map(|c| c.migration)).unwrap_or(true);
    save_check(paths, &UpdateCheck { latest: latest.to_owned(), migration, checked_at: now(), failed, last_error });
}

/// Sem release mais nova, a instalação registrada só está em dia se responde assinada com a versão dela: uma
/// atualização que manteve a nova registrada depois de falhar deixa o serviço parado, e repetir religa.
async fn ensure_up(ctx: &Context, installed: &Installed, registered: &mut bool, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
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
    *registered = true;
    service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&installed.binary)).await?;
    progress(Step::Start);
    wait_up(&client, &version, before, None).await?;
    Ok(Outcome::Updated)
}

/// Tira o serviço e apaga o programa e o registro. Os dados (`--data`) e o aceite ficam.
///
/// # Errors
///
/// Falha do gerenciador de serviços ou ao apagar a pasta do programa.
pub async fn uninstall(ctx: &Context) -> Result<(), Failure> {
    // O marcador é a única pista dos arquivos fora do lugar: remover só com eles de volta.
    settle_pending(&ctx.paths)?;
    service::unregister(&ctx.paths.home, &ctx.paths.service, &SERVICE).await?;
    let gone = |path: &Path, e: std::io::Error| Failure::new("whatsapp_remove_failed", format!("{}: {e}", path.display()));
    let bin = ctx.paths.service.join("bin");
    match std::fs::remove_dir_all(&bin) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(gone(&bin, e)),
        _ => {}
    }
    for file in [ctx.paths.restore_pending(), ctx.paths.record()] {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(gone(&file, e)),
            _ => {}
        }
    }
    Ok(())
}

/// Uma troca de versão em curso: a nova grava a cópia `messages.db.schema<v>.bak` antes de migrar, e só grava o
/// `control.json` depois.
#[derive(Clone, Copy, Debug)]
struct Migrating<'a> {
    home: &'a Path,
    data: &'a Path,
    /// O começo da tentativa: cópia mais velha é de outra.
    since: SystemTime,
}

impl Migrating<'_> {
    /// A nova está migrando: a cópia desta tentativa (ou a que ela ainda escreve) está no disco e o processo vive.
    async fn in_progress(&self) -> bool {
        fresh_copy(self.data, self.since, true)
            && tokio::time::timeout(QUERY_TIMEOUT, service::process_running(self.home, &SERVICE)).await.is_ok_and(|r| r.unwrap_or(false))
    }
}

/// Espera o processo novo responder ao `status` com assinatura válida e a versão esperada. Numa troca
/// (`migrating`), passado o [`START_TIMEOUT`], a espera segue até o [`MIGRATION_TIMEOUT`] enquanto a nova estiver
/// migrando os dados ([`Migrating::in_progress`]).
async fn wait_up(client: &Client, version: &str, before: Option<u32>, migrating: Option<Migrating<'_>>) -> Result<(), Failure> {
    let started = Instant::now();
    let mut last = String::from("sem control.json");
    loop {
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
        let mut pause = Duration::from_millis(500);
        if started.elapsed() >= START_TIMEOUT {
            // Depois de gravar o `control.json` a nova já abriu os dados: cair e religar daí em diante não é migrar.
            let opened = client.claimed().is_some_and(|control| Some(control.pid) != before);
            let busy = match migrating {
                Some(migrating) if !opened => migrating.in_progress().await,
                _ => false,
            };
            let limit = if busy { MIGRATION_TIMEOUT } else { START_TIMEOUT };
            if busy {
                last = "migrando os dados".into();
                pause = MIGRATION_POLL;
            }
            if started.elapsed() >= limit {
                break;
            }
        }
        tokio::time::sleep(pause).await;
    };
    Err(Failure::new("whatsapp_not_started", format!("o serviço não respondeu em {}s ({last})", started.elapsed().as_secs())))
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
/// `control.json`, e voltar a anterior sobre o banco migrado a deixaria saindo com 3. `same_format`: o arquivo
/// existia e já estava no esquema da nova, que só acrescenta colunas sem subir o `user_version`, e a anterior o
/// abre; ficar com a nova deixaria o WhatsApp parado. Fora isso, quem abriu os dados fica com eles (com arquivo
/// novo, a nova o criou no esquema dela).
fn after_failure(opened: bool, source: Option<u32>, same_format: bool) -> AfterFailure {
    match (source, opened) {
        (Some(schema), _) => AfterFailure::Restore(schema),
        (None, true) if !same_format => AfterFailure::KeepNew,
        (None, _) => AfterFailure::Rollback,
    }
}

/// Registra a anterior de novo, sobre dados que a nova não abriu, e espera ela responder: o gerenciador dar o
/// serviço por iniciado não prova que ela subiu.
async fn rollback(ctx: &Context, old: &Installed, failure: Failure) -> Failure {
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    let back = match service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await {
        Ok(()) => match release::check(&old.binary).await {
            Ok(version) => wait_up(&client, &version, before, None).await,
            Err(e) => Err(e),
        },
        Err(e) => Err(Failure::from(e)),
    };
    match back {
        Ok(()) => failure,
        Err(e) => Failure::new("whatsapp_update_no_rollback",
            format!("{}; e o serviço não voltou para {}: {}", failure.detail, old.tag, e.detail)),
    }
}

/// Quanto a troca espera um envio aprovado sair: parar o programa no meio o deixaria `unconfirmed`.
const SEND_WAIT: Duration = Duration::from_secs(30);

/// Espera nenhum rascunho estar `sending`; com a vez do instalador, outro envio não começa. Serviço fora, binário
/// sem a rota ou resposta que não se lê não seguram a troca; o prazo estourado vai ao log e a troca segue.
async fn wait_sends(client: &Client) {
    let deadline = Instant::now() + SEND_WAIT;
    while client.call(reqwest::Method::GET, "/v1/outbox", &[("state", "sending")], None).await.is_ok_and(|reply| still_sending(&reply)) {
        if Instant::now() >= deadline {
            tracing::warn!(secs = SEND_WAIT.as_secs(), "whatsapp: um envio continuava saindo; a troca segue");
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// O `status` do binário diz pareamento em curso (QR na tela ou código no celular).
fn pairing(status: &Value) -> bool {
    status["state"] == "pairing"
}

/// A lista `{"drafts": [...]}` do `GET /v1/outbox?state=sending` tem algum rascunho.
fn still_sending(reply: &crate::client::Reply) -> bool {
    reply.status == 200 && serde_json::from_slice::<Value>(&reply.body)
        .is_ok_and(|body| body["drafts"].as_array().is_some_and(|drafts| !drafts.is_empty()))
}

/// Devolve a cópia que a nova gravou antes de migrar e religa a anterior: o serviço para sem sair do gerenciador
/// (registrar a anterior com a nova de pé deixaria ela abrir o banco migrado e sair com 3), a cópia vai para o
/// lugar do `messages.db` e a anterior é registrada. Sem a parada confirmada ou com a troca dos arquivos falhando,
/// fica o [`keep_new`]; na troca que falha, a nova sobe de novo ([`after_restore_failed`]). `fetched`: a nova foi baixada nesta chamada.
async fn restore(ctx: &Context, old: &Installed, new: &Installed, schema: u32, fetched: bool, since: SystemTime, failure: Failure) -> Failure {
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
    let swapped = tokio::task::spawn_blocking(move || restore_backup(&data, schema, &mut || std::thread::sleep(UNDO_PAUSE))).await
        .unwrap_or_else(|e| Err(NotRestored { why: format!("a troca dos arquivos parou no meio: {e}"), stuck: Vec::new(), interrupted: true }));
    if let Err(failed) = swapped {
        let why = after_restore_failed(&ctx.paths, failed, || service::start(&ctx.paths.home, &SERVICE),
            || service::set_autostart(&ctx.paths.home, &SERVICE, false)).await;
        return keep_new(&ctx.paths, new, not_restored(why));
    }
    let client = Client::new(ctx.paths.data.clone());
    let before = client.claimed().map(|c| c.pid);
    let back = match service::register(&ctx.paths.home, &ctx.paths.service, &ctx.service(&old.binary)).await {
        Ok(()) => wait_up(&client, &version, before, None).await,
        Err(e) => Err(Failure::from(e)),
    };
    match back {
        Ok(()) => {
            tracing::warn!(from = %old.tag, to = %new.tag, schema, "whatsapp: a versão nova não respondeu; os dados e a anterior voltaram");
            discard_download(old, &new.binary, fetched);
            Failure::new("whatsapp_update_restored", format!(
                "{}; os dados voltaram para a cópia de antes da troca e {} voltou a rodar; mensagens que chegaram durante \
                 a tentativa (até uns {}s) podem não voltar; o banco da tentativa ficou em messages.db.migrated",
                failure.detail, old.tag, since.elapsed().map_or(0, |d| d.as_secs())))
        }
        Err(e) => Failure::new("whatsapp_update_no_rollback", format!(
            "{}; os dados voltaram para a cópia de antes da troca (o banco da tentativa ficou em messages.db.migrated), \
             mas {} não voltou a rodar: {}", failure.detail, old.tag, e.detail)),
    }
}

/// A cópia não voltou e o serviço está parado. Com os arquivos do banco no lugar, a nova, que segue registrada,
/// sobe de novo sobre os dados migrados; fora do lugar, ela criaria um banco vazio e fica parada.
async fn after_restore_failed<S, SF, O, OF>(paths: &Paths, failed: NotRestored, start: S, off: O) -> String
where
    S: FnOnce() -> SF,
    SF: std::future::Future<Output = Result<(), service::Error>>,
    O: FnOnce() -> OF,
    OF: std::future::Future<Output = Result<(), service::Error>>,
{
    if !failed.in_place() {
        // Sem os pares (a troca parou no meio), não há o que pôr de volta sozinho: fica só o serviço parado.
        if failed.stuck.is_empty() {
            return format!("{}; o serviço ficou parado", failed.why);
        }
        let held = match write_pending(paths, &failed.stuck) {
            Ok(()) => "; o Hangar põe os arquivos de volta na próxima passada ou no próximo Atualizar".to_owned(),
            Err(e) => format!("; e o registro dos arquivos presos não gravou: {e}"),
        };
        let logon = match off().await {
            Ok(()) => "; até lá a subida no logon fica desligada".to_owned(),
            Err(e) => format!("; e a subida no logon não desligou: {}: {}", e.step, e.detail),
        };
        return format!("{}; o serviço ficou parado{held}{logon}", failed.why);
    }
    match start().await {
        Ok(()) => format!("{}; a versão nova foi religada sobre os dados migrados", failed.why),
        Err(e) => format!("{}; e a versão nova não religou: {}: {}; o serviço ficou parado", failed.why, e.step, e.detail),
    }
}

/// O código do Atualizar e do Instalar recusados enquanto arquivos de uma volta seguem fora do lugar.
pub const RESTORE_PENDING: &str = "whatsapp_restore_pending";

/// Põe de volta os arquivos que uma volta deixou fora do lugar ([`Paths::restore_pending`]), religa a subida no
/// logon e sobe o serviço. `Ok(false)` sem nada pendente; `Ok(true)` quando voltaram e o serviço subiu.
///
/// # Errors
///
/// [`RESTORE_PENDING`] com os arquivos que seguem presos; a falha do gerenciador ao religar.
pub async fn recover(ctx: &Context) -> Result<bool, Failure> {
    recover_with(&ctx.paths, || async {
        let installed = current(&ctx.paths).ok_or_else(not_installed)?;
        let version = release::check(&installed.binary).await?;
        let client = Client::new(ctx.paths.data.clone());
        let before = client.claimed().map(|c| c.pid);
        service::set_autostart(&ctx.paths.home, &SERVICE, true).await?;
        service::start(&ctx.paths.home, &SERVICE).await?;
        // O gerenciador dar o serviço por iniciado não prova que ele abriu o banco: o marcador só sai com a resposta.
        wait_up(&client, &version, before, None).await
    }).await
}

/// Grava os arquivos presos em [`Paths::restore_pending`].
fn write_pending(paths: &Paths, stuck: &[Stuck]) -> Result<(), String> {
    let body = serde_json::to_vec(stuck).map_err(|e| e.to_string())?;
    write_file(&paths.restore_pending(), &body)
}

/// Há arquivos de uma volta fora do lugar: o marcador, ou o banco ausente com o migrado ao lado (o marcador que não
/// gravou). A passada do relógio recupera antes de procurar versão, e na vez do instalador.
pub fn recovery_pending(paths: &Paths) -> bool {
    paths.restore_pending().exists() || !implicit_pending(paths).is_empty()
}

/// [`recover`] pela passada do relógio, já na vez: o resultado vai ao diário.
pub async fn auto_recover(ctx: &Context) -> Option<AutoUpdate> {
    let tag = current(&ctx.paths)?.tag;
    let result = recover(ctx).await.map(|_| Outcome::Restored);
    Some(AutoUpdate { from: tag.clone(), to: tag, result })
}

/// Sem marcador, o banco ausente com o migrado ao lado é o par preso de uma volta cujo marcador não gravou: subir
/// criaria um banco vazio por cima.
fn implicit_pending(paths: &Paths) -> Vec<Stuck> {
    let name = |suffix: &str| paths.data.join(format!("messages.db{suffix}"));
    let at = |suffix: &str| paths.data.join(format!("{MIGRATED}{suffix}"));
    if name("").try_exists().unwrap_or(true) || !at("").try_exists().unwrap_or(false) {
        return Vec::new();
    }
    DB_FILES.iter().filter(|suffix| at(suffix).exists() && !name(suffix).exists())
        .map(|suffix| Stuck { name: name(suffix), at: at(suffix) }).collect()
}

/// [`recover`] com quem religa o serviço.
async fn recover_with<F, Fut>(paths: &Paths, up: F) -> Result<bool, Failure>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), Failure>>,
{
    if !settle_pending(paths)? {
        return Ok(false);
    }
    // O marcador só sai com o serviço de pé: religar que falha fica para a próxima, que acha tudo no lugar e só religa.
    up().await?;
    let file = paths.restore_pending();
    match std::fs::remove_file(&file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(Failure::new(RESTORE_PENDING, format!("{}: {e}", file.display()))),
        _ => {}
    }
    tracing::info!("whatsapp: os arquivos do banco voltaram ao lugar e o serviço respondeu");
    Ok(true)
}

/// Põe de volta o que o marcador lista. `Ok(false)` sem marcador; `Ok(true)` com tudo no lugar e o marcador ainda
/// no disco, para quem chama decidir quando ele sai.
///
/// # Errors
///
/// [`RESTORE_PENDING`] com os arquivos que seguem presos, ou o marcador que não se lê.
fn settle_pending(paths: &Paths) -> Result<bool, Failure> {
    let file = paths.restore_pending();
    let stuck: Vec<Stuck> = match std::fs::read(&file) {
        // A gravação é atômica: ilegível é mão de fora, e sem os pares não há como saber onde estão os arquivos.
        Ok(body) => serde_json::from_slice(&body).map_err(|e| Failure::new(RESTORE_PENDING, format!(
            "{}: {e}; confira os messages.db.migrated* em {} e apague esse registro", file.display(), paths.data.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match implicit_pending(paths) {
            implicit if implicit.is_empty() => return Ok(false),
            implicit => implicit,
        },
        Err(e) => return Err(Failure::new(RESTORE_PENDING, format!("{}: {e}", file.display()))),
    };
    let left: Vec<Stuck> = stuck.into_iter().filter(|s| !settle(s)).collect();
    if !left.is_empty() {
        let _ = write_pending(paths, &left);
        return Err(Failure::new(RESTORE_PENDING, format!("arquivos do banco fora do lugar desde uma volta anterior: {}", stuck_list(&left))));
    }
    Ok(true)
}

/// Só com a confirmação do dono, o esquema `v` da cópia `messages.db.schema<v>.bak` que a nova gravou desde
/// `since`: a do esquema que o arquivo tinha antes da troca, ou, quando ele não se leu, a única gravada nesta
/// tentativa. Cópia de outra tentativa não serve: devolvê-la apagaria o que chegou depois dela.
fn restore_source(mode: Mode, data: &Path, file_schema: Option<u32>, since: SystemTime) -> Option<u32> {
    if mode != Mode::Confirmed {
        return None;
    }
    let fresh = |schema: &u32| std::fs::metadata(backup_path(data, *schema)).and_then(|m| m.modified()).is_ok_and(|at| at >= since);
    if let Some(schema) = file_schema {
        return Some(schema).filter(fresh);
    }
    let mut found = std::fs::read_dir(data).ok()?
        .filter_map(|entry| backup_schema(entry.ok()?.file_name().to_str()?, false))
        .filter(fresh);
    let schema = found.next()?;
    found.next().is_none().then_some(schema)
}

/// O começo do nome de toda cópia que o binário grava ao lado do `messages.db` antes de migrar.
const BACKUP_PREFIX: &str = "messages.db.schema";
/// O começo do conjunto migrado que a volta tira do caminho (`messages.db.migrated`, `-wal`…).
pub(crate) const MIGRATED: &str = "messages.db.migrated";
/// A cópia a caminho do lugar do `messages.db`, durante a volta.
pub(crate) const RESTORING: &str = "messages.db.restoring";

/// O esquema `v` de uma cópia `messages.db.schema<v>.bak`; com `with_tmp`, também da `.bak.tmp` que o binário
/// ainda escreve antes de renomear.
pub(crate) fn backup_schema(name: &str, with_tmp: bool) -> Option<u32> {
    let rest = name.strip_prefix(BACKUP_PREFIX)?;
    rest.strip_suffix(".bak").or_else(|| rest.strip_suffix(".bak.tmp").filter(|_| with_tmp))?.parse().ok()
}

fn backup_path(data: &Path, schema: u32) -> PathBuf {
    data.join(format!("{BACKUP_PREFIX}{schema}.bak"))
}

/// Uma cópia `messages.db.schema<v>.bak` gravada desde `since`; com `writing`, também a `.bak.tmp` que o binário
/// ainda escreve antes de renomear.
fn fresh_copy(data: &Path, since: SystemTime, writing: bool) -> bool {
    let Ok(entries) = std::fs::read_dir(data) else { return false };
    entries.flatten().any(|entry| {
        entry.file_name().to_str().and_then(|name| backup_schema(name, writing)).is_some()
            && entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at >= since)
    })
}

/// Para o serviço e espera o pid do `control.json` sair e o gerenciador dizer que nenhum processo ficou: o
/// gerenciador sozinho não vê um programa que escapou dele. O pid é olhado antes porque é barato; no Windows cada
/// pergunta ao gerenciador abre um PowerShell.
async fn stop_confirmed(home: &Path, managed_dir: &Path, data: &Path) -> Result<(), String> {
    let stuck = || format!("o gerenciador de serviços não respondeu em {}s", QUERY_TIMEOUT.as_secs());
    tokio::time::timeout(QUERY_TIMEOUT, service::stop(home, managed_dir, &SERVICE)).await.map_err(|_| stuck())?
        .map_err(|e| format!("o serviço não parou: {}: {}", e.step, e.detail))?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        let pid = live_pid(data)?;
        if pid.is_none() && !tokio::time::timeout(QUERY_TIMEOUT, service::process_running(home, &SERVICE)).await.map_err(|_| stuck())?
            .map_err(|e| format!("não consegui confirmar que o serviço parou: {}: {}", e.step, e.detail))? {
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
pub(crate) const DB_FILES: [&str; 4] = ["", "-wal", "-shm", "-journal"];

/// Põe a cópia `messages.db.schema<schema>.bak` no lugar do `messages.db`, com o programa parado. Nada se apaga:
/// o conjunto migrado sai do caminho para `messages.db.migrated*`, onde fica até a próxima volta, e a cópia
/// continua para uma segunda tentativa. A cópia entra por um arquivo à parte gravado no disco antes da troca.
///
/// # Errors
///
/// O que falhou e o estado em que os arquivos ficaram: o que tinha saído do caminho volta.
fn restore_backup(data: &Path, schema: u32, pause: &mut dyn FnMut()) -> Result<(), NotRestored> {
    let backup = backup_path(data, schema);
    let staged = data.join(RESTORING);
    let copied = std::fs::copy(&backup, &staged)
        .and_then(|_| std::fs::OpenOptions::new().write(true).open(&staged)?.sync_all());
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&staged);
        return Err(NotRestored { why: format!("{}: {e}; nada mudou", backup.display()), stuck: Vec::new(), interrupted: false });
    }
    let mut moved = Vec::new();
    for suffix in DB_FILES {
        let (from, to) = (data.join(format!("messages.db{suffix}")), data.join(format!("{MIGRATED}{suffix}")));
        // Sem o arquivo agora, o da volta anterior com o mesmo nome iria junto com o banco desta.
        let (result, path) = if from.exists() {
            (hangar_core::fs::replace(&from, &to).map(|()| true), &from)
        } else {
            (std::fs::remove_file(&to).map(|()| false).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(false) } else { Err(e) }), &to)
        };
        match result {
            Ok(true) => moved.push((from.clone(), to.clone())),
            Ok(false) => {}
            Err(e) => return Err(retry(undo(&staged, &moved, format!("{}: {e}", path.display())), pause)),
        }
    }
    hangar_core::fs::replace(&staged, &data.join("messages.db")).map_err(|e| retry(undo(&staged, &moved, format!("{}: {e}", staged.display())), pause))
}

/// A volta da cópia que falhou, com os arquivos do banco que não voltaram ao lugar; `interrupted`: a troca parou
/// no meio e não se sabe onde eles estão.
#[derive(Debug)]
struct NotRestored {
    why: String,
    stuck: Vec<Stuck>,
    interrupted: bool,
}

impl NotRestored {
    /// Os arquivos do banco estão onde estavam antes da volta.
    fn in_place(&self) -> bool {
        self.stuck.is_empty() && !self.interrupted
    }
}

/// Um arquivo do banco fora do lugar: estava em `name` e ficou em `at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stuck {
    name: PathBuf,
    at: PathBuf,
}

/// Quantas vezes a volta tenta de novo o que ficou preso, e a espera entre elas: quem segura o arquivo logo
/// depois da parada (antivírus, indexador) costuma soltá-lo em segundos.
const UNDO_RETRIES: usize = 3;
const UNDO_PAUSE: Duration = Duration::from_secs(3);

/// Tenta de novo pôr no lugar o que ficou preso, com `pause` antes de cada tentativa.
fn retry(mut failed: NotRestored, pause: &mut dyn FnMut()) -> NotRestored {
    if failed.stuck.is_empty() {
        return failed;
    }
    for _ in 0..UNDO_RETRIES {
        pause();
        failed.stuck.retain(|stuck| !settle(stuck));
        if failed.stuck.is_empty() {
            failed.why.push_str("; o que tinha ficado preso voltou na repetição");
            break;
        }
    }
    failed
}

/// O arquivo preso está de volta no lugar. Com os dois no disco, o do lugar pode ter dados novos, e nada é
/// sobrescrito; sem nenhum dos dois, ou sem conseguir olhar, a pendência fica: subir criaria um banco vazio.
fn settle(stuck: &Stuck) -> bool {
    match (stuck.name.try_exists(), stuck.at.try_exists()) {
        (Ok(true), Ok(false)) => true,
        (Ok(false), Ok(true)) => hangar_core::fs::replace(&stuck.at, &stuck.name).is_ok(),
        _ => false,
    }
}

/// Os pares presos, para a mensagem.
fn stuck_list(stuck: &[Stuck]) -> String {
    stuck.iter().map(|s| format!("{} ficou como {}", s.name.display(), s.at.display())).collect::<Vec<_>>().join("; ")
}

/// Põe de volta o que saiu do caminho e diz como os arquivos ficaram.
fn undo(staged: &Path, moved: &[(PathBuf, PathBuf)], why: String) -> NotRestored {
    let _ = std::fs::remove_file(staged);
    let mut stuck = Vec::new();
    let mut said = Vec::new();
    for (from, to) in moved.iter().rev() {
        if let Err(e) = hangar_core::fs::replace(to, from) {
            said.push(format!("{} ficou como {}: {e}", from.display(), to.display()));
            stuck.push(Stuck { name: from.clone(), at: to.clone() });
        }
    }
    if stuck.is_empty() {
        NotRestored { why: format!("{why}; os arquivos do banco voltaram como estavam"), stuck, interrupted: false }
    } else {
        NotRestored { why: format!("{why}; e não voltaram: {}", said.join("; ")), stuck, interrupted: false }
    }
}

/// O pid do `control.json` sem conferir se o processo vive: a nova pode gravar o arquivo e morrer antes da
/// conferência, e a prova de que ela abriu os dados tem que sobreviver a isso.
fn recorded_pid(data: &Path) -> Option<u32> {
    control::read_control(data).ok().map(|c| c.pid)
}

/// [`opened_by_new`] pelo arquivo depois da falha; arquivo que não se lê conta como aberto, porque voltar sobre
/// dados migrados deixa o WhatsApp parado. A cópia que a nova grava desde `since` também conta: ela a grava antes
/// de subir o `user_version`, e o `control.json` só vem depois.
fn opened_after(registered: bool, before: Option<u32>, data: &Path, since: SystemTime) -> bool {
    if registered && fresh_copy(data, since, false) {
        return true;
    }
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

    /// Dados de esquema mais novo (saída 3) ou cópia de antes de migrar que não gravou (saída 4): o systemd para de
    /// religar a cada 10 s e a unit fica `failed` com a saída.
    #[test]
    fn the_unit_does_not_restart_after_the_schema_newer_exit() {
        let ctx = Context::new(Path::new("/home/u"));
        let unit = service::systemd_unit(&ctx.service(Path::new("/home/u/.hangar/whatsapp-service/bin/0.1.0/hangar-whatsapp")));
        assert!(unit.contains("Restart=always\nRestartSec=10\nRestartPreventExitStatus=3 4\n\n[Install]\n"), "{unit}");
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

    /// Programa de mentira: `--version` diz `version`; `--schema-version` diz `schema` e `--data-schema-version`,
    /// `data`; `None` sai 2, como o binário que não conhece a flag.
    #[cfg(unix)]
    fn fake_program(path: &Path, version: &str, schema: Option<u32>, data: Option<u32>) {
        let answer = |n: Option<u32>| n.map_or_else(|| "exit 2".to_owned(), |n| format!("echo {n}"));
        let (schema, data) = (answer(schema), answer(data));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\ncase \"$1\" in\n--version) echo 'hangar-whatsapp {version} (api 1)' ;;\n\
            --schema-version) {schema} ;;\n--data-schema-version) {data} ;;\n*) exit 1 ;;\nesac\n")).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    /// [`format_changes`] perguntando ao programa `binary` sobre os dados em `data`, como a troca faz.
    #[cfg(unix)]
    async fn format_changes_of(binary: &Path, data: &Path) -> bool {
        let (new, file) = tokio::join!(release::schema_of(binary), release::data_schema_of(binary, data));
        format_changes(new, file)
    }

    /// Só o arquivo já no esquema da nova (ou adiante), ou sem arquivo, deixa o formato igual; quem não diz conta como mudança.
    #[test]
    fn the_data_format_stays_only_when_the_file_is_already_at_the_new_schema() {
        for (new, file, changes) in [(Some(2), Some(2), false), (Some(2), Some(3), false), (Some(2), Some(0), false),
            (Some(2), Some(1), true), (None, Some(2), true), (Some(2), None, true), (None, None, true), (None, Some(0), true)] {
            assert_eq!(format_changes(new, file), changes, "novo {new:?}, arquivo {file:?}");
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
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(!opened_after(true, None, data, since), "sem arquivo, a nova não abriu");
        std::fs::write(data.join("control.json"), "{").unwrap();
        assert!(opened_after(true, None, data, since));
        assert!(!opened_after(false, None, data, since), "sem o registro, a nova nem subiu");
    }

    fn set_mtime(path: &Path, at: SystemTime) {
        std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
    }

    /// A cópia que a nova gravou nesta tentativa prova que ela abriu os dados, antes do `control.json`; a `.bak.tmp`
    /// ainda sendo escrita não, e cópia de outra tentativa também não.
    #[test]
    fn a_copy_of_this_attempt_proves_the_new_version_opened_the_data() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        // Horas fixas: as cópias têm a hora que o teste dá a elas.
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let (fresh, stale) = (since + Duration::from_secs(5), since - Duration::from_secs(3600));
        let copy = |name: &str, at: SystemTime| {
            write_db_file(&data.join(name), "before");
            set_mtime(&data.join(name), at);
        };
        copy("messages.db.schema1.bak", stale);
        copy("messages.db.schema1.bak.tmp", fresh);
        assert!(!opened_after(true, None, data, since), "cópia velha e cópia pela metade");
        assert!(!fresh_copy(data, since, false));
        assert!(fresh_copy(data, since, true), "a cópia sendo escrita mostra a nova trabalhando");
        copy("messages.db.schema1.bak", fresh);
        assert!(opened_after(true, None, data, since), "sem control.json, a cópia desta tentativa basta");
        assert!(!opened_after(false, None, data, since), "sem o registro, a cópia é de outro processo");
        write_db_file(&data.join("messages.db.schemaX.bak"), "x");
        std::fs::remove_file(data.join("messages.db.schema1.bak")).unwrap();
        assert!(!fresh_copy(data, since, false), "nome que não é de cópia");
    }

    /// Os nomes de cópia que o binário grava, com o esquema; o resto (outro prefixo, esquema que não é número) não é cópia.
    #[test]
    fn copy_names_carry_their_schema() {
        assert_eq!(backup_schema("messages.db.schema1.bak", false), Some(1));
        assert_eq!(backup_schema("messages.db.schema12.bak", false), Some(12));
        assert_eq!(backup_schema("messages.db.schema1.bak.tmp", false), None, "a pela metade só com with_tmp");
        assert_eq!(backup_schema("messages.db.schema1.bak.tmp", true), Some(1));
        assert_eq!(backup_schema("messages.db.schemaX.bak", true), None);
        assert_eq!(backup_schema("session.db.schema1.bak", true), None);
        assert_eq!(backup_path(Path::new("d"), 2), Path::new("d").join("messages.db.schema2.bak"));
    }

    /// Sem unit do serviço (a casa de teste), o processo não conta como vivo e a cópia sozinha não estende a espera.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_migration_in_progress_extends_the_wait() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join("dados");
        std::fs::create_dir_all(&data).unwrap();
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        write_db_file(&data.join("messages.db.schema1.bak.tmp"), "before");
        set_mtime(&data.join("messages.db.schema1.bak.tmp"), since + Duration::from_secs(5));
        let migrating = Migrating { home: home.path(), data: &data, since };
        assert!(!migrating.in_progress().await, "sem processo do serviço, ninguém migra");
    }

    /// Sem unit ou plist o gerenciador não diz saída nenhuma: não é a cópia que falhou.
    #[cfg(unix)]
    #[tokio::test]
    async fn without_the_service_the_backup_did_not_fail() {
        let home = tempfile::tempdir().unwrap();
        assert!(!backup_failed(home.path(), &home.path().join("hangar-whatsapp")).await);
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
        restore_backup(data, 1, &mut || {}).unwrap();
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
        let failed = restore_backup(data, 1, &mut || {}).unwrap_err();
        assert!(failed.why.contains("messages.db.schema1.bak") && failed.why.ends_with("nada mudou"), "{}", failed.why);
        assert!(failed.in_place());
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
        let failed = restore_backup(data, 1, &mut || {}).unwrap_err();
        assert!(failed.why.contains("messages.db-shm") && failed.why.ends_with("os arquivos do banco voltaram como estavam"), "{}", failed.why);
        assert!(failed.in_place());
        assert_eq!(read(&data.join("messages.db")), "migrated");
        assert_eq!(read(&data.join("messages.db-wal")), "wal");
        assert_eq!(read(&data.join("messages.db-shm")), "shm");
        for gone in ["messages.db.migrated", "messages.db.migrated-wal", "messages.db.restoring"] {
            assert!(!data.join(gone).exists(), "{gone} ficou");
        }
        assert_eq!(read(&data.join("messages.db.schema1.bak")), "before");
    }

    /// A cópia que não volta com os arquivos no lugar religa a nova sobre os dados migrados, e o religar que falha
    /// aparece; com os arquivos fora do lugar, ela não sobe.
    #[tokio::test]
    async fn a_restore_that_fails_with_the_files_in_place_starts_the_new_version_again() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        write_db_file(&data.join("messages.db"), "migrated");
        let paths = Paths::new(dir.path());
        let never = || async { panic!("desligou a subida no logon com os arquivos no lugar") };
        let started = std::cell::Cell::new(false);
        let why = after_restore_failed(&paths, restore_backup(data, 1, &mut || {}).unwrap_err(), || async { started.set(true); Ok(()) }, never).await;
        assert!(started.get(), "a nova não foi religada");
        assert!(why.contains("religada") && !why.contains("ficou parado"), "{why}");

        let refused = || async { Err(service::Error { step: "systemctl restart".into(), detail: "recusou".into() }) };
        let why = after_restore_failed(&paths, restore_backup(data, 1, &mut || {}).unwrap_err(), refused, never).await;
        assert!(why.contains("systemctl restart: recusou") && why.contains("ficou parado"), "{why}");
        assert!(!paths.restore_pending().exists());
    }

    /// Preso que segue preso depois das tentativas: o marcador guarda cada par e a subida no logon desliga, para o
    /// logon não criar um `messages.db` vazio; nada religa.
    #[tokio::test]
    async fn a_file_still_stuck_is_recorded_and_the_logon_start_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let pair = Stuck { name: dir.path().join("messages.db"), at: dir.path().join("messages.db.migrated") };
        let stuck = NotRestored { why: "messages.db ficou como messages.db.migrated".into(), stuck: vec![pair.clone()], interrupted: false };
        let off = std::cell::Cell::new(false);
        let why = after_restore_failed(&paths, stuck, || async { panic!("religou com os arquivos fora do lugar") },
            || async { off.set(true); Ok(()) }).await;
        assert!(why.contains("ficou parado"), "{why}");
        assert!(off.get(), "a subida no logon ficou ligada");
        let held: Vec<Stuck> = serde_json::from_slice(&std::fs::read(paths.restore_pending()).unwrap()).unwrap();
        assert_eq!(held, [pair]);
    }

    /// O que ficou preso na volta é tentado de novo: o que solta o arquivo entre as tentativas deixa tudo no lugar;
    /// o que nunca solta segue preso depois das três.
    #[test]
    fn a_stuck_file_goes_back_when_it_is_released_between_tries() {
        let dir = tempfile::tempdir().unwrap();
        let (name, at) = (dir.path().join("messages.db"), dir.path().join("messages.db.migrated"));
        write_db_file(&at, "migrated");
        // Uma pasta com conteúdo no lugar do banco: o rename de volta falha até ela sair.
        std::fs::create_dir_all(name.join("x")).unwrap();
        let failed = || NotRestored { why: "x".into(), stuck: vec![Stuck { name: name.clone(), at: at.clone() }], interrupted: false };
        let mut pauses = 0;
        let kept = retry(failed(), &mut || pauses += 1);
        assert_eq!((kept.stuck.len(), pauses), (1, UNDO_RETRIES), "{kept:?}");
        let back = retry(failed(), &mut || { let _ = std::fs::remove_dir_all(&name); });
        assert!(back.in_place(), "{back:?}");
        assert_eq!(read(&name), "migrated");
    }

    /// A passada seguinte põe de volta o que estava preso, apaga o marcador e sobe o serviço; sem marcador, nada.
    #[tokio::test]
    async fn the_next_pass_puts_the_files_back_and_starts_the_service() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let never = || async { panic!("subiu sem nada pendente") };
        assert_eq!(recover_with(&paths, never).await, Ok(false));
        let (name, at) = (paths.data.join("messages.db"), paths.data.join("messages.db.migrated"));
        std::fs::create_dir_all(&paths.data).unwrap();
        write_db_file(&at, "migrated");
        write_pending(&paths, &[Stuck { name: name.clone(), at }]).unwrap();
        let refused = || async { Err(Failure::new("whatsapp_not_started", "não respondeu")) };
        assert!(recover_with(&paths, refused).await.is_err());
        assert!(paths.restore_pending().exists(), "religar falhou e a pendência sumiu");
        let up = std::cell::Cell::new(false);
        assert_eq!(recover_with(&paths, || async { up.set(true); Ok(()) }).await, Ok(true));
        assert_eq!(read(&name), "migrated");
        assert!(!paths.restore_pending().exists(), "o marcador ficou");
        assert!(up.get(), "o serviço não subiu");
    }

    /// Sem marcador (ele não gravou), o banco ausente com o migrado ao lado é tratado como o par preso: volta ao lugar
    /// antes de subir, nunca um banco vazio por cima.
    #[tokio::test]
    async fn a_missing_database_next_to_the_migrated_one_is_put_back_without_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(&paths.data).unwrap();
        assert!(!recovery_pending(&paths));
        write_db_file(&paths.data.join("messages.db.migrated"), "migrated");
        write_db_file(&paths.data.join("messages.db.migrated-wal"), "wal");
        assert!(recovery_pending(&paths));
        let up = std::cell::Cell::new(false);
        assert_eq!(recover_with(&paths, || async { up.set(true); Ok(()) }).await, Ok(true));
        assert_eq!((read(&paths.data.join("messages.db")), read(&paths.data.join("messages.db-wal"))), ("migrated".into(), "wal".into()));
        assert!(up.get() && !recovery_pending(&paths));
    }

    /// Sem nenhum dos dois arquivos (quarentena, apagado à mão) a pendência fica: subir criaria um banco vazio.
    #[tokio::test]
    async fn a_pair_with_both_files_gone_stays_pending() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        write_pending(&paths, &[Stuck { name: paths.data.join("messages.db"), at: paths.data.join("messages.db.migrated") }]).unwrap();
        let got = recover_with(&paths, || async { panic!("subiu sem o banco") }).await;
        assert_eq!(got.map_err(|f| f.code), Err(RESTORE_PENDING));
        assert!(paths.restore_pending().exists());
    }

    /// A confirmação vale para a tag que o dono viu: com outra mais nova na hora, a troca volta a perguntar.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_confirmation_for_another_tag_asks_again() {
        let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        fake_program(&binary, "0.1.0", None, None);
        fake_program(&ctx.paths.service.join("bin/0.2.0/hangar-whatsapp"), "0.2.0", Some(2), Some(1));
        assert_eq!(update_expecting(&ctx, Mode::Confirmed, Some("v0.1.5"), &|_| {}).await, Ok(Outcome::NeedsConfirm));
        assert_eq!(current(&ctx.paths).map(|c| c.tag).as_deref(), Some("v0.1.0"));
    }

    /// Com um arquivo ainda fora do lugar (aqui o lugar dele está ocupado), o Atualizar do dono recusa com o código
    /// e a lista, sem procurar versão, e o marcador fica para a próxima tentativa.
    #[tokio::test]
    async fn updating_refuses_while_the_files_are_out_of_place() {
        let (_dir, ctx, _, _) = installed_latest(axum::Router::new()).await;
        let (name, at) = (ctx.paths.data.join("messages.db"), ctx.paths.data.join("messages.db.migrated"));
        std::fs::create_dir_all(&ctx.paths.data).unwrap();
        write_db_file(&name, "vazio");
        write_db_file(&at, "migrated");
        write_pending(&ctx.paths, &[Stuck { name, at }]).unwrap();
        let refused = update(&ctx, Mode::Manual, &|_| {}).await.unwrap_err();
        assert_eq!(refused.code, RESTORE_PENDING);
        assert!(refused.detail.contains("messages.db.migrated"), "{}", refused.detail);
        assert!(ctx.paths.restore_pending().exists());
    }

    /// A cópia desta tentativa leva à volta mesmo sem a prova de que a nova abriu os dados: o binário migra antes
    /// de gravar o `control.json`.
    #[test]
    fn a_backup_of_this_attempt_restores_even_without_proof_of_opening() {
        assert_eq!(after_failure(false, Some(1), false), AfterFailure::Restore(1));
        assert_eq!(after_failure(true, Some(1), false), AfterFailure::Restore(1));
        assert_eq!(after_failure(true, None, false), AfterFailure::KeepNew);
        assert_eq!(after_failure(false, None, false), AfterFailure::Rollback);
    }

    /// Troca sem mudança de formato, com o arquivo já existente: a anterior abre o arquivo, e volta mesmo que a nova
    /// o tenha aberto. Mudança de formato, esquema desconhecido ou arquivo que a nova criou (0) não entram aqui.
    #[test]
    fn a_failed_switch_that_keeps_the_format_goes_back_to_the_previous_version() {
        assert!(same_format(Some(2), Some(2)), "o esquema da nova é igual ao do arquivo");
        assert!(!same_format(Some(2), Some(1)), "muda o formato");
        assert!(!same_format(Some(2), None), "arquivo ilegível");
        assert!(!same_format(None, Some(2)), "a nova não diz o esquema");
        assert!(!same_format(Some(2), Some(0)), "a nova criou o arquivo no esquema dela");
        assert_eq!(after_failure(true, None, true), AfterFailure::Rollback, "a nova abriu, e mesmo assim a anterior volta");
        assert_eq!(after_failure(false, None, true), AfterFailure::Rollback);
        assert_eq!(after_failure(true, Some(1), true), AfterFailure::Restore(1), "cópia desta tentativa vale mais");
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
        assert_eq!(restore_source(Mode::Confirmed, data, None, since), Some(1), "o esquema do arquivo não se leu");
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
        write_update_check(&ctx.paths, &UpdateCheck { latest: "v0.1.0".into(), migration: true, checked_at: "x".into(), failed, last_error: None }).unwrap();
        assert_eq!(update(&ctx, Mode::Auto, &|_| {}).await, Ok(Outcome::AlreadyLatest));
        assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }), "o registro não mudou");
        let check = read_update_check(&ctx.paths).unwrap();
        assert_eq!((check.latest.as_str(), check.migration, check.failed), ("v0.1.0", false, None));
        assert!(check.checked_at.ends_with('Z'), "{}", check.checked_at);
    }

    /// O arquivo atrás do esquema da nova, a nova sem `--schema-version` ou o arquivo que não se lê: a troca pode
    /// subir o `user_version`, e sem confirmação nada é registrado e a procura fica gravada. O programa instalado
    /// (aqui sem nenhuma das flags) não é consultado.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_release_that_may_change_the_data_format_waits_for_confirmation() {
        let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        fake_program(&binary, "0.1.0", None, None);
        let downloaded = ctx.paths.service.join("bin/0.2.0/hangar-whatsapp");
        for (schema, data) in [(Some(2), Some(1)), (None, Some(1)), (None, Some(2)), (Some(2), None)] {
            for mode in [Mode::Auto, Mode::Manual] {
                let _ = std::fs::remove_file(ctx.paths.update_check());
                fake_program(&downloaded, "0.2.0", schema, data);
                assert_eq!(update(&ctx, mode, &|_| {}).await, Ok(Outcome::NeedsConfirm), "{mode:?} {schema:?} {data:?}");
                assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary: binary.clone() }), "{mode:?}");
                // O programa já estava na pasta antes da procura (o GitHub de mentira não serve download): não é desta chamada.
                assert!(downloaded.exists(), "{mode:?}: o programa que já estava na pasta saiu");
                assert!(binary.exists(), "{mode:?}: a versão instalada continua");
                let check = read_update_check(&ctx.paths).unwrap();
                // Sem um dos esquemas lidos, a confirmação pedida não prende a tag: a próxima passada pergunta de novo.
                let unread = (schema.is_none() || data.is_none()).then(|| CheckFailure { tag: "v0.2.0".into(), code: SCHEMA_UNREAD.into() });
                assert_eq!((check.latest.as_str(), check.migration, check.failed, check.last_error), ("v0.2.0", true, None, unread), "{mode:?}");
                assert_eq!(skip_known(&ctx.paths, "v0.2.0"), schema.is_some() && data.is_some(), "{mode:?}");
            }
        }
    }

    /// A falha antes de registrar a nova (aqui o download) fica em `last_error` e não prende a tag: o relógio tenta
    /// de novo na próxima passada. A tentativa que passa do download a apaga.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failure_before_registering_does_not_hold_the_tag() {
        let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        fake_program(&binary, "0.1.0", None, None);
        let download = Some(CheckFailure { tag: "v0.2.0".into(), code: release::CODES.download_failed.into() });
        // Sem o programa na pasta, o download vai ao GitHub de mentira, que não serve nada.
        for _ in 0..2 {
            let got = update(&ctx, Mode::Auto, &|_| {}).await;
            assert_eq!(got.map_err(|f| f.code), Err(release::CODES.download_failed), "a segunda passada tenta de novo");
            let check = read_update_check(&ctx.paths).unwrap();
            assert_eq!((check.latest.as_str(), check.migration, &check.failed, &check.last_error), ("v0.2.0", true, &None, &download),
                "sem resposta conta como mudança, mas não prende a tag");
        }
        assert!(matches!(auto_wants(&ctx).await, AutoPlan::Switch { .. }));
        fake_program(&ctx.paths.service.join("bin/0.2.0/hangar-whatsapp"), "0.2.0", Some(2), Some(1));
        assert_eq!(update(&ctx, Mode::Manual, &|_| {}).await, Ok(Outcome::NeedsConfirm));
        let check = read_update_check(&ctx.paths).unwrap();
        assert_eq!((check.migration, check.failed, check.last_error), (true, None, None));
    }

    /// Só a falha depois de registrar a nova prende a tag (`failed`), e a espera por confirmação não a apaga; a de
    /// antes descreve só a última tentativa. Dar certo, ou outra tag, limpa as duas.
    #[test]
    fn only_a_failure_after_registering_holds_the_tag() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let failure = |tag: &str, code: &str| Some(CheckFailure { tag: tag.into(), code: code.into() });
        let (before, after) = (Attempt { migration: None, registered: false, schema_unread: false }, Attempt { migration: Some(false), registered: true, schema_unread: false });
        let read = || { let c = read_update_check(&paths).unwrap(); (c.migration, c.failed, c.last_error) };
        record_check(&paths, "v0.2.0", &after, &Err(Failure::new("whatsapp_not_started", "x")));
        assert_eq!(read(), (false, failure("v0.2.0", "whatsapp_not_started"), None));
        record_check(&paths, "v0.2.0", &before, &Err(Failure::new("whatsapp_download_failed", "x")));
        assert_eq!(read(), (false, failure("v0.2.0", "whatsapp_not_started"), failure("v0.2.0", "whatsapp_download_failed")),
            "a migração lida antes fica, e a tag segue presa");
        record_check(&paths, "v0.2.0", &Attempt { migration: Some(true), registered: false, schema_unread: false }, &Ok(Outcome::NeedsConfirm));
        assert_eq!(read(), (true, failure("v0.2.0", "whatsapp_not_started"), None));
        record_check(&paths, "v0.2.0", &after, &Ok(Outcome::Updated));
        assert_eq!(read(), (false, None, None));
        record_check(&paths, "v0.2.0", &after, &Err(Failure::new("whatsapp_not_started", "x")));
        record_check(&paths, "v0.3.0", &before, &Err(Failure::new("whatsapp_download_failed", "x")));
        assert_eq!(read(), (true, None, failure("v0.3.0", "whatsapp_download_failed")), "outra tag não herda nada");
        let old = r#"{"latest":"v0.2.0","migration":true,"checked_at":"x"}"#;
        std::fs::write(paths.update_check(), old).unwrap();
        assert_eq!(read(), (true, None, None), "arquivo de antes de last_error");
    }

    /// Só o pareamento em curso segura a troca do relógio; pareado ou sem pareamento, ela segue.
    #[test]
    fn only_a_pairing_in_progress_holds_the_clock() {
        assert!(pairing(&json!({"state": "pairing"})));
        assert!(!pairing(&json!({"state": "connected"})));
        assert!(!pairing(&json!({"state": "unpaired"})));
    }

    /// Só um rascunho listado como `sending` segura a troca; binário sem o estado ou resposta ilegível não.
    #[test]
    fn only_a_draft_still_sending_holds_the_switch() {
        let reply = |status, body: &str| crate::client::Reply { status, body: body.as_bytes().to_vec() };
        assert!(still_sending(&reply(200, r#"{"drafts":[{"id":"a"}]}"#)));
        assert!(!still_sending(&reply(200, r#"{"drafts":[]}"#)));
        assert!(!still_sending(&reply(400, r#"{"drafts":[{"id":"a"}]}"#)), "binário que não conhece o estado");
        assert!(!still_sending(&reply(200, "x")));
    }

    /// A passada decide fora da vez: sem instalação, sem release nova (a procura gravada limpa a falha da tag),
    /// GitHub fora e release nova a tentar.
    #[tokio::test]
    async fn the_clock_decides_without_the_turn() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(auto_wants(&Context::new(empty.path())).await, AutoPlan::NotInstalled);

        let (_dir, ctx, _, _) = installed_latest(axum::Router::new()).await;
        let failed = Some(CheckFailure { tag: "v0.1.0".into(), code: "whatsapp_not_started".into() });
        write_update_check(&ctx.paths, &UpdateCheck { latest: "v0.1.0".into(), migration: true, checked_at: "x".into(), failed, last_error: None }).unwrap();
        assert_eq!(auto_wants(&ctx).await, AutoPlan::Done(AutoUpdate { from: "v0.1.0".into(), to: "v0.1.0".into(), result: Ok(Outcome::AlreadyLatest) }));
        let check = read_update_check(&ctx.paths).unwrap();
        assert_eq!((check.migration, check.failed, check.last_error), (false, None, None));

        let down = Context { github: Github { api: "http://127.0.0.1:9".into(), download: "http://127.0.0.1:9/nada".into() }, ..ctx.clone() };
        let AutoPlan::Done(AutoUpdate { from, to, result }) = auto_wants(&down).await else { panic!("GitHub fora não é troca") };
        assert_eq!((from.as_str(), to.as_str(), result.map_err(|f| f.code)), ("v0.1.0", "v0.1.0", Err(release::CODES.download_failed)));

        let (_dir, ctx, _, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
        assert_eq!(auto_wants(&ctx).await, AutoPlan::Switch { from: "v0.1.0".into(), latest: Release { tag: "v0.2.0".into() } });
        assert_eq!(read_update_check(&ctx.paths), None, "decidir a troca não grava procura: a tentativa grava");
    }

    /// O relógio não baixa de novo a tag que já parou na confirmação ou falhou depois de registrar; a tela, sim. A
    /// procura gravada só ganha a hora nova. A migração que a última tentativa não leu (`last_error`) não prende.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_clock_skips_a_tag_it_already_knows_will_stop() {
        let failed = Some(CheckFailure { tag: "v0.2.0".into(), code: "whatsapp_not_started".into() });
        let download = Some(CheckFailure { tag: "v0.2.0".into(), code: release::CODES.download_failed.into() });
        for (migration, failed, last_error, skipped) in [(true, None, None, true), (false, failed, None, true), (true, None, download, false)] {
            let (_dir, ctx, binary, _) = installed_with(&["v0.2.0", "v0.1.0"], axum::Router::new()).await;
            fake_program(&binary, "0.1.0", None, None);
            let known = UpdateCheck { latest: "v0.2.0".into(), migration, checked_at: "x".into(), failed: failed.clone(), last_error: last_error.clone() };
            write_update_check(&ctx.paths, &known).unwrap();
            let downloaded = ctx.paths.service.join("bin/0.2.0/hangar-whatsapp");
            let plan = auto_wants(&ctx).await;
            if !skipped {
                assert!(matches!(plan, AutoPlan::Switch { .. }), "{plan:?}");
                continue;
            }
            assert_eq!(plan, AutoPlan::Done(AutoUpdate { from: "v0.1.0".into(), to: "v0.2.0".into(), result: Ok(Outcome::Skipped) }), "{migration} {failed:?}");
            assert_eq!(update(&ctx, Mode::Auto, &|_| {}).await, Ok(Outcome::Skipped), "na vez, a mesma regra");
            assert!(!downloaded.parent().unwrap().exists(), "o relógio baixou a tag que ia parar");
            let check = read_update_check(&ctx.paths).unwrap();
            assert_eq!((check.latest.as_str(), check.migration, &check.failed), ("v0.2.0", migration, &failed));
            assert_ne!(check.checked_at, "x", "a hora da procura não andou");
            fake_program(&downloaded, "0.2.0", Some(2), Some(1));
            assert_eq!(update(&ctx, Mode::Manual, &|_| {}).await, Ok(Outcome::NeedsConfirm), "{migration} {failed:?}");
            assert_eq!(current(&ctx.paths), Some(Installed { tag: "v0.1.0".into(), binary }));
        }
    }

    /// O arquivo já no esquema da nova, ou sem arquivo, não muda o formato, pela resposta do programa
    /// novo; o atrás muda. A troca em si não roda aqui (mexeria no gerenciador de serviços da máquina).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_same_schema_does_not_stop_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let (new, data) = (dir.path().join("new/hangar-whatsapp"), dir.path().join("dados"));
        for (file, changes) in [(2, false), (0, false), (1, true)] {
            fake_program(&new, "0.3.1", Some(2), Some(file));
            assert_eq!(format_changes_of(&new, &data).await, changes, "arquivo {file}");
        }
    }
}
