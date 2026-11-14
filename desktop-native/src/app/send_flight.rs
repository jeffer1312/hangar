//! A mensagem enviada viaja de onde saiu (o campo ou o painel "Na fila") até a bolha dela na conversa, em vez de sumir
//! de um lugar e aparecer no outro. Para a fila ela não viaja: o painel nasce do campo já com a linha dela.
use std::{cell::Cell, rc::Rc, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use super::{Hangar, conversation_text, panes::Area};
use crate::{motion, theme};

/// Destino que não aparece dentro disto: a cópia some e a mensagem entra com o fade de sempre.
const TAKEOFF_WAIT: Duration = Duration::from_millis(600);
/// Destino anunciado e não medido dentro disto: a mensagem aparece no lugar, sem viagem.
const LANDING_WAIT: Duration = Duration::from_millis(200);
/// Depois do pouso a cópia fica mais um instante por cima da bolha: um quadro com as duas, nunca um sem nenhuma.
const OVERLAP: Duration = Duration::from_millis(50);
/// Recuo do texto dentro da bolha do usuário (`user_bubble`) e dentro da linha do painel.
const BUBBLE_PAD: (f32, f32) = (14., 10.);
const QUEUE_PAD: f32 = 4.;

/// Onde cada ponta foi desenhada pela última vez, em coordenadas da janela.
#[derive(Default)]
pub(super) struct Places {
    pub input: Cell<Option<Bounds<Pixels>>>,
    pub queue: Cell<Option<Bounds<Pixels>>>,
    pub bubble: Cell<Option<Bounds<Pixels>>>,
    /// Altura da linha da bolha, medida com ela ainda fechada: a linha abre até essa altura ao longo do voo.
    pub row: Cell<f32>,
}

/// `idle`: saiu do campo com a sessão parada, então vai direto à conversa. `shade`: quanto do fundo da bolha a cópia
/// já tinha quando o destino trocou de linha; o voo recomeça dali. `target`: a linha da conversa e desde quando.
pub(super) struct Flight { text: String, from: Point<Pixels>, left: Instant, from_queue: bool, idle: bool, shade: f32, target: Option<(String, Instant)> }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase { Waiting, Flying(f32), Landed, Over }

/// Texto simples e curto viaja; vazio, longo ou de muitas linhas entra com o fade.
pub(super) fn flies(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.chars().count() <= 400 && text.lines().count() <= 6
}

/// Em que ponto a viagem está: `left` desde a saída, `target` desde que o destino apareceu, `measured` se ele já tem
/// lugar na tela, `flight` a duração do voo.
fn phase(left: Duration, target: Option<Duration>, measured: bool, flight: Duration) -> Phase {
    match target {
        None if left < TAKEOFF_WAIT => Phase::Waiting,
        None => Phase::Over,
        Some(since) if !measured => if since < LANDING_WAIT { Phase::Waiting } else { Phase::Over },
        Some(since) if since < flight => Phase::Flying(since.as_secs_f32() / flight.as_secs_f32()),
        Some(since) if since < flight + OVERLAP => Phase::Landed,
        Some(_) => Phase::Over,
    }
}

/// Fração da altura do painel no progresso `t` (0 a 1, já com a curva) de um movimento que partiu de `from`: abrindo
/// vai até 1, fechando até 0. Inverter no meio parte de onde estava, sem salto.
pub(super) fn panel_fraction(from: f32, opening: bool, t: f32) -> f32 {
    let to = if opening { 1. } else { 0. };
    from + (to - from) * t.clamp(0., 1.)
}

fn between(from: Pixels, to: Pixels, t: f32) -> Pixels { from + (to - from) * t }

/// Grava onde o elemento pai foi desenhado; o pai precisa ser `relative`.
pub(super) fn record(places: &Rc<Places>, pick: fn(&Places) -> &Cell<Option<Bounds<Pixels>>>) -> impl IntoElement {
    let places = places.clone();
    canvas(move |bounds, _, _| pick(&places).set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
}

impl Hangar {
    /// No Enter: o texto sai do campo. `idle` diz que a sessão estava parada: a mensagem vai à conversa, não à fila.
    pub(super) fn send_takeoff(&mut self, text: &str, idle: bool, window: &Window, cx: &mut Context<Self>) {
        self.send_abort(cx);
        if cx.reduce_motion() || !window.is_window_active() || self.in_tab_preview || !flies(text) { return; }
        let Some(input) = self.flight_places.input.get() else { return };
        self.send_flight = Some(Flight { text: text.trim().to_owned(), from: input.origin, left: Instant::now(), from_queue: false, idle, shade: 0., target: None });
        // O campo esconde o texto de exemplo enquanto a cópia está por cima dele.
        self.redraw(Area::Bottom, cx);
    }

    /// No clique do Orientar: o texto sai da linha do painel, a única na tela, antes de a bolha ser desenhada.
    pub(super) fn send_takeoff_from_queue(&mut self, text: &str, cx: &mut Context<Self>) {
        self.send_abort(cx);
        let shown = self.queue.shown_text.as_deref() == Some(text.trim()) && flies(text);
        if cx.reduce_motion() || !super::chrome::window_active() || !shown { return; }
        let Some(queue) = self.flight_places.queue.get() else { return };
        let from = point(queue.origin.x + px(QUEUE_PAD), queue.origin.y);
        self.send_flight = Some(Flight { text: text.trim().to_owned(), from, left: Instant::now(), from_queue: true, idle: false, shade: 0., target: None });
        self.redraw(Area::Bottom, cx);
    }

    /// Há uma viagem esperando a bolha de `text` aparecer na conversa.
    pub(super) fn send_expects(&self, text: &str) -> bool {
        self.send_flight.as_ref()
            .is_some_and(|f| f.target.is_none() && (f.idle || f.from_queue) && f.text == text.trim() && f.left.elapsed() < TAKEOFF_WAIT)
    }

    /// A cópia que saiu do campo é este envio: a bolha apagada de "enviando" não aparece junto com ela.
    pub(super) fn send_carries(&self, text: &str) -> bool {
        self.send_flight.as_ref().is_some_and(|f| !f.from_queue && f.text == text.trim())
    }

    /// Larga a viagem em curso: a bolha ou a linha que esperava a cópia, invisível, é redesenhada e aparece.
    fn send_abort(&mut self, cx: &mut Context<Self>) {
        if self.send_flight.take().is_none() { return; }
        self.redraw(Area::Conversation, cx);
        self.redraw(Area::Bottom, cx);
    }

    /// A bolha do usuário `id` passou a aparecer na conversa: se é a mensagem que saiu do campo (com a sessão parada)
    /// ou do painel, pousa ali. Devolve se a viagem ficou com a linha, que então não entra com o fade.
    pub(super) fn send_lands_on_bubble(&mut self, id: &str, text: &str, cx: &mut Context<Self>) -> bool {
        if cx.reduce_motion() || !self.follow_pinned() || !super::chrome::window_active() { return false; }
        let text = text.trim();
        if !self.send_expects(text) {
            // Saiu do painel: a linha dele, a única na tela, é a origem.
            let shown = self.queue.shown_text.as_deref() == Some(text) && flies(text);
            let Some(queue) = self.flight_places.queue.get().filter(|_| shown) else { return false };
            let from = point(queue.origin.x + px(QUEUE_PAD), queue.origin.y);
            // Outra viagem em curso dá a vez: o que ela escondia aparece.
            self.send_abort(cx);
            self.send_flight = Some(Flight { text: text.to_owned(), from, left: Instant::now(), from_queue: true, idle: false, shade: 0., target: None });
        }
        // A linha nasce fechada e sem lugar medido: abre ao longo do voo.
        self.flight_places.bubble.set(None);
        self.flight_places.row.set(0.);
        if let Some(flight) = &mut self.send_flight { flight.target = Some((id.to_owned(), Instant::now())); }
        true
    }

    /// A linha do painel com `text` apareceu: a mensagem que saiu do campo com a sessão trabalhando chegou. A cópia
    /// some agora e a linha, que nasce do campo com o painel, leva o texto: a viagem até ela seria de poucos px.
    pub(super) fn send_reaches_queue(&mut self, text: &str) {
        if self.send_flight.as_ref().is_some_and(|f| f.target.is_none() && !f.from_queue && !f.idle && f.text == text.trim()) {
            self.send_flight = None;
        }
    }

    /// O texto que ainda não conta para o painel "Na fila": saiu do campo com a sessão parada, ou já pousa na conversa.
    /// Passado o prazo da viagem com ele ainda na fila, o painel abre como sempre.
    pub(super) fn send_skips_queue(&self) -> Option<&str> {
        self.send_flight.as_ref().filter(|f| !f.from_queue && (f.idle || f.target.is_some())).map(|f| f.text.as_str())
    }

    /// A cópia saiu do campo e ainda pode estar por cima dele: o texto de exemplo espera.
    pub(super) fn send_over_field(&self) -> bool { self.send_flight.as_ref().is_some_and(|f| !f.from_queue) }

    fn send_phase(&self) -> Option<Phase> {
        let flight = self.send_flight.as_ref()?;
        let measured = flight.target.is_some() && self.flight_places.bubble.get().is_some();
        Some(phase(flight.left.elapsed(), flight.target.as_ref().map(|(_, at)| at.elapsed()), measured, motion::FLIGHT.total()))
    }

    /// A linha da conversa em que a viagem pousa.
    pub(super) fn send_bubble_target(&self) -> Option<&str> {
        self.send_flight.as_ref()?.target.as_ref().map(|(row, _)| row.as_str())
    }

    /// A viagem pousa na bolha `id`: ela grava o próprio lugar.
    pub(super) fn send_targets_bubble(&self, id: &str) -> bool { self.send_bubble_target() == Some(id) }

    /// A linha do destino trocou de id (o envio virou a mensagem gravada): a viagem segue para a mesma bolha. A linha
    /// nova ainda não tem lugar medido: a cópia fica onde está e o voo recomeça dali até ela.
    pub(super) fn send_follows_bubble(&mut self, id: &str, text: &str) -> bool {
        let Some(flight) = self.send_flight.as_mut().filter(|f| f.text == text.trim()) else { return false };
        let Some((row, at)) = &mut flight.target else { return false };
        if let Some(old) = self.flight_places.bubble.take() {
            let t = motion::FLIGHT.ease(motion::FLIGHT.raw(*at));
            let (pad_x, pad_y) = (px(BUBBLE_PAD.0), px(BUBBLE_PAD.1));
            // `from` é onde o texto começa; o canto da cópia recua o recuo da bolha, e volta a somá-lo aqui.
            flight.from = point(between(flight.from.x - pad_x, old.origin.x, t) + pad_x, between(flight.from.y - pad_y, old.origin.y, t) + pad_y);
            flight.shade += (1. - flight.shade) * t;
        }
        (*row, *at) = (id.to_owned(), Instant::now());
        true
    }

    /// Quanto da altura a linha da bolha `id` já abriu, de 0 a 1, enquanto a cópia não chega nela; a bolha fica
    /// invisível lá dentro. `None` fora da viagem ou depois do pouso: a linha é a de sempre.
    pub(super) fn send_row_growth(&self, id: &str) -> Option<f32> {
        if !self.send_targets_bubble(id) { return None; }
        match self.send_phase()? {
            Phase::Waiting => Some(0.),
            Phase::Flying(raw) => Some(motion::FLIGHT.ease(raw)),
            Phase::Landed | Phase::Over => None,
        }
    }

    /// O texto da linha do painel fica invisível: a cópia saiu dele para a conversa.
    pub(super) fn send_hides_queue_text(&self) -> bool { self.send_flight.as_ref().is_some_and(|f| f.from_queue) }

    /// A cópia em viagem, por cima de tudo, em coordenadas da janela. Chamado no desenho da raiz, que pede os quadros.
    pub(super) fn render_send_flight(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let phase = self.send_phase()?;
        let flight = self.send_flight.as_ref()?;
        let to = flight.target.as_ref().and_then(|_| self.flight_places.bubble.get());
        let (text, start, shade, from_queue) = (SharedString::from(flight.text.clone()), flight.from, flight.shade, flight.from_queue);
        if phase == Phase::Over {
            self.send_abort(cx);
            return None;
        }
        motion::request_frame(window, cx);
        // A linha da bolha abre a cada quadro do voo: a lista a mede de novo e a conversa cede o lugar aos poucos.
        if let Some(row) = self.send_bubble_target().and_then(|id| self.row_ids.iter().position(|row| row == id)) {
            self.follow_content_changed(cx);
            self.list_state.remeasure_items(row..row + 1);
            self.redraw(Area::Conversation, cx);
        }
        // Texto novo no campo: a cópia parada por cima dele atrapalharia quem já digita a próxima.
        if to.is_none() && !from_queue && !self.composer.read(cx).value().is_empty() { return None; }
        let t = match phase { Phase::Flying(raw) => motion::FLIGHT.ease(raw), Phase::Landed => 1., _ => 0. };
        let (pad_x, pad_y) = BUBBLE_PAD;
        // O texto parte de onde estava: o canto da cópia recua o que a bolha tem de recuo.
        let from = point(start.x - px(pad_x), start.y - px(pad_y));
        let end = to.map_or(from, |to| to.origin);
        let at = point(between(from.x, end.x, t), between(from.y, end.y, t));
        let copy = conversation_text(div(), true).px(px(pad_x)).py(px(pad_y)).rounded(px(18.))
            .bg(theme::user_bubble().opacity(shade + (1. - shade) * t)).text_color(theme::text())
            .when_some(to, |el, to| el.w(to.size.width)).child(text);
        Some(deferred(anchored().position_mode(AnchoredPositionMode::Window).position(at).child(copy)).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    // Nome a nome: o `*` traria o `test` do gpui por cima do atributo padrão.
    use super::{Phase, flies, panel_fraction, phase};
    use std::time::Duration;

    const FLIGHT: Duration = Duration::from_millis(320);
    fn ms(n: u64) -> Duration { Duration::from_millis(n) }

    #[test]
    fn only_short_plain_text_flies() {
        assert!(flies("manda o print da tela de custos"));
        assert!(!flies("   "));
        assert!(!flies(&"a".repeat(401)));
        assert!(!flies("1\n2\n3\n4\n5\n6\n7"));
    }

    #[test]
    fn a_flight_never_keeps_the_message_hidden_past_its_deadlines() {
        assert_eq!(phase(ms(100), None, false, FLIGHT), Phase::Waiting);
        assert_eq!(phase(ms(600), None, false, FLIGHT), Phase::Over, "o destino não apareceu");
        assert_eq!(phase(ms(300), Some(ms(100)), false, FLIGHT), Phase::Waiting);
        assert_eq!(phase(ms(300), Some(ms(200)), false, FLIGHT), Phase::Over, "o destino não foi medido");
        assert_eq!(phase(ms(300), Some(ms(160)), true, FLIGHT), Phase::Flying(0.5));
        assert_eq!(phase(ms(900), Some(ms(330)), true, FLIGHT), Phase::Landed);
        assert_eq!(phase(ms(900), Some(ms(370)), true, FLIGHT), Phase::Over);
    }

    #[test]
    fn the_panel_reverses_from_where_it_was() {
        assert_eq!(panel_fraction(0., true, 0.25), 0.25);
        assert_eq!(panel_fraction(1., false, 0.25), 0.75);
        // Abria e estava em 30% quando a mensagem saiu da fila: fecha dali, sem ir a 100% antes.
        assert_eq!(panel_fraction(0.3, false, 0.), 0.3);
        assert_eq!(panel_fraction(0.3, false, 1.), 0.);
        // Fechava e estava em 60% quando outra chegou: reabre dali, sem voltar a 0.
        assert_eq!(panel_fraction(0.6, true, 0.), 0.6);
        assert_eq!(panel_fraction(0.6, true, 0.5), 0.8);
        assert_eq!(panel_fraction(0.6, true, 2.), 1., "passado do fim, fica no fim");
    }
}
