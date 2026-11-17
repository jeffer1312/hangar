//! Argumentos quaisquer do CLI: o pedido sai inteiro (o texto do `json.dumps` volta ao mesmo objeto)
//! ou o comando manda o CLI cru, nunca outra coisa.
use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::Value;

fn arg() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("--model".to_owned()),
        Just("--effort=x".to_owned()),
        Just("--thinking".to_owned()),
        Just("--profile".to_owned()),
        Just("--dangerously-skip-permissions".to_owned()),
        "\\PC{0,12}",
        "[\\x01-\\x1f\"\\\\a-zç🚀]{0,8}",
    ]
}

proptest! {
    #[test]
    fn request_is_whole_or_raw(provider in prop_oneof![Just("claude"), Just("pi"), Just("omp"), Just("kimi")],
                               args in proptest::collection::vec(arg(), 0..6),
                               pwd in "\\PC{0,10}") {
        let vars = BTreeMap::from([("PWD".to_owned(), pwd)]);
        let mut err = Vec::new();
        match hangar_open_cli::request(provider, &args, &vars, std::path::Path::new("/"), &mut err) {
            Ok(body) => {
                let text = hangar_core::pycompat::pyjson::dumps(&Value::Object(body.clone()), false);
                prop_assert!(text.is_ascii());
                prop_assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), Value::Object(body));
                prop_assert!(err.is_empty());
            }
            Err(code) => {
                prop_assert_eq!(code, hangar_open_cli::RAW);
                let said = String::from_utf8(err).unwrap();
                prop_assert!(said.is_empty() || said.contains("não cabe no pedido do Hangar"), "{}", said);
            }
        }
    }
}
