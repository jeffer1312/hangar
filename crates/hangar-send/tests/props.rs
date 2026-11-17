//! Propriedades das entradas que o comando interpreta: nomes no caminho, caminhos do Git Bash, texto
//! do `--gravar`, o `--root` do atalho e qualquer JSON que o servidor devolva.

mod common;

use common::{FakeServer, Home, Route, plain_host};
use proptest::prelude::*;
use serde_json::Value;

const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-~";

proptest! {
    #[test]
    fn path_segment_round_trips_and_keeps_only_unreserved(name in "\\PC{0,24}") {
        let encoded = hangar_send::path_segment(&name);
        let decoded = percent_encoding::percent_decode_str(&encoded).decode_utf8().unwrap();
        prop_assert_eq!(decoded.as_ref(), name.as_str());
        let bytes = encoded.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'%' {
                prop_assert!(bytes[at + 1..at + 3].iter().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)));
                at += 3;
            } else {
                prop_assert!(UNRESERVED.contains(&bytes[at]), "{:?}", encoded);
                at += 1;
            }
        }
    }

    #[test]
    fn msys_drive_path_is_identity_off_windows_and_idempotent_on_it(path in "(/[a-zA-Z]{0,2}(/[a-zA-Z0-9 ._-]{0,6}){0,3}|[A-Za-z]:/x|\\PC{0,12})") {
        prop_assert_eq!(hangar_send::msys_drive_path(&path, false), path.clone());
        let once = hangar_send::msys_drive_path(&path, true);
        prop_assert_eq!(hangar_send::msys_drive_path(&once, true), once.clone());
        if once != path {
            let drive = once.as_bytes()[0];
            prop_assert!(drive.is_ascii_uppercase() && once.as_bytes()[1] == b':' && once.as_bytes()[2] == b'/');
        }
    }

    #[test]
    fn read_text_universal_never_leaves_carriage_returns(text in "[a-zç\r\n\u{feff} ]{0,40}") {
        let read = hangar_send::read_text_universal(text.as_bytes()).unwrap();
        prop_assert!(!read.contains('\r'));
        prop_assert_eq!(read.starts_with('\u{feff}'), text.starts_with('\u{feff}'));
        prop_assert_eq!(read.clone(), text.replace("\r\n", "\n").replace('\r', "\n"));
    }

    #[test]
    fn read_text_universal_refuses_what_is_not_utf8(bytes in proptest::collection::vec(any::<u8>(), 0..32)) {
        prop_assert_eq!(hangar_send::read_text_universal(&bytes).is_some(), std::str::from_utf8(&bytes).is_ok());
    }

    #[test]
    fn split_root_only_takes_a_leading_root(dir in "[a-z/]{1,12}", rest in proptest::collection::vec("[a-z-]{0,6}", 0..4)) {
        let mut args = vec!["--root".to_owned(), dir.clone()];
        args.extend(rest.iter().cloned());
        let (root, left) = hangar_send::split_root(&args);
        prop_assert_eq!(root, Some(std::path::PathBuf::from(&dir)));
        prop_assert_eq!(left, rest.clone());
        let (root, left) = hangar_send::split_root(&rest);
        if rest.first().map(String::as_str) != Some("--root") || rest.len() < 2 {
            prop_assert!(root.is_none());
            prop_assert_eq!(left, rest.clone());
        }
    }
}

/// JSON qualquer, com inteiro maior que 2^64, texto fora do ASCII e aninhamento.
fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        Just(serde_json::from_str::<Value>("123456789012345678901234567890").unwrap()),
        any::<f64>().prop_filter("finito", |f| f.is_finite()).prop_map(Value::from),
        "\\PC{0,8}".prop_map(Value::String),
        Just(Value::String("ação ✓ 🚀".into())),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| prop_oneof![
        proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
        proptest::collection::btree_map(prop_oneof![Just("name".to_owned()), Just("state".to_owned()), Just("cwd".to_owned()),
                                                    Just("peers".to_owned()), Just("owner".to_owned()), Just("session".to_owned()),
                                                    Just("alias".to_owned()), Just("warning".to_owned()), Just("avisos".to_owned()),
                                                    "[a-z_]{1,8}"], inner, 0..5)
            .prop_map(|map| Value::Object(map.into_iter().collect())),
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Qualquer corpo do servidor sai em 0, 1 ou 2, nunca em pânico.
    #[test]
    fn any_server_body_ends_with_a_known_code(body in any_json(), verb in 0usize..5) {
        let server = FakeServer::start(vec![Route::new("*", "", 200, &body.to_string())]);
        let home = Home::new(server.port, "");
        let args: &[&str] = match verb {
            0 => &["--list"],
            1 => &["alvo", "oi"],
            2 => &["--aceitar-par", "https://x--guest.y.hangar.dev.br/par/z"],
            3 => &["--group", "x"],
            _ => &["--new", "nova", "--provider", "claude"],
        };
        let done = home.run(args, &[], &plain_host());
        prop_assert!(done.code <= 2, "código {} para {:?}: {}", done.code, args, done.stderr);
    }
}
