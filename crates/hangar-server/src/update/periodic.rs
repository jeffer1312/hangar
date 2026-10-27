//! Verificação periódica do modo pacote (o `_auto_update_loop` do Python, sem `git fetch`): a cada
//! hora lê o índice e, com as automações ligadas e versão mais nova publicada, lança o executor.
//! Cada portão que recusa vira um log; a próxima hora tenta de novo.
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use super::executor::{Args, StartError, Task};
use super::{Layout, Release};
use crate::accounts::environment::AccountEnvironment;

/// O `_AUTO_UPDATE_INTERVALO` do Python.
pub const INTERVAL: Duration = Duration::from_secs(3600);
/// Atualização que falhou segura novas tentativas por 24 h.
const FAILURE_WINDOW: chrono::TimeDelta = chrono::TimeDelta::hours(24);

/// O que um tique pergunta à máquina; o teste troca.
pub trait Machine {
    /// O canal que o Atualizar segue (`super::channel`); `None` fora do modo pacote.
    fn package_channel(&self) -> Option<String>;
    fn automations(&self) -> bool;
    fn state(&self) -> Map<String, Value>;
    /// A versão publicada quando é mais nova que a instalada.
    fn newer(&self, channel: &str) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// Alguma sessão trabalhando agora.
    fn working(&self) -> impl Future<Output = Result<bool, String>> + Send;
    fn launch(&self) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Tick {
    Launched { version: String },
    Skipped(String),
}

/// Um tique. `now` vem de fora (relógio do teste).
pub async fn tick(machine: &impl Machine, now: chrono::DateTime<chrono::FixedOffset>) -> Tick {
    let skip = |why: &str| Tick::Skipped(why.to_owned());
    let Some(channel) = machine.package_channel() else { return skip("fora do modo pacote") };
    // Como no motor `git`: o automático segue só a main; canal de teste atualiza pelo botão.
    if channel != "latest" { return skip("canal de teste"); }
    if !machine.automations() { return skip("automações desligadas"); }
    let state = machine.state();
    if state.get("fase").and_then(Value::as_str) == Some("rodando") { return skip("atualizacao ja rodando"); }
    if state.get("ok") == Some(&Value::Bool(false)) {
        let at = state.get("ts").and_then(Value::as_str).and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok());
        // Hora ilegível abre, como no Python, com aviso.
        if at.is_none() { tracing::warn!("update: hora ilegível no estado da atualização; a janela de repetição não segura"); }
        if at.is_some_and(|at| now - at < FAILURE_WINDOW) { return skip("ultima atualizacao falhou"); }
    }
    let version = match machine.newer(&channel).await {
        Ok(Some(version)) => version,
        Ok(None) => return skip("em dia"),
        Err(e) => return Tick::Skipped(format!("índice ilegível: {e}")),
    };
    match machine.working().await {
        Ok(false) => {}
        Ok(true) => return skip("sessao trabalhando"),
        Err(e) => return Tick::Skipped(format!("lista indisponível: {e}")),
    }
    match machine.launch().await {
        Ok(()) => Tick::Launched { version },
        Err(e) => Tick::Skipped(format!("executor não lançado: {e}")),
    }
}

/// A máquina de verdade.
pub struct Live {
    pub env: AccountEnvironment,
    pub list: Arc<crate::list::bridge::ListBridge>,
    pub listen: SocketAddr,
    pub client: reqwest::Client,
}

impl Live {
    fn layout(&self) -> Layout { Layout::new(&self.env.home) }
    /// A pasta do Claude que a lista lê (a mesma de `loops::automations`).
    fn claude_dir(&self) -> std::path::PathBuf {
        self.list.env().dirs.as_ref().map_or_else(|| self.env.claude_base.clone(), |d| d.claude.clone())
    }
}

impl Machine for Live {
    fn package_channel(&self) -> Option<String> {
        matches!(super::http::mode(&self.env.home), super::http::Mode::Package(_)).then(|| super::channel(&self.layout()))
    }

    fn automations(&self) -> bool { crate::push::watch::Settings::read(&self.claude_dir(), &crate::push::server_env()).automations }

    fn state(&self) -> Map<String, Value> {
        super::for_screen(&super::state_dir(&self.env.claude_base), &self.layout(), super::running_package().map(|p| p.version.as_str()))
    }

    async fn newer(&self, channel: &str) -> Result<Option<String>, String> {
        let installed = super::installed(&self.layout()).and_then(|m| m.version.parse().ok());
        let release = Release::github(channel);
        super::check(&self.client, &release, &super::platform(), installed).await.map(|e| e.map(|e| e.version)).map_err(|e| e.to_string())
    }

    async fn working(&self) -> Result<bool, String> {
        let produced = self.list.snapshot().await.map_err(|e| e.code.to_owned())?;
        Ok(produced.rows.iter().any(|row| row.state == "working"))
    }

    async fn launch(&self) -> Result<(), String> {
        let (home, claude) = (self.env.home.clone(), self.env.claude_base.clone());
        let args = Args { task: Task::Update { channel: None }, listen: self.listen };
        let require_scope = super::http::topology().await == super::restart::Topology::Systemd;
        tokio::task::spawn_blocking(move || {
            super::executor::launch(&home, &claude, &args, require_scope).map(|_| ()).map_err(|e| match e {
                StartError::Busy => "ja rodando".to_owned(),
                StartError::Launch(why) => why,
            })
        }).await.map_err(|e| e.to_string())?
    }
}

/// O laço: o primeiro tique depois de um intervalo (nunca na subida). Só o `main` liga.
pub fn spawn(machine: Live) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(INTERVAL).await;
            match tick(&machine, chrono::Local::now().fixed_offset()).await {
                Tick::Launched { version } => tracing::info!(version, "update: atualização automática lançada"),
                Tick::Skipped(why) if why == "fora do modo pacote" || why == "em dia" => tracing::debug!("update: automático: {why}"),
                Tick::Skipped(why) => tracing::info!("update: automático pulado: {why}"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    struct Fake {
        channel: Option<&'static str>,
        automations: bool,
        state: Value,
        newer: Option<&'static str>,
        working: bool,
        launched: AtomicUsize,
    }

    impl Default for Fake {
        fn default() -> Fake {
            Fake { channel: Some("latest"), automations: true, state: json!({"fase": "pronto", "ok": true}), newer: Some("0.1.0.9"),
                working: false, launched: AtomicUsize::new(0) }
        }
    }

    impl Machine for Fake {
        fn package_channel(&self) -> Option<String> { self.channel.map(str::to_owned) }
        fn automations(&self) -> bool { self.automations }
        fn state(&self) -> Map<String, Value> { self.state.as_object().cloned().unwrap() }
        async fn newer(&self, _channel: &str) -> Result<Option<String>, String> { Ok(self.newer.map(str::to_owned)) }
        async fn working(&self) -> Result<bool, String> { Ok(self.working) }
        async fn launch(&self) -> Result<(), String> { self.launched.fetch_add(1, Ordering::SeqCst); Ok(()) }
    }

    fn now() -> chrono::DateTime<chrono::FixedOffset> { chrono::DateTime::parse_from_rfc3339("2026-10-24T12:00:00-03:00").unwrap() }

    async fn launches(fake: Fake) -> usize {
        tick(&fake, now()).await;
        fake.launched.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn update_periodic_launches_only_with_automations_on_and_a_newer_version() {
        let fake = Fake::default();
        assert_eq!(tick(&fake, now()).await, Tick::Launched { version: "0.1.0.9".into() });
        assert_eq!(fake.launched.load(Ordering::SeqCst), 1);
        assert_eq!(launches(Fake { automations: false, ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { newer: None, ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { channel: None, ..Fake::default() }).await, 0, "modo desenvolvedor e máquina não migrada");
        assert_eq!(launches(Fake { channel: Some("beta"), ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { working: true, ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { state: json!({"fase": "rodando"}), ..Fake::default() }).await, 0);
    }

    #[tokio::test]
    async fn update_periodic_waits_a_day_after_a_failure() {
        let failed = |ts: &str| Fake { state: json!({"fase": "pronto", "ok": false, "ts": ts}), ..Fake::default() };
        assert_eq!(launches(failed("2026-10-24T08:00:00-03:00")).await, 0, "falhou há 4 h");
        assert_eq!(launches(failed("2026-10-23T11:00:00-03:00")).await, 1, "falhou há 25 h");
        assert_eq!(launches(failed("lixo")).await, 1, "hora ilegível abre, como no Python");
    }
}
