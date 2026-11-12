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
/// Adiado, confere de novo cedo para pegar a janela ociosa.
pub const RECHECK: Duration = Duration::from_secs(300);
/// Atualização que falhou segura novas tentativas por 24 h.
const FAILURE_WINDOW: chrono::TimeDelta = chrono::TimeDelta::hours(24);
/// Sessão ou fila que nunca param não seguram a atualização para sempre; a entrega incerta segura.
pub(crate) const DEFER_CEILING: chrono::TimeDelta = chrono::TimeDelta::hours(24);

/// O que um tique pergunta à máquina; o teste troca.
pub trait Machine {
    /// O canal que o Atualizar segue (`super::channel`); `None` fora do modo pacote.
    fn package_channel(&self) -> Option<String>;
    fn automations(&self) -> bool;
    fn state(&self) -> super::StateSnapshot;
    /// A versão publicada quando é mais nova que a instalada.
    fn newer(&self, channel: &str) -> impl Future<Output = Result<Option<String>, String>> + Send;
    /// O que segura a atualização agora (`busy`).
    fn busy(&self) -> impl Future<Output = Result<Option<Busy>, String>> + Send;
    fn launch(&self) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Tick {
    Launched { version: String },
    Skipped(String),
    Deferred(String),
}

/// O que segura a atualização, do mais forte ao mais fraco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy { Uncertain, Working, Queue }

impl Busy {
    pub(crate) fn why(self) -> &'static str {
        match self { Busy::Uncertain => "entrega incerta", Busy::Working => "sessao trabalhando", Busy::Queue => "fila com mensagem esperando" }
    }
}

/// O que segura a atualização nesta máquina; fila ilegível segura como fila esperando.
pub async fn busy_now(list: &crate::list::bridge::ListBridge, state: &crate::state::live::StateEnv) -> Result<Option<Busy>, String> {
    let produced = list.snapshot().await.map_err(|e| e.code.to_owned())?;
    let working = produced.rows.iter().any(|row| row.state == "working");
    let Some(registry) = state.runtime.get() else { return Ok(working.then_some(Busy::Working)) };
    let files = registry.queue_files().await;
    let queues = tokio::task::spawn_blocking(move || {
        files.into_values().filter_map(|(_, path, created)| queue_hold(&path, created)).collect::<Vec<_>>()
    }).await.map_err(|e| e.to_string())?;
    if queues.contains(&Busy::Uncertain) { return Ok(Some(Busy::Uncertain)); }
    Ok(if working { Some(Busy::Working) } else { queues.first().copied() })
}

/// Ilegível (antivírus, escrita em curso, arquivo estragado) não prova que nada espera: segura como fila,
/// que o teto de 24 h ainda solta, em vez de reiniciar o servidor no escuro.
fn queue_hold(path: &std::path::Path, created: f64) -> Option<Busy> {
    let unreadable = |why: String| {
        if crate::warn_limit::allow(path.to_str(), "update_queue_unreadable") {
            tracing::warn!(path = %path.display(), code = "update_queue_unreadable", "update: fila ilegível segura a atualização: {why}");
        }
        Some(Busy::Queue)
    };
    let bytes = match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return unreadable(error.to_string()),
    };
    let state = match crate::runtime::queue::State::load(&bytes) { Ok((state, _)) => state, Err(error) => return unreadable(error.to_string()) };
    if state.delivery_uncertain() { return Some(Busy::Uncertain); }
    (!crate::runtime::queue::waiting_ids(&state, created).is_empty()).then_some(Busy::Queue)
}

type Time = chrono::DateTime<chrono::FixedOffset>;

/// Desde quando está adiado e quanto esperar até o próximo tique.
pub fn next(since: Option<Time>, outcome: &Tick, now: Time) -> (Option<Time>, Duration) {
    match outcome {
        Tick::Deferred(_) => (Some(since.unwrap_or(now)), RECHECK),
        _ => (None, INTERVAL),
    }
}

/// Um tique. `now` vem de fora (relógio do teste); `since` é desde quando a atualização está adiada.
pub async fn tick(machine: &impl Machine, now: Time, since: Option<Time>) -> Tick {
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
    // Já adiada, a máquina ainda ocupada responde sem ir ao índice a cada `RECHECK`.
    let early = if since.is_some() { Some(machine.busy().await) } else { None };
    if let Some(Ok(Some(busy))) = &early && (*busy == Busy::Uncertain || since.is_none_or(|since| now - since < DEFER_CEILING)) {
        return Tick::Deferred(busy.why().to_owned());
    }
    let version = match machine.newer(&channel).await {
        Ok(Some(version)) => version,
        Ok(None) => return skip("em dia"),
        // Já adiada, o índice fora não encerra o adiamento nem zera o teto.
        Err(e) if since.is_some() => {
            if since.is_some_and(|since| now - since >= DEFER_CEILING) && crate::warn_limit::allow(None, "update_deferred_index_down") {
                tracing::warn!(code = "update_deferred_index_down", "update: adiada há mais de 24 h e o índice não responde ({e}); segue adiada");
            }
            return Tick::Deferred(format!("índice ilegível: {e}"));
        }
        Err(e) => return Tick::Skipped(format!("índice ilegível: {e}")),
    };
    // A leitura de antes do índice não vale depois dele: durante a busca uma sessão começa ou uma entrega fica incerta.
    let busy = machine.busy().await;
    match busy {
        Ok(None) => {}
        Ok(Some(busy)) if busy == Busy::Uncertain || since.is_none_or(|since| now - since < DEFER_CEILING) => {
            return Tick::Deferred(busy.why().to_owned());
        }
        Ok(Some(busy)) => tracing::warn!("update: adiada há 24 h ({}); atualiza assim mesmo", busy.why()),
        // Já adiada, não saber se a máquina parou não solta nem zera o teto.
        Err(e) if since.is_some() => {
            if since.is_some_and(|since| now - since >= DEFER_CEILING) && crate::warn_limit::allow(None, "update_deferred_list_down") {
                tracing::warn!(code = "update_deferred_list_down", "update: adiada há mais de 24 h e a lista não responde ({e}); segue adiada");
            }
            return Tick::Deferred(format!("lista indisponível: {e}"));
        }
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
    pub state: Arc<crate::state::live::StateEnv>,
    pub listen: SocketAddr,
    pub client: reqwest::Client,
    pub runtime: crate::config_sync::runtime::RuntimeConfig,
}

impl Live {
    fn layout(&self) -> Layout { Layout::new(&self.env.home) }

}

impl Machine for Live {
    fn package_channel(&self) -> Option<String> {
        matches!(super::http::mode(&self.env.home), super::http::Mode::Package(_)).then(|| super::channel(&self.layout()))
    }

    fn automations(&self) -> bool {
        let value=self.runtime.get("automations");
        let enabled=crate::orchestration::truthy(&value);
        crate::orchestration::codec::discard(value);
        enabled
    }

    fn state(&self) -> super::StateSnapshot {
        super::for_screen(&super::state_dir(&self.env.claude_base), &self.layout(), super::running_package().map(|p| p.version.as_str()))
    }

    async fn newer(&self, channel: &str) -> Result<Option<String>, String> {
        // Canal trocado na tela: o publicado no outro vale mesmo com número menor, como no Atualizar; a mesma
        // versão não é troca nenhuma.
        let manifest = super::installed(&self.layout());
        let installed = manifest.as_ref().filter(|m| m.channel == channel).and_then(|m| m.version.parse().ok());
        let release = Release::github(channel);
        let entry = super::check(&self.client, &release, &super::platform(), installed).await.map_err(|e| e.to_string())?;
        Ok(entry.map(|e| e.version).filter(|v| manifest.as_ref().is_none_or(|m| *v != m.version)))
    }

    async fn busy(&self) -> Result<Option<Busy>, String> { busy_now(&self.list, &self.state).await }

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
        let (mut since, mut wait) = (None, INTERVAL);
        loop {
            tokio::time::sleep(wait).await;
            let now = chrono::Local::now().fixed_offset();
            let outcome = tick(&machine, now, since).await;
            let first = since.is_none();
            (since, wait) = next(since, &outcome, now);
            match outcome {
                Tick::Launched { version } => tracing::info!(version, "update: atualização automática lançada"),
                Tick::Skipped(why) if why == "fora do modo pacote" || why == "em dia" => tracing::debug!("update: automático: {why}"),
                Tick::Skipped(why) => tracing::info!("update: automático pulado: {why}"),
                Tick::Deferred(why) if first => tracing::info!("update: automático adiado: {why}"),
                Tick::Deferred(why) => tracing::debug!("update: automático ainda adiado: {why}"),
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
        busy: Option<Busy>,
        list_down: bool,
        index_down: bool,
        // Uma entrega fica incerta enquanto o índice é buscado.
        uncertain_after_index: bool,
        launched: AtomicUsize,
        newer_calls: AtomicUsize,
    }

    impl Default for Fake {
        fn default() -> Fake {
            Fake { channel: Some("latest"), automations: true, state: json!({"fase": "pronto", "ok": true}), newer: Some("0.1.0.9"),
                busy: None, list_down: false, index_down: false, uncertain_after_index: false, launched: AtomicUsize::new(0), newer_calls: AtomicUsize::new(0) }
        }
    }

    impl Machine for Fake {
        fn package_channel(&self) -> Option<String> { self.channel.map(str::to_owned) }
        fn automations(&self) -> bool { self.automations }
        fn state(&self) -> crate::update::StateSnapshot { self.state.as_object().cloned().unwrap().into() }
        async fn newer(&self, _channel: &str) -> Result<Option<String>, String> {
            self.newer_calls.fetch_add(1, Ordering::SeqCst);
            if self.index_down { Err("index_timeout".into()) } else { Ok(self.newer.map(str::to_owned)) }
        }
        async fn busy(&self) -> Result<Option<Busy>, String> {
            if self.list_down { return Err("list_timeout".into()); }
            if self.uncertain_after_index && self.newer_calls.load(Ordering::SeqCst) > 0 { return Ok(Some(Busy::Uncertain)); }
            Ok(self.busy)
        }
        async fn launch(&self) -> Result<(), String> { self.launched.fetch_add(1, Ordering::SeqCst); Ok(()) }
    }

    fn now() -> chrono::DateTime<chrono::FixedOffset> { chrono::DateTime::parse_from_rfc3339("2026-10-24T12:00:00-03:00").unwrap() }

    async fn launches(fake: Fake) -> usize {
        tick(&fake, now(), None).await;
        fake.launched.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn update_periodic_launches_only_with_automations_on_and_a_newer_version() {
        let fake = Fake::default();
        assert_eq!(tick(&fake, now(), None).await, Tick::Launched { version: "0.1.0.9".into() });
        assert_eq!(fake.launched.load(Ordering::SeqCst), 1);
        assert_eq!(launches(Fake { automations: false, ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { newer: None, ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { channel: None, ..Fake::default() }).await, 0, "modo desenvolvedor e máquina não migrada");
        assert_eq!(launches(Fake { channel: Some("beta"), ..Fake::default() }).await, 0);
        assert_eq!(launches(Fake { state: json!({"fase": "rodando"}), ..Fake::default() }).await, 0);
    }

    #[tokio::test]
    async fn update_periodic_waits_a_day_after_a_failure() {
        let failed = |ts: &str| Fake { state: json!({"fase": "pronto", "ok": false, "ts": ts}), ..Fake::default() };
        assert_eq!(launches(failed("2026-10-24T08:00:00-03:00")).await, 0, "falhou há 4 h");
        assert_eq!(launches(failed("2026-10-23T11:00:00-03:00")).await, 1, "falhou há 25 h");
        assert_eq!(launches(failed("lixo")).await, 1, "hora ilegível abre, como no Python");
    }

    #[tokio::test]
    async fn update_periodic_defers_while_a_session_works_a_queue_waits_or_a_delivery_is_uncertain() {
        for (busy, why) in [(Busy::Working, "sessao trabalhando"), (Busy::Queue, "fila com mensagem esperando"), (Busy::Uncertain, "entrega incerta")] {
            let fake = Fake { busy: Some(busy), ..Fake::default() };
            assert_eq!(tick(&fake, now(), None).await, Tick::Deferred(why.into()));
            assert_eq!(fake.launched.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn update_periodic_ceiling_releases_work_and_queue_but_not_an_uncertain_delivery() {
        let run = |busy, hours| async move {
            tick(&Fake { busy: Some(busy), ..Fake::default() }, now(), Some(now() - chrono::TimeDelta::hours(hours))).await
        };
        assert_eq!(run(Busy::Working, 23).await, Tick::Deferred("sessao trabalhando".into()), "adiado há 23 h ainda segura");
        assert_eq!(run(Busy::Working, 24).await, Tick::Launched { version: "0.1.0.9".into() });
        assert_eq!(run(Busy::Queue, 25).await, Tick::Launched { version: "0.1.0.9".into() });
        assert_eq!(run(Busy::Uncertain, 48).await, Tick::Deferred("entrega incerta".into()), "entrega incerta segura sempre");
    }

    #[test]
    fn update_periodic_rechecks_every_5_minutes_while_deferred() {
        let (later, deferred) = (now() + chrono::TimeDelta::minutes(5), Tick::Deferred("sessao trabalhando".into()));
        assert_eq!(next(None, &deferred, now()), (Some(now()), RECHECK));
        assert_eq!(next(Some(now()), &deferred, later), (Some(now()), RECHECK), "o adiamento conta desde o primeiro");
        assert_eq!(next(Some(now()), &Tick::Launched { version: "0.1.0.9".into() }, later), (None, INTERVAL));
        assert_eq!(next(Some(now()), &Tick::Skipped("em dia".into()), later), (None, INTERVAL));
    }

    #[tokio::test]
    async fn update_periodic_unknown_busy_keeps_a_deferral_without_resetting_the_ceiling() {
        let down = || Fake { list_down: true, ..Fake::default() };
        assert!(matches!(tick(&down(), now(), Some(now() - chrono::TimeDelta::hours(23))).await, Tick::Deferred(_)), "já adiada, segue adiada");
        assert!(matches!(tick(&down(), now(), None).await, Tick::Skipped(_)), "sem adiamento, só pula como antes");
        let (since, wait) = next(Some(now()), &tick(&down(), now(), Some(now())).await, now());
        assert_eq!((since, wait), (Some(now()), RECHECK), "o teto conta do primeiro adiamento");
    }

    #[tokio::test]
    async fn update_periodic_busy_deferral_does_not_fetch_the_index() {
        let since = Some(now() - chrono::TimeDelta::hours(1));
        let busy = Fake { busy: Some(Busy::Working), ..Fake::default() };
        assert!(matches!(tick(&busy, now(), since).await, Tick::Deferred(_)));
        assert_eq!(busy.newer_calls.load(Ordering::SeqCst), 0, "ocupada, nem vai ao índice");
    }

    #[tokio::test]
    async fn update_periodic_unreadable_index_keeps_the_deferral_start() {
        let first = now() - chrono::TimeDelta::hours(23);
        // Livre agora: o tique chega ao índice, que está fora.
        let down = Fake { index_down: true, ..Fake::default() };
        let (since, _) = next(Some(first), &tick(&down, now(), Some(first)).await, now());
        assert_eq!(down.newer_calls.load(Ordering::SeqCst), 1, "foi ao índice");
        assert_eq!(since, Some(first), "índice fora não zera o teto de 24 h");
        let later = now() + chrono::TimeDelta::hours(1);
        let queued = Fake { busy: Some(Busy::Queue), ..Fake::default() };
        assert_eq!(tick(&queued, later, since).await, Tick::Launched { version: "0.1.0.9".into() });
    }

    #[tokio::test]
    async fn update_periodic_past_the_ceiling_rereads_busy_after_the_index() {
        // Passado o teto, o trabalho solta; mas a entrega que ficou incerta durante a busca do índice segura.
        let first = now() - chrono::TimeDelta::hours(25);
        let fake = Fake { busy: Some(Busy::Working), uncertain_after_index: true, ..Fake::default() };
        assert_eq!(tick(&fake, now(), Some(first)).await, Tick::Deferred(Busy::Uncertain.why().to_owned()));
        assert_eq!(fake.launched.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn update_periodic_unreadable_queue_holds_like_a_waiting_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fila.json");
        assert_eq!(queue_hold(&path, 0.0), None, "sem arquivo, nada espera");
        std::fs::write(&path, b"{estragado").unwrap();
        assert_eq!(queue_hold(&path, 0.0), Some(Busy::Queue));
    }
}
