//! Liga os lançadores ao contexto e à porta de escrita da sessão, sem chamar o Python.
use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::future::BoxFuture;
use hangar_core::http::{detail, json_response};
use hangar_session_tools::{editor, external, shell};
use serde_json::json;

use crate::{
    routes::AppState,
    session_open::terminal::{Mux, ProcessMux},
};

const ROUTES: &[(&str, &str)] = &[
    ("/api/sessions/{name}/shell", "POST"),
    ("/api/sessions/{name}/open-editor", "POST"),
    ("/api/sessions/{name}/open-terminal", "POST"),
];

struct ShellMux(ProcessMux);
impl shell::Mux for ShellMux {
    fn run(&self, args: Vec<String>) -> BoxFuture<'_, shell::Output> {
        Box::pin(async move {
            let out = self.0.run(args).await;
            shell::Output {
                code: out.code,
                stdout: out.stdout,
                stderr: out.stderr,
            }
        })
    }
    fn scope(&self) -> BoxFuture<'_, Vec<String>> {
        self.0.scope()
    }
    fn warning(&self, message: &str) {
        tracing::warn!("{message}");
        let event = if message.contains("diretório") && message.contains("não saiu") {
            "mux.shell_kill_unconfirmed"
        } else if message.contains("diretório") {
            "mux.shell_cwd_unreadable"
        } else {
            "mux.scope_perdido"
        };
        crate::diag::record(
            self.0.diary.clone(),
            vec![json!({"evento":event,"nivel":"aviso"})],
            "shell",
        );
    }
}

fn unavailable() -> Response {
    detail(
        StatusCode::SERVICE_UNAVAILABLE,
        "session_context_unavailable",
        "contexto da sessão indisponível",
        json!({}),
    )
}
fn busy() -> Response {
    json_response(StatusCode::CONFLICT, hangar_core::http::busy_body())
}

pub(crate) async fn public(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    let headers = req.headers().clone();
    let mut response = inner(st, peer, name, req).await;
    crate::http_cors::apply(&headers, &mut response);
    response
}

async fn inner(st: Arc<AppState>, peer: SocketAddr, name: String, req: Request) -> Response {
    let operation = req.uri().path().rsplit('/').next().unwrap_or("").to_owned();
    let path = percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy();
    let decision = hangar_core::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    let forward = crate::routes::forward_for(&st, peer, &req);
    if let Some(response) = crate::http_route_response::response(
        &decision,
        req.method(),
        req.uri(),
        req.headers(),
        forward.https,
        "",
    ) {
        return response;
    }
    let (_, caller) = match crate::routes::who(&st, peer, &req).await {
        Ok(value) => value,
        Err(error) => return crate::auth::unavailable(error),
    };
    match caller {
        Some(crate::share::caller::Caller::Owner) => {}
        Some(crate::share::caller::Caller::Guest(grant))
            if operation == "shell"
                && crate::share::caller::guest_allowed(req.method(), req.uri().path(), &grant) => {}
        Some(crate::share::caller::Caller::Guest(_)) => {
            return detail(
                StatusCode::FORBIDDEN,
                "erro_fora_do_convite",
                "fora da sessão compartilhada",
                json!({}),
            );
        }
        None => {
            if let Err(response) = crate::routes::owner_gate(&st, peer, &req).await {
                return response;
            }
        }
    }
    let rows = match crate::shortcuts::http::rows(&st).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let before = rows.iter().find(|row| row.name == name).cloned();
    match crate::shortcuts::http::blocked(&st, rows, &name).await {
        Ok(true) => return busy(),
        Ok(false) => {}
        Err(response) => return response,
    }
    let Some(runtime) = st.state.runtime.get() else {
        return unavailable();
    };
    let pass = match runtime.ingress().enter(&name, st.write_gate_wait).await {
        Ok(pass) => pass,
        Err(_) => return busy(),
    };
    let rows = match crate::shortcuts::http::rows(&st).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let row = rows.iter().find(|row| row.name == name).cloned();
    if !crate::list::local_transfers::same_life(before.as_ref(), row.as_ref()) {
        return busy();
    }
    match crate::shortcuts::http::blocked(&st, rows, &name).await {
        Ok(true) => return busy(),
        Ok(false) => {}
        Err(response) => return response,
    }
    // A desconexão de quem clicou não pode soltar a guarda com o processo ainda sendo aberto.
    tokio::spawn(async move {
        let _pass = pass;
        let mux = ProcessMux {
            diary: crate::diag::diary_dir_in(&st.accounts.env),
            ..ProcessMux::default()
        };
        let env = st
            .accounts
            .env
            .base
            .iter()
            .filter(|(key, _)| {
                !hangar_core::terminal_process::PRIVATE_ENV_KEYS
                    .iter()
                    .any(|private| key.eq_ignore_ascii_case(private))
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let program = st.list.env().capture_program.to_string_lossy().into_owned();
        let result = if operation == "open-terminal" {
            let found = mux
                .run(vec![
                    program,
                    "has-session".into(),
                    "-t".into(),
                    format!("={name}"),
                ])
                .await;
            if found.code != Some(0) {
                return missing("sessao nao existe");
            }
            external::open(&name, &env, mux.scope().await).await
        } else {
            let Some(row) = row else {
                return missing("sessao nao existe");
            };
            let cwd = row.cwd.filter(|cwd| !cwd.is_empty());
            if operation == "shell" {
                let cwd =
                    cwd.unwrap_or_else(|| st.accounts.env.home.to_string_lossy().into_owned());
                shell::Service {
                    mux: Arc::new(ShellMux(mux)),
                    program,
                    windows: cfg!(windows),
                }
                .open(&name, &cwd)
                .await
            } else {
                let Some(cwd) = cwd else {
                    return missing("sessao nao encontrada");
                };
                let fallback = match st
                    .settings_snapshot()
                    .and_then(|snapshot| snapshot.value("CP_EDITOR"))
                {
                    Ok(value) => value.unwrap_or_else(|| "code".into()),
                    Err(_) => return unavailable(),
                };
                let path = st.accounts.env.claude_base.join("runtime-config.json");
                let binary =
                    match tokio::task::spawn_blocking(move || editor::configured(&path, &fallback))
                        .await
                    {
                        Ok(binary) => binary,
                        Err(_) => return unavailable(),
                    };
                tracing::info!(session = %name, bin = %binary, cwd = %cwd, "OPEN-EDITOR");
                editor::open(&binary, &cwd, &env).await
            }
        };
        match result {
            Ok(value) => json_response(StatusCode::OK, value),
            Err(error) if error.code.is_empty() => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
            Err(error) => detail(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                error.code,
                &error.msg,
                error.params,
            ),
        }
    })
    .await
    .unwrap_or_else(|_| unavailable())
}

fn missing(message: &str) -> Response {
    detail(
        StatusCode::NOT_FOUND,
        "erro_sessao_inexistente",
        message,
        json!({}),
    )
}
