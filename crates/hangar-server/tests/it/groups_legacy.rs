//! Par 1:1 entre máquinas no Rust com as respostas do Python: o golden `group_cross_routes.json`
//! roda a mesma sequência nas rotas do FastAPI com `casa`, `lab` e `anon` (sem id) no mesmo processo.
//! Aqui são três servidores de verdade, cada um com a pasta, o id e o `peers.json` dele; `lab` é
//! alcançado por um repasse que anota o pedido e `off` aceita a conexão e a derruba sem responder.
#![cfg(unix)]
use crate::common;
use crate::fake;
use crate::list_support;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use fake::{OWNER, SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::peers::{PeerBook, PeerClient};
use hangar_server::groups::service::GroupService;
use hangar_server::groups::store::PairDir;
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NAMES: [&str; 4] = ["s0", "s1", "s2", "s3"];
/// Passos do golden que passavam pelo iniciador `pair_cross`, que saiu.
const REPLACED: [&str; 5] = ["cross_with_local", "cross_unknown_server", "cross_refused", "cross_network_down", "cross_pair"];

struct Machine { addr: SocketAddr, python: Arc<fake::Fake>, pair: PathBuf, share: Arc<hangar_server::share::ShareState> }

type Calls = Arc<Mutex<Vec<Value>>>;

/// Quatro sessões Claude (`s0`..`s3`), id `id` e `peers.json` em `root/peers.json` (escrito depois).
async fn machine(root: &Path, id: &str) -> Machine {
    let script = list_support::sessions(root, NAMES.len());
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
    state.groups = Some(Arc::new(GroupService::new(PairDir::new(pair.clone(), root.join("arquivo")), Arc::new(PythonOrq::from_state(&state)), id.into())));
    state.peers = Arc::new(PeerClient::new(PeerBook::new(Some(root.join("peers.json")))));
    let share = crate::share_pair_routes::share_state(&root.join("shares.json"), &pair, Arc::new(Mutex::new(1_000.0)));
    state.share = Some(share.clone());
    let state = Arc::new(state);
    let app = router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Machine { addr, python, pair, share }
}

/// Repasse a `target` que anota `{to, method, path, body}` de cada pedido.
async fn recorder(target: SocketAddr, label: &'static str, calls: Calls) -> SocketAddr {
    let app = Router::new().fallback(move |req: Request| {
        let calls = calls.clone();
        async move {
            let (parts, body) = req.into_parts();
            if !crate::app_route(parts.uri.path()) {
                return Response::builder().status(404).body(axum::body::Body::empty()).unwrap();
            }
            let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
            calls.lock().unwrap().push(json!({"to": label, "method": parts.method.as_str(), "path": parts.uri.path(),
                "body": serde_json::from_slice::<Value>(&bytes).ok()}));
            let mut fwd = client().request(parts.method.clone(), format!("http://{target}{}", parts.uri.path())).body(bytes.to_vec());
            for name in ["authorization", "content-type"] {
                if let Some(v) = parts.headers.get(name) { fwd = fwd.header(name, v); }
            }
            let resp = fwd.send().await.unwrap();
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap();
            Response::builder().status(status).header("content-type", "application/json").body(axum::body::Body::from(bytes)).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Máquina que lê o pedido inteiro, anota e fecha sem responder: a rede caída depois de enviar.
async fn dropper(label: &'static str, calls: Calls) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let calls = calls.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = sock.read(&mut chunk).await else { return };
                    if n == 0 { return; }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break i + 4; }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                while buf.len() < head_end + length {
                    let Ok(n) = sock.read(&mut chunk).await else { return };
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let mut line = head.lines().next().unwrap_or_default().split(' ');
                let (method, path) = (line.next().unwrap_or_default().to_owned(), line.next().unwrap_or_default().to_owned());
                if crate::app_route(&path) {
                    calls.lock().unwrap().push(json!({"to": label, "method": method, "path": path,
                        "body": serde_json::from_slice::<Value>(&buf[head_end..]).ok()}));
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    addr
}

fn peers_json(root: &Path, lab: SocketAddr, off: SocketAddr) {
    std::fs::write(root.join("peers.json"), json!({
        "lab": {"base_url": format!("http://{lab}"), "token": OWNER},
        "off": {"base_url": format!("http://{off}"), "token": OWNER},
    }).to_string()).unwrap();
}

async fn call(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let mut req = client().request(method.parse().unwrap(), format!("http://{addr}/api/sessions/{path}")).bearer_auth(OWNER);
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn inputs(m: &Machine) -> Vec<usize> {
    NAMES.iter().map(|n| m.python.hits_to(&format!("/api/sessions/{n}/input"))).collect()
}

/// O texto da rede caída depende da biblioteca: no golden é `<rede>`.
fn mask_network(v: &Value) -> Value {
    fn find(v: &Value) -> Option<String> {
        match v {
            Value::String(s) if s.starts_with("off inacessível: ") => Some(s.clone()),
            Value::Object(m) => m.values().find_map(find),
            Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    fn swap(v: &Value, actual: &str) -> Value {
        match v {
            Value::String(s) => Value::String(s.replace(actual, "off inacessível: <rede>")),
            Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), swap(x, actual))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(|x| swap(x, actual)).collect()),
            other => other.clone(),
        }
    }
    match find(v) { Some(actual) => swap(v, &actual), None => v.clone() }
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_routes_answer_like_python() {
    let dirs: HashMap<&str, tempfile::TempDir> = ["casa", "lab", "anon"].into_iter().map(|m| (m, tempfile::tempdir().unwrap())).collect();
    let casa = machine(dirs["casa"].path(), "casa").await;
    let lab = machine(dirs["lab"].path(), "lab").await;
    let anon = machine(dirs["anon"].path(), "").await;
    let calls: Calls = Arc::default();
    let lab_front = recorder(lab.addr, "lab", calls.clone()).await;
    let off = dropper("off", calls.clone()).await;
    for d in dirs.values() { peers_json(d.path(), lab_front, off); }
    let machines: HashMap<&str, &Machine> = HashMap::from([("casa", &casa), ("lab", &lab), ("anon", &anon)]);
    let refusal = json!({"detail": {"code": "erro_fila_nao_digitada", "params": {}, "msg": "composer ilegível"}});

    for step in common::golden("group_cross_routes.json").as_array().unwrap() {
        let name = step["name"].as_str().unwrap();
        let at = machines[step["at"].as_str().unwrap()];
        // O iniciador do par 1:1 saiu: pedido novo vira grupo entre máquinas (`groups_federation`).
        // O par que `cross_pair` criava é semeado, para os passos seguintes do receptor antigo.
        if REPLACED.contains(&name) {
            if name == "cross_pair" {
                std::fs::create_dir_all(&casa.pair).unwrap();
                std::fs::create_dir_all(&lab.pair).unwrap();
                std::fs::write(casa.pair.join("s0.json"), json!({"peers": ["lab::s1"], "task": "t1", "gid": "c0c0c0c0", "harness": {}}).to_string()).unwrap();
                std::fs::write(lab.pair.join("s1.json"), json!({"peers": ["casa::s0"], "task": "t1", "gid": "c1c1c1c1", "harness": {}}).to_string()).unwrap();
            }
            continue;
        }
        if let Some(seed) = step.get("seed").and_then(Value::as_object) {
            std::fs::create_dir_all(&at.pair).unwrap();
            for (n, sidecar) in seed { std::fs::write(at.pair.join(format!("{n}.json")), sidecar.to_string()).unwrap(); }
        }
        let failing = step["fail_delivery"].as_str().map(|m| machines[m]);
        if let Some(m) = failing { m.python.set_input_reply(Some((StatusCode::BAD_REQUEST, refusal.clone()))); }
        calls.lock().unwrap().clear();
        let before: Vec<(&str, Vec<usize>)> = machines.iter().map(|(k, m)| (*k, inputs(m))).collect();

        let (status, body) = call(at.addr, step["method"].as_str().unwrap(), step["path"].as_str().unwrap(), step.get("body")).await;
        if let Some(m) = failing { m.python.set_input_reply(None); }
        if step["python_only"] == true {
            // I-1A: o 422 é do Rust, com código, sem repasse.
            assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")), "{name}");
            continue;
        }
        // Diferença decidida: o texto da recusa `Mix` diz o que vale hoje (o par 1:1 antigo), não o do Python.
        let mut body = body;
        if body["detail"]["code"] == "erro_pareamento_mistura_cross" {
            assert!(body["detail"]["msg"].as_str().unwrap().starts_with("sessão em par 1:1 antigo"), "{name}: {body}");
            body["detail"]["msg"] = step["response"]["detail"]["msg"].clone();
        }
        assert_eq!((status as u64, common::canon(&mask_network(&body))), (step["status"].as_u64().unwrap(), common::canon(&step["response"])), "{name}");
        let mut delivered: Vec<String> = before.iter().flat_map(|(k, b)| {
            let after = inputs(machines[k]);
            NAMES.iter().zip(b.iter().zip(after)).filter(|(_, (b, a))| *a > **b).map(|(n, _)| format!("{k}:{n}")).collect::<Vec<_>>()
        }).collect();
        delivered.sort();
        assert_eq!(json!(delivered), step["delivered"], "{name}: quem recebeu recado");
        let mut sent = calls.lock().unwrap().clone();
        // I-8B: antes da recusa de mistura, o Rust tenta converter o par antigo; lab não conhece casa
        // no peers.json dela, recusa, e o par segue 1:1 com a resposta de sempre.
        if name == "cross_local_mix" {
            assert!(sent.iter().any(|c| c["body"]["convert_from_legacy"].is_string()), "{name}: a conversão foi tentada");
            sent.retain(|c| c["path"] != "/api/sessions/s1/group" && !c["body"]["convert_from_legacy"].is_string());
        }
        assert_eq!(Value::Array(sent), step["calls"], "{name}: o que saiu para outra máquina");
    }
    // O par desfeito dos dois lados; o par direto de `lab` (s3 ↔ casa::s2) continua lá.
    assert!(!casa.pair.join("s0.json").exists() && !lab.pair.join("s1.json").exists());
    assert!(lab.pair.join("s3.json").is_file());
}

/// Saída de quem tem par externo: o Rust desfaz o lado de fora sem o Python e os avisos aparecem.
#[tokio::test(flavor = "multi_thread")]
async fn delete_pair_ends_external_pair() {
    let dir = tempfile::tempdir().unwrap();
    let casa = machine(dir.path(), "casa").await;
    let external = |local: &str, peer: &str| {
        let (s, _) = casa.share.store.create_redeemed(local, Some("t:1".into()), hangar_server::share::store::Kind::Pair).unwrap();
        // Porta 1: a outra máquina fora do ar responde na hora.
        casa.share.pairs.add(hangar_server::share::pairs::ExternalPair { share_id: s.id.clone(), local_session: local.into(),
            alias: "pc-ana".into(), peer_owner: "Ana".into(), peer_session: peer.into(), peer_address: "http://127.0.0.1:1".into(),
            peer_token: "tok".into(), created_at: 1.0 }).unwrap();
        s.id
    };
    std::fs::create_dir_all(&casa.pair).unwrap();
    std::fs::write(casa.pair.join("s0.json"), json!({"peers": ["pc-ana::Y"], "task": "", "gid": "abababab", "harness": {}}).to_string()).unwrap();
    let first = external("s0", "Y");
    std::fs::write(dir.path().join("peers.json"), json!({"pc-ana": {}}).to_string()).unwrap();
    let skipped = json!({"sessao": "pc-ana::Y", "erro": {"code": "erro_par_endereco_ambiguo", "params": {"peer": "pc-ana::Y"},
        "msg": "'pc-ana' é ao mesmo tempo máquina tua e par externo"}});
    let (status, body) = call(casa.addr, "DELETE", "s0/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_pareamento_saida_falhou");
    assert_eq!(body["warning"]["params"]["avisos"], json!([skipped]));
    assert_eq!(inputs(&casa), [1, 0, 0, 0], "quem saiu é avisado");
    assert!(!casa.pair.join("s0.json").exists());
    assert!(casa.share.pairs.all().is_empty() && casa.share.store.all().iter().all(|s| s.id != first || s.revoked_at.is_some()));

    // Outro lado fora do ar: a falha aparece no aviso, a saída vale.
    std::fs::remove_file(dir.path().join("peers.json")).unwrap();
    std::fs::write(casa.pair.join("s1.json"), json!({"peers": ["pc-ana::Z"], "task": "", "gid": "cdcdcdcd", "harness": {}}).to_string()).unwrap();
    external("s1", "Z");
    let (status, body) = call(casa.addr, "DELETE", "s1/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["params"]["avisos"][0]["erro"]["code"], "erro_peer_nao_avisado");
    assert!(!casa.pair.join("s1.json").exists());
    assert!(casa.python.internal_bodies("external-pairs/end").is_empty(), "o Python não é chamado");
}

/// O temporário da sessão vira pasta: a volta atrás do vínculo não consegue apagá-lo. Cada teste usa
/// uma sessão: o diário só passa uma vez por sessão e código no processo.
fn break_undo(m: &Machine, name: &str) { std::fs::create_dir(m.pair.join(format!("{name}.json.tmp"))).unwrap(); }

async fn restore_failed_in_diary(m: &Machine) {
    let python = m.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_restore_failed")).await;
}

/// Quem recebe e não avisa a sessão nem desfaz: o 500 do Python, nunca "pareamento desfeito".
#[tokio::test(flavor = "multi_thread")]
async fn pair_remote_whose_undo_fails_is_500() {
    let dir = tempfile::tempdir().unwrap();
    let casa = machine(dir.path(), "casa").await;
    casa.python.set_input_reply(Some((StatusCode::BAD_REQUEST,
        json!({"detail": {"code": "erro_fila_nao_digitada", "params": {}, "msg": "composer ilegível"}}))));
    casa.python.hold_input(true);
    let addr = casa.addr;
    let asked = tokio::spawn(async move { call(addr, "POST", "s1/pair-remote", Some(&json!({"initiator": "lab::s1"}))).await });
    let s1 = casa.pair.join("s1.json");
    fake::wait_until(move || s1.is_file()).await;
    break_undo(&casa, "s1");
    casa.python.hold_input(false);
    casa.python.release.notify_one();
    assert_eq!(asked.await.unwrap(), (500, json!("Internal Server Error")));
    restore_failed_in_diary(&casa).await;
}

/// BRG-13/BRG-35: a linha do orquestrador não entra em par vindo de outra máquina; nada é gravado.
#[tokio::test(flavor = "multi_thread")]
async fn pair_remote_refuses_the_orchestrator() {
    let dir = tempfile::tempdir().unwrap();
    let casa = machine(dir.path(), "casa").await;
    casa.python.set_internal("orq/is-orchestrator", StatusCode::OK, json!({"names": ["s2"]}));
    let (status, body) = call(casa.addr, "POST", "s2/pair-remote", Some(&json!({"initiator": "lab::s1"}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_sessao_orq")));
    assert!(!casa.pair.join("s2.json").exists());
    assert_eq!(inputs(&casa), [0, 0, 0, 0]);
}
