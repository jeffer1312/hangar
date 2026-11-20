//! Par 1:1 antigo entre máquinas virando grupo entre máquinas sozinho (I-8B): duas máquinas de
//! verdade no processo (`casa`, `lab`), cada uma com a pasta, o id e o `peers.json` dela.
#![cfg(unix)]
use crate::fake;
use crate::groups_support::*;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use fake::client;
use hangar_server::groups::convert::group_id;
use hangar_server::groups::federation::{self, Timing};
use hangar_server::groups::store::PairDir;
use serde_json::{Value, json};

/// sha1("casa::s0\nlab::s1") = 3c09fbe2ae99b90a9869e4348c8d7c9abcc06cf5, conferido fora do código.
const GID: &str = "3c09fbe2ae99";
const LEGACY_GID: &str = "0a0b0c0d";

/// Só `casa` e `lab`: o par antigo é de duas máquinas.
struct Net { casa: Machine, lab: Machine, _dirs: Vec<tempfile::TempDir> }

async fn net() -> Net {
    let dirs: Vec<tempfile::TempDir> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let casa = machine(dirs[0].path(), "casa").await;
    let lab = machine(dirs[1].path(), "lab").await;
    peers(&casa, &[("lab", lab.addr)]);
    peers(&lab, &[("casa", casa.addr)]);
    Net { casa, lab, _dirs: dirs }
}

/// O sidecar como o Python gravava o par 1:1 entre máquinas.
fn legacy(m: &Machine, name: &str, peer: &str) {
    std::fs::create_dir_all(&m.pair).unwrap();
    std::fs::write(m.pair.join(format!("{name}.json")), json!({"peers": [peer], "task": "t", "gid": LEGACY_GID, "harness": {}}).to_string()).unwrap();
}

fn records(m: &Machine) -> Vec<String> {
    PairDir::new(m.pair.clone(), PathBuf::new()).records().unwrap().into_iter().map(|r| r.gid).collect()
}

/// Novas tentativas de conversão a cada 100 ms.
fn quick_retry(m: &Machine) { timing(m, Timing { retry_ladder: vec![Duration::from_millis(100)], ..fast() }); }

/// A subida da máquina, que converte os pares antigos dela.
async fn start(m: &Machine) { federation::spawn(m.state.clone()).expect("máquina com id").await.unwrap(); }

/// Espera o diário da máquina registrar `code`.
async fn diary_has(m: &Machine, code: &'static str) {
    let python = m.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["codigo"] == code)).await;
}

/// Hangar antigo: rota que não existe responde o 404 do FastAPI, sem código.
async fn old_machine() -> SocketAddr {
    let old = Router::new().fallback(|| async { (axum::http::StatusCode::NOT_FOUND, [("content-type", "application/json")], r#"{"detail":"Not Found"}"#) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, old).await.unwrap() });
    addr
}

/// `target` atrás de um interruptor: desligado responde como Hangar antigo, ligado repassa.
async fn upgradable(target: SocketAddr, upgraded: Arc<std::sync::atomic::AtomicBool>) -> SocketAddr {
    let app = Router::new().fallback(move |req: axum::extract::Request| {
        let upgraded = upgraded.clone();
        async move {
            if !upgraded.load(std::sync::atomic::Ordering::SeqCst) {
                return axum::http::Response::builder().status(404).header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"detail":"Not Found"}"#)).unwrap();
            }
            let (parts, body) = req.into_parts();
            let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
            let mut fwd = client().request(parts.method.clone(), format!("http://{target}{}", parts.uri.path())).body(bytes.to_vec());
            for name in ["authorization", "content-type"] {
                if let Some(v) = parts.headers.get(name) { fwd = fwd.header(name, v); }
            }
            let resp = fwd.send().await.unwrap();
            let status = resp.status().as_u16();
            let text = resp.bytes().await.unwrap();
            axum::http::Response::builder().status(status).header("content-type", "application/json").body(axum::body::Body::from(text)).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn assert_converted(n: &Net) {
    let rec = record(&n.casa, GID).expect("a dona (menor id) grava o registro");
    assert_eq!((rec["owner"].clone(), rec["version"].clone(), rec["task"].clone()), (json!("casa"), json!(1), json!("t")));
    assert_eq!(members(&rec["members"]), ["casa::s0", "lab::s1"]);
    assert_eq!(records(&n.casa), [GID]);
    assert!(records(&n.lab).is_empty(), "só a dona tem registro");
    let s0 = sidecar(&n.casa, "s0").unwrap();
    assert_eq!((s0["gid"].clone(), s0["peers"].clone()), (json!(GID), json!(["lab::s1"])));
    assert_eq!(s0["fed"], json!({"owner": "casa", "local": true, "version": 1}));
    let s1 = sidecar(&n.lab, "s1").unwrap();
    assert_eq!((s1["gid"].clone(), s1["peers"].clone()), (json!(GID), json!(["casa::s0"])));
    assert_eq!(s1["fed"], json!({"owner": "casa", "local": false, "version": 1}));
}

#[test]
fn gid_is_deterministic() {
    assert_eq!(group_id("casa::s0", "lab::s1"), GID);
    assert_eq!(group_id("lab::s1", "casa::s0"), GID, "a ordem de quem calcula não muda o gid");
}

/// Review Focus 5: as duas sobem juntas e chegam ao mesmo grupo, sem dois registros.
#[tokio::test(flavor = "multi_thread")]
async fn both_sides_convert_to_the_same_group() {
    let n = net().await;
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    tokio::join!(start(&n.casa), start(&n.lab));
    assert_converted(&n);
}

#[tokio::test(flavor = "multi_thread")]
async fn conversion_is_idempotent() {
    let n = net().await;
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    // A participante já converteu e a dona caiu antes de gravar: a subida da dona termina a conversão.
    let body = json!({"protocol": 1, "owner": "casa", "version": 1, "task": "t", "members": [member("casa", "s0"), member("lab", "s1")],
        "joining": [], "convert_from_legacy": "casa::s0"});
    let (status, answer) = call(n.lab.addr, "PUT", &format!("/api/groups/{GID}"), Some(&body)).await;
    assert_eq!(status, 200, "{answer}");
    start(&n.casa).await;
    assert_converted(&n);
    start(&n.casa).await;
    start(&n.lab).await;
    let (status, answer) = call(n.lab.addr, "PUT", &format!("/api/groups/{GID}"), Some(&body)).await;
    assert_eq!(status, 200, "o mesmo envio de novo não é recusado: {answer}");
    assert_converted(&n);
}

#[tokio::test(flavor = "multi_thread")]
async fn old_version_peer_stays_legacy() {
    let n = net().await;
    peers(&n.casa, &[("lab", old_machine().await)]);
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    let before = sidecar(&n.casa, "s0").unwrap();
    start(&n.casa).await;
    assert_eq!(sidecar(&n.casa, "s0").unwrap(), before, "o par segue 1:1, sem `fed`");
    assert!(records(&n.casa).is_empty(), "sem registro");
    // A outra máquina atualizou: a próxima subida converte.
    peers(&n.casa, &[("lab", n.lab.addr)]);
    start(&n.casa).await;
    assert_converted(&n);
}

/// As máquinas do dono atualizam em dias diferentes: a dona tenta de novo sozinha, sem reiniciar.
#[tokio::test(flavor = "multi_thread")]
async fn conversion_retries_after_the_peer_upgrades() {
    let n = net().await;
    quick_retry(&n.casa);
    let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    peers(&n.casa, &[("lab", upgradable(n.lab.addr, upgraded.clone()).await)]);
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    start(&n.casa).await;
    assert!(records(&n.casa).is_empty(), "lab ainda é antiga: segue 1:1");
    upgraded.store(true, std::sync::atomic::Ordering::SeqCst);
    // O sidecar da dona é o último a ser gravado na conversão.
    let casa = n.casa.pair.clone();
    fake::wait_until(move || std::fs::read(casa.join("s0.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|s| s["fed"]["owner"] == "casa")).await;
    assert_converted(&n);
}

/// `/pair` só com sessões daqui, uma delas ainda em par 1:1 antigo: converte o par antes e entra
/// pelo grupo entre máquinas; com a outra máquina antiga, a recusa de hoje.
#[tokio::test(flavor = "multi_thread")]
async fn local_pair_with_legacy_member_converts_then_joins() {
    let n = net().await;
    let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    peers(&n.casa, &[("lab", upgradable(n.lab.addr, upgraded.clone()).await)]);
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s2/pair", Some(&json!({"peers": ["s0"]}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_pareamento_mistura_cross")), "{body}");
    assert!(sidecar(&n.casa, "s2").is_none() && records(&n.casa).is_empty());
    upgraded.store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s2/pair", Some(&json!({"peers": ["s0"]}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["gid"], GID, "entra no grupo convertido");
    assert_eq!(members(&record(&n.casa, GID).unwrap()["members"]), ["casa::s0", "casa::s2", "lab::s1"]);
    let lab = n.lab.pair.clone();
    fake::wait_until(move || std::fs::read(lab.join("s1.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|s| s["peers"].as_array().unwrap().contains(&json!("casa::s2")))).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn outside_pair_is_never_converted() {
    let n = net().await;
    outside_pair(&n.casa, "s0", "pc-ana", "Y");
    // Par de fora cujo registro sumiu: o apelido não é máquina do peers.json.
    std::fs::write(n.casa.pair.join("s2.json"), json!({"peers": ["pc-velho::Z"], "task": "", "gid": "cdcdcdcd", "harness": {}}).to_string()).unwrap();
    start(&n.casa).await;
    let s0 = sidecar(&n.casa, "s0").unwrap();
    assert_eq!((s0["gid"].clone(), s0.get("fed").cloned()), (json!("abababab"), None));
    assert!(sidecar(&n.casa, "s2").unwrap().get("fed").is_none());
    assert!(records(&n.casa).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn alias_colliding_with_peer_is_not_converted() {
    let n = net().await;
    // O apelido do par de fora é também o nome de uma máquina do peers.json.
    outside_pair(&n.casa, "s0", "lab", "s1");
    legacy(&n.lab, "s1", "casa::s0");
    start(&n.casa).await;
    assert!(sidecar(&n.casa, "s0").unwrap().get("fed").is_none());
    assert!(sidecar(&n.lab, "s1").unwrap().get("fed").is_none());
    assert!(records(&n.casa).is_empty());
    diary_has(&n.casa, "grupos_conversao_ambigua").await;
}

/// O `peers.json` de casa guarda lab por uma chave antiga: a identidade é conferida antes, a
/// conversão é recusada sem gravar nada, e nenhuma das pontas cria um segundo grupo.
#[tokio::test(flavor = "multi_thread")]
async fn peers_key_differs_from_server_id_converts_once() {
    let n = net().await;
    peers(&n.casa, &[("lab-velho", n.lab.addr)]);
    legacy(&n.casa, "s0", "lab-velho::s1");
    legacy(&n.lab, "s1", "casa::s0");
    let (casa_before, lab_before) = (sidecar(&n.casa, "s0").unwrap(), sidecar(&n.lab, "s1").unwrap());
    tokio::join!(start(&n.casa), start(&n.lab));
    start(&n.casa).await;
    assert_eq!(sidecar(&n.casa, "s0").unwrap(), casa_before);
    assert_eq!(sidecar(&n.lab, "s1").unwrap(), lab_before);
    assert!(records(&n.casa).is_empty() && records(&n.lab).is_empty());
    diary_has(&n.casa, "erro_grupo_identificador_divergente").await;
}

/// Máquina antiga desfaz o par pelo `unpair-remote` depois de a dona converter: a saída passa
/// pela dona e chega à outra máquina, nunca é saída solta daqui.
#[tokio::test(flavor = "multi_thread")]
async fn old_peer_unpair_after_owner_converted() {
    let n = net().await;
    legacy(&n.casa, "s0", "lab::s1");
    legacy(&n.lab, "s1", "casa::s0");
    start(&n.casa).await;
    assert_converted(&n);
    let (status, answer) = call(n.casa.addr, "POST", "/api/sessions/s0/unpair-remote", Some(&json!({"peer": "lab::s1"}))).await;
    assert_eq!(status, 200, "{answer}");
    if let Some(rec) = record(&n.casa, GID) { assert_eq!(members(&rec["members"]), ["casa::s0"]); }
    assert!(sidecar(&n.casa, "s0").is_none_or(|s| !s["peers"].as_array().unwrap().contains(&json!("lab::s1"))));
    let lab = n.lab.pair.clone();
    fake::wait_until(move || !lab.join("s1.json").exists()).await;
}

/// Membro daqui ainda em par 1:1 antigo que não converteu: entrar no grupo novo perderia o par sem
/// aviso; a recusa diz com que máquina é o par.
#[tokio::test(flavor = "multi_thread")]
async fn unconverted_legacy_member_is_refused() {
    let n = net().await;
    legacy(&n.casa, "s0", "lab::s1");
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/pair", Some(&json!({"peers": ["lab::s2"]}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_par_antigo")), "{body}");
    assert_eq!(body["detail"]["params"]["servidor"], "lab");
    assert_eq!(sidecar(&n.casa, "s0").unwrap()["peers"], json!(["lab::s1"]));
    assert!(records(&n.casa).is_empty() && sidecar(&n.lab, "s2").is_none());
}

/// O par antigo que chega pelo `/pair-remote` de uma máquina antiga vira grupo quando ela atualiza.
#[tokio::test(flavor = "multi_thread")]
async fn pair_remote_converts_after_the_peer_upgrades() {
    let n = net().await;
    quick_retry(&n.casa);
    let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    peers(&n.casa, &[("lab", upgradable(n.lab.addr, upgraded.clone()).await)]);
    legacy(&n.lab, "s1", "casa::s0");
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/pair-remote", Some(&json!({"initiator": "lab::s1", "task": "t"}))).await;
    assert_eq!(status, 200, "{body}");
    upgraded.store(true, std::sync::atomic::Ordering::SeqCst);
    let casa = n.casa.pair.clone();
    fake::wait_until(move || std::fs::read(casa.join("s0.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|s| s["fed"]["owner"] == "casa")).await;
    assert_converted(&n);
}
