//! Os 9 passos do tour, na ordem, cada um com a condição de que a ação dele possa acontecer agora. Passo sem condição
//! sai do tour e vira dica, oferecida quando a condição surgir; o primeiro, sem ela, só some.
use std::collections::BTreeSet;
use super::{anchors, Step};

/// O que o app tem agora e decide quais passos cabem.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Sessões na lista, de todas as máquinas.
    pub sessions: usize,
    /// Contas Claude da máquina da sessão aberta.
    pub accounts: usize,
    /// Uma conta Codex na mesma lista: o anel leva a conversa para ela.
    pub codex: bool,
    pub transcription: bool,
    /// Agente da sessão aberta e legível.
    pub open: Option<Agent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent { Claude, Codex, Other }

impl Agent {
    pub fn of(provider: &str) -> Self {
        match provider { "claude" => Agent::Claude, "codex" => Agent::Codex, _ => Agent::Other }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need { EmptyList, Sessions(usize), Accounts, Nothing, OpenSession, Transcription, ClaudeOrCodex }

impl Need {
    fn holds(self, s: &Snapshot) -> bool {
        match self {
            Need::EmptyList => s.sessions == 0,
            Need::Sessions(n) => s.sessions >= n,
            Need::Accounts => s.accounts >= 2 || s.codex,
            Need::Nothing => true,
            Need::OpenSession => s.open.is_some(),
            Need::Transcription => s.transcription,
            Need::ClaudeOrCodex => matches!(s.open, Some(Agent::Claude | Agent::Codex)),
        }
    }
}

/// Id do passo de Shift+Tab: no Claude e no Codex ele troca o modo (`ctl-mode`).
pub const PERMISSION: &str = "permission";

const fn step(id: &'static str, anchor: &'static str, title_key: &'static str, body_key: &'static str) -> Step {
    Step { id, anchor, title_key, body_key }
}

/// Os ids são os de `TourEvent::step`. `SESSION_ROW` é a família: a linha certa sai na hora de desenhar.
const TOUR: [(Step, Need); 9] = [
    (step("new", anchors::NEW_SESSION, "tour_new_title", "tour_new_body"), Need::EmptyList),
    (step("menu", anchors::SESSION_ROW, "tour_menu_title", "tour_menu_body"), Need::Sessions(1)),
    (step("group", anchors::SESSION_ROW, "tour_group_title", "tour_group_body"), Need::Sessions(2)),
    (step("account", anchors::COMPOSER_ACCOUNT, "tour_account_title", "tour_account_body"), Need::Accounts),
    (step("search", anchors::TOPBAR_SEARCH, "tour_search_title", "tour_search_body"), Need::Nothing),
    (step("number", anchors::SESSION_ROW, "tour_number_title", "tour_number_body"), Need::Sessions(2)),
    (step("attach", anchors::COMPOSER, "tour_attach_title", "tour_attach_body"), Need::OpenSession),
    (step("dictation", anchors::DICTATION_TOGGLE, "tour_dictation_title", "tour_dictation_body"), Need::Transcription),
    (step(PERMISSION, anchors::CTL_MODE, "tour_permission_title", "tour_permission_body"), Need::ClaudeOrCodex),
];

/// Os passos que entram no tour e os que viram dica, na ordem.
fn split(s: &Snapshot) -> (Vec<Step>, Vec<(Step, Need)>) {
    let (mut steps, mut tips) = (Vec::new(), Vec::new());
    for (step, need) in TOUR {
        if need.holds(s) { steps.push(step); } else if need != Need::EmptyList { tips.push((step, need)); }
    }
    (steps, tips)
}

/// Id da dica nascida de um passo, para não se confundir com as dicas próprias no visto.
pub fn tip_id(step: &Step) -> String { format!("tour-{}", step.id) }

/// Planeja o tour com a lista lida e guarda os passos que viraram dica até a condição deles chegar.
#[derive(Default)]
pub struct Planner { list_read: bool, planned: bool, waiting: Vec<(Step, Need)> }

impl Planner {
    pub fn list_read(&mut self) { self.list_read = true; }

    /// A lista já chegou e o tour ainda não foi planejado.
    pub fn due(&self) -> bool { self.list_read && !self.planned }

    /// Os passos do tour. Os que ficaram de fora esperam como dica, salvo a dica já vista (`dicas`): com o tour já visto,
    /// é ela que sobrevive a reabrir o app.
    pub fn plan(&mut self, s: &Snapshot, dicas: &BTreeSet<String>) -> Vec<Step> {
        let (steps, mut waiting) = split(s);
        waiting.retain(|(step, _)| !dicas.contains(&tip_id(step)));
        self.planned = true;
        self.waiting = waiting;
        steps
    }

    pub fn waiting(&self) -> bool { !self.waiting.is_empty() }

    /// Os passos guardados cuja condição chegou; saem da espera.
    pub fn ready(&mut self, s: &Snapshot) -> Vec<Step> {
        let (ready, waiting): (Vec<_>, Vec<_>) = self.waiting.drain(..).partition(|(_, need)| need.holds(s));
        self.waiting = waiting;
        ready.into_iter().map(|(step, _)| step).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tour::{state::Seen, Choice, Tour, TourEvent};

    fn ids(steps: &[Step]) -> Vec<&'static str> { steps.iter().map(|s| s.id).collect() }

    fn tip_ids(s: &Snapshot) -> Vec<&'static str> { split(s).1.iter().map(|(step, _)| step.id).collect() }

    const FULL: Snapshot = Snapshot { sessions: 2, accounts: 2, codex: false, transcription: true, open: Some(Agent::Claude) };

    #[test]
    fn one_session_turns_drag_and_number_into_tips() {
        let s = Snapshot { sessions: 1, ..FULL };
        let (steps, _) = split(&s);
        assert!(!ids(&steps).contains(&"group") && !ids(&steps).contains(&"number"));
        assert_eq!(tip_ids(&s), ["group", "number"]);
    }

    #[test]
    fn one_account_without_codex_turns_the_ring_into_a_tip() {
        let s = Snapshot { accounts: 1, codex: false, ..FULL };
        assert_eq!(tip_ids(&s), ["account"]);
        assert!(!tip_ids(&Snapshot { accounts: 1, codex: true, ..FULL }).contains(&"account"), "o Codex basta");
    }

    #[test]
    fn empty_list_starts_with_the_first_session_and_a_list_with_sessions_drops_it() {
        let empty = Snapshot { sessions: 0, open: None, ..FULL };
        assert_eq!(split(&empty).0.first().map(|s| s.id), Some("new"));
        let (steps, tips) = split(&FULL);
        assert_eq!(ids(&steps), ["menu", "group", "account", "search", "number", "attach", "dictation", "permission"]);
        assert!(tips.is_empty(), "com sessão, o primeiro passo some sem virar dica");
    }

    #[test]
    fn tip_waits_for_its_condition() {
        let mut planner = Planner::default();
        planner.plan(&Snapshot { sessions: 1, ..FULL }, &BTreeSet::new());
        assert!(planner.ready(&Snapshot { sessions: 1, ..FULL }).is_empty());
        assert_eq!(ids(&planner.ready(&FULL)), ["group", "number"]);
        assert!(!planner.waiting());
    }

    #[test]
    fn reopening_keeps_waiting_only_for_missing_and_unseen_tips() {
        // Tour já visto noutra abertura: "number" já foi vista como dica; "menu" tem a condição e não vira dica.
        let mut planner = Planner::default();
        planner.plan(&Snapshot { sessions: 1, ..FULL }, &[tip_id(&TOUR[5].0)].into());
        assert!(planner.waiting());
        assert_eq!(ids(&planner.ready(&FULL)), ["group"]);
        let mut planner = Planner::default();
        planner.plan(&FULL, &BTreeSet::new());
        assert!(!planner.waiting());
    }

    #[test]
    fn shift_tab_points_at_mode_on_any_agent() {
        let (step, _) = TOUR.iter().find(|(step, _)| step.id == PERMISSION).unwrap();
        assert_eq!(step.anchor, anchors::CTL_MODE);
    }

    #[test]
    fn search_advances_only_its_own_step() {
        let mut tour = Tour::with_steps(Seen::default(), split(&FULL).0);
        let current = |tour: &Tour| tour.steps[tour.current.unwrap()].id;
        assert!(!tour.tour_event(TourEvent::SearchOpened));
        assert_eq!(current(&tour), "menu", "outro passo ativo: nada muda");
        while current(&tour) != "search" { crate::tour::tests::pick(&mut tour, Choice::Skip); }
        tour.tour_event(TourEvent::SearchOpened);
        assert_eq!(current(&tour), "number");
    }

    #[test]
    fn every_event_names_a_step() {
        let events = [TourEvent::SessionCreated, TourEvent::MenuOpened, TourEvent::GroupClosed, TourEvent::AccountMoved,
            TourEvent::SearchOpened, TourEvent::NumberedSession, TourEvent::ContextAdded, TourEvent::DictationStarted,
            TourEvent::PermissionCycled];
        let named: Vec<_> = events.iter().map(|e| e.step()).collect();
        assert_eq!(named, TOUR.map(|(step, _)| step.id));
    }

    #[test]
    fn every_step_text_exists_in_both_languages() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../messages");
        for language in ["pt.json", "en.json"] {
            let text = std::fs::read_to_string(dir.join(language)).unwrap();
            let dict: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
            let missing: Vec<_> = TOUR.iter().flat_map(|(step, _)| [step.title_key, step.body_key])
                .filter(|key| !dict.contains_key(&format!("native_{key}"))).collect();
            assert!(missing.is_empty(), "{language} sem native_*: {missing:?}");
        }
    }
}
