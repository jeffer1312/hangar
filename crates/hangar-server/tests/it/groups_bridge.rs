//! Ponte privada dos grupos (`/__hangar_server/groups`): o que o Python pede ao Rust no modo
//! `rust`/`pending`, quando só o Rust grava `.hangar-pair`.
#![cfg(unix)]
use crate::fake;
use crate::list_support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

/// Sem fatos dados, a rota leva os fatos vazios que o Python mandaria sem orquestração ativa.
async fn bridge(addr: SocketAddr, op: &str, mut args: Value, secret: Option<&str>) -> (u16, Value) {
    if op == "group.route" && args.get("facts").is_none() { args["facts"] = json!({"orchestrators": [], "phases": {}}); }
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

/// `{status, body}` da rota, sem o que a ponte devolve ao Python para ele fazer depois.
fn answered(mut out: Value) -> Value {
    if let Some(map) = out.as_object_mut() { map.remove("deliveries"); map.remove("post"); }
    out
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
        replace_task: false, harness, orq: false, outside: Default::default(), linked: false }).await.ok().unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn route_answers_like_the_public_route() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    let refused = json!({"peer": "s0"});
    let via_bridge = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": "pair", "body": refused})).await;
    assert_eq!(answered(via_bridge.clone()), public(&srv, "POST", "s0/pair", Some(&refused)).await);
    assert_eq!(via_bridge["status"], 400);
    let paired = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": "pair",
        "body": {"peer": "s1", "peers": [], "task": "t", "replace_task": false, "notify_members": true, "orq": false}})).await;
    assert_eq!(paired["status"], 200, "{paired}");
    assert_eq!(paired["body"]["members"], json!(["s0", "s1"]));
    let contract = result(&srv, "group.route", json!({"method": "GET", "name": "s1", "route": "contract", "body": null})).await;
    assert_eq!(answered(contract), public(&srv, "GET", "s1/pair/contract", None).await);
    let left = result(&srv, "group.route", json!({"method": "DELETE", "name": "s1", "route": "pair", "body": null})).await;
    assert_eq!(left["status"], 200, "{left}");
    assert!(!srv.pair.join("s0.json").exists());
}

/// Corpo que a rota recusa é 422 daqui: o Python, ao receber, pediria à ponte de novo.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_request_never_goes_back_to_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    for (route, body) in [("pair", json!({"peer": 1})), ("group-message", json!({"texto": "x"})), ("pair-remote", json!({})), ("unpair-remote", json!(null))] {
        let out = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": route, "body": body})).await;
        assert_eq!(out["status"], 422, "{route}: {out}");
        assert_eq!(out["body"]["detail"]["code"], "erro_corpo_invalido", "{route}");
    }
    assert_eq!(srv.python.hits_to("/api/sessions/s0/pair") + srv.python.hits_to("/api/sessions/s0/group-message")
        + srv.python.hits_to("/api/sessions/s0/pair-remote") + srv.python.hits_to("/api/sessions/s0/unpair-remote"), 0);
    let (status, body) = bridge(srv.private, "group.route", json!({"method": "PUT", "name": "s0", "route": "pair", "body": null}), Some(SECRET)).await;
    assert_eq!((status, body["ok"].clone(), body["error"]["code"].clone()), (200, json!(false), json!("groups_bridge_invalid_request")));
}

/// Fechamento que não passou pelo `delete` do Rust (Connect, convidado): a fase do `orq` vem no
/// pedido e a saída não pergunta nada de volta ao Python.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_leave_never_calls_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    std::fs::create_dir_all(&srv.pair).unwrap();
    for (me, other) in [("s0", "s1"), ("s1", "s0")] {
        std::fs::write(srv.pair.join(format!("{me}.json")), json!({"peers": [other], "task": "t", "gid": "g1", "harness": {}, "orq": true}).to_string()).unwrap();
    }
    let out = result(&srv, "group.leave", json!({"name": "s0", "phases": {"g1": "live"}})).await;
    assert_eq!(out, json!({"ex_peers": ["s1"], "warnings": [], "deliveries": []}));
    assert!(!srv.pair.join("s0.json").exists());
    assert!(srv.pair.join("s1.json").exists(), "execução viva mantém o grupo orq de um");
    assert!(srv.python.internal_bodies("orq/group-phase").is_empty(), "a fase veio no pedido");

    // Grupo entre máquinas desta dona que se dissolve: o aviso de saída ao membro Pi, que só o
    // Python atende, volta na resposta em vez de o Rust chamar o `/input` dele.
    let fed = json!({"owner": "casa", "local": true, "version": 1});
    for (me, other) in [("s0", "s1"), ("s1", "s0")] {
        std::fs::write(srv.pair.join(format!("{me}.json")), json!({"peers": [other], "task": "t", "gid": "g2",
            "harness": {"s1": "pi"}, "fed": fed}).to_string()).unwrap();
    }
    std::fs::create_dir_all(srv.pair.join("groups")).unwrap();
    std::fs::write(srv.pair.join("groups/g2.json"), json!({"protocol": 1, "gid": "g2", "owner": "casa", "version": 1, "task": "t",
        "members": [{"server": "casa", "name": "s0", "provider": "claude"}, {"server": "casa", "name": "s1", "provider": "pi"}], "outside": []}).to_string()).unwrap();
    let out = result(&srv, "group.leave", json!({"name": "s0", "phases": {}})).await;
    let deliveries = out["deliveries"].as_array().unwrap();
    assert_eq!(deliveries.len(), 1, "{out}");
    assert_eq!(deliveries[0]["name"], "s1");
    assert!(deliveries[0]["text"].as_str().unwrap().contains("Você saiu do grupo de trabalho entre máquinas"), "{out}");
    assert_eq!(srv.python.hits_to("/api/sessions/s1/input"), 0, "o Rust não chamou o Python dentro da ponte");
    assert!(!srv.pair.join("s1.json").exists(), "ficou sozinho: o grupo acabou");
}

/// O `PUT /pair/contract` que chega pela porta do Connect segue pela ponte como as outras rotas.
#[tokio::test(flavor = "multi_thread")]
async fn contract_write_goes_through_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    group(&srv, "s0", &["s1"]).await;
    let written = result(&srv, "group.route", json!({"method": "PUT", "name": "s0", "route": "contract", "body": {"content": "x\n", "mtime": null}})).await;
    assert_eq!(written["status"], 200, "{written}");
    let read = result(&srv, "group.route", json!({"method": "GET", "name": "s1", "route": "contract", "body": null})).await;
    assert_eq!((read["body"]["content"].clone(), read["body"]["mtime"].clone()), (json!("x\n"), written["body"]["mtime"].clone()), "{read}");
}

fn mtime(path: &Path) -> f64 {
    std::fs::metadata(path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

/// BRG-20 (I-5A): o Python manda os fatos do `orq` no corpo e o Rust associa o time sob o lock de
/// grupo, sem chamar o Python de volta.
#[tokio::test(flavor = "multi_thread")]
async fn orq_associate_never_calls_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    std::fs::create_dir_all(&srv.pair).unwrap();
    std::fs::write(srv.pair.join("arbiter.json"), json!({"peers": [], "task": "obra", "gid": "real", "harness": {}, "orq": true}).to_string()).unwrap();
    let (draft, real) = (srv.pair.join("regras-draft-p.md"), srv.pair.join("regras-real.md"));
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

/// Rota que chega pela ponte do Python (8768): os fatos do `orq` vêm no pedido, e o que só o Python
/// faz (entregar a sessão que ele atende, promover o time) volta na resposta. Nenhuma chamada ao
/// Python durante a rota.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_route_never_calls_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 4).await;
    let calls = |p: &fake::Fake| ["orq/is-orchestrator", "orq/group-phase", "orq/promote"].iter().map(|r| p.internal_bodies(r).len()).sum::<usize>()
        + p.list_facts_calls.load(std::sync::atomic::Ordering::SeqCst) + p.input_calls();
    let before = calls(&srv.python);
    let facts = json!({"orchestrators": ["s3"], "phases": {}});
    let refused = result(&srv, "group.route", json!({"method": "POST", "name": "s3", "route": "pair", "body": {"peers": ["s0"]}, "facts": facts})).await;
    assert_eq!((refused["status"].clone(), refused["body"]["detail"]["code"].clone()), (json!(409), json!("erro_sessao_orq")), "o fato do Python decide");
    let paired = result(&srv, "group.route", json!({"method": "POST", "name": "s0", "route": "pair", "body": {"peers": ["s1"]}, "facts": facts})).await;
    assert_eq!(paired["status"], 200, "{paired}");
    let mut told: Vec<&str> = paired["deliveries"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    told.sort();
    assert_eq!(told, ["s0", "s1"], "o protocolo volta ao Python para entregar");
    let gid = paired["body"]["gid"].as_str().unwrap().to_owned();
    let left = result(&srv, "group.route", json!({"method": "DELETE", "name": "s1", "route": "pair", "body": null,
        "facts": {"orchestrators": [], "phases": {gid: "ended"}}})).await;
    assert_eq!(left["status"], 200, "{left}");
    assert_eq!(left["deliveries"][0]["name"], "s1", "o aviso de saída também");
    let orq = result(&srv, "group.route", json!({"method": "POST", "name": "s2", "route": "pair", "body": {"orq": true}, "facts": facts})).await;
    assert_eq!(orq["status"], 200, "{orq}");
    assert_eq!((orq["post"][0]["op"].clone(), orq["post"][0]["name"].clone()), (json!("promote"), json!("s2")), "a promoção fica com o Python");
    // A saída de um grupo `orq` usa a fase que o Python mandou.
    let orq_gid = orq["body"]["gid"].as_str().unwrap().to_owned();
    let gone = result(&srv, "group.route", json!({"method": "DELETE", "name": "s2", "route": "pair", "body": null,
        "facts": {"orchestrators": [], "phases": {orq_gid: "ended"}}})).await;
    assert_eq!(gone["status"], 200, "{gone}");
    assert!(!srv.pair.join("s2.json").exists(), "execução acabada: o grupo orq sozinho sai");
    assert_eq!(calls(&srv.python), before, "nenhuma chamada ao Python dentro da rota da ponte");
}
