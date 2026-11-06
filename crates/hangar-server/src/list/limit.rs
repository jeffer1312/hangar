//! O aviso de limite de uso do Claude com terminal, visto pela lista: ela relê o fim do transcript quando
//! ele muda, já gravado (o plugin avisa o fim do turno antes de o Claude gravar a linha). Cada aviso vai uma
//! vez ao vigia da troca de conta, e só o da vida atual do agente: depois da troca a conversa segue no mesmo
//! arquivo, com o aviso velho por último até a vida nova responder.
use std::path::Path;

use hangar_api::session::SessionRow;

use super::capped::Capped;
use crate::runtime::protocol::UsageLimit;

/// Por sessão: o (transcript, mtime) já lido e o aviso já entregue (pela hora da linha).
#[derive(Default)]
pub struct LimitWatch(Capped<String, Seen>);

#[derive(Default)]
struct Seen { jsonl: String, mtime: Option<f64>, sent: Option<f64> }

/// Folga entre a hora do processo e a da linha, que vêm de relógios diferentes.
const SLACK: f64 = 2.0;

impl LimitWatch {
    /// O aviso novo da sessão parada, para o vigia. `agent_started`: quando o agente desta vida nasceu.
    pub fn seen(&mut self, row: &SessionRow, agent_started: Option<f64>, now: f64) -> Option<UsageLimit> {
        self.fresh(row, agent_started, |path| crate::session_write::limit_switch::file_limit_at(path, now))
    }

    /// A recusa do login nova, com o código dela. Cada aviso tem o seu `LimitWatch`: a hora entregue é por tipo.
    pub fn seen_auth(&mut self, row: &SessionRow, agent_started: Option<f64>) -> Option<String> {
        self.fresh(row, agent_started, crate::session_write::limit_switch::file_auth_error_at)
    }

    fn fresh<T>(&mut self, row: &SessionRow, agent_started: Option<f64>, read: impl Fn(&Path) -> Option<(T, Option<f64>)>) -> Option<T> {
        // Aguardando também: o menu do limite deixa a sessão assim, e só a troca o fecha.
        if row.provider != "claude" || row.headless || !matches!(row.state.as_str(), "idle" | "awaiting_input") { return None; }
        let jsonl = row.jsonl.as_deref().filter(|j| !j.is_empty())?;
        if self.0.get(&row.name).is_some_and(|s| s.jsonl == jsonl && s.mtime == row.last_activity) { return None; }
        let found = read(Path::new(jsonl));
        // Pela sessão, não pelo arquivo: a troca move a conversa para a pasta da conta nova, com o aviso dentro.
        let sent = self.0.get(&row.name).and_then(|s| s.sent);
        let mut seen = Seen { jsonl: jsonl.to_owned(), mtime: row.last_activity, sent };
        // Sem a hora do agente não dá para dizer que o aviso é desta vida: não guarda, relê no próximo tique.
        if found.is_some() && agent_started.is_none() {
            super::facts::note("rust.list_limit_seen", &row.name, "limit_seen_no_agent_start".into(), "aviso de limite sem a hora do agente; conferido de novo no próximo tique");
            return None;
        }
        let fresh = found.as_ref().and_then(|(_, at)| *at).zip(agent_started).filter(|(at, born)| *at + SLACK >= *born);
        let limit = match (found, fresh) {
            (Some((limit, Some(at))), Some(_)) if sent != Some(at) => { seen.sent = Some(at); Some(limit) }
            _ => None,
        };
        self.0.insert(row.name.clone(), seen);
        limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE_AT: f64 = 1792244628.273;
    const NOW: f64 = 1792249428.0;

    fn row(jsonl: &Path, mtime: f64) -> SessionRow {
        let mut row: SessionRow = serde_json::from_value(serde_json::json!({"name": "s1", "provider": "claude", "state": "idle"})).unwrap();
        row.jsonl = Some(jsonl.to_string_lossy().into_owned());
        row.last_activity = Some(mtime);
        row
    }

    /// A linha real do limite semanal vai ao vigia uma vez em vários tiques; a vida nova não a manda de novo.
    #[test]
    fn the_weekly_limit_on_the_list_reaches_the_switcher_once() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        std::fs::write(&jsonl, include_str!("../../tests/it/fixtures/usage_limit/claude_weekly_limit_tui_2026_10.jsonl")).unwrap();
        let mut watch = LimitWatch::default();
        assert_eq!(watch.seen(&row(&jsonl, 1.0), None, NOW), None, "sem a hora do agente, espera");
        let first = watch.seen(&row(&jsonl, 1.0), Some(LINE_AT - 600.0), NOW);
        assert_eq!(first, Some(UsageLimit { window: "seven_day".into(), resets_at: Some(1792270800.0) }));
        assert_eq!(watch.seen(&row(&jsonl, 1.0), Some(LINE_AT - 600.0), NOW), None, "o mesmo tique de novo");
        std::fs::write(&jsonl, format!("{}{}\n", std::fs::read_to_string(&jsonl).unwrap(), r#"{"type":"mode","mode":"x"}"#)).unwrap();
        assert_eq!(watch.seen(&row(&jsonl, 2.0), Some(LINE_AT - 600.0), NOW), None, "arquivo mudou, o aviso é o mesmo");
        let mut fresh = LimitWatch::default();
        assert_eq!(fresh.seen(&row(&jsonl, 2.0), Some(LINE_AT + 60.0), NOW), None, "a vida nova não herda o aviso da anterior");
        let mut working = row(&jsonl, 3.0);
        working.state = "working".into();
        assert_eq!(LimitWatch::default().seen(&working, Some(LINE_AT - 600.0), NOW), None, "só a parada");
        // A troca move a conversa para a pasta da conta nova: o aviso já entregue não vai de novo.
        let moved = dir.path().join("outra-conta.jsonl");
        std::fs::copy(&jsonl, &moved).unwrap();
        assert_eq!(watch.seen(&row(&moved, 5.0), Some(LINE_AT - 600.0), NOW), None, "o mesmo aviso no arquivo movido");
        let mut headless = row(&jsonl, 3.0);
        headless.headless = true;
        assert_eq!(LimitWatch::default().seen(&headless, Some(LINE_AT - 600.0), NOW), None, "sem terminal o ator vê");
    }

    /// O Claude Code abre o menu do limite junto com o aviso: a sessão fica aguardando e o vigia é quem o fecha.
    #[test]
    fn the_limit_reaches_the_switcher_while_its_menu_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        std::fs::write(&jsonl, include_str!("../../tests/it/fixtures/usage_limit/claude_weekly_limit_tui_2026_10.jsonl")).unwrap();
        let mut menu = row(&jsonl, 1.0);
        menu.state = "awaiting_input".into();
        assert_eq!(LimitWatch::default().seen(&menu, Some(LINE_AT - 600.0), NOW),
            Some(UsageLimit { window: "seven_day".into(), resets_at: Some(1792270800.0) }));
    }

    /// A recusa do login vai ao vigia uma vez, e a vida reaberta não herda a recusa da anterior.
    #[test]
    fn a_refused_login_reaches_the_switcher_once_and_only_from_this_life() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("c.jsonl");
        let line = serde_json::json!({"type":"assistant","isSidechain":false,"isApiErrorMessage":true,"error":"authentication_failed",
            "timestamp":"2026-10-17T13:43:48.273Z","message":{"content":[]}});
        std::fs::write(&jsonl, format!("{line}\n")).unwrap();
        let mut watch = LimitWatch::default();
        assert_eq!(watch.seen_auth(&row(&jsonl, 1.0), Some(LINE_AT - 600.0)).as_deref(), Some("authentication_failed"));
        assert_eq!(watch.seen_auth(&row(&jsonl, 2.0), Some(LINE_AT - 600.0)), None, "a mesma linha não conta como segunda recusa");
        assert_eq!(LimitWatch::default().seen_auth(&row(&jsonl, 2.0), Some(LINE_AT + 60.0)), None, "a vida nova ainda não falhou");
    }
}
