//! Sonho da memória (`crates/hangar-dream`): uma máquina, a escolhida, relê de noite as transcrições
//! de todas e corrige a pasta `memory/` do Claude Code de cada projeto. Aqui ficam o relógio, o
//! modelo pelo `claude` CLI, as rotas e as chamadas aos peers.

pub mod cli;
pub mod inventory;
pub mod peers;
pub mod round;
pub mod routes;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::Utc;
use hangar_dream::config::Config;
use hangar_dream::state::State;
use tokio::sync::Notify;

use crate::routes::AppState;

pub(crate) struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn of(st: &AppState) -> Self {
        Self { dir: st.hangar_dir.join("dream") }
    }
    pub fn config(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    pub fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    pub fn runs(&self) -> PathBuf {
        self.dir.join("runs")
    }
    pub fn work(&self) -> PathBuf {
        self.dir.join("work")
    }
}

/// Id desta máquina no `servidor::sessao`; sem id, `local`.
pub(crate) fn this_machine(st: &AppState) -> String {
    let id = st.groups.as_ref().map_or_else(|| st.server_id.clone(), |g| g.server_id());
    if id.is_empty() { "local".into() } else { id }
}

pub(crate) fn load_config(st: &AppState) -> Result<Config, String> {
    Config::load(&Paths::of(st).config()).map_err(|e| format!("dream_config_unreadable: {e}"))
}

pub(crate) fn load_state(st: &AppState) -> Result<State, String> {
    State::load(&Paths::of(st).state()).map_err(|e| format!("dream_state_unreadable: {e}"))
}

/// Rodada em andamento (uma por servidor): o `run_id`.
pub(crate) static RUNNING: LazyLock<Mutex<Option<String>>> = LazyLock::new(Default::default);
/// A configuração mudou: o relógio refaz a conta do próximo horário.
pub(crate) static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        st.startup.wait().await;
        loop {
            let wait = match next_wait(&st) {
                Some(wait) => wait,
                None => {
                    WAKE.notified().await;
                    continue;
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = WAKE.notified() => continue,
            }
            if let Err(e) = round::run(&st, round::Options::nightly()).await {
                st.diag.record_line(serde_json::json!({"evento": "dream.round_failed", "nivel": "aviso", "motivo": e}));
            }
        }
    })
}

/// Quanto falta para o próximo sonho; `None` = esta máquina não sonha (desligado ou outra escolhida).
fn next_wait(st: &AppState) -> Option<std::time::Duration> {
    let config = load_config(st).ok()?;
    if !config.enabled || config.machine != this_machine(st) {
        return None;
    }
    let zone = iana_time_zone::get_timezone().ok()?.parse::<chrono_tz::Tz>().ok()?;
    let schedule = hangar_scheduled_tasks::Schedule::Weekly { time: config.hour.clone(), weekdays: (0..7).collect() };
    let last = load_state(st).ok().and_then(|s| s.last_round_at).unwrap_or_else(|| Utc::now() - chrono::Duration::days(1));
    let next = hangar_scheduled_tasks::next_run(&schedule, last, &zone).ok()??;
    Some((next - Utc::now()).to_std().unwrap_or_default())
}
