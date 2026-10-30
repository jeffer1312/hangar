//! Barreira da preparação inicial; a saúde permanece disponível enquanto os passos executam.
use axum::{extract::{Request, State}, middleware::Next, response::{IntoResponse, Response}};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Readiness(tokio::sync::watch::Sender<bool>, bool);
impl Default for Readiness { fn default() -> Self { Self(tokio::sync::watch::channel(true).0, false) } }
impl Readiness {
    pub fn pending() -> Self { Self(tokio::sync::watch::channel(false).0, true) }
    pub fn prepares(&self) -> bool { self.1 }
    pub fn is_ready(&self) -> bool { *self.0.borrow() }
    pub fn finish(&self) { self.0.send_replace(true); }
    pub async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|ready| *ready).await;
    }
}
/// O cancelamento da porta pede a parada; o cleanup externo conserva o handle para aguardá-la.
#[derive(Default)]
pub struct Producers(std::sync::Mutex<Option<crate::AbortOnDrop>>);
impl Producers {
    pub(crate) fn register(self: &Arc<Self>, task: tokio::task::JoinHandle<()>) -> ProducerScope {
        let mut slot = self.0.lock().unwrap();
        assert!(slot.is_none(), "produtores da porta já registrados");
        *slot = Some(crate::AbortOnDrop(task));
        ProducerScope(self.clone())
    }
    fn cancel(&self) {
        if let Some(task) = self.0.lock().unwrap().as_ref() { task.0.abort(); }
    }
    pub(crate) async fn shutdown(&self) {
        let task = self.0.lock().unwrap().take();
        if let Some(mut task) = task {
            task.0.abort();
            let _ = (&mut task.0).await;
        }
    }
}
pub(crate) struct ProducerScope(Arc<Producers>);
impl Drop for ProducerScope { fn drop(&mut self) { self.0.cancel(); } }

pub fn unavailable() -> Response {
    (axum::http::StatusCode::SERVICE_UNAVAILABLE, [(axum::http::header::CONTENT_TYPE, "application/json")], "{\"detail\":\"server_starting\"}").into_response()
}
pub async fn guard(State(state): State<Arc<crate::routes::AppState>>, request: Request, next: Next) -> Response {
    if !state.startup.is_ready() && !(request.uri().path() == "/__hangar_server/health" && matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD)) { return unavailable(); }
    next.run(request).await
}

pub(crate) async fn maintenance(state: Arc<crate::routes::AppState>) {
    let pairing = state.settings_snapshot().and_then(|snapshot| {
        crate::machine_peers::pairing::Settings::from_snapshot(&snapshot)
    });
    match pairing {
        Ok(settings) => {
            let printed = tokio::task::spawn_blocking(move || {
                use std::io::IsTerminal;
                crate::machine_peers::pairing::print(
                    &settings,
                    &crate::machine_peers::network::lan_ip(),
                    // stdout pertence ao anúncio privado lido pelo Supervisor.
                    std::io::stderr().is_terminal(),
                    &mut std::io::stderr().lock(),
                )
            }).await;
            if !matches!(printed, Ok(Ok(()))) {
                tracing::warn!(code = "pairing_print_failed", "não foi possível imprimir o pareamento");
            }
        }
        Err(error) => tracing::warn!(code = "pairing_config_failed", %error, "pareamento sem configuração válida"),
    }
    state.costs.schedule_warmup(std::time::Duration::from_secs(30));
    crate::pages::routes::spawn_sweep(state.list.clone(), state.pages.clone());
    // Só aqui, nunca no `serve_until`: os testes sobem servidores contra o tmux de verdade.
    #[cfg(unix)]
    tokio::spawn({
        let term = state.term.clone();
        async move { term.restore_after_crash().await }
    });
    // Reservas pré-aquecidas que um servidor que caiu deixou (todo sistema: a varredura abaixo é só Linux
    // e não apaga transcript nem marcadores). Só aqui, pelo mesmo motivo do tmux acima.
    // Atualização: o commit do checkout na subida (modo desenvolvedor) e a verificação periódica do
    // pacote. Só aqui, pelo mesmo motivo do tmux acima.
    {
        let home = state.accounts.env.home.clone();
        let _ = tokio::task::spawn_blocking(move || crate::update::dev::note_boot(&home)).await;
        match (crate::http_client().build(),crate::config_sync::runtime::RuntimeConfig::from_state(&state)) {
            (Ok(client),Ok(runtime)) => { crate::update::periodic::spawn(crate::update::periodic::Live {
                env: state.accounts.env.clone(), list: state.list.clone(), listen: state.cfg.listen, client, runtime }); }
            (Err(e),_) => tracing::warn!("update: verificação periódica sem cliente http: {e}"),
            (_,Err(e)) => tracing::warn!("update: verificação periódica sem configuração: {e}"),
        }
    }
    let prewarm = state.prewarm.clone();
    let _prewarm_sweeper = prewarm.spawn_sweeper();

    crate::update::git_engine::periodic::spawn(&state);
    std::future::pending::<()>().await;
}

pub async fn prepare_sessions(env: crate::accounts::environment::AccountEnvironment) {
    // Órfãos do cano têm dono só: com o runtime ligado, o Rust varre uma vez, antes de anunciar o
    // gateway (o Python só abre sessões depois do anúncio). Só aqui, pelo mesmo motivo do tmux acima.
    // `CP_RUST_NO_ORPHAN_SWEEP=1`: o teste que sobe este binário de verdade nunca varre processos da máquina.
    if matches!(crate::config::Config::runtime_instance(), Ok(Some(_)))
        && std::env::var_os("CP_RUST_NO_ORPHAN_SWEEP").is_none_or(|value| value != "1") {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        if home.is_none() { tracing::warn!("HOME ausente: varredura de canos órfãos pulada"); }
        if let Some(home) = home {
            let swept = tokio::task::spawn_blocking(move || {
                let base = home.join(".hangar");
                crate::runtime::process::sweep_orphans(&base.join("claude-headless"), &base.join("codex-sessions"),
                    &home.to_string_lossy())
            }).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "varredura de canos órfãos falhou"); None });
            if let Some(count) = swept.filter(|count| *count > 0) { tracing::info!(count, "canos de sessão já encerrada finalizados"); }
        }
    }

    if matches!(crate::config::Config::runtime_instance(), Ok(Some(_))) {
    let _ = tokio::task::spawn_blocking(move || crate::runtime::claude_reopen::settle_switches(&env)).await;
    }

}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::{OnceLock, atomic::{AtomicBool, Ordering}};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn startup_cleanup_waits_for_publication_after_the_http_future_is_cancelled() {
        let producers = Arc::new(Producers::default());
        let published = Arc::new(OnceLock::<Arc<AtomicBool>>::new());
        let (entered, entering) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let destination = published.clone();
        let scope = producers.register(tokio::spawn(async move {
            entered.send(()).unwrap();
            // Representa o trecho síncrono de criação/publicação: abort não o interrompe.
            blocked.recv().unwrap();
            destination.set(Arc::new(AtomicBool::new(false))).unwrap();
            std::future::pending::<()>().await;
        }));
        entering.await.unwrap();
        drop(scope); // O select de lib descarta a futura HTTP antes de entrar no cleanup.
        assert!(published.get().is_none());
        let destination = published.clone();
        let mut cleanup = tokio::spawn(async move {
            producers.shutdown().await;
            // A mesma ordem de omp_sync/codex_setup: só ler a célula depois do join.
            destination.get().expect("serviço publicado antes do cleanup").store(true, Ordering::Release);
        });
        let waiting = tokio::time::timeout(std::time::Duration::from_millis(30), &mut cleanup).await.is_err();
        release.send(()).unwrap();
        assert!(waiting, "cleanup não pode ler serviços enquanto a publicação está executando");
        tokio::time::timeout(std::time::Duration::from_secs(2), cleanup).await.unwrap().unwrap();
        assert!(published.get().unwrap().load(Ordering::Acquire), "serviço tardio também foi fechado");
    }
}
