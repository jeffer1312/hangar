//! Ponte privada dos grupos (`/__hangar_server/groups`): o que o Python pede ao Rust no modo
//! `rust`/`pending`, quando só o Rust grava `.hangar-pair`.
#![cfg(unix)]
use crate::fake;
use crate::list_support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fake::{SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::GroupService;
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
    state.accounts = hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map(
            [("HOME".into(),home.to_string_lossy().into_owned()),
             ("USERPROFILE".into(),home.to_string_lossy().into_owned())].into()));
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into())));
    let pair = home.join(".claude/.hangar-pair");
    let groups = Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")), Arc::new(PythonOrq::from_state(&state).with_paths(home.clone(),pair.clone())), "casa".into()));
    state.groups = Some(groups.clone());
    let state = Arc::new(state);
    let public = serve(router(state.clone())).await;
    let private = serve(terminal_router(state)).await;
    Server { public, private, python, groups, pair }
}

/// Fatos enviados são ignorados: o Rust consulta o cofre da máquina.
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

#[tokio::test(flavor = "multi_thread")]
async fn bridge_needs_the_secret_and_the_private_port() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let leave = json!({"name": "s0", "phases": {}});
    assert_eq!(bridge(srv.private, "group.leave", leave.clone(), None).await.0, 404);
    assert_eq!(bridge(srv.private, "group.leave", leave.clone(), Some("outro")).await.0, 404);
    assert_eq!(bridge(srv.public, "group.leave", leave, Some(SECRET)).await.0, 404, "a porta pública não tem a ponte");
    assert_eq!(bridge(srv.private, "group.nada", json!({}), Some(SECRET)).await.0, 400);
    let health: Value = serde_json::from_str(&client().get(format!("http://{}/__hangar_server/health", srv.public)).send().await.unwrap()
        .text().await.unwrap()).unwrap();
    assert_eq!(health["groups"], true);
}

/// R4 (contrato c): a operação `group.route` saiu com as rotas de grupo do Python; pedi-la é a recusa
/// de operação desconhecida, como `group.nada`.
#[tokio::test(flavor = "multi_thread")]
async fn r4_16_group_route_operation_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let route = json!({"method": "POST", "name": "s0", "route": "pair", "body": {"peer": "s1"}, "facts": {}});
    assert_eq!(bridge(srv.private, "group.route", route, Some(SECRET)).await.0, 400);
    assert!(!srv.pair.join("s0.json").exists(), "nada pareado pela ponte");
}

/// Fechamento pela ponte usa a execução do cofre, sem confiar na fase enviada nem consultar Python.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_leave_never_calls_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    std::fs::create_dir_all(&srv.pair).unwrap();
    for (me, other) in [("s0", "s1"), ("s1", "s0")] {
        std::fs::write(srv.pair.join(format!("{me}.json")), json!({"peers": [other], "task": "t", "gid": "g1", "harness": {}, "orq": true}).to_string()).unwrap();
    }
    native_run(&srv, "g1", false);
    let out = result(&srv, "group.leave", json!({"name": "s0", "phases": {"g1": "ended"}})).await;
    assert_eq!(out, json!({"ex_peers": ["s1"], "warnings": []}));
    assert!(!srv.pair.join("s0.json").exists());
    assert!(srv.pair.join("s1.json").exists(), "execução viva mantém o grupo orq de um");
    assert!(srv.python.internal_bodies("orq/group-phase").is_empty(), "a fase foi lida no cofre");

    // Grupo entre máquinas desta dona que se dissolve: o aviso de saída ao membro sem dono no Rust (o
    // pane dele é só shell) sai depois da saída, em processo, e a recusa vira aviso do fechamento.
    let fed = json!({"owner": "casa", "local": true, "version": 1});
    for (me, other) in [("s0", "s1"), ("s1", "s0")] {
        std::fs::write(srv.pair.join(format!("{me}.json")), json!({"peers": [other], "task": "t", "gid": "g2",
            "harness": {"s1": "pi"}, "fed": fed}).to_string()).unwrap();
    }
    std::fs::create_dir_all(srv.pair.join("groups")).unwrap();
    std::fs::write(srv.pair.join("groups/g2.json"), json!({"protocol": 1, "gid": "g2", "owner": "casa", "version": 1, "task": "t",
        "members": [{"server": "casa", "name": "s0", "provider": "claude"}, {"server": "casa", "name": "s1", "provider": "pi"}], "outside": []}).to_string()).unwrap();
    let out = result(&srv, "group.leave", json!({"name": "s0", "phases": {}})).await;
    let unbound = "vínculo gerenciado indisponível; escrita suspensa";
    assert_eq!(out["warnings"], json!([{"sessao": "s1", "erro": {"code": "erro_envio_falhou", "params": {"erro": unbound}, "msg": unbound}}]), "{out}");
    assert!(out.get("deliveries").is_none(), "{out}");
    assert_eq!(srv.python.input_calls(), 0, "o Rust não chamou o Python");
    assert!(!srv.pair.join("s1.json").exists(), "ficou sozinho: o grupo acabou");
}

fn mtime(path: &Path) -> f64 {
    std::fs::metadata(path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

/// A associação resolve a vida e o grupo localmente, sob o lock, sem chamar o Python de volta.
#[tokio::test(flavor = "multi_thread")]
async fn orq_associate_never_calls_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    std::fs::create_dir_all(&srv.pair).unwrap();
    std::fs::write(srv.pair.join("arbiter.json"), json!({"peers": [], "task": "obra", "gid": "real", "harness": {}, "orq": true}).to_string()).unwrap();
    let home = srv.pair.parent().unwrap().parent().unwrap();
    native_identity(home,"planner");
    let gid = hangar_server::orchestration::context::draft(home,"planner","unused").unwrap();
    let (draft, real) = (srv.pair.join(format!("regras-{gid}.md")), srv.pair.join("regras-real.md"));
    std::fs::write(&draft, "| papel | conta |\n").unwrap();
    let facts = |gid: &str| json!({"active_gid": null, "draft_gid": "draft-p", "groups": {gid: {"phase": null, "archived": false}}});
    let args = |gid: &str, mtime: f64| json!({"name": "planner", "gid": gid, "mtime": mtime, "facts": facts(gid)});

    let missing = result(&srv, "group.orq_associate", args("missing", mtime(&draft))).await;
    assert_eq!((missing["status"].clone(), missing["body"]["detail"]["code"].clone()), (json!(409), json!("erro_orq_celula_invalida")));
    let stale = result(&srv, "group.orq_associate", args("real", 1.5)).await;
    assert_eq!((stale["status"].clone(), stale["body"]["detail"]["code"].clone()), (json!(409), json!("erro_orq_arquivo_mudou")));
    assert!(!real.exists(), "a recusa não move o time");

    // O lock de grupo segura a associação: uma saída ou um join concorrente não a vê pela metade.
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let groups = srv.groups.clone();
    let holder = tokio::spawn(async move { groups.locked(async { let _ = locked_tx.send(()); held.await.ok() }).await });
    locked_rx.await.unwrap();
    let private = srv.private;
    let ask = args("real", mtime(&draft));
    let mut associating = tokio::spawn(async move { bridge(private, "group.orq_associate", ask, Some(SECRET)).await });
    assert!(tokio::time::timeout(Duration::from_millis(200), &mut associating).await.is_err(), "a associação espera o lock");
    release.send(()).unwrap();
    holder.await.unwrap();
    let (status, body) = associating.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let done = &body["result"];
    assert_eq!(done["status"], 200, "{done}");
    assert_eq!((done["body"]["ok"].clone(), done["body"]["gid"].clone(), done["body"]["grouped"].clone(), done["body"]["session_prefix"].clone()),
        (json!(true), json!("real"), json!(true), json!("planner")));
    assert_eq!(done["body"]["arquivo"], json!(real.to_str().unwrap()));
    assert!(done["body"]["mtime"].as_f64().unwrap() > 0.0);
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "| papel | conta |\n");
    assert_eq!(std::fs::read_to_string(&draft).unwrap(), "<!-- hangar-orq-group: real -->\n");
    assert!(srv.python.internal_bodies("orq/associate").is_empty() && srv.python.internal_bodies("orq/group-phase").is_empty(),
        "nenhuma pergunta de volta ao Python");
}

fn native_identity(home: &Path, name: &str) {
    let root = home.join(".hangar/claude-headless");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(format!("{name}.json")),json!({"key":format!("key-{name}")}).to_string()).unwrap();
}
fn native_run(srv: &Server, gid: &str, ended: bool) {
    let home = srv.pair.parent().unwrap().parent().unwrap();
    let root = hangar_server::orchestration::runs::root(home).join(gid);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("orq.json"),"{\"auto\":true}").unwrap();
    let mut events = json!({"tipo":"execucao_inicio","gid":gid}).to_string()+"\n";
    if ended {events += "{\"tipo\":\"execucao_fim\"}\n";}
    std::fs::write(root.join("eventos.jsonl"),events).unwrap();
    std::fs::write(root.join("vigia.json"),json!({"ts":chrono::Utc::now().to_rfc3339(),"pid":std::process::id()}).to_string()).unwrap();
}
