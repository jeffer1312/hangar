//! O porteiro autoriza antes do disco; consultas ao Python trazem apenas identidade e raízes.
use crate::{
    proxy::Forward,
    routes::{AppState, pass},
};
use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use hangar_workspace::{Operation, WorkspaceError};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
const MAX_BODY: usize = 4 * 1024 * 1024;
const CONTEXT: &str = "workspace_context";

fn response(value: Value, status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}
fn failure(e: WorkspaceError, route: &str) -> Response {
    let detail = if let Some(code) = &e.code {
        let msg = if route.contains("search") || route.contains("resolver") {
            "Não deu para completar a busca."
        } else {
            "Não deu para acessar esse arquivo ou pasta."
        };
        json!({"code":code,"params":{"msg":msg},"msg":msg})
    } else if route.ends_with("path-diff") {
        let msg = "Não deu para montar o diff.";
        json!({"code":"erro_git_diff","params":{"msg":msg},"msg":msg})
    } else {
        e.detail
    };
    response(json!({"detail":detail}), e.status)
}
pub(crate) fn private_ok(st: &AppState, peer: SocketAddr, headers: &HeaderMap) -> bool {
    let token = headers
        .get("x-hangar-internal")
        .map(|h| h.as_bytes())
        .unwrap_or_default();
    let external = headers.get_all("x-forwarded-for").iter().any(|h| {
        h.to_str().map_or(true, |s| {
            s.split(',').any(|p| {
                p.trim()
                    .parse::<std::net::IpAddr>()
                    .map_or(true, |p| !p.is_loopback())
            })
        })
    });
    peer.ip().is_loopback()
        && !external
        && !st.cfg.internal_secret.is_empty()
        && bool::from(token.ct_eq(st.cfg.internal_secret.as_bytes()))
}
/// Vaga cheia recusa na hora em vez de enfileirar atrás de um fetch lento. Escrita só volta como
/// `unavailable` antes de rodar; depois, nunca se repete.
async fn execute(st: &Arc<AppState>, op: Operation) -> hangar_workspace::Result<Value> {
    let mutation = op.is_mutation();
    let slots = if matches!(op, Operation::HeadInfo { .. } | Operation::BranchOf { .. }) {
        &st.workspace_meta_slots
    } else if mutation {
        &st.workspace_slots
    } else {
        &st.workspace_read_slots
    };
    let permit = slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| hangar_workspace::busy())?;
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hangar_workspace::execute(op)
    })
    .await
    {
        // Um comando anterior da mesma escrita pode já ter mudado o disco.
        Ok(Err(e)) if mutation && hangar_workspace::is_unavailable(&e) => {
            Err(hangar_workspace::error(500, e.detail.as_str().unwrap_or("git falhou")))
        }
        Ok(result) => result,
        Err(_) if mutation => Err(hangar_workspace::error(
            503,
            "Não foi possível confirmar o resultado. Confira o estado antes de repetir.",
        )),
        Err(_) => Err(hangar_workspace::unavailable("pânico na leitura")),
    }
}
pub async fn private(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let bytes =
        match tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY))
            .await
        {
            Ok(Ok(b)) => b,
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match execute(&st, op).await {
        Ok(result) => response(json!({"ok":true,"result":result}), 200),
        Err(error) => response(json!({"ok":false,"error":error}), 200),
    }
}
pub fn matches(method: &Method, path: &str) -> bool {
    if path.starts_with("/api/fs/") {
        return match *method {
            Method::GET => matches!(
                path,
                "/api/fs/roots" | "/api/fs/scan" | "/api/fs/branches" | "/api/fs/git"
            ),
            Method::POST => matches!(
                path,
                "/api/fs/mkdir"
                    | "/api/fs/git/fetch"
                    | "/api/fs/git/pull"
                    | "/api/fs/git/switch"
                    | "/api/fs/git/branch"
            ),
            _ => false,
        };
    }
    let Some(tail) = path
        .strip_prefix("/api/sessions/")
        .and_then(|p| p.split_once('/').map(|(_, p)| p))
    else {
        return false;
    };
    match *method {
        Method::GET => {
            matches!(
                tail,
                "branches"
                    | "git/files"
                    | "git/log"
                    | "git/last-message"
                    | "files/list"
                    | "files/read"
                    | "files/search"
                    | "file"
                    | "file/text"
            ) || tail.starts_with("git/commit/")
                && tail.split('/').count() == 4
                && ["files", "diff", "diff-full", "diff-worktree", "branches"]
                    .contains(&tail.rsplit('/').next().unwrap_or(""))
        }
        Method::POST => matches!(
            tail,
            "checkout"
                | "git"
                | "git/diff"
                | "git/path-diff"
                | "git/discard"
                | "git/commit"
                | "git/revert"
                | "git/cherry-pick"
                | "git/push"
                | "git/reset"
                | "git/branch"
                | "git/tag"
                | "files/write"
                | "files/resolver"
                | "file/text"
        ),
        _ => false,
    }
}
/// `Err(None)`: sessão inexistente, resposta normal do Python. `Err(Some(motivo))`: falha.
async fn context(st: &AppState, name: Option<&str>) -> Result<Value, Option<&'static str>> {
    let query = name
        .map(|n| {
            format!(
                "?name={}",
                percent_encoding::utf8_percent_encode(n, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .unwrap_or_default();
    let req = axum::http::Request::get(format!(
        "http://{}/internal/workspace/context{query}",
        st.cfg.upstream
    ))
    .header("x-hangar-internal", &st.cfg.internal_secret)
    .body(Body::empty())
    .map_err(|_| Some("pedido"))?;
    let resp = tokio::time::timeout(Duration::from_secs(10), st.http.request(req))
        .await
        .map_err(|_| Some("prazo"))?
        .map_err(|_| Some("conexao"))?;
    if resp.status() == StatusCode::NOT_FOUND && name.is_some() {
        return Err(None);
    }
    if !resp.status().is_success() {
        return Err(Some("status"));
    }
    let bytes = tokio::time::timeout(Duration::from_secs(10), resp.into_body().collect())
        .await
        .map_err(|_| Some("prazo"))?
        .map_err(|_| Some("conexao"))?
        .to_bytes();
    if bytes.len() > MAX_BODY {
        return Err(Some("tamanho"));
    }
    serde_json::from_slice(&bytes).map_err(|_| Some("json"))
}
fn bool_param(value: Option<&String>, default: bool) -> Option<bool> {
    match value.map(|s| s.to_lowercase()).as_deref() {
        None => Some(default),
        Some("1" | "true" | "on" | "yes") => Some(true),
        Some("0" | "false" | "off" | "no") => Some(false),
        _ => None,
    }
}
/// Pasta do git da sessão: a worktree onde o agente trabalha, ou o cwd (o Python já decide).
fn git_cwd(ctx: &Value) -> Option<&str> {
    ctx["session"]["git_cwd"].as_str()
}

fn operation(name: &str, args: Value) -> Option<Operation> {
    serde_json::from_value(json!({"op":name,"args":args})).ok()
}
fn sessions(ctx: &Value, top: Option<&str>) -> Vec<String> {
    let Some(top) = top else { return Vec::new() };
    let root = hangar_workspace::real(Path::new(top));
    let mut names = ctx["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let cwd = s["cwd"].as_str()?;
            (hangar_workspace::real(Path::new(cwd)).starts_with(&root))
                .then(|| s["name"].as_str().unwrap_or("").to_owned())
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Git/arquivos que não rodou responde 503 com o código; o Python nunca atende no lugar.
fn refuse(st: &AppState, session: &str, route: &str, code: &'static str, motivo: &str) -> Response {
    let (event, msg, reason) = match code {
        hangar_workspace::BUSY => (
            "rust.workspace_busy",
            "Git ocupado, tente em instantes.".to_owned(),
            "vagas cheias",
        ),
        CONTEXT => (
            "rust.workspace_failed",
            format!("Git/arquivos indisponível: sem os dados da sessão ({motivo})."),
            "contexto da sessão indisponível",
        ),
        _ => (
            "rust.workspace_failed",
            format!("Git/arquivos indisponível: {motivo}"),
            // O diário só leva frase fixa; a categoria separa causas no limite por minuto.
            if motivo.starts_with("git não") {
                "git não iniciou"
            } else if motivo.starts_with("pânico") {
                "pânico na leitura"
            } else {
                "comando sem proteção da árvore"
            },
        ),
    };
    if crate::warn_limit::allow(Some(session), &format!("{code}:{motivo}")) {
        // O motivo vem do próprio Rust (prazo, git ausente), nunca do stderr do git.
        tracing::warn!(session = %session, route = %route, code, motivo = %motivo, "Git/arquivos recusado");
    }
    st.diag.report(event, session, code, reason);
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

pub async fn public(st: Arc<AppState>, req: Request, forward: Forward) -> Response {
    let route = req.uri().path().to_owned();
    let name = route
        .strip_prefix("/api/sessions/")
        .and_then(|p| p.split_once('/').map(|(n, _)| n))
        .and_then(|n| {
            percent_encoding::percent_decode_str(n)
                .decode_utf8()
                .ok()
                .map(|s| s.into_owned())
        });
    let tail = match &name {
        Some(_) => route
            .strip_prefix("/api/sessions/")
            .and_then(|p| p.split_once('/'))
            .map_or("", |(_, t)| t),
        None => route.as_str(),
    }
    .to_owned();
    // Pasta sem sessão (`/api/fs/*`) vai ao diário sem nome.
    let session = name.clone().unwrap_or_default();
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return response(json!({"detail":"request body too large"}), 413),
    };
    let ctx = match context(&st, name.as_deref()).await {
        Ok(ctx) => ctx,
        // Sessão inexistente: o 404 é do Python, nada roda.
        Err(None) => return pass(&st, Request::from_parts(parts, Body::from(bytes)), &forward).await,
        Err(Some(reason)) => return refuse(&st, &session, &tail, CONTEXT, reason),
    };
    match run(&st, parts, bytes, &tail, ctx, &forward).await {
        Ok(response) => response,
        Err(e) if hangar_workspace::is_unavailable(&e) => {
            let code = if e.code.as_deref() == Some(hangar_workspace::BUSY) {
                hangar_workspace::BUSY
            } else {
                hangar_workspace::UNAVAILABLE
            };
            refuse(&st, &session, &tail, code, e.detail.as_str().unwrap_or(""))
        }
        Err(e) => {
            // Prazo do git (rede pendurada, repositório enorme) estoura igual no Python.
            if e.status >= 500 && e.status != 504 {
                if crate::warn_limit::allow(Some(&session), "workspace_error") {
                    // Sem o detalhe: o stderr do git pode citar caminhos e nomes do usuário.
                    tracing::warn!(session = %session, route = %tail, status = e.status,
                        code = "workspace_error", "Git/arquivos no Rust falhou");
                }
                // Inclui a escrita cujo git não iniciou: vira 500 porque um comando anterior pode ter rodado.
                st.diag.report("rust.workspace_failed", &session, "workspace_error", "Git/arquivos falhou");
            }
            failure(e, &tail)
        }
    }
}

async fn run(
    st: &Arc<AppState>,
    parts: axum::http::request::Parts,
    bytes: Bytes,
    tail: &str,
    ctx: Value,
    forward: &Forward,
) -> hangar_workspace::Result<Response> {
    let route = parts.uri.path().to_owned();
    let params = form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<HashMap<String, String>>();
    let method = parts.method.clone();
    let payload = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => v,
            _ => return Ok(pass(st, Request::from_parts(parts, Body::from(bytes)), forward).await),
        }
    };
    if method == Method::POST && !body_valid(tail, &payload) {
        return Ok(pass(st, Request::from_parts(parts, Body::from(bytes)), forward).await);
    }
    let Some(op) = map_operation(tail, &method, &params, &payload, &ctx) else {
        return Ok(pass(st, Request::from_parts(parts, Body::from(bytes)), forward).await);
    };
    if tail == "file" {
        let path = execute(st, op).await?;
        return Ok(serve_file(
            Path::new(path.as_str().unwrap_or("")),
            &parts.headers,
            bool_param(params.get("download"), false).unwrap_or(false),
        )
        .await);
    }
    let mut result = if tail == "files/resolver" {
        let cwd = ctx["session"]["cwd"].as_str().unwrap_or("").to_owned();
        let jsonl = ctx["session"]["jsonl"].as_str().map(str::to_owned);
        let paths = payload["caminhos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let permit = st
            .workspace_read_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| hangar_workspace::busy())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            resolve_many(&cwd, jsonl.as_deref(), &paths)
        })
        .await
        .unwrap_or_else(|_| Err(hangar_workspace::unavailable("pânico na leitura")))?
    } else if tail == "file/text" {
        let path = execute(st, op).await?;
        let path = path.as_str().unwrap_or("");
        let args = if method == Method::POST {
            json!({"alvo":path,"path":payload["path"],"texto":payload["text"],"digest_lido":payload["digest"]})
        } else {
            json!({"alvo":path,"path":params.get("path")})
        };
        let Some(next) = operation(
            if method == Method::POST {
                "write_at"
            } else {
                "read_at"
            },
            args,
        ) else {
            return Ok(pass(st, Request::from_parts(parts, Body::from(bytes)), forward).await);
        };
        execute(st, next).await?
    } else if route.starts_with("/api/fs/")
        && matches!(
            tail,
            "/api/fs/branches"
                | "/api/fs/git"
                | "/api/fs/git/fetch"
                | "/api/fs/git/pull"
                | "/api/fs/git/switch"
                | "/api/fs/git/branch"
        )
    {
        folder_operation(st, op, tail, &ctx, &payload, &params).await?
    } else {
        execute(st, op).await?
    };
    if tail == "git/files" {
        let cwd = git_cwd(&ctx).unwrap_or("");
        let sequencer = execute(st, Operation::SequencerState { cwd: cwd.into() }).await?;
        result = json!({"files":result,"sequencer":sequencer});
    }
    if tail == "git/log" {
        if params.get("q").is_none_or(|q| q.is_empty()) {
            result = hangar_workspace::git::lanes(result.as_array().cloned().unwrap_or_default());
        }
        let summary = execute(
            st,
            Operation::GitSummary {
                cwd: git_cwd(&ctx).map(str::to_owned),
            },
        )
        .await
        .unwrap_or(Value::Null);
        result = json!({"commits":result,"ahead":summary["ahead"],"behind":summary["behind"]});
    }
    if tail.starts_with("git/commit/") && tail.ends_with("/files") {
        result = json!({"files":result});
    }
    Ok(response(result, 200))
}

fn body_valid(tail: &str, body: &Value) -> bool {
    let Some(map) = body.as_object() else {
        return false;
    };
    let (allowed, required): (&[&str], &[&str]) = match tail {
        "checkout" => (&["branch"], &["branch"]),
        "git/diff" | "git/discard" => (&["path"], &["path"]),
        "git/path-diff" => (&["path", "escopo"], &["path"]),
        "git/commit" => (&["message", "paths", "amend", "new_branch"], &["message"]),
        "git/revert" | "git/cherry-pick" => (&["sha"], &["sha"]),
        "git/branch" => (&["name", "sha", "switch_after"], &["name"]),
        "git/tag" => (&["name", "sha", "message"], &["name"]),
        "files/resolver" => (&["caminhos"], &[]),
        "file/text" | "files/write" => (&["path", "text", "digest"], &["path", "text"]),
        "/api/fs/git/fetch" | "/api/fs/git/pull" => (&["root", "path"], &["root"]),
        "/api/fs/git/switch" => (
            &["root", "path", "branch", "confirm_sessions"],
            &["root", "branch"],
        ),
        "/api/fs/git/branch" => (
            &[
                "root",
                "path",
                "name",
                "base",
                "checkout",
                "confirm_sessions",
            ],
            &["root", "name"],
        ),
        "git" => (&["action"], &["action"]),
        "git/reset" => (&["sha", "mode"], &["sha", "mode"]),
        _ => return true,
    };
    if map.keys().any(|k| !allowed.contains(&k.as_str()))
        || required.iter().any(|k| !body[*k].is_string())
    {
        return false;
    }
    if ["/api/fs/git/switch", "/api/fs/git/branch"].contains(&tail)
        && required
            .iter()
            .any(|k| body[*k].as_str().is_some_and(str::is_empty))
    {
        return false;
    }
    if ["path", "digest", "base"]
        .iter()
        .any(|k| map.get(*k).is_some_and(|v| !v.is_null() && !v.is_string()))
    {
        return false;
    }
    if ["checkout", "confirm_sessions"]
        .iter()
        .any(|k| map.get(*k).is_some_and(|v| !v.is_boolean()))
    {
        return false;
    }
    if ["amend", "switch_after"]
        .iter()
        .any(|k| map.get(*k).is_some_and(|v| !v.is_boolean()))
    {
        return false;
    }
    if ["sha", "new_branch", "escopo"]
        .iter()
        .any(|k| map.get(*k).is_some_and(|v| !v.is_null() && !v.is_string()))
    {
        return false;
    }
    if tail == "files/resolver"
        && !body["caminhos"]
            .as_array()
            .is_some_and(|paths| paths.iter().all(Value::is_string))
    {
        return false;
    }
    if tail == "git/commit"
        && map.get("paths").is_some_and(|v| {
            !v.as_array()
                .is_some_and(|paths| paths.iter().all(Value::is_string))
        })
    {
        return false;
    }
    if tail == "git"
        && ![
            "status",
            "pull",
            "fetch",
            "stash",
            "stash-pop",
            "log",
            "revert-abort",
            "cherry-pick-abort",
        ]
        .contains(&body["action"].as_str().unwrap_or(""))
    {
        return false;
    }
    if tail == "git/reset"
        && !["soft", "mixed", "hard"].contains(&body["mode"].as_str().unwrap_or(""))
    {
        return false;
    }
    true
}

fn map_operation(
    tail: &str,
    method: &Method,
    q: &HashMap<String, String>,
    body: &Value,
    ctx: &Value,
) -> Option<Operation> {
    let mut a = if *method == Method::POST {
        body.as_object()?.clone()
    } else {
        serde_json::Map::new()
    };
    if tail.starts_with("/api/fs/") {
        a.insert("roots".into(), ctx["roots"].clone());
        if *method == Method::GET {
            if let Some(root) = q.get("root") {
                a.insert("root".into(), json!(root));
            }
            if let Some(path) = q.get("path") {
                a.insert("path".into(), json!(path));
            }
        }
        return match tail {
            "/api/fs/roots" => operation("list_roots", json!({"roots":ctx["roots"]})),
            "/api/fs/scan" => operation("scan_dir", json!(a)),
            "/api/fs/mkdir" => operation("make_dir", json!(a)),
            _ => operation(
                "scan_dir",
                json!({"root":a.get("root")?,"path":a.get("path"),"roots":ctx["roots"]}),
            ),
        };
    }
    let cwd = ctx["session"]["cwd"].as_str()?;
    // O git segue o agente até a worktree; arquivos e citações continuam na pasta de abertura.
    let git = tail == "branches" || tail == "checkout" || tail == "git" || tail.starts_with("git/");
    a.insert("cwd".into(), json!(if git { git_cwd(ctx)? } else { cwd }));
    let op = match tail {
        "branches" => "list_branches",
        "checkout" => "switch_branch",
        "git" => "git_action",
        "git/files" => "changed_files",
        "git/last-message" => "last_commit_message",
        "git/log" => {
            let n = q
                .get("n")
                .map(|s| s.parse::<i64>())
                .transpose()
                .ok()?
                .unwrap_or(50)
                .clamp(1, 2000);
            a.insert("n".into(), json!(n));
            a.insert("grep".into(), json!(q.get("q")));
            "git_log"
        }
        "git/diff" => "file_diff",
        "git/path-diff" => {
            a.entry("escopo").or_insert(json!("branch"));
            "path_diff"
        }
        "git/discard" => "discard_file",
        "git/commit" => {
            if a.get("message")?.as_str()?.is_empty() {
                return None;
            }
            "commit"
        }
        "git/revert" => "revert_commit",
        "git/cherry-pick" => "cherry_pick",
        "git/push" => "push",
        "git/reset" => "reset_to",
        "git/branch" => "create_branch_at",
        "git/tag" => "create_tag",
        "files/list" => {
            a.insert("path".into(), json!(q.get("path")));
            a.insert(
                "so_modificados".into(),
                json!(bool_param(q.get("so_modificados"), true)?),
            );
            "list_dir"
        }
        "files/read" => {
            a.insert("path".into(), json!(q.get("path")?));
            "read_file"
        }
        "files/write" => {
            let text = a.remove("text")?;
            a.insert("texto".into(), text);
            let digest = a.remove("digest").unwrap_or(Value::Null);
            a.insert("digest_lido".into(), digest);
            "write_file"
        }
        "files/search" => {
            a.insert("q".into(), json!(q.get("q")?));
            a.insert(
                "mode".into(),
                json!(q.get("mode").map(String::as_str).unwrap_or("names")),
            );
            "search"
        }
        "files/resolver" => "resolver",
        "file" | "file/text" => {
            if tail == "file" {
                bool_param(q.get("download"), false)?;
            }
            let path = if *method == Method::POST {
                body.get("path")?.as_str()?
            } else {
                q.get("path")?
            };
            return operation(
                "resolve_cited",
                json!({"cwd":cwd,"jsonl":ctx["session"]["jsonl"].as_str()?,"path":path,"write":*method==Method::POST}),
            );
        }
        _ if tail.starts_with("git/commit/") => {
            let p = tail.split('/').collect::<Vec<_>>();
            a.insert("sha".into(), json!(p[2]));
            match p[3] {
                "files" => "commit_files",
                "diff" => {
                    a.insert("path".into(), json!(q.get("path")?));
                    "commit_file_diff"
                }
                "diff-full" => "commit_diff",
                "diff-worktree" => "diff_vs_worktree",
                "branches" => "branches_containing",
                _ => return None,
            }
        }
        _ => return None,
    };
    operation(op, json!(a))
}

async fn folder_operation(
    st: &Arc<AppState>,
    guard: Operation,
    tail: &str,
    ctx: &Value,
    body: &Value,
    q: &HashMap<String, String>,
) -> hangar_workspace::Result<Value> {
    // Como o api.py: recusa da pasta sai como erro_criacao_sessao; em /api/fs/branches, também a
    // do git. O código de reserva continua no campo `code` e não é afetado.
    let session_error = |e: hangar_workspace::WorkspaceError| hangar_workspace::WorkspaceError {
        detail: json!({"code":"erro_criacao_sessao","params":{},"msg":e.detail}),
        ..e
    };
    execute(st, guard).await.map_err(session_error)?;
    let root = if tail == "/api/fs/branches" || tail == "/api/fs/git" {
        q.get("root").map(String::as_str)
    } else {
        body["root"].as_str()
    }
    .unwrap_or("");
    let path = body["path"]
        .as_str()
        .or_else(|| q.get("path").map(String::as_str))
        .unwrap_or(root);
    let cwd = hangar_workspace::real(Path::new(path))
        .to_string_lossy()
        .into_owned();
    if tail == "/api/fs/branches" {
        return execute(st, Operation::ListBranches { cwd })
            .await
            .map_err(session_error);
    }
    let status = execute(st, Operation::FolderStatus { cwd: cwd.clone() }).await?;
    let affected = sessions(ctx, status["toplevel"].as_str());
    if tail != "/api/fs/git"
        && let Some(top) = status["toplevel"].as_str()
        && !hangar_workspace::real(Path::new(top))
            .starts_with(hangar_workspace::real(Path::new(root)))
    {
        return Err(hangar_workspace::error(
            400,
            "repositório fora da raiz autorizada",
        ));
    }
    let op = match tail {
        "/api/fs/git" => None,
        "/api/fs/git/fetch" => Some(Operation::FolderFetch { cwd }),
        "/api/fs/git/pull" => Some(Operation::FolderPull { cwd }),
        "/api/fs/git/switch" => Some(Operation::FolderSwitch {
            cwd,
            branch: body["branch"]
                .as_str()
                .ok_or_else(|| hangar_workspace::error(422, "branch required"))?
                .into(),
            sessions: affected,
            confirm_sessions: body["confirm_sessions"].as_bool().unwrap_or(false),
        }),
        "/api/fs/git/branch" => Some(Operation::FolderCreateBranch {
            cwd,
            name: body["name"]
                .as_str()
                .ok_or_else(|| hangar_workspace::error(422, "name required"))?
                .into(),
            base: body["base"].as_str().map(str::to_owned),
            checkout: body["checkout"].as_bool().unwrap_or(false),
            sessions: affected,
            confirm_sessions: body["confirm_sessions"].as_bool().unwrap_or(false),
        }),
        _ => None,
    };
    let mut result = if let Some(op) = op {
        execute(st, op).await?
    } else {
        status
    };
    if result["repo"] == true {
        result["sessions"] = json!(sessions(ctx, result["toplevel"].as_str()));
    }
    Ok(result)
}
fn resolve_many(
    cwd: &str,
    jsonl: Option<&str>,
    paths: &[String],
) -> hangar_workspace::Result<Value> {
    let cited = jsonl
        .map(|p| hangar_workspace::citations::cwds(hangar_workspace::citations::Transcript::File(Path::new(p)), paths))
        .unwrap_or(json!({}));
    let mut found = serde_json::Map::new();
    let mut outside = 0;
    for path in paths {
        let mut bases = cited[path]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !bases.iter().any(|b| b == cwd) {
            bases.push(cwd.into());
        }
        for suffix in [false, true] {
            for base in &bases {
                let result = hangar_workspace::files::resolver(
                    Path::new(base),
                    std::slice::from_ref(path),
                    suffix,
                )?;
                if let Some(mut target) = result["ok"].get(path).cloned() {
                    if hangar_workspace::real(Path::new(base))
                        != hangar_workspace::real(Path::new(cwd))
                    {
                        target["relativo"] = Value::Null;
                    }
                    found.insert(path.clone(), target);
                    break;
                }
            }
            if found.contains_key(path) {
                break;
            }
        }
        if !found.contains_key(path) && cited.get(path).is_some() && outside < 30 {
            outside += 1;
            if let Some(jsonl) = jsonl
                && let Some(p) = hangar_workspace::citations::find_elsewhere(
                    hangar_workspace::citations::Transcript::File(Path::new(jsonl)),
                    Path::new(cwd),
                    path,
                    &bases,
                    true,
                )
            {
                found.insert(
                    path.clone(),
                    json!({"relativo":null,"real":p.to_string_lossy()}),
                );
            }
        }
    }
    Ok(
        json!({"faltam":paths.iter().filter(|p|!found.contains_key(*p)).collect::<Vec<_>>(),"ok":found}),
    )
}

enum Piece {
    Bytes(Bytes),
    Data { offset: u64, left: u64 },
}
fn file_stream(file: tokio::fs::File, pieces: VecDeque<Piece>) -> Body {
    Body::from_stream(futures_util::stream::try_unfold(
        (file, pieces),
        |(mut file, mut pieces)| async move {
            match pieces.pop_front() {
                None => Ok::<_, std::io::Error>(None),
                Some(Piece::Bytes(bytes)) => Ok(Some((bytes, (file, pieces)))),
                Some(Piece::Data { offset, left }) => {
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    let mut bytes = vec![0; (left.min(64 * 1024)) as usize];
                    let n = file.read(&mut bytes).await?;
                    if n == 0 {
                        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
                    }
                    bytes.truncate(n);
                    if left > n as u64 {
                        pieces.push_front(Piece::Data {
                            offset: offset + n as u64,
                            left: left - n as u64,
                        });
                    }
                    Ok(Some((Bytes::from(bytes), (file, pieces))))
                }
            }
        },
    ))
}
pub(crate) fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
enum RangeError {
    Malformed(&'static str),
    Unsatisfiable,
}

fn ranges(value: &str, size: u64) -> Result<Vec<(u64, u64)>, RangeError> {
    let (units, text) = value
        .split_once('=')
        .ok_or(RangeError::Malformed("Malformed range header."))?;
    if !units.trim().eq_ignore_ascii_case("bytes") {
        return Err(RangeError::Malformed("Only support bytes range"));
    }
    let mut out = Vec::new();
    for part in text.split(',') {
        let Some((a, b)) = part.trim().split_once('-') else {
            continue;
        };
        let (a, b) = (a.trim(), b.trim());
        if a.is_empty() && b.is_empty() {
            continue;
        }
        let parsed = if a.is_empty() {
            b.parse::<i128>().ok().map(|count| {
                (
                    i128::from(size).saturating_sub(count).max(0),
                    i128::from(size),
                )
            })
        } else {
            a.parse::<i128>().ok().and_then(|start| {
                if b.is_empty() {
                    Some((start, i128::from(size)))
                } else {
                    b.parse::<i128>()
                        .ok()
                        .map(|end| (start, end.saturating_add(1).min(i128::from(size))))
                }
            })
        };
        if let Some(range) = parsed {
            out.push(range);
        }
    }
    if out.is_empty() {
        return Err(RangeError::Malformed(
            "Range header: range must be requested",
        ));
    }
    if out
        .iter()
        .any(|(start, _)| *start < 0 || *start >= i128::from(size))
    {
        return Err(RangeError::Unsatisfiable);
    }
    if out.iter().any(|(start, end)| start > end) {
        return Err(RangeError::Malformed(
            "Range header: start must be less than end",
        ));
    }
    out.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in out {
        let (start, end) = (start as u64, end as u64);
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    Ok(merged)
}
async fn serve_file(path: &Path, headers: &HeaderMap, download: bool) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return response(json!({"detail":"file not found"}), 404),
    };
    serve_open_file(file, path, headers, download, None).await
}

pub(crate) async fn serve_open_file(file: tokio::fs::File, path: &Path, headers: &HeaderMap, download: bool, media: Option<&str>) -> Response {
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    let representation = if download { "download" } else { "isolated" };
    let etag = format!("\"{representation}-{:x}-{size:x}\"", modified.as_nanos());
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|h| h == etag.as_str())
    {
        let mut r = StatusCode::NOT_MODIFIED.into_response();
        r.headers_mut().insert(header::ETAG, etag.parse().unwrap());
        r.headers_mut()
            .insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
        return r;
    }
    let guessed = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let media = media.unwrap_or(&guessed).to_owned();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let html = !download && ["text/html", "application/xhtml+xml"].contains(&media.as_str());
    // Como o Starlette: texto declara utf-8, e o invólucro do HTML é sempre text/html.
    let content_type = if html {
        "text/html; charset=utf-8".to_owned()
    } else if media.starts_with("text/") {
        format!("{media}; charset=utf-8")
    } else {
        media.clone()
    };
    let mut r = Response::new(Body::empty());
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(header::ETAG, etag.parse().unwrap());
    h.insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    if download {
        h.insert(
            "content-security-policy",
            "sandbox; script-src 'none'".parse().unwrap(),
        );
        // O `quote` do Python, como o Starlette: qualquer byte fora dele vai em `filename*`, e
        // caractere de controle nunca chega cru ao cabeçalho.
        const QUOTE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'_')
            .remove(b'.')
            .remove(b'-')
            .remove(b'~')
            .remove(b'/');
        let encoded = percent_encoding::utf8_percent_encode(&name, QUOTE).to_string();
        let disposition = if encoded == name {
            format!("attachment; filename=\"{name}\"")
        } else {
            format!("attachment; filename*=utf-8''{encoded}")
        };
        h.insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());
    }
    if html {
        use base64::Engine;
        let title = html_escape(&name);
        let prefix = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><style>html,body{{margin:0;height:100%;overflow:hidden}}iframe{{display:block;width:100%;height:100%;border:0}}</style></head><body><iframe title=\"{title}\" sandbox=\"allow-scripts allow-popups\" referrerpolicy=\"no-referrer\" src=\"data:{media};charset=utf-8;base64,"
        );
        let suffix = b"\"></iframe></body></html>";
        let body = Body::from_stream(futures_util::stream::try_unfold(
            (file, 0u8, Vec::<u8>::new(), prefix),
            move |(mut file, phase, mut remainder, prefix)| async move {
                if phase == 0 {
                    return Ok::<_, std::io::Error>(Some((
                        Bytes::from(prefix.clone()),
                        (file, 1, remainder, prefix),
                    )));
                }
                if phase == 2 {
                    return Ok(None);
                }
                let mut bytes = vec![0; 48 * 1024];
                let n = file.read(&mut bytes).await?;
                if n == 0 {
                    let tail = base64::engine::general_purpose::STANDARD.encode(&remainder)
                        + std::str::from_utf8(suffix).unwrap();
                    return Ok(Some((Bytes::from(tail), (file, 2, Vec::new(), prefix))));
                }
                bytes.truncate(n);
                remainder.extend(bytes);
                let cut = remainder.len() / 3 * 3;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&remainder[..cut]);
                let rest = remainder.split_off(cut);
                Ok(Some((Bytes::from(encoded), (file, 1, rest, prefix))))
            },
        ));
        *r.body_mut() = body;
        return r;
    }
    if ["image/svg+xml", "application/xml", "text/xml"].contains(&media.as_str()) {
        r.headers_mut().insert(
            "content-security-policy",
            "sandbox; script-src 'none'".parse().unwrap(),
        );
    }
    r.headers_mut()
        .insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    let last = chrono::DateTime::<chrono::Utc>::from_timestamp(modified.as_secs() as i64, 0)
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default();
    if let Ok(h) = last.parse() {
        r.headers_mut().insert(header::LAST_MODIFIED, h);
    }
    let use_range = headers
        .get(header::IF_RANGE)
        .is_none_or(|h| h == etag.as_str() || h == last.as_str());
    let range = if use_range {
        headers.get(header::RANGE).and_then(|h| h.to_str().ok())
    } else {
        None
    };
    let mut pieces = VecDeque::new();
    if let Some(range) = range {
        let ranges = match ranges(range, size) {
            Ok(ranges) => ranges,
            Err(RangeError::Malformed(message)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    message,
                )
                    .into_response();
            }
            Err(RangeError::Unsatisfiable) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        (header::CONTENT_RANGE, format!("bytes */{size}")),
                        (header::CONTENT_TYPE, "text/plain; charset=utf-8".into()),
                    ],
                    "",
                )
                    .into_response();
            }
        };
        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
        if ranges.len() == 1 {
            let (start, end) = ranges[0];
            r.headers_mut().insert(
                header::CONTENT_RANGE,
                format!("bytes {start}-{}/{size}", i128::from(end) - 1)
                    .parse()
                    .unwrap(),
            );
            r.headers_mut().insert(
                header::CONTENT_LENGTH,
                (end - start).to_string().parse().unwrap(),
            );
            if end > start {
                pieces.push_back(Piece::Data {
                    offset: start,
                    left: end - start,
                });
            }
        } else {
            let boundary = "hangar-workspace-range";
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                format!("multipart/byteranges; boundary={boundary}")
                    .parse()
                    .unwrap(),
            );
            for (start, end) in ranges {
                pieces.push_back(Piece::Bytes(Bytes::from(format!("--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{size}\r\n\r\n", i128::from(end)-1))));
                if end > start {
                    pieces.push_back(Piece::Data {
                        offset: start,
                        left: end - start,
                    });
                }
                pieces.push_back(Piece::Bytes(Bytes::from_static(b"\r\n")));
            }
            pieces.push_back(Piece::Bytes(Bytes::from(format!("--{boundary}--\r\n"))));
        }
    } else {
        r.headers_mut()
            .insert(header::CONTENT_LENGTH, size.to_string().parse().unwrap());
        if size > 0 {
            pieces.push_back(Piece::Data {
                offset: 0,
                left: size,
            });
        }
    }
    *r.body_mut() = file_stream(file, pieces);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_follows_the_agent_worktree_and_files_stay_in_the_opening_folder() {
        let ctx = json!({"session":{"name":"s","cwd":"/repo","jsonl":"/repo/s.jsonl","git_cwd":"/repo-x"}});
        let q = HashMap::new();
        let op = |tail: &str| format!("{:?}", map_operation(tail, &Method::GET, &q, &json!({}), &ctx).unwrap());
        assert!(op("git/files").contains("\"/repo-x\""));
        assert!(op("branches").contains("\"/repo-x\""));
        assert!(op("files/list").contains("cwd: \"/repo\""));
    }
}
