//! Rotas de instalar, atualizar e conectar conta do CLIProxyAPI. O trabalho é do
//! `hangar-cliproxy-setup`; aqui ficam a rota, a senha de gerenciamento e a contagem de sessões.
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_cliproxy_setup::detect::{self, Install, Paths};
use hangar_cliproxy_setup::install::{self, Context, Installer};
use hangar_cliproxy_setup::login;
use hangar_cliproxy_setup::release::{Github, Platform};
use serde_json::{Value, json};

use super::{proxy, store};
use crate::accounts::environment::AccountEnvironment;
use crate::list::bridge::ListBridge;
use crate::routes::AppState;
use crate::session_write::json_response;

const RESET_PROBES: usize = 2;
const RESET_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Uma máquina, um proxy: a instalação em andamento é do processo inteiro.
static INSTALLER: LazyLock<Arc<Installer>> = LazyLock::new(|| Arc::new(Installer::default()));

fn coded(status: u16, code: &str, msg: &str, params: Value) -> Response {
    crate::provider_models::ModelError::coded(status, code, msg, params).response()
}

fn context(st: &AppState) -> Context {
    let env = st.accounts.env.clone();
    let reader = env.clone();
    Context {
        paths: Paths::new(&env.home),
        platform: Platform::local(),
        github: Github::default(),
        persist: Arc::new(move |key| proxy::set_management_key(&env, key).map_err(std::io::Error::other)),
        management_key: Arc::new(move || proxy::management_key(&reader)),
        before_swap: None,
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, fallback: T) -> T {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|error| {
        tracing::error!(%error, "cliproxy: leitura da instalação caiu; a tela recebe o valor de reserva");
        fallback
    })
}

/// O campo `install` do `GET /api/engines/cliproxy`.
pub async fn status(st: &AppState) -> Value {
    let paths = Paths::new(&st.accounts.env.home);
    let state = blocking(move || install::current(&paths), Install::Absent).await;
    let job = INSTALLER.job();
    // Com uma tarefa em curso a tela relê a cada segundo: a versão e a entrega, que perguntam ao proxy e ao systemd, ficam para depois.
    let quiet = job["done"] == false;
    let paths = Paths::new(&st.accounts.env.home);
    let (kind, version, handover) = match &state {
        Install::Absent => ("absent", None, Value::Null),
        Install::External { .. } if quiet => ("external", None, Value::Null),
        Install::External { .. } => {
            let key = proxy::management_key(&st.accounts.env);
            let version = install::running_tag(&paths, key.as_deref()).await;
            let handover = match install::handover(&paths).await {
                Ok(_) => json!({"ok": true}),
                Err(failure) => json!({"ok": false, "code": failure.code, "detail": failure.detail}),
            };
            ("external", version, handover)
        }
        Install::Managed { tag, .. } => ("managed", Some(tag.clone()), Value::Null),
    };
    let latest = match kind {
        "managed" | "external" => INSTALLER.latest(&Github::default()).await
            .inspect_err(|code| tracing::warn!(code, "cliproxy: não consegui ler a versão mais nova no GitHub"))
            .ok().map(|r| r.tag),
        _ => None,
    };
    let update_available = kind == "managed" && matches!((&version, &latest), (Some(v), Some(l)) if install::newer(l, v));
    // Quantas sessões a atualização ou a entrega cortariam: a confirmação diz o número.
    let sessions = match kind {
        "managed" | "external" => engine_sessions(&st.accounts.env, &st.list).await.ok(),
        _ => None,
    };
    let local = proxy::local(&st.accounts.env).ok().flatten().map(|local| local.base_url);
    let running = match local.clone() {
        Some(base) => blocking(move || detect::port_open(&base), false).await,
        None => false,
    };
    let accounts = match kind {
        "absent" => None,
        _ => connected(st).as_array().map(Vec::len),
    };
    json!({"state": kind, "version": version, "latest": latest, "update_available": update_available,
        "running": running, "base_url": local, "accounts": accounts, "sessions": sessions, "handover": handover, "job": job})
}

/// Contas logadas no proxy, de qualquer provedor, para o assistente mostrar antes de existir motor.
pub fn connected(st: &AppState) -> Value {
    let records = proxy::credentials(&st.accounts.env)
        .inspect_err(|error| tracing::warn!(%error, "cliproxy: contas do proxy ilegíveis; o assistente mostra nenhuma"))
        .unwrap_or_default();
    json!(records.iter().filter(|(_, r)| r["disabled"] != true && r["type"].is_string())
        .map(|(_, r)| json!({"type": r["type"], "email": r["email"].as_str().unwrap_or(""), "binding": proxy::binding_id(r), "prefix":r["prefix"], "plan":r["plan_type"]}))
        .collect::<Vec<_>>())
}

/// Janela dos GPT 6 quando a máquina não tem catálogo do Codex (a medida está em docs/decisoes).
const GPT6_CONTEXT: u64 = 872_000;

/// O proxy não informa o limite de contexto; o catálogo do Codex da máquina informa, por modelo. Sem o limite,
/// o Claude Code assume 1M e só compacta depois de o modelo já recusar a conversa.
pub fn fill_context(codex_home: &std::path::Path, models: &mut [Value]) {
    let catalog = std::fs::read(codex_home.join("models_cache.json")).ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).unwrap_or(Value::Null);
    let limit_of = |slug: &str| catalog["models"].as_array()?.iter().find(|m| m["slug"] == slug)
        .and_then(|m| m["max_context_window"].as_u64().or_else(|| m["context_window"].as_u64()));
    for model in models.iter_mut().filter(|m| m["context_length"].is_null()) {
        let Some(slug) = model["id"].as_str().map(|id| id.rsplit('/').next().unwrap_or(id).to_owned()) else { continue };
        if let Some(limit) = limit_of(&slug).or_else(|| slug.starts_with("gpt-6").then_some(GPT6_CONTEXT)) {
            model["context_length"] = json!(limit);
        }
    }
}

/// Sessões vivas num motor que aponta para o proxy local: atualizar corta a resposta delas. Motor que já saiu do
/// catálogo conta também: a sessão aberta nele segue no endereço de quando nasceu, e pode ser o proxy.
async fn engine_sessions(env: &AccountEnvironment, list: &ListBridge) -> Result<usize, Response> {
    let local = proxy::local(env).ok().flatten();
    let catalog = store::visible(env);
    let served: HashSet<&str> = catalog.iter()
        .filter(|(_, item)| proxy::serves(local.as_ref(), item["base_url"].as_str().unwrap_or("")))
        .map(|(name, _)| name.as_str()).collect();
    let produced = list.snapshot().await
        .map_err(|_| coded(503, "erro_lista_indisponivel", "não consegui ler as sessões abertas", json!({})))?;
    Ok(produced.rows.iter()
        .filter(|row| row.engine.as_deref().is_some_and(|e| served.contains(e) || !catalog.contains_key(e)))
        .count())
}

/// Sem confirmação, a atualização reconfere as sessões depois do download e desiste se alguma abriu no proxy.
fn recheck_sessions(st: &AppState) -> install::BeforeSwap {
    let (env, list) = (st.accounts.env.clone(), st.list.clone());
    Arc::new(move || {
        let (env, list) = (env.clone(), list.clone());
        Box::pin(async move {
            match engine_sessions(&env, &list).await {
                Ok(0) => Ok(()),
                Ok(n) => Err(install::Failure { code: "erro_cliproxy_sessoes_ativas", detail: format!("{n} sessões passaram a usar o proxy") }),
                Err(_) => Err(install::Failure { code: "erro_lista_indisponivel", detail: "não consegui reconferir as sessões abertas".into() }),
            }
        })
    })
}

fn management(st: &AppState) -> Result<login::Management, Response> {
    let env = &st.accounts.env;
    let Some(local) = proxy::local(env).ok().flatten() else {
        return Err(coded(409, "erro_cliproxy_ausente", "CLIProxyAPI não encontrado nesta máquina", json!({})));
    };
    let Some(key) = proxy::management_key(env) else {
        return Err(coded(409, "erro_cliproxy_sem_senha", "falta a senha de gerenciamento do CLIProxyAPI", json!({})));
    };
    Ok(login::Management { base_url: local.base_url, key })
}

fn login_error(error: login::Error) -> Response {
    let status = match error {
        login::Error::BadInput(_) => 400,
        login::Error::Rejected | login::Error::Disabled | login::Error::PortBusy | login::Error::Outdated(_) => 409,
        login::Error::Network(_) | login::Error::Proxy(..) => 502,
    };
    let detail = match &error {
        login::Error::BadInput(what) => format!("campo inválido: {what}"),
        login::Error::Outdated(version) => format!("CLIProxyAPI {version}: o login pelo Hangar exige a versão 8 ou mais nova"),
        login::Error::Network(e) => e.clone(),
        login::Error::Proxy(code, e) => format!("HTTP {code} {e}"),
        _ => String::new(),
    };
    coded(status, error.code(), &detail, json!({}))
}

fn body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).ok().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

fn started(result: Result<(), install::Busy>) -> Response {
    match result {
        Ok(()) => json_response(StatusCode::ACCEPTED, json!({"job": INSTALLER.job()})),
        Err(install::Busy) => coded(409, "erro_cliproxy_ocupado", "já existe uma instalação em andamento", json!({})),
    }
}

pub async fn handle(st: &AppState, method: &Method, path: &str, bytes: &[u8]) -> Option<Response> {
    let rest = path.strip_prefix("/api/engines/cliproxy/")?;
    Some(match (method, rest) {
        // Só o estado da instalação, sem sondar modelos nem contas: é o que a página de Contas lê ao abrir.
        (&Method::GET, "install") => json_response(StatusCode::OK, status(st).await),
        (&Method::POST, "install") => started(INSTALLER.start("install", context(st))),
        // Atualizar e entregar reiniciam o proxy: com sessão nele, só com a confirmação de quem pediu.
        (&Method::POST, kind @ ("update" | "adopt")) => {
            let sessions = match engine_sessions(&st.accounts.env, &st.list).await { Ok(n) => n, Err(response) => return Some(response) };
            let confirmed = body(bytes)["confirm"] == true;
            if sessions > 0 && !confirmed {
                return Some(coded(409, "erro_cliproxy_sessoes_ativas", "há sessões usando o proxy", json!({"sessions": sessions})));
            }
            let mut ctx = context(st);
            if !confirmed {
                ctx.before_swap = Some(recheck_sessions(st));
            }
            started(INSTALLER.start(if kind == "update" { "update" } else { "adopt" }, ctx))
        }
        (&Method::POST, "management-key/reset") => {
            let ctx = context(st);
            let reset = tokio::task::spawn_blocking(move || install::reset_management(&ctx)).await
                .unwrap_or_else(|e| Err(install::Failure { code: "cliproxy_config_ilegivel", detail: e.to_string() }));
            match reset {
                Ok(()) => {
                    // Cada senha recusada conta para o bloqueio do 127.0.0.1, então espera a releitura do config e
                    // confere no máximo duas vezes. Só vale como aplicada a resposta que passou pela autenticação.
                    let mut applied = false;
                    match management(st) {
                        Ok(m) => {
                            let mut reason = "o proxy ainda não a aceitou";
                            for _ in 0..RESET_PROBES {
                                tokio::time::sleep(RESET_WAIT).await;
                                match login::status(&m, "hangar-reset-probe").await {
                                    Err(login::Error::Rejected | login::Error::Network(_)) => {}
                                    Ok(_) | Err(login::Error::Proxy(400..=499, _)) => { applied = true; break; }
                                    Err(other) => { reason = other.code(); break; }
                                }
                            }
                            if !applied {
                                tracing::warn!(reason, "cliproxy: senha nova gravada, mas a aplicação não foi confirmada");
                            }
                        }
                        Err(_) => tracing::warn!("cliproxy: senha nova gravada, mas não deu para ler o endereço e a senha para conferir"),
                    }
                    // `applied: false`: a senha já está gravada; o login seguinte usa ela, sem gerar outra.
                    json_response(StatusCode::OK, json!({"status": "reset", "applied": applied}))
                }
                Err(failure) => coded(409, failure.code, &failure.detail, json!({})),
            }
        }
        (&Method::POST, "login") => {
            let m = match management(st) { Ok(m) => m, Err(response) => return Some(response) };
            match login::start(&m, body(bytes)["provider"].as_str().unwrap_or("")).await {
                Ok((url, state)) => json_response(StatusCode::OK, json!({"url": url, "state": state})),
                Err(error) => login_error(error),
            }
        }
        _ => {
            let (state, callback) = match rest.strip_prefix("login/") {
                Some(tail) => match tail.strip_suffix("/callback") { Some(state) => (state, true), None => (tail, false) },
                None => return None,
            };
            let m = match management(st) { Ok(m) => m, Err(response) => return Some(response) };
            match (method, callback) {
                (&Method::GET, false) => match login::status(&m, state).await {
                    // A conta nova ganha o prefixo que a deixa fixável por sessão.
                    Ok(login::Status::Ok { auth_id }) => {
                        let naming = proxy::naming(&st.accounts).await;
                        let account = auth_id.and_then(|id| {
                            proxy::credentials(&st.accounts.env).ok()?.into_iter().find_map(|(path, record)| {
                                if path.file_name()?.to_str()? != id || record["disabled"] == true { return None; }
                                Some(json!({"type":record["type"], "email":record["email"], "binding":proxy::binding_id(&record), "prefix":record["prefix"], "plan":record["plan_type"]}))
                            })
                        });
                        json_response(StatusCode::OK, json!({"status": "ok", "naming": naming, "account":account}))
                    },
                    Ok(login::Status::Wait) => json_response(StatusCode::OK, json!({"status": "wait"})),
                    Ok(login::Status::Failed(e)) => json_response(StatusCode::OK, json!({"status": "error", "error": e})),
                    Err(error) => login_error(error),
                },
                (&Method::POST, true) => match login::callback(&m, state, body(bytes)["redirect_url"].as_str().unwrap_or("")).await {
                    Ok(()) => json_response(StatusCode::OK, json!({"status": "sent"})),
                    Err(error) => login_error(error),
                },
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_limit_comes_from_the_codex_catalog_and_falls_back_for_gpt6() {
        let codex = tempfile::tempdir().unwrap();
        std::fs::write(codex.path().join("models_cache.json"),
            br#"{"models":[{"slug":"gpt-5.6-sol","context_window":272000,"max_context_window":872000},{"slug":"gpt-6.2-sol","context_window":500000,"max_context_window":900000}]}"#).unwrap();
        let mut models = vec![json!({"id":"conta/gpt-6.2-sol","context_length":null}), json!({"id":"gpt-5.6-sol","context_length":null}),
            json!({"id":"gpt-6-luna","context_length":null}), json!({"id":"claude-x","context_length":null}), json!({"id":"gpt-6.1-sol","context_length":400000})];
        fill_context(codex.path(), &mut models);
        let limits: Vec<_> = models.iter().map(|m| m["context_length"].as_u64()).collect();
        assert_eq!(limits, [Some(900_000), Some(872_000), Some(872_000), None, Some(400_000)]);
        let mut without = vec![json!({"id":"gpt-6.2-sol","context_length":null}), json!({"id":"gpt-5.6-sol","context_length":null})];
        fill_context(&codex.path().join("sem-catalogo"), &mut without);
        assert_eq!(without.iter().map(|m| m["context_length"].as_u64()).collect::<Vec<_>>(), [Some(872_000), None]);
    }
}
