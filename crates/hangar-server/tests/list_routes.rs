//! Rotas da lista do dono no Rust (`GET /api/sessions` e `/api/sessions/events`): um produtor por
//! servidor, `list_error` uma vez na transição, `nav` uma vez por cliente, retrato de até 2 s fresco
//! depois de invalidação, 503 com código e convidado com o Python. O multiplexador é um script que
//! conta as chamadas e recusa enquanto existir o arquivo `fail`.
#![cfg(unix)]
mod fake;
mod list_support;

use fake::{OWNER, client, next_named};
use hangar_server::list::hub::HeadlessSource;
use list_support::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Cinco listas abertas pagam a varredura de uma: o produtor é do servidor, não da conexão.
#[tokio::test(flavor = "multi_thread")]
async fn one_producer_for_many_clients() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let mut streams = Vec::new();
    for _ in 0..5 {
        streams.push(open(srv.addr).await);
    }
    for es in &mut streams {
        let ev = next_named(es, "sessions").await;
        assert_eq!(names(&serde_json::from_str(&ev.data).unwrap()), ["s0", "s1"]);
    }
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let scans = calls(dir.path(), "list-panes");
    assert!((1..=5).contains(&scans), "uma varredura por tique, não por cliente: {scans}");
    assert!(srv.python.list_facts_last.lock().unwrap()["owner_clients"] == 5, "o Python sabe quantas listas do dono estão abertas");
    drop(streams);
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let after = calls(dir.path(), "list-panes");
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert_eq!(calls(dir.path(), "list-panes"), after, "sem cliente, o produtor para");
}

/// Falha vira `list_error` uma vez; a volta reemite a mesma lista para limpar o erro na tela.
#[tokio::test(flavor = "multi_thread")]
async fn error_then_recovery_reemits_same_list() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let mut es = open(srv.addr).await;
    let first = next_named(&mut es, "sessions").await.data;
    std::fs::write(dir.path().join("fail"), "").unwrap();
    let err = next_named(&mut es, "list_error").await;
    assert_eq!(serde_json::from_str::<Value>(&err.data).unwrap()["code"], "mux_unavailable");
    std::fs::remove_file(dir.path().join("fail")).unwrap();
    let again = next_named(&mut es, "sessions").await.data;
    assert_eq!(again, first, "mesma lista, reemitida depois do erro");
    let python = srv.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.list_failed" && d["codigo"] == "mux_unavailable")).await;
}

/// O navegador pedido sai uma vez em cada lista aberta, nunca de novo no tique seguinte.
#[tokio::test(flavor = "multi_thread")]
async fn nav_once_per_client() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    set_facts(&srv.python, "nav", json!({"s0": {"url": "http://127.0.0.1:5173/", "ts": 7.0}}));
    let mut a = open(srv.addr).await;
    let nav = next_named(&mut a, "nav").await;
    assert_eq!(serde_json::from_str::<Value>(&nav.data).unwrap(), json!({"name": "s0", "url": "http://127.0.0.1:5173/"}));
    let mut b = open(srv.addr).await;
    next_named(&mut b, "nav").await;
    let more = tokio::time::timeout(Duration::from_millis(3500), next_named(&mut a, "nav")).await;
    assert!(more.is_err(), "o mesmo pedido não sai duas vezes na mesma lista");
}

/// `GET` dentro de 2 s reaproveita o retrato; depois de invalidar, produz de novo.
#[tokio::test(flavor = "multi_thread")]
async fn get_after_invalidate_is_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let (status, body) = get(srv.addr, OWNER).await;
    assert_eq!((status, names(&body)), (200, vec!["s0"]));
    get(srv.addr, OWNER).await;
    assert_eq!(calls(dir.path(), "list-panes"), 1, "retrato de até 2 s");
    srv.list.invalidate();
    get(srv.addr, OWNER).await;
    assert_eq!(calls(dir.path(), "list-panes"), 2, "invalidado não serve o de antes");
    assert_eq!(srv.python.hits_to("/api/sessions"), 0, "a lista do dono nunca vai ao Python");
}

#[tokio::test(flavor = "multi_thread")]
async fn mux_unavailable_is_503() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    std::fs::write(dir.path().join("fail"), "").unwrap();
    let (status, body) = get(srv.addr, OWNER).await;
    assert_eq!(status, 503);
    assert_eq!(body["detail"]["code"], "erro_mux_indisponivel");
    assert_eq!(body["detail"]["params"]["detalhe"], "mux_refused");
    let python = srv.python.clone();
    fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.list_route_failed" && d["codigo"] == "mux_unavailable")).await;
}

/// Sem nenhuma resposta dos fatos, acesso e escondidas são desconhecidos: nem `GET` nem SSE servem.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_facts_are_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    *srv.python.list_facts.lock().unwrap() = (json!({"quebrado": true}), Duration::ZERO);
    let (status, body) = get(srv.addr, OWNER).await;
    assert_eq!((status, &body["detail"]["code"], &body["detail"]["params"]["detalhe"]),
        (503, &json!("erro_lista_indisponivel"), &json!("list_facts_unknown")));
    let mut es = open(srv.addr).await;
    let err = next_named(&mut es, "list_error").await;
    assert_eq!(serde_json::from_str::<Value>(&err.data).unwrap()["code"], "list_facts_unknown");
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_token_goes_to_python() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    assert_eq!(get(srv.addr, "convidado").await, (200, json!("from-python")));
    let resp = client().get(format!("http://{}/api/sessions/events?token=convidado", srv.addr)).send().await.unwrap();
    assert_eq!(resp.text().await.unwrap(), "from-python");
    let resp = client().post(format!("http://{}/api/sessions", srv.addr)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(resp.text().await.unwrap(), "from-python", "criar sessão continua no Python");
    assert_eq!(calls(dir.path(), "list-panes"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_from_owner_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    set_facts(&srv.python, "hidden", json!(["s1"]));
    let (_, body) = get(srv.addr, OWNER).await;
    assert_eq!(names(&body), ["s0"]);
    let mut es = open(srv.addr).await;
    let ev = next_named(&mut es, "sessions").await;
    assert_eq!(names(&serde_json::from_str(&ev.data).unwrap()), ["s0"]);
}

/// Fatos que caem depois de uma resposta boa marcam as linhas; a lista continua servida.
#[tokio::test(flavor = "multi_thread")]
async fn facts_down_marks_rows_not_list() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let mut es = open(srv.addr).await;
    let ok: Value = serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap();
    assert_eq!(ok[0]["problema"], Value::Null);
    *srv.python.list_facts.lock().unwrap() = (json!({"quebrado": true}), Duration::ZERO);
    let marked: Value = serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap();
    assert_eq!(marked[0]["problema"], "list_facts_unavailable");
}

struct Runtime(BTreeMap<String, Value>);

impl HeadlessSource for Runtime {
    fn snapshots(&self) -> futures_util::future::BoxFuture<'_, BTreeMap<String, Value>> {
        Box::pin(async move { self.0.clone() })
    }
}

/// Sessão sem terminal: com o runtime de pé, o estado vem do retrato dele (pela chave da sessão),
/// sem `list_runtime_absent`; retrato com erro aparece na linha.
#[tokio::test(flavor = "multi_thread")]
async fn headless_rows_take_the_runtime_state() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 0).await;
    let hl = dir.path().join("home/.hangar/claude-headless");
    std::fs::create_dir_all(&hl).unwrap();
    for (name, key) in [("h0", "kh0"), ("h1", "kh1")] {
        std::fs::write(hl.join(format!("{name}.json")), json!({"name": name, "session_id": format!("sid-{name}"),
            "cwd": dir.path(), "key": key}).to_string()).unwrap();
    }
    srv.list.set_runtime(Arc::new(Runtime(BTreeMap::from([
        ("kh0".into(), json!({"view": {"alive": true, "public_state": {"state": "working", "label": "pensando"}}})),
        ("kh1".into(), json!({"error": "runtime_closed"})),
    ]))));
    let (status, body) = get(srv.addr, OWNER).await;
    assert_eq!(status, 200);
    let row = |n: &str| body.as_array().unwrap().iter().find(|r| r["name"] == n).unwrap().clone();
    assert_eq!((row("h0")["state"].clone(), row("h0")["problema"].clone()), (json!("working"), Value::Null));
    assert_eq!(row("h1")["problema"], "list_runtime_unavailable");
}

/// Marcador escrito sai na lista sem esperar o tique de 1,5 s, e sem uma descoberta por escrita.
#[tokio::test(flavor = "multi_thread")]
async fn marker_change_publishes_without_tick() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let mut es = open(srv.addr).await;
    next_named(&mut es, "sessions").await;
    let start = std::time::Instant::now();
    let scans = calls(dir.path(), "list-panes");
    let mut slow = Vec::new();
    for (k, state) in ["working", "idle", "working", "idle", "working"].into_iter().enumerate() {
        // Fases diferentes do tique: só o tique daria uma espera de até 1,5 s.
        tokio::time::sleep(Duration::from_millis(330 * k as u64 % 1100)).await;
        let t = std::time::Instant::now();
        write_marker(dir.path(), 0, state);
        let (_, took) = until_state(&mut es, 0, state, t).await;
        if took > Duration::from_millis(600) { slow.push(took); }
    }
    assert!(slow.is_empty(), "marcador demorou o tique: {slow:?}");
    let ticks = (start.elapsed().as_secs_f64() / 1.5).ceil() as usize + 1;
    let scans = calls(dir.path(), "list-panes") - scans;
    assert!(scans <= ticks, "a escrita não roda a descoberta: {scans} varreduras em {ticks} tiques");
}

/// Rajada de escritas vira uma publicação com todas, não uma por arquivo.
#[tokio::test(flavor = "multi_thread")]
async fn burst_of_writes_coalesces() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 10).await;
    let mut es = open(srv.addr).await;
    next_named(&mut es, "sessions").await;
    for (round, state) in ["working", "idle", "working"].into_iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(400 * round as u64)).await;
        let t = std::time::Instant::now();
        for i in 0..10 {
            write_marker(dir.path(), i, state);
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
        // Um tique pode cair no meio da rajada e publicar parte dela, e o FSEvents do macOS entrega a
        // rajada em lotes; o que o teste barra é uma publicação por arquivo, que seriam dez.
        let mut published = 0;
        let took = loop {
            let rows: Value = serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap();
            published += 1;
            if rows.as_array().unwrap().iter().all(|r| r["state"] == state) { break t.elapsed() }
        };
        assert!(published <= 3, "rajada saiu em {published} publicações");
        assert!(took < Duration::from_millis(700), "rajada esperou o tique: {took:?}");
    }
}

/// Só o sidecar de grupo de `s1` muda: sem o `pair_*` na assinatura a lista ficava com o selo velho.
#[tokio::test(flavor = "multi_thread")]
async fn pair_change_alone_reemits_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 2).await;
    let mut es = open(srv.addr).await;
    let first: Value = serde_json::from_str(&next_named(&mut es, "sessions").await.data).unwrap();
    assert_eq!(first[1]["pair_peers"], Value::Null);
    let pair = dir.path().join("home/.claude/.hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();
    std::fs::write(pair.join("s1.json"), json!({"peers": ["s0"], "task": "T", "gid": "g1"}).to_string()).unwrap();
    let again: Value = tokio::time::timeout(Duration::from_millis(3500), next_named(&mut es, "sessions")).await
        .map(|ev| serde_json::from_str(&ev.data).unwrap()).expect("a lista não reemitiu");
    assert_eq!((again[1]["pair_peers"].clone(), again[1]["pair_gid"].clone()), (json!(["s0"]), json!("g1")));
    std::fs::remove_file(pair.join("s1.json")).unwrap();
    let gone: Value = tokio::time::timeout(Duration::from_millis(3500), next_named(&mut es, "sessions")).await
        .map(|ev| serde_json::from_str(&ev.data).unwrap()).expect("a lista não reemitiu a saída");
    assert_eq!(gone[1]["pair_peers"], Value::Null);
}

