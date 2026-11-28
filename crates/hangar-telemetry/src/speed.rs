//! Relógio das medidas de velocidade, em memória: abertura até pronta, abertura até o primeiro recado
//! entregue e entrega até a primeira resposta. O tempo é monotônico, contado de uma âncora do processo
//! (`Duration`); quem chama conta a `Measure` devolvida.
use std::collections::HashMap;
use std::time::Duration;

use crate::events::{Dims, Event, OpenedBy, Provider, Surface};

/// Espera máxima por um sinal; passou disso a medida cai (a faixa já seria `ge30s`).
const WAIT: Duration = Duration::from_secs(600);
/// Sessão sem abertura nem mensagem do dono por mais que isto sai do relógio.
const KNOWN_FOR: Duration = Duration::from_secs(24 * 3600);
const CAP: usize = 1024;
/// O observador (o chat aberto que lê a tela) que começou depois disto da abertura não mede a abertura.
const GRACE: Duration = Duration::from_secs(10);
/// Confirmação que chega depois disto do envio é a conferência do fim do turno: a resposta já passou.
const FRESH: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq)]
pub struct Measure { pub event: Event, pub dims: Dims, pub took: Duration }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Session { pub provider: Provider, pub surface: Surface }

impl Session {
    fn dims(self) -> Dims { Dims::new().with("provider", self.provider).with("surface", self.surface) }
}

struct Open { started: Duration, opened_by: OpenedBy, ready: bool, first_send: bool }

/// `owner_sends`: marcas do texto das mensagens do dono ainda não entregues; só a entrega de uma delas arma
/// `delivered` (envio dela) para a primeira resposta.
struct Known { session: Session, seen: Duration, open: Option<Open>, owner_sends: Vec<u64>, delivered: Option<Duration>, last_text: Option<Duration> }

/// Só sessões abertas ou escritas pelo dono entram (`opened`, `touch`); sinal de outra sessão é ignorado.
#[derive(Default)]
pub struct SpeedClock(HashMap<String, Known>);

impl SpeedClock {
    /// Abertura do dono, antes do lançamento. `first_send`: aberta por agente sem prompt inicial, espera o primeiro recado.
    pub fn opened(&mut self, name: &str, session: Session, opened_by: OpenedBy, first_send: bool, started: Duration) {
        self.make_room(started);
        let open = Some(Open { started, opened_by, ready: false, first_send });
        self.0.insert(name.to_owned(), Known { session, seen: started, open, owner_sends: Vec::new(), delivered: None, last_text: None });
    }

    /// A abertura falhou.
    pub fn forget(&mut self, name: &str) { self.0.remove(name); }

    /// Mensagem do dono pela API, pela marca do texto: a entrega dela mede a primeira resposta.
    pub fn touch(&mut self, name: &str, session: Session, message: u64, now: Duration) {
        if !self.0.contains_key(name) { self.make_room(now); }
        let known = self.0.entry(name.to_owned())
            .or_insert(Known { session, seen: now, open: None, owner_sends: Vec::new(), delivered: None, last_text: None });
        known.session = session;
        known.seen = now;
        if known.owner_sends.len() >= 8 { known.owner_sends.remove(0); }
        known.owner_sends.push(message);
    }

    /// O envio dessa mensagem foi recusado: a marca dela sai.
    pub fn untouch(&mut self, name: &str, message: u64) {
        let Some(known) = self.0.get_mut(name) else { return };
        if let Some(at) = known.owner_sends.iter().rposition(|m| *m == message) { known.owner_sends.remove(at); }
    }

    pub fn knows(&self, name: &str) -> bool { self.0.contains_key(name) }

    /// Quem paga para ler a tela pergunta antes.
    pub fn awaits_ready(&self, name: &str) -> bool { self.0.get(name).and_then(|k| k.open.as_ref()).is_some_and(|open| !open.ready) }

    /// A sessão aceitou a primeira mensagem (agente de pé): `session_ready` uma vez por abertura. `since`: desde quando
    /// quem deu o sinal observava; observador que nasceu bem depois da abertura não a mede.
    pub fn ready(&mut self, name: &str, since: Duration, now: Duration) -> Option<Measure> {
        let known = self.0.get_mut(name)?;
        let open = known.open.as_mut().filter(|open| !open.ready)?;
        open.ready = true;
        let (took, opened_by, late) = (now.saturating_sub(open.started), open.opened_by, since > open.started + GRACE);
        if !open.first_send { known.open = None; }
        (took <= WAIT && !late).then(|| Measure { event: Event::SessionReady, dims: known.session.dims().with("opened_by", opened_by), took })
    }

    /// Mensagem enviada em `sent` e confirmada no agente agora. Recado de outra sessão fecha o `session_first_send`.
    /// A primeira resposta só arma com mensagem do dono, sem orientação no meio do turno, confirmada logo depois do
    /// envio e sem texto do agente desde o envio (senão a confirmação é a conferência de depois da resposta).
    pub fn delivered(&mut self, name: &str, message: u64, from_session: bool, steered: bool, sent: Duration, now: Duration) -> Option<Measure> {
        let known = self.0.get_mut(name)?;
        if let Some(at) = known.owner_sends.iter().position(|m| *m == message) {
            known.owner_sends.remove(at);
            let fresh = sent <= now && now - sent <= FRESH && known.last_text.is_none_or(|text| text < sent);
            if !steered && fresh && known.delivered.is_none_or(|at| now.saturating_sub(at) > WAIT) { known.delivered = Some(sent); }
        }
        let open = known.open.as_mut().filter(|open| open.first_send && from_session)?;
        open.first_send = false;
        let took = now.saturating_sub(open.started);
        if open.ready { known.open = None; }
        (took <= WAIT).then(|| Measure { event: Event::SessionFirstSend, dims: known.session.dims(), took })
    }

    /// Texto da resposta na tela: `first_reply` da entrega armada, uma vez. `since` como em `ready`.
    pub fn replied(&mut self, name: &str, since: Duration, now: Duration) -> Option<Measure> {
        let known = self.0.get_mut(name)?;
        known.last_text = Some(now);
        let sent = known.delivered.take()?;
        let took = now.saturating_sub(sent);
        (took <= WAIT && since <= sent).then(|| Measure { event: Event::FirstReply, dims: known.session.dims(), took })
    }

    /// O consentimento mudou: nada medido antes entra no período novo.
    pub fn clear(&mut self) { self.0.clear(); }

    // ponytail: varredura inteira só no teto; cheio de sessões vivas, recomeça vazio.
    fn make_room(&mut self, now: Duration) {
        if self.0.len() < CAP { return; }
        self.0.retain(|_, known| now.saturating_sub(known.seen) <= KNOWN_FOR);
        if self.0.len() >= CAP { self.0.clear(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: Session = Session { provider: Provider::Claude, surface: Surface::Terminal };
    const CODEX: Session = Session { provider: Provider::Codex, surface: Surface::Headless };
    const ZERO: Duration = Duration::ZERO;

    fn ms(ms: u64) -> Duration { Duration::from_millis(ms) }
    fn measure(event: Event, dims: Dims, took: u64) -> Option<Measure> { Some(Measure { event, dims, took: ms(took) }) }
    fn codex() -> Dims { Dims::new().with("provider", Provider::Codex).with("surface", Surface::Headless) }

    #[test]
    fn ready_counts_once_from_the_open_with_who_opened() {
        let mut clock = SpeedClock::default();
        assert_eq!(clock.ready("a", ZERO, ms(10)), None, "sessão que o dono não abriu não conta");
        clock.opened("a", CLAUDE, OpenedBy::Person, false, ms(1_000));
        assert!(clock.awaits_ready("a"));
        assert_eq!(clock.ready("a", ZERO, ms(3_500)), measure(Event::SessionReady,
            Dims::new().with("provider", Provider::Claude).with("surface", Surface::Terminal).with("opened_by", OpenedBy::Person), 2_500));
        assert_eq!(clock.ready("a", ZERO, ms(4_000)), None, "uma vez por abertura");
        assert!(!clock.awaits_ready("a"));
    }

    #[test]
    fn ready_after_the_wait_or_from_a_late_observer_does_not_count() {
        let mut clock = SpeedClock::default();
        clock.opened("a", CLAUDE, OpenedBy::Person, false, ms(0));
        assert_eq!(clock.ready("a", ZERO, ms(601_000)), None);
        clock.opened("b", CLAUDE, OpenedBy::Agent, false, ms(0));
        assert_eq!(clock.ready("b", ms(120_000), ms(121_000)), None, "o chat aberto 2 min depois não mede a abertura");
        assert!(!clock.awaits_ready("b"));
    }

    #[test]
    fn a_failed_open_leaves_the_clock() {
        let mut clock = SpeedClock::default();
        clock.opened("a", CODEX, OpenedBy::Person, false, ms(0));
        clock.forget("a");
        assert!(!clock.knows("a"));
    }

    #[test]
    fn first_send_waits_for_a_relay_of_another_session() {
        let mut clock = SpeedClock::default();
        clock.opened("filha", CODEX, OpenedBy::Agent, true, ms(0));
        assert!(clock.ready("filha", ZERO, ms(1_000)).is_some());
        assert_eq!(clock.delivered("filha", 1, false, false, ms(2_000), ms(2_000)), None, "mensagem da pessoa não fecha o primeiro recado");
        assert_eq!(clock.delivered("filha", 1, true, false, ms(7_000), ms(7_000)), measure(Event::SessionFirstSend, codex(), 7_000));
        assert_eq!(clock.delivered("filha", 1, true, false, ms(8_000), ms(8_000)), None, "só o primeiro recado");
    }

    #[test]
    fn first_send_only_for_who_waits_for_it() {
        let mut clock = SpeedClock::default();
        clock.opened("p", CODEX, OpenedBy::Person, false, ms(0));
        clock.opened("prompt", CODEX, OpenedBy::Agent, false, ms(0));
        assert_eq!(clock.delivered("p", 1, true, false, ms(1_000), ms(1_000)), None);
        assert_eq!(clock.delivered("prompt", 1, true, false, ms(1_000), ms(1_000)), None, "prompt inicial não passa pela fila");
    }

    #[test]
    fn first_reply_runs_from_the_owner_message_sent_to_the_first_text() {
        let mut clock = SpeedClock::default();
        clock.delivered("velha", 1, false, false, ms(0), ms(0));
        assert_eq!(clock.replied("velha", ZERO, ms(100)), None, "sessão desconhecida não mede");
        clock.touch("velha", CODEX, 1, ms(500));
        clock.delivered("velha", 1, false, false, ms(1_000), ms(1_200));
        assert_eq!(clock.replied("velha", ZERO, ms(3_000)), measure(Event::FirstReply, codex(), 2_000), "do envio ao primeiro texto");
        assert_eq!(clock.replied("velha", ZERO, ms(3_100)), None, "o resto da prévia não conta de novo");
    }

    #[test]
    fn a_delivery_that_is_not_an_owner_message_does_not_arm_the_reply() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 1, ms(0));
        clock.delivered("s", 1, false, false, ms(100), ms(200));
        clock.replied("s", ZERO, ms(300));
        clock.delivered("s", 1, false, false, ms(1_000), ms(1_100));
        assert_eq!(clock.replied("s", ZERO, ms(2_000)), None, "segunda entrega sem mensagem do dono (convidado, comando, vida anterior)");
    }

    #[test]
    fn steer_stale_and_already_answered_deliveries_do_not_arm_the_reply() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 1, ms(0));
        clock.delivered("s", 1, false, true, ms(100), ms(150));
        assert_eq!(clock.replied("s", ZERO, ms(200)), None, "orientação no meio do turno");
        clock.touch("s", CLAUDE, 1, ms(1_000));
        clock.delivered("s", 1, false, false, ms(1_000), ms(60_000));
        assert_eq!(clock.replied("s", ZERO, ms(61_000)), None, "confirmada muito depois do envio: a resposta já passou");
        clock.touch("s", CLAUDE, 1, ms(70_000));
        clock.replied("s", ZERO, ms(70_500));
        clock.delivered("s", 1, false, false, ms(70_000), ms(71_000));
        assert_eq!(clock.replied("s", ZERO, ms(72_000)), None, "texto visto depois do envio e antes da confirmação");
        clock.touch("s", CLAUDE, 1, ms(80_000));
        clock.delivered("s", 1, false, false, ms(80_000), ms(80_100));
        assert_eq!(clock.replied("s", ZERO, ms(700_000)), None, "entrega sem resposta em 10 min caiu");
    }

    #[test]
    fn only_the_owner_message_itself_arms_the_reply() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 7, ms(0));
        clock.delivered("s", 9, false, false, ms(100), ms(200));
        assert_eq!(clock.replied("s", ZERO, ms(300)), None, "linha confirmada de outra mensagem (convidado, recado) não arma");
        clock.touch("s", CLAUDE, 8, ms(1_000));
        clock.delivered("s", 8, false, false, ms(1_000), ms(1_100));
        assert!(clock.replied("s", ZERO, ms(2_000)).is_some(), "a do dono continua valendo");
    }

    #[test]
    fn a_refused_send_leaves_no_mark() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 7, ms(0));
        clock.untouch("s", 7);
        clock.delivered("s", 7, false, false, ms(100), ms(200));
        assert_eq!(clock.replied("s", ZERO, ms(300)), None);
    }

    #[test]
    fn a_send_time_after_now_is_dropped() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 1, ms(0));
        clock.delivered("s", 1, false, false, ms(5_000), ms(1_000));
        assert_eq!(clock.replied("s", ZERO, ms(2_000)), None, "relógio de parede adiantado");
    }

    #[test]
    fn a_reply_seen_by_a_late_observer_does_not_count() {
        let mut clock = SpeedClock::default();
        clock.touch("s", CLAUDE, 1, ms(0));
        clock.delivered("s", 1, false, false, ms(1_000), ms(1_100));
        assert_eq!(clock.replied("s", ms(20_000), ms(21_000)), None, "a prévia só passou a ser lida 20 s depois");
    }

    #[test]
    fn the_map_never_grows_past_its_cap() {
        let mut clock = SpeedClock::default();
        for i in 0..(CAP + 10) { clock.touch(&i.to_string(), CLAUDE, 1, ms(0)); }
        assert!(clock.0.len() <= CAP);
    }
}
