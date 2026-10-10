//! Lista acordada por arquivo, no próprio processo: o diário tem limite por minuto e por código
//! no processo inteiro, e a falha provocada aqui roubaria o aviso que `list_routes` espera.
#![cfg(unix)]
use crate::fake;
use crate::list_support;

use fake::next_named;
use list_support::*;
use std::time::Duration;

/// Lista em erro: marcador escrito não republica a lista de antes da falha por cima do erro.
#[tokio::test(flavor = "multi_thread")]
async fn marker_during_error_keeps_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let mut es = open(srv.addr).await;
    next_named(&mut es, "sessions").await;
    std::fs::write(dir.path().join("fail"), "").unwrap();
    next_named(&mut es, "list_error").await;
    write_marker(dir.path(), 0, "working");
    let more = tokio::time::timeout(Duration::from_millis(1200), next_named(&mut es, "sessions")).await;
    assert!(more.is_err(), "lista velha publicada sobre o erro");
}
