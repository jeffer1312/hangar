//! Trava da abertura única: todo campo da tabela de abertura (`hangar_session_open_fields`) tem
//! flag no `--new` e chega ao corpo do `POST /api/sessions` com a chave do pedido.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;

use common::{Answer, FakeHost, FakeServer, Home, Route};
use hangar_session_open_fields::{Cli, FIELDS};
use serde_json::Value;

#[test]
fn new_says_every_open_field() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions", 200, r#"{"name": "s", "provider": "claude"}"#)]);
    let home = Home::new(server.port, "");
    let prep = BTreeMap::from([("conta --prep".to_owned(), Answer { stdout: "/prep/{2}\n".into(), ..Answer::default() })]);
    let host = FakeHost::new(Some(prep), BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() })]));
    for field in FIELDS {
        let flags: &[&str] = match field.cli { Cli::Positional => &[], Cli::Args(flags) => flags };
        let mut args = vec!["--new", "s", "/p"];
        args.extend_from_slice(flags);
        let done = home.run(&args, &[], &host);
        assert_eq!(done.code, 0, "{flags:?}: {}", done.stderr);
        let post = server.requests().into_iter().rfind(|r| r.method == "POST" && r.path == "/api/sessions")
            .unwrap_or_else(|| panic!("{flags:?}: sem POST /api/sessions"));
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert!(body.get(field.body).is_some_and(|v| !v.is_null()), "`{}` não chega ao corpo pelo --new {flags:?}: {body}", field.body);
    }
}

/// Sem servidor:: o `--new` pede a escolha da máquina; a saída dá o nome com a máquina de lá e onde e por quê.
#[test]
fn new_asks_for_the_machine_and_prints_the_target_and_the_reason() {
    let answer = r#"{"name": "s", "provider": "claude", "alvo": "nb::s",
        "placement": {"server": "nb", "reason": {"code": "least_loaded", "local_out": "working"}, "text": "nb tem menos sessões trabalhando"}}"#;
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions", 200, answer)]);
    let home = Home::new(server.port, "");
    let host = FakeHost::new(None, BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() })]));
    let done = home.run(&["--new", "s", "/p"], &[], &host);
    assert_eq!(done.code, 0, "{}", done.stderr);
    let post = server.requests().into_iter().rfind(|r| r.path == "/api/sessions").unwrap();
    let body: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body["machine"], "auto", "{body}");
    assert!(done.stdout.contains("sessão criada: nb::s (/p)") && done.stdout.contains("hangar-send nb::s \"msg\""), "{}", done.stdout);
    assert!(done.stdout.contains("máquina: nb — nb tem menos sessões trabalhando"), "{}", done.stdout);
    // Fora do balanceamento nada é dito além da linha de sempre.
    let kept = r#"{"name": "s", "provider": "claude", "alvo": "s", "placement": {"server": "pc", "reason": {"code": "not_balanced", "why": "link"}, "text": "x"}}"#;
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions", 200, kept)]);
    let done = Home::new(server.port, "").run(&["--new", "s", "/p"], &[], &host);
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert!(!done.stdout.contains("máquina:"), "{}", done.stdout);
    // Servidor e --machine juntos é recusado antes de qualquer pedido.
    let done = home.run(&["--new", "nb::s", "--machine", "vm"], &[], &host);
    assert_eq!(done.code, 2, "{}", done.stderr);
}
