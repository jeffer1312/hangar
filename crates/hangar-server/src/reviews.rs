//! Rotas das revisões (`crates/hangar-reviews`): só o dono; o convidado recebe 403. O servidor liga as rotas e guarda o
//! contexto dos repositórios por uns segundos, porque a aba e a lista pedem detalhe em sequência.
use crate::routes::AppState;
use axum::{extract::Request, http::{Method, StatusCode}, response::Response};
use hangar_reviews::{Action, Failure, Known};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

const ROUTES: &[(&str, &str)] = &[
    ("/api/reviews", "GET"), ("/api/reviews/branch", "GET"), ("/api/reviews/detail", "GET"), ("/api/reviews/action", "POST"),
];

/// O contexto (sessões e, na lista, os hosts) vale isto: abrir a aba e trocar de revisão não releem tudo.
const KNOWN_TTL: Duration = Duration::from_secs(20);

pub fn matches(_method: &Method, path: &str) -> bool {
    let path = crate::workspace_http::decoded(path);
    let path = path.trim_end_matches('/');
    path == "/api/reviews" || path.starts_with("/api/reviews/")
}

pub(crate) async fn serve_forwarded(st: Arc<AppState>, peer: std::net::SocketAddr, mut req: Request, guest: bool) -> Response {
    if guest { return error(403, "guest_forbidden", "convidado não acessa revisões"); }
    req.extensions_mut().insert(crate::session_write::bridge::BridgedOwner);
    route(st, peer, req).await
}

pub async fn route(st: Arc<AppState>, peer: std::net::SocketAddr, req: Request) -> Response {
    let headers = req.headers().clone();
    let method = req.method().clone();
    let mut response = route_inner(st, peer, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn route_inner(st: Arc<AppState>, peer: std::net::SocketAddr, req: Request) -> Response {
    let path = crate::workspace_http::decoded(req.uri().path()).into_owned();
    if let Some(response) = crate::http_cors::preflight(req.method(), req.headers()) { return response; }
    let decision = crate::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    let forward = crate::routes::forward_for(&st, peer, &req);
    if let Some(response) = crate::http_route_response::response(&decision, req.method(), req.uri(), req.headers(), forward.https, "") {
        return response;
    }
    match crate::routes::who(&st, peer, &req).await {
        Err(e) => return crate::auth::unavailable(e),
        Ok((_, Some(crate::share::caller::Caller::Guest(_)))) => return error(403, "guest_forbidden", "convidado não acessa revisões"),
        _ => {}
    }
    if let Err(response) = crate::routes::owner_gate(&st, peer, &req).await { return response; }
    let query: HashMap<String, String> = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    match path.as_str() {
        "/api/reviews" => match known(&st, true).await {
            Ok((_, known)) => ok(json!(hangar_reviews::list(&known).await)),
            Err(e) => failure(e),
        },
        "/api/reviews/branch" => match query.get("path") {
            Some(dir) => branch(&st, dir, query.get("summary").is_some_and(|v| v == "1")).await,
            None => error(422, "bad_request", "falta o caminho"),
        },
        "/api/reviews/detail" => match target(&query) {
            Some((host, repo, number)) => match known(&st, false).await {
                Ok((_, known)) => hangar_reviews::detail(&known, &host, &repo, number).await.map(|d| ok(json!(d))).unwrap_or_else(failure),
                Err(e) => failure(e),
            },
            None => error(422, "bad_request", "host, repositório ou número inválido"),
        },
        "/api/reviews/action" => action(&st, req).await,
        _ => error(404, "not_found", "rota de revisões desconhecida"),
    }
}

fn ok(value: Value) -> Response { hangar_core::http::json_response(StatusCode::OK, value) }

fn error(status: u16, code: &str, msg: &str) -> Response {
    hangar_core::http::detail(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), code, msg, json!({}))
}

fn failure(e: Failure) -> Response { error(e.status(), e.code, &e.message) }

fn target(query: &HashMap<String, String>) -> Option<(String, String, u64)> {
    let host = query.get("host")?.to_ascii_lowercase();
    let repo = query.get("repo")?.clone();
    let number = query.get("number")?.parse().ok()?;
    (hangar_reviews::valid_host(&host) && hangar_reviews::valid_repo(&repo)).then_some((host, repo, number))
}

struct Cached { at: Instant, roots: Vec<String>, known: Arc<Known> }

static KNOWN: LazyLock<Mutex<HashMap<bool, Cached>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// O contexto das worktrees (raízes, pastas recentes, sessões) traduzido para revisões, com as raízes para a checagem
/// de caminho.
async fn known(st: &Arc<AppState>, with_hosts: bool) -> Result<(Vec<String>, Arc<Known>), Failure> {
    if let Some(c) = KNOWN.lock().unwrap_or_else(|e| e.into_inner()).get(&with_hosts).filter(|c| c.at.elapsed() < KNOWN_TTL) {
        return Ok((c.roots.clone(), c.known.clone()));
    }
    let ctx = crate::worktree_routes::context(st).await.map_err(|m| Failure::new("context_unavailable", m))?;
    let sessions = ctx.sessions.iter().filter_map(|s| {
        let dir = s.git_cwd.clone().or_else(|| s.worktree_path.clone()).or_else(|| s.cwd.clone()).filter(|d| !d.is_empty())?;
        Some((s.name.clone(), dir))
    }).collect();
    let roots = ctx.roots.clone();
    let known = tokio::task::spawn_blocking(move || hangar_reviews::known(&ctx.roots, &ctx.cwds, sessions, with_hosts))
        .await.map_err(|_| Failure::new("context_unavailable", "leitura dos repositórios não terminou"))?;
    let known = Arc::new(known);
    KNOWN.lock().unwrap_or_else(|e| e.into_inner()).insert(with_hosts, Cached { at: Instant::now(), roots: roots.clone(), known: known.clone() });
    Ok((roots, known))
}

/// Pasta de sessão ou de repositório dentro das raízes (a mesma regra das worktrees).
async fn branch(st: &Arc<AppState>, dir: &str, summary_only: bool) -> Response {
    let (roots, known) = match known(st, false).await { Ok(k) => k, Err(e) => return failure(e) };
    let real = hangar_workspace::real(Path::new(dir));
    let allowed = known.session_dirs.iter().any(|d| d == &real) || hangar_workspace::worktrees::allowed_worktree(dir, &roots).is_ok();
    if !allowed { return error(403, "path_not_allowed", "pasta fora das raízes e das sessões"); }
    match hangar_reviews::branch(&known, &real, summary_only).await {
        Ok(state) => ok(json!(state)),
        Err(e) => failure(e),
    }
}

async fn action(st: &Arc<AppState>, req: Request) -> Response {
    let bytes = match tokio::time::timeout(Duration::from_secs(6), axum::body::to_bytes(req.into_body(), 256 * 1024)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return error(422, "bad_request", "corpo inválido"),
    };
    let Ok(body) = serde_json::from_slice::<Action>(&bytes) else { return error(422, "bad_request", "corpo inválido") };
    match hangar_reviews::act(body).await {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => {
            if e.code == "action_failed" { st.diag.report("rust.review_action_failed", "", e.code, "ação de revisão falhou no provedor"); }
            failure(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_reads_query() {
        let q: HashMap<String, String> = [("host", "GitHub.com"), ("repo", "a/b"), ("number", "12")].into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        assert_eq!(target(&q), Some(("github.com".into(), "a/b".into(), 12)));
    }

    #[test]
    fn routes_are_claimed() {
        assert!(matches(&Method::GET, "/api/reviews"));
        assert!(matches(&Method::GET, "/api/reviews/branch"));
        assert!(!matches(&Method::GET, "/api/reviewsx"));
    }
}
