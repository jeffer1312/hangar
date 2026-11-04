use super::{
    Catalog, CommandInfo,
    cache::{Cache, valid_commands},
    probe::NativeProbe,
};
use crate::{accounts::AccountService, routes::AppState};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct Context {
    pub cwd: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub settings: Value,
    pub terminal: bool,
    pub cli: Vec<Value>,
    pub terminal_commands: Value,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            cwd: None,
            config_dir: None,
            settings: Value::Null,
            terminal: true,
            cli: Vec::new(),
            terminal_commands: Value::Null,
        }
    }
}

impl Context {
    pub fn headless(metadata: &Value, view: &Value) -> Self {
        Self {
            cwd: metadata["cwd"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            config_dir: metadata["config_dir"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            settings: metadata["claude_settings"].clone(),
            terminal: false,
            cli: valid_commands(&view["commands"]),
            terminal_commands: view["terminal_commands"].clone(),
        }
    }
}

#[derive(Default)]
pub struct Service {
    catalog: Mutex<Catalog>,
    caches: Mutex<HashMap<PathBuf, Arc<Cache>>>,
}

impl Service {
    pub async fn list(
        self: &Arc<Self>,
        accounts: AccountService,
        context: Context,
    ) -> Result<Vec<CommandInfo>, &'static str> {
        let cache = self
            .caches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(accounts.env.home.clone())
            .or_insert_with(|| Arc::new(Cache::new(&accounts.env.home)))
            .clone();
        let cli = if context.cli.is_empty() {
            cache
                .get(
                    Arc::new(NativeProbe {
                        accounts: accounts.clone(),
                    }),
                    context.config_dir.clone(),
                )
                .await
        } else {
            Some(context.cli.clone())
        };
        let home = accounts.env.home.clone();
        let config_dir = context
            .config_dir
            .clone()
            .unwrap_or_else(|| accounts.env.claude_base.clone());
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            let excluded: Vec<String> = if context.terminal {
                Vec::new()
            } else {
                let names = context
                    .terminal_commands
                    .as_array()
                    .filter(|v| !v.is_empty())
                    .cloned()
                    .unwrap_or_else(|| vec![json!("color"), json!("doctor")]);
                // O CLI ainda marca reload-plugins como TUI embora o aceite em stream-json.
                names
                    .into_iter()
                    .filter_map(|v| {
                        v.as_str()
                            .filter(|n| *n != "reload-plugins")
                            .map(str::to_owned)
                    })
                    .collect()
            };
            let commands = service
                .catalog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .compose(
                    &home,
                    context.cwd.as_deref(),
                    cli.as_deref(),
                    &excluded,
                    context.terminal,
                );
            match crate::claude_customizations::filter_commands(
                context.cwd,
                config_dir,
                context.settings,
                &commands,
            ) {
                Ok(hidden) => commands
                    .into_iter()
                    .filter(|c| !hidden.contains(&c.name))
                    .collect(),
                Err(error) => {
                    tracing::warn!(
                        code = error.code,
                        "filtro de comandos indisponível; conservando o catálogo completo"
                    );
                    commands
                }
            }
        })
        .await
        .map_err(|_| "claude_commands_catalog_failed")
    }
}

pub async fn route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (forward, who) = match crate::routes::who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let Some(name) = crate::session_write::session_name(req.uri().path()).filter(|name| {
        who.as_ref()
            .is_some_and(|caller| caller.can(name, req.method()))
    }) else {
        return crate::routes::pass(&st, req, &forward).await;
    };
    let codex = st
        .accounts
        .env
        .home
        .join(".hangar/codex-sessions")
        .join(format!(
            "{}.json",
            crate::list::discover_other::sanitize_session_name(&name)
        ));
    let runtime_codex = match st.state.runtime.get() {
        Some(runtime) => runtime.catalog_provider(&name).await.as_deref() == Some("codex"),
        None => false,
    };
    if runtime_codex || codex.is_file() {
        return crate::session_write::codex::commands(State(st), ConnectInfo(peer), req).await;
    }
    let headers = req.headers().clone();
    let mut response = serve(&st, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    response
}

pub async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let Some(name) = crate::session_write::session_name(req.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let context = match context(st, &name).await {
        Ok(context) => context,
        Err(code) => return failed(st, &name, code),
    };
    match st.command_catalog.list(st.accounts.clone(), context).await {
        Ok(commands) => crate::session_write::json_response(StatusCode::OK, json!(commands)),
        Err(code) => failed(st, &name, code),
    }
}

pub(crate) fn matches(path: &str) -> bool {
    path.strip_prefix("/api/sessions/")
        .and_then(|tail| tail.split_once('/'))
        .is_some_and(|(name, route)| !name.is_empty() && route == "commands")
}

fn failed(st: &AppState, name: &str, code: &'static str) -> Response {
    st.diag.report(
        "rust.commands_failed",
        name,
        code,
        "catálogo de comandos indisponível",
    );
    crate::session_write::json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        crate::session_write::detail_body(
            code,
            "Não foi possível consultar os comandos desta sessão.",
            json!({}),
        ),
    )
}

async fn context(st: &Arc<AppState>, name: &str) -> Result<Context, &'static str> {
    if let Some(runtime) = st.state.runtime.get() {
        if let Some(view) = runtime.claude_catalog_view(name).await {
            let (metadata, view) = view.map_err(|_| "claude_commands_runtime_unavailable")?;
            return Ok(Context::headless(&metadata, &view));
        }
        if let Some(target) = runtime
            .terminal_target(name)
            .await
            .map_err(|_| "claude_commands_runtime_unavailable")?
        {
            let procs = st.list.env().procs.clone();
            let pid = i64::from(target.metadata.agent_pid);
            let found = tokio::task::spawn_blocking(move || process_context(&*procs, pid, None))
                .await
                .map_err(|_| "claude_commands_context_failed")?;
            if found.cwd.is_some() {
                return Ok(found);
            }
        }
    }
    let path = st
        .accounts
        .env
        .home
        .join(".hangar/claude-headless")
        .join(format!(
            "{}.json",
            crate::list::discover_other::sanitize_session_name(name)
        ));
    let metadata = tokio::task::spawn_blocking(move || read_metadata(&path))
        .await
        .map_err(|_| "claude_commands_context_failed")?;
    if let Some(metadata) = metadata {
        return Ok(Context::headless(&metadata, &Value::Null));
    }
    match st.list.discovered_session(name).await {
        Ok(Some((row, pid))) => {
            let procs = st.list.env().procs.clone();
            tokio::task::spawn_blocking(move || {
                let cwd = row.cwd.filter(|v| !v.is_empty()).map(PathBuf::from);
                match pid {
                    Some(pid) => process_context(&*procs, i64::from(pid), cwd),
                    None => Context {
                        cwd,
                        ..Default::default()
                    },
                }
            })
            .await
            .map_err(|_| "claude_commands_context_failed")
        }
        outcome => {
            let code = outcome
                .err()
                .map(|e| e.code)
                .unwrap_or("claude_commands_session_missing");
            st.diag.report(
                "rust.commands_context",
                name,
                code,
                "sem pasta da sessão; catálogo global de reserva",
            );
            Ok(Context::default())
        }
    }
}

fn read_metadata(path: &Path) -> Option<Value> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(value) if value.is_object() => Some(value),
            _ => {
                tracing::warn!(
                    code = "claude_commands_sidecar_invalid",
                    "sidecar da sessão ilegível; usando contexto de reserva"
                );
                None
            }
        },
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(code = "claude_commands_sidecar_unreadable", io_kind = ?error.kind(), "sidecar da sessão ilegível");
            }
            None
        }
    }
}

fn process_context(
    procs: &dyn crate::list::procs::ProcessView,
    pid: i64,
    cwd: Option<PathBuf>,
) -> Context {
    let variable = |key| match procs.env_var(pid, key) {
        Ok(value) => value.filter(|v| !v.is_empty()),
        Err(_) => {
            tracing::warn!(
                code = "claude_commands_environ_unreadable",
                "ambiente do agente ilegível; usando contexto de reserva"
            );
            None
        }
    };
    let settings = variable("HANGAR_CLAUDE_SETTINGS")
        .and_then(|raw| serde_json::from_str(&raw.to_string_lossy()).ok())
        .unwrap_or(Value::Null);
    Context {
        cwd: cwd.or_else(|| procs.cwd(pid)),
        config_dir: variable("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        settings,
        ..Default::default()
    }
}
