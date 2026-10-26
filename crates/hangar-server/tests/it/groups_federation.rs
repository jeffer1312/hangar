//! Grupo entre máquinas do dono: duas ou três máquinas de verdade no processo (`casa`, `lab`, `vm`),
//! cada uma com a pasta, o id e o `peers.json` dela. A fila anda com escada curta e a dona espera
//! a confirmação por pouco tempo, para a máquina fora do ar não segurar o teste.
#![cfg(unix)]
use crate::fake;
use crate::groups_support::*;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::http::Method;
use fake::{SECRET, client};
use hangar_server::groups::federation;
use hangar_server::groups::outbox::OutboxItem;
use hangar_server::share::caller::{GuestGrant, guest_allowed};
use hangar_server::share::store::Kind;
use serde_json::{Value, json};

async fn pair(m: &Machine, name: &str, body: Value) -> (u16, Value) { call(m.addr, "POST", &format!("/api/sessions/{name}/pair"), Some(&body)).await }

/// A versão nova chega às participantes pela fila, depois da resposta.
async fn peers_become(m: &Machine, name: &str, want: &[&str]) {
    let (pair, name, want) = (m.pair.clone(), name.to_owned(), want.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    fake::wait_until(move || std::fs::read(pair.join(format!("{name}.json"))).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()).is_some_and(|v| peers_of(&v) == want)).await;
}


#[tokio::test(flavor = "multi_thread")]
async fn three_locals_plus_one_remote() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s1", "s2", "lab::s3"], "task": "t"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"], Value::Null);
    assert_eq!(body["owner"], "casa");
    let gid = body["gid"].as_str().unwrap();
    let rec = record(&n.casa, gid).expect("a dona grava o registro");
    assert_eq!(members(&rec["members"]), ["casa::s0", "casa::s1", "casa::s2", "lab::s3"]);
    assert_eq!(rec["version"], 1);
    for name in ["s0", "s1", "s2"] {
        let s = sidecar(&n.casa, name).unwrap();
        assert_eq!(s["fed"], json!({"owner": "casa", "local": true, "version": 1}), "{name}");
        // O protocolo sai depois da resposta.
        let (python, path) = (n.casa.python.clone(), format!("/api/sessions/{name}/input"));
        fake::wait_until(move || python.hits_to(&path) == 1).await;
    }
    assert_eq!(peers_of(&sidecar(&n.casa, "s0").unwrap()), ["lab::s3", "s1", "s2"]);
    let remote = sidecar(&n.lab, "s3").expect("lab grava o membro dela");
    assert_eq!(peers_of(&remote), ["casa::s0", "casa::s1", "casa::s2"]);
    assert_eq!(remote["fed"], json!({"owner": "casa", "local": false, "version": 1}));
    assert_eq!(remote["task"], "t");
    // A participante avisa depois de responder ao PUT.
    let python = n.lab.python.clone();
    fake::wait_until(move || python.hits_to("/api/sessions/s3/input") == 1).await;
    assert!(record(&n.lab, gid).is_none(), "só a dona tem registro");
    assert!(n.lab.changes.load(Ordering::SeqCst) > 0, "o PUT avisa a lista");
    // O grupo como cada máquina o vê.
    let (status, seen) = call(n.lab.addr, "GET", "/api/sessions/s3/group", None).await;
    assert_eq!(status, 200, "{seen}");
    assert_eq!((seen["server_id"].clone(), seen["owner"].clone(), seen["gid"].clone()), (json!("lab"), json!("casa"), json!(gid)));
    assert_eq!(members(&seen["members"]), ["casa::s0", "casa::s1", "casa::s2", "lab::s3"]);
    let (status, none) = call(n.lab.addr, "GET", "/api/sessions/s0/group", None).await;
    assert_eq!((status, none["detail"]["code"].clone()), (404, json!("erro_sessao_sem_grupo")));
}

#[tokio::test(flavor = "multi_thread")]
async fn same_name_on_two_servers_are_two_members() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s0"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(members(&record(&n.casa, body["gid"].as_str().unwrap()).unwrap()["members"]), ["casa::s0", "lab::s0"]);
    assert_eq!(peers_of(&sidecar(&n.casa, "s0").unwrap()), ["lab::s0"]);
    assert_eq!(peers_of(&sidecar(&n.lab, "s0").unwrap()), ["casa::s0"]);
}

/// Review Focus 1: `lab` confirma, `vm` não responde; quando `vm` volta, não sobra sidecar lá.
#[tokio::test(flavor = "multi_thread")]
async fn join_partial_when_one_server_is_down() {
    let n = net().await;
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", closed_port().await)]);
    join_gives_up(&n.casa);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1", "vm::s2"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_membro_nao_confirmado");
    assert_eq!(body["warning"]["params"]["membros"], json!(["vm::s2"]));
    assert_eq!(body["members"], json!(["s0", "lab::s1"]));
    let gid = body["gid"].as_str().unwrap().to_owned();
    assert_eq!(members(&record(&n.casa, &gid).unwrap()["members"]), ["casa::s0", "lab::s1"]);
    peers_become(&n.lab, "s1", &["casa::s0"]).await;
    // vm volta: a fila entrega o grupo atual, sem `s2`.
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", n.vm.addr)]);
    let fed = federation::existing(&n.casa.pair).unwrap();
    let pending = { let fed = fed.clone(); move || fed.outbox().pending().is_empty() };
    fake::wait_until(pending).await;
    assert!(sidecar(&n.vm, "s2").is_none(), "nada nasce em vm");
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_member_is_cancelled_in_pending_push() {
    let n = net().await;
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", closed_port().await)]);
    join_gives_up(&n.casa);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1", "vm::s2"]})).await;
    assert_eq!(status, 200, "{body}");
    let fed = federation::existing(&n.casa.pair).unwrap();
    let to_vm: Vec<OutboxItem> = fed.outbox().pending().into_iter().filter(|i| i.destination() == "vm").collect();
    assert!(!to_vm.is_empty(), "o envio a vm continua na fila");
    for item in to_vm {
        let OutboxItem::Push { joining, .. } = item else { panic!("só Push para vm: {item:?}") };
        assert!(joining.is_empty(), "s2 saiu do joining: {joining:?}");
    }
}

/// Ninguém de outra máquina confirma: 502 e nada fica.
#[tokio::test(flavor = "multi_thread")]
async fn nobody_confirmed_is_502_and_undone() {
    let n = net().await;
    peers(&n.casa, &[("lab", closed_port().await), ("vm", n.vm.addr)]);
    join_gives_up(&n.casa);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (502, json!("erro_grupo_membro_nao_confirmado")), "{body}");
    assert!(sidecar(&n.casa, "s0").is_none());
    assert!(std::fs::read_dir(n.casa.pair.join("groups")).unwrap().all(|e| {
        let name = e.unwrap().file_name().into_string().unwrap();
        name.starts_with("outbox") || name.starts_with("self")
    }), "registro apagado");
}

#[tokio::test(flavor = "multi_thread")]
async fn push_never_creates_sidecar_outside_joining() {
    let n = net().await;
    let body = json!({"protocol": 1, "owner": "casa", "version": 1, "task": "", "members": [member("casa", "s0"), member("lab", "s1"), member("lab", "s2")],
        "joining": ["s1"]});
    let (status, answer) = push(&n.lab, "g1", body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["server_id"], "lab");
    assert!(sidecar(&n.lab, "s1").is_some());
    assert!(sidecar(&n.lab, "s2").is_none(), "fora do joining não nasce");
    assert_eq!(peers_of(&sidecar(&n.lab, "s1").unwrap()), ["casa::s0", "s2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn late_push_does_not_resurrect_pending_leave() {
    let n = net().await;
    // A dona fora do ar: o `Leave` fica na fila de lab.
    peers(&n.lab, &[("casa", closed_port().await)]);
    let all = json!([member("casa", "s0"), member("lab", "s1"), member("lab", "s2")]);
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": all, "joining": ["s1", "s2"]})).await;
    assert_eq!(status, 200, "{answer}");
    let fed = federation::existing(&n.lab.pair).unwrap();
    // s2 saiu (o que a saída da Task 9 faz): sidecar apagado e `Leave` à dona.
    std::fs::remove_file(n.lab.pair.join("s2.json")).unwrap();
    fed.outbox().enqueue(vec![OutboxItem::Leave { owner: "casa".into(), gid: "g1".into(), member: "s2".into(), origin: "lab".into() }]).unwrap();
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s2"]})).await;
    assert_eq!(status, 200, "{answer}");
    assert!(sidecar(&n.lab, "s2").is_none(), "quem saiu não volta");
    assert_eq!(peers_of(&sidecar(&n.lab, "s1").unwrap()), ["casa::s0"], "s2 também some da lista dos outros");
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_push_after_dissolve_is_ignored() {
    let n = net().await;
    let all = json!([member("casa", "s0"), member("lab", "s1")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": all, "joining": ["s1"]})).await.0, 200);
    assert!(sidecar(&n.lab, "s1").is_some());
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 3, "dissolved": true})).await;
    assert_eq!(status, 200, "{answer}");
    assert!(sidecar(&n.lab, "s1").is_none(), "sem ninguém daqui, o grupo some");
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s1"]})).await;
    assert_eq!((status, answer["ignored"].clone()), (200, json!(true)), "{answer}");
    assert!(sidecar(&n.lab, "s1").is_none(), "envio velho não recria");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejoin_after_dissolve_skips_the_version_the_machine_remembers() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s1"], "task": "t"})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    // lab já esteve neste grupo até a versão 3 e o deixou: lembra a versão, sem sidecar.
    std::fs::create_dir_all(n.lab.pair.join("groups")).unwrap();
    std::fs::write(n.lab.pair.join("groups/applied.versions"), json!({format!("casa::{gid}"): 3}).to_string()).unwrap();
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s3"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["gid"], gid.as_str());
    let remote = sidecar(&n.lab, "s3").expect("lab grava o membro novo");
    assert_eq!(remote["fed"], json!({"owner": "casa", "local": false, "version": 4}));
    assert_eq!(record(&n.casa, &gid).unwrap()["version"], 4);
    assert_eq!(sidecar(&n.casa, "s0").unwrap()["fed"]["version"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn older_version_is_ignored() {
    let n = net().await;
    let all = json!([member("casa", "s0"), member("lab", "s1")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 3, "task": "nova", "members": all, "joining": ["s1"]})).await.0, 200);
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "task": "velha", "members": all, "joining": ["s1"]})).await;
    assert_eq!((status, answer["version"].clone()), (200, json!(3)), "{answer}");
    let s = sidecar(&n.lab, "s1").unwrap();
    assert_eq!((s["task"].clone(), s["fed"]["version"].clone()), (json!("nova"), json!(3)));
}

#[tokio::test(flavor = "multi_thread")]
async fn asymmetric_peers_refuses_put() {
    let n = net().await;
    peers(&n.lab, &[("casa", n.casa.addr)]);
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "vm", "version": 1, "members": [member("vm", "s0"), member("lab", "s1")],
        "joining": ["s1"]})).await;
    assert_eq!((status, answer["detail"]["code"].clone()), (409, json!("erro_grupo_peer_nao_registrado")), "{answer}");
    assert!(sidecar(&n.lab, "s1").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_with_different_owners_is_refused() {
    let n = net().await;
    assert_eq!(pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await.0, 200);
    assert_eq!(pair(&n.lab, "s2", json!({"peers": ["s3"]})).await.0, 200);
    let lab_before = sidecar(&n.lab, "s2").unwrap();
    let casa_before = sidecar(&n.casa, "s0").unwrap();
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s2"]})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_fusao_entre_donos")), "{body}");
    assert_eq!(sidecar(&n.lab, "s2").unwrap(), lab_before);
    assert_eq!(sidecar(&n.casa, "s0").unwrap(), casa_before);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_group_with_outside_member_becomes_federated() {
    let n = net().await;
    outside_pair(&n.casa, "s0", "pc-ana", "Y");
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s2"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["gid"], "abababab", "o grupo local vira o grupo entre máquinas");
    let rec = record(&n.casa, "abababab").unwrap();
    assert_eq!(rec["outside"], json!([{"server": "casa", "address": "pc-ana::Y"}]));
    let s0 = sidecar(&n.casa, "s0").unwrap();
    assert_eq!(peers_of(&s0), ["lab::s2", "pc-ana::Y"]);
    assert_eq!(s0["fed"]["owner"], "casa");
}

/// I-6A: o par de fora só vale na máquina dele.
#[tokio::test(flavor = "multi_thread")]
async fn outside_member_access_stays_on_its_machine() {
    let n = net().await;
    let share_id = outside_pair(&n.casa, "s0", "pc-ana", "Y");
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s1", "lab::s2"]})).await;
    assert_eq!(status, 200, "{body}");
    let granted: Vec<String> = n.casa.share.store.children(&share_id).into_iter().map(|c| c.session).collect();
    assert!(granted.contains(&"s1".to_owned()), "membro daqui ganha ver e recado: {granted:?}");
    assert!(peers_of(&sidecar(&n.casa, "s1").unwrap()).contains(&"pc-ana::Y".to_owned()));
    let lab = sidecar(&n.lab, "s2").unwrap();
    assert!(!peers_of(&lab).contains(&"pc-ana::Y".to_owned()), "lab não grava o par de fora");
    assert!(n.lab.share.store.all().is_empty() && n.lab.share.pairs.all().is_empty(), "nenhum acesso em lab");
}

#[tokio::test(flavor = "multi_thread")]
async fn join_keeps_fed_and_linked_by() {
    let n = net().await;
    assert_eq!(pair(&n.casa, "s0", json!({"peers": ["lab::s3"]})).await.0, 200);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let mut s1 = sidecar(&n.casa, "s1").unwrap();
    s1["linked_by"] = json!("s0");
    std::fs::write(n.casa.pair.join("s1.json"), s1.to_string()).unwrap();
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s2"]})).await;
    assert_eq!(status, 200, "{body}");
    let s1 = sidecar(&n.casa, "s1").unwrap();
    assert_eq!(s1["linked_by"], "s0");
    assert_eq!(s1["fed"], json!({"owner": "casa", "local": true, "version": 3}));
    peers_become(&n.lab, "s3", &["casa::s0", "casa::s1", "casa::s2"]).await;
}

/// Quem inicia não é a dona: a dona (lab) empurra para casa, que não segura o passe de `s0`.
#[tokio::test(flavor = "multi_thread")]
async fn join_from_participant_does_not_deadlock() {
    let n = net().await;
    assert_eq!(pair(&n.lab, "s1", json!({"peers": ["s2"]})).await.0, 200);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"], Value::Null, "confirmado, sem esperar o prazo");
    assert_eq!(body["owner"], "lab");
    let mut got: Vec<String> = body["members"].as_array().unwrap().iter().map(|m| m.as_str().unwrap().to_owned()).collect();
    got.sort();
    assert_eq!(got, ["lab::s1", "lab::s2", "s0"]);
    let s0 = sidecar(&n.casa, "s0").unwrap();
    assert_eq!(s0["fed"], json!({"owner": "lab", "local": false, "version": 1}));
    // O protocolo chega pela máquina dela, depois de ela responder ao PUT.
    let python = n.casa.python.clone();
    fake::wait_until(move || python.hits_to("/api/sessions/s0/input") == 1).await;
    assert!(record(&n.lab, body["gid"].as_str().unwrap()).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn old_version_peer_is_refused_with_code() {
    let n = net().await;
    // Hangar antigo: rota que não existe responde o 404 do FastAPI, sem código.
    let old = Router::new().fallback(|| async { (axum::http::StatusCode::NOT_FOUND, [("content-type", "application/json")], r#"{"detail":"Not Found"}"#) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, old).await.unwrap() });
    peers(&n.casa, &[("old", addr)]);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["old::x"]})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_grupo_peer_versao_antiga")), "{body}");
    assert!(sidecar(&n.casa, "s0").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn outside_pair_on_fed_group_links_at_owner() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    // O par de fora de lab entra no grupo daqui (antes recusado como mistura com máquina própria).
    let groups = n.lab.state.groups.clone().unwrap();
    let outside = std::collections::HashSet::from(["pc-bia::Z".to_owned()]);
    groups.external_link("s1", "pc-bia::Z", Default::default(), outside).await.expect("grupo entre máquinas aceita par de fora");
    let side = groups.link("s1").await.unwrap().unwrap();
    federation::outside_changed(&n.lab.state, &side, "pc-bia::Z", true).await;
    let casa = n.casa.pair.clone();
    fake::wait_until(move || std::fs::read(casa.join("groups").join(format!("{gid}.json"))).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|r| r["outside"] == json!([{"server": "lab", "address": "pc-bia::Z"}]))).await;
}

fn grant(kind: Kind) -> GuestGrant {
    GuestGrant { token: "t".into(), token_hash: "h".into(), sessions: HashMap::from([("s0".to_string(), kind)]), pair_roots: vec![], invite: true }
}

#[test]
fn guest_cannot_read_group() {
    for kind in [Kind::Share, Kind::Pair] {
        assert!(!guest_allowed(&Method::GET, "/api/sessions/s0/group", &grant(kind)), "{kind:?}");
    }
}

/// Rotas entre máquinas fora da porta do convidado; a recusa do porteiro leva código
/// (`erro_fora_do_convite`), nunca o 404 sem código que outra máquina leria como versão antiga.
#[test]
fn guest_groups_routes_refused_with_code() {
    for (method, path) in [(Method::GET, "/api/groups/g1"), (Method::PUT, "/api/groups/g1"), (Method::POST, "/api/groups/g1/ops")] {
        assert!(!guest_allowed(&method, path, &grant(Kind::Share)), "{method} {path}");
    }
}

/// Sem o token de dono, as rotas entre máquinas recusam com código.
#[tokio::test(flavor = "multi_thread")]
async fn groups_routes_without_owner_token_have_a_code() {
    let n = net().await;
    let resp = client().get(format!("http://{}/api/groups/g1", n.casa.addr)).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let body: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(body["detail"]["code"], "erro_nao_autorizado");
    let (status, body) = call(n.casa.addr, "GET", "/api/groups/nada", None).await;
    assert_eq!((status, body["detail"]["code"].clone(), body["server_id"].clone()), (404, json!("erro_grupo_inexistente"), json!("casa")));
}

/// Porta do Connect: o Python autentica o dono e repassa pela ponte privada; a resposta leva código
/// e `server_id`. Convidado repassado é recusado com código.
#[tokio::test(flavor = "multi_thread")]
async fn connect_forwarded_groups_routes_answer_with_code() {
    let n = net().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let app = hangar_server::routes::terminal_router(n.casa.state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let bridged = |method: reqwest::Method, path: &str| client().request(method, format!("http://{private}/__hangar_server/forwarded"))
        .header("x-hangar-path", path).header("x-hangar-internal", SECRET);
    let body = |r: reqwest::Response| async move { (r.status().as_u16(), serde_json::from_str::<Value>(&r.text().await.unwrap()).unwrap()) };
    let (status, got) = body(bridged(reqwest::Method::GET, "/api/groups/nada").send().await.unwrap()).await;
    assert_eq!((status, got["detail"]["code"].clone(), got["server_id"].clone()), (404, json!("erro_grupo_inexistente"), json!("casa")));
    let (status, got) = body(bridged(reqwest::Method::GET, "/api/sessions/s0/group").send().await.unwrap()).await;
    assert_eq!((status, got["detail"]["code"].clone()), (404, json!("erro_sessao_sem_grupo")));
    let (status, got) = body(bridged(reqwest::Method::GET, "/api/groups/nada").header("x-hangar-forwarded-guest", "1").send().await.unwrap()).await;
    assert_eq!((status, got["detail"]["code"].clone()), (403, json!("erro_fora_do_convite")));
}

/// Renomear pendente nesta máquina vence o envio da dona com o nome velho.
#[tokio::test(flavor = "multi_thread")]
async fn pending_rename_wins_over_old_push() {
    let n = net().await;
    peers(&n.lab, &[("casa", closed_port().await)]);
    let old = json!([member("casa", "s0"), member("lab", "s1")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": old, "joining": ["s1"]})).await.0, 200);
    // O que o renomear da Task 9 faz: aplica aqui e deixa o `Rename` na fila da dona.
    n.lab.state.groups.clone().unwrap().rename("s1", "s2").await.unwrap();
    federation::existing(&n.lab.pair).unwrap().outbox().enqueue(vec![OutboxItem::Rename { owner: "casa".into(), gid: "g1".into(),
        from: "s1".into(), to: "s2".into(), origin: "lab".into() }]).unwrap();
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "task": "t2", "members": old, "joining": []})).await;
    assert_eq!(status, 200, "{answer}");
    let s2 = sidecar(&n.lab, "s2").expect("o nome novo continua no grupo");
    assert_eq!((s2["task"].clone(), s2["fed"]["version"].clone()), (json!("t2"), json!(2)));
    assert!(sidecar(&n.lab, "s1").is_none(), "o nome velho não volta");
}

/// Quem inicia solta o passe de `s0` antes de esperar a dona: um fechamento da porta (rename,
/// troca de conta) não fica preso atrás de outra máquina.
#[tokio::test(flavor = "multi_thread")]
async fn initiator_releases_ingress_before_calling_the_owner() {
    let n = net().await;
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", closed_port().await)]);
    let addr = n.casa.addr;
    let asked = tokio::spawn(async move { call(addr, "POST", "/api/sessions/s0/pair", Some(&json!({"peers": ["lab::s1", "vm::s2"]}))).await });
    // O registro gravado = a dona já espera vm, e segue esperando bem mais que o teto do `close`.
    let pair_dir = n.casa.pair.clone();
    fake::wait_until(move || std::fs::read_dir(pair_dir.join("groups")).is_ok_and(|d| d.flatten()
        .any(|e| { let f = e.file_name().into_string().unwrap(); f.ends_with(".json") && !f.starts_with("outbox") && !f.starts_with("self") }))).await;
    n.casa.runtime.ingress().close("s0", Duration::from_secs(10)).await.expect("o passe de s0 já saiu");
    n.casa.runtime.ingress().open("s0");
    assert_eq!(asked.await.unwrap().0, 200);
}

/// O `PUT` da dona não passa pela porta da sessão: com ela retida, o membro entra mesmo assim.
#[tokio::test(flavor = "multi_thread")]
async fn participant_put_does_not_enter_the_session_gate() {
    let n = net().await;
    n.lab.runtime.ingress().hold("s1", Duration::from_millis(10)).await.unwrap();
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": [member("casa", "s0"), member("lab", "s1")],
        "joining": ["s1"]})).await;
    assert_eq!(status, 200, "{answer}");
    assert!(sidecar(&n.lab, "s1").is_some());
}

/// A versão aplicada sobrevive a reiniciar: um envio velho atrasado não recria quem a dissolução tirou.
#[tokio::test(flavor = "multi_thread")]
async fn older_version_after_restart_is_ignored() {
    let n = net().await;
    let all = json!([member("casa", "s0"), member("lab", "s1")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": all, "joining": ["s1"]})).await.0, 200);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 3, "dissolved": true})).await.0, 200);
    federation::forget(&n.lab.pair);
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s1"]})).await;
    assert_eq!((status, answer["ignored"].clone()), (200, json!(true)), "{answer}");
    assert!(sidecar(&n.lab, "s1").is_none(), "envio velho não recria depois de reiniciar");
}

/// Subida: a dona reenvia cada registro dela, e a participante pede à dona cada grupo em que tem membro.
#[tokio::test(flavor = "multi_thread")]
async fn startup_resends_and_asks_the_owner() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    let bump = |version: u64, task: &str| {
        let mut rec = record(&n.casa, &gid).unwrap();
        rec["version"] = json!(version);
        rec["task"] = json!(task);
        std::fs::write(n.casa.pair.join("groups").join(format!("{gid}.json")), rec.to_string()).unwrap();
    };
    // O envio da versão 2 se perdeu (a dona caiu antes): a subida da dona reenvia.
    bump(2, "nova");
    federation::spawn(n.casa.state.clone()).expect("dona com id").await.unwrap();
    let lab = n.lab.pair.clone();
    fake::wait_until(move || std::fs::read(lab.join("s1.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|s| s["task"] == "nova" && s["fed"]["version"] == 2)).await;
    // A participante subiu sem ter recebido a versão 3: pede à dona.
    bump(3, "mais nova");
    federation::spawn(n.lab.state.clone()).expect("participante com id").await.unwrap();
    let s1 = sidecar(&n.lab, "s1").unwrap();
    assert_eq!((s1["task"].clone(), s1["fed"]["version"].clone()), (json!("mais nova"), json!(3)));
}

/// Na dona, o acesso de fora ao membro novo que não grava volta a quem pediu como aviso com código.
#[tokio::test(flavor = "multi_thread")]
async fn outside_grant_failure_reaches_the_caller() {
    let n = net().await;
    let share_id = outside_pair(&n.casa, "s0", "pc-ana", "Y");
    n.casa.share.store.revoke(&share_id).unwrap();
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s1", "lab::s2"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_acesso_externo_falhou", "{body}");
}

/// Dois joins à mesma máquina saem com a versão atual do registro: o segundo `PUT` tem a mesma
/// versão do primeiro e um `joining` novo, que não pode ser ignorado.
#[tokio::test(flavor = "multi_thread")]
async fn two_joins_to_same_machine_both_get_sidecars() {
    let n = net().await;
    let all = json!([member("casa", "s0"), member("lab", "s1"), member("lab", "s2")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s1"]})).await.0, 200);
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s2"]})).await;
    assert_eq!(status, 200, "{answer}");
    assert!(sidecar(&n.lab, "s2").is_some(), "o segundo membro também entra: {answer}");
    assert_eq!(peers_of(&sidecar(&n.lab, "s1").unwrap()), ["casa::s0", "s2"]);
}

/// Queda no meio da gravação deixa versões misturadas: o reenvio da mesma versão conserta.
#[tokio::test(flavor = "multi_thread")]
async fn mixed_versions_after_crash_are_repaired() {
    let n = net().await;
    let all = json!([member("casa", "s0"), member("lab", "s1"), member("lab", "s2")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 3, "task": "nova", "members": all, "joining": ["s1", "s2"]})).await.0, 200);
    let mut s2 = sidecar(&n.lab, "s2").unwrap();
    s2["fed"]["version"] = json!(2);
    s2["task"] = json!("velha");
    std::fs::write(n.lab.pair.join("s2.json"), s2.to_string()).unwrap();
    let (status, answer) = push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 3, "task": "nova", "members": all, "joining": []})).await;
    assert_eq!(status, 200, "{answer}");
    let s2 = sidecar(&n.lab, "s2").unwrap();
    assert_eq!((s2["task"].clone(), s2["fed"]["version"].clone()), (json!("nova"), json!(3)));
}

/// A dona com o CP_SERVER_ID trocado não responde "grupo inexistente": a participante nunca
/// dissolve o grupo por uma resposta que não seja a dissolução.
#[tokio::test(flavor = "multi_thread")]
async fn owner_id_change_does_not_dissolve_participants() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    // A mesma pasta, agora servida com outro id.
    let renamed = machine(&n.casa.root, "casa2").await;
    let (status, answer) = call(renamed.addr, "GET", &format!("/api/groups/{gid}"), None).await;
    assert_eq!((status, answer["detail"]["code"].clone()), (409, json!("erro_grupo_identificador_trocado")), "{answer}");
    peers(&n.lab, &[("casa", renamed.addr), ("vm", n.vm.addr)]);
    federation::spawn(n.lab.state.clone()).expect("participante com id").await.unwrap();
    let s1 = sidecar(&n.lab, "s1").expect("o grupo continua em lab");
    assert_eq!(s1["fed"]["owner"], "casa");
}

/// A participante responde ao `PUT` antes de avisar as sessões: um aviso lento não faz a dona tirar
/// do grupo quem já entrou.
#[tokio::test(flavor = "multi_thread")]
async fn participant_answers_before_notices() {
    let n = net().await;
    n.lab.python.hold_input(true);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    n.lab.python.hold_input(false);
    n.lab.python.release.notify_waiters();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"], Value::Null, "confirmado sem esperar o aviso: {body}");
    let python = n.lab.python.clone();
    fake::wait_until(move || python.hits_to("/api/sessions/s1/input") == 1).await;
}

/// Um `Leave` que entra na fila enquanto o `PUT` espera o lock do grupo vale para ele.
#[tokio::test(flavor = "multi_thread")]
async fn leave_enqueued_while_waiting_for_the_lock_is_honoured() {
    let n = net().await;
    peers(&n.lab, &[("casa", closed_port().await)]);
    let all = json!([member("casa", "s0"), member("lab", "s1"), member("lab", "s2")]);
    assert_eq!(push(&n.lab, "g1", json!({"protocol": 1, "owner": "casa", "version": 1, "members": all, "joining": ["s1"]})).await.0, 200);
    let groups = n.lab.state.groups.clone().unwrap();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (free_tx, free_rx) = tokio::sync::oneshot::channel::<()>();
    let lock = tokio::spawn(async move { groups.locked(async move { let _ = held_tx.send(()); let _ = free_rx.await; }).await });
    held_rx.await.unwrap();
    let addr = n.lab.addr;
    let body = json!({"protocol": 1, "owner": "casa", "version": 2, "members": all, "joining": ["s2"]});
    let asked = tokio::spawn(async move { call(addr, "PUT", "/api/groups/g1", Some(&body)).await });
    // Dá tempo de o PUT passar das leituras de fora do lock; mais curto só deixa de exercitar a corrida.
    tokio::time::sleep(Duration::from_millis(300)).await;
    federation::existing(&n.lab.pair).unwrap().outbox().enqueue(vec![OutboxItem::Leave { owner: "casa".into(), gid: "g1".into(),
        member: "s2".into(), origin: "lab".into() }]).unwrap();
    free_tx.send(()).unwrap();
    lock.await.unwrap();
    assert_eq!(asked.await.unwrap().0, 200);
    assert!(sidecar(&n.lab, "s2").is_none(), "a saída pendente vale");
}

/// Ninguém de fora confirmou numa fusão: os dois contratos voltam como eram.
#[tokio::test(flavor = "multi_thread")]
async fn undo_restores_both_contracts() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let a = body["gid"].as_str().unwrap().to_owned();
    assert_eq!(pair(&n.casa, "s2", json!({"peers": ["s3"]})).await.0, 200);
    let b = sidecar(&n.casa, "s2").unwrap()["gid"].as_str().unwrap().to_owned();
    let contract = |gid: &str| n.casa.pair.join(format!("grupo-{gid}.md"));
    std::fs::write(contract(&a), "contrato A").unwrap();
    std::fs::write(contract(&b), "contrato B").unwrap();
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", closed_port().await)]);
    join_gives_up(&n.casa);
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s2", "vm::x"]})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (502, json!("erro_grupo_membro_nao_confirmado")), "{body}");
    assert_eq!(std::fs::read_to_string(contract(&a)).unwrap(), "contrato A");
    assert_eq!(std::fs::read_to_string(contract(&b)).unwrap(), "contrato B");
}

/// `op: join` que chega a uma máquina que não é a dona do grupo é recusado sem gravar nada.
#[tokio::test(flavor = "multi_thread")]
async fn misaddressed_join_is_refused() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    let ask = json!({"op": "join", "protocol": 1, "origin": "vm", "members": [member("lab", "s2")], "task": "", "replace_task": false, "expected": {}});
    let (status, answer) = call(n.lab.addr, "POST", &format!("/api/groups/{gid}/ops"), Some(&ask)).await;
    assert_eq!((status, answer["detail"]["code"].clone()), (404, json!("erro_grupo_inexistente")), "{answer}");
    assert!(record(&n.lab, &gid).is_none() && sidecar(&n.lab, "s2").is_none());
}

/// Par de fora que não muda o registro não faz a lista reemitir.
#[tokio::test(flavor = "multi_thread")]
async fn outside_noop_does_not_touch_the_list() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    let groups = n.casa.state.groups.clone().unwrap();
    let before = n.casa.changes.load(Ordering::SeqCst);
    let gone = hangar_server::groups::outbox::OutsideOp::Unlink(hangar_server::groups::model::OutsideRef { server: "casa".into(), address: "pc-x::Y".into() });
    federation::owner_outside(&n.casa.state, &groups, &gid, gone).await.ok().unwrap();
    assert_eq!(n.casa.changes.load(Ordering::SeqCst), before);
}

/// Id trocado sem grupo entre máquinas vivo: o grupo novo grava o id atual, e a operação seguinte
/// não o recusa como troca.
#[tokio::test(flavor = "multi_thread")]
async fn id_change_without_groups_then_new_group_works() {
    let n = net().await;
    std::fs::create_dir_all(n.casa.pair.join("groups")).unwrap();
    std::fs::write(n.casa.pair.join("groups/self.json"), json!({"server_id": "antigo"}).to_string()).unwrap();
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = pair(&n.casa, "s2", json!({"peers": ["s0"]})).await;
    assert_eq!(status, 200, "{body}");
    let saved: Value = serde_json::from_slice(&std::fs::read(n.casa.pair.join("groups/self.json")).unwrap()).unwrap();
    assert_eq!(saved["server_id"], "casa");
}

/// Saída na participante grava na hora e a lista reemite sem esperar a dona.
#[tokio::test(flavor = "multi_thread")]
async fn participant_leave_touches_the_list() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let before = n.lab.changes.load(Ordering::SeqCst);
    let (status, body) = call(n.lab.addr, "DELETE", "/api/sessions/s1/pair", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(sidecar(&n.lab, "s1").is_none());
    assert!(n.lab.changes.load(Ordering::SeqCst) > before, "a saída avisa a lista");
}

/// Na dona, o protocolo ao membro novo sai depois da resposta: uma sessão lenta não estoura o prazo
/// de quem pediu com o grupo já gravado.
#[tokio::test(flavor = "multi_thread")]
async fn owner_answers_before_newcomer_notices() {
    let n = net().await;
    n.casa.python.hold_input(true);
    let asked = tokio::time::timeout(Duration::from_secs(15), pair(&n.casa, "s0", json!({"peers": ["lab::s1"]}))).await;
    n.casa.python.hold_input(false);
    n.casa.python.release.notify_waiters();
    let (status, body) = asked.expect("a dona responde sem esperar o aviso");
    assert_eq!(status, 200, "{body}");
    let python = n.casa.python.clone();
    fake::wait_until(move || python.hits_to("/api/sessions/s0/input") == 1).await;
}

/// Subida com o id trocado e grupo entre máquinas vivo: a fila não abre, e nada sai com o id novo.
#[tokio::test(flavor = "multi_thread")]
async fn startup_with_changed_id_does_not_open_the_queue() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    federation::forget(&n.casa.pair);
    let renamed = machine(&n.casa.root, "casa2").await;
    federation::spawn(renamed.state.clone()).expect("máquina com id").await.unwrap();
    assert!(federation::existing(&n.casa.pair).is_none(), "a fila não abriu com o id trocado");
}

/// `/deliver` pela porta do Connect: o recado à sessão que só o Python atende volta na resposta,
/// para ele entregar depois, sem o Rust chamá-lo dentro do pedido dele.
#[tokio::test(flavor = "multi_thread")]
async fn connect_forwarded_deliver_defers_python_sessions() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    let python = n.lab.python.clone();
    fake::wait_until(move || python.hits_to("/api/sessions/s1/input") == 1).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let app = hangar_server::routes::terminal_router(n.lab.state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let ask = json!({"protocol": 1, "origin": "casa", "sender": "s0", "text": "oi", "notice": "n1"});
    let resp = client().post(format!("http://{private}/__hangar_server/forwarded")).header("x-hangar-path", format!("/api/groups/{gid}/deliver"))
        .header("x-hangar-internal", SECRET).header("content-type", "application/json").body(ask.to_string()).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let got: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(got["hangar_deliveries"], json!([{"name": "s1", "text": "[grupo: casa::s0] oi", "steer": false}]), "{got}");
    assert_eq!(got["failed"], json!([]));
    assert_eq!(inputs(&n.lab, "s1"), 1, "o Rust não chamou o Python");
}

/// Fusão entre máquinas cujo contrato do grupo absorvido não pôde ser herdado: o grupo fica, mas a
/// resposta avisa com código e o diário recebe a falha.
#[tokio::test(flavor = "multi_thread")]
async fn fusion_contract_failure_is_a_coded_warning() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let a = body["gid"].as_str().unwrap().to_owned();
    assert_eq!(pair(&n.casa, "s2", json!({"peers": ["s3"]})).await.0, 200);
    let b = sidecar(&n.casa, "s2").unwrap()["gid"].as_str().unwrap().to_owned();
    for gid in [&a, &b] { std::fs::write(n.casa.pair.join(format!("grupo-{gid}.md")), [0xff, 0xfe, b'x']).unwrap(); }
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["s2"]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_contrato_nao_herdado", "{body}");
    assert_eq!(body["warning"]["params"]["grupos"].as_array().map(Vec::len), Some(1), "{body}");
    let python = n.casa.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_contract_merge_failed")).await;
}

/// Registro da dona ilegível: o grupo da sessão responde com o código da falha, nunca com a vista
/// do sidecar como se nada tivesse acontecido.
#[tokio::test(flavor = "multi_thread")]
async fn session_group_with_unreadable_record_is_a_store_failure() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    std::fs::write(n.casa.pair.join("groups").join(format!("{gid}.json")), "{torto").unwrap();
    let (status, answer) = call(n.casa.addr, "GET", "/api/sessions/s0/group", None).await;
    assert_eq!(status, 500, "{answer}");
}

/// Registro da dona ilegível no aviso de grupo: a recusa com código volta, e a causa vai ao diário.
#[tokio::test(flavor = "multi_thread")]
async fn group_notice_with_unreadable_record_reaches_the_journal() {
    let n = net().await;
    let (status, body) = pair(&n.casa, "s0", json!({"peers": ["lab::s1"]})).await;
    assert_eq!(status, 200, "{body}");
    let gid = body["gid"].as_str().unwrap().to_owned();
    std::fs::write(n.casa.pair.join("groups").join(format!("{gid}.json")), "{torto").unwrap();
    let (status, answer) = call(n.casa.addr, "POST", "/api/sessions/s0/group-message", Some(&json!({"text": "oi"}))).await;
    assert_eq!((status, answer["detail"]["params"]["detalhe"].clone()), (503, json!("groups_store_failed")), "{answer}");
    let python = n.casa.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_store_failed")).await;
}
