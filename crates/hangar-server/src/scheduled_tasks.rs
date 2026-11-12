//! Ligação do agendador com a identidade do dono e os serviços de sessão do servidor.
use std::net::SocketAddr;
use std::sync::{Arc, Weak};

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono_tz::Tz;
use hangar_scheduled_tasks::{
    DispatchHandle, Dispatcher, Engine, Error, Host, HostFuture, Outcome, Status, Store,
    SystemClock, Target, Task, TaskInput,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::routes::AppState;
use crate::session_open::{OpenCaller, SessionOpenRequest};

#[derive(Default)]
pub struct Service {
    engine: OnceCell<Result<Arc<Engine>, Error>>,
    dispatch: std::sync::OnceLock<DispatchHandle>,
}
impl Service {
    /// Costura para servidor isolado, sem inicializar produtores, sessões ou hooks da máquina.
    pub fn with_engine(engine: Arc<Engine>, dispatch: DispatchHandle) -> Self {
        let service = Self::from_engine(engine);
        let _ = service.dispatch.set(dispatch);
        service
    }
    pub fn from_engine(engine: Arc<Engine>) -> Self {
        Self {
            engine: OnceCell::new_with(Some(Ok(engine))),
            dispatch: std::sync::OnceLock::new(),
        }
    }

    async fn engine(&self, state: &Arc<AppState>) -> Result<Arc<Engine>, Error> {
        self.engine
            .get_or_init(|| async {
                let home = state.accounts.env.home.clone();
                let (store, timezone) = tokio::task::spawn_blocking(move || {
                    let timezone = iana_time_zone::get_timezone()
                        .map_err(|_| Error::unavailable())?
                        .parse::<Tz>()
                        .map_err(|_| Error::unavailable())?;
                    Ok::<_, Error>((
                        Store::open(&home.join(".hangar/scheduled-tasks.sqlite3"))?,
                        timezone,
                    ))
                })
                .await
                .map_err(|_| Error::unavailable())??;
                Ok(Arc::new(Engine::new(
                    Arc::new(store),
                    timezone,
                    Arc::new(SessionHost(Arc::downgrade(state))),
                    Arc::new(SystemClock),
                )))
            })
            .await
            .clone()
    }
}

pub(crate) fn spawn(
    state: Arc<AppState>,
    restoration: crate::startup::Readiness,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_after_restoration(state, restoration))
}

/// Só publica o despachante depois do restauro e da preparação efetiva dos provedores.
pub async fn run_after_restoration(state: Arc<AppState>, restoration: crate::startup::Readiness) {
    state.startup.wait().await;
    restoration.wait().await;
    let engine = match state.scheduled_tasks.engine(&state).await {
        Ok(engine) => engine,
        Err(error) => {
            tracing::warn!(code = error.code, "agendador indisponível");
            return;
        }
    };
    if let Err(error) = engine.store.recover() {
        tracing::warn!(code = error.code, "recuperação do agendador falhou");
        return;
    }
    let automatic = match state
        .settings_snapshot()
        .and_then(|s| s.value("CP_AUTOMACOES"))
    {
        Ok(value) => value.is_none_or(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        }),
        Err(_) => {
            tracing::warn!(
                code = "scheduled_automation_config_unavailable",
                "agendamento automático sem configuração; execução manual preservada"
            );
            false
        }
    };
    let (handle, dispatcher) = Dispatcher::new(engine, automatic);
    if state.scheduled_tasks.dispatch.set(handle).is_err() {
        return;
    }
    dispatcher.run().await;
}

struct SessionHost(Weak<AppState>);
impl Host for SessionHost {
    fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>> {
        Box::pin(async move {
            let state = self.0.upgrade().ok_or_else(Error::unavailable)?;
            match target {
                Target::New {
                    launch,
                    worktree,
                    base,
                } => {
                    validate_launch(&launch)?;
                    Ok(Target::New {
                        launch,
                        worktree,
                        base,
                    })
                }
                Target::Session { name, session_key } => {
                    if name.is_empty()
                        || name.len() > 64
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    {
                        return Err(Error::invalid("Nome de sessão inválido."));
                    }
                    let home = state.accounts.env.home.clone();
                    let lookup = name.clone();
                    let recorded = tokio::task::spawn_blocking(move || {
                        crate::session_open::record::live(&home, &lookup)
                    })
                    .await
                    .map_err(|_| Error::unavailable())?
                    .map_err(|_| Error::unavailable())?;
                    let Some(runtime) = state.state.runtime.get() else {
                        return Err(if recorded.is_some() {
                            Error::unavailable()
                        } else {
                            missing_session()
                        });
                    };
                    let route = if let Some(mark) = runtime.asleep(&name).await {
                        crate::session_write::Write::Asleep(mark.key)
                    } else {
                        crate::session_write::route_write(
                            runtime,
                            &name,
                            crate::session_write::WriteRoute::Input,
                            state.write_gate_wait,
                        )
                        .await
                    };
                    let key = match route {
                        crate::session_write::Write::Rust(_pass, target) => target.key,
                        crate::session_write::Write::Asleep(key)
                        | crate::session_write::Write::Opening(key) => key,
                        crate::session_write::Write::Busy => return Err(changed_life()),
                        crate::session_write::Write::Failed(_) => return Err(Error::unavailable()),
                        _ => return Err(missing_session()),
                    };
                    if session_key
                        .as_ref()
                        .is_some_and(|expected| expected != &key)
                        || recorded.as_ref().is_some_and(|spec| spec.key != key)
                    {
                        return Err(changed_life());
                    }
                    Ok(Target::Session {
                        name,
                        session_key: Some(key),
                    })
                }
            }
        })
    }
    fn dispatch(&self, attempt: hangar_scheduled_tasks::Attempt) -> HostFuture<'_, Outcome> {
        Box::pin(async move {
            let Some(state) = self.0.upgrade() else {
                return failed("O servidor está encerrando.", false, None);
            };
            match &attempt.task.input.target {
                Target::Session {
                    name,
                    session_key: Some(key),
                } => {
                    let (status, body) = crate::session_write::input::write_bound_input(
                        &state,
                        name,
                        key,
                        &attempt.task.input.prompt,
                    )
                    .await;
                    if body["uncertain"] == true {
                        failed(
                            "A entrada foi conservada, mas a entrega não foi confirmada. Confira a conversa antes de repetir.",
                            true,
                            Some(name.clone()),
                        )
                    } else if status.is_success() {
                        Outcome {
                            status: Status::Sent,
                            error: None,
                            session: Some(name.clone()),
                        }
                    } else {
                        failed(
                            body["detail"]["msg"]
                                .as_str()
                                .unwrap_or("Não foi possível confirmar o envio para a sessão."),
                            status.is_server_error(),
                            Some(name.clone()),
                        )
                    }
                }
                Target::Session { .. } => {
                    failed("A identidade da sessão não foi vinculada.", false, None)
                }
                Target::New {
                    launch,
                    worktree,
                    base,
                } => {
                    let mut request = match validate_launch(launch) {
                        Ok(request) => request,
                        Err(error) => return failed(&error.message, false, None),
                    };
                    request.name = Some(attempt.session_name.clone());
                    request.initial_prompt = Some(attempt.task.input.prompt.clone());
                    request.remember_provider = false;
                    request.take_reserved_name = false;
                    if *worktree {
                        request.branch = Some(attempt.session_name.clone());
                        request.new_branch = true;
                        request.base = base.clone();
                    }
                    let host = crate::session_open::server::ServerHost::new(state.clone()).await;
                    match crate::session_open::server::decide(
                        &state,
                        &host,
                        request,
                        OpenCaller::Owner,
                    )
                    .await
                    {
                        crate::session_open::server::Decision::Opened(opened) => {
                            let name = opened.body["name"]
                                .as_str()
                                .unwrap_or(&attempt.session_name)
                                .to_owned();
                            if opened.body["avisos"]
                                .as_array()
                                .is_some_and(|warnings| !warnings.is_empty())
                            {
                                failed(
                                    "A conversa foi criada com um aviso. Confira se recebeu o pedido antes de repetir.",
                                    true,
                                    Some(name),
                                )
                            } else {
                                Outcome {
                                    status: Status::Sent,
                                    error: None,
                                    session: Some(name),
                                }
                            }
                        }
                        crate::session_open::server::Decision::Refused(error) => {
                            failed(&error.msg, error.status >= 500, None)
                        }
                        crate::session_open::server::Decision::NotOurs => {
                            failed("Este destino precisa do lançador Rust.", false, None)
                        }
                    }
                }
            }
        })
    }
}

fn validate_launch(launch: &Value) -> Result<SessionOpenRequest, Error> {
    let Some(object) = launch.as_object() else {
        return Err(Error::invalid("Opções de abertura inválidas."));
    };
    let allowed = [
        "cwd",
        "provider",
        "config_dir",
        "codex_account",
        "engine",
        "engine_account",
        "model",
        "effort",
        "service_tier",
        "permission_mode",
        "subagent_model",
        "claude_customizations",
        "jev",
        "omp_profile",
        "read_only",
        "headless",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Error::invalid(
            "As opções contêm campos internos ou segredos.",
        ));
    }
    SessionOpenRequest::parse(
        &serde_json::to_vec(launch).map_err(|_| Error::invalid("Opções inválidas."))?,
    )
    .map_err(|_| Error::invalid("As opções da nova conversa são inválidas."))
}
fn failed(message: &str, uncertain: bool, session: Option<String>) -> Outcome {
    // O resultado não guarda a saída bruta do harness, que pode conter credenciais.
    let message = crate::diag_report::Scrubber::new(None, None)
        .scrub(message)
        .into_owned();
    Outcome {
        status: if uncertain {
            Status::Uncertain
        } else {
            Status::Failed
        },
        error: Some(message),
        session,
    }
}
fn missing_session() -> Error {
    Error {
        status: 404,
        code: "scheduled_session_missing",
        message: "A sessão de destino não existe mais.".into(),
    }
}
fn changed_life() -> Error {
    Error {
        status: 409,
        code: "scheduled_session_changed",
        message: "A sessão de destino foi encerrada ou recriada. Escolha o destino novamente."
            .into(),
    }
}
fn error(error: Error) -> Response {
    hangar_core::http::json_response(
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        json!({"detail":{"code":error.code,"msg":error.message}}),
    )
}
fn task_value(task: Task) -> Value {
    let mut value = serde_json::to_value(task).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("attempt");
    }
    value
}

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route(
            "/api/scheduled-tasks",
            axum::routing::get(collection)
                .post(collection)
                .options(collection),
        )
        .route(
            "/api/scheduled-tasks/{id}",
            axum::routing::put(item).delete(item).options(item),
        )
        .route(
            "/api/scheduled-tasks/{id}/enabled",
            axum::routing::patch(enabled).options(enabled),
        )
        .route(
            "/api/scheduled-tasks/{id}/run",
            axum::routing::post(run).options(run),
        )
}

pub async fn collection(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    serve(state, peer, None, None, req).await
}
pub async fn item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    serve(state, peer, Some(id), None, req).await
}
pub async fn enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    serve(state, peer, Some(id), Some("enabled"), req).await
}
pub async fn run(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    serve(state, peer, Some(id), Some("run"), req).await
}
async fn serve(
    state: Arc<AppState>,
    peer: SocketAddr,
    id: Option<String>,
    action: Option<&str>,
    req: Request,
) -> Response {
    let headers = req.headers().clone();
    let method = req.method().clone();
    if let Some(response) = crate::http_cors::preflight(&method, &headers) {
        return response;
    }
    if crate::routes::on_guest_port(&req)
        || matches!(
            crate::share::caller::caller(&req),
            Some(crate::share::caller::Caller::Guest(_))
        )
    {
        let mut response = error(Error {
            status: 403,
            code: "scheduled_owner_only",
            message: "Só o dono pode acessar as tarefas agendadas.".into(),
        });
        crate::routes::cors(&headers, response.headers_mut());
        return response;
    }
    if let Err(mut response) = crate::routes::owner_gate(&state, peer, &req).await {
        crate::routes::cors(&headers, response.headers_mut());
        return response;
    }
    let mut response = match inner(state, id, action, req).await {
        Ok((status, value)) => hangar_core::http::json_response(status, value),
        Err(cause) => error(cause),
    };
    crate::routes::cors(&headers, response.headers_mut());
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}
async fn inner(
    state: Arc<AppState>,
    id: Option<String>,
    action: Option<&str>,
    req: Request,
) -> Result<(StatusCode, Value), Error> {
    let engine = state.scheduled_tasks.engine(&state).await?;
    let method = req.method().clone();
    let revision = req.uri().query().and_then(|query| {
        form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "revision")
            .and_then(|(_, value)| value.parse::<u64>().ok())
    });
    if matches!(method, Method::GET | Method::HEAD) && id.is_none() {
        return Ok((
            StatusCode::OK,
            json!({"tasks":engine.store.list()?.into_iter().map(task_value).collect::<Vec<_>>(),"server_time":engine.now().to_rfc3339(),"timezone":engine.timezone.name()}),
        ));
    }
    if method == Method::DELETE {
        engine.store.delete(
            id.as_deref()
                .ok_or_else(|| Error::invalid("Identificador ausente."))?,
            revision.ok_or_else(|| Error::invalid("Informe a revisão da tarefa."))?,
        )?;
        return Ok((StatusCode::OK, json!({"ok":true})));
    }
    if action == Some("run") {
        let bytes = to_bytes(req.into_body(), 1024)
            .await
            .map_err(|_| Error::invalid("O corpo desta ação deve estar vazio."))?;
        if !bytes.is_empty() && serde_json::from_slice::<Value>(&bytes).ok() != Some(json!({})) {
            return Err(Error::invalid("O corpo desta ação deve estar vazio."));
        }
        state
            .scheduled_tasks
            .dispatch
            .get()
            .ok_or_else(Error::unavailable)?
            .run_now(
                id.as_deref()
                    .ok_or_else(|| Error::invalid("Identificador ausente."))?,
            )?;
        return Ok((StatusCode::ACCEPTED, json!({"ok":true})));
    }
    let bytes = to_bytes(req.into_body(), 192 * 1024)
        .await
        .map_err(|_| Error::invalid("O corpo da tarefa ultrapassa 192 KiB."))?;
    let mut body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::invalid("Pedido em formato inválido."))?;
    if action == Some("enabled") {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Enabled {
            enabled: bool,
            revision: u64,
        }
        let patch: Enabled = serde_json::from_value(body)
            .map_err(|_| Error::invalid("Informe enabled e revision."))?;
        return Ok((
            StatusCode::OK,
            task_value(engine.store.set_enabled(
                id.as_deref().unwrap_or_default(),
                patch.enabled,
                patch.revision,
                engine.now(),
                &engine.timezone,
            )?),
        ));
    }
    let revision = if method == Method::PUT {
        body.as_object_mut()
            .and_then(|b| b.remove("revision"))
            .and_then(|r| r.as_u64())
            .ok_or_else(|| Error::invalid("Informe a revisão da tarefa."))?
    } else {
        0
    };
    let input: TaskInput = serde_json::from_value(body)
        .map_err(|_| Error::invalid("Informe todos os campos da tarefa no formato esperado."))?;
    let task = if method == Method::POST {
        engine.create(input).await?
    } else {
        engine
            .update(id.as_deref().unwrap_or_default(), input, revision)
            .await?
    };
    Ok((
        if method == Method::POST {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        task_value(task),
    ))
}
