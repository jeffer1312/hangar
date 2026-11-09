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
/// Funções simples, não `async fn`: cada camada `async` guardaria outra cópia do futuro do pedido, que em debug já
/// estoura a pilha da thread de teste.
pub(crate) fn measure(st: &AppState, route: &'static str, work: impl Future<Output = Response>) -> impl Future<Output = Response> {
    let dir = crate::diag::diary_dir_in(&st.accounts.env);
    measure_into(route, work, move |line| {
        // O descarte pode vir no desligamento do runtime, onde `spawn_blocking` entraria em pânico.
        if tokio::runtime::Handle::try_current().is_ok() { crate::diag::record(dir, vec![line], "ditado"); }
    })
}

fn measure_into(route: &'static str, work: impl Future<Output = Response>, record: impl FnOnce(serde_json::Value)) -> impl Future<Output = Response> {
    TIMING.scope(RefCell::default(), async move {
        // Declarado antes de `work`: descartado o pedido, o passo em curso soma o tempo antes de a linha sair.
        let mut line = Line { route, started: Instant::now(), record: Some(record) };
        let response = work.await;
        line.write(Some(response.status().as_u16()));
        response
    })
}

/// A linha do pedido. Sem status (conexão que caiu e descartou o pedido), sai no `Drop` como `cancelled`.
struct Line<R: FnOnce(serde_json::Value)> { route: &'static str, started: Instant, record: Option<R> }

impl<R: FnOnce(serde_json::Value)> Line<R> {
    fn write(&mut self, status: Option<u16>) {
        let Some(record) = self.record.take() else { return };
        let mut timing = TIMING.try_with(RefCell::take).unwrap_or_default();
        let level = match status {
            None => { timing.code = "cancelled".into(); "aviso" }
            Some(status) if status >= 400 => "erro",
            Some(_) if timing.code.is_empty() => "ok",
            Some(_) => "aviso",
        };
        record(json!({"evento": "dictation.timing", "nivel": level, "rota": self.route, "status": status, "codigo": timing.code,
            "provider": timing.provider, "bytes": timing.bytes, "config_ms": timing.config_ms, "stt_ms": timing.stt_ms,
            "organize_ms": timing.organize_ms, "total_ms": self.started.elapsed().as_millis() as u64}));
    }
}

impl<R: FnOnce(serde_json::Value)> Drop for Line<R> {
    fn drop(&mut self) { self.write(None); }
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

    /// Conexão que cai no meio descarta o pedido: a linha sai mesmo assim, com o tempo das etapas até ali.
    #[tokio::test(start_paused = true)]
    async fn dictation_timing_records_a_request_dropped_mid_step() {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = lines.clone();
        let request = measure_into("ditado", async {
            timed(Step::Config, tokio::time::sleep(std::time::Duration::from_millis(40))).await;
            timed(Step::Stt, std::future::pending::<Response>()).await
        }, move |line| sink.lock().unwrap().push(line));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), request).await.is_err());
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = &lines[0];
        assert_eq!((line["nivel"].as_str(), line["codigo"].as_str(), line["status"].as_u64()), (Some("aviso"), Some("cancelled"), None), "{line}");
        assert_eq!((line["config_ms"].as_u64(), line["stt_ms"].as_u64(), line["total_ms"].as_u64()), (Some(40), Some(260), Some(300)), "{line}");
    }
}
