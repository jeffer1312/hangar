//! Rotas de grupo de uma máquina no Rust (`/pair`, `DELETE /pair`, `/group-message`,
//! `/pair/contract`) com as respostas do Python: o golden `group_routes.json` roda a mesma sequência
//! nas rotas do FastAPI. As sessões são Claude de mentira (tmux falso da lista); o aviso vai ao `/input`
//! do Python falso, porque nenhuma delas tem entrada no runtime do Rust.
#![cfg(unix)]
use crate::common;
use crate::fake;
use crate::list_support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fake::{OWNER, SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::GroupService;
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

struct Server { addr: SocketAddr, python: Arc<fake::Fake>, groups: Arc<GroupService>, pair: PathBuf, runtime: Arc<RuntimeRegistry> }

/// `n` sessões Claude (`s0`..) e o roteador de grupo montado na frente do principal.
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
    // Sem entrada aberta: só a porta de entrada de cada nome conta. A política nunca é chamada.
    let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
    assert!(state.state.runtime.set(runtime.clone()).is_ok());
    state.write_gate_wait = Duration::from_secs(10);
    let state = Arc::new(state);
    let app = router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, python, groups, pair, runtime }
}

async fn call(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>, token: &str) -> (u16, Value) {
    let mut req = client().request(method.parse().unwrap(), format!("http://{addr}/api/sessions/{path}")).bearer_auth(token);
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn input_hits(python: &fake::Fake, names: &[&str]) -> Vec<usize> {
    names.iter().map(|n| python.hits_to(&format!("/api/sessions/{n}/input"))).collect()
}

fn orchestrators(python: &fake::Fake, names: &Value) {
    python.set_internal("orq/is-orchestrator", StatusCode::OK, json!({"names": names}));
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_answer_like_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 4).await;
    let steps = common::golden("group_routes.json");
    let golden_gid = "9a9a9a9a";
    let names = ["s0", "s1", "s2", "s3"];
    let mut gid = String::new();
    for step in steps.as_array().unwrap() {
        let name = step["name"].as_str().unwrap();
        orchestrators(&srv.python, step.get("orchestrators").unwrap_or(&json!([])));
        if let Some(text) = step["contract"].as_str() {
            std::fs::write(srv.pair.join(format!("grupo-{gid}.md")), text).unwrap();
        }
        let before = input_hits(&srv.python, &names);
        let (status, body) = call(srv.addr, step["method"].as_str().unwrap(), step["path"].as_str().unwrap(), step.get("body"), OWNER).await;
        if step["python_only"] == true {
            assert_eq!((status, body), (200, json!("from-python")), "{name}: o corpo que o FastAPI recusa vai a ele");
            assert_eq!(serde_json::from_slice::<Value>(&srv.python.last_body()).unwrap(), step["body"], "{name}: corpo intacto");
            continue;
        }
        if gid.is_empty() && let Some(g) = body["gid"].as_str() {
            assert!(g.len() == 8 && g.bytes().all(|b| b.is_ascii_hexdigit()), "{name}: gid {g}");
            gid = g.to_owned();
        }
        let actual = common::canon(&body).replace(&gid, golden_gid).replace(srv.pair.to_str().unwrap(), "<pair>");
        assert_eq!((status as u64, actual), (step["status"].as_u64().unwrap(), common::canon(&step["response"])), "{name}");
        let after = input_hits(&srv.python, &names);
        let delivered: Vec<&str> = names.iter().zip(before.iter().zip(&after)).filter(|(_, (b, a))| a > b).map(|(n, _)| *n).collect();
        assert_eq!(json!(delivered), step["delivered"], "{name}: quem recebeu recado");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_delivers_protocol_only_to_newcomers() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    srv.python.set_internal("pair/text", StatusCode::OK, json!({"text": "protocolo do grupo"}));
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"], "task": "t"})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [1, 1, 0]);
    assert_eq!(serde_json::from_slice::<Value>(&srv.python.last_body()).unwrap(), json!({"text": "protocolo do grupo", "steer": false}));
    let gid = body["gid"].as_str().unwrap();
    let contract = srv.pair.join(format!("grupo-{gid}.md")).to_str().unwrap().to_owned();
    assert_eq!(srv.python.internal_bodies("pair/text"), vec![
        json!({"kind": "group", "me": "s0", "others": ["s1"], "task": "t", "contract": contract, "contract_remote": false,
            "harness": {"s0": "claude", "s1": "claude"}, "peer": "", "owner": ""}),
        json!({"kind": "group", "me": "s1", "others": ["s0"], "task": "t", "contract": contract, "contract_remote": false,
            "harness": {"s0": "claude", "s1": "claude"}, "peer": "", "owner": ""}),
    ]);
    let (status, _) = call(srv.addr, "POST", "s2/pair", Some(&json!({"peer": "s0"})), OWNER).await;
    assert_eq!(status, 200);
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [1, 1, 1], "veterano não é acordado");
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_is_undone_when_no_notice_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let refusal = json!({"code": "erro_fila_nao_digitada", "params": {}, "msg": "composer ilegível"});
    srv.python.set_input_reply(Some((StatusCode::BAD_REQUEST, json!({"detail": refusal.clone()}))));
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await;
    assert_eq!(status, 502, "{body}");
    assert_eq!(body["detail"], json!({"code": "erro_pareamento_desfeito",
        "params": {"avisos": [{"sessao": "s0", "erro": refusal}, {"sessao": "s1", "erro": refusal}]},
        "msg": "pareamento desfeito: falha ao avisar as sessões (s0: composer ilegível; s1: composer ilegível)"}));
    let left: Vec<_> = std::fs::read_dir(&srv.pair).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "nenhum sidecar: {left:?}");
}

/// Ninguém avisado e a volta atrás que falha no disco: o 500 do Python (o `pair.restore` dele
/// levantava), nunca "pareamento desfeito".
#[tokio::test(flavor = "multi_thread")]
async fn pair_whose_undo_fails_is_500_not_undone() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let refusal = json!({"code": "erro_fila_nao_digitada", "params": {}, "msg": "composer ilegível"});
    srv.python.set_input_reply(Some((StatusCode::BAD_REQUEST, json!({"detail": refusal}))));
    srv.python.hold_input(true);
    let addr = srv.addr;
    let pairing = tokio::spawn(async move { call(addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await });
    let (s0, s1) = (srv.pair.join("s0.json"), srv.pair.join("s1.json"));
    fake::wait_until(move || s0.is_file() && s1.is_file()).await;
    // O temporário de s0 vira pasta: a restauração não consegue apagá-lo.
    std::fs::create_dir(srv.pair.join("s0.json.tmp")).unwrap();
    srv.python.hold_input(false);
    srv.python.release.notify_one();
    assert_eq!(pairing.await.unwrap(), (500, json!("Internal Server Error")));
    let python = srv.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_restore_failed")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_notice_failure_is_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    srv.runtime.ingress().hold("s1", Duration::from_millis(10)).await.unwrap();
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_pareamento_aviso_parcial");
    assert_eq!(body["warning"]["params"]["avisos"][0]["sessao"], "s1");
    assert_eq!(body["warning"]["params"]["avisos"][0]["erro"]["code"], "session_transfer_busy");
    assert!(body["warning"]["msg"].as_str().unwrap().starts_with("aviso falhou em: s1: "));
    assert!(srv.pair.join("s0.json").is_file() && srv.pair.join("s1.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn notice_waiting_on_closed_ingress_does_not_block_rename() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    srv.runtime.ingress().close("s1", Duration::from_millis(10)).await.unwrap();
    let addr = srv.addr;
    let pairing = tokio::spawn(async move { call(addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await });
    let s1 = srv.pair.join("s1.json");
    fake::wait_until(move || s1.is_file()).await;
    tokio::time::timeout(Duration::from_secs(2), srv.groups.rename("c", "c2")).await
        .expect("o grupo não fica preso na entrega").unwrap();
    assert!(!pairing.is_finished(), "o /pair ainda espera a porta de s1");
    srv.runtime.ingress().open("s1");
    let (status, body) = pairing.await.unwrap();
    assert_eq!((status, body["warning"].clone()), (200, Value::Null));
}

/// Um rename que fecha a porta de s0 enquanto o `/pair` dela está entre a conferência e a gravação
/// (preso nos fatos da lista) espera a gravação: o grupo nunca fica com o nome velho por cima.
#[tokio::test(flavor = "multi_thread")]
async fn rename_closing_the_gate_waits_for_the_join_write() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    srv.python.list_facts.lock().unwrap().1 = Duration::from_millis(800);
    let addr = srv.addr;
    let pairing = tokio::spawn(async move { call(addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await });
    let python = srv.python.clone();
    fake::wait_until(move || python.list_facts_calls.load(std::sync::atomic::Ordering::SeqCst) > 0).await;
    assert!(!srv.pair.join("s0.json").exists(), "o /pair ainda não gravou");
    srv.runtime.ingress().close("s0", Duration::from_secs(5)).await.expect("o /pair solta a porta depois de gravar");
    assert!(srv.pair.join("s0.json").is_file(), "o fechamento esperou o join gravar");
    srv.groups.rename("s0", "s0x").await.unwrap();
    srv.runtime.ingress().open("s0");
    let (status, body) = pairing.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let s1: Value = serde_json::from_str(&std::fs::read_to_string(srv.pair.join("s1.json")).unwrap()).unwrap();
    assert_eq!(s1["peers"], json!(["s0x"]), "o companheiro aponta para o nome novo");
    assert!(!srv.pair.join("s0.json").exists() && srv.pair.join("s0x.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_ingress_of_the_caller_is_busy() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    srv.runtime.ingress().hold("s0", Duration::from_millis(10)).await.unwrap();
    for (method, path, body) in [("POST", "s0/pair", Some(json!({"peers": ["s1"]}))), ("DELETE", "s0/pair", None),
                                 ("POST", "s0/group-message", Some(json!({"text": "oi"})))] {
        let (status, reply) = call(srv.addr, method, path, body.as_ref(), OWNER).await;
        assert_eq!((status, reply["detail"]["code"].clone()), (409, json!("session_transfer_busy")), "{method} {path}");
    }
    // O contrato não confere a troca, como no Python.
    assert_eq!(call(srv.addr, "GET", "s0/pair/contract", None, OWNER).await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_goes_to_python_and_cross_machine_stays_in_rust() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), "outro").await;
    assert_eq!((status, body), (200, json!("from-python")));
    // Sem `peers.json`, a máquina `lab` é desconhecida: recusa limpa, nada gravado.
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["lab::x"]})), OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone()), (502, json!("erro_pareamento_rejeitado")));
    assert!(!srv.pair.join("s0.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn orchestrator_lookup_failure_is_503() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    srv.python.set_internal("orq/is-orchestrator", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone()), (503, json!("erro_lista_indisponivel")));
    assert!(!srv.pair.join("s0.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn uncertain_promotion_restores_and_goes_to_the_diary() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    srv.python.set_internal("orq/promote", StatusCode::CONFLICT,
        json!({"detail": {"code": "erro_orq_arquivo_mudou", "params": {}, "msg": "o time já pertence a outro grupo"}}));
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"orq": true})), OWNER).await;
    assert_eq!((status, body["detail"]["msg"].clone()), (409, json!("o time já pertence a outro grupo")));
    srv.python.set_internal("orq/promote", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"orq": true})), OWNER).await;
    // Python sem resposta não é "o arquivo mudou": indisponível, com o código.
    assert_eq!((status, body["detail"]["code"].clone(), body["detail"]["params"]["detalhe"].clone()),
        (503, json!("erro_grupo_indisponivel"), json!("groups_orq_promote_status_500")));
    assert!(!srv.pair.join("s0.json").exists(), "o join volta atrás nos dois casos");
    let python = srv.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_orq_promote_uncertain"
        && d["codigo"] == "groups_orq_promote_status_500")).await;
    assert_eq!(srv.python.diag().iter().filter(|d| d["evento"] == "rust.groups_orq_promote_uncertain").count(), 1,
        "o conflito do Python não é incerteza");
}
