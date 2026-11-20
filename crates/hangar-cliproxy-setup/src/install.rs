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

/// O filtro por cabeçalho das regras de `payload` chegou no v8; sem versão conhecida, a regra fica de fora.
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
    match config::read(&ctx.paths.home) {
        Ok(Some(config)) => config.base_url.map_err(|_| Failure::new("cliproxy_porta_invalida",
            format!("{}: server.port (ou port) fora de 1–65535", ctx.paths.config().display()))),
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
    let Some(raw) = detect::unit_config(&text) else { return Ok(None) };
    let raw = raw.to_string_lossy();
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
        if let Err(e) = service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: old.clone(), config }).await {
            return Err(Failure::new(failure.code, format!("{}; e o serviço não voltou para {installed}: {}", failure.detail, Failure::from(e).detail)));
        }
        return Err(failure);
    }
    detect::write_record(&ctx.paths, &latest.tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    // `payload` da pessoa fica como está: a regra só entra num config sem ele.
    if understands_rule(version.as_ref()) {
        ensure(ctx, true)?;
    }
    if let Some(dir) = old.parent().filter(|d| Some(*d) != binary.parent()) {
        std::fs::remove_dir_all(dir).map_err(|e| Failure::new("cliproxy_versao_antiga_ficou", format!("{}: {e}", dir.display())))?;
    }
    Ok(Outcome::Updated { tag: latest.tag })
}

impl Installer {
    pub fn start(self: &Arc<Self>, kind: &'static str, ctx: Context) -> Result<(), Busy> {
        {
            let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
            if job.as_ref().is_some_and(|j| j.result.is_none()) {
                return Err(Busy);
            }
            *job = Some(Job { kind, step: if kind == "update" { Step::Download } else { Step::Config }, result: None });
        }
        let this = self.clone();
        tokio::spawn(async move {
            let progress_owner = this.clone();
            let progress = move |step: Step| {
                if let Some(job) = progress_owner.job.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                    job.step = step;
                }
            };
            let result = if kind == "update" { update(&ctx, &progress).await } else { install(&ctx, &progress).await };
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
        assert_eq!(read(Some(&unit)), Ok(None), "sem --config o proxy lê o padrão");
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
