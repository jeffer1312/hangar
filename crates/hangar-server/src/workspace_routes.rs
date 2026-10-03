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
fn private_ok(st: &AppState, peer: SocketAddr, headers: &HeaderMap) -> bool {
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
async fn execute(st: &Arc<AppState>, op: Operation) -> hangar_workspace::Result<Value> {
    let slots = if matches!(op, Operation::HeadInfo { .. } | Operation::BranchOf { .. }) {
        &st.workspace_meta_slots
    } else if op.is_mutation() {
        &st.workspace_slots
    } else {
        &st.workspace_read_slots
    };
    let permit = slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| hangar_workspace::error(503, "operação indisponível"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hangar_workspace::execute(op)
    })
    .await
    .map_err(|_| {
        hangar_workspace::error(
            503,
            "Não foi possível confirmar o resultado. Confira o estado antes de repetir.",
        )
    })?
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
async fn context(st: &AppState, name: Option<&str>) -> Option<Value> {
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
    .ok()?;
    let resp = tokio::time::timeout(Duration::from_secs(10), st.http.request(req))
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = tokio::time::timeout(Duration::from_secs(10), resp.into_body().collect())
        .await
        .ok()?
        .ok()?
        .to_bytes();
    if bytes.len() > MAX_BODY {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn query(req: &Request) -> HashMap<String, String> {
    form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect()
}
fn bool_param(value: Option<&String>, default: bool) -> Option<bool> {
    match value.map(|s| s.to_lowercase()).as_deref() {
        None => Some(default),
        Some("1" | "true" | "on" | "yes") => Some(true),
        Some("0" | "false" | "off" | "no") => Some(false),
        _ => None,
    }
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

pub async fn public(st: Arc<AppState>, req: Request, forward: Forward) -> Response {
    let route = req.uri().path().to_owned();
    let params = query(&req);
    let method = req.method().clone();
    let name = route
        .strip_prefix("/api/sessions/")
        .and_then(|p| p.split_once('/').map(|(n, _)| n))
        .and_then(|n| {
            percent_encoding::percent_decode_str(n)
                .decode_utf8()
                .ok()
                .map(|s| s.into_owned())
        });
    let Some(ctx) = context(&st, name.as_deref()).await else {
        tracing::warn!(
            code = "workspace_context_unavailable",
            "metadados indisponíveis; reserva Python"
        );
        return pass(&st, req, &forward).await;
    };
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return response(json!({"detail":"request body too large"}), 413),
    };
    let payload = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => v,
            _ => return pass(&st, Request::from_parts(parts, Body::from(bytes)), &forward).await,
        }
    };
    let tail = if name.is_some() {
        route
            .strip_prefix("/api/sessions/")
            .unwrap()
            .split_once('/')
            .unwrap()
            .1
    } else {
        route.as_str()
    };
    if method == Method::POST && !body_valid(tail, &payload) {
        return pass(&st, Request::from_parts(parts, Body::from(bytes)), &forward).await;
    }
    let Some(op) = map_operation(tail, &method, &params, &payload, &ctx) else {
        return pass(&st, Request::from_parts(parts, Body::from(bytes)), &forward).await;
    };
    if tail == "file" {
        match execute(&st, op).await {
            Ok(path) => {
                return serve_file(
                    Path::new(path.as_str().unwrap_or("")),
                    &parts.headers,
                    bool_param(params.get("download"), false).unwrap_or(false),
                )
                .await;
            }
            Err(e) => return failure(e, tail),
        }
    }
    let result = if tail == "files/resolver" {
        let cwd = ctx["session"]["cwd"].as_str().unwrap_or("").to_owned();
        let jsonl = ctx["session"]["jsonl"].as_str().map(str::to_owned);
        let paths = payload["caminhos"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let permit = st
            .workspace_read_slots
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            resolve_many(&cwd, jsonl.as_deref(), &paths)
        })
        .await
        .unwrap_or_else(|_| Err(hangar_workspace::error(503, "operação indisponível")))
    } else if tail == "file/text" {
        match execute(&st, op).await {
            Ok(path) => {
                let path = path.as_str().unwrap_or("");
                let args = if method == Method::POST {
                    json!({"alvo":path,"path":payload["path"],"texto":payload["text"],"digest_lido":payload["digest"]})
                } else {
                    json!({"alvo":path,"path":params.get("path")})
                };
                if let Some(next) = operation(
                    if method == Method::POST {
                        "write_at"
                    } else {
                        "read_at"
                    },
                    args,
                ) {
                    execute(&st, next).await
                } else {
                    return pass(&st, Request::from_parts(parts, Body::from(bytes)), &forward)
                        .await;
                }
            }
            Err(e) => Err(e),
        }
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
        folder_operation(&st, op, tail, &ctx, &payload, &params).await
    } else {
        execute(&st, op).await
    };
    match result {
        Err(e) => failure(e, tail),
        Ok(mut result) => {
            if tail == "git/files" {
                let cwd = ctx["session"]["cwd"].as_str().unwrap_or("");
                let sequencer = execute(&st, Operation::SequencerState { cwd: cwd.into() }).await;
                match sequencer {
                    Ok(s) => result = json!({"files":result,"sequencer":s}),
                    Err(e) => return failure(e, tail),
                }
            }
            if tail == "git/log" {
                if params.get("q").is_none_or(|q| q.is_empty()) {
                    result = hangar_workspace::git::lanes(
                        result.as_array().cloned().unwrap_or_default(),
                    );
                }
                let summary = execute(
                    &st,
                    Operation::GitSummary {
                        cwd: ctx["session"]["cwd"].as_str().map(str::to_owned),
                    },
                )
                .await
                .unwrap_or(Value::Null);
                result =
                    json!({"commits":result,"ahead":summary["ahead"],"behind":summary["behind"]});
            }
            if tail.starts_with("git/commit/") && tail.ends_with("/files") {
                result = json!({"files":result});
            }
            response(result, 200)
        }
    }
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
    a.insert("cwd".into(), json!(cwd));
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
    execute(st, guard).await?;
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
        return execute(st, Operation::ListBranches { cwd }).await;
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
        .map(|p| hangar_workspace::citations::cwds(Path::new(p), paths))
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
                    Path::new(jsonl),
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
fn html_escape(text: &str) -> String {
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
    let media = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut r = Response::new(Body::empty());
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, media.parse().unwrap());
    h.insert(header::ETAG, etag.parse().unwrap());
    h.insert(header::CACHE_CONTROL, "max-age=60".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    if download {
        h.insert(
            "content-security-policy",
            "sandbox; script-src 'none'".parse().unwrap(),
        );
        let encoded =
            percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let disposition = if name.is_ascii() && !name.contains(['"', '\r', '\n']) {
            format!("attachment; filename=\"{name}\"")
        } else {
            format!("attachment; filename*=utf-8''{encoded}")
        };
        h.insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());
    }
    if !download && ["text/html", "application/xhtml+xml"].contains(&media.as_str()) {
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
