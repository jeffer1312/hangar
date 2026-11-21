//! Instalar e atualizar o proxy, uma tarefa por vez, com o andamento consultável pela tela.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config;
use crate::detect::{self, Install, Paths};
use crate::release::{self, Github, Platform, Release};
use crate::service::{self, Service};

const DEFAULT_BASE: &str = "http://127.0.0.1:8317";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const LATEST_TTL: Duration = Duration::from_secs(3600);
const LATEST_FAILURE_TTL: Duration = Duration::from_secs(300);
/// Espera a releitura do config pelo proxy: perguntar antes gasta uma tentativa de senha com o config velho.
const RELOAD_WAIT: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Download,
    Config,
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
    /// Proxy de fora do Hangar: só o config foi completado.
    Configured,
    Updated { tag: String },
    /// Proxy de fora entregue ao Hangar: mesmo config e mesmas contas, rodando pelo serviço dele.
    Adopted { tag: String },
}

/// Quem mantém ligado o proxy de fora: é o que a entrega desliga e, se a versão do Hangar não responder, religa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Runner {
    /// Nada o liga e a porta está livre: não há o que desligar.
    Idle,
    /// Unit de usuário, com o estado dela antes da entrega para a volta repor igual.
    SystemdUser { unit: String, enabled: bool, active: bool },
    LaunchAgent(std::path::PathBuf),
}

/// A senha de gerenciamento nova vai para o Hangar antes do config.
pub type Persist = Arc<dyn Fn(&str) -> std::io::Result<()> + Send + Sync>;
/// A senha de gerenciamento que o Hangar guarda, se houver.
pub type ManagementKey = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Conferência feita logo antes de trocar o programa: o download leva tempo, e uma sessão pode ter aberto no proxy.
pub type BeforeSwap = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Failure>> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    pub platform: Platform,
    pub github: Github,
    pub persist: Persist,
    pub management_key: ManagementKey,
    pub before_swap: Option<BeforeSwap>,
}

#[derive(Clone, Debug)]
struct Job {
    kind: &'static str,
    step: Step,
    result: Option<Result<Outcome, Failure>>,
}

/// Uma instalação ou atualização por vez; o andamento fica em memória para o `GET` da tela.
#[derive(Default)]
pub struct Installer {
    job: Mutex<Option<Job>>,
    latest: tokio::sync::Mutex<Option<(Instant, Result<Release, String>)>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

/// O que a rota de gerenciamento respondeu: a versão vem no cabeçalho mesmo quando a senha é recusada.
struct Probe {
    version: Option<semver::Version>,
    rejected: bool,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Config => "config",
            Self::Service => "service",
            Self::Start => "start",
        }
    }
}

impl Failure {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self { code, detail: detail.into() }
    }
}

impl From<release::Error> for Failure {
    fn from(e: release::Error) -> Self {
        let detail = format!("{e:?}");
        Self::new(e.code(), detail)
    }
}

impl From<service::Error> for Failure {
    fn from(e: service::Error) -> Self {
        Self::new(e.code(), format!("{}: {}", e.step, e.detail))
    }
}

impl From<config::EnsureError> for Failure {
    fn from(e: config::EnsureError) -> Self {
        match e {
            config::EnsureError::Io(e) => Self::new("cliproxy_config_ilegivel", e.to_string()),
            config::EnsureError::InvalidYaml => Self::new("cliproxy_config_yaml_invalido", "config.yaml com YAML inválido"),
            config::EnsureError::Unsupported => Self::new("cliproxy_config_manual", "config.yaml numa forma que o Hangar não completa sozinho"),
        }
    }
}

fn now_unix() -> u64 {
    hangar_core::clock::wall_now() as u64
}

fn ensure(ctx: &Context, ultrafast_rule: bool) -> Result<config::Ensured, Failure> {
    let persist = ctx.persist.clone();
    Ok(config::ensure(&ctx.paths.home, now_unix(), move |key| persist(key), ultrafast_rule)?)
}

/// A versão que o proxy em execução anuncia, que só sai na rota de gerenciamento. Ali pedido sem a senha certa conta
/// como tentativa errada, e as erradas seguidas bloqueiam o 127.0.0.1, login incluído: só pergunta com a senha guardada.
async fn running_version(base: &str, key: Option<&str>) -> Option<Probe> {
    let key = key?;
    if crate::login::rejected(key) {
        return Some(Probe { version: None, rejected: true });
    }
    let client = hangar_core::net::http_client().timeout(Duration::from_secs(5)).build().ok()?;
    let response = client.get(format!("{base}/v0/management/config")).bearer_auth(key).send().await.ok()?;
    let version = response.headers().get("x-cpa-version").and_then(|v| v.to_str().ok())
        .and_then(|v| semver::Version::parse(v.trim().trim_start_matches('v')).ok());
    let rejected = matches!(response.status().as_u16(), 401 | 403);
    if rejected {
        crate::login::note_rejected(key);
    } else if response.status().is_success() {
        crate::login::note_accepted(key);
    }
    Some(Probe { version, rejected })
}

/// Só proxy que filtra regras de `payload` por cabeçalho entende a regra (versão em docs/decisoes); sem versão
/// conhecida, ela fica de fora.
fn understands_rule(version: Option<&semver::Version>) -> bool { version.is_some_and(|v| v.major >= 8) }

/// Senha de gerenciamento que o Hangar não conhece: troca por uma nova, guardada por `persist`.
pub fn reset_management(ctx: &Context) -> Result<(), Failure> {
    let persist = ctx.persist.clone();
    config::reset_management(&ctx.paths.home, now_unix(), move |key| persist(key))?;
    Ok(())
}

/// O endereço do config; a porta padrão só quando não há config. Porta inválida para antes de mexer em arquivo
/// ou serviço: o proxy subiria noutra porta que a tela e o login não acham. YAML ilegível fica para o `ensure`.
fn base_url(ctx: &Context) -> Result<String, Failure> {
    base_url_at(&ctx.paths.home)
}

fn base_url_at(home: &std::path::Path) -> Result<String, Failure> {
    match config::read(home) {
        Ok(Some(config)) => config.base_url.map_err(|_| Failure::new("cliproxy_porta_invalida",
            format!("{}: server.port (ou port) fora de 1–65535", config::path(home).display()))),
        Ok(None) => Ok(DEFAULT_BASE.into()),
        // Config que existe e não se lê não vira a porta padrão: a instalação olharia a porta errada.
        Err(config::ReadError::Io(e)) => Err(Failure::new("cliproxy_config_ilegivel", e.to_string())),
        Err(config::ReadError::InvalidYaml) => Err(Failure::new("cliproxy_config_yaml_invalido", "config.yaml com YAML inválido")),
    }
}

fn tag_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim_start_matches('v')).ok()
}

async fn port_open(base: &str) -> bool {
    let base = base.to_owned();
    tokio::task::spawn_blocking(move || detect::port_open(&base)).await.unwrap_or(false)
}

/// Recusas que a espera aceita antes de parar de perguntar a versão: abaixo do limite que bloqueia o 127.0.0.1.
const MAX_REJECTED: u32 = 2;

/// Espera o proxy desta versão responder. Outro programa na porta também aceita conexão: com a versão e a senha
/// conhecidas, só a versão anunciada pelo proxy confirma; sem elas (tag fora de semver, senha recusada), basta a porta abrir.
async fn wait_ours(base: &str, expected: Option<&semver::Version>, key: Option<&str>) -> Result<(), Failure> {
    let deadline = Instant::now() + START_TIMEOUT;
    let mut rejected = 0;
    while Instant::now() < deadline {
        let up = match expected {
            Some(expected) if key.is_some() && rejected < MAX_REJECTED => match running_version(base, key).await {
                Some(probe) => {
                    rejected += u32::from(probe.rejected);
                    probe.version.as_ref() == Some(expected)
                }
                None => false,
            },
            _ => port_open(base).await,
        };
        if up {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let version = expected.map(|v| format!(" na versão {v}")).unwrap_or_default();
    Err(Failure::new("cliproxy_nao_subiu", format!("o proxy não respondeu em {base}{version} depois de {}s", START_TIMEOUT.as_secs())))
}

/// O config que a unit externa passa ao proxy, com `%h`/`$HOME` resolvidos; outro que não o lido pelo Hangar
/// deixa a configuração manual em vez de completar um arquivo que o proxy nem lê. `units` são todos os lugares
/// da unit, para achar o drop-in que troca o comando.
fn external_config(unit: Option<&std::path::Path>, units: &[std::path::PathBuf], home: &std::path::Path)
    -> Result<Option<std::path::PathBuf>, Failure> {
    let Some(unit) = unit else { return Ok(None) };
    let manual = |detail: String| Failure::new("cliproxy_config_manual", detail);
    // Unit que não se lê: não dá para saber qual config o proxy usa, e completar o nosso seria às cegas.
    let text = std::fs::read_to_string(unit).map_err(|e| manual(format!("não consegui ler {}: {e}", unit.display())))?;
    if detect::exec_overridden(units) {
        return Err(manual(format!("um drop-in troca o comando de {}", unit.display())));
    }
    // Sem `--config` o proxy lê o `config.yaml` da pasta em que roda: a do `WorkingDirectory=`, ou a home do
    // usuário numa unit de usuário. Numa unit de sistema sem pasta declarada, não dá para saber.
    let raw = match detect::unit_config(&text) {
        Some(raw) => raw.to_string_lossy().into_owned(),
        None => match detect::unit_working_dir(&text) {
            Some(dir) => format!("{}/config.yaml", dir.strip_prefix('~').map_or(dir.clone(), |rest| format!("%h{rest}"))),
            None if detect::is_system_unit(unit) =>
                return Err(manual(format!("{} não diz em que pasta o proxy roda nem qual config ele lê", unit.display()))),
            None => "%h/config.yaml".to_owned(),
        },
    };
    if detect::is_system_unit(unit) && (raw.contains("%h") || raw.contains("$HOME") || raw.contains("${HOME}")) {
        return Err(manual(format!("{} é de sistema: a pasta do {raw} depende do usuário que a roda", unit.display())));
    }
    let home = home.to_string_lossy();
    let expanded = raw.replace("%h", &home).replace("${HOME}", &home).replace("$HOME", &home);
    Ok(Some(std::path::PathBuf::from(expanded)))
}

/// Mesmo arquivo, mesmo por atalho: compara os caminhos resolvidos quando os dois existem.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn current(paths: &Paths) -> Install {
    detect::state(paths, std::env::var_os("PATH").as_deref(), &detect::system_units(&paths.home))
}

/// A versão que o proxy em execução anuncia, como tag; só com a senha guardada (ver `running_version`).
pub async fn running_tag(paths: &Paths, key: Option<&str>) -> Option<String> {
    let base = base_url_at(&paths.home).ok()?;
    running_version(&base, key).await?.version.map(|v| format!("v{v}"))
}

async fn systemctl_says(verb: &str, unit: &str) -> String {
    let output = tokio::process::Command::new("systemctl").args(["--user", verb, unit]).stdin(std::process::Stdio::null()).output().await;
    output.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
}

/// Pasta das contas relativa no config: o proxy a resolve contra a pasta em que roda. O serviço do Hangar roda na
/// pessoal; o de fora, rodando noutra, gravou as contas num lugar que o do Hangar não acharia.
fn accounts_dir_moves(unit_text: &str, home: &std::path::Path) -> Option<String> {
    let config = config::read(home).ok().flatten()?;
    let dir = config.auth_dir.as_deref().map(str::trim).filter(|d| !d.is_empty())?;
    if std::path::Path::new(dir).is_absolute() || dir.starts_with('~') {
        return None;
    }
    let raw = detect::unit_working_dir(unit_text)?;
    let home_text = home.to_string_lossy();
    let expanded = raw.replace("%h", &home_text).replace("${HOME}", &home_text).replace("$HOME", &home_text);
    let expanded = expanded.strip_prefix('~').map_or(expanded.clone(), |rest| format!("{home_text}{rest}"));
    (!same_file(std::path::Path::new(&expanded), home)).then_some(expanded)
}

/// O proxy de fora pode passar para o Hangar? Só o que lê o mesmo config, com as contas na mesma pasta, e que um
/// serviço conhecido liga (ou que está parado): o Hangar desliga esse serviço, nunca o desinstala.
pub async fn handover(paths: &Paths) -> Result<Runner, Failure> {
    let Install::External { unit, .. } = current(paths) else {
        return Err(Failure::new("erro_cliproxy_nao_externo", "só o proxy de fora do Hangar é entregue a ele"));
    };
    let base = base_url_at(&paths.home)?;
    let running = port_open(&base).await;
    let ours = paths.config();
    let other_config = |other: &std::path::Path| Failure::new("cliproxy_entrega_outro_config",
        format!("o proxy de fora lê {}, não {}", other.display(), ours.display()));
    if let Some(unit) = unit.as_deref() {
        if detect::is_system_unit(unit) {
            return Err(Failure::new("cliproxy_entrega_unit_de_sistema", format!("{} é de sistema: desligá-la pede administrador", unit.display())));
        }
        let units = detect::system_units(&paths.home);
        let read = external_config(Some(unit), &units, &paths.home)
            .map_err(|f| Failure::new("cliproxy_entrega_outro_config", f.detail))?;
        if let Some(other) = read.filter(|c| !same_file(c, &ours)) {
            return Err(other_config(other.as_path()));
        }
        let text = std::fs::read_to_string(unit).unwrap_or_default();
        if let Some(dir) = accounts_dir_moves(&text, &paths.home) {
            return Err(Failure::new("cliproxy_entrega_pasta_das_contas",
                format!("o auth-dir é relativo e o serviço de fora roda em {dir}")));
        }
        let name = unit.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let active = systemctl_says("is-active", &name).await == "active";
        if running && !active {
            return Err(Failure::new("cliproxy_entrega_sem_servico", format!("{base} responde, mas {name} está parado")));
        }
        let enabled = systemctl_says("is-enabled", &name).await == "enabled";
        return Ok(Runner::SystemdUser { unit: name, enabled, active });
    }
    if cfg!(target_os = "macos") && let Some(plist) = detect::launch_agents(&paths.home).into_iter().next() {
        let text = std::fs::read_to_string(&plist).unwrap_or_default();
        return match detect::plist_config(&text) {
            Some(config) if same_file(&config, &ours) => Ok(Runner::LaunchAgent(plist)),
            Some(other) => Err(other_config(other.as_path())),
            None => Err(Failure::new("cliproxy_entrega_outro_config", format!("{} não diz qual config o proxy lê", plist.display()))),
        };
    }
    if running {
        return Err(Failure::new("cliproxy_entrega_sem_servico", format!("{base} responde, mas nenhum serviço conhecido liga esse proxy")));
    }
    Ok(Runner::Idle)
}

impl Runner {
    async fn stop(&self) -> Result<(), Failure> {
        match self {
            Self::Idle | Self::SystemdUser { enabled: false, active: false, .. } => Ok(()),
            Self::SystemdUser { unit, enabled: true, .. } => Ok(service::run("systemctl disable", "systemctl", &["--user", "disable", "--now", unit]).await?),
            Self::SystemdUser { unit, .. } => Ok(service::run("systemctl stop", "systemctl", &["--user", "stop", unit]).await?),
            Self::LaunchAgent(plist) => Ok(service::run("launchctl unload", "launchctl", &["unload", "-w", &plist.to_string_lossy()]).await?),
        }
    }

    /// Repõe o serviço de fora como estava antes da entrega.
    async fn restore(&self) -> Result<(), Failure> {
        match self {
            Self::Idle => Ok(()),
            Self::SystemdUser { unit, enabled, active } => {
                if *enabled {
                    service::run("systemctl enable", "systemctl", &["--user", "enable", unit]).await?;
                }
                if *active {
                    service::run("systemctl start", "systemctl", &["--user", "start", unit]).await?;
                }
                Ok(())
            }
            Self::LaunchAgent(plist) => Ok(service::run("launchctl load", "launchctl", &["load", "-w", &plist.to_string_lossy()]).await?),
        }
    }

    fn was_running(&self) -> bool {
        matches!(self, Self::SystemdUser { active: true, .. } | Self::LaunchAgent(_))
    }
}

pub async fn install(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let base = base_url(ctx)?;
    let (tag, binary) = match current(&ctx.paths) {
        Install::External { unit, .. } => {
            progress(Step::Config);
            let units = detect::system_units(&ctx.paths.home);
            if let Some(other) = external_config(unit.as_deref(), &units, &ctx.paths.home)?.filter(|c| !same_file(c, &ctx.paths.config())) {
                return Err(Failure::new("cliproxy_config_manual",
                    format!("o serviço do proxy lê {}, não {}", other.display(), ctx.paths.config().display())));
            }
            // A versão só se pergunta com a senha guardada; sem ela, a regra do Ultra Fast fica de fora por ora.
            let version = running_version(&base, (ctx.management_key)().as_deref()).await.and_then(|probe| probe.version);
            let ensured = ensure(ctx, understands_rule(version.as_ref()))?;
            // Primeira configuração: a senha acabou de nascer no `ensure`. Depois da releitura do config pelo
            // proxy, uma pergunta com ela decide a regra; ela só entra quando o config ainda não tem `payload`.
            if version.is_none() && ensured.management_key.is_some() {
                tokio::time::sleep(RELOAD_WAIT).await;
                let version = running_version(&base, (ctx.management_key)().as_deref()).await.and_then(|probe| probe.version);
                if understands_rule(version.as_ref()) {
                    ensure(ctx, true)?;
                }
            }
            return Ok(Outcome::Configured);
        }
        // Proxy do Hangar já respondendo: só completa o config, que ele relê sozinho. Reiniciar o serviço aqui cortaria
        // as sessões no proxy sem a confirmação que a atualização pede; com ele parado, registra e sobe de novo.
        Install::Managed { tag, .. } if port_open(&base).await => {
            progress(Step::Config);
            ensure(ctx, understands_rule(tag_version(&tag).as_ref()))?;
            return Ok(Outcome::Installed { tag });
        }
        Install::Managed { tag, binary } => (tag, binary),
        Install::Absent => {
            // Algo já atende na porta e não é instalação conhecida: subir outro proxy ali falharia calado.
            if port_open(&base).await {
                return Err(Failure::new("cliproxy_porta_ocupada", format!("{base} já responde e não é um proxy instalado nesta máquina")));
            }
            progress(Step::Download);
            let latest = release::latest(&ctx.github).await?;
            let binary = release::fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.dir).await?;
            (latest.tag, binary)
        }
    };
    progress(Step::Config);
    // O programa é o da release que o Hangar baixou: a tag diz se a regra é entendida.
    let version = tag_version(&tag);
    ensure(ctx, understands_rule(version.as_ref()))?;
    progress(Step::Service);
    // O registro vem antes do serviço: se o serviço ou a subida falharem, a próxima tentativa já vê a instalação
    // como do Hangar e registra de novo, em vez de achar a porta ocupada por um proxy sem dono.
    detect::write_record(&ctx.paths, &tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: binary.clone(), config: ctx.paths.config() }).await?;
    progress(Step::Start);
    // O `ensure` pode ter acabado de gravar o config: a porta e a senha relidas são as que o proxy vai usar.
    wait_ours(&base_url(ctx)?, version.as_ref(), (ctx.management_key)().as_deref()).await?;
    Ok(Outcome::Installed { tag })
}

/// A falha da versão nova, ou, se nem a anterior voltou, um código próprio: a tela só diz "segue funcionando"
/// quando a volta deu certo.
fn rolled_back(failure: Failure, installed: &str, back: Result<(), Failure>) -> Failure {
    match back {
        Ok(()) => failure,
        Err(e) => Failure::new("cliproxy_atualizacao_sem_volta",
            format!("{}; e o serviço não voltou para {installed}: {}", failure.detail, e.detail)),
    }
}

pub fn newer(candidate: &str, installed: &str) -> bool {
    let parse = |t: &str| semver::Version::parse(t.trim_start_matches('v')).ok();
    matches!((parse(candidate), parse(installed)), (Some(c), Some(i)) if c > i)
}

pub async fn update(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let Install::Managed { tag: installed, binary: old } = current(&ctx.paths) else {
        return Err(Failure::new("erro_cliproxy_externo", "só o proxy instalado pelo Hangar é atualizado por aqui"));
    };
    progress(Step::Download);
    let latest = release::latest(&ctx.github).await?;
    if !newer(&latest.tag, &installed) {
        return Ok(Outcome::Updated { tag: installed });
    }
    let base = base_url(ctx)?;
    let binary = release::fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.dir).await?;
    if let Some(check) = &ctx.before_swap {
        check().await?;
    }
    let version = tag_version(&latest.tag);
    // A regra do Ultra Fast só entra depois de a versão nova responder: a antiga, se voltar, aplicaria o tier em
    // todo pedido por não entender o filtro de cabeçalho.
    ensure(ctx, false)?;
    progress(Step::Service);
    let config = ctx.paths.config();
    service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: binary.clone(), config: config.clone() }).await?;
    progress(Step::Start);
    // O registro só passa para a versão nova depois de ela responder: senão a próxima tentativa veria as tags
    // iguais e diria "atualizado" sem nada rodando. Falhou, o serviço volta para o programa anterior.
    if let Err(failure) = wait_ours(&base, version.as_ref(), (ctx.management_key)().as_deref()).await {
        let back = service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: old.clone(), config }).await;
        return Err(rolled_back(failure, &installed, back.map_err(Failure::from)));
    }
    detect::write_record(&ctx.paths, &latest.tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    // Daqui em diante a versão nova já roda e está registrada: o que falta é arremate, e falha nele não desfaz a troca.
    // `payload` da pessoa fica como está: a regra só entra num config sem ele.
    if understands_rule(version.as_ref()) && let Err(failure) = ensure(ctx, true) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "cliproxy: atualizado, mas a regra do Ultra Fast não entrou no config");
    }
    if let Some(dir) = old.parent().filter(|d| Some(*d) != binary.parent())
        && let Err(error) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %error, "cliproxy: atualizado, mas a pasta da versão anterior ficou");
    }
    Ok(Outcome::Updated { tag: latest.tag })
}

async fn wait_port(base: &str) -> bool {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if port_open(base).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Tira o serviço do Hangar e religa o de fora; só vale como volta se o de fora, que respondia, voltou a responder.
async fn give_back(paths: &Paths, runner: &Runner, base: &str) -> Result<(), Failure> {
    service::unregister(&paths.home, &paths.dir).await?;
    runner.restore().await?;
    if runner.was_running() && !wait_port(base).await {
        return Err(Failure::new("cliproxy_nao_subiu", format!("o serviço de fora não voltou a responder em {base}")));
    }
    Ok(())
}

/// A falha da entrega, ou, se nem o serviço de fora voltou, um código próprio: só a volta certa diz "segue funcionando".
fn handed_back(failure: Failure, back: Result<(), Failure>) -> Failure {
    match back {
        Ok(()) => failure,
        Err(e) => Failure::new("cliproxy_entrega_sem_volta", format!("{}; e o serviço de fora não voltou: {}", failure.detail, e.detail)),
    }
}

/// Passa o proxy de fora para o serviço do Hangar, com o mesmo config e a mesma pasta de contas: nada é copiado nem
/// desinstalado. O registro só nasce depois de a versão do Hangar responder; antes disso, a falha religa o de fora.
pub async fn adopt(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    // Antes do download, para a recusa chegar logo.
    handover(&ctx.paths).await?;
    progress(Step::Download);
    let latest = release::latest(&ctx.github).await?;
    let binary = release::fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.dir).await?;
    let discard = |failure: Failure| {
        if let Some(dir) = binary.parent() && let Err(error) = std::fs::remove_dir_all(dir) {
            tracing::warn!(dir = %dir.display(), %error, "cliproxy: entrega desfeita, mas a pasta baixada ficou");
        }
        failure
    };
    if let Some(check) = &ctx.before_swap {
        check().await.map_err(discard)?;
    }
    progress(Step::Config);
    // O download leva tempo: o serviço de fora é lido de novo, e é esse estado que a volta repõe.
    let runner = handover(&ctx.paths).await.map_err(discard)?;
    let version = tag_version(&latest.tag);
    // Como na atualização, a regra do Ultra Fast só entra com a versão nova respondendo.
    ensure(ctx, false).map_err(discard)?;
    let base = base_url(ctx).map_err(discard)?;
    progress(Step::Service);
    if let Err(failure) = runner.stop().await {
        // Parar pode ter desligado sem parar: repõe o que havia.
        let back = runner.restore().await;
        return Err(discard(handed_back(failure, back)));
    }
    let service = Service { binary: binary.clone(), config: ctx.paths.config() };
    let started = service::register(&ctx.paths.home, &ctx.paths.dir, &service).await.map_err(Failure::from);
    progress(Step::Start);
    let up = match started {
        Ok(()) => wait_ours(&base, version.as_ref(), (ctx.management_key)().as_deref()).await,
        Err(failure) => Err(failure),
    };
    if let Err(failure) = up {
        let back = give_back(&ctx.paths, &runner, &base).await;
        return Err(discard(handed_back(failure, back)));
    }
    detect::write_record(&ctx.paths, &latest.tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    if understands_rule(version.as_ref()) && let Err(failure) = ensure(ctx, true) {
        tracing::warn!(code = failure.code, detail = %failure.detail, "cliproxy: entregue, mas a regra do Ultra Fast não entrou no config");
    }
    Ok(Outcome::Adopted { tag: latest.tag })
}

impl Installer {
    pub fn start(self: &Arc<Self>, kind: &'static str, ctx: Context) -> Result<(), Busy> {
        {
            let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
            if job.as_ref().is_some_and(|j| j.result.is_none()) {
                return Err(Busy);
            }
            *job = Some(Job { kind, step: if kind == "install" { Step::Config } else { Step::Download }, result: None });
        }
        let this = self.clone();
        tokio::spawn(async move {
            let progress_owner = this.clone();
            let work = tokio::spawn(async move {
                let progress = move |step: Step| {
                    if let Some(job) = progress_owner.job.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        job.step = step;
                    }
                };
                match kind {
                    "update" => update(&ctx, &progress).await,
                    "adopt" => adopt(&ctx, &progress).await,
                    _ => install(&ctx, &progress).await,
                }
            });
            // Tarefa que cai no meio também termina: sem resultado, toda tentativa seguinte veria "ocupado" até o servidor reiniciar.
            let result = work.await.unwrap_or_else(|error| {
                tracing::error!(kind, %error, "cliproxy: a tarefa de instalação parou no meio");
                Err(Failure::new("cliproxy_tarefa_caiu", format!("a tarefa parou no meio: {error}")))
            });
            if let Some(job) = this.job.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                job.result = Some(result);
            }
            *this.latest.lock().await = None;
        });
        Ok(())
    }

    pub fn job(&self) -> Value {
        let job = self.job.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = job.as_ref() else { return Value::Null };
        let (done, error, outcome) = match &job.result {
            None => (false, Value::Null, Value::Null),
            Some(Ok(outcome)) => (true, Value::Null, json!(match outcome {
                Outcome::Installed { .. } => "installed",
                Outcome::Configured => "configured",
                Outcome::Updated { .. } => "updated",
                Outcome::Adopted { .. } => "adopted",
            })),
            Some(Err(f)) => (true, json!({ "code": f.code, "detail": f.detail }), Value::Null),
        };
        json!({ "kind": job.kind, "step": job.step.name(), "done": done, "error": error, "outcome": outcome })
    }

    /// A release mais nova, relida no máximo uma vez por hora; falha fica guardada só 5 min, para a rede que voltou não esperar a hora.
    pub async fn latest(&self, github: &Github) -> Result<Release, String> {
        let mut cached = self.latest.lock().await;
        let fresh = |(at, value): &(Instant, Result<Release, String>)| {
            at.elapsed() < if value.is_ok() { LATEST_TTL } else { LATEST_FAILURE_TTL }
        };
        if let Some((_, value)) = cached.as_ref().filter(|entry| fresh(entry)) {
            return value.clone();
        }
        let value = release::latest(github).await.map_err(|e| e.code().to_owned());
        *cached = Some((Instant::now(), value.clone()));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ultrafast_rule_needs_a_proxy_that_filters_by_header() {
        let v = |s: &str| semver::Version::parse(s).ok();
        assert!(understands_rule(v("8.0.56").as_ref()));
        assert!(!understands_rule(v("7.9.12").as_ref()));
        assert!(!understands_rule(None), "versão desconhecida não leva a regra");
    }

    #[test]
    fn an_update_that_could_not_go_back_has_its_own_code() {
        let failure = || Failure::new("cliproxy_nao_subiu", "a v8.1.0 não respondeu");
        assert_eq!(rolled_back(failure(), "v8.0.79", Ok(())).code, "cliproxy_nao_subiu", "voltou: segue funcionando");
        let lost = rolled_back(failure(), "v8.0.79", Err(Failure::new("cliproxy_servico_falhou", "systemctl")));
        assert_eq!(lost.code, "cliproxy_atualizacao_sem_volta");
        assert!(lost.detail.contains("v8.0.79") && lost.detail.contains("systemctl"));
    }

    #[test]
    fn a_handover_that_could_not_go_back_has_its_own_code() {
        let failure = || Failure::new("cliproxy_nao_subiu", "a v8.0.79 não respondeu");
        assert_eq!(handed_back(failure(), Ok(())).code, "cliproxy_nao_subiu", "o de fora voltou: segue funcionando");
        let lost = handed_back(failure(), Err(Failure::new("cliproxy_servico_falhou", "systemctl start")));
        assert_eq!(lost.code, "cliproxy_entrega_sem_volta");
        assert!(lost.detail.contains("systemctl start"));
    }

    /// Unit de usuário do proxy de fora na pasta pessoal do teste (a primeira que o systemd escolhe) e o config numa porta livre.
    fn external_with_unit(home: &std::path::Path, unit: &str, config: &str) {
        let units = home.join(".config/systemd/user");
        std::fs::create_dir_all(&units).unwrap();
        std::fs::write(units.join("cli-proxy-api.service"), unit).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::fs::create_dir_all(home.join(".cli-proxy-api")).unwrap();
        std::fs::write(crate::config::path(home), format!("port: {port}\napi-keys:\n  - k\n{config}")).unwrap();
    }

    #[tokio::test]
    async fn only_a_proxy_reading_the_same_config_is_handed_over() {
        let home = tempfile::tempdir().unwrap();
        external_with_unit(home.path(), "[Service]\nExecStart=/usr/bin/cli-proxy-api --config /etc/cliproxy/config.yaml\n", "");
        let refused = handover(&Paths::new(home.path())).await.map_err(|f| f.code);
        assert_eq!(refused, Err("cliproxy_entrega_outro_config"), "as contas do outro config sumiriam");
    }

    #[tokio::test]
    async fn a_relative_accounts_folder_must_not_move() {
        let home = tempfile::tempdir().unwrap();
        let unit = "[Service]\nWorkingDirectory=/srv/cliproxy\nExecStart=/usr/bin/cli-proxy-api --config %h/.cli-proxy-api/config.yaml\n";
        external_with_unit(home.path(), unit, "auth-dir: auths\n");
        let refused = handover(&Paths::new(home.path())).await.map_err(|f| f.code);
        assert_eq!(refused, Err("cliproxy_entrega_pasta_das_contas"), "o serviço do Hangar roda na pasta pessoal e não acharia as contas");
    }

    #[test]
    fn an_absolute_or_home_accounts_folder_never_moves() {
        let home = tempfile::tempdir().unwrap();
        let unit = "[Service]\nWorkingDirectory=/srv/cliproxy\nExecStart=/usr/bin/cli-proxy-api\n";
        std::fs::create_dir_all(home.path().join(".cli-proxy-api")).unwrap();
        for dir in ["~/.cli-proxy-api", "/var/lib/cliproxy"] {
            std::fs::write(crate::config::path(home.path()), format!("port: 8317\nauth-dir: {dir}\n")).unwrap();
            assert_eq!(accounts_dir_moves(unit, home.path()), None, "{dir}");
        }
        std::fs::write(crate::config::path(home.path()), "port: 8317\nauth-dir: auths\n").unwrap();
        assert_eq!(accounts_dir_moves(unit, home.path()), Some("/srv/cliproxy".into()));
        assert_eq!(accounts_dir_moves("[Service]\nWorkingDirectory=%h\n", home.path()), None, "a pasta pessoal é a do serviço do Hangar");
    }

    fn context(home: &std::path::Path) -> Context {
        Context { paths: Paths::new(home), platform: Platform::local(), github: Github::default(), persist: Arc::new(|_| Ok(())),
            management_key: Arc::new(|| None), before_swap: None }
    }

    #[tokio::test]
    async fn the_version_is_never_asked_without_the_management_key() {
        // Pedido sem senha conta como tentativa errada no proxy: nem a conexão pode sair.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        assert!(running_version(&base, None).await.is_none());
        assert_eq!(listener.accept().map_err(|e| e.kind()).err(), Some(std::io::ErrorKind::WouldBlock));
    }

    /// Proxy instalado pelo Hangar e respondendo numa porta deste teste.
    fn managed_running(home: &std::path::Path) -> (Context, std::net::TcpListener) {
        let ctx = context(home);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::create_dir_all(home.join(".cli-proxy-api")).unwrap();
        std::fs::write(ctx.paths.config(), format!("port: {}\napi-keys:\n  - k\n", listener.local_addr().unwrap().port())).unwrap();
        let binary = ctx.paths.dir.join("bin/8.0.79/cli-proxy-api");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"x").unwrap();
        detect::write_record(&ctx.paths, "v8.0.79", &binary).unwrap();
        (ctx, listener)
    }

    #[tokio::test]
    async fn installing_over_a_running_managed_proxy_only_completes_the_config() {
        // Reiniciar o serviço cortaria as sessões no proxy: com ele respondendo, o pedido não chega ao serviço.
        let home = tempfile::tempdir().unwrap();
        let (ctx, _listener) = managed_running(home.path());
        let steps = Mutex::new(Vec::new());
        let outcome = install(&ctx, &|step| steps.lock().unwrap().push(step)).await;
        assert_eq!(outcome, Ok(Outcome::Installed { tag: "v8.0.79".into() }));
        assert_eq!(*steps.lock().unwrap(), [Step::Config], "sem passo de serviço nem de subida");
    }

    #[tokio::test]
    async fn a_task_that_panics_still_ends_the_job() {
        let installer = Arc::new(Installer::default());
        let home = tempfile::tempdir().unwrap();
        let (mut ctx, _listener) = managed_running(home.path());
        // O `ensure` grava a senha nova de gerenciamento: a gravação que entra em pânico derruba a tarefa no meio.
        ctx.persist = Arc::new(|_| panic!("persist"));
        installer.start("install", ctx.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        // estavel: o Installer não expõe sinal de fim; confere o estado publicado com teto de 10 s, que só estoura se travar.
        while installer.job()["done"] != true && Instant::now() < deadline { tokio::time::sleep(Duration::from_millis(20)).await; }
        assert_eq!(installer.job()["error"]["code"], "cliproxy_tarefa_caiu");
        assert!(installer.start("install", ctx).is_ok(), "a próxima tentativa não fica presa em ocupado");
    }

    #[test]
    fn an_invalid_port_stops_before_anything_changes() {
        let home = tempfile::tempdir().unwrap();
        let ctx = context(home.path());
        assert_eq!(base_url(&ctx), Ok(DEFAULT_BASE.to_owned()), "sem config vale a porta padrão");
        std::fs::create_dir_all(home.path().join(".cli-proxy-api")).unwrap();
        std::fs::write(ctx.paths.config(), "port: 70000\napi-keys:\n  - k\n").unwrap();
        assert_eq!(base_url(&ctx).map_err(|f| f.code), Err("cliproxy_porta_invalida"));
        std::fs::write(ctx.paths.config(), "port: 9000\napi-keys:\n  - k\n").unwrap();
        assert_eq!(base_url(&ctx), Ok("http://127.0.0.1:9000".to_owned()));
    }

    #[test]
    fn the_unit_config_is_resolved_against_home() {
        let home = tempfile::tempdir().unwrap();
        let unit = home.path().join("cli-proxy-api.service");
        let ours = crate::config::path(home.path());
        let units = [unit.clone()];
        let read = |unit: Option<&std::path::Path>| external_config(unit, &units, home.path());
        std::fs::write(&unit, "[Service]\nExecStart=/usr/bin/cli-proxy-api --config %h/.cli-proxy-api/config.yaml\n").unwrap();
        assert_eq!(read(Some(&unit)), Ok(Some(ours.clone())), "a unit do AUR lê o mesmo arquivo");
        std::fs::write(&unit, "[Service]\nExecStart=/usr/bin/cli-proxy-api --config /etc/cliproxy/config.yaml\n").unwrap();
        assert_eq!(read(Some(&unit)), Ok(Some(std::path::PathBuf::from("/etc/cliproxy/config.yaml"))));
        std::fs::write(&unit, "[Service]\nExecStart=/usr/bin/cli-proxy-api\n").unwrap();
        assert_eq!(read(Some(&unit)), Ok(Some(home.path().join("config.yaml"))), "sem --config: o config.yaml da pasta em que roda, a home");
        std::fs::write(&unit, "[Service]\nWorkingDirectory=%h/.cli-proxy-api\nExecStart=/usr/bin/cli-proxy-api\n").unwrap();
        assert_eq!(read(Some(&unit)), Ok(Some(ours.clone())), "a pasta declarada decide o config");
        assert_eq!(read(None), Ok(None));
        let unreadable = read(Some(&home.path().join("sumiu.service")));
        assert_eq!(unreadable.map_err(|f| f.code), Err("cliproxy_config_manual"), "unit ilegível não vira o config padrão");
        let drop_in = home.path().join("cli-proxy-api.service.d");
        std::fs::create_dir_all(&drop_in).unwrap();
        std::fs::write(drop_in.join("override.conf"), "[Service]\nExecStart=\nExecStart=/usr/bin/cli-proxy-api --config /srv/c.yaml\n").unwrap();
        assert_eq!(read(Some(&unit)).map_err(|f| f.code), Err("cliproxy_config_manual"), "drop-in que troca o comando pede configuração manual");
    }

    #[test]
    fn an_unreadable_config_never_becomes_the_default_port() {
        let home = tempfile::tempdir().unwrap();
        // Uma pasta no lugar do config.yaml: existe e não se lê.
        std::fs::create_dir_all(crate::config::path(home.path())).unwrap();
        assert_eq!(base_url(&context(home.path())).map_err(|f| f.code), Err("cliproxy_config_ilegivel"));
    }

    #[test]
    fn only_a_higher_semver_is_an_update() {
        assert!(newer("v8.0.80", "v8.0.79"));
        assert!(newer("v8.1.0", "8.0.99"));
        assert!(!newer("v8.0.79", "v8.0.79"));
        assert!(!newer("v8.0.78", "v8.0.79"));
        assert!(!newer("nightly", "v8.0.79"), "tag fora de semver nunca troca o programa");
    }
}
