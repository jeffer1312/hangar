//! `dictation.timing`: quanto cada etapa de um pedido de ditado levou, para achar onde vai o tempo
//! entre parar de falar e o texto aparecer. Sem texto, chave nem caminho.
use std::{cell::RefCell, future::Future};

use tokio::time::Instant;

use axum::response::Response;
use serde_json::json;

use crate::routes::AppState;

#[derive(Default)]
struct Timing { config_ms: u64, stt_ms: u64, organize_ms: u64, provider: String, bytes: Option<usize>, code: String }

tokio::task_local! { static TIMING: RefCell<Timing>; }

pub(crate) enum Step { Config, Stt, Organize }

/// Soma o tempo de `work` na etapa do pedido medido; fora de `measure` não faz nada. A soma sai no
/// `Drop`: a etapa cortada por prazo (futuro descartado) é a lenta que se quer ver.
pub(crate) async fn timed<F: Future>(step: Step, work: F) -> F::Output {
    let _elapsed = Elapsed { step, started: Instant::now() };
    work.await
}

struct Elapsed { step: Step, started: Instant }

impl Drop for Elapsed {
    fn drop(&mut self) {
        let ms = self.started.elapsed().as_millis() as u64;
        note(|t| *match self.step { Step::Config => &mut t.config_ms, Step::Stt => &mut t.stt_ms, Step::Organize => &mut t.organize_ms } += ms);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// O passo cortado pelo prazo é justamente o lento: o tempo dele não pode sumir com o futuro descartado.
    #[tokio::test(start_paused = true)]
    async fn dictation_timing_keeps_a_step_cut_by_a_deadline() {
        let timing = TIMING.scope(RefCell::default(), async {
            let cut = tokio::time::timeout(std::time::Duration::from_millis(500), timed(Step::Stt, std::future::pending::<()>())).await;
            assert!(cut.is_err());
            TIMING.with(RefCell::take)
        }).await;
        assert_eq!(timing.stt_ms, 500);
    }
}
