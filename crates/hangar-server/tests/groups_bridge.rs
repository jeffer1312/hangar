//! Ponte privada dos grupos (`/__hangar_server/groups`): o que o Python pede ao Rust no modo
//! `rust`/`pending`, quando só o Rust grava `.hangar-pair`.
#![cfg(unix)]
mod common;
mod fake;
mod list_support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::{OWNER, SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::{GroupService, JoinOwned};
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router, terminal_router};
use serde_json::{Value, json};

struct Server { public: SocketAddr, private: SocketAddr, python: Arc<fake::Fake>, groups: Arc<GroupService>, pair: PathBuf }

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    addr
}

/// `n` sessões Claude (`s0`..), a porta pública e a privada do mesmo estado.
async fn server(root: &Path, n: usize) -> Server {
    let script = list_support::sessions(root, n);
    let home = root.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let pair = home.join(".claude/.hangar-pair");
    let groups = Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")), Arc::new(PythonOrq::from_state(&state)), "casa".into()));
    state.groups = Some(groups.clone());
    let state = Arc::new(state);
    let public = serve(router(state.clone())).await;
    let private = serve(terminal_router(state)).await;
    Server { public, private, python, groups, pair }
}

async fn bridge(addr: SocketAddr, op: &str, args: Value, secret: Option<&str>) -> (u16, Value) {
    let mut req = client().post(format!("http://{addr}/__hangar_server/groups")).header("content-type", "application/json")
        .body(json!({"op": op, "args": args}).to_string());
    if let Some(secret) = secret { req = req.header("x-hangar-internal", secret); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
}

async fn result(srv: &Server, op: &str, args: Value) -> Value {
    let (status, body) = bridge(srv.private, op, args, Some(SECRET)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{op}: {body}");
    body["result"].clone()
}

async fn public(srv: &Server, method: &str, path: &str, body: Option<&Value>) -> Value {
    let mut req = client().request(method.parse().unwrap(), format!("http://{}/api/sessions/{path}", srv.public)).bearer_auth(OWNER);
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    json!({"status": status, "body": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))})
}

async fn group(srv: &Server, name: &str, others: &[&str]) {
    let harness: BTreeMap<String, String> = std::iter::once(name).chain(others.iter().copied()).map(|n| (n.to_owned(), "claude".to_owned())).collect();
    srv.groups.join(JoinOwned { name: name.into(), others: others.iter().map(|o| (*o).to_owned()).collect(), task: String::new(),
        replace_task: false, harness, orq: false }).await.ok().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_needs_the_secret_and_the_private_port() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    assert_eq!(bridge(srv.private, "group.leave", json!({"name": "s0"}), None).await.0, 404);
    assert_eq!(bridge(srv.private, "group.leave", json!({"name": "s0"}), Some("outro")).await.0, 404);
    assert_eq!(bridge(srv.public, "group.leave", json!({"name": "s0"}), Some(SECRET)).await.0, 404, "a porta pública não tem a ponte");
    assert_eq!(bridge(srv.private, "group.nada", json!({}), Some(SECRET)).await.0, 400);
    let health: Value = serde_json::from_str(&client().get(format!("http://{}/__hangar_server/health", srv.public)).send().await.unwrap()
        .text().await.unwrap()).unwrap();
    assert_eq!(health["groups"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn leave_returns_ex_peers() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    group(&srv, "s0", &["s1"]).await;
    assert_eq!(result(&srv, "group.leave", json!({"name": "s0"})).await, json!({"ex_peers": ["s1"], "warnings": []}));
    assert!(!srv.pair.join("s0.json").exists() && !srv.pair.join("s1.json").exists(), "grupo de 1 não existe");
    assert_eq!(result(&srv, "group.leave", json!({"name": "s0"})).await, json!({"ex_peers": [], "warnings": []}), "idempotente");
}

#[tokio::test(flavor = "multi_thread")]
async fn route_answers_like_the_public_route() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    let refused = json!({"peer": "s0"});
    let via_bridge = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": "pair", "body": refused})).await;
    assert_eq!(via_bridge, public(&srv, "POST", "s0/pair", Some(&refused)).await);
    assert_eq!(via_bridge["status"], 400);
    let paired = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": "pair",
        "body": {"peer": "s1", "peers": [], "task": "t", "replace_task": false, "notify_members": true, "orq": false}})).await;
    assert_eq!(paired["status"], 200, "{paired}");
    assert_eq!(paired["body"]["members"], json!(["s0", "s1"]));
    let contract = result(&srv, "group.route", json!({"method": "GET", "name": "s1", "route": "contract", "body": null})).await;
    assert_eq!(contract, public(&srv, "GET", "s1/pair/contract", None).await);
    let left = result(&srv, "group.route", json!({"method": "DELETE", "name": "s1", "route": "pair", "body": null})).await;
    assert_eq!(left["status"], 200, "{left}");
    assert!(!srv.pair.join("s0.json").exists());
}

/// O que a rota repassaria ao Python volta como erro: o Python, ao receber, pediria à ponte de novo.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_request_never_goes_back_to_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    for (route, body) in [("pair", json!({"peer": 1})), ("group-message", json!({"texto": "x"})), ("pair-remote", json!({})), ("unpair-remote", json!(null))] {
        let out = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": route, "body": body})).await;
        assert_eq!(out["status"], 500, "{route}: {out}");
        assert_eq!(out["body"]["detail"]["code"], "erro_grupo_indisponivel", "{route}");
        assert_eq!(out["body"]["detail"]["params"]["detalhe"], "groups_bridge_relay", "{route}");
    }
    assert_eq!(srv.python.hits_to("/api/sessions/s0/pair") + srv.python.hits_to("/api/sessions/s0/group-message")
        + srv.python.hits_to("/api/sessions/s0/pair-remote") + srv.python.hits_to("/api/sessions/s0/unpair-remote"), 0);
    let (status, body) = bridge(srv.private, "group.route", json!({"method": "PUT", "name": "s0", "route": "pair", "body": null}), Some(SECRET)).await;
    assert_eq!((status, body["ok"].clone(), body["error"]["code"].clone()), (200, json!(false), json!("groups_bridge_invalid_request")));
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_and_external_pair_ops() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    group(&srv, "s0", &["s1"]).await;
    assert_eq!(result(&srv, "group.rename", json!({"old": "s0", "new": "s9"})).await, json!({}));
    let s1: Value = serde_json::from_str(&std::fs::read_to_string(srv.pair.join("s1.json")).unwrap()).unwrap();
    assert_eq!(s1["peers"], json!(["s9"]));
    let linked = result(&srv, "group.external_link", json!({"local": "s2", "address": "fora::x", "harness": {"s2": "claude"}})).await;
    let gid = linked["gid"].as_str().unwrap();
    assert!(gid.len() == 8 && gid.bytes().all(|b| b.is_ascii_hexdigit()), "{linked}");
    let (status, refused) = bridge(srv.private, "group.external_link", json!({"local": "s1", "address": "fora::y", "harness": {}}), Some(SECRET)).await;
    assert_eq!((status, refused["error"]["code"].clone()), (200, json!("erro_pareamento_mistura_cross")), "sessão já agrupada");
    assert_eq!(result(&srv, "group.external_unlink", json!({"local": "s2", "address": "fora::x"})).await, json!({}));
    assert!(!srv.pair.join("s2.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn orq_associate_holds_the_group_lock() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    group(&srv, "s0", &["s1"]).await;
    srv.python.set_internal("orq/associate", StatusCode::OK, json!({"ok": true, "gid": "g1", "grouped": true}));
    *srv.python.internal_delay.lock().unwrap() = Duration::from_millis(600);
    let private = srv.private;
    let associating = tokio::spawn(async move {
        bridge(private, "group.orq_associate", json!({"name": "arb", "gid": "g1", "mtime": 1.5}), Some(SECRET)).await
    });
    let python = srv.python.clone();
    fake::wait_until(move || !python.internal_bodies("orq/associate").is_empty()).await;
    assert!(tokio::time::timeout(Duration::from_millis(200), srv.groups.leave("s0")).await.is_err(), "a saída espera a associação");
    let (status, body) = associating.await.unwrap();
    assert_eq!((status, body), (200, json!({"ok": true, "result": {"status": 200, "body": {"ok": true, "gid": "g1", "grouped": true}}})));
    assert_eq!(srv.python.internal_bodies("orq/associate"), vec![json!({"name": "arb", "gid": "g1", "mtime": 1.5})]);
    *srv.python.internal_delay.lock().unwrap() = Duration::ZERO;
    srv.python.set_internal("orq/associate", StatusCode::CONFLICT, json!({"detail": {"code": "erro_orq_celula_invalida", "params": {}, "msg": "x"}}));
    let refused = result(&srv, "group.orq_associate", json!({"name": "arb", "gid": "g1", "mtime": 1.5})).await;
    assert_eq!(refused, json!({"status": 409, "body": {"detail": {"code": "erro_orq_celula_invalida", "params": {}, "msg": "x"}}}));
    srv.groups.leave("s0").await.ok().unwrap();
}
