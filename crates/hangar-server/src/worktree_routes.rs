//! Worktrees e suas preferências: o Python fornece somente fatos das sessões e conversas.
use crate::routes::AppState;
use axum::{
    body::Body,
    extract::Request,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use hangar_workspace::worktrees::{self, Session};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

const CONTEXT: &str = "worktrees_context";
const MAX_CONTEXT: usize = 16 * 1024 * 1024;

#[derive(Deserialize, Clone)]
pub(crate) struct Context {
    pub roots: Vec<String>,
    pub cwds: Vec<String>,
    pub sessions: Vec<Session>,
    pub project_bases: Vec<String>,
    #[serde(default)]
    pub activities: Option<Vec<Activity>>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Activity {
    pub cwd: String,
    pub mtime: f64,
}

pub fn matches(method: &Method, path: &str) -> bool {
    match *method {
        Method::GET => matches!(
            path,
            "/api/worktrees" | "/api/worktrees/detail" | "/api/worktrees/settings"
        ),
        Method::PUT => path == "/api/worktrees/settings",
        Method::POST => matches!(
            path,
            "/api/worktrees/create" | "/api/worktrees/delete" | "/api/worktrees/delete-merged"
        ),
        _ => false,
    }
}

fn response(value: Value, status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

/// Como o `bool` do FastAPI; valor fora disso fica com o 422 dele.
fn flag(value: Option<&String>) -> Option<bool> {
    match value.map(|s| s.to_lowercase()).as_deref() {
        None => Some(true),
        Some("1" | "true" | "on" | "yes" | "t" | "y") => Some(true),
        Some("0" | "false" | "off" | "no" | "f" | "n") => Some(false),
        _ => None,
    }
}

pub(crate) async fn context(st: &AppState) -> Result<Context, &'static str> {
    let req = axum::http::Request::get(format!(
        "http://{}/internal/worktrees/context",
        st.cfg.upstream
    ))
    .header("x-hangar-internal", &st.cfg.internal_secret)
    .body(Body::empty())
    .map_err(|_| "pedido")?;
    // A lista de pastas do Arquivo pode levar dezenas de segundos na primeira leitura.
    let resp = tokio::time::timeout(Duration::from_secs(60), st.http.request(req))
        .await
        .map_err(|_| "prazo")?
        .map_err(|e| {
            tracing::warn!(error = %e, "worktrees: contexto sem conexão com o Python");
            "conexao"
        })?;
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "worktrees: contexto recusado pelo Python");
        return Err("status");
    }
    // O teto vale durante a leitura: corpo grande não chega a ser guardado inteiro.
    let body = http_body_util::Limited::new(resp.into_body(), MAX_CONTEXT);
    let bytes = tokio::time::timeout(Duration::from_secs(60), body.collect())
        .await
        .map_err(|_| "prazo")?
        .map_err(|_| "tamanho")?
        .to_bytes();
    // O erro do serde diz linha e coluna, nunca o conteúdo.
    serde_json::from_slice(&bytes).map_err(|e| {
        tracing::warn!(error = %e, "worktrees: contexto do Python fora do formato");
        "json"
    })
}

/// Não rodou: 503 com código e motivo, como as outras rotas do Rust; o Python nunca atende.
fn refuse(st: &AppState, route: &str, code: &'static str, motivo: &str) -> Response {
    let msg = if code == hangar_workspace::BUSY {
        "Git ocupado, tente em instantes.".to_owned()
    } else {
        format!("Lista de worktrees indisponível: {motivo}.")
    };
    if crate::warn_limit::allow(None, &format!("{code}:{motivo}")) {
        tracing::warn!(route = %route, code, motivo = %motivo, "worktrees recusado");
    }
    // O diário só leva frase fixa; o motivo separa as causas.
    let reason = match motivo {
        "prazo" => "contexto do Python sem resposta no prazo",
        "conexao" => "sem conexão com o Python",
        "status" => "Python recusou o contexto",
        "json" | "tamanho" => "contexto do Python fora do formato",
        "vagas cheias" => "vagas cheias",
        "pânico na leitura" => "pânico na leitura",
        _ => "lista de worktrees indisponível",
    };
    st.diag.report("rust.worktrees_failed", "", code, reason);
    let mut resp = response(
        json!({"ok":false,"error_code":code,"message":msg,
            "detail":{"code":code,"params":{"motivo":motivo},"msg":msg}}),
        503,
    );
    if code == hangar_workspace::BUSY {
        resp.headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("2"));
    }
    resp
}

pub async fn public(st: Arc<AppState>, req: Request) -> Response {
    if req.uri().path() == "/api/worktrees/settings" {
        return settings(st, req).await;
    }
    if req.method() == Method::POST {
        return mutation(st, req).await;
    }
    let route = req.uri().path().to_owned();
    let params = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<HashMap<String, String>>();
    let detail = route == "/api/worktrees/detail";
    let measure = flag(params.get("sizes"));
    let path = params.get("path").cloned();
    // Pedido malformado: o 422 do FastAPI, no formato que os clientes já leem.
    if (detail && path.is_none()) || (!detail && measure.is_none()) {
        return response(json!({"detail":"parâmetros de worktrees inválidos"}), 422);
    }
    let ctx = match context(&st).await {
        Ok(ctx) => ctx,
        Err(motivo) => return refuse(&st, &route, CONTEXT, motivo),
    };
    // Sem vaga de leitura: quem limita os `git` da lista é o teto global do núcleo, e pedido
    // que espera por ele não vira 503 de "ocupado" como no Python, que também esperava.
    let repo = params.get("repo").cloned();
    let done = tokio::task::spawn_blocking(move || -> hangar_workspace::Result<Value> {
        if detail {
            let path = worktrees::allowed_worktree(&path.unwrap_or_default(), &ctx.roots)?;
            Ok(worktrees::status_of(
                &path,
                &ctx.sessions,
                true,
                &ctx.project_bases,
            ))
        } else {
            let repo = repo
                .map(|r| worktrees::allowed_repo(&r, &ctx.roots))
                .transpose()?;
            let repos = worktrees::list_all(
                &ctx.cwds,
                &ctx.sessions,
                Some(&ctx.roots),
                repo.as_deref(),
                measure.unwrap_or(true),
                &ctx.project_bases,
            );
            let problem=worktrees::discovery_problem(&ctx.roots);
            Ok(json!({"repos": repos,"warning":problem.map(|_|"worktree_discovery_incomplete"),"discovery_partial":problem.is_some(),"discovery_problem":problem}))
        }
    })
    .await;
    match done {
        Ok(Ok(value)) => response(value, 200),
        // Recusa de caminho: o mesmo `{"detail": ...}` do HTTPException do Python.
        Ok(Err(e)) => response(json!({"detail": e.detail}), e.status),
        Err(e) => {
            tracing::error!(panic = e.is_panic(), "worktrees: leitura não terminou");
            refuse(
                &st,
                &route,
                hangar_workspace::UNAVAILABLE,
                "pânico na leitura",
            )
        }
    }
}

async fn body<T: serde::de::DeserializeOwned>(req: Request) -> hangar_workspace::Result<T> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(6),
        axum::body::to_bytes(req.into_body(), 64 * 1024),
    )
    .await
    .map_err(|_| hangar_workspace::error(422, "corpo de worktrees inválido"))?
    .map_err(|_| hangar_workspace::error(422, "corpo de worktrees inválido"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| hangar_workspace::error(422, "corpo de worktrees inválido"))
}
fn result(value: hangar_workspace::Result<Value>) -> Response {
    match value {
        Ok(v) => response(v, 200),
        Err(e) => response(json!({"detail":e.detail}), e.status),
    }
}
async fn settings(st: Arc<AppState>, req: Request) -> Response {
    use hangar_workspace::worktree_settings::WorktreeSettings;
    let file = st.origins_home.join(".hangar/worktree-settings.json");
    let home = st.origins_home.clone();
    let update = if req.method() == Method::PUT {
        match body::<WorktreeSettings>(req).await {
            Ok(v) => Some(v),
            Err(e) => return result(Err(e)),
        }
    } else {
        None
    };
    let done = tokio::task::spawn_blocking(move || {
        if let Some(settings) = update {
            settings.save_at(&file, &home)?;
            Ok(json!(settings))
        } else {
            let settings = WorktreeSettings::load_at(&file)?;
            settings.location_path(&home)?;
            Ok(json!(settings))
        }
    })
    .await;
    result(done.unwrap_or_else(|_| {
        Err(hangar_workspace::error(
            503,
            "configuração de worktrees indisponível",
        ))
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    repo: String,
    branch: String,
    name: String,
    #[serde(default)]
    new_branch: bool,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    fetch: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    repo: String,
    path: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    delete_branch: bool,
    #[serde(default)]
    keep_branch: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteMerged {
    repo: String,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    lossy: Option<Vec<String>>,
    #[serde(default)]
    ready_only: bool,
}

pub(crate) async fn fresh_context(st: &AppState) -> hangar_workspace::Result<Context> {
    let mut ctx = context(st)
        .await
        .map_err(|_| hangar_workspace::error(503, "fatos de worktrees indisponíveis"))?;
    let (panes, problem) = st
        .list
        .env()
        .mux
        .list_panes_checked()
        .await
        .map_err(|_| hangar_workspace::error(503, "terminais de worktrees indisponíveis"))?;
    if problem.is_some() {
        return Err(hangar_workspace::error(
            503,
            "terminais de worktrees ilegíveis",
        ));
    }
    let panes = live_panes(&ctx.sessions, panes)?;
    ctx.sessions.extend(panes.into_iter().map(|p| Session {
        name: p.session,
        cwd: Some(p.cwd),
        ..Session::default()
    }));
    Ok(ctx)
}

/// Pasta desconhecida trava a remoção. Terminal morto não tem processo nem pasta e não prende nada,
/// nem a sessão que só tem terminais mortos.
fn live_panes(
    sessions: &[Session],
    panes: Vec<crate::list::mux::Pane>,
) -> hangar_workspace::Result<Vec<crate::list::mux::Pane>> {
    // Morto que ainda informa pasta continua protegendo essa pasta.
    let gone = |p: &crate::list::mux::Pane| p.dead && p.cwd.is_empty();
    let only_dead = |name: &str| {
        let mut own = panes.iter().filter(|p| p.session == name).peekable();
        own.peek().is_some() && own.all(gone)
    };
    if sessions.iter().any(|s| {
        !only_dead(&s.name)
            && [
                s.cwd.as_deref(),
                s.git_cwd.as_deref(),
                s.worktree_path.as_deref(),
            ]
            .into_iter()
            .flatten()
            .all(|p| p.is_empty())
    }) {
        return Err(hangar_workspace::error(
            503,
            "pasta de sessão desconhecida; remoção conservada",
        ));
    }
    let panes: Vec<_> = panes.into_iter().filter(|p| !gone(p)).collect();
    if panes.iter().any(|p| p.cwd.is_empty()) {
        return Err(hangar_workspace::error(
            503,
            "terminais de worktrees ilegíveis",
        ));
    }
    Ok(panes)
}

pub(crate) fn final_context(
    st: &Arc<AppState>,
    runtime: &tokio::runtime::Handle,
    path: &str,
) -> hangar_workspace::Result<Context> {
    let ctx = runtime.block_on(fresh_context(st))?;
    worktrees::allowed_worktree(path, &ctx.roots)?;
    if hangar_workspace::worktree_cleanup::in_use(path, &ctx.sessions) {
        return Err(hangar_workspace::error(
            409,
            "sessão ou terminal abriu dentro da worktree",
        ));
    }
    Ok(ctx)
}

async fn mutation(st: Arc<AppState>, req: Request) -> Response {
    let route = req.uri().path().to_owned();
    if route == "/api/worktrees/create" {
        let b = match body::<Create>(req).await {
            Ok(v) => v,
            Err(e) => return result(Err(e)),
        };
        let ctx = match context(&st).await {
            Ok(ctx) => ctx,
            Err(_) => {
                return result(Err(hangar_workspace::error(
                    503,
                    "fatos de worktrees indisponíveis",
                )));
            }
        };
        let done = tokio::task::spawn_blocking(move || {
            let repo = worktrees::allowed_repo(&b.repo, &ctx.roots)?;
            let root = ctx
                .roots
                .iter()
                .filter(|r| Path::new(&repo).starts_with(hangar_workspace::real(Path::new(r))))
                .max_by_key(|r| r.len())
                .ok_or_else(|| hangar_workspace::error(403, "root not allowed"))?;
            let name = crate::list::discover_other::sanitize_session_name(&b.name);
            if b.repo.is_empty() || b.branch.is_empty() || name.is_empty() {
                return Err(hangar_workspace::error(400, "nome de pasta inválido"));
            }
            if b.fetch {
                hangar_workspace::git::network_checked(
                    Path::new(&repo),
                    &["fetch", "--all", "--prune"],
                )?;
            }
            let settings = hangar_workspace::worktree_settings::WorktreeSettings::load_at(
                &st.origins_home.join(".hangar/worktree-settings.json"),
            )?;
            let location = settings.location_path(&st.origins_home)?;
            let created = hangar_workspace::git::create_worktree_at(
                Path::new(&repo),
                &b.branch,
                &name,
                Path::new(root),
                b.new_branch,
                b.base.as_deref(),
                &location,
            )?;
            if created[1] != true {
                return Err(hangar_workspace::error(
                    409,
                    "essa branch já é a da pasta principal",
                ));
            }
            st.list.invalidate();
            Ok(json!({"path":created[0]}))
        })
        .await;
        // Caiu no meio: pode já ter mexido no disco, então não é a recusa 503 de antes de agir.
        return result(done.unwrap_or_else(|_| {
            Err(hangar_workspace::error(
                500,
                "criação de worktree indisponível",
            ))
        }));
    }
    if route == "/api/worktrees/delete" {
        let b = match body::<Delete>(req).await {
            Ok(v) => v,
            Err(e) => return result(Err(e)),
        };
        let ctx = match fresh_context(&st).await {
            Ok(ctx) => ctx,
            Err(e) => return result(Err(e)),
        };
        let runtime = tokio::runtime::Handle::current();
        let done = tokio::task::spawn_blocking(move || {
            let repo = worktrees::allowed_repo(&b.repo, &ctx.roots)?;
            let path = worktrees::allowed_repo(&b.path, &ctx.roots)?;
            let guard = |path: &str| final_context(&st, &runtime, path).map(|_| ());
            let value = hangar_workspace::worktree_cleanup::remove_with_guard(
                &repo,
                &path,
                &ctx.sessions,
                &ctx.project_bases,
                &st.origins_home,
                hangar_workspace::worktree_cleanup::RemoveOptions {
                    confirm: b.confirm,
                    delete_branch: b.delete_branch,
                    keep_branch: b.keep_branch,
                    ..Default::default()
                },
                &guard,
            );
            st.list.invalidate();
            value
        })
        .await;
        return result(done.unwrap_or_else(|_| {
            Err(hangar_workspace::error(
                500,
                "remoção de worktree indisponível",
            ))
        }));
    }
    let b = match body::<DeleteMerged>(req).await {
        Ok(v) => v,
        Err(e) => return result(Err(e)),
    };
    if b.confirm && b.paths.is_none() {
        return result(Err(hangar_workspace::error(
            422,
            "confirmar exige a lista das worktrees mostradas",
        )));
    }
    let ctx = match fresh_context(&st).await {
        Ok(ctx) => ctx,
        Err(e) => return result(Err(e)),
    };
    let roots = ctx.roots.clone();
    let repo = b.repo.clone();
    let (main,paths)=match tokio::task::spawn_blocking(move || -> hangar_workspace::Result<_> {
        let repo=worktrees::allowed_repo(&repo,&roots)?;
        let root=worktrees::repo_root_of(&repo).unwrap_or_else(||repo.clone());
        let main=worktrees::main_repo_of(&root);
        let paths=worktrees::worktree_paths(&main);
        Ok((main,paths))
    }).await {
        Ok(Ok(value))=>value,
        Ok(Err(e))=>return result(Err(e)),
        Err(_)=>return result(Err(hangar_workspace::error(503,"repositório indisponível"))),
    };
    let mut removed = Vec::new();
    for path in paths {
        if b.paths.as_ref().is_some_and(|paths| {
            !paths.iter().any(|p| {
                hangar_workspace::real(Path::new(p)) == hangar_workspace::real(Path::new(&path))
            })
        }) {
            continue;
        }
        let ctx = match fresh_context(&st).await {
            Ok(ctx) => ctx,
            Err(e) => return result(Err(e)),
        };
        let home = st.origins_home.clone();
        let path = path.clone();
        let repo = main.clone();
        let confirm = b.confirm
            && b.lossy.as_ref().is_some_and(|paths| {
                paths.iter().any(|p| {
                    hangar_workspace::real(Path::new(p)) == hangar_workspace::real(Path::new(&path))
                })
            });
        let runtime = tokio::runtime::Handle::current();
        let reader = st.clone();
        let done =
            tokio::task::spawn_blocking(move || -> hangar_workspace::Result<Option<String>> {
                let status = worktrees::status_of(&path, &ctx.sessions, false, &ctx.project_bases);
                if status["merged"] != true
                    || status["degraded"] != false
                    || (b.ready_only && status["dirty"].as_u64() != Some(0))
                    || hangar_workspace::worktree_cleanup::in_use(&path, &ctx.sessions)
                {
                    return Ok(None);
                }
                let loses = status["dirty"].as_u64().unwrap_or(1) > 0
                    || status["ignored"].as_array().is_none_or(|a| !a.is_empty());
                if loses && !confirm {
                    return Ok(None);
                }
                let guard = |path: &str| final_context(&reader, &runtime, path).map(|_| ());
                hangar_workspace::worktree_cleanup::remove_with_guard(
                    &repo,
                    &path,
                    &ctx.sessions,
                    &ctx.project_bases,
                    &home,
                    hangar_workspace::worktree_cleanup::RemoveOptions {
                        confirm: loses,
                        merged_only: true,
                        require_clean: b.ready_only,
                        ..Default::default()
                    },
                    &guard,
                )?;
                Ok(Some(path))
            })
            .await;
        match done {
            Ok(Ok(Some(path))) => removed.push(path),
            Ok(Ok(None)) => {}
            Ok(Err(mut e)) => {
                if !removed.is_empty() {
                    e.detail = json!(format!(
                        "{} (já removidas: {})",
                        e.detail.as_str().unwrap_or("remoção falhou"),
                        removed.join(", ")
                    ));
                }
                st.list.invalidate();
                return result(Err(e));
            }
            Err(_) => {
                return result(Err(hangar_workspace::error(
                    500,
                    "remoção de worktree indisponível",
                )));
            }
        }
    }
    st.list.invalidate();
    response(json!({"removed":removed}), 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::mux::Pane;

    fn pane(session: &str, cwd: &str, dead: bool) -> Pane {
        Pane { session: session.into(), cwd: cwd.into(), dead, ..Pane::default() }
    }

    fn session(name: &str, cwd: &str) -> Session {
        Session { name: name.into(), cwd: Some(cwd.into()), ..Session::default() }
    }

    #[test]
    fn dead_terminals_do_not_block_removal() {
        let sessions = [session("viva", "/w"), session("morta", "")];
        let panes = vec![pane("viva", "/w", false), pane("morta", "", true), pane("atalho", "", true), pane("com-pasta", "/x", true)];
        let live = live_panes(&sessions, panes).unwrap();
        assert_eq!(live.iter().map(|p| p.session.as_str()).collect::<Vec<_>>(), ["viva", "com-pasta"]);
    }

    #[test]
    fn unknown_folder_still_blocks_removal() {
        let headless = [session("sem-terminal", "")];
        assert_eq!(live_panes(&headless, vec![]).unwrap_err().status, 503);
        let alive = [session("viva", "")];
        assert_eq!(live_panes(&alive, vec![pane("viva", "", false)]).unwrap_err().status, 503);
        assert_eq!(live_panes(&[], vec![pane("solto", "", false)]).unwrap_err().status, 503);
    }
}
