//! `dictation.timing`: quanto cada etapa de um pedido de ditado levou, para achar onde vai o tempo
//! entre parar de falar e o texto aparecer. Sem texto, chave nem caminho.
use std::{cell::RefCell, future::Future, time::Instant};

use axum::response::Response;
use serde_json::json;

use crate::routes::AppState;

#[derive(Default)]
struct Timing { config_ms: u64, stt_ms: u64, organize_ms: u64, provider: String, bytes: Option<usize>, code: String }

tokio::task_local! { static TIMING: RefCell<Timing>; }

pub(crate) enum Step { Config, Stt, Organize }

/// Soma o tempo de `work` na etapa do pedido medido; fora de `measure` não faz nada.
pub(crate) async fn timed<F: Future>(step: Step, work: F) -> F::Output {
    let started = Instant::now();
    let output = work.await;
    let ms = started.elapsed().as_millis() as u64;
    note(|t| *match step { Step::Config => &mut t.config_ms, Step::Stt => &mut t.stt_ms, Step::Organize => &mut t.organize_ms } += ms);
    output
}

fn note(change: impl FnOnce(&mut Timing)) {
    let _ = TIMING.try_with(|t| change(&mut t.borrow_mut()));
}

/// O que só quem transcreve ou organiza sabe: provedor (nome de exibição), tamanho e código da falha.
pub(crate) fn transcribed(provider: Option<&str>, bytes: usize, code: Option<&str>) {
    note(|t| {
        t.provider = provider.unwrap_or_default().to_owned();
        t.bytes = Some(bytes);
        if let Some(code) = code { t.code = code.to_owned(); }
    });
}

pub(crate) fn organized(code: Option<&str>) {
    if let Some(code) = code { note(|t| t.code = code.to_owned()); }
}

/// Roda o pedido e grava uma linha no diário do servidor (`~/.hangar/logs/diario/servidor-rust`).
pub(crate) async fn measure(st: &AppState, route: &'static str, work: impl Future<Output = Response>) -> Response {
    let started = Instant::now();
    let (response, timing) = TIMING.scope(RefCell::default(), async {
        let response = work.await;
        (response, TIMING.with(RefCell::take))
    }).await;
    let status = response.status().as_u16();
    let level = if status >= 400 { "erro" } else if timing.code.is_empty() { "ok" } else { "aviso" };
    let line = json!({"evento": "dictation.timing", "nivel": level, "rota": route, "status": status, "codigo": timing.code,
        "provider": timing.provider, "bytes": timing.bytes, "config_ms": timing.config_ms, "stt_ms": timing.stt_ms,
        "organize_ms": timing.organize_ms, "total_ms": started.elapsed().as_millis() as u64});
    crate::diag::record(crate::diag::diary_dir_in(&st.accounts.env), vec![line], "ditado");
    response
}
