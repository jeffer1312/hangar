//! `--responder`: a resposta à pergunta da filha ligada vai à rota `child-answer` com os cabeçalhos de identidade desta
//! sessão e as respostas cruas; o servidor resolve quem chama, monta o `/answer` e diz se algo pode ter saído.

mod common;

use std::collections::BTreeMap;

use common::{Answer, FakeHost, FakeServer, Home, Outcome, Route, plain_host};
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

fn answered_with(status: u16, body: &str) -> Outcome {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", status, body)]);
    as_parent(&Home::new(server.port, ""), &["--responder", "f", "q1", "1"], &[])
}

#[test]
fn the_replies_go_to_the_child_answer_route_with_this_session_identity_headers() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, r#"{"ok": true, "fallback": false, "maybe_sent": true}"#)]);
    let home = Home::new(server.port, "");
    let done = as_parent(&home, &["--responder", "filha x", "q2", "2", "texto:outra pasta"], &[("TMUX_PANE", "%3"), ("CP_SESSION_NAME", "pai")]);
    assert_eq!((done.code, done.stdout.as_str()), (0, "respondida: filha x\n"), "{}", done.stderr);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/sessions/filha%20x/child-answer");
    assert_eq!(requests[0].header("authorization"), Some("Bearer tok"));
    assert_eq!((requests[0].header("x-hangar-key"), requests[0].header("x-hangar-pane"), requests[0].header("x-hangar-session")),
        (Some("k1"), Some("%3"), Some("pai")));
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body, json!({"interaction_id": "q2", "replies": ["2", "texto:outra pasta"]}), "o pai não vai no corpo");
}

#[test]
fn an_unconfirmed_answer_says_not_to_send_again() {
    let done = answered_with(200, r#"{"sent": true, "confirmed": false, "message": "enviada, sem confirmação; não tente de novo", "maybe_sent": true}"#);
    assert_eq!((done.code, done.stdout.as_str()), (0, "enviada, sem confirmação: f (não tente de novo)\n"), "{}", done.stderr);
}

/// 1 só quando o servidor prova que nada saiu; o resto é 4, "não repita".
#[test]
fn only_a_refusal_that_proves_nothing_left_exits_one() {
    let not_linked = answered_with(403, r#"{"detail": {"code": "erro_nao_ligada", "params": {}, "msg": "erro_nao_ligada: 'f' não é filha ligada a você"}, "maybe_sent": false}"#);
    assert_eq!((not_linked.code, not_linked.stderr.as_str()), (1, "erro: erro_nao_ligada: 'f' não é filha ligada a você\n"));
    let stale = answered_with(409, r#"{"detail": {"code": "erro_interacao_desatualizada", "params": {}, "msg": "Essa pergunta mudou."}, "maybe_sent": false}"#);
    assert_eq!((stale.code, stale.stderr.as_str()), (1, "erro: erro_interacao_desatualizada: Essa pergunta mudou.\n"));
    for (status, body) in [(409, r#"{"detail": {"code": "erro_terminal_ocupado", "msg": "x"}, "maybe_sent": true}"#),
        (500, r#"{"detail": "x"}"#), (409, r#"{"detail": {"code": "outro", "msg": "x"}}"#), (200, r#"{"ok": false}"#), (200, "ilegível")] {
        let done = answered_with(status, body);
        assert_eq!(done.code, 4, "{status} {body}: {}", done.stderr);
        assert!(done.stderr.contains("pode ter saído: não repita"), "{}", done.stderr);
    }
    // Servidor sem a rota: o 404 não abriu pedido nenhum.
    assert_eq!(answered_with(404, r#"{"detail": "Not Found"}"#).code, 1);
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
fn a_server_down_sent_nothing() {
    let done = as_parent(&Home::new(common::dead_port(), ""), &["--responder", "f", "q1", "1"], &[]);
    assert_eq!(done.code, 2, "{}", done.stderr);
}

#[test]
fn nothing_is_sent_without_a_reply_outside_a_session_from_the_attached_client_or_to_another_machine() {
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
    // Só o cliente humano anexado diz o nome: palpite, não identidade.
    let tmux = BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() }),
        ("list-clients".to_owned(), Answer { stdout: "0\tvista\n".into(), ..Answer::default() })]);
    let attached = home.run(&["--responder", "f", "q1", "1"], &[("TMUX", "/tmp/tmux-1000/default,1,0")], &FakeHost::new(Some(BTreeMap::new()), tmux));
    assert_eq!(attached.code, 2, "{}", attached.stderr);
    assert!(attached.stderr.starts_with("erro: --responder não usa a identidade do cliente tmux anexado ('vista')"), "{}", attached.stderr);
    assert!(server.requests().is_empty());
}
