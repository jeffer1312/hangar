//! Portas dos comandos de execução da sessão.
use crate::routes::AppState;
use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
const ROUTES: &[(&str, &str)] = &[
    ("/api/sessions/{name}/runners", "GET"),
    ("/api/sessions/{name}/runners/custom", "POST"),
    ("/api/sessions/{name}/run", "POST"),
    ("/api/sessions/{name}/run/stop", "POST"),
    ("/api/sessions/{name}/run/pane", "GET"),
];
pub fn matches(path: &str) -> bool {
    path.strip_prefix("/api/sessions/")
        .and_then(|s| s.split_once('/'))
        .is_some_and(|(_, op)| {
            // Com barra final também: o contrato da rota responde o redirecionamento.
            matches!(
                op.strip_suffix('/').unwrap_or(op),
                "runners" | "runners/custom" | "run" | "run/stop" | "run/pane"
            )
        })
}
fn error(e: hangar_projects::Error) -> Response {
    let detail = match e.code {
        Some(code) => json!({"code":code,"params":{},"msg":e.detail}),
        None => json!(e.detail),
    };
    hangar_core::http::json_response(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        json!({"detail":detail}),
    )
}
async fn service(st: &AppState) -> Result<hangar_projects::Service, Response> {
    let env = &st.accounts.env;
    let settings = st
        .settings_snapshot()
        .map_err(|e| error(hangar_projects::Error::new(503, e.to_string())))?;
    let projects = settings
        .value("CP_PROJECTS_DIR")
        .map_err(|e| error(hangar_projects::Error::new(503, e.to_string())))?
        .map(PathBuf::from)
        .unwrap_or_else(|| env.claude_base.join("projects"));
    Ok(hangar_projects::Service {
        preferences: projects
            .parent()
            .unwrap_or(&projects)
            .join(".hangar-runner.json"),
        program: vec![st.list.env().capture_program.to_string_lossy().into_owned()],
        scope: tokio::task::spawn_blocking(crate::session_open::terminal::systemd_scope)
            .await
            .unwrap_or_default(),
        env: env
            .base
            .iter()
            .filter(|(k, _)| !crate::session_open::terminal::SERVER_SECRETS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
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

    let Some(name) = session else {
        return unavailable();
    };
    use crate::shortcuts::http::{blocked, rows};
    let find = |rows: &[hangar_api::session::SessionRow]| rows.iter().find(|r| r.name == name).cloned();
    let discovered = match rows(&st).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let mut row = find(&discovered);
    let ingress = if method != "GET" && method != "HEAD" {
        match blocked(&st, discovered, &name).await {
            Ok(false) => {}
            Ok(true) => return busy(),
            Err(r) => return r,
        }
        let Some(runtime) = st.state.runtime.get() else {
            return unavailable();
        };
        let pass = match runtime.ingress().enter(&name, st.write_gate_wait).await {
            Ok(p) => p,
            Err(_) => return busy(),
        };
        let current = match rows(&st).await {
            Ok(r) => r,
            Err(r) => return r,
        };
        let next = find(&current);
        if !crate::list::local_transfers::same_life(row.as_ref(), next.as_ref()) {
            return busy();
        }
        match blocked(&st, current, &name).await {
            Ok(false) => {}
            Ok(true) => return busy(),
            Err(r) => return r,
        }
        row = next;
        Some(pass)
    } else {
        None
    };
    let Some(cwd) = row.and_then(|r| r.cwd).filter(|s| !s.is_empty()) else {
        return error(hangar_projects::Error::coded(
            404,
            "erro_sessao_inexistente",
            "sessao nao encontrada",
        ));
    };
    let op = path
        .strip_prefix("/api/sessions/")
        .and_then(|p| p.split_once('/'))
        .map_or_else(String::new, |(_, op)| op.to_owned());
    tokio::spawn(async move {
        let _ingress = ingress;
        match execute(service, &op, cwd, value).await {
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
    service: hangar_projects::Service,
    op: &str,
    cwd: String,
    value: Value,
) -> Result<Value, hangar_projects::Error> {
    // Arquivos do projeto podem estar num disco lento: a leitura sai das threads do runtime.
    let blocking = |f: Box<dyn FnOnce(&hangar_projects::Service, &str) -> Result<Value, hangar_projects::Error> + Send>| {
        let (service, cwd) = (service.clone(), cwd.clone());
        async move {
            tokio::task::spawn_blocking(move || f(&service, &cwd))
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(error = %e, code = "runner_read_failed", "leitura dos comandos da sessão falhou");
                    Err(hangar_projects::Error::coded(500, "runner_read_failed", "leitura dos comandos falhou"))
                })
        }
    };
    match op {
        "runners" => {
            let mut out = blocking(Box::new(|s, cwd| {
                Ok(json!({"detected":s.detect_runners(cwd),"custom":s.custom_commands(cwd),"remembered":s.remembered(cwd)}))
            }))
            .await?;
            out["running"] = json!(service.run_status(&cwd).await);
            Ok(out)
        }
        "runners/custom" => {
            let commands = value["commands"].clone();
            blocking(Box::new(move |s, cwd| Ok(json!(s.set_custom_commands(cwd, commands)?)))).await
        }
        "run" => Ok(json!(
            service
                .start_run(
                    &cwd,
                    value["command"].as_str().ok_or_else(|| hangar_projects::Error::coded(
                        422,
                        "erro_corpo_invalido",
                        "command é obrigatório"
                    ))?
                )
                .await?
        )),
        "run/stop" => {
            service.stop_run(&cwd).await?;
            Ok(json!({"ok":true}))
        }
        _ => Ok(json!({"pane":service.run_pane(&cwd).await})),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runner_routes_with_a_trailing_slash_stay_in_rust() {
        assert!(super::matches("/api/sessions/demo/runners/"));
        assert!(super::matches("/api/sessions/demo/run/stop/"));
        assert!(!super::matches("/api/sessions/demo/runners/x"));
    }
}
