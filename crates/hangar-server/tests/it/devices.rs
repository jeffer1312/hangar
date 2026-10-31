//! Aparelhos que abrem a lista do dono (`device=<tipo>;<id>`): o primeiro acesso vai ao
//! `devices.json` e ao evento `devices`; convidado, porta do Connect e parâmetro inválido não gravam.
use crate::fake::{OWNER, client, next_any, next_named, sse, Events};
use crate::list_support::{open, server, server_trusting};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn file(root: &Path) -> PathBuf { root.join("home/.hangar/devices.json") }

fn recorded(root: &Path) -> Vec<String> {
    let devices: Value = serde_json::from_slice(&std::fs::read(file(root)).unwrap()).unwrap();
    devices.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap().to_owned()).collect()
}

async fn open_device(addr: SocketAddr, query: &str, forwarded_for: Option<&str>) -> Events {
    let mut req = client().get(format!("http://{addr}/api/sessions/events?token={OWNER}&{query}"));
    if let Some(ip) = forwarded_for { req = req.header("x-forwarded-for", ip); }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), 200, "{query}");
    sse(resp)
}

#[tokio::test(flavor = "multi_thread")]
async fn first_access_is_recorded_and_a_known_device_is_checked_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let first = open_device(srv.addr, "device=ios%3Babc-1", None).await;
    assert_eq!(recorded(dir.path()), ["abc-1"]);
    drop(first);
    std::fs::remove_file(file(dir.path())).unwrap();
    let _again = open_device(srv.addr, "device=ios%3Babc-1", None).await;
    assert!(!file(dir.path()).exists(), "aparelho conhecido não toca o disco");
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_connect_port_and_invalid_devices_record_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server_trusting(dir.path(), 1, "127.0.0.1").await;
    let long = format!("device=ios%3B{}", "a".repeat(65));
    for query in ["device=desktop%3Babc", long.as_str(), "device=ios%3Ba_b", "device=ios"] {
        let _es = open_device(srv.addr, query, None).await;
    }
    // Token que o Rust não conhece segue ao Python; o convidado com concessão, na porta dele, está em
    // `share_guest_serving::guest_list_with_a_device_records_nothing`.
    let resp = client().get(format!("http://{}/api/sessions/events?token=convidado&device=ios%3Bg-1", srv.addr)).send().await.unwrap();
    assert_eq!(resp.text().await.unwrap(), "from-python", "convidado segue ao Python");
    let _connect = open_device(srv.addr, "device=ios%3Bc-1", Some("192.0.2.1")).await;
    assert!(!file(dir.path()).exists());
    // O mesmo caminho repassado de outro endereço grava: quem barra é o endereço do Connect.
    let _lan = open_device(srv.addr, "device=pwa%3Blan-1", Some("203.0.113.5")).await;
    assert_eq!(recorded(dir.path()), ["lan-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_list_sends_the_snapshot_and_again_only_when_a_device_joins() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let mut watching = open(srv.addr).await;
    assert_eq!(next_named(&mut watching, "devices").await.data, "[]");
    let _phone = open_device(srv.addr, "device=android%3Bphone-1", None).await;
    let snapshot: Value = serde_json::from_str(&next_named(&mut watching, "devices").await.data).unwrap();
    let devices = snapshot.as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!((&devices[0]["id"], &devices[0]["kind"]), (&Value::from("phone-1"), &Value::from("android")));
    assert!(chrono::DateTime::parse_from_rfc3339(devices[0]["first_seen"].as_str().unwrap()).is_ok());
    // Toda conexão do dono começa com o retrato.
    let mut again = open_device(srv.addr, "device=android%3Bphone-1", None).await;
    assert_eq!(serde_json::from_str::<Value>(&next_named(&mut again, "devices").await.data).unwrap(), snapshot);
    // Aparelho já conhecido não muda o retrato: nada novo em dois tiques.
    let quiet = tokio::time::timeout(Duration::from_millis(3500), async {
        loop { if next_any(&mut watching).await.event == "devices" { return; } }
    }).await;
    assert!(quiet.is_err(), "retrato repetido sem aparelho novo");
}
