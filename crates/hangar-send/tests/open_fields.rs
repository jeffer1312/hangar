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
