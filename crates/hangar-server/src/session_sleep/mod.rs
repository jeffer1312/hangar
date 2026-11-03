//! Vigia de adormecer: a sessão oculta e parada além do prazo tem o processo fechado e acorda na próxima
//! mensagem. A lista só pré-seleciona; quem decide é a reconferência do `sleep_*` com a porta fechada.
use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_api::session::SessionRow;
use tokio::task::JoinHandle;

use crate::routes::AppState;
use crate::session_visibility::auto::{self, Sleep};

const ROUND: Duration = Duration::from_secs(300);
/// Espera pela primeira produção boa da lista antes de desistir da rodada.
const FIRST_LIST: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

/// Nomes das linhas que a vigia deve tentar adormecer. Claude sem terminal fica fora: ele já estaciona sozinho.
pub fn candidates(rows: &[SessionRow], cfg: &Sleep, now: f64) -> Vec<String> {
    let limit = f64::from(cfg.after_minutes) * 60.0;
    rows.iter().filter(|r| {
        matches!(r.provider.as_str(), "claude" | "codex")
            && !(r.provider == "claude" && r.headless)
            && r.hidden_from_list
            && r.state == "idle"
            && r.problema.is_none()
            && r.pending_questions == 0
            && !r.asleep
            && r.linked_by.is_none()
            && !rows.iter().any(|c| c.linked_by.as_deref() == Some(r.name.as_str()))
            && r.orq_arbiter.is_none()
            && !r.loop_status.as_deref().is_some_and(|s| crate::loops::store::ACTIVE.contains(&s))
            && r.last_activity.is_some_and(|last| now - last > limit)
    }).map(|r| r.name.clone()).collect()
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

pub fn spawn(st: Arc<AppState>) -> JoinHandle<()> { tokio::spawn(watch(st)) }

pub async fn watch(st: Arc<AppState>) {
    loop {
        tokio::time::sleep(ROUND).await;
        round(&st).await;
    }
}

async fn round(st: &Arc<AppState>) {
    let Some(home) = st.list.env().dirs.as_ref().map(|d| d.home.clone()) else { return };
    let cfg = match tokio::task::spawn_blocking(move || auto::load(&home)).await {
        Ok(Ok(auto)) => auto.sleep,
        _ => { st.diag.report("rust.session_sleep_failed", "", "session_sleep_config", "configuração de adormecer ilegível"); return }
    };
    if !cfg.enabled { return }
    let Some(registry) = st.side.runtime.get().cloned() else { return };
    // Assina só durante a rodada: lista aberta sem ninguém olhando custa um produtor de pé.
    let rows = {
        let sub = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
        let start = Instant::now();
        loop {
            if let Some(rows) = sub.rows() { break Some(rows) }
            if start.elapsed() >= FIRST_LIST { break None }
            tokio::time::sleep(POLL).await;
        }
    };
    let Some(rows) = rows else { return };
    let after = Duration::from_secs(u64::from(cfg.after_minutes) * 60);
    let mut slept = Vec::new();
    for name in candidates(&rows, &cfg, now()) {
        let Some(row) = rows.iter().find(|r| r.name == name) else { continue };
        let result = if row.provider == "codex" { registry.sleep_codex(&name, after).await }
            else { registry.sleep_claude_terminal(&name, after).await };
        match result {
            Ok(true) => slept.push(name),
            Ok(false) => {}
            Err(_) => st.diag.report("rust.session_sleep_failed", &name, "session_sleep_failed", "a sessão oculta não adormeceu"),
        }
    }
    if !slept.is_empty() {
        let events = slept.iter().map(|n| serde_json::json!({"evento":"sessao.adormecida","nivel":"ok","sessao":n})).collect();
        crate::diag::record(crate::diag::diary_dir(), events, "adormecer");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: f64 = 1_000_000.0;
    const CFG: Sleep = Sleep { enabled: true, after_minutes: 10 };

    fn row(extra: serde_json::Value) -> SessionRow {
        let mut v = json!({"name":"a","provider":"claude","headless":false,"hidden_from_list":true,"state":"idle","last_activity":NOW-601.0});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    fn picks(rows: Vec<SessionRow>) -> Vec<String> { candidates(&rows, &CFG, NOW) }

    #[test]
    fn hidden_idle_past_deadline_is_picked() {
        assert_eq!(picks(vec![row(json!({}))]), vec!["a"]);
        assert_eq!(picks(vec![row(json!({"provider":"codex","headless":true}))]), vec!["a"]);
    }

    #[test]
    fn exact_deadline_is_not_picked() {
        assert!(picks(vec![row(json!({"last_activity":NOW-600.0}))]).is_empty());
        assert!(picks(vec![row(json!({"last_activity":null}))]).is_empty());
    }

    #[test]
    fn busy_or_excluded_rows_are_left_out() {
        for extra in [
            json!({"hidden_from_list":false}), json!({"state":"awaiting_input"}), json!({"pending_questions":1}),
            json!({"problema":"x"}), json!({"linked_by":"p"}), json!({"orq_arbiter":"o"}),
            json!({"loop_status":"running"}), json!({"loop_status":"paused_awaiting"}), json!({"provider":"pi"}),
            json!({"provider":"omp"}), json!({"provider":"kimi"}), json!({"asleep":true}), json!({"headless":true}),
        ] {
            assert!(picks(vec![row(extra.clone())]).is_empty(), "{extra}");
        }
    }

    #[test]
    fn parent_of_a_linked_child_is_left_out() {
        let rows = vec![row(json!({})), row(json!({"name":"b","linked_by":"a"}))];
        assert!(picks(rows).is_empty());
    }

    #[test]
    fn finished_loop_does_not_block() {
        assert_eq!(picks(vec![row(json!({"loop_status":"done"}))]), vec!["a"]);
    }
}
