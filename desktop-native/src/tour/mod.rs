//! Tour de primeiro uso: balões presos às peças reais da tela, lidas pelo registro de âncoras (`app::popup`).
//! Aqui fica a fila (qual balão aparece agora) e o passo atual, sem janela; o desenho é o `balloon`.
// Dica, novidade e parte dos eventos só ganham chamador com as dicas, a sincronização e as novidades.
#![allow(dead_code)]
pub mod anchors;
pub mod balloon;
pub mod state;
pub mod steps;

use std::{collections::VecDeque, time::{Duration, Instant}};
use gpui_kit::{Bounds, Pixels};
use state::{Outcome, Seen, Switch};

/// Quanto dura a comemoração de um passo feito antes do próximo balão.
pub const CELEBRATION: Duration = Duration::from_millis(1300);

/// O que segura qualquer balão: diálogo aberto por cima (conexão, primeira execução, novidades), janela escondida na
/// bandeja, ou o que já foi visto ainda não lido.
#[derive(Clone, Copy, Debug)]
pub struct Gate { pub dialogs_open: bool, pub window_visible: bool, pub server_read: bool }

impl Gate {
    fn free(&self) -> bool { !self.dialogs_open && self.window_visible && self.server_read }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Tour, Tip, News }

/// Um passo do tour. As chaves são as de `tr`, sem o `native_`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step { pub id: &'static str, pub anchor: &'static str, pub title_key: &'static str, pub body_key: &'static str }

/// A ação real que faz um passo avançar, avisada de onde ela acontece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TourEvent {
    SessionCreated, MenuOpened, GroupClosed, AccountMoved, SearchOpened, NumberedSession, ContextAdded, DictationStarted,
    PermissionCycled,
}

impl TourEvent {
    /// O id do passo em `steps`.
    fn step(self) -> &'static str {
        match self {
            TourEvent::SessionCreated => "new",
            TourEvent::MenuOpened => "menu",
            TourEvent::GroupClosed => "group",
            TourEvent::AccountMoved => "account",
            TourEvent::SearchOpened => "search",
            TourEvent::NumberedSession => "number",
            TourEvent::ContextAdded => "attach",
            TourEvent::DictationStarted => "dictation",
            TourEvent::PermissionCycled => steps::PERMISSION,
        }
    }
}

/// Dica ou novidade esperando a vez, com a âncora e os textos já prontos.
#[derive(Clone, Debug, PartialEq)]
pub struct Note { pub id: String, pub anchor: String, pub title: String, pub body: String }

/// O balão deste quadro.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub kind: Kind,
    pub id: String,
    pub anchor: String,
    pub title: String,
    pub body: String,
    /// No tour: este passo e o total, contando de 1.
    pub progress: Option<(usize, usize)>,
    /// O passo acabou de ser feito: a comemoração começou aqui.
    pub celebrating: Option<Instant>,
}

/// Botão do balão.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice { Skip, Later, TipOk, TipsOff, Close }

pub struct Tour {
    pub seen: Seen,
    steps: Vec<Step>,
    /// Passo na tela; `None` com o tour encerrado, adiado ("Ver depois") ou já visto.
    current: Option<usize>,
    /// Algum passo desta volta foi feito pela ação, não pulado: o fim grava `feito`.
    done_any: bool,
    celebrated: Option<(usize, Instant)>,
    tips: VecDeque<Note>,
    news: Option<Note>,
    /// O local já foi lido (e, com a sincronização, o primeiro servidor respondeu ou falhou).
    pub server_read: bool,
    /// Onde o balão apareceu por último: a comemoração continua ali quando a peça some com a própria ação.
    pub last_target: Option<(String, Bounds<Pixels>)>,
    generation: u64,
}

impl Tour {
    pub fn new(seen: Seen) -> Self { Self::with_steps(seen, Vec::new()) }

    pub fn with_steps(seen: Seen, steps: Vec<Step>) -> Self {
        let current = (seen.tour.is_none() && !steps.is_empty()).then_some(0);
        Self { seen, steps, current, done_any: false, celebrated: None, tips: VecDeque::new(), news: None, server_read: false,
            last_target: None, generation: 0 }
    }

    /// Os passos dependem da lista de sessões: entram quando ela é lida, e de novo no "Ver o tour de novo".
    pub fn set_steps(&mut self, steps: Vec<Step>) {
        self.current = (self.seen.tour.is_none() && !steps.is_empty()).then_some(0);
        (self.steps, self.done_any, self.celebrated) = (steps, false, None);
    }

    /// O que gravar: o estado e a geração desta gravação (`state::save`).
    pub fn to_save(&mut self) -> (Seen, u64) {
        self.generation += 1;
        (self.seen.clone(), self.generation)
    }

    /// Um balão por vez: a novidade que a pessoa pediu, a comemoração, o passo do tour, e só depois a dica.
    pub fn next_surface(&self, gate: &Gate) -> Option<Surface> {
        if !gate.free() { return None; }
        if let Some(news) = &self.news { return Some(note(Kind::News, news)); }
        if let Some((step, at)) = self.celebrated.filter(|(_, at)| at.elapsed() < CELEBRATION) { return Some(self.step_surface(step, Some(at))); }
        if let Some(step) = self.current { return Some(self.step_surface(step, None)); }
        self.tips.front().map(|tip| note(Kind::Tip, tip))
    }

    fn step_surface(&self, index: usize, celebrating: Option<Instant>) -> Surface {
        let step = self.steps[index];
        Surface { kind: Kind::Tour, id: step.id.to_owned(), anchor: step.anchor.to_owned(), title: crate::i18n::tr(step.title_key),
            body: crate::i18n::tr(step.body_key), progress: Some((index + 1, self.steps.len())), celebrating }
    }

    pub fn running(&self) -> bool { self.current.is_some() }

    /// Diz se o visto mudou e precisa ser gravado.
    pub fn choose(&mut self, choice: Choice) -> bool {
        match choice {
            Choice::Skip => self.advance(false),
            // Não grava: o tour volta na próxima abertura.
            Choice::Later => { self.current = None; self.celebrated = None; false }
            Choice::TipOk => match self.tips.pop_front() {
                Some(tip) => self.seen.dicas.insert(tip.id),
                None => false,
            },
            Choice::TipsOff => {
                self.tips.clear();
                self.seen.dicas_desligadas = Some(Switch::now(true));
                true
            }
            Choice::Close => { self.news = None; false }
        }
    }

    /// A ação real aconteceu: avança só se é a do passo na tela. Diz se o visto mudou.
    pub fn tour_event(&mut self, event: TourEvent) -> bool {
        match self.current {
            Some(index) if self.steps[index].id == event.step() => self.advance(true),
            _ => false,
        }
    }

    fn advance(&mut self, done: bool) -> bool {
        let Some(index) = self.current else { return false };
        if done {
            self.done_any = true;
            self.celebrated = Some((index, Instant::now()));
        }
        if index + 1 < self.steps.len() {
            self.current = Some(index + 1);
            return false;
        }
        self.current = None;
        self.seen.tour = self.seen.tour.max(Some(if self.done_any { Outcome::Feito } else { Outcome::Pulado }));
        true
    }

    /// "Ver o tour de novo": zera só o tour, as dicas ficam. Diz se o visto mudou.
    pub fn restart(&mut self) -> bool {
        self.current = (!self.steps.is_empty()).then_some(0);
        (self.done_any, self.celebrated) = (false, None);
        self.seen.tour.take().is_some()
    }

    /// Dica que ainda não foi vista entra na fila; espera o tour acabar.
    pub fn offer_tip(&mut self, tip: Note) {
        if self.seen.tips_off() || self.seen.dicas.contains(&tip.id) || self.tips.iter().any(|t| t.id == tip.id) { return; }
        self.tips.push_back(tip);
    }

    /// "Mostrar no app" de uma novidade: aparece na hora, por cima do tour.
    pub fn show_news(&mut self, news: Note) { self.news = Some(news); }
}

fn note(kind: Kind, note: &Note) -> Surface {
    Surface { kind, id: note.id.clone(), anchor: note.anchor.clone(), title: note.title.clone(), body: note.body.clone(), progress: None,
        celebrating: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: Gate = Gate { dialogs_open: false, window_visible: true, server_read: true };

    fn steps() -> Vec<Step> {
        ["new", "search", "menu"].map(|id| Step { id, anchor: anchors::COMPOSER, title_key: "tour_later", body_key: "tour_later" }).to_vec()
    }

    fn tip(id: &str) -> Note { Note { id: id.to_owned(), anchor: anchors::QUEUE_PANEL.to_owned(), title: "t".into(), body: "b".into() } }

    #[test]
    fn open_dialog_holds_the_tour() {
        let tour = Tour::with_steps(Seen::default(), steps());
        assert_eq!(tour.next_surface(&Gate { dialogs_open: true, ..FREE }), None);
        assert!(tour.next_surface(&FREE).is_some());
    }

    #[test]
    fn hidden_window_or_unread_state_holds_everything() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.offer_tip(tip("fila"));
        assert_eq!(tour.next_surface(&Gate { window_visible: false, ..FREE }), None);
        assert_eq!(tour.next_surface(&Gate { server_read: false, ..FREE }), None);
    }

    #[test]
    fn tour_comes_before_a_pending_tip_and_the_tip_follows_the_end() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.offer_tip(tip("fila"));
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!((shown.kind, shown.id.as_str(), shown.progress), (Kind::Tour, "new", Some((1, 3))));
        tour.choose(Choice::Later);
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!((shown.kind, shown.id.as_str()), (Kind::Tip, "fila"));
    }

    #[test]
    fn skipping_every_step_records_skipped() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(!tour.choose(Choice::Skip));
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "search");
        assert!(!tour.choose(Choice::Skip));
        assert!(tour.choose(Choice::Skip), "o fim grava");
        assert_eq!(tour.seen.tour, Some(Outcome::Pulado));
        assert_eq!(tour.next_surface(&FREE), None);
    }

    #[test]
    fn one_step_done_by_the_action_records_done_and_celebrates() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(!tour.tour_event(TourEvent::SearchOpened), "o passo na tela é outro");
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "new");
        tour.choose(Choice::Skip);
        assert!(!tour.tour_event(TourEvent::SearchOpened));
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!(shown.id, "search", "a comemoração fica no passo feito");
        assert!(shown.celebrating.is_some());
        assert!(tour.choose(Choice::Skip));
        assert_eq!(tour.seen.tour, Some(Outcome::Feito));
    }

    #[test]
    fn later_closes_without_recording_and_seen_tour_does_not_start() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(!tour.choose(Choice::Later));
        assert_eq!(tour.seen, Seen::default());
        assert_eq!(tour.next_surface(&FREE), None);
        let done = Tour::with_steps(Seen { tour: Some(Outcome::Pulado), ..Default::default() }, steps());
        assert_eq!(done.next_surface(&FREE), None);
    }

    #[test]
    fn restart_resets_the_tour_but_keeps_the_tips() {
        let seen = Seen { tour: Some(Outcome::Feito), dicas: ["fila".to_owned()].into(), ..Default::default() };
        let mut tour = Tour::with_steps(seen, steps());
        assert!(tour.restart());
        assert_eq!(tour.seen.tour, None);
        assert!(tour.seen.dicas.contains("fila"));
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "new");
    }

    #[test]
    fn seen_tip_or_tips_off_never_queue() {
        let mut tour = Tour::new(Seen { dicas: ["fila".to_owned()].into(), ..Default::default() });
        tour.offer_tip(tip("fila"));
        assert_eq!(tour.next_surface(&FREE), None);
        tour.offer_tip(tip("citar"));
        assert!(tour.choose(Choice::TipsOff));
        tour.offer_tip(tip("abas"));
        assert_eq!(tour.next_surface(&FREE), None);
    }

    #[test]
    fn news_shows_over_the_tour_and_closes_without_recording() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.show_news(tip("novidade"));
        assert_eq!(tour.next_surface(&FREE).unwrap().kind, Kind::News);
        assert!(!tour.choose(Choice::Close));
        assert_eq!(tour.next_surface(&FREE).unwrap().kind, Kind::Tour);
    }
}
