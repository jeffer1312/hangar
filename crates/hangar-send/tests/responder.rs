//! `--responder`: a resposta à pergunta da filha ligada vai à rota `child-answer` assinada por esta sessão, com as
//! respostas cruas; o servidor monta o `/answer` e decide as recusas.

mod common;

use common::{FakeServer, Home, Outcome, Route, plain_host};
use serde_json::{Value, json};

/// Uma sessão sem terminal chamada `pai`, achada pelo sidecar.
fn as_parent(home: &Home, args: &[&str], extra: &[(&str, &str)]) -> Outcome {
    let dir = home.dir.path().join("headless");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pai.json"), "{\n  \"name\": \"pai\",\n  \"key\": \"k1\"\n}\n").unwrap();
    let dir = dir.to_string_lossy().into_owned();
    let mut vars = vec![("HANGAR_HEADLESS_DIR", dir.as_str()), ("CP_SESSION_KEY", "k1")];
    vars.extend_from_slice(extra);
    home.run(args, &vars, &plain_host())
}

#[test]
fn the_replies_go_to_the_child_answer_route_signed_by_this_session() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, r#"{"ok": true, "fallback": false}"#)]);
    let home = Home::new(server.port, "");
    let done = as_parent(&home, &["--responder", "filha x", "q2", "2", "outra pasta"], &[]);
    assert_eq!((done.code, done.stdout.as_str()), (0, "respondida: filha x\n"), "{}", done.stderr);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/sessions/filha%20x/child-answer");
    assert_eq!(requests[0].header("authorization"), Some("Bearer tok"));
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body, json!({"parent": "pai", "interaction_id": "q2", "replies": ["2", "outra pasta"]}));
}

#[test]
fn an_unconfirmed_answer_says_not_to_send_again() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200,
        r#"{"sent": true, "confirmed": false, "message": "enviada, sem confirmação; não tente de novo"}"#)]);
    let home = Home::new(server.port, "");
    let done = as_parent(&home, &["--responder", "f", "q1", "1"], &[]);
    assert_eq!((done.code, done.stdout.as_str()), (0, "enviada, sem confirmação: f (não tente de novo)\n"), "{}", done.stderr);
}

#[test]
fn the_server_refusal_keeps_its_code() {
    let refusal = r#"{"detail": {"code": "erro_nao_ligada", "params": {}, "msg": "erro_nao_ligada: 'f' não é filha ligada a você"}}"#;
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 403, refusal)]);
    let home = Home::new(server.port, "");
    let done = as_parent(&home, &["--responder", "f", "q1", "1"], &[]);
    assert_eq!((done.code, done.stderr.as_str()), (1, "erro: erro_nao_ligada: 'f' não é filha ligada a você\n"));
    let stale = r#"{"detail": {"code": "erro_interacao_desatualizada", "params": {}, "msg": "Essa pergunta mudou."}}"#;
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 409, stale)]);
    let done = as_parent(&Home::new(server.port, ""), &["--responder", "f", "q1", "1"], &[]);
    assert_eq!((done.code, done.stderr.as_str()), (1, "erro: erro_interacao_desatualizada: Essa pergunta mudou.\n"));
}

/// Sem resposta no prazo a escrita pode ter saído: 4, nunca "nada saiu".
#[test]
fn a_cut_answer_may_have_been_sent() {
    let mut post = Route::new("POST", "/api/sessions/", 200, r#"{"ok": true}"#);
    post.replies[0].delay = 2.0;
    let server = FakeServer::start(vec![post]);
    let home = Home::new(server.port, "");
    let done = as_parent(&home, &["--responder", "f", "q1", "1"], &[("API_TIMEOUT", "0.5")]);
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert!(done.stderr.contains("pode ter saído"), "{}", done.stderr);
}

#[test]
fn nothing_is_sent_without_a_reply_outside_a_session_or_to_another_machine() {
    let server = FakeServer::start(vec![Route::new("*", "", 200, "{}")]);
    let home = Home::new(server.port, "");
    for args in [&["--responder", "f", "q1"][..], &["--responder", "f"], &["--responder"]] {
        let done = as_parent(&home, args, &[]);
        assert_eq!(done.code, 2, "{args:?}");
        assert!(done.stderr.starts_with("uso: hangar-send --responder "), "{}", done.stderr);
    }
    let remote = as_parent(&home, &["--responder", "lab::f", "q1", "1"], &[]);
    assert_eq!((remote.code, remote.stderr.as_str()), (2, "erro: --responder só alcança filha desta máquina\n"));
    let outside = home.run(&["--responder", "f", "q1", "1"], &[], &plain_host());
    assert_eq!((outside.code, outside.stderr.as_str()), (2, "erro: --responder roda dentro da sessão que abriu a filha\n"));
    assert!(server.requests().is_empty());
}
