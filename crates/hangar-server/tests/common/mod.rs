// crates/hangar-server/tests/common/mod.rs
//! Caminhos e comparação dos testes de contrato com o golden do Python.
#![allow(dead_code)]

use std::path::PathBuf;

use hangar_server::transcript::pyjson;
use serde_json::Value;

pub fn contract() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract")
}

pub fn golden(name: &str) -> Value {
    let path = contract().join("golden").join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    pyjson::loads_lossless(&raw).expect("golden é JSON")
}

/// Forma canônica para comparar: a mesma serialização dos dois lados, chaves ordenadas.
pub fn canon(v: &Value) -> String {
    pyjson::dumps(v, true)
}
