//! Aviso de grupo e contrato num grupo entre máquinas: os dois passam pela dona (`casa`). Três
//! máquinas de verdade no processo, cada uma com pasta, id e `peers.json` próprios; os pares de fora
//! são servidores falsos que anotam cada recado.
#![cfg(unix)]
use crate::fake;
use crate::groups_support::*;
use crate::share_pair_routes::{Other, other_machine};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fake::{OWNER, client};
use hangar_server::groups::federation::Timing;
use hangar_server::share::store::Kind;
use serde_json::{Value, json};

async fn say(m: &Machine, name: &str, text: &str) -> (u16, Value) {
    call(m.addr, "POST", &format!("/api/sessions/{name}/group-message"), Some(&json!({"text": text}))).await
}

fn last_text(m: &Machine) -> String { m.sink.last_any().unwrap() }

/// Espera cada sessão de `m` em `names` chegar a `n` recados.
async fn reach(m: &Machine, names: &[&str], n: usize) {
    let (sink, names): (_, Vec<String>) = (m.sink.clone(), names.iter().map(|s| (*s).to_owned()).collect());
    fake::wait_until(move || names.iter().all(|s| sink.count(s) >= n)).await;
}

/// `casa::s0` + `casa::s1` + `lab::s2` + `vm::s3`, dona `casa`, com o protocolo já entregue.
async fn group(n: &Net) -> String {
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/pair", Some(&json!({"peers": ["s1", "lab::s2", "vm::s3"]}))).await;
    assert_eq!(status, 200, "{body}");
    reach(&n.casa, &["s0", "s1"], 1).await;
    reach(&n.lab, &["s2"], 1).await;
    reach(&n.vm, &["s3"], 1).await;
    body["gid"].as_str().unwrap().to_owned()
}

/// Par de fora de `local` em `m`, apontando para o servidor falso `base`; o sidecar ganha o endereço.
fn outside(m: &Machine, local: &str, alias: &str, peer: &str, base: &str) {
    let (s, _) = m.share.store.create_redeemed(local, Some("t:1".into()), Kind::Pair).unwrap();
    m.share.pairs.add(hangar_server::share::pairs::ExternalPair { share_id: s.id, local_session: local.into(), alias: alias.into(),
        peer_owner: alias.into(), peer_session: peer.into(), peer_address: base.into(), peer_token: format!("tok-{alias}-{}", "t".repeat(30)),
        created_at: 1.0 }).unwrap();
    let path = m.pair.join(format!("{local}.json"));
    let mut side: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    side["peers"].as_array_mut().unwrap().push(json!(format!("{alias}::{peer}")));
    std::fs::write(path, side.to_string()).unwrap();
}

fn group_notices(o: &Other) -> Vec<Value> {
    o.calls().into_iter().filter(|c| c["path"] == "/api/pair/message" && c["body"]["group"] == true).map(|c| c["body"].clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn notice_reaches_every_machine() {
    let n = net().await;
    group(&n).await;
    let (status, body) = say(&n.lab, "s2", "oi de lab").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["pulados"].clone(), body["warning"].clone()), (json!([]), Value::Null), "{body}");
    reach(&n.casa, &["s0", "s1"], 2).await;
    reach(&n.vm, &["s3"], 2).await;
    assert_eq!(last_text(&n.vm), "[grupo: lab::s2] oi de lab");
    assert_eq!(last_text(&n.casa), "[grupo: lab::s2] oi de lab");
    assert_eq!(inputs(&n.lab, "s2"), 1, "quem mandou não recebe o próprio aviso");

    let (status, body) = say(&n.casa, "s0", "oi da dona").await;
    assert_eq!((status, body["pulados"].clone()), (200, json!([])), "{body}");
    reach(&n.lab, &["s2"], 2).await;
    assert_eq!(last_text(&n.lab), "[grupo: casa::s0] oi da dona");
    reach(&n.casa, &["s1"], 3).await;
    assert_eq!(inputs(&n.casa, "s0"), 2);
}

/// Review Focus 6: o membro de fora recebe uma vez, pela máquina do par, com o `notice` da dona.
#[tokio::test(flavor = "multi_thread")]
async fn notice_reaches_outside_once() {
    let n = net().await;
    group(&n).await;
    let (ana, base) = other_machine().await;
    outside(&n.casa, "s0", "pc-ana", "Y", &base);
    assert_eq!(say(&n.lab, "s2", "oi").await.0, 200);
    let got = group_notices(&ana);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0]["text"].clone(), got[0]["session"].clone()), (json!("oi"), json!("Y")));
    assert!(got[0]["notice"].as_str().is_some_and(|s| !s.is_empty()), "{got:?}");
}

/// Duas máquinas próprias com par para a mesma máquina de fora mandam o mesmo `notice`: lá ele entra
/// uma vez só (`share_pair_routes::outside_notice_deduped_across_own_machines`).
#[tokio::test(flavor = "multi_thread")]
async fn own_machines_send_the_same_notice_outside() {
    let n = net().await;
    group(&n).await;
    let (ana, base) = other_machine().await;
    outside(&n.casa, "s0", "pc-ana", "Y", &base);
    outside(&n.lab, "s2", "pc-ana", "Y", &base);
    assert_eq!(say(&n.vm, "s3", "oi").await.0, 200);
    let got = group_notices(&ana);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0]["notice"], got[1]["notice"], "{got:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn storm_counts_only_at_the_owner() {
    let n = net().await;
    group(&n).await;
    for (m, name) in [(&n.lab, "s2"), (&n.vm, "s3"), (&n.lab, "s2"), (&n.casa, "s0"), (&n.vm, "s3")] {
        let (status, body) = say(m, name, "marco").await;
        assert_eq!(status, 200, "{body}");
    }
    let (status, body) = say(&n.lab, "s2", "marco").await;
    assert_eq!((status, body["detail"]["code"].clone()), (429, json!("erro_group_message_tempestade")), "{body}");
}

/// Aviso que chegou de fora em `lab` sobe à dona e alcança as outras máquinas, sem sair a nenhum par
/// de fora (nem ao de `casa`, nem de volta ao de `lab`).
#[tokio::test(flavor = "multi_thread")]
async fn outside_notice_never_leaves_to_third_parties() {
    let n = net().await;
    group(&n).await;
    let (ana, ana_base) = other_machine().await;
    let (bia, bia_base) = other_machine().await;
    outside(&n.casa, "s0", "pc-ana", "Y", &ana_base);
    outside(&n.lab, "s2", "pc-bia", "W", &bia_base);
    // A tarefa acaba depois de a dona responder, e a dona só responde com tudo entregue.
    hangar_server::groups::fed_notice::relay_outside(n.lab.state.clone(), "s2".into(), "pc-bia::W".into(), "oi de fora".into(), "n-1".into())
        .await.unwrap();
    reach(&n.casa, &["s0", "s1"], 2).await;
    reach(&n.vm, &["s3"], 2).await;
    assert_eq!(last_text(&n.vm), "[de fora: pc-bia::W via lab] oi de fora");
    assert_eq!(last_text(&n.casa), "[de fora: pc-bia::W via lab] oi de fora");
    assert!(group_notices(&ana).is_empty() && group_notices(&bia).is_empty(), "nenhum par de fora recebe de novo");
    assert_eq!(inputs(&n.lab, "s2"), 1, "a ponta já entregou aos dela no recado");
}

#[tokio::test(flavor = "multi_thread")]
async fn contract_put_conflict_is_409() {
    let n = net().await;
    let gid = group(&n).await;
    let (status, got) = call(n.lab.addr, "GET", "/api/sessions/s2/pair/contract", None).await;
    assert_eq!(status, 200, "{got}");
    assert_eq!((got["path"].clone(), got["owner"].clone(), got["content"].clone(), got["mtime"].clone()), (json!(""), json!("casa"), json!(""), Value::Null));
    let put = |content: &str, mtime: &Value| json!({"content": content, "mtime": mtime});
    let (status, first) = call(n.lab.addr, "PUT", "/api/sessions/s2/pair/contract", Some(&put("decisão A\n", &Value::Null))).await;
    assert_eq!(status, 200, "{first}");
    let contract = n.casa.state.groups.as_ref().unwrap().contract_path(&gid);
    assert_eq!(std::fs::read_to_string(&contract).unwrap(), "decisão A\n", "o arquivo mora na dona");
    let (status, body) = call(n.vm.addr, "PUT", "/api/sessions/s3/pair/contract", Some(&put("outra\n", &Value::Null))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_contrato_mudou")), "criação é exclusiva: {body}");
    let (status, second) = call(n.vm.addr, "PUT", "/api/sessions/s3/pair/contract", Some(&put("decisão B\n", &first["mtime"]))).await;
    assert_eq!(status, 200, "{second}");
    let (status, body) = call(n.lab.addr, "PUT", "/api/sessions/s2/pair/contract", Some(&put("velho\n", &first["mtime"]))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("erro_contrato_mudou")), "{body}");
    let (status, here) = call(n.casa.addr, "GET", "/api/sessions/s0/pair/contract", None).await;
    assert_eq!(status, 200, "{here}");
    assert_eq!((here["path"].clone(), here["content"].clone(), here["owner"].clone(), here["mtime"].clone()),
        (json!(contract.to_string_lossy()), json!("decisão B\n"), json!("casa"), second["mtime"].clone()));
}

/// Registro ilegível na dona: o mesmo 503 de grupo indisponível do `PUT`/`ops`, também nas rotas
/// de contrato e aviso.
#[tokio::test(flavor = "multi_thread")]
async fn unreadable_record_at_owner_is_503() {
    let n = net().await;
    let gid = group(&n).await;
    let record = n.casa.pair.join("groups").join(format!("{gid}.json"));
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    let (status, body) = call(n.casa.addr, "GET", &format!("/api/groups/{gid}/contract"), None).await;
    assert_eq!((status, body["detail"]["code"].clone(), body["detail"]["params"]["detalhe"].clone(), body["server_id"].clone()),
        (503, json!("erro_grupo_indisponivel"), json!("groups_store_failed"), json!("casa")), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_down_contract_is_503() {
    let n = net().await;
    group(&n).await;
    peers(&n.lab, &[("casa", closed_port().await), ("vm", n.vm.addr)]);
    let (status, body) = call(n.lab.addr, "GET", "/api/sessions/s2/pair/contract", None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (503, json!("erro_grupo_dono_indisponivel")), "{body}");
    let (status, body) = call(n.lab.addr, "PUT", "/api/sessions/s2/pair/contract", Some(&json!({"content": "x", "mtime": null}))).await;
    assert_eq!((status, body["detail"]["code"].clone()), (503, json!("erro_grupo_dono_indisponivel")), "{body}");
}

/// Máquina que não responde a tempo: a dona desiste dela antes de quem mandou desistir da dona, e
/// só os membros dela vêm em `pulados`.
#[tokio::test(flavor = "multi_thread")]
async fn slow_machine_is_skipped_before_the_origin_gives_up() {
    let n = net().await;
    group(&n).await;
    federation_deliver_wait(&n.casa, Duration::from_secs(3));
    let hit = Arc::new(tokio::sync::Notify::new());
    let h = hit.clone();
    let slow = axum::Router::new().fallback(move || {
        let h = h.clone();
        async move { h.notify_one(); tokio::time::sleep(Duration::from_secs(60)).await; "" }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, slow).await.unwrap() });
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", slow_addr)]);
    let started = std::time::Instant::now();
    let (status, body) = say(&n.lab, "s2", "oi").await;
    tokio::time::timeout(Duration::from_secs(15), hit.notified()).await.expect("a dona tentou a máquina lenta");
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["pulados"].clone(), body["warning"].clone()), (json!(["vm::s3"]), Value::Null), "{body}");
    // Quem mandou desiste da dona aos 20 s (`OWNER_WAIT_TIMEOUT`).
    assert!(started.elapsed() < Duration::from_secs(15), "resposta antes de quem mandou desistir: {:?}", started.elapsed());
    reach(&n.casa, &["s0", "s1"], 2).await;
}

/// Sessão da própria dona que não recebe a tempo: a dona responde no prazo dela, com essas sessões
/// em `pulados`, e as outras máquinas recebem em paralelo.
#[tokio::test(flavor = "multi_thread")]
async fn slow_owner_session_is_skipped_in_time() {
    let n = net().await;
    group(&n).await;
    federation_deliver_wait(&n.casa, Duration::from_secs(3));
    for name in ["s0", "s1"] { n.casa.sink.hold(name, Duration::from_secs(20)); }
    let started = std::time::Instant::now();
    let (status, body) = say(&n.lab, "s2", "oi").await;
    let took = started.elapsed();
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["pulados"].clone(), body["warning"].clone()), (json!(["casa::s0", "casa::s1"]), Value::Null), "{body}");
    // Prazo de 3 s contra o padrão de 15 s: passar de 10 s é a dona ignorando o prazo dela.
    assert!(took < Duration::from_secs(10), "a dona não espera a sessão lenta além do prazo: {took:?}");
    reach(&n.vm, &["s3"], 2).await;
}

/// `vm` atrás de um repasse que recusa o primeiro pedido com 503 de modo `python` (a fila tenta de
/// novo depois da escada) e entrega os outros: a confirmação dela chega à dona depois de 8 s.
async fn first_refused(target: SocketAddr) -> SocketAddr {
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let seen = seen.clone();
        async move {
            use axum::response::IntoResponse;
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return (axum::http::StatusCode::SERVICE_UNAVAILABLE, [("content-type", "application/json")],
                    json!({"detail": {"code": "groups_need_rust_server"}}).to_string()).into_response();
            }
            let (parts, body) = req.into_parts();
            let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
            let resp = client().request(parts.method, format!("http://{target}{}", parts.uri.path())).bearer_auth(OWNER)
                .header("content-type", "application/json").body(bytes).send().await.unwrap();
            let status = resp.status();
            (status, [("content-type", "application/json")], resp.bytes().await.unwrap()).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Entrada pedida por uma participante com um membro que confirma perto do prazo da dona: quem pediu
/// espera a dona e recebe o resultado real, não "dona indisponível" aos 8 s.
#[tokio::test(flavor = "multi_thread")]
async fn join_from_participant_with_slow_member_reports_the_real_outcome() {
    let n = net().await;
    // vm confirma na segunda tentativa, aos 9 s: depois dos 8 s de leitura comum, antes do teto da dona.
    timing(&n.casa, Timing { ladder: vec![Duration::from_secs(9)], ..fast() });
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/pair", Some(&json!({"peers": ["lab::s1"]}))).await;
    assert_eq!(status, 200, "{body}");
    peers(&n.casa, &[("lab", n.lab.addr), ("vm", first_refused(n.vm.addr).await)]);
    let started = std::time::Instant::now();
    let (status, body) = call(n.lab.addr, "POST", "/api/sessions/s2/pair", Some(&json!({"peers": ["s1", "vm::s3"]}))).await;
    assert!(started.elapsed() > Duration::from_secs(8), "a confirmação de vm chega depois de 8 s: {:?}", started.elapsed());
    assert_eq!(status, 200, "{body}");
    assert_eq!((body["owner"].clone(), body["warning"].clone()), (json!("casa"), Value::Null), "{body}");
    assert!(body["members"].as_array().unwrap().contains(&json!("vm::s3")), "{body}");
}

fn federation_deliver_wait(m: &Machine, wait: Duration) { timing(m, Timing { deliver_wait: wait, ..fast() }); }

/// Dona fora: os membros da mesma máquina continuam se avisando e as outras vêm em `pulados`.
#[tokio::test(flavor = "multi_thread")]
async fn owner_down_same_machine_still_delivers() {
    let n = net().await;
    let (status, body) = call(n.casa.addr, "POST", "/api/sessions/s0/pair", Some(&json!({"peers": ["lab::s2", "lab::s3"]}))).await;
    assert_eq!(status, 200, "{body}");
    reach(&n.casa, &["s0"], 1).await;
    reach(&n.lab, &["s2", "s3"], 1).await;
    peers(&n.lab, &[("casa", closed_port().await), ("vm", n.vm.addr)]);
    let (status, body) = say(&n.lab, "s2", "oi").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["pulados"], json!(["casa::s0"]), "{body}");
    assert_eq!(body["warning"]["code"], "erro_grupo_dono_indisponivel", "{body}");
    reach(&n.lab, &["s3"], 2).await;
    assert_eq!(last_text(&n.lab), "[grupo: s2] oi");
    assert_eq!(inputs(&n.casa, "s0"), 1);
}
