//! Par 1:1 entre máquinas no Rust com as respostas do Python: o golden `group_cross_routes.json`
//! roda a mesma sequência nas rotas do FastAPI com `casa`, `lab` e `anon` (sem id) no mesmo processo.
//! Aqui são três servidores de verdade, cada um com a pasta, o id e o `peers.json` dele; `lab` é
//! alcançado por um repasse que anota o pedido e `off` aceita a conexão e a derruba sem responder.
#![cfg(unix)]
mod common;
mod fake;
mod list_support;

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

struct Machine { addr: SocketAddr, python: Arc<fake::Fake>, pair: PathBuf }

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
    let state = Arc::new(state);
    let app = router(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Machine { addr, python, pair }
}

/// Repasse a `target` que anota `{to, method, path, body}` de cada pedido.
async fn recorder(target: SocketAddr, label: &'static str, calls: Calls) -> SocketAddr {
    let app = Router::new().fallback(move |req: Request| {
        let calls = calls.clone();
        async move {
            let (parts, body) = req.into_parts();
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
                calls.lock().unwrap().push(json!({"to": label, "method": method, "path": path,
                    "body": serde_json::from_slice::<Value>(&buf[head_end..]).ok()}));
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
            assert_eq!((status, body), (200, json!("from-python")), "{name}: o corpo que o FastAPI recusa vai a ele");
            continue;
        }
        assert_eq!((status as u64, common::canon(&mask_network(&body))), (step["status"].as_u64().unwrap(), common::canon(&step["response"])), "{name}");
        let mut delivered: Vec<String> = before.iter().flat_map(|(k, b)| {
            let after = inputs(machines[k]);
            NAMES.iter().zip(b.iter().zip(after)).filter(|(_, (b, a))| *a > **b).map(|(n, _)| format!("{k}:{n}")).collect::<Vec<_>>()
        }).collect();
        delivered.sort();
        assert_eq!(json!(delivered), step["delivered"], "{name}: quem recebeu recado");
        assert_eq!(Value::Array(calls.lock().unwrap().clone()), step["calls"], "{name}: o que saiu para outra máquina");
    }
    // O par desfeito dos dois lados; o par direto de `lab` (s3 ↔ casa::s2) continua lá.
    assert!(!casa.pair.join("s0.json").exists() && !lab.pair.join("s1.json").exists());
    assert!(lab.pair.join("s3.json").is_file());
}

/// Saída de quem tem par externo: o Python desfaz o lado de fora e os avisos dele aparecem.
#[tokio::test(flavor = "multi_thread")]
async fn delete_pair_ends_external_pair() {
    let dir = tempfile::tempdir().unwrap();
    let casa = machine(dir.path(), "casa").await;
    std::fs::create_dir_all(&casa.pair).unwrap();
    std::fs::write(casa.pair.join("s0.json"), json!({"peers": ["pc-ana::Y"], "task": "", "gid": "abababab", "harness": {}}).to_string()).unwrap();
    std::fs::write(casa.pair.join("external_pairs.json"), json!([{"share_id": "sh1", "local_session": "s0", "alias": "pc-ana",
        "peer_owner": "Ana", "peer_session": "Y", "peer_address": "https://a.tail.ts.net:8443", "peer_token": "tok", "created_at": 1.0}]).to_string()).unwrap();
    let skipped = json!({"sessao": "pc-ana::Y", "erro": {"code": "erro_par_endereco_ambiguo", "params": {"peer": "pc-ana::Y"},
        "msg": "'pc-ana' é ao mesmo tempo máquina tua e par externo"}});
    casa.python.set_internal("external-pairs/end", StatusCode::OK, json!({"errors": [skipped.clone()]}));
    let (status, body) = call(casa.addr, "DELETE", "s0/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(casa.python.internal_bodies("external-pairs/end"), vec![json!({"name": "s0", "peer": "pc-ana::Y"})]);
    assert_eq!(body["warning"]["code"], "erro_pareamento_saida_falhou");
    assert_eq!(body["warning"]["params"]["avisos"], json!([skipped]));
    assert_eq!(inputs(&casa), [1, 0, 0, 0], "quem saiu é avisado");
    assert!(!casa.pair.join("s0.json").exists());

    // Python fora: a falha aparece no aviso, a saída vale.
    std::fs::write(casa.pair.join("s1.json"), json!({"peers": ["pc-ana::Z"], "task": "", "gid": "cdcdcdcd", "harness": {}}).to_string()).unwrap();
    std::fs::write(casa.pair.join("external_pairs.json"), json!([{"share_id": "sh2", "local_session": "s1", "alias": "pc-ana",
        "peer_owner": "Ana", "peer_session": "Z", "peer_address": "https://a.tail.ts.net:8443", "peer_token": "tok", "created_at": 1.0}]).to_string()).unwrap();
    casa.python.set_internal("external-pairs/end", StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    let (status, body) = call(casa.addr, "DELETE", "s1/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["params"]["avisos"][0]["erro"]["code"], "erro_peer_nao_avisado");
    assert!(!casa.pair.join("s1.json").exists());
}

/// O temporário da sessão vira pasta: a volta atrás do vínculo não consegue apagá-lo. Cada teste usa
/// uma sessão: o diário só passa uma vez por sessão e código no processo.
fn break_undo(m: &Machine, name: &str) { std::fs::create_dir(m.pair.join(format!("{name}.json.tmp"))).unwrap(); }

async fn restore_failed_in_diary(m: &Machine) {
    let python = m.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_restore_failed")).await;
}

/// Quem inicia e não desfaz o lado daqui quando o outro não confirma: o 500 do Python, nunca "desfeito".
#[tokio::test(flavor = "multi_thread")]
async fn pair_cross_whose_undo_fails_is_500() {
    let dir = tempfile::tempdir().unwrap();
    let casa = machine(dir.path(), "casa").await;
    let off = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let off_addr = off.local_addr().unwrap();
    peers_json(dir.path(), off_addr, off_addr);
    let addr = casa.addr;
    let asked = tokio::spawn(async move { call(addr, "POST", "s0/pair", Some(&json!({"peers": ["off::x"]}))).await });
    let (sock, _) = off.accept().await.unwrap();
    assert!(casa.pair.join("s0.json").is_file(), "o lado daqui grava antes de chamar o outro");
    break_undo(&casa, "s0");
    drop((sock, off));
    assert_eq!(asked.await.unwrap(), (500, json!("Internal Server Error")));
    restore_failed_in_diary(&casa).await;
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
