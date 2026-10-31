//! Tour de primeiro uso: balões presos às peças reais da tela, lidas pelo registro de âncoras (`app::popup`).
//! Aqui fica a fila (qual balão aparece agora) e o passo atual, sem janela; o desenho é o `balloon`.
pub mod anchors;
pub mod balloon;
pub mod state;
pub mod steps;

use std::{collections::{BTreeSet, VecDeque}, time::{Duration, Instant}};
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

    /// A peça do passo: a novidade presa a ela também é aprendida pela ação.
    fn anchor(self) -> &'static str {
        match self {
            TourEvent::SessionCreated => anchors::NEW_SESSION,
            TourEvent::MenuOpened | TourEvent::GroupClosed | TourEvent::NumberedSession => anchors::SESSION_ROW,
            TourEvent::AccountMoved => anchors::COMPOSER_ACCOUNT,
            TourEvent::SearchOpened => anchors::TOPBAR_SEARCH,
            TourEvent::ContextAdded => anchors::COMPOSER,
            TourEvent::DictationStarted => anchors::DICTATION_TOGGLE,
            TourEvent::PermissionCycled => anchors::CTL_MODE,
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
pub enum Choice { Skip, SkipTour, Later, TipOk, TipsOff, Close }

pub struct Tour {
    pub seen: Seen,
    steps: Vec<Step>,
    /// Passo na tela; `None` com o tour encerrado, adiado ("Ver depois") ou já visto.
    current: Option<usize>,
    /// Algum passo desta volta foi feito pela ação, não pulado: o fim grava `done`.
    done_any: bool,
    celebrated: Option<(usize, Instant)>,
    tips: VecDeque<Note>,
    /// A novidade do "Mostrar no app" e, feita a ação da peça dela, desde quando comemora.
    news: Option<(Note, Option<Instant>)>,
    /// O primeiro servidor respondeu ou falhou na leitura do visto: antes disso o tour abriria sem saber que outro
    /// aparelho já o mostrou.
    pub server_read: bool,
    /// Um passo desta volta já apareceu, ou a pessoa pediu "Ver o tour de novo": o visto que chega do servidor não o tira
    /// da tela.
    keep_open: bool,
    /// Onde o balão apareceu por último: a comemoração continua ali quando a peça some com a própria ação.
    pub last_target: Option<(String, Bounds<Pixels>)>,
    generation: u64,
    /// `tour.json` existe mas não foi lido: gravar agora apagaria o que já foi visto.
    pub read_failed: bool,
    /// Celulares novos cuja dica ainda não foi respondida: só vão ao visto com a resposta, senão a dica que não chegou
    /// à tela (peça fora, app fechado) se perderia para eles.
    pending_devices: BTreeSet<String>,
}

impl Tour {
    pub fn new(seen: Seen) -> Self {
        Self { seen, steps: Vec::new(), current: None, done_any: false, celebrated: None, tips: VecDeque::new(), news: None,
            server_read: false, keep_open: false, last_target: None, generation: 0, read_failed: false, pending_devices: BTreeSet::new() }
    }

    /// O que muda o balão na tela: quem chama compara antes e depois para não redesenhar à toa.
    pub fn visible_state(&self) -> (Option<usize>, Option<usize>, Option<(String, bool)>) {
        (self.current, self.celebrated.map(|c| c.0), self.news.as_ref().map(|(n, d)| (n.id.clone(), d.is_some())))
    }

    #[cfg(test)]
    pub fn with_steps(seen: Seen, steps: Vec<Step>) -> Self {
        let mut tour = Self::new(seen);
        tour.set_steps(steps);
        tour
    }

    /// Os passos dependem da lista de sessões: entram quando ela é lida, e de novo no "Ver o tour de novo".
    pub fn set_steps(&mut self, steps: Vec<Step>) {
        // Reaberto no meio do tour: o que já passou não volta; "Ver o tour de novo" (keep_open) mostra tudo.
        let steps: Vec<Step> = if self.keep_open { steps } else {
            steps.into_iter().filter(|step| !self.seen.tips.contains(&steps::tip_id(step))).collect()
        };
        self.current = (self.seen.tour.is_none() && !steps.is_empty()).then_some(0);
        (self.steps, self.done_any, self.celebrated) = (steps, false, None);
    }

    /// O que gravar: o estado e a geração desta gravação (`state::save`); nada com a leitura falha.
    pub fn to_save(&mut self) -> Option<(Seen, u64)> {
        if self.read_failed { return None; }
        self.generation += 1;
        Some((self.seen.clone(), self.generation))
    }

    /// Um balão por vez: a novidade que a pessoa pediu, a comemoração, o passo do tour, e só depois a dica.
    pub fn next_surface(&self, gate: &Gate) -> Option<Surface> {
        if !gate.free() { return None; }
        if let Some((news, done)) = &self.news && done.is_none_or(|at| at.elapsed() < CELEBRATION) {
            return Some(Surface { celebrating: *done, ..note(Kind::News, news) });
        }
        if let Some((step, at)) = self.celebrated.filter(|(_, at)| at.elapsed() < CELEBRATION) { return Some(self.step_surface(step, Some(at))); }
        if let Some(step) = self.current { return Some(self.step_surface(step, None)); }
        // Desligadas noutro aparelho depois de entrar na fila: não aparecem.
        self.tips.front().filter(|_| !self.seen.tips_off()).map(|tip| note(Kind::Tip, tip))
    }

    fn step_surface(&self, index: usize, celebrating: Option<Instant>) -> Surface {
        let step = self.steps[index];
        Surface { kind: Kind::Tour, id: step.id.to_owned(), anchor: step.anchor.to_owned(), title: crate::i18n::tr(step.title_key),
            body: crate::i18n::tr(step.body_key), progress: Some((index + 1, self.steps.len())), celebrating }
    }

    /// O balão a que o botão pertence: o passo, a dica da frente ou a novidade.
    fn owner(&self, choice: Choice) -> Option<&str> {
        match choice {
            Choice::Skip | Choice::SkipTour | Choice::Later => self.current.map(|index| self.steps[index].id),
            Choice::TipOk | Choice::TipsOff => self.tips.front().map(|tip| tip.id.as_str()),
            Choice::Close => self.news.as_ref().filter(|(_, done)| done.is_none()).map(|(news, _)| news.id.as_str()),
        }
    }

    /// `id` é o do balão clicado: clique que chega depois de o balão mudar (duplo clique, ação que avançou o passo) não
    /// vale para o seguinte. Diz se o visto mudou e precisa ser gravado.
    pub fn choose(&mut self, id: &str, choice: Choice) -> bool {
        if self.owner(choice) != Some(id) { return false; }
        match choice {
            Choice::Skip => self.advance(false),
            // Quem já conhece o app encerra de uma vez: não volta sozinho, só por "Ver o tour de novo".
            Choice::SkipTour => {
                (self.current, self.celebrated) = (None, None);
                self.seen.tour = self.seen.tour.max(Some(if self.done_any { Outcome::Done } else { Outcome::Skipped }));
                true
            }
            // Não grava: o tour volta na próxima abertura.
            Choice::Later => { self.current = None; self.celebrated = None; false }
            Choice::TipOk => match self.tips.pop_front() {
                Some(tip) => {
                    let devices = tip.id == steps::PRESENCE && self.devices_answered();
                    self.seen.tips.insert(tip.id) | devices
                }
                None => false,
            },
            Choice::TipsOff => {
                if self.tips.front().is_some_and(|tip| tip.id == steps::PRESENCE) { self.devices_answered(); }
                self.tips.clear();
                self.seen.tips_switch = Some(Switch::now(true));
                true
            }
            Choice::Close => { self.news = None; false }
        }
    }

    /// A ação real aconteceu: avança só se é a do passo na tela. Diz se o visto mudou.
    pub fn tour_event(&mut self, event: TourEvent) -> bool {
        // A novidade não grava nada: só comemora e sai.
        if let Some((news, done)) = &mut self.news && done.is_none() && news.anchor == event.anchor() { *done = Some(Instant::now()); }
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
        // Passo que já passou pelo tour não volta como dica quando a condição dele some e reaparece.
        let marked = self.seen.tips.insert(steps::tip_id(&self.steps[index]));
        if index + 1 < self.steps.len() {
            self.current = Some(index + 1);
            return marked;
        }
        self.current = None;
        self.seen.tour = self.seen.tour.max(Some(if self.done_any { Outcome::Done } else { Outcome::Skipped }));
        true
    }

    /// O visto de um servidor chegou. Tour que ainda não apareceu e que o servidor diz visto não abre. Diz se o visto mudou.
    pub fn absorb(&mut self, server: Seen) -> bool {
        let before = self.seen.clone();
        // `devices` é só deste app.
        self.seen.merge(server.for_server());
        if self.seen.tour.is_some() && !self.keep_open { self.current = None; }
        self.seen != before
    }

    /// O balão deste quadro foi desenhado.
    pub fn showing(&mut self, kind: Kind) { if kind == Kind::Tour { self.keep_open = true; } }

    /// "Ver o tour de novo": zera só o tour, as dicas ficam; quem chama refaz os passos (`set_steps`). Diz se o visto mudou.
    pub fn restart(&mut self) -> bool {
        self.keep_open = true;
        self.seen.tour.take().is_some()
    }

    /// Dica que ainda não foi vista entra na fila; espera o tour acabar.
    pub fn offer_tip(&mut self, tip: Note) {
        if !self.may_offer(&tip.id) { return; }
        self.tips.push_back(tip);
    }

    /// A dica ainda pode aparecer (pode já estar na fila): é o que decide desenhar a âncora dela.
    /// A do celular volta com aparelho novo pendente, mesmo já vista: a junção com o servidor a devolveria ao visto.
    pub fn tip_pending(&self, id: &str) -> bool {
        !self.seen.tips_off() && (!self.seen.tips.contains(id) || (id == steps::PRESENCE && !self.pending_devices.is_empty()))
    }

    /// A dica entraria agora na fila.
    pub fn may_offer(&self, id: &str) -> bool { self.tip_pending(id) && !self.tips.iter().any(|t| t.id == id) }

    pub fn queued_tips(&self) -> usize { self.tips.len() }

    /// A dica da frente cuja peça não está na tela vai para o fim, para não segurar as outras.
    pub fn defer_tip(&mut self) {
        if let Some(tip) = self.tips.pop_front() { self.tips.push_back(tip); }
    }

    /// O "Mostrar dicas" das Configurações. Diz se o visto mudou.
    pub fn set_tips(&mut self, on: bool) -> bool {
        if self.seen.tips_off() != on { return false; }
        if !on { self.tips.clear(); }
        self.seen.tips_switch = Some(Switch::now(!on));
        true
    }

    /// Ids novos no retrato de aparelhos da lista do dono ficam pendentes e trazem a dica do celular de volta, uma vez por
    /// aparelho: só a resposta à dica os grava. Diz se há aparelho esperando a dica (a fila esvaziada por desligar as
    /// dicas a recebe de volta no próximo retrato); o visto não muda.
    pub fn devices_seen(&mut self, devices: &serde_json::Value) -> bool {
        let ids = devices.as_array().into_iter().flatten().filter_map(|device| device.get("id")?.as_str());
        for id in ids {
            if !self.seen.devices.contains(id) { self.pending_devices.insert(id.to_owned()); }
        }
        !self.pending_devices.is_empty()
    }

    /// A dica do celular foi respondida: os aparelhos pendentes passam ao visto. Diz se ele mudou.
    fn devices_answered(&mut self) -> bool {
        let before = self.seen.devices.len();
        self.seen.devices.append(&mut self.pending_devices);
        self.seen.devices.len() != before
    }

    /// "Mostrar no app" de uma novidade: aparece na hora, por cima do tour.
    pub fn show_news(&mut self, news: Note) { self.news = Some((news, None)); }
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

    /// Clique no balão que está na vez.
    pub(crate) fn pick(tour: &mut Tour, choice: Choice) -> bool {
        let id = tour.owner(choice).unwrap_or_default().to_owned();
        tour.choose(&id, choice)
    }

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
        pick(&mut tour, Choice::Later);
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!((shown.kind, shown.id.as_str()), (Kind::Tip, "fila"));
    }

    #[test]
    fn skipping_the_tour_ends_it_for_good_until_asked_again() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(pick(&mut tour, Choice::SkipTour), "grava");
        assert_eq!(tour.seen.tour, Some(Outcome::Skipped));
        assert_eq!(tour.next_surface(&FREE), None);
        let reopened = Tour::with_steps(tour.seen.clone(), steps());
        assert_eq!(reopened.next_surface(&FREE), None, "não volta sozinho na próxima abertura");
        let mut asked = Tour::with_steps(tour.seen.clone(), steps());
        assert!(asked.restart());
        asked.set_steps(steps());
        assert_eq!(asked.next_surface(&FREE).unwrap().id, "new", "\"Ver o tour de novo\" traz de volta");
    }

    #[test]
    fn skipping_every_step_records_skipped() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(pick(&mut tour, Choice::Skip), "o passo pulado não volta como dica");
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "search");
        assert!(pick(&mut tour, Choice::Skip));
        assert!(pick(&mut tour, Choice::Skip), "o fim grava");
        assert_eq!(tour.seen.tour, Some(Outcome::Skipped));
        assert!(tour.seen.tips.contains("tour-search"));
        assert_eq!(tour.next_surface(&FREE), None);
    }

    #[test]
    fn one_step_done_by_the_action_records_done_and_celebrates() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(!tour.tour_event(TourEvent::SearchOpened), "o passo na tela é outro");
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "new");
        pick(&mut tour, Choice::Skip);
        assert!(tour.tour_event(TourEvent::SearchOpened), "o passo feito não volta como dica");
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!(shown.id, "search", "a comemoração fica no passo feito");
        assert!(shown.celebrating.is_some());
        assert!(pick(&mut tour, Choice::Skip));
        assert_eq!(tour.seen.tour, Some(Outcome::Done));
    }

    #[test]
    fn later_closes_without_recording_and_seen_tour_does_not_start() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        assert!(!pick(&mut tour, Choice::Later));
        assert_eq!(tour.seen, Seen::default());
        assert_eq!(tour.next_surface(&FREE), None);
        let done = Tour::with_steps(Seen { tour: Some(Outcome::Skipped), ..Default::default() }, steps());
        assert_eq!(done.next_surface(&FREE), None);
    }

    #[test]
    fn restart_resets_the_tour_but_keeps_the_tips() {
        let seen = Seen { tour: Some(Outcome::Done), tips: ["fila".to_owned()].into(), ..Default::default() };
        let mut tour = Tour::with_steps(seen, steps());
        assert!(tour.restart());
        tour.set_steps(steps());
        assert_eq!(tour.seen.tour, None);
        assert!(tour.seen.tips.contains("fila"));
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "new");
    }

    #[test]
    fn reopening_mid_tour_skips_the_steps_already_passed() {
        let seen = Seen { tips: ["tour-new".to_owned()].into(), ..Default::default() };
        let tour = Tour::with_steps(seen, steps());
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "search", "o passo feito antes de fechar não volta");
    }

    #[test]
    fn seen_tip_or_tips_off_never_queue() {
        let mut tour = Tour::new(Seen { tips: ["fila".to_owned()].into(), ..Default::default() });
        tour.offer_tip(tip("fila"));
        assert_eq!(tour.next_surface(&FREE), None);
        tour.offer_tip(tip("citar"));
        assert!(pick(&mut tour, Choice::TipsOff));
        tour.offer_tip(tip("abas"));
        assert_eq!(tour.next_surface(&FREE), None);
    }

    #[test]
    fn click_from_a_balloon_that_already_left_is_ignored() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.offer_tip(tip("fila"));
        tour.offer_tip(tip("citar"));
        tour.choose("new", Choice::Later);
        assert!(tour.choose("fila", Choice::TipOk));
        assert!(!tour.choose("fila", Choice::TipOk), "o segundo clique era da dica que já saiu");
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "citar");

        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.tour_event(TourEvent::SessionCreated);
        assert!(!tour.choose("new", Choice::Skip), "o Pular era do passo que a ação já fez");
        assert_eq!(tour.steps[tour.current.unwrap()].id, "search");
    }

    #[test]
    fn unreadable_state_is_never_saved() {
        let mut tour = Tour::new(Seen::default());
        assert!(tour.to_save().is_some());
        tour.read_failed = true;
        assert_eq!(tour.to_save(), None);
    }

    #[test]
    fn seen_on_the_server_cancels_only_a_tour_not_yet_shown_nor_asked_for() {
        let done = || Seen { tour: Some(Outcome::Done), devices: ["ios:x".to_owned()].into(), ..Default::default() };
        let mut waiting = Tour::with_steps(Seen::default(), steps());
        assert!(waiting.absorb(done()));
        assert_eq!(waiting.next_surface(&FREE), None);
        assert!(waiting.seen.devices.is_empty(), "aparelhos nunca vêm do servidor");

        let mut on_screen = Tour::with_steps(Seen::default(), steps());
        on_screen.showing(on_screen.next_surface(&FREE).unwrap().kind);
        on_screen.absorb(done());
        assert_eq!(on_screen.next_surface(&FREE).unwrap().id, "new");

        let mut asked = Tour::with_steps(Seen { tour: Some(Outcome::Done), ..Default::default() }, steps());
        asked.restart();
        asked.set_steps(steps());
        assert!(asked.absorb(done()));
        assert_eq!(asked.seen.tour, Some(Outcome::Done));
        assert_eq!(asked.next_surface(&FREE).unwrap().id, "new", "o tour pedido pela pessoa continua");

        let mut nothing_new = Tour::new(Seen::default());
        assert!(!nothing_new.absorb(Seen::default()));
    }

    #[test]
    fn hidden_tip_goes_to_the_back_and_the_next_one_shows() {
        let mut tour = Tour::new(Seen::default());
        tour.offer_tip(tip("fila"));
        tour.offer_tip(tip("citar"));
        tour.defer_tip();
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "citar");
        assert!(pick(&mut tour, Choice::TipOk), "o Entendi vale para a que está na tela");
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "fila");
        assert!(!tour.may_offer("citar") && !tour.may_offer("fila"), "vista e na fila não entram de novo");
        assert!(tour.tip_pending("fila"), "a da fila ainda desenha a âncora");
    }

    #[test]
    fn tips_switch_turns_them_off_and_back_on() {
        let mut tour = Tour::new(Seen::default());
        tour.offer_tip(tip("fila"));
        assert!(tour.set_tips(false));
        assert!(!tour.set_tips(false), "já desligadas");
        assert_eq!(tour.next_surface(&FREE), None);
        assert!(!tour.tip_pending("citar"));
        assert!(tour.set_tips(true));
        assert_eq!(tour.seen.tips_switch.as_ref().map(|s| s.on), Some(false));
        tour.offer_tip(tip("citar"));
        assert_eq!(tour.next_surface(&FREE).unwrap().id, "citar");
    }

    #[test]
    fn tips_turned_off_elsewhere_drop_the_queued_one() {
        let mut tour = Tour::new(Seen::default());
        tour.offer_tip(tip("fila"));
        tour.seen.tips_switch = Some(Switch::now(true));
        assert_eq!(tour.next_surface(&FREE), None);
    }

    #[test]
    fn a_new_device_brings_the_phone_tip_back_once() {
        let devices = |ids: &[&str]| serde_json::Value::Array(ids.iter()
            .map(|id| serde_json::json!({"id": id, "kind": "ios", "first_seen": "2026-10-30T12:00:00Z"})).collect());
        let presence = || Note { id: steps::PRESENCE.to_owned(), ..tip("x") };
        let mut tour = Tour::new(Seen::default());
        assert!(!tour.devices_seen(&devices(&[])), "retrato vazio não dispara");
        assert!(!tour.devices_seen(&serde_json::json!({"id": "x"})), "formato estranho não dispara");
        assert!(tour.devices_seen(&devices(&["a"])), "o primeiro celular também dispara");
        assert!(tour.seen.devices.is_empty(), "só a resposta grava o aparelho");
        tour.offer_tip(presence());
        assert!(pick(&mut tour, Choice::TipOk));
        assert_eq!(tour.seen.devices, ["a".to_owned()].into());
        assert!(!tour.devices_seen(&devices(&["a"])), "o mesmo retrato de novo não");
        assert!(!tour.may_offer(steps::PRESENCE));
        assert!(tour.devices_seen(&devices(&["a", "b"])));
        assert!(tour.may_offer(steps::PRESENCE), "aparelho novo traz a dica de volta, mesmo já vista");
        tour.offer_tip(presence());
        assert!(pick(&mut tour, Choice::TipsOff), "Não mostrar dicas também responde");
        assert_eq!(tour.seen.devices, ["a".to_owned(), "b".to_owned()].into());
    }

    #[test]
    fn a_phone_tip_that_never_showed_comes_back_on_the_next_opening() {
        let devices = serde_json::json!([{"id": "a", "kind": "ios", "first_seen": "2026-10-30T12:00:00Z"}]);
        let mut tour = Tour::new(Seen { tips: [steps::PRESENCE.to_owned()].into(), ..Default::default() });
        assert!(tour.devices_seen(&devices));
        assert!(tour.may_offer(steps::PRESENCE));
        // O app fecha com a dica na fila, sem aparecer: o visto gravado não leva o aparelho.
        let mut reopened = Tour::new(tour.seen.clone());
        assert!(reopened.devices_seen(&devices), "o aparelho ainda é novo");
        assert!(reopened.may_offer(steps::PRESENCE));
    }

    #[test]
    fn news_shows_over_the_tour_and_closes_without_recording() {
        let mut tour = Tour::with_steps(Seen::default(), steps());
        tour.show_news(tip("novidade"));
        assert_eq!(tour.next_surface(&FREE).unwrap().kind, Kind::News);
        assert!(!pick(&mut tour, Choice::Close));
        assert_eq!(tour.next_surface(&FREE).unwrap().kind, Kind::Tour);
    }

    #[test]
    fn news_learned_by_the_action_on_its_piece_celebrates_and_leaves() {
        let mut tour = Tour::new(Seen::default());
        tour.show_news(Note { anchor: anchors::TOPBAR_SEARCH.to_owned(), ..tip("novidade") });
        assert!(!tour.tour_event(TourEvent::DictationStarted));
        assert!(tour.next_surface(&FREE).unwrap().celebrating.is_none(), "a ação de outra peça não conta");
        assert!(!tour.tour_event(TourEvent::SearchOpened), "a novidade não grava nada");
        let shown = tour.next_surface(&FREE).unwrap();
        assert_eq!(shown.kind, Kind::News);
        assert!(shown.celebrating.is_some());
        assert!(!tour.choose("novidade", Choice::Close), "comemorando, o Fechar já não vale");
        tour.news = Some((tip("novidade"), Instant::now().checked_sub(CELEBRATION)));
        assert_eq!(tour.next_surface(&FREE), None, "acabada a comemoração, o balão sai");
    }

    /// A peça de cada evento é a do passo dele: a novidade presa a ela avança com a mesma ação.
    #[test]
    fn every_event_points_at_the_piece_of_its_step() {
        use steps::{Agent, Planner, Snapshot};
        let full = Snapshot { sessions: 2, accounts: Some(2), codex: true, transcription: Some(true), open: Some(Agent::Claude) };
        let empty = Snapshot { sessions: 0, ..full };
        let all: Vec<Step> = [full, empty].iter().flat_map(|s| Planner::default().plan(s, &Default::default())).collect();
        for event in [TourEvent::SessionCreated, TourEvent::MenuOpened, TourEvent::GroupClosed, TourEvent::AccountMoved,
            TourEvent::SearchOpened, TourEvent::NumberedSession, TourEvent::ContextAdded, TourEvent::DictationStarted,
            TourEvent::PermissionCycled] {
            let step = all.iter().find(|step| step.id == event.step()).unwrap_or_else(|| panic!("{event:?} sem passo"));
            assert_eq!(event.anchor(), step.anchor, "{event:?}");
        }
    }
}
