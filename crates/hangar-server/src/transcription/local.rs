use super::{audio, cloud::{self, AttemptFailure}, model::ProviderConfig, process::{self, ManagedChild}};
use crate::list::procs::{ProcessView, SystemProcs};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, sync::{Mutex as SyncMutex, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::sync::{Mutex, watch};

#[derive(Clone, PartialEq, Eq)]
struct Key { program: String, model: String, language: String }

impl Key {
    fn of(provider: &ProviderConfig) -> Self {
        Self { program: provider.executable_path.clone(), model: provider.model_path.clone(),
            language: if provider.language.trim().is_empty() { "pt".into() } else { provider.language.clone() } }
    }
}

#[derive(Serialize, Deserialize)]
struct Record { pid: u32, birth: f64, owner: String, parent: u32, parent_birth: f64 }

struct Running { process: ManagedChild, key: Key, address: String, record: Option<PathBuf> }

impl Running {
    async fn stop(&mut self) {
        self.process.stop().await;
        if let Some(path) = &self.record { let _ = std::fs::remove_file(path); }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(path) = &self.record { let _ = std::fs::remove_file(path); }
    }
}

#[derive(Default)]
struct View { id: String, key: Option<Key>, state: &'static str, error: Option<String> }

pub(crate) struct LocalWhisper {
    running: Mutex<Option<Running>>,
    view: SyncMutex<View>,
    client: reqwest::Client,
    stopping: watch::Sender<bool>,
    shutdown_started: AtomicBool,
    lifecycle: Mutex<()>,
}

impl Default for LocalWhisper {
    fn default() -> Self {
        Self { running: Mutex::new(None), view: SyncMutex::new(View::default()),
            stopping: watch::channel(false).0, shutdown_started: AtomicBool::new(false), lifecycle: Mutex::new(()),
            client: reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
                .build().expect("Cliente HTTP local com parâmetros válidos") }
    }
}

impl LocalWhisper {
    fn publish(&self, provider: &ProviderConfig, state: &'static str, error: Option<String>) {
        *self.view.lock().unwrap_or_else(|e| e.into_inner()) = View {
            id: provider.id.clone(), key: Some(Key::of(provider)), state, error,
        };
    }

    pub fn status(&self, id: &str) -> (&'static str, Option<String>) {
        let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.id == id { (view.state, view.error.clone()) } else { ("not_started", None) }
    }

    pub async fn reconcile(&self, providers: &[ProviderConfig]) {
        let keep = {
            let view = self.view.lock().unwrap_or_else(|e| e.into_inner());
            view.key.is_none() || providers.iter().any(|p| p.kind == "whisper_cpp" && view.key.as_ref() == Some(&Key::of(p)))
        };
        if !keep { self.stop_current(false).await; }
    }

    pub async fn shutdown(&self) {
        self.stop_current(true).await;
    }

    async fn stop_current(&self, final_stop: bool) {
        if final_stop { self.shutdown_started.store(true, Ordering::SeqCst); }
        let _lifecycle = self.lifecycle.lock().await;
        // Cancela também a conversão e quem espera a vez, antes de pedir o lock da inferência.
        self.stopping.send_replace(true);
        if let Some(mut running) = self.running.lock().await.take() { running.stop().await; }
        *self.view.lock().unwrap_or_else(|e| e.into_inner()) = View::default();
        if !self.shutdown_started.load(Ordering::SeqCst) { self.stopping.send_replace(false); }
    }

    pub async fn recover_registered(&self, state_path: &str) {
        if state_path.is_empty() { return; }
        let Ok(current) = self.running.try_lock() else { return; };
        if current.is_some() { return; }
        if let Some(directory) = Path::new(state_path).parent()
            && let Err(error) = recover(&directory.join("transcription-local.json")).await {
            tracing::warn!(code = error.error.code, "O registro do Whisper não pôde ser recuperado; a posse será conferida antes de iniciar.");
        }
    }

    pub async fn transcribe(&self, provider: &ProviderConfig, content: Bytes, name: Option<&str>,
        vocabulary: &str, state_path: &str, timeout: Duration) -> Result<String, AttemptFailure> {
        let mut stopping = self.stopping.subscribe();
        let cancelled = || AttemptFailure::unavailable("whisper_stopping", "A transcrição local foi encerrada pelo servidor.");
        if *stopping.borrow() || self.shutdown_started.load(Ordering::SeqCst) { return Err(cancelled()); }
        let deadline = tokio::time::Instant::now() + timeout;
        let result = tokio::time::timeout_at(deadline, async {
            tokio::select! {
                biased;
                _ = stopping.changed() => Err(cancelled()),
                result = self.run(provider, content, name, vocabulary, state_path, deadline) => result,
            }
        }).await;
        let result = result.unwrap_or_else(|_| {
            let mut error = AttemptFailure::unavailable("whisper_timeout", "O Whisper não concluiu a transcrição a tempo.");
            error.error.status = 504;
            Err(error)
        });
        if let Err(error) = &result
            && error.error.code != "whisper_stopping" && !self.shutdown_started.load(Ordering::SeqCst) {
            self.publish(provider, "failed", Some(error.error.detail.clone()));
        }
        result
    }

    async fn run(&self, provider: &ProviderConfig, content: Bytes, name: Option<&str>, vocabulary: &str,
        state_path: &str, deadline: tokio::time::Instant) -> Result<String, AttemptFailure> {
        let program = process::expand_path(&provider.executable_path);
        let model = process::expand_path(&provider.model_path);
        if !program.is_file() { return Err(AttemptFailure::unavailable("whisper_executable_missing", "O executável whisper-server não foi encontrado no servidor.")); }
        if !model.is_file() { return Err(AttemptFailure::unavailable("whisper_model_missing", "O arquivo do modelo Whisper não foi encontrado no servidor.")); }
        let mut current = self.running.lock().await;
        let content = audio::normalize(provider, content, name).await?;
        let dead = current.as_mut().is_some_and(|r| !matches!(r.process.child.try_wait(), Ok(None)));
        if (dead || current.as_ref().is_some_and(|r| r.key != Key::of(provider)))
            && let Some(mut old) = current.take() { old.stop().await;
        }
        // A porta é reservada e solta antes de o Whisper subir: outro processo pode tomá-la nesse meio, e o
        // Whisper sai antes de escutar. Partida feita aqui que morre assim tenta de novo, numa porta nova.
        let mut starts = 0;
        loop {
            let fresh = current.is_none();
            if fresh {
                self.publish(provider, "starting", None);
                let record = if state_path.is_empty() { None } else { Path::new(state_path).parent().map(|p| p.join("transcription-local.json")) };
                if let Some(path) = &record { recover(path).await?; }
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await
                    .map_err(|_| AttemptFailure::unavailable("whisper_port_failed", "Não foi possível reservar uma porta local para o Whisper."))?;
                let port = listener.local_addr().map_err(|_| AttemptFailure::unavailable("whisper_port_failed", "Porta local indisponível."))?.port();
                let owner = crate::accounts::claude_login::nonce()
                    .map_err(|_| AttemptFailure::unavailable("whisper_identity_failed", "Não foi possível identificar o processo local."))?;
                let key = Key::of(provider);
                let mut command = process::command(&program);
                command.args(["--host", "127.0.0.1", "--port"]).arg(port.to_string())
                    .arg("--model").arg(&model).arg("--language").arg(&key.language).arg("--no-gpu")
                    .env("HANGAR_TRANSCRIPTION_OWNER", &owner);
                drop(listener);
                let process = ManagedChild::spawn(command, "whisper_start_failed", "Não foi possível iniciar o whisper-server. Confira o executável e o modelo.").await?;
                let mut running = Running { process, key, address: format!("http://127.0.0.1:{port}"), record: None };
                if let Some(path) = record {
                    if let Err(error) = record_process(&path, running.process.child.id().unwrap_or_default(), owner).await {
                        running.stop().await;
                        return Err(error);
                    }
                    running.record = Some(path);
                }
                *current = Some(running);
            }
            let running = current.as_mut().unwrap();
            match wait_ready(running, deadline, &self.client).await {
                Ok(()) => break,
                Err(error) if fresh && matches!(error.error.code.as_str(), "whisper_start_failed" | "whisper_port_not_owned") && starts < 2 => {
                    starts += 1;
                    if let Some(mut dead) = current.take() { dead.stop().await; }
                }
                Err(error) => return Err(error),
            }
        }
        let running = current.as_mut().unwrap();
        self.publish(provider, "ready", None);
        let request = ProviderConfig { kind: "openai".into(), model: "local".into(), language: running.key.language.clone(), ..Default::default() };
        cloud::transcribe_to(&self.client, &request, &format!("{}/inference", running.address), content,
            Some("audio.wav"), vocabulary, deadline.saturating_duration_since(tokio::time::Instant::now())).await
    }
}

/// Espera o Whisper recém-lançado responder na porta dele, e confere que a porta é mesmo dele.
async fn wait_ready(running: &mut Running, deadline: tokio::time::Instant, client: &reqwest::Client) -> Result<(), AttemptFailure> {
    loop {
        if !matches!(running.process.child.try_wait(), Ok(None)) {
            return Err(AttemptFailure::unavailable("whisper_start_failed", "O processo Whisper encerrou antes de ficar disponível."));
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { return Err(AttemptFailure::unavailable("whisper_timeout", "O modelo Whisper não carregou a tempo.")); }
        let health = client.get(format!("{}/health", running.address)).timeout(remaining.min(Duration::from_millis(500))).send().await;
        if health.is_ok_and(|r| r.status().is_success()) {
            let port = reqwest::Url::parse(&running.address).ok().and_then(|u| u.port()).unwrap_or(0);
            if !process::owns_port(running.process.child.id().unwrap_or_default(), port).await.unwrap_or(false) {
                return Err(AttemptFailure::unavailable("whisper_port_not_owned", "A porta de transcrição não pertence ao processo Whisper iniciado pelo Hangar."));
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn record_process(path: &Path, pid: u32, owner: String) -> Result<(), AttemptFailure> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let view = SystemProcs::default();
        let parent = std::process::id();
        view.prefetch(&[pid as i64, parent as i64]);
        let record = Record { pid, owner, parent, birth: view.start_time(pid as i64).ok_or(())?,
            parent_birth: view.start_time(parent as i64).ok_or(())? };
        let directory = path.parent().ok_or(())?;
        std::fs::create_dir_all(directory).map_err(|_| ())?;
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| ())?;
        serde_json::to_writer(&mut file, &record).map_err(|_| ())?;
        file.persist(path).map_err(|_| ())?;
        Ok::<_, ()>(())
    }).await.ok().and_then(Result::ok).ok_or_else(|| AttemptFailure::unavailable("whisper_identity_failed", "Não foi possível registrar a propriedade do processo Whisper."))
}

async fn recover(path: &Path) -> Result<(), AttemptFailure> {
    if !path.exists() { return Ok(()); }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let record: Record = serde_json::from_slice(&std::fs::read(&path).map_err(|_| ())?).map_err(|_| ())?;
        let view = SystemProcs::default();
        view.prefetch(&[record.pid as i64, record.parent as i64]);
        if view.start_time(record.pid as i64) == Some(record.birth) {
            let same_owner = view.env_var(record.pid as i64, "HANGAR_TRANSCRIPTION_OWNER").ok().flatten()
                .is_some_and(|v| v.to_string_lossy() == record.owner);
            if !same_owner { return Err(()); }
            if view.start_time(record.parent as i64) == Some(record.parent_birth) { return Err(()); }
            #[cfg(unix)]
            unsafe {
                if record.pid > i32::MAX as u32 || libc::getpgid(record.pid as i32) != record.pid as i32
                    || libc::kill(-(record.pid as i32), libc::SIGKILL) != 0 { return Err(()); }
            }
            #[cfg(not(unix))]
            return Err(());
        }
        std::fs::remove_file(path).map_err(|_| ())
    }).await.ok().and_then(Result::ok).ok_or_else(|| AttemptFailure::unavailable("whisper_recovery_failed",
        "Há um registro de processo Whisper cuja propriedade não pôde ser confirmada. Confira a outra instância do Hangar antes de iniciar."))
}
