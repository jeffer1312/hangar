//! Ticket da sessão, leitura uma por vez, resumo do chip e envio das decisões, com processo real.
mod common;

use common::{Bench, review};
use hangar_work_review::{DecideBody, Via};
use serde_json::{Value, json};
use std::time::Duration;

fn body(review_id: &str, decisions: Value) -> DecideBody { serde_json::from_value(json!({"review_id": review_id, "decisions": decisions})).unwrap() }

#[tokio::test(flavor = "multi_thread")]
async fn no_link_and_no_branch_is_not_linked() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.session("s1", "life-1", None);
    assert!(bench.engine.session_review("s1").await.unwrap().is_none());
    assert!(bench.engine.session_review("desconhecida").await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_connector_without_the_capability_is_unsupported() {
    let bench = Bench::new();
    bench.connector("plain", false, &[]);
    let session = bench.session("s1", "life-1", Some("PM-1"));
    bench.link("plain", "1", &session);
    let linked = bench.engine.session_review("s1").await.unwrap().unwrap();
    assert!(!linked.supported);
    assert!(linked.review.is_none());
    assert!(bench.calls("plain", "work_review.get").is_empty(), "nem pergunta ao conector");
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_review_is_read_and_published_for_this_life_only() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["47"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    let linked = bench.engine.session_review("s1").await.unwrap().unwrap();
    let target = &linked.target;
    assert_eq!((target.via, target.connector.as_str(), target.ticket_id.as_str(), target.key.as_str()), (Via::Ticket, "help", "1", "PM-1"));
    assert!(linked.supported);
    assert_eq!(linked.review.unwrap().id, "r-1");
    let summary = &bench.summaries(&[("s1", Some("life-1"))])["s1"];
    assert_eq!((summary["review_id"].as_str(), summary["state"].as_str(), summary["badge"]["tone"].as_str()), (Some("r-1"), Some("waiting"), Some("warning")));
    // Review Focus 5: renomeada, fechada ou recriada sai do evento.
    assert!(bench.summaries(&[("s1", Some("life-2"))]).get("s1").is_none(), "a sessão recriada não herda");
    assert!(bench.summaries(&[("outra", None)]).get("s1").is_none(), "a que saiu da lista some");
}

#[tokio::test(flavor = "multi_thread")]
async fn branch_lookup_matches_the_key_and_caches_until_the_branch_changes() {
    let bench = Bench::new();
    bench.connector("help", true, &["--plain-key", "on"]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    bench.session("s1", "life-1", Some("PM-7"));
    let linked = bench.engine.session_review("s1").await.unwrap().unwrap();
    assert_eq!((linked.target.via, linked.target.ticket_id.as_str(), linked.target.key.as_str()), (Via::Branch, "PM-7", "PM-7"));
    bench.engine.session_review("s1").await.unwrap();
    assert_eq!(bench.calls("help", "work_tracking.get").len(), 1, "o achado pela branch fica guardado");
    bench.session("s1", "life-1", Some("PM-8"));
    bench.engine.session_review("s1").await.unwrap();
    let asked: Vec<Value> = bench.calls("help", "work_tracking.get").into_iter().map(|params| params["ticket_id"].clone()).collect();
    assert_eq!(asked, vec![json!("PM-7"), json!("PM-8")], "branch nova é outra busca");
}

#[tokio::test(flavor = "multi_thread")]
async fn branch_mismatch_is_cached_as_negative_but_an_error_is_not() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.connector("down", true, &["--fail", "work_tracking.get=unavailable"]);
    bench.session("s1", "life-1", Some("PM-7"));
    let failure = bench.engine.session_review("s1").await.unwrap_err();
    assert_eq!(failure.code(), "unavailable", "sem achado e com erro, a pessoa vê o erro");
    bench.engine.session_review("s1").await.unwrap_err();
    assert_eq!(bench.calls("help", "work_tracking.get").len(), 1, "key #PM-7 não é a branch: negativo guardado");
    assert_eq!(bench.calls("down", "work_tracking.get").len(), 2, "erro não vira negativo");
}

#[tokio::test(flavor = "multi_thread")]
async fn ticket_link_wins_over_the_branch() {
    let bench = Bench::new();
    bench.connector("help", true, &["--plain-key", "on"]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", Some("PM-7"));
    bench.link("help", "1", &session);
    assert_eq!(bench.engine.session_review("s1").await.unwrap().unwrap().target.via, Via::Ticket);
    assert!(bench.calls("help", "work_tracking.get").is_empty());
}

/// Review Focus 2: o pedido que chega durante a leitura não abre outra; relê uma vez no fim.
#[tokio::test(flavor = "multi_thread")]
async fn one_read_at_a_time_with_a_single_reread() {
    let bench = Bench::new();
    bench.connector("help", true, &["--delay", "work_review.get=500"]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    for _ in 0..3 { bench.engine.refresh(session.clone()); }
    bench.changed().await;
    // A segunda leitura lê o arquivo depois do atraso: a mudança daqui entra nela.
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:05:00Z"));
    bench.changed().await;
    assert_eq!(bench.summaries(&[("s1", Some("life-1"))])["s1"]["updated_at"], "2026-11-18T10:05:00Z");
    let quiet = tokio::time::timeout(Duration::from_millis(1500), bench.engine.changed().notified()).await;
    assert!(quiet.is_err(), "nenhuma terceira leitura publicou");
    assert_eq!(bench.calls("help", "work_review.get").len(), 2, "três pedidos, duas leituras");
}

#[tokio::test(flavor = "multi_thread")]
async fn first_sight_only_rereads_a_session_without_summary() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.refresh_if_unseen(session.clone());
    bench.changed().await;
    bench.engine.refresh_if_unseen(session.clone());
    bench.engine.session_review("s1").await.unwrap();
    assert_eq!(bench.calls("help", "work_review.get").len(), 2, "a segunda primeira vista não leu; a outra é a da pessoa");
}

/// Review Focus 5: o vínculo trocado de ticket descarta o resumo do anterior, mesmo se a leitura nova falha.
#[tokio::test(flavor = "multi_thread")]
async fn relinked_ticket_drops_the_old_summary_and_the_failure_goes_to_diag() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.connector("down", true, &["--fail", "work_review.get=timeout"]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.refresh(session.clone());
    bench.changed().await;
    assert!(bench.summaries(&[("s1", Some("life-1"))]).get("s1").is_some());
    bench.link("down", "2", &session);
    bench.engine.refresh(session.clone());
    bench.changed().await;
    assert!(bench.summaries(&[("s1", Some("life-1"))]).get("s1").is_none());
    bench.diag.reported().await;
    let reported = bench.diag.0.lock().unwrap().clone();
    assert_eq!(reported, vec![("work_review_refresh_failed".into(), "s1".into(), "timeout".into())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn background_failure_keeps_the_previous_summary_and_warns_once() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.session_review("s1").await.unwrap();
    // Revisão fora do contrato: a leitura falha (protocol_error).
    bench.set_review("help", json!({"id": "r-1"}));
    bench.engine.session_review("s1").await.unwrap_err();
    assert!(bench.diag.0.lock().unwrap().is_empty(), "a falha que a pessoa pediu vai para a tela, não ao diário");
    bench.engine.refresh(session.clone());
    bench.diag.reported().await;
    assert_eq!(bench.summaries(&[("s1", Some("life-1"))])["s1"]["review_id"], "r-1", "vale o resumo anterior");
    bench.engine.refresh(session.clone());
    let again = tokio::time::timeout(Duration::from_millis(1500), bench.diag.1.notified()).await;
    assert!(again.is_err(), "o mesmo aviso não se repete no mesmo minuto");
    assert_eq!(bench.diag.0.lock().unwrap().len(), 1);
}

/// Review Focus 1: decisão errada nunca chega ao conector.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_decisions_never_reach_the_connector() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["1", "2"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    let refused = [
        ("r-0", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "igualar"}]), "stale_review"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}]), "invalid_params"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "apagar"}]), "invalid_params"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "manter"}]), "invalid_params"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "manter", "reason": "   "}]), "invalid_params"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "igualar"}, {"id": "9", "choice": "igualar"}]), "invalid_params"),
        ("r-1", json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "manter", "reason": "x".repeat(2001)}]), "invalid_params"),
    ];
    for (review_id, decisions, code) in refused {
        let failure = bench.engine.decide("s1", body(review_id, decisions.clone())).await.unwrap_err();
        assert_eq!(failure.code(), code, "{review_id} {decisions}");
    }
    assert!(bench.calls("help", "work_review.decide").is_empty());
    let decisions = json!([{"id": "1", "choice": "igualar"}, {"id": "2", "choice": "manter", "reason": "padrão do kit"}]);
    let after = bench.engine.decide("s1", body("r-1", decisions.clone())).await.unwrap();
    assert_eq!(after.unwrap().id, "r-1", "relida depois do envio");
    let sent = bench.calls("help", "work_review.decide");
    assert_eq!(sent, vec![json!({"ticket_id": "1", "review_id": "r-1", "decisions": decisions})]);
}

/// Contrato item 13: lote já aceito (mesmo `review_id`, nenhuma enviada ainda pendente) e revisão fora
/// de `waiting` passam ao conector, que é idempotente.
#[tokio::test(flavor = "multi_thread")]
async fn accepted_batch_and_left_waiting_pass_through() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["51"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.decide("s1", body("r-1", json!([{"id": "1", "choice": "igualar"}]))).await.unwrap();
    bench.set_review("help", review("r-1", "applying", &[], "2026-11-18T10:01:00Z"));
    bench.engine.decide("s1", body("r-1", json!([{"id": "51", "choice": "manter"}]))).await.unwrap();
    assert_eq!(bench.calls("help", "work_review.decide").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn asset_is_cached_and_a_new_round_drops_the_old_one() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["1"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.session_review("s1").await.unwrap();
    let image = bench.engine.asset("s1", "a:1", "r-1").await.unwrap();
    assert_eq!(image.mime, "image/png");
    assert!(image.bytes.starts_with(b"\x89PNG"));
    assert_eq!(bench.engine.asset("s1", "a:1", "r-1").await.unwrap(), image);
    assert_eq!(bench.calls("help", "work_review.asset").len(), 1, "a segunda veio do disco");
    bench.set_review("help", review("r-2", "running", &[], "2026-11-18T10:01:00Z"));
    bench.engine.session_review("s1").await.unwrap();
    bench.engine.asset("s1", "a:1", "r-1").await.unwrap();
    assert_eq!(bench.calls("help", "work_review.asset").len(), 2, "rodada nova apagou as imagens da anterior");
}

/// O celular que perde a conexão no meio do envio não pode deixar o chip parado em `waiting`.
#[tokio::test(flavor = "multi_thread")]
async fn decide_dropped_mid_send_still_finishes_and_rereads() {
    let bench = Bench::new();
    bench.connector("help", true, &["--delay", "work_review.decide=1500"]);
    bench.set_review("help", review("r-1", "waiting", &["1"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.session_review("s1").await.unwrap();
    bench.changed().await;
    let decide = bench.engine.decide("s1", body("r-1", json!([{"id": "1", "choice": "igualar"}])));
    tokio::select! {
        _ = decide => panic!("o envio devia estar preso no atraso do conector"),
        () = bench.wait_call("help", "work_review.decide") => {}
    }
    bench.set_review("help", review("r-1", "applying", &[], "2026-11-18T10:01:00Z"));
    let reread = tokio::time::timeout(Duration::from_secs(10), bench.engine.changed().notified()).await;
    assert!(reread.is_ok(), "o envio interrompido não releu a revisão");
    assert_eq!(bench.summaries(&[("s1", Some("life-1"))])["s1"]["state"], "applying");
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_session_forgets_its_branch_lookup() {
    let bench = Bench::new();
    bench.connector("help", true, &["--plain-key", "on"]);
    bench.set_review("help", review("r-1", "running", &[], "2026-11-18T10:00:00Z"));
    bench.session("s1", "life-1", Some("PM-7"));
    bench.engine.session_review("s1").await.unwrap();
    bench.summaries(&[]);
    bench.engine.session_review("s1").await.unwrap();
    assert_eq!(bench.calls("help", "work_tracking.get").len(), 2, "a busca da sessão que saiu não fica guardada");
}

#[tokio::test(flavor = "multi_thread")]
async fn asset_uses_the_ticket_already_read_for_the_session() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["1"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.session_review("s1").await.unwrap();
    bench.store.remove("help", "1").unwrap();
    assert!(bench.engine.asset("s1", "a:1", "r-1").await.is_ok(), "a imagem não relê os Tickets a cada pedido");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_current_round_goes_to_the_disk_cache() {
    let bench = Bench::new();
    bench.connector("help", true, &[]);
    bench.set_review("help", review("r-1", "waiting", &["1"], "2026-11-18T10:00:00Z"));
    let session = bench.session("s1", "life-1", None);
    bench.link("help", "1", &session);
    bench.engine.session_review("s1").await.unwrap();
    for _ in 0..2 { bench.engine.asset("s1", "a:1", "r-0").await.unwrap(); }
    assert_eq!(bench.calls("help", "work_review.asset").len(), 2, "rodada que não é a atual não vai ao disco");
    for _ in 0..2 { bench.engine.asset("s1", "a:1", "r-1").await.unwrap(); }
    assert_eq!(bench.calls("help", "work_review.asset").len(), 3, "a atual fica");
}
