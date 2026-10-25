//! Reserva pré-aquecida do Claude sem terminal: enquanto a tela de nova conversa está aberta, um
//! `claude -p` já sobe no cano, sem sidecar (fica fora da lista), e a abertura o adota no envio.
//! Passa pelo lançador único: `resolve_open`, o começo da abertura (`open::prepare_account`) e a
//! subida do cano sem sessão registrada (`gateway::spawn_unregistered`). Plano:
//! `docs/superpowers/plans/2026-10-22-reserva-pre-aquecida-claude.md`.
pub mod http;
pub mod store;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::accounts::environment::AccountEnvironment;
use crate::runtime::claude_launch::Launch;
use crate::runtime::process::{self, Cano};
use crate::session_open::{Account, AgentProvider, OpenCaller, OpenError, SessionOpenRequest, SessionSpec, Surface};

pub use store::{Reserve, Store, Ticket};

/// Variáveis que mudam a cada reserva (nome, chave e o token da ponte, que é HMAC dos dois): fora da impressão.
const IDENTITY: [&str; 4] = ["CP_SESSION_NAME", "CP_SESSION_KEY", "HANGAR_CANO_KEY", "HANGAR_PLUGIN_TOKEN"];

fn unsupported(what: &str) -> OpenError {
    OpenError::new(422, "erro_prewarm_nao_suportado", format!("a reserva pré-aquecida não vale para {what}"))
}

/// O que a reserva atende: Claude sem terminal, sem motor, worktree, ligação, criadora nem só leitura.
/// `name` e `initial_prompt` não fazem parte do pedido de reserva.
pub fn scope(request: &SessionOpenRequest) -> Result<(), OpenError> {
    if request.name.is_some() || request.initial_prompt.is_some() {
        return Err(OpenError::invalid_body("a reserva não leva name nem initial_prompt"));
    }
    adoptable(request)
}

/// O `scope` sem `name`/`initial_prompt`: o pedido de abertura que pode adotar uma reserva.
pub fn adoptable(request: &SessionOpenRequest) -> Result<(), OpenError> {
    let set = |v: &Option<String>| v.as_deref().is_some_and(|v| !v.is_empty());
    let refused = [
        (request.provider.as_deref().is_some_and(|p| p != "claude"), "outro provedor"),
        (request.headless == Some(false), "sessão com terminal"),
        (set(&request.engine) || request.engine_account.is_some(), "motor"),
        (request.branch.is_some() || request.new_branch || request.base.is_some(), "worktree"),
        (request.link, "sessão ligada"),
        (request.creator.is_some(), "sessão com criadora"),
        (request.read_only, "só leitura"),
        (request.codex_account.is_some() || set(&request.omp_profile), "outro provedor"),
    ];
    match refused.into_iter().find(|(hit, _)| *hit) {
        Some((_, what)) => Err(unsupported(what)),
        None => Ok(()),
    }
}

/// Impressão do lançamento: hash da pasta, do argv e do ambiente, sem `--session-id`/`--resume` e sem
/// as variáveis de identidade. Mesma impressão = o processo da reserva é o que a abertura subiria.
pub fn fingerprint(launch: &Launch, cwd: &str) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut feed = |part: &str| { ctx.update(part.as_bytes()); ctx.update(&[0]); };
    feed(cwd);
    let mut args = launch.program.iter();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--session-id" | "--resume") { args.next(); continue; }
        feed(arg);
    }
    feed("\u{1e}env");
    for (key, value) in launch.env.iter().filter(|(k, _)| !IDENTITY.contains(&k.as_str())) {
        feed(key);
        feed(value);
    }
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// O spec que a adoção compara: o resolvido sem o que muda a cada abertura. Fora: `key` e `conversation`
/// (a adoção fica com as da reserva, que já estão no processo), `name` (conferido à parte pelo `confirm`),
/// `created_at` (relógio da resolução) e `creator` (o escopo já recusa criadora).
pub fn adoption_spec(spec: &SessionSpec) -> SessionSpec {
    SessionSpec { key: String::new(), name: String::new(), conversation: None, created_at: 0.0, creator: None, ..spec.clone() }
}

/// O lançamento decidido de uma reserva, ainda sem processo.
pub struct Planned {
    /// Com a chave e o nome definitivos e `conversation` = o id da conversa.
    pub spec: SessionSpec,
    /// O modo de volta do plano que a resolução decidiu (entra no lançamento): a adoção o compara também.
    pub previous_non_plan: Option<crate::session_open::PermissionMode>,
    /// As escolhas de plugins e skills gravadas para a conversa da reserva: a sessão adotada usa estas.
    pub settings: Option<Value>,
    pub session_id: String,
    pub fingerprint: String,
    /// A casa onde o nome é reservado (`NameClaim`).
    pub home: PathBuf,
    /// A pasta efetiva da conta (a do servidor quando a sessão usa a padrão): onde o transcript nasce.
    pub config_dir: PathBuf,
    pub account: crate::accounts::AccountKey,
    /// Comando e ambiente do cano; o ambiente é segredo.
    pub launch: Launch,
}

/// O processo que acabou de subir, com a guarda compartilhada da conta ainda presa: quem chama a solta
/// só depois de pôr a reserva no `Store`, para a exclusão da conta sempre achá-la.
pub struct Spawned { pub cano: Cano, pub guard: Option<crate::accounts::AccountGuard> }

/// A parte da reserva que precisa do servidor (resolução, conta, subida). Os testes trocam por falsos.
pub trait Launcher: Send + Sync {
    fn new_session_id(&self) -> String;
    /// Resolve o pedido e monta o lançamento para a conversa `sid`, sem processo nem sidecar.
    fn plan<'a>(&'a self, request: &'a SessionOpenRequest, sid: &'a str) -> BoxFuture<'a, Result<Planned, OpenError>>;
    /// O começo da abertura (login, guarda, `--prep`) e a subida do cano.
    fn spawn<'a>(&'a self, planned: &'a Planned, sidecar_dir: &'a Path) -> BoxFuture<'a, Result<Spawned, OpenError>>;
}

/// Vida e morte do processo da reserva.
pub trait Processes: Send + Sync {
    fn alive<'a>(&'a self, cano: &'a Cano, key: &'a str) -> BoxFuture<'a, bool>;
    fn kill<'a>(&'a self, cano: &'a Cano, key: &'a str, sidecar_dir: &'a Path) -> BoxFuture<'a, Result<(), &'static str>>;
}

/// O cano de verdade (`process::kill` mata o grupo do `setsid` conferindo a identidade).
pub struct RealProcesses;

impl Processes for RealProcesses {
    fn alive<'a>(&'a self, cano: &'a Cano, key: &'a str) -> BoxFuture<'a, bool> {
        let (pid, key) = (cano.pid, key.to_owned());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || process::liveness(pid, &key) == process::Liveness::Ours).await.unwrap_or(false)
        })
    }
    fn kill<'a>(&'a self, cano: &'a Cano, key: &'a str, sidecar_dir: &'a Path) -> BoxFuture<'a, Result<(), &'static str>> {
        Box::pin(async move { process::kill(cano, key, sidecar_dir).await.map_err(|e| e.code()) })
    }
}

/// O lançador do servidor: o mesmo `ServerHost`, os mesmos fatos e o mesmo ambiente do runtime que
/// a abertura usa.
pub struct ServerLauncher { st: Arc<crate::routes::AppState>, host: tokio::sync::OnceCell<crate::session_open::server::ServerHost> }

impl ServerLauncher {
    pub fn new(st: Arc<crate::routes::AppState>) -> Self { Self { st, host: tokio::sync::OnceCell::new() } }

    async fn host(&self) -> &crate::session_open::server::ServerHost {
        self.host.get_or_init(|| crate::session_open::server::ServerHost::new(self.st.clone())).await
    }

}

/// O ambiente com que o runtime sobe o cano das sessões.
fn runtime_env(st: &crate::routes::AppState) -> AccountEnvironment {
    st.state.runtime.get().map_or_else(AccountEnvironment::capture, |runtime| runtime.codex_environment())
}

fn failed(detail: String) -> OpenError { OpenError::new(500, "erro_criacao_sessao", detail) }

/// `Launcher::plan` sobre um `ServerHost` que quem chama já tem (a abertura reusa o dela).
pub(crate) async fn plan_on(st: &crate::routes::AppState, host: &crate::session_open::server::ServerHost, request: &SessionOpenRequest, sid: &str)
    -> Result<Planned, OpenError> {
    use crate::session_open::open::OpenHost;
    let (facts_env, handle, asked) = (AccountEnvironment::from_map(host.env().clone()), tokio::runtime::Handle::current(), request.clone());
    let resolved = tokio::task::spawn_blocking(move || {
        let facts = crate::session_open::facts::LocalFacts::new(facts_env, handle);
        crate::session_open::resolve_open(&asked, &OpenCaller::Owner, &facts)
    }).await.unwrap_or_else(|_| Err(failed("a resolução da reserva não terminou".into())))?;
    // O que o pedido não disse (provedor e terminal padrão do servidor) também precisa caber.
    let spec = resolved.spec;
    if spec.provider != AgentProvider::Claude { return Err(unsupported("outro provedor")); }
    if spec.surface != Surface::Headless { return Err(unsupported("sessão com terminal")); }
    if matches!(spec.account, Account::Engine { .. }) { return Err(unsupported("motor")); }
    let mut spec = spec;
    spec.conversation = Some(sid.to_owned());
    let settings = crate::session_open::open::conversation_settings(host, &spec, sid, false).await?;
    let mut meta = crate::session_open::open::headless_sidecar(&spec, sid, resolved.previous_non_plan, settings.as_ref(), None);
    meta["name"] = json!(spec.name);
    meta["key"] = json!(spec.key);
    let env = runtime_env(st);
    let config_dir = spec.account.config_dir().map_or_else(|| env.claude_base.clone(), PathBuf::from);
    let account = crate::session_open::open::account_key(host, &spec, None)?;
    let (bridge, handle) = (host.bridge(), tokio::runtime::Handle::current());
    let (planned_spec, planned_meta, previous_non_plan) = (spec.clone(), meta, resolved.previous_non_plan);
    let launch = tokio::task::spawn_blocking(move || {
        let facts = crate::session_open::facts::LocalFacts::new(env.clone(), handle);
        crate::runtime::claude_launch::plan_for(&env, bridge.as_ref(), &planned_spec, &planned_meta, &facts)
    }).await.map_err(|_| failed("o comando da reserva não foi montado".into()))?
        .map_err(|e| failed(format!("o comando da reserva não foi montado: {}", e.code)))?;
    let fingerprint = fingerprint(&launch, &spec.cwd);
    Ok(Planned { session_id: sid.to_owned(), fingerprint, home: host.home().to_owned(), config_dir, account, launch, spec, previous_non_plan, settings })
}

impl Launcher for ServerLauncher {
    fn new_session_id(&self) -> String { crate::session_open::open::new_session_id() }

    fn plan<'a>(&'a self, request: &'a SessionOpenRequest, sid: &'a str) -> BoxFuture<'a, Result<Planned, OpenError>> {
        Box::pin(async move { plan_on(&self.st, self.host().await, request, sid).await })
    }

    fn spawn<'a>(&'a self, planned: &'a Planned, sidecar_dir: &'a Path) -> BoxFuture<'a, Result<Spawned, OpenError>> {
        Box::pin(async move {
            use crate::session_open::claude;
            use crate::session_open::open::OpenHost;
            let host = self.host().await;
            let spec = &planned.spec;
            let guard = prepare(host, spec).await?;
            if let Err(error) = claude::pretrust_async(claude::claude_json(host.terminal(), host.home(), spec.account.config_dir()), spec.cwd.clone(),
                host.terminal().windows).await {
                tracing::warn!(code = "prewarm_confiar_pasta", "{error}");
            }
            let (dir, config_dir) = (sidecar_dir.to_owned(), spec.account.config_dir().map(str::to_owned));
            let stamp = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&dir)?;
                Ok::<_, std::io::Error>(crate::runtime::gateway::claude_config_stamp(config_dir.as_deref()))
            }).await.map_err(|_| failed("a pasta do cano não foi preparada".into()))?
                .map_err(|e| failed(format!("a pasta do cano não foi preparada: {:?}", e.kind())))?;
            let launch = process::LaunchSpec { provider: process::Provider::Claude, key: spec.key.clone(), cwd: spec.cwd.clone().into(),
                program: planned.launch.program.clone(), env: planned.launch.env.clone().into_iter().collect(), sidecar_dir: sidecar_dir.to_owned() };
            let cano = crate::runtime::gateway::spawn_unregistered(&launch, stamp).await
                .map_err(|e| OpenError::new(503, "erro_prewarm_nao_subiu", format!("o processo da reserva não subiu: {}", e.code())))?;
            Ok(Spawned { cano, guard })
        })
    }
}

/// O começo da abertura para a reserva: o modelo de motor sai do `settings.json` (a reserva nasceria nele)
/// e a conta é preparada como na abertura (login, guarda, reconciliação no processo); os avisos vão ao log.
async fn prepare(host: &dyn crate::session_open::open::OpenHost, spec: &SessionSpec)
    -> Result<Option<crate::accounts::AccountGuard>, OpenError> {
    for warning in crate::session_open::claude::drop_foreign_model(host.home(), spec.account.config_dir()) {
        tracing::warn!(code = "prewarm_aviso", "{warning}");
    }
    let (guard, warnings) = crate::session_open::open::prepare_account(host, spec, None).await?;
    for warning in warnings { tracing::warn!(code = "prewarm_aviso", "{warning}"); }
    Ok(guard)
}

/// O evento do diário (`sessao.prewarm`): etapa e motivo, nunca conversa.
pub(crate) fn diary_event(name: &str, etapa: &str, motivo: &str) -> Value {
    let nivel = if etapa == "reservada" || etapa == "adotada" { "ok" } else { "aviso" };
    json!({"evento": "sessao.prewarm", "nivel": nivel, "sessao": name, "provider": "claude", "etapa": etapa, "motivo": motivo})
}
