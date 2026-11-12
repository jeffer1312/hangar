//! Portas do launcher de projetos e dos comandos da sessão.
use crate::routes::AppState;
use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
const ROUTES: &[(&str, &str)] = &[
    ("/api/projects", "GET"),
    ("/api/projects", "POST"),
    ("/api/projects/{name}", "DELETE"),
    ("/api/projects/{name}/start", "POST"),
    ("/api/projects/{name}/stop", "POST"),
    ("/api/projects/{name}/pane", "GET"),
    ("/api/sessions/{name}/runners", "GET"),
    ("/api/sessions/{name}/runners/custom", "POST"),
    ("/api/sessions/{name}/run", "POST"),
    ("/api/sessions/{name}/run/stop", "POST"),
    ("/api/sessions/{name}/run/pane", "GET"),
];
pub fn matches(path: &str) -> bool {
    path == "/api/projects"
        || path.starts_with("/api/projects/")
        || path
            .strip_prefix("/api/sessions/")
            .and_then(|s| s.split_once('/'))
            .is_some_and(|(_, op)| {
                matches!(
                    op,
                    "runners" | "runners/custom" | "run" | "run/stop" | "run/pane"
                )
            })
}
fn error(e: hangar_projects::Error) -> Response {
    hangar_core::http::json_response(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        json!({"detail":e.detail}),
    )
}
async fn service(st: &AppState) -> Result<hangar_projects::Service, Response> {
    let env = &st.accounts.env;
    let backend = crate::session_open::cli::backend_dir_for_home(&env.home);
    let config = hangar_core::user_data::path(&env.home, backend.as_deref(), "projects.json")
        .map_err(|e| error(hangar_projects::Error::new(500, e.to_string())))?
        .unwrap_or_else(|| env.home.join(".hangar/config/projects.json"));
    let settings = st
        .settings_snapshot()
        .map_err(|e| error(hangar_projects::Error::new(503, e.to_string())))?;
    let projects = settings
        .value("CP_PROJECTS_DIR")
        .map_err(|e| error(hangar_projects::Error::new(503, e.to_string())))?
        .map(PathBuf::from)
        .unwrap_or_else(|| env.claude_base.join("projects"));
    Ok(hangar_projects::Service {
        config,
        preferences: projects
            .parent()
            .unwrap_or(&projects)
            .join(".hangar-runner.json"),
        program: vec![st.list.env().capture_program.to_string_lossy().into_owned()],
        scope: tokio::task::spawn_blocking(crate::session_open::terminal::systemd_scope)
            .await
            .unwrap_or_default(),
        env: env.base.clone(),
        windows: cfg!(windows),
    })
}
pub async fn public(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let headers = req.headers().clone();
    let method = req.method().clone();
    let mut response = inner(st, peer, req).await;
    crate::http_cors::apply(&headers, &mut response);
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}
async fn inner(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let path = percent_encoding::percent_decode_str(req.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    let method = req.method().clone();
    let decision = crate::http_contract::resolve(ROUTES, method.as_str(), &path);
    let forward = crate::routes::forward_for(&st, peer, &req);
    if let Some(r) = crate::http_route_response::response(
        &decision,
        &method,
        req.uri(),
        req.headers(),
        forward.https,
        "",
    ) {
        return r;
    }
    let session = path
        .strip_prefix("/api/sessions/")
        .and_then(|p| p.split_once('/'))
        .map(|(name, _)| name.to_owned());
    if let Some(name) = &session {
        let (_, caller) = match crate::routes::who(&st, peer, &req).await {
            Ok(v) => v,
            Err(e) => return crate::auth::unavailable(e),
        };
        if !caller.as_ref().is_some_and(|c| c.can(name, &method)) {
            return match crate::routes::owner_gate(&st, peer, &req).await {
                Err(r) => r,
                Ok(_) => hangar_core::http::json_response(
                    StatusCode::FORBIDDEN,
                    json!({"detail":"fora da sessao compartilhada"}),
                ),
            };
        }
    } else if let Err(r) = crate::routes::owner_gate(&st, peer, &req).await {
        return r;
    }

    let service = match service(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let bytes = match crate::http_body_limit::read(req.into_body()).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "There was an error parsing the body",
            )
                .into_response();
        }
    };
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => {
                return hangar_core::http::json_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({"detail":"corpo JSON inválido"}),
                );
            }
        }
    };

    let ingress = if method != "GET" && method != "HEAD" {
        if let Some(name) = &session {
            let rows = match st.list.discover(None).await {
                Ok(r) => r,
                Err(_) => return unavailable(),
            };
            match crate::list::local_transfers::transfer_active(
                st.list.env(),
                name,
                rows.as_ref().clone(),
            )
            .await
            {
                Ok(false) => {}
                Ok(true) => return busy(),
                Err(_) => return unavailable(),
            }
            let Some(runtime) = st.state.runtime.get() else {
                return unavailable();
            };
            let pass = match runtime.ingress().enter(name, st.write_gate_wait).await {
                Ok(p) => p,
                Err(_) => return busy(),
            };
            let current = match st.list.discover(None).await {
                Ok(r) => r,
                Err(_) => return unavailable(),
            };
            if !crate::list::local_transfers::same_life(
                rows.iter().find(|r| &r.name == name),
                current.iter().find(|r| &r.name == name),
            ) {
                return busy();
            }
            match crate::list::local_transfers::transfer_active(
                st.list.env(),
                name,
                current.as_ref().clone(),
            )
            .await
            {
                Ok(false) => {}
                Ok(true) => return busy(),
                Err(_) => return unavailable(),
            }
            Some(pass)
        } else {
            None
        }
    } else {
        None
    };
    tokio::spawn(async move {
        let _ingress = ingress;
        match execute(&st, &service, &path, method.as_str(), value).await {
            Ok(v) => hangar_core::http::json_response(StatusCode::OK, v),
            Err(e) => error(e),
        }
    })
    .await
    .unwrap_or_else(|_| unavailable())
}
fn unavailable() -> Response {
    error(hangar_projects::Error::new(
        503,
        "contexto da sessão indisponível",
    ))
}
fn busy() -> Response {
    crate::session_write::json_response(StatusCode::CONFLICT, crate::session_write::busy_body())
}
async fn execute(
    st: &AppState,
    service: &hangar_projects::Service,
    path: &str,
    method: &str,
    value: Value,
) -> Result<Value, hangar_projects::Error> {
    if path == "/api/projects" {
        if method == "GET" || method == "HEAD" {
            return Ok(json!(service.list().await?));
        }
        let field = |name: &str| {
            value[name]
                .as_str()
                .ok_or_else(|| hangar_projects::Error::new(422, format!("{name} é obrigatório")))
        };
        let port = if value["port"].is_null() {
            None
        } else {
            Some(
                value["port"]
                    .as_i64()
                    .or_else(|| value["port"].as_str().and_then(|s| s.parse().ok()))
                    .ok_or_else(|| hangar_projects::Error::new(422, "port deve ser inteiro"))?,
            )
        };
        return Ok(json!(
            service
                .upsert(
                    field("name")?,
                    field("cwd")?,
                    field("command")?,
                    port,
                    value["stop_command"].as_str()
                )
                .await?
        ));
    }
    if let Some(rest) = path.strip_prefix("/api/projects/") {
        let (name, op) = rest.split_once('/').unwrap_or((rest, ""));
        return match op {
            "start" => Ok(json!(service.start(name).await?)),
            "stop" => {
                service.stop(name).await?;
                Ok(json!({"ok":true}))
            }
            "pane" => Ok(json!({"pane":service.pane(name).await?})),
            _ => {
                service.remove(name).await?;
                Ok(json!({"ok":true}))
            }
        };
    }
    let (name, op) = path
        .strip_prefix("/api/sessions/")
        .unwrap()
        .split_once('/')
        .unwrap();
    let rows = st
        .list
        .discover(None)
        .await
        .map_err(|_| hangar_projects::Error::new(503, "contexto da sessão indisponível"))?;
    let cwd = rows
        .iter()
        .find(|r| r.name == name)
        .and_then(|r| r.cwd.as_deref())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| hangar_projects::Error::new(404, "sessao nao encontrada"))?;
    match op {
        "runners" => Ok(
            json!({"detected":service.detect_runners(cwd),"custom":service.custom_commands(cwd),"remembered":service.remembered(cwd),"running":service.run_status(cwd).await}),
        ),
        "runners/custom" => Ok(json!(
            service.set_custom_commands(cwd, value["commands"].clone())?
        )),
        "run" => Ok(json!(
            service
                .start_run(
                    cwd,
                    value["command"]
                        .as_str()
                        .ok_or_else(|| hangar_projects::Error::new(422, "command é obrigatório"))?
                )
                .await?
        )),
        "run/stop" => {
            service.stop_run(cwd).await?;
            Ok(json!({"ok":true}))
        }
        _ => Ok(json!({"pane":service.run_pane(cwd).await})),
    }
}
