//! Rotas do Computer Use (`crates/hangar-computer-control-setup`): só o dono, conferido aqui; o
//! convidado recebe 403. `/install` só recusa (a instalação é sozinha).
use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use hangar_computer_control_setup as setup;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::routes::AppState;

const BODY_LIMIT: usize = 64 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestHostBody {
    host: String,
    #[serde(default)]
    proxy_command: String,
}

pub(crate) fn matches(method: &Method, path: &str) -> bool {
    matches!(
        (method.as_str(), normalized(path).as_str()),
        ("GET", "/api/computer-control" | "/api/computer-control/windows-setup")
            | ("PUT", "/api/computer-control")
            | ("POST", "/api/computer-control/targets" | "/api/computer-control/test-host" | "/api/computer-control/install")
    )
}

/// O caminho como o uvicorn o veria (`%2F` decodificado, `//` juntos): variação de escrita não pode
/// levar ao Python, que não confere o dono.
fn normalized(path: &str) -> String {
    let decoded = crate::workspace_http::decoded(path);
    let mut out = String::with_capacity(decoded.len());
    for c in decoded.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    out
}

pub(crate) async fn route(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = reply(st, peer, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    response
}

async fn reply(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    match crate::routes::who(&st, peer, &req).await {
        Err(e) => return crate::auth::unavailable(e),
        Ok((_, Some(crate::share::caller::Caller::Guest(_)))) => {
            return hangar_core::http::detail(StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessão compartilhada", json!({}));
        }
        _ => {}
    }
    if let Err(response) = crate::routes::owner_gate(&st, peer, &req).await {
        return response;
    }
    let headers = req.headers().clone();
    let env = st.accounts.env.clone();
    let path = normalized(req.uri().path());
    let result = match path.as_str() {
        "/api/computer-control" if *req.method() == Method::PUT => {
            let Some(body) = body::<setup::SaveRequest>(req).await else {
                return crate::session_write::invalid_body(&headers);
            };
            let state = st.clone();
            blocking(&st, move |paths| {
                let config = || crate::config_sync::runtime::RuntimeConfig::from_state(&state).map_err(setup::Error::io);
                // O env do Jev relido sob a trava: o que a página Jev gravou no meio não volta atrás.
                let jev = || -> Result<_, setup::Error> { Ok(config()?.jev_env()) };
                setup::save(paths, &facts(&state)?, &config()?.jev_aux(), &body, jev)
            })
            .await
        }
        // App antigo ainda tem o botão Instalar: o hcu agora chega sozinho, nada é feito aqui.
        "/api/computer-control/install" => {
            return hangar_core::http::detail(StatusCode::GONE, "erro_computer_control_update_app",
                "Atualize o app: o Computer Use agora é instalado sozinho", json!({}));
        }
        "/api/computer-control" => blocking(&st, |_| Ok(())).await,
        "/api/computer-control/targets" => {
            let Some(body) = body::<setup::TargetRequest>(req).await.filter(setup::TargetRequest::valid) else {
                return crate::session_write::invalid_body(&headers);
            };
            let state = st.clone();
            blocking(&st, move |paths| {
                setup::create_target(paths, &body)?;
                // Alvo novo liga o MCP sozinho; falha do registro vai ao diário, não à resposta.
                crate::computer_control_install::register_in(&state, paths);
                Ok(())
            })
            .await
        }
        "/api/computer-control/test-host" => {
            let Some(body) = body::<TestHostBody>(req).await else {
                return crate::session_write::invalid_body(&headers);
            };
            match ssh_program(&env).await {
                Ok(ssh) => setup::test_host(&body.host, &body.proxy_command, ssh.as_deref(), &env.base).await,
                Err(e) => Err(e),
            }
        }
        _ => {
            // Vários `host`: vale o último, como no FastAPI.
            let host = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
                .filter(|(k, _)| k == "host").last().map(|(_, v)| v.into_owned()).unwrap_or_default();
            let paths = setup::Paths::new(env.home.clone(), Vec::new());
            match ssh_program(&env).await {
                Ok(ssh) => setup::windows_setup(&paths, &host, ssh.as_deref(), &env.base).await,
                Err(e) => Err(e),
            }
        }
    };
    match result {
        Ok(value) => hangar_core::http::json_response(StatusCode::OK, value),
        Err(e) => hangar_core::http::detail(
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), e.code, &e.msg, e.params),
    }
}

/// A escrita (se houver) e o estado inteiro, fora do executor: tudo é disco e uma sonda de 0,5 s.
async fn blocking(
    st: &Arc<AppState>,
    write: impl FnOnce(&setup::Paths) -> Result<(), setup::Error> + Send + 'static,
) -> Result<Value, setup::Error> {
    let st = st.clone();
    tokio::task::spawn_blocking(move || {
        let paths = paths(&st)?;
        write(&paths)?;
        setup::state(&paths, &facts(&st)?)
    })
    .await
    .map_err(setup::Error::io)?
}

/// O `ssh` pelo PATH do servidor, como o `shutil.which`; a busca olha disco, então fora do executor.
async fn ssh_program(env: &crate::accounts::environment::AccountEnvironment) -> Result<Option<std::path::PathBuf>, setup::Error> {
    let env = env.clone();
    tokio::task::spawn_blocking(move || crate::cli_probe::resolve_program("ssh", &env)).await.map_err(setup::Error::io)
}

/// A mesma lista de contas do `sync_jev`: as do catálogo, menos a pasta padrão.
pub(crate) fn paths(st: &AppState) -> Result<setup::Paths, setup::Error> {
    let env = &st.accounts.env;
    let default = crate::accounts::catalog::resolved(&env.home.join(".claude"));
    let accounts = st.accounts.claude_accounts().map_err(|e| setup::Error::io(e.message))?
        .into_iter()
        .map(|(_, account)| account.home)
        .filter(|home| crate::accounts::catalog::resolved(home) != default)
        .collect();
    Ok(setup::Paths::new(env.home.clone(), accounts))
}

fn facts(st: &AppState) -> Result<setup::Facts, setup::Error> {
    let config = crate::config_sync::runtime::RuntimeConfig::from_state(st).map_err(setup::Error::io)?;
    let env = &st.accounts.env;
    Ok(setup::Facts {
        jev: config.jev_env(),
        cliproxy_keys: crate::jev_aux::cliproxy_keys(&env.home),
        cliproxy_installed: crate::cli_probe::resolve_program("cli-proxy-api", env).is_some(),
    })
}

async fn body<T: DeserializeOwned>(req: Request) -> Option<T> {
    let bytes = axum::body::to_bytes(req.into_body(), BODY_LIMIT).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}
