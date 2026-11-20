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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Download,
    Config,
    Service,
    Start,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed { tag: String },
    /// Proxy de fora do Hangar: só o config foi completado.
    Configured,
    Updated { tag: String },
}

/// A senha de gerenciamento nova vai para o Hangar antes do config.
pub type Persist = Arc<dyn Fn(&str) -> std::io::Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    pub platform: Platform,
    pub github: Github,
    pub persist: Persist,
}

fn now_unix() -> u64 {
    hangar_core::clock::wall_now() as u64
}

fn ensure(ctx: &Context, ultrafast_rule: bool) -> Result<config::Ensured, Failure> {
    let persist = ctx.persist.clone();
    Ok(config::ensure(&ctx.paths.home, now_unix(), move |key| persist(key), ultrafast_rule)?)
}

/// A versão que o proxy em execução anuncia; a rota de gerenciamento manda o cabeçalho mesmo sem a senha.
async fn running_version(base: &str) -> Option<semver::Version> {
    let client = hangar_core::net::http_client().timeout(Duration::from_secs(5)).build().ok()?;
    let response = client.get(format!("{base}/v0/management/config")).send().await.ok()?;
    let version = response.headers().get("x-cpa-version")?.to_str().ok()?;
    semver::Version::parse(version.trim().trim_start_matches('v')).ok()
}

/// O filtro por cabeçalho das regras de `payload` chegou no v8; sem versão conhecida, a regra fica de fora.
fn understands_rule(version: Option<&semver::Version>) -> bool { version.is_some_and(|v| v.major >= 8) }

/// Senha de gerenciamento que o Hangar não conhece: troca por uma nova, guardada por `persist`.
pub fn reset_management(ctx: &Context) -> Result<(), Failure> {
    let persist = ctx.persist.clone();
    config::reset_management(&ctx.paths.home, now_unix(), move |key| persist(key))?;
    Ok(())
}

fn base_url(ctx: &Context) -> String {
    config::read(&ctx.paths.home).ok().flatten().and_then(|c| c.base_url.ok()).unwrap_or_else(|| DEFAULT_BASE.into())
}

async fn port_open(base: &str) -> bool {
    let base = base.to_owned();
    tokio::task::spawn_blocking(move || detect::port_open(&base)).await.unwrap_or(false)
}

async fn wait_up(base: &str) -> Result<(), Failure> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if port_open(base).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(Failure::new("cliproxy_nao_subiu", format!("o proxy não respondeu em {base} depois de {}s", START_TIMEOUT.as_secs())))
}

pub fn current(paths: &Paths) -> Install {
    detect::state(paths, std::env::var_os("PATH").as_deref(), &detect::system_units(&paths.home))
}

pub async fn install(ctx: &Context, progress: &(dyn Fn(Step) + Send + Sync)) -> Result<Outcome, Failure> {
    let (tag, binary) = match current(&ctx.paths) {
        Install::External { .. } => {
            progress(Step::Config);
            let version = running_version(&base_url(ctx)).await;
            ensure(ctx, understands_rule(version.as_ref()))?;
            return Ok(Outcome::Configured);
        }
        Install::Managed { tag, binary } => (tag, binary),
        Install::Absent => {
            // Algo já atende na porta e não é instalação conhecida: subir outro proxy ali falharia calado.
            let base = base_url(ctx);
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
    ensure(ctx, understands_rule(semver::Version::parse(tag.trim_start_matches('v')).ok().as_ref()))?;
    progress(Step::Service);
    service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: binary.clone(), config: ctx.paths.config() }).await?;
    detect::write_record(&ctx.paths, &tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    progress(Step::Start);
    wait_up(&base_url(ctx)).await?;
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
    let binary = release::fetch(&ctx.github, &latest, ctx.platform, &ctx.paths.dir).await?;
    progress(Step::Service);
    service::register(&ctx.paths.home, &ctx.paths.dir, &Service { binary: binary.clone(), config: ctx.paths.config() }).await?;
    detect::write_record(&ctx.paths, &latest.tag, &binary).map_err(|e| Failure::new("cliproxy_registro_falhou", e.to_string()))?;
    progress(Step::Start);
    wait_up(&base_url(ctx)).await?;
    if let Some(dir) = old.parent().filter(|d| Some(*d) != binary.parent()) {
        std::fs::remove_dir_all(dir).map_err(|e| Failure::new("cliproxy_versao_antiga_ficou", format!("{}: {e}", dir.display())))?;
    }
    Ok(Outcome::Updated { tag: latest.tag })
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

    /// A release mais nova, relida no máximo uma vez por hora; falha também fica guardada por esse prazo.
    pub async fn latest(&self, github: &Github) -> Result<Release, String> {
        let mut cached = self.latest.lock().await;
        if let Some((_, value)) = cached.as_ref().filter(|(at, _)| at.elapsed() < LATEST_TTL) {
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
    fn only_a_higher_semver_is_an_update() {
        assert!(newer("v8.0.80", "v8.0.79"));
        assert!(newer("v8.1.0", "8.0.99"));
        assert!(!newer("v8.0.79", "v8.0.79"));
        assert!(!newer("v8.0.78", "v8.0.79"));
        assert!(!newer("nightly", "v8.0.79"), "tag fora de semver nunca troca o programa");
    }
}
