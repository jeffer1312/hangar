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

struct Server { addr: SocketAddr, python: Arc<fake::Fake>, groups: Arc<GroupService>, pair: PathBuf, runtime: Arc<RuntimeRegistry>,
    share: Arc<hangar_server::share::ShareState> }

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
    let share = crate::share_pair_routes::share_state(&root.join("shares.json"), &pair, Arc::new(std::sync::Mutex::new(1_000.0)));
    state.share = Some(share.clone());
    // Sem entrada aberta: só a porta de entrada de cada nome conta. A política nunca é chamada.
    let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
    assert!(state.state.runtime.set(runtime.clone()).is_ok());
    state.write_gate_wait = Duration::from_secs(10);
    state.hangar_dir = root.join("hangar");
    let state = Arc::new(state);
    let app = router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, python, groups, pair, runtime, share }
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
            // I-1A: o 422 é do Rust, com código; o Python nunca recebe o corpo.
            assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")), "{name}");
            assert_eq!(srv.python.hits_to(&format!("/api/sessions/{}", step["path"].as_str().unwrap())), 0, "{name}");
            continue;
        }
        if gid.is_empty() && let Some(g) = body["gid"].as_str() {
            assert!(g.len() == 8 && g.bytes().all(|b| b.is_ascii_hexdigit()), "{name}: gid {g}");
            gid = g.to_owned();
        }
        // A leitura do contrato passou a trazer a dona e o `mtime` (gravar exige o lido); o golden do Python não os tem.
        let mut body = body;
        if step["path"].as_str().unwrap().ends_with("/pair/contract") && status == 200 {
            let map = body.as_object_mut().unwrap();
            assert_eq!(map.remove("owner"), Some(json!("casa")), "{name}");
            assert!(map.remove("mtime").is_some_and(|m| m.is_string()), "{name}");
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
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"], "task": "t"})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [1, 1, 0]);
    // O texto sai do Rust: o Python não é perguntado.
    let gid = body["gid"].as_str().unwrap();
    let contract = srv.pair.join(format!("grupo-{gid}.md")).to_str().unwrap().to_owned();
    let sent: Value = serde_json::from_slice(&srv.python.last_body()).unwrap();
    let text = sent["text"].as_str().unwrap();
    assert!(text.starts_with("[painel: grupo de trabalho] Você, 's1', está num grupo de trabalho na tarefa: t."), "{text}");
    assert!(text.contains(&format!("vão em {contract} ")) && text.contains("hangar-send s0 \"msg\""), "{text}");
    assert!(srv.python.internal_bodies("pair/text").is_empty());
    let (status, _) = call(srv.addr, "POST", "s2/pair", Some(&json!({"peer": "s0"})), OWNER).await;
    assert_eq!(status, 200);
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [1, 1, 1], "veterano não é acordado");
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_pair_marks_the_child_records_linked_at_and_sends_no_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"peer": "s1", "link": true})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [0, 0, 0], "ninguém recebe o protocolo");
    assert!(srv.python.internal_bodies("pair/text").is_empty());
    let child: Value = serde_json::from_str(&std::fs::read_to_string(srv.pair.join("s1.json")).unwrap()).unwrap();
    assert_eq!(child["linked_by"], "s0");
    let parent: Value = serde_json::from_str(&std::fs::read_to_string(srv.pair.join("s0.json")).unwrap()).unwrap();
    assert!(parent.get("linked_by").is_none());
    let returns = hangar_server::groups::link_state::read(&dir.path().join("hangar")).unwrap();
    assert!(returns["s1"].linked_at > 0.0 && returns["s1"].returned_at.is_none() && !returns.contains_key("s0"));
    // Remoto não liga.
    let (status, body) = call(srv.addr, "POST", "s2/pair", Some(&json!({"peer": "lab::x", "link": true})), OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_link_remoto")));
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
/// (preso na varredura da lista) espera a gravação: o grupo nunca fica com o nome velho por cima.
#[tokio::test(flavor = "multi_thread")]
async fn rename_closing_the_gate_waits_for_the_join_write() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    std::fs::write(dir.path().join("slow"), "").unwrap();
    let addr = srv.addr;
    let pairing = tokio::spawn(async move { call(addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await });
    let root = dir.path().to_owned();
    fake::wait_until(move || list_support::calls(&root, "list-panes") > 0).await;
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
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_peer_nao_registrado")));
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

/// Grupo misto: sessão local entra no grupo que tem membro de fora; pedir o de fora pelo nome é
/// entrar no grupo da ponta daqui; máquina própria continua sem mistura.
#[tokio::test(flavor = "multi_thread")]
async fn local_session_joins_a_group_with_an_outside_member() {
    use hangar_server::share::pairs::ExternalPair;
    use hangar_server::share::store::Kind;
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 4).await;
    let (s, _) = srv.share.store.create_redeemed("s0", Some("t:1".into()), Kind::Pair).unwrap();
    srv.share.pairs.add(ExternalPair { share_id: s.id, local_session: "s0".into(), alias: "pc-ana".into(), peer_owner: "pc-ana".into(),
        peer_session: "Y".into(), peer_address: "https://a.tail.ts.net:8443".into(), peer_token: "t".repeat(32), created_at: 1.0 }).unwrap();
    srv.groups.external_link("s0", "pc-ana::Y", Default::default(), ["pc-ana::Y".to_owned()].into()).await.unwrap();

    let (status, body) = call(srv.addr, "POST", "s1/pair", Some(&json!({"peers": ["s0"]})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["members"], json!(["s1", "s0", "pc-ana::Y"]));
    assert_eq!(srv.groups.link("s1").await.unwrap().unwrap().peers, ["s0", "pc-ana::Y"]);

    let (status, body) = call(srv.addr, "POST", "s2/pair", Some(&json!({"peers": ["pc-ana::Y"]})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.groups.link("s2").await.unwrap().unwrap().peers, ["s0", "s1", "pc-ana::Y"]);

    let (status, body) = call(srv.addr, "POST", "s3/pair", Some(&json!({"peers": ["s0"], "orq": true})), OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_orq_par_externo")));
    assert!(srv.groups.link("s3").await.unwrap().is_none());
    // Máquina própria agora entra pelo grupo entre máquinas; `lab` fora do peers.json recusa antes de gravar.
    for (name, peers) in [("s3", json!(["lab::x", "s0"])), ("s0", json!(["lab::x"]))] {
        let (status, body) = call(srv.addr, "POST", &format!("{name}/pair"), Some(&json!({"peers": peers})), OWNER).await;
        assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_peer_nao_registrado")), "{name}");
    }
    assert!(srv.groups.link("s3").await.unwrap().is_none());
}

fn seed_outside(srv: &Server, local: &str, alias: &str, peer: &str, address: &str) {
    use hangar_server::share::pairs::ExternalPair;
    use hangar_server::share::store::Kind;
    let (s, _) = srv.share.store.create_redeemed(local, Some("t:1".into()), Kind::Pair).unwrap();
    srv.share.pairs.add(ExternalPair { share_id: s.id, local_session: local.into(), alias: alias.into(), peer_owner: alias.into(),
        peer_session: peer.into(), peer_address: address.into(), peer_token: format!("tok-{alias}-{}", "t".repeat(30)), created_at: 1.0 }).unwrap();
}

fn write_group(srv: &Server, name: &str, peers: &[&str]) {
    std::fs::create_dir_all(&srv.pair).unwrap();
    std::fs::write(srv.pair.join(format!("{name}.json")), json!({"peers": peers, "task": "t", "gid": "abababab", "harness": {}}).to_string()).unwrap();
}

/// Aviso local num grupo com dois pares de fora: cada máquina de fora recebe uma vez, pela chave
/// do próprio par, com `session` para o Hangar antigo; os locais recebem como antes.
#[tokio::test(flavor = "multi_thread")]
async fn group_notice_reaches_each_outside_pair_once() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    let (ana, ana_base) = crate::share_pair_routes::other_machine().await;
    let (bia, bia_base) = crate::share_pair_routes::other_machine().await;
    seed_outside(&srv, "s0", "pc-ana", "Y", &ana_base);
    seed_outside(&srv, "s1", "pc-bia", "W", &bia_base);
    write_group(&srv, "s0", &["s1", "pc-ana::Y", "pc-bia::W"]);
    write_group(&srv, "s1", &["s0", "pc-ana::Y", "pc-bia::W"]);
    let (status, body) = call(srv.addr, "POST", "s0/group-message", Some(&json!({"text": "oi"})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"], Value::Null, "{body}");
    assert_eq!(input_hits(&srv.python, &["s0", "s1", "s2"]), [0, 1, 0]);
    for (other, session, alias) in [(&ana, "Y", "pc-ana"), (&bia, "W", "pc-bia")] {
        let calls = other.calls();
        assert_eq!(calls.len(), 1, "{alias}: {calls:?}");
        assert_eq!(calls[0]["path"], "/api/pair/message");
        let mut body = calls[0]["body"].clone();
        assert!(body.as_object_mut().unwrap().remove("notice").is_some_and(|n| n.as_str().is_some_and(|n| !n.is_empty())), "{body}");
        assert_eq!(body, json!({"text": "oi", "from": "s0", "group": true, "session": session}));
        assert_eq!(calls[0]["auth"], json!(format!("Bearer tok-{alias}-{}", "t".repeat(30))));
    }
}

/// Dois pares com a mesma máquina: um POST por par (os grupos de lá podem ser outros), todos com
/// o mesmo `notice`, que o outro lado usa para não entregar duas vezes à mesma sessão.
#[tokio::test(flavor = "multi_thread")]
async fn group_notice_to_two_pairs_of_one_machine_carries_one_notice_id() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    let (ana, ana_base) = crate::share_pair_routes::other_machine().await;
    seed_outside(&srv, "s0", "pc-ana", "Y", &ana_base);
    seed_outside(&srv, "s1", "pc-ana", "Z", &ana_base);
    write_group(&srv, "s0", &["s1", "pc-ana::Y", "pc-ana::Z"]);
    write_group(&srv, "s1", &["s0", "pc-ana::Y", "pc-ana::Z"]);
    let (status, body) = call(srv.addr, "POST", "s0/group-message", Some(&json!({"text": "oi"})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    let calls = ana.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0]["body"]["notice"].is_string(), "{calls:?}");
    assert_eq!(calls[0]["body"]["notice"], calls[1]["body"]["notice"]);
}

/// Recado vindo de fora não volta a ser espalhado como aviso de grupo.
#[tokio::test(flavor = "multi_thread")]
async fn group_notice_refuses_outside_header() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let (ana, ana_base) = crate::share_pair_routes::other_machine().await;
    seed_outside(&srv, "s0", "pc-ana", "Y", &ana_base);
    write_group(&srv, "s0", &["s1", "pc-ana::Y"]);
    write_group(&srv, "s1", &["s0", "pc-ana::Y"]);
    let (status, body) = call(srv.addr, "POST", "s0/group-message", Some(&json!({"text": "  [de fora: pc-ana::Y] oi"})), OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_group_message_resposta")));
    assert!(ana.calls().is_empty());
    assert_eq!(input_hits(&srv.python, &["s0", "s1"]), [0, 0]);
}

/// BRG-10/PAIR-12: corpo que o Rust não aceita é 422 com código daqui, nunca repasse ao Python.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_field_is_422_without_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    for (path, body) in [("s0/pair", json!({"peers": ["s1"], "bogus": 1})), ("s0/group-message", json!({"texto": "x"})),
        ("s0/pair-remote", json!({"initiator": 1})), ("s0/unpair-remote", json!({}))] {
        let (status, body) = call(srv.addr, "POST", path, Some(&body), OWNER).await;
        assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")), "{path}");
        assert_eq!(srv.python.hits_to(&format!("/api/sessions/{path}")), 0, "{path}");
    }
    assert!(!srv.pair.join("s0.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn surrogate_task_is_422() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let resp = client().post(format!("http://{}/api/sessions/s0/pair", srv.addr)).bearer_auth(OWNER)
        .header("content-type", "application/json").body(r#"{"peers": ["s1"], "task": "meio \ud83d emoji"}"#).send().await.unwrap();
    let status = resp.status().as_u16();
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")));
    assert_eq!(srv.python.hits_to("/api/sessions/s0/pair"), 0);
    assert!(!srv.pair.join("s0.json").exists());
}

/// BRG-25: fechamento que o Rust só repassa (Pi/omp/Kimi, sessão sem entrada aqui) sai do grupo
/// daqui, depois que o Python fechou; o Python não pede mais pela ponte.
#[tokio::test(flavor = "multi_thread")]
async fn repassed_close_leaves_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 3).await;
    write_group(&srv, "s0", &["s1", "s2"]);
    write_group(&srv, "s1", &["s0", "s2"]);
    write_group(&srv, "s2", &["s0", "s1"]);
    let (status, _) = call(srv.addr, "DELETE", "s0", None, OWNER).await;
    assert_eq!(status, 200);
    assert_eq!(srv.python.hits_to("/api/sessions/s0"), 1, "o Python fecha");
    assert!(!srv.pair.join("s0.json").exists(), "quem fechou saiu");
    assert_eq!(srv.groups.link("s1").await.unwrap().unwrap().peers, ["s2"]);
    assert_eq!(srv.groups.link("s2").await.unwrap().unwrap().peers, ["s1"]);
}

/// PAIR-24/47: `orq` sem companheiro cria o grupo de um; quem entra depois herda a marca e o grupo
/// `orq` não recebe protocolo.
#[tokio::test(flavor = "multi_thread")]
async fn orq_pair_without_peer_creates_a_group_of_one_without_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let (status, body) = call(srv.addr, "POST", "s0/pair", Some(&json!({"orq": true})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    let arb = srv.groups.link("s0").await.unwrap().unwrap();
    assert!(arb.orq && arb.peers.is_empty());
    let (status, body) = call(srv.addr, "POST", "s1/pair", Some(&json!({"peer": "s0"})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert!(srv.groups.link("s1").await.unwrap().unwrap().orq);
    assert_eq!(input_hits(&srv.python, &["s0", "s1"]), [0, 0]);
}

/// Contrato ausente é vazio (como no Python); ilegível é 500 com código, nunca "vazio".
#[tokio::test(flavor = "multi_thread")]
async fn unreadable_contract_is_500_with_code() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    assert_eq!(call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await.0, 200);
    let (status, body) = call(srv.addr, "GET", "s0/pair/contract", None, OWNER).await;
    assert_eq!((status, body["content"].clone()), (200, json!("")), "{body}");
    let gid = srv.groups.link("s0").await.unwrap().unwrap().gid;
    std::fs::create_dir_all(srv.groups.contract_path(&gid)).unwrap();
    let (status, body) = call(srv.addr, "GET", "s0/pair/contract", None, OWNER).await;
    assert_eq!((status, body["detail"]["code"].clone(), body["detail"]["params"]["detalhe"].clone()),
        (500, json!("erro_grupo_indisponivel"), json!("groups_contract_unreadable")), "{body}");
}

/// O acesso de quem é de fora ao membro novo que não grava aparece na resposta, com código.
#[tokio::test(flavor = "multi_thread")]
async fn outside_grant_failure_is_a_coded_warning() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    seed_outside(&srv, "s0", "pc-ana", "Y", "https://a.tail.ts.net:8443");
    srv.groups.external_link("s0", "pc-ana::Y", Default::default(), ["pc-ana::Y".to_owned()].into()).await.unwrap();
    // Convite revogado com o registro do par ainda no disco: dar o acesso falha.
    let id = srv.share.pairs.all()[0].share_id.clone();
    srv.share.store.revoke(&id).unwrap();
    let (status, body) = call(srv.addr, "POST", "s1/pair", Some(&json!({"peers": ["s0"]})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_acesso_externo_falhou", "{body}");
}

/// Grupo de uma máquina: a leitura traz o `mtime` e a dona, e gravar com esse `mtime` passa.
#[tokio::test(flavor = "multi_thread")]
async fn local_contract_read_then_write_with_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    assert_eq!(call(srv.addr, "POST", "s0/pair", Some(&json!({"peers": ["s1"]})), OWNER).await.0, 200);
    let gid = srv.groups.link("s0").await.unwrap().unwrap().gid;
    std::fs::write(srv.groups.contract_path(&gid), "antes\n").unwrap();
    let (status, read) = call(srv.addr, "GET", "s0/pair/contract", None, OWNER).await;
    assert_eq!((status, read["content"].clone(), read["owner"].clone()), (200, json!("antes\n"), json!("casa")), "{read}");
    assert!(read["mtime"].is_string(), "{read}");
    let (status, body) = call(srv.addr, "PUT", "s0/pair/contract", Some(&json!({"content": "depois\n", "mtime": read["mtime"]})), OWNER).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(std::fs::read_to_string(srv.groups.contract_path(&gid)).unwrap(), "depois\n");
}
