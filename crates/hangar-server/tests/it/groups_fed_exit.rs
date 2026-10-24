//! Sair, morrer, renomear e dissolver num grupo entre máquinas: a máquina do membro grava na hora e
//! avisa a dona pela fila; a dona tira o membro e, com uma máquina só com membros, dissolve.
//! Mesmas três máquinas no processo do `groups_federation.rs` (`casa`, `lab`, `vm`).
#![cfg(unix)]
use crate::fake;
use crate::groups_support::*;

use hangar_server::groups::federation;
use hangar_server::groups::outbox::OutboxItem;
use serde_json::{Value, json};

/// Grupo criado em `m` com `name` e `others`; devolve o gid.
async fn pair(m: &Machine, name: &str, others: &[&str]) -> String {
    let (status, body) = call(m.addr, "POST", &format!("/api/sessions/{name}/pair"), Some(&json!({"peers": others}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"], Value::Null, "{body}");
    body["gid"].as_str().unwrap().to_owned()
}

async fn unpair(m: &Machine, name: &str) {
    let (status, body) = call(m.addr, "DELETE", &format!("/api/sessions/{name}/pair"), None).await;
    assert_eq!(status, 200, "{body}");
}

/// O sidecar de `name` em `m` chega a `want` (a fila entrega depois da resposta).
async fn sidecar_becomes(m: &Machine, name: &str, want: impl Fn(Option<Value>) -> bool + Send + 'static) {
    let path = m.pair.join(format!("{name}.json"));
    fake::wait_until(move || want(read_json(path.clone()))).await;
}

async fn record_becomes(m: &Machine, gid: &str, want: impl Fn(Option<Value>) -> bool + Send + 'static) {
    let path = m.pair.join("groups").join(format!("{gid}.json"));
    fake::wait_until(move || want(read_json(path.clone()))).await;
}

fn has_members(want: &'static [&'static str]) -> impl Fn(Option<Value>) -> bool + Send + 'static {
    move |r| r.is_some_and(|r| members(&r["members"]) == want)
}

fn has_peers(want: &'static [&'static str]) -> impl Fn(Option<Value>) -> bool + Send + 'static {
    move |s| s.is_some_and(|s| peers_of(&s) == want)
}

/// Dona fora do ar: a saída vale na hora em lab, e o `Leave` espera na fila.
#[tokio::test(flavor = "multi_thread")]
async fn leave_never_waits_for_the_owner() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "lab::s2"]).await;
    sidecar_becomes(&n.lab, "s2", has_peers(&["casa::s0", "s1"])).await;
    peers(&n.lab, &[("casa", closed_port().await), ("vm", n.vm.addr)]);
    unpair(&n.lab, "s1").await;
    assert!(sidecar(&n.lab, "s1").is_none());
    assert_eq!(peers_of(&sidecar(&n.lab, "s2").unwrap()), ["casa::s0"], "o companheiro da mesma máquina já não vê s1");
    let fed = federation::existing(&n.lab.pair).unwrap();
    let leave = OutboxItem::Leave { owner: "casa".into(), gid: gid.clone(), member: "s1".into(), origin: "lab".into() };
    assert!(fed.outbox().pending().contains(&leave), "{:?}", fed.outbox().pending());
    // A dona volta: tira s1 e empurra o grupo novo.
    peers(&n.lab, &[("casa", n.casa.addr), ("vm", n.vm.addr)]);
    record_becomes(&n.casa, &gid, has_members(&["casa::s0", "lab::s2"])).await;
    sidecar_becomes(&n.casa, "s0", has_peers(&["lab::s2"])).await;
}

/// A morte (o que a varredura chama) sai pelo mesmo caminho: a fila, nunca `/unpair-remote`.
#[tokio::test(flavor = "multi_thread")]
async fn dead_member_leaves_through_the_queue() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "vm::s2"]).await;
    sidecar_becomes(&n.vm, "s2", has_peers(&["casa::s0", "lab::s1"])).await;
    let groups = n.lab.state.groups.clone().unwrap();
    let (ex, warnings) = hangar_server::groups::exit::leave_and_notify(&n.lab.state, &groups, "s1").await.unwrap();
    assert_eq!(warnings, Vec::<Value>::new());
    assert_eq!(sorted(&json!(ex), |p| p.as_str().unwrap().to_owned()), ["casa::s0", "vm::s2"]);
    record_becomes(&n.casa, &gid, has_members(&["casa::s0", "vm::s2"])).await;
    sidecar_becomes(&n.vm, "s2", has_peers(&["casa::s0"])).await;
    assert_eq!(peers_of(&sidecar(&n.casa, "s0").unwrap()), ["vm::s2"], "os outros continuam no grupo");
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_reaches_the_owner() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "vm::s2"]).await;
    sidecar_becomes(&n.lab, "s1", |s| s.is_some()).await;
    let groups = n.lab.state.groups.clone().unwrap();
    hangar_server::groups::fed_exit::rename(&n.lab.state, &groups, "s1", "s3").await.expect("grupo entre máquinas").unwrap();
    assert!(sidecar(&n.lab, "s1").is_none());
    assert!(sidecar(&n.lab, "s3").is_some());
    record_becomes(&n.casa, &gid, has_members(&["casa::s0", "lab::s3", "vm::s2"])).await;
    sidecar_becomes(&n.casa, "s0", has_peers(&["lab::s3", "vm::s2"])).await;
    sidecar_becomes(&n.vm, "s2", has_peers(&["casa::s0", "lab::s3"])).await;
    let s3 = sidecar(&n.lab, "s3").unwrap();
    assert_eq!((s3["fed"]["owner"].clone(), peers_of(&s3)), (json!("casa"), vec!["casa::s0".to_owned(), "vm::s2".to_owned()]));
}

/// O `Push` com o nome velho chega antes de a dona aplicar o `Rename`: o membro novo fica.
#[tokio::test(flavor = "multi_thread")]
async fn push_before_rename_keeps_new_name() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1"]).await;
    sidecar_becomes(&n.lab, "s1", |s| s.is_some()).await;
    peers(&n.lab, &[("casa", closed_port().await), ("vm", n.vm.addr)]);
    let groups = n.lab.state.groups.clone().unwrap();
    hangar_server::groups::fed_exit::rename(&n.lab.state, &groups, "s1", "s3").await.expect("grupo entre máquinas").unwrap();
    let (status, answer) = push(&n.lab, &gid, json!({"protocol": 1, "owner": "casa", "version": 2, "members": [member("casa", "s0"), member("lab", "s1")]})).await;
    assert_eq!(status, 200, "{answer}");
    let s3 = sidecar(&n.lab, "s3").expect("o nome novo continua no grupo");
    assert_eq!((s3["fed"]["version"].clone(), peers_of(&s3)), (json!(2), vec!["casa::s0".to_owned()]));
    assert!(sidecar(&n.lab, "s1").is_none());
}

/// Sobrou uma máquina com membros (a dona): o grupo vira local comum, com o par de fora.
#[tokio::test(flavor = "multi_thread")]
async fn dissolve_keeps_a_local_group_with_outside_pairs() {
    let n = net().await;
    outside_pair(&n.casa, "s0", "pc-ana", "Y");
    let gid = pair(&n.casa, "s0", &["s1", "lab::s2"]).await;
    sidecar_becomes(&n.lab, "s2", |s| s.is_some()).await;
    unpair(&n.lab, "s2").await;
    record_becomes(&n.casa, &gid, |r| r.is_none()).await;
    sidecar_becomes(&n.casa, "s0", |s| s.is_some_and(|s| s.get("fed").is_none())).await;
    let s0 = sidecar(&n.casa, "s0").unwrap();
    assert_eq!((s0["gid"].clone(), peers_of(&s0)), (json!(gid), vec!["pc-ana::Y".to_owned(), "s1".to_owned()]));
    let s1 = sidecar(&n.casa, "s1").unwrap();
    assert!(s1.get("fed").is_none());
    assert_eq!(peers_of(&s1), ["pc-ana::Y", "s0"]);
}

/// Sobrou uma máquina com membros (a participante): lá fica um grupo local comum.
#[tokio::test(flavor = "multi_thread")]
async fn dissolve_leaves_a_local_group_on_the_remaining_machine() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "lab::s2"]).await;
    sidecar_becomes(&n.lab, "s2", has_peers(&["casa::s0", "s1"])).await;
    unpair(&n.casa, "s0").await;
    assert!(record(&n.casa, &gid).is_none(), "a dona dissolve no mesmo passo");
    sidecar_becomes(&n.lab, "s1", |s| s.is_some_and(|s| s.get("fed").is_none() && peers_of(&s) == ["s2"])).await;
    // Cada sidecar é um arquivo: o de `s2` pode chegar depois do de `s1`.
    sidecar_becomes(&n.lab, "s2", |s| s.is_some_and(|s| s.get("fed").is_none())).await;
    let s2 = sidecar(&n.lab, "s2").unwrap();
    assert_eq!((s2["gid"].clone(), peers_of(&s2)), (json!(gid), vec!["s1".to_owned()]));
}

/// Sobrou só a participante: o contrato da dona vai para o arquivo, como o de um grupo que acabou.
#[tokio::test(flavor = "multi_thread")]
async fn dissolve_archives_the_owner_contract() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "lab::s2"]).await;
    sidecar_becomes(&n.lab, "s2", |s| s.is_some()).await;
    let contract = n.casa.pair.join(format!("grupo-{gid}.md"));
    std::fs::write(&contract, "combinado").unwrap();
    unpair(&n.casa, "s0").await;
    assert!(record(&n.casa, &gid).is_none());
    assert!(!contract.exists(), "o contrato saiu da pasta do grupo");
    let archived: Vec<String> = std::fs::read_dir(n.casa.root.join("arquivo")).unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert!(archived.iter().any(|f| f.starts_with(&format!("grupo-{gid}-"))), "{archived:?}");
}

/// Join parcial numa dona sem membro próprio: quem não confirmou sai e, sobrando uma máquina só
/// com membros, o grupo dissolve como numa saída.
#[tokio::test(flavor = "multi_thread")]
async fn partial_join_on_owner_without_members_dissolves() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "vm::s2"]).await;
    sidecar_becomes(&n.vm, "s2", |s| s.is_some()).await;
    unpair(&n.casa, "s0").await;
    // O envio lê o registro na hora: reescrevê-lo antes da entrega dissolveria o grupo antes da hora.
    sidecar_becomes(&n.lab, "s1", has_peers(&["vm::s2"])).await;
    sidecar_becomes(&n.vm, "s2", has_peers(&["lab::s1"])).await;
    // Registro com uma máquina só: o que fica de uma dissolução interrompida no meio.
    let path = n.casa.pair.join("groups").join(format!("{gid}.json"));
    let mut rec = record(&n.casa, &gid).unwrap();
    rec["members"] = json!([member("lab", "s1")]);
    std::fs::write(&path, rec.to_string()).unwrap();
    // vm recusa `s9` (sessão inexistente lá); lab confirma `s3`.
    let (status, body) = call(n.lab.addr, "POST", "/api/sessions/s1/pair", Some(&json!({"peers": ["s3", "vm::s9"]}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_membro_nao_confirmado", "{body}");
    assert!(record(&n.casa, &gid).is_none(), "uma máquina só com membros: dissolve");
    sidecar_becomes(&n.lab, "s1", |s| s.is_some_and(|s| s.get("fed").is_none() && peers_of(&s) == ["s3"])).await;
}

/// O par de fora de quem sai acaba como hoje; o grupo entre máquinas segue sem ele.
#[tokio::test(flavor = "multi_thread")]
async fn leave_ends_outside_pairs_as_today() {
    let n = net().await;
    outside_pair(&n.casa, "s0", "pc-ana", "Y");
    let gid = pair(&n.casa, "s0", &["lab::s1", "vm::s2"]).await;
    sidecar_becomes(&n.vm, "s2", |s| s.is_some()).await;
    unpair(&n.casa, "s0").await;
    assert!(n.casa.share.pairs.all().is_empty(), "o par de fora acabou");
    assert!(sidecar(&n.casa, "s0").is_none());
    assert_eq!(members(&record(&n.casa, &gid).unwrap()["members"]), ["lab::s1", "vm::s2"]);
    sidecar_becomes(&n.lab, "s1", has_peers(&["vm::s2"])).await;
}

/// Dona sem membro próprio continua dona enquanto houver membros em duas máquinas.
#[tokio::test(flavor = "multi_thread")]
async fn owner_without_own_members_stays_owner() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["lab::s1", "vm::s2"]).await;
    sidecar_becomes(&n.vm, "s2", |s| s.is_some()).await;
    unpair(&n.casa, "s0").await;
    let rec = record(&n.casa, &gid).expect("a dona mantém o registro");
    assert_eq!((rec["owner"].clone(), members(&rec["members"])), (json!("casa"), vec!["lab::s1".to_owned(), "vm::s2".to_owned()]));
    sidecar_becomes(&n.lab, "s1", has_peers(&["vm::s2"])).await;
    sidecar_becomes(&n.vm, "s2", has_peers(&["lab::s1"])).await;
    assert_eq!(sidecar(&n.lab, "s1").unwrap()["fed"]["owner"], "casa");
}

/// Máquina antiga ainda manda `/unpair-remote` a um grupo que a dona já tem como entre máquinas:
/// passa pelo `remove_member` da dona, e o grupo de dois dissolve.
#[tokio::test(flavor = "multi_thread")]
async fn old_peer_unpair_after_owner_converted() {
    let n = net().await;
    let gid = pair(&n.casa, "s0", &["s1", "lab::s2"]).await;
    sidecar_becomes(&n.lab, "s2", |s| s.is_some()).await;
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/unpair-remote", Some(&json!({"peer": "lab::s2"}))).await;
    assert_eq!(status, 200, "{body}");
    assert!(record(&n.casa, &gid).is_none(), "uma máquina só com membros: dissolve");
    let s0 = sidecar(&n.casa, "s0").expect("s0 continua com s1");
    assert_eq!(peers_of(&s0), ["s1"]);
    assert!(s0.get("fed").is_none());
    sidecar_becomes(&n.lab, "s2", |s| s.is_none()).await;
}
