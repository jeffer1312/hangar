//! A regra na montagem da lista: a linha sai pela marca do dono ou por tempo parada; esperando
//! você, a marca manual cai de vez. Roda depois de `mark_stale` e antes de `gate_problems`, que
//! tira da linha o `problema` que a regra precisa ver.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hangar_api::session::SessionRow;

use super::auto::{self, AutoHide};
use super::{blocking, registered};
use super::store::{self, Mark};
use crate::diag::DiagClient;
use crate::list::mux::Mux;

/// `(oculta, tirar a marca)`. `state == "idle"` já exclui trabalhando e carregando; com `problema`, o
/// "parada" pode ser falso (retrato do runtime quebrado, fatos do Python velhos).
pub fn decide(row: &SessionRow, mark: Option<&Mark>, cfg: &AutoHide, now: f64) -> (bool, bool) {
    let waiting = row.state == "awaiting_input" || row.pending_questions > 0;
    let manual = mark.is_some_and(|m| m.hidden);
    if manual && waiting { return (false, true); }
    let since = |last: f64| mark.and_then(|m| m.shown_at).map_or(last, |shown| last.max(shown));
    let auto = cfg.enabled && !waiting && row.state == "idle" && row.problema.is_none()
        && row.last_activity.is_some_and(|last| now - since(last) > f64::from(cfg.after_minutes) * 60.0);
    (manual || auto, false)
}

/// Ausência da chave volta a ser perguntada depois disso: falha passageira do multiplexador não
/// deixa a sessão sem chave pela vida toda, e o tmux cru não é perguntado a cada tique.
const ABSENT_RETRY: Duration = Duration::from_secs(60);

/// A chave do pane por nome e vida `t:`, guardando também a ausência (o cache da sugestão de nome não guarda).
static PANE_KEYS: LazyLock<Mutex<HashMap<(String, String), (Option<String>, Instant)>>> = LazyLock::new(Mutex::default);

async fn pane_key(mux: &Mux, row: &SessionRow) -> Option<String> {
    let id = (row.name.clone(), row.lifecycle_id.clone()?);
    if let Some((key, at)) = PANE_KEYS.lock().unwrap_or_else(PoisonError::into_inner).get(&id)
        && (key.is_some() || at.elapsed() < ABSENT_RETRY) {
        return key.clone();
    }
    let key = mux.session_env(&row.name, "CP_SESSION_KEY").await;
    let mut keys = PANE_KEYS.lock().unwrap_or_else(PoisonError::into_inner);
    // ponytail: esquece tudo ao passar de 512 vidas; vale até haver centenas de sessões abertas.
    if keys.len() > 512 { keys.clear(); }
    keys.insert(id, (key.clone(), Instant::now()));
    key
}

/// Falha deixa a linha visível e vai ao log e ao diário; a rodada seguinte tenta de novo.
fn failed(diag: &DiagClient, code: &str, error: &io::Error) {
    // Roda a cada rodada da lista: arquivo ilegível repetiria a linha a cada 1,5 s.
    if crate::warn_limit::allow(None, code) {
        tracing::warn!(code, %error, "ocultar da lista: arquivo ilegível ou não gravado");
    }
    diag.report("rust.session_visibility", "", code, "marcas de oculta ilegiveis ou nao gravadas");
}

pub async fn fill_rows(home: &Path, mux: &Mux, rows: &mut [SessionRow], diag: &DiagClient) {
    for row in rows.iter_mut() { row.hidden_from_list = false; }
    let at = home.to_owned();
    let (marks, cfg) = match blocking(move || Ok((store::read(&at)?, auto::load(&at)?))).await {
        Ok(read) => read,
        Err(error) => return failed(diag, "session_visibility_unreadable", &error),
    };
    // Entrada `hidden: false` (deixada pelo "Mostrar") só importa com o automático ligado.
    if !cfg.enabled && !marks.values().any(|m| m.hidden) { return; }
    let mut keys = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        keys.push(match row.lifecycle_id.as_deref().and_then(|l| l.strip_prefix("k:")) {
            Some(key) => Some(key.to_owned()),
            None if !row.headless => pane_key(mux, row).await,
            None => None,
        });
    }
    // Sem registro a sessão não volta pelo "Mostrar" (a rota recusa): nem a marca nem o automático a escondem.
    let (at, asked) = (home.to_owned(), keys.iter().flatten().chain(marks.keys()).cloned().collect::<HashSet<_>>());
    let known = match blocking(move || {
        let mut known = HashSet::new();
        for key in asked { if registered(&at, &key)? { known.insert(key); } }
        Ok(known)
    }).await {
        Ok(known) => known,
        Err(error) => return failed(diag, "session_visibility_unreadable", &error),
    };
    let now = crate::state::monitor::wall_now();
    let mut clear = Vec::new();
    for (row, key) in rows.iter_mut().zip(keys) {
        let Some(key) = key.filter(|k| known.contains(k)) else { continue };
        let (hidden, clear_mark) = decide(row, marks.get(&key), &cfg, now);
        row.hidden_from_list = hidden;
        if clear_mark { clear.push(key); }
    }
    let stale = marks.keys().any(|k| !known.contains(k));
    if clear.is_empty() && !stale { return; }
    let at = home.to_owned();
    let written = blocking(move || {
        if !clear.is_empty() { store::unhide_if_hidden(&at, &clear, now)?; }
        // Na dúvida a marca fica: só some a chave cujo registro o disco confirma que não existe.
        if stale { store::prune(&at, |key| registered(&at, key).unwrap_or(true))?; }
        Ok(())
    }).await;
    if let Err(error) = written { failed(diag, "session_visibility_store_failed", &error); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ON: AutoHide = AutoHide { enabled: true, after_minutes: 180 };
    const NOW: f64 = 1_000_000.0;
    const MIN: f64 = 60.0;

    fn row(state: &str, idle_minutes: Option<f64>) -> SessionRow {
        serde_json::from_value(json!({"name": "s", "state": state, "last_activity": idle_minutes.map(|m| NOW - m * MIN)})).unwrap()
    }

    fn hidden(mark: bool, shown_minutes_ago: Option<f64>) -> Mark {
        Mark { hidden: mark, shown_at: shown_minutes_ago.map(|m| NOW - m * MIN) }
    }

    #[test]
    fn idle_past_the_deadline_hides() {
        assert_eq!(decide(&row("idle", Some(181.0)), None, &ON, NOW), (true, false));
        assert_eq!(decide(&row("idle", Some(179.0)), None, &ON, NOW), (false, false));
    }

    #[test]
    fn show_restarts_the_deadline() {
        assert_eq!(decide(&row("idle", Some(181.0)), Some(&hidden(false, Some(10.0))), &ON, NOW), (false, false));
    }

    #[test]
    fn a_problem_keeps_it_visible() {
        let mut stale = row("idle", Some(600.0));
        stale.problema = Some("list_facts_unavailable".into());
        assert_eq!(decide(&stale, None, &ON, NOW), (false, false));
    }

    #[test]
    fn working_or_waiting_never_hides_by_time() {
        assert_eq!(decide(&row("working", Some(600.0)), None, &ON, NOW), (false, false));
        assert_eq!(decide(&row("awaiting_input", Some(600.0)), None, &ON, NOW), (false, false));
        let mut asked = row("idle", Some(600.0));
        asked.pending_questions = 1;
        assert_eq!(decide(&asked, None, &ON, NOW), (false, false));
    }

    #[test]
    fn auto_off_keeps_it_visible() {
        assert_eq!(decide(&row("idle", Some(600.0)), None, &AutoHide::default(), NOW), (false, false));
    }

    #[test]
    fn the_manual_mark_hides_until_the_session_waits() {
        assert_eq!(decide(&row("idle", Some(1.0)), Some(&hidden(true, None)), &AutoHide::default(), NOW), (true, false));
        assert_eq!(decide(&row("awaiting_input", Some(1.0)), Some(&hidden(true, None)), &ON, NOW), (false, true));
        let mut asked = row("working", Some(1.0));
        asked.pending_questions = 2;
        assert_eq!(decide(&asked, Some(&hidden(true, None)), &ON, NOW), (false, true));
    }

    #[test]
    fn without_activity_only_the_mark_decides() {
        assert_eq!(decide(&row("idle", None), None, &ON, NOW), (false, false));
        assert_eq!(decide(&row("idle", None), Some(&hidden(true, None)), &ON, NOW), (true, false));
    }

    #[tokio::test]
    async fn a_waiting_session_loses_the_mark_and_a_closed_one_is_pruned() {
        let home = tempfile::tempdir().unwrap();
        let (live, gone) = ("a".repeat(32), "b".repeat(32));
        let specs = home.path().join(".hangar/session-specs");
        std::fs::create_dir_all(&specs).unwrap();
        std::fs::write(specs.join(format!("{live}.json")), "{}").unwrap();
        store::set_hidden(home.path(), &live, true, 1.0).unwrap();
        store::set_hidden(home.path(), &gone, true, 1.0).unwrap();
        let mut rows: Vec<SessionRow> = vec![serde_json::from_value(json!({"name": "s", "lifecycle_id": format!("k:{live}"),
            "state": "awaiting_input"})).unwrap()];
        let mux = Mux::with_program("missing-test-mux", Duration::from_secs(1));
        let diag = DiagClient::new("127.0.0.1:9".parse().unwrap(), "s".into());
        fill_rows(home.path(), &mux, &mut rows, &diag).await;
        assert!(!rows[0].hidden_from_list);
        let marks = store::read(home.path()).unwrap();
        assert!(!marks[&live].hidden && marks[&live].shown_at.is_some(), "a marca caiu: {marks:?}");
        assert!(!marks.contains_key(&gone), "sem registro sai do arquivo");
    }
}

#[cfg(test)]
mod tick {
    use super::*;
    use serde_json::json;

    /// `last_activity` fica fora da assinatura: sem o campo nela, a lista não reemitiria ao vencer o prazo.
    #[test]
    fn crossing_the_deadline_changes_the_list_signature() {
        let cfg = AutoHide { enabled: true, after_minutes: 180 };
        let mut row: SessionRow = serde_json::from_value(json!({"name": "s", "state": "idle", "last_activity": 0.0})).unwrap();
        row.hidden_from_list = decide(&row, None, &cfg, 179.0 * 60.0).0;
        let before = crate::list::sig::list_sig([&row]);
        row.hidden_from_list = decide(&row, None, &cfg, 181.0 * 60.0).0;
        assert!(row.hidden_from_list);
        assert_ne!(crate::list::sig::list_sig([&row]), before);
    }
}
