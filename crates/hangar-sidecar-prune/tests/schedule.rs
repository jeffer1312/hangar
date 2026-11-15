//! R6-01: uma rodada na hora, depois uma por dia; rodada que falhou tenta de novo em `RETRY`, não em 24 h.
use std::time::Duration;

use hangar_sidecar_prune::{INTERVAL, RETRY, every};
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn r6_01_first_round_at_once_a_failed_one_retries_soon_then_one_a_day() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut calls = 0;
    let task = tokio::spawn(every(move || {
        calls += 1;
        tx.send(Instant::now()).unwrap();
        // A primeira falha (descoberta ainda subindo); as seguintes dão certo.
        let ok = calls > 1;
        async move { ok }
    }));
    let start = Instant::now();
    let mut at = Vec::new();
    for _ in 0..4 {
        at.push(rx.recv().await.unwrap() - start);
    }
    task.abort();
    assert_eq!(at, [Duration::ZERO, RETRY, RETRY + INTERVAL, RETRY + INTERVAL * 2]);
    assert_eq!((RETRY, INTERVAL), (Duration::from_secs(300), Duration::from_secs(86_400)));
}
