//! Plugins do Claude Code da pasta principal (`~/.claude`) pela CLI oficial: só o dono, uma mudança
//! por vez, e a mudança termina mesmo com o cliente fora.
use std::{future::Future, net::SocketAddr, path::{Path, PathBuf}, str::FromStr, sync::Arc, time::Duration};

use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::StatusCode,
    response::Response,
};
use hangar_core::http::{detail, json_response};
use hangar_plugins::{
    AutoUpdateError, CliError, CliOutput, CliRunner, InputError, MarketplaceName, MarketplaceSource,
    OperationResult, PluginId, Plugins, PluginsError, PluginsState, apply_auto_update, read_state,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    accounts::environment::AccountEnvironment,
    command_capture::{CaptureOptions, capture_bytes},
    config_sync::bundle::Error as EditError,
    routes::{AppState, gate},
};

/// Toda chamada a `claude plugin` liga estas com `1`: sem atualização da própria CLI e sem tráfego que
/// não seja o da operação.
pub(crate) const QUIET_ENV: &[&str] = &["DISABLE_AUTOUPDATER", "DISABLE_UPDATES", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"];
/// E tira esta: os plugins da sessão mudariam o que a CLI lê.
pub(crate) const SESSION_PLUGINS_ENV: &str = "CLAUDE_CODE_PLUGIN_DIRS";
/// O pane: a CLI se penduraria nele.
const PANE_ENV: &[&str] = &["TMUX", "TMUX_PANE"];

/// Estado do servidor: o cadeado das mudanças e o último `check` bem-sucedido.
#[derive(Default)]
pub struct Service {
    busy: Arc<tokio::sync::Mutex<()>>,
    checked_at: std::sync::Mutex<Option<String>>,
}

/// `claude plugin …` na pasta padrão: sem `CLAUDE_CONFIG_DIR` e com cwd na home, para nenhum
/// `.claude/settings.json` de projeto mudar o resultado.
#[derive(Clone)]
struct Runner {
    env: AccountEnvironment,
    program: Option<Vec<String>>,
}

impl Runner {
    fn new(state: &AppState) -> Self {
        let program = state.accounts.claude_program().map(|argv| argv.iter().map(|part| part.to_string_lossy().into_owned()).collect());
        let mut env = state.accounts.env.clone();
        env.base.retain(|key, _| {
            let key = key.to_ascii_uppercase();
            key != SESSION_PLUGINS_ENV && !PANE_ENV.contains(&key.as_str()) && !key.starts_with("CP_SESSION_")
        });
        for key in QUIET_ENV {
            env.base.insert((*key).to_owned(), "1".to_owned());
        }
        Self { env, program }
    }
}

impl CliRunner for Runner {
    fn run(&self, args: &[&str], timeout: Duration) -> impl Future<Output = Result<CliOutput, CliError>> + Send {
        let argv = self.program.clone().map(|mut argv| {
            argv.extend(args.iter().map(|arg| (*arg).to_owned()));
            argv
        });
        let env = self.env.clone();
        async move {
            let Some(argv) = argv else { return Err(CliError::Missing) };
            let cwd = Some(env.home.clone());
            let options = CaptureOptions { cwd, preserve_account: false, ..Default::default() };
            match capture_bytes(argv, env, timeout, options).await {
                Ok(output) => Ok(CliOutput {
                    success: output.success,
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                }),
                Err(failure) => Err(match failure.kind.as_str() {
                    "FileNotFoundError" => CliError::Missing,
                    "timeout" => CliError::Timeout,
                    other => CliError::Spawn(other.to_owned()),
                }),
            }
        }
    }
}

/// O que uma mudança devolve antes da releitura: `None` para o `check`, que não tem resultado.
type Change = Result<Option<OperationResult>, Response>;

struct Context {
    plugins: Plugins<Runner>,
    main_dir: PathBuf,
    backups: PathBuf,
    app: Arc<AppState>,
}

#[derive(Deserialize)]
struct SourceBody {
    source: String,
}

#[derive(Deserialize)]
struct AutoUpdateBody {
    enabled: bool,
}

#[derive(Clone, Copy)]
enum PluginOp {
    Install,
    Update,
    Enable,
    Disable,
}

/// Erro já é resposta: o `?` devolve a recusa como está.
type Reply = Result<Response, Response>;

/// `GET /api/plugins`.
pub async fn state(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Reply {
    owner(&st, peer, &request).await?;
    let state = Context::new(st).reread().await?;
    Ok(json_response(StatusCode::OK, json!(state)))
}

/// `GET /api/plugins/available`.
pub async fn available(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Reply {
    owner(&st, peer, &request).await?;
    let plugins = Context::new(st).plugins.available().await.map_err(plugins_error)?;
    Ok(json_response(StatusCode::OK, json!({"plugins": plugins})))
}

/// `GET /api/plugins/{id}/details`.
pub async fn details(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let id: PluginId = parsed(&raw)?;
    let details = Context::new(st).plugins.details(&id).await.map_err(plugins_error)?;
    Ok(json_response(StatusCode::OK, json!(details)))
}

/// `POST /api/plugins/check`: baixa todos os marketplaces e guarda a hora do sucesso.
pub async fn check(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Reply {
    owner(&st, peer, &request).await?;
    mutate(st, move |context| async move {
        context.plugins.check().await.map_err(plugins_error)?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        *context.app.claude_plugins.checked_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
        Ok(None)
    })
    .await
}

pub async fn install(st: State<Arc<AppState>>, peer: ConnectInfo<SocketAddr>, id: RoutePath<String>, request: Request) -> Reply {
    plugin_op(PluginOp::Install, st, peer, id, request).await
}

pub async fn update(st: State<Arc<AppState>>, peer: ConnectInfo<SocketAddr>, id: RoutePath<String>, request: Request) -> Reply {
    plugin_op(PluginOp::Update, st, peer, id, request).await
}

pub async fn enable(st: State<Arc<AppState>>, peer: ConnectInfo<SocketAddr>, id: RoutePath<String>, request: Request) -> Reply {
    plugin_op(PluginOp::Enable, st, peer, id, request).await
}

pub async fn disable(st: State<Arc<AppState>>, peer: ConnectInfo<SocketAddr>, id: RoutePath<String>, request: Request) -> Reply {
    plugin_op(PluginOp::Disable, st, peer, id, request).await
}

/// `DELETE /api/plugins/{id}`: a CLI apaga `enabledPlugins[id]`, e o espelho não apaga nas contas;
/// o `false` no principal é o que chega a elas.
pub async fn uninstall(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let id: PluginId = parsed(&raw)?;
    mutate(st, move |context| async move {
        context.settings_writable().await?;
        let result = context.plugins.uninstall(&id).await.map_err(plugins_error)?;
        context.disable_in_settings(vec![id.as_str().to_owned()]).await?;
        Ok(Some(result))
    })
    .await
}

/// `POST /api/plugins/marketplaces` `{source}`.
pub async fn marketplace_add(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Reply {
    owner(&st, peer, &request).await?;
    let body: SourceBody = read_body(request).await?;
    let source: MarketplaceSource = parsed(&body.source)?;
    mutate(st, move |context| async move { context.plugins.marketplace_add(&source).await.map(Some).map_err(plugins_error) }).await
}

/// `POST /api/plugins/marketplaces/{name}/update`.
pub async fn marketplace_update(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let name: MarketplaceName = parsed(&raw)?;
    mutate(st, move |context| async move { context.plugins.marketplace_update(&name).await.map(Some).map_err(plugins_error) }).await
}

/// `DELETE /api/plugins/marketplaces/{name}`: os plugins dele que estavam instalados ficam `false`
/// no principal, como no [`uninstall`].
pub async fn marketplace_remove(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let name: MarketplaceName = parsed(&raw)?;
    mutate(st, move |context| async move {
        context.settings_writable().await?;
        let before = context.reread().await?;
        let removed: Vec<String> =
            before.plugins.into_iter().filter(|plugin| plugin.marketplace == name.as_str()).map(|plugin| plugin.id).collect();
        let result = context.plugins.marketplace_remove(&name).await.map_err(plugins_error)?;
        if !removed.is_empty() {
            context.disable_in_settings(removed).await?;
        }
        Ok(Some(result))
    })
    .await
}

/// `PUT /api/plugins/marketplaces/{name}/auto-update` `{enabled}`.
pub async fn auto_update(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let name: MarketplaceName = parsed(&raw)?;
    let body: AutoUpdateBody = read_body(request).await?;
    mutate(st, move |context| async move {
        let settings = context.settings_path();
        let backups = context.backups.clone();
        let edit = tokio::task::spawn_blocking(move || {
            crate::config_sync::files::edit_json(&settings, &backups, |current| {
                apply_auto_update(current, &name, body.enabled).map_err(auto_update_error)
            })
        });
        settings_written(edit.await)?;
        Ok(Some(OperationResult { outcome: "ok".to_owned(), message: String::new(), old_version: None, new_version: None }))
    })
    .await
}

/// Só o dono: o `owner_gate` deixaria o convidado passar. Não segura o `&Request` no `await`
/// (o corpo não é `Sync`, e o handler deixaria de ser `Send`).
fn owner(st: &AppState, peer: SocketAddr, request: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let admission = gate(st, peer, request);
    async move {
        match admission.await {
            Ok((_, true)) => Ok(()),
            Ok((_, false)) => Err(failure(StatusCode::FORBIDDEN, "plugins_owner_only", None)),
            Err(error) => Err(crate::auth::unavailable(error)),
        }
    }
}

async fn plugin_op(
    op: PluginOp,
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(raw): RoutePath<String>,
    request: Request,
) -> Reply {
    owner(&st, peer, &request).await?;
    let id: PluginId = parsed(&raw)?;
    mutate(st, move |context| async move {
        let result = match op {
            PluginOp::Install => context.plugins.install(&id).await,
            PluginOp::Update => context.plugins.update(&id).await,
            PluginOp::Enable => context.plugins.enable(&id).await,
            PluginOp::Disable => context.plugins.disable(&id).await,
        };
        result.map(Some).map_err(plugins_error)
    })
    .await
}

/// Uma mudança por servidor. Ela roda numa tarefa própria que leva o cadeado: o cliente que cai
/// não mata a CLI pela metade (a captura encerra o filho quando quem espera desiste).
async fn mutate<F, Fut>(st: Arc<AppState>, change: F) -> Reply
where
    F: FnOnce(Arc<Context>) -> Fut + Send + 'static,
    Fut: Future<Output = Change> + Send + 'static,
{
    let Ok(guard) = st.claude_plugins.busy.clone().try_lock_owned() else {
        return Err(failure(StatusCode::CONFLICT, "plugins_busy", None));
    };
    let context = Arc::new(Context::new(st));
    let task = tokio::spawn(async move {
        let _guard = guard;
        let result = change(context.clone()).await?;
        let state = context.reread().await?;
        let mut body = json!(state);
        if let Some(result) = result {
            body["result"] = json!(result);
        }
        Ok::<_, Response>(json_response(StatusCode::OK, body))
    });
    task.await.unwrap_or_else(|error| Err(task_failed(&error)))
}

impl Context {
    fn new(app: Arc<AppState>) -> Self {
        let home = &app.accounts.env.home;
        Self {
            plugins: Plugins::new(Runner::new(&app)),
            // A mesma pasta que o espelho de contas lê (`accounts/claude_config.rs`).
            main_dir: home.join(".claude"),
            backups: home.join(".hangar/plugins/backups"),
            app,
        }
    }

    fn settings_path(&self) -> PathBuf {
        self.main_dir.join("settings.json")
    }

    /// Sempre do disco: a CLI, o `/plugin` das sessões e o omp-sync gravam os mesmos arquivos.
    async fn reread(&self) -> Result<PluginsState, Response> {
        let settings = read_settings(&self.settings_path()).await;
        let installed = self.plugins.installed().await.map_err(plugins_error)?;
        let main_dir = self.main_dir.clone();
        // Lê o catálogo e o clone de cada marketplace: fora das threads do runtime.
        let mut state = tokio::task::spawn_blocking(move || read_state(&main_dir, &settings, &installed))
            .await
            .map_err(|error| task_failed(&error))?;
        state.checked_at = self.app.claude_plugins.checked_at.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(state)
    }

    /// Conferido antes da CLI: depois dela a remoção já valeu, e o `false` que a leva às contas
    /// precisa caber no arquivo.
    async fn settings_writable(&self) -> Result<(), Response> {
        let settings = read_settings(&self.settings_path()).await;
        let fits = settings.as_object().is_some_and(|map| map.get("enabledPlugins").is_none_or(Value::is_object));
        if fits { Ok(()) } else { Err(failure(StatusCode::INTERNAL_SERVER_ERROR, "plugins_settings_unreadable", None)) }
    }

    async fn disable_in_settings(&self, ids: Vec<String>) -> Result<(), Response> {
        let settings = self.settings_path();
        let backups = self.backups.clone();
        let edit = tokio::task::spawn_blocking(move || {
            crate::config_sync::files::edit_json(&settings, &backups, |current| {
                let enabled = current
                    .as_object_mut()
                    .ok_or_else(|| unreadable("settings.json não é objeto"))?
                    .entry("enabledPlugins")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| unreadable("enabledPlugins não é objeto"))?;
                for id in &ids {
                    enabled.insert(id.clone(), Value::Bool(false));
                }
                Ok(())
            })
        });
        settings_written(edit.await)
    }
}

/// Ausente vale `{}`; ilegível vale `null`, e a lista sai sem o que dependia dele.
async fn read_settings(path: &Path) -> Value {
    match tokio::fs::read(path).await {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or(Value::Null),
        Err(_) => json!({}),
    }
}

async fn read_body<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, Response> {
    let raw = to_bytes(request.into_body(), 64 * 1024).await.map_err(|_| body_invalid())?;
    serde_json::from_slice(&raw).map_err(|_| body_invalid())
}

fn body_invalid() -> Response {
    detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo inválido", json!({}))
}

fn unreadable(message: &str) -> EditError {
    EditError::new("plugins_settings_unreadable", message, json!({}))
}

fn auto_update_error(error: AutoUpdateError) -> EditError {
    EditError::new(error.code(), error.code(), json!({}))
}

fn settings_written(result: Result<Result<bool, EditError>, tokio::task::JoinError>) -> Result<(), Response> {
    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) if error.code == AutoUpdateError::AutoUpdateUnavailable.code() => {
            Err(failure(StatusCode::CONFLICT, error.code, None))
        }
        // Outro escritor (o Claude, o espelho) mexeu no arquivo no meio: é tentar de novo, não arquivo ruim.
        Ok(Err(error)) if error.code == "config_sync_changed" => {
            Err(failure(StatusCode::CONFLICT, "plugins_busy", Some(&error.message)))
        }
        // `config_sync_invalid_json` e os outros: nada foi gravado.
        Ok(Err(error)) => Err(failure(StatusCode::INTERNAL_SERVER_ERROR, "plugins_settings_unreadable", Some(&error.message))),
        Err(error) => Err(failure(StatusCode::INTERNAL_SERVER_ERROR, "plugins_settings_unreadable", Some(&error.to_string()))),
    }
}

/// Caminho ou corpo fora do formato: 422 com o código da entrada.
fn parsed<T: FromStr<Err = InputError>>(raw: &str) -> Result<T, Response> {
    raw.parse().map_err(|error: InputError| failure(StatusCode::UNPROCESSABLE_ENTITY, error.code(), None))
}

fn task_failed(error: &tokio::task::JoinError) -> Response {
    tracing::warn!(code = "plugins_task_failed", %error, "plugins: a tarefa caiu");
    failure(StatusCode::BAD_GATEWAY, "plugins_cli_failed", Some(&error.to_string()))
}

fn plugins_error(error: PluginsError) -> Response {
    let status = match error {
        PluginsError::NotFound(_) | PluginsError::UnknownMarketplace(_) => StatusCode::NOT_FOUND,
        PluginsError::BuiltinLocked | PluginsError::NeedsReview(_) => StatusCode::CONFLICT,
        PluginsError::CliMissing => StatusCode::SERVICE_UNAVAILABLE,
        PluginsError::CliFailed(_) => StatusCode::BAD_GATEWAY,
        PluginsError::CliTimeout => StatusCode::GATEWAY_TIMEOUT,
    };
    failure(status, error.code(), error.message())
}

/// `params.message` leva o texto da CLI quando há; o cliente mostra o texto do código.
fn failure(status: StatusCode, code: &str, message: Option<&str>) -> Response {
    let params = message.map_or_else(|| json!({}), |message| json!({"message": message}));
    detail(status, code, message.unwrap_or(code), params)
}

/// O Rust responde a família inteira, inclusive o método recusado (405): nada vai ao Python.
pub fn matches(path: &str) -> bool {
    path == "/api/plugins" || path.starts_with("/api/plugins/")
}
