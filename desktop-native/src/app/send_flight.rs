//! A mensagem enviada viaja de onde saiu (o campo ou o painel "Na fila") até onde fica: a bolha dela na conversa ou a
//! linha dela no painel. O destino nasce no Enter, local, sem esperar o servidor: a linha do servidor toma o lugar
//! dele depois, sem outro voo.
use std::{cell::{Cell, RefCell}, collections::{HashMap, HashSet}, rc::Rc, sync::OnceLock, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use super::{Hangar, chat_text, conversation_text, panes::Area, user_bubble};
use crate::{api::dto::ChatEventExt, delivery::SessionKey, motion, queue_panel::{QueueEntry, QueuePanel}, theme};

/// A linha local da bolha de quem acabou de enviar, no fim da conversa, até a do servidor chegar.
pub(super) const LOCAL_SEND: &str = "__sending__";
/// A entrada local do painel "Na fila" de quem enviou com a sessão trabalhando, até a do servidor chegar.
pub(super) const LOCAL_QUEUE: &str = "__local__";
/// Altura fixa da nota de ação da faixa de baixo: ela entra e sai por essa altura, sem medida.
pub(super) const NOTE_HEIGHT: f32 = 28.;

/// O Orientar espera o servidor mostrar a bolha; sem ela dentro disto, a cópia some e a mensagem entra com o fade.
const TAKEOFF_WAIT: Duration = Duration::from_millis(600);
/// Destino que não é medido dentro disto: a mensagem aparece no lugar, sem viagem.
const LANDING_WAIT: Duration = Duration::from_millis(200);
/// Depois do pouso a cópia fica mais um instante por cima do destino: um quadro com os dois, nunca um sem nenhum.
const OVERLAP: Duration = Duration::from_millis(50);
/// Recuo do texto dentro da bolha do usuário (`user_bubble`) e dentro da linha do painel.
const BUBBLE_PAD: (f32, f32) = (14., 10.);
const QUEUE_PAD: f32 = 4.;
/// O servidor já respondeu ao envio e a linha dele não chegou dentro disto: o destino local sai sozinho. Antes da
/// resposta ele fica: o servidor pode levar bem mais que isto para pôr a mensagem na fila.
const SETTLE: Duration = Duration::from_secs(5);
/// O registro de um envio feito com a sessão parada que nunca passou pela fila sai depois disto.
const LOCAL_MAX: Duration = Duration::from_secs(20);
/// Quadro que demorou mais que isto parou a tela: as animações em curso não contam o tempo perdido.
const STALL: Duration = Duration::from_millis(50);
const FRAME: Duration = Duration::from_millis(16);
/// A sessão começa a trabalhar um instante antes de a fila soltar a mensagem que ela mesma vai responder: só depois
/// disto, com a mensagem ainda na fila, o turno é de outra e ela vai para o painel.
const HANDOFF: Duration = Duration::from_secs(1);
/// Quantos textos lembrar (pousados, enviados com a sessão parada).
const REMEMBERED: usize = 8;

/// Onde cada ponta foi desenhada pela última vez, em coordenadas da janela.
#[derive(Default)]
pub(super) struct Places {
    pub input: Cell<Option<Bounds<Pixels>>>,
    pub queue: Cell<Option<Bounds<Pixels>>>,
    pub bubble: Cell<Option<Bounds<Pixels>>>,
    /// Altura da linha da bolha, medida com ela ainda fechada: a linha abre até essa altura ao longo do voo.
    pub row: Cell<f32>,
    /// Só do registro: o último aviso de ação da faixa de baixo que ele escreveu.
    note: RefCell<Option<String>>,
    /// Quando a raiz desenhou o último quadro.
    frame: Cell<Option<Instant>>,
    /// As linhas que chegaram e ainda abrem a altura, com a altura inteira de cada uma, medida com ela fechada.
    opening: RefCell<HashMap<String, f32>>,
}

#[derive(Clone, Debug, PartialEq)]
enum Target { Bubble(String), Queue }

/// `target`: o destino e desde quando o voo corre; `guess`: o lugar da linha do painel pela conta das medidas fixas,
/// até ela ser medida; `seen`: só do registro, o que ele já escreveu desta viagem.
pub(super) struct Flight {
    text: String, from: Point<Pixels>, left: Instant, from_queue: bool, target: Option<(Target, Instant)>, guess: Option<Bounds<Pixels>>, seen: Cell<Seen>,
}

/// A última fase, se o destino já foi medido e se a linha da bolha está invisível, como o registro os escreveu.
#[derive(Clone, Copy, Default)]
struct Seen { phase: Option<&'static str>, measured: bool, hidden: bool }

/// O destino local de um envio: de qual conversa, o texto, desde quando, e os ids (de mensagens e de entradas da
/// fila) que já existiam no Enter. Só o que chegar depois toma o lugar dele. `sent`: quando o servidor respondeu ao
/// envio. `fresh`: só do registro, quantas entradas novas do servidor ele já descreveu.
pub(super) struct Local { key: SessionKey, text: String, since: Instant, known: HashSet<String>, sent: Option<Instant>, fresh: usize }

/// Mensagem enviada com a sessão parada: fica na conversa, nunca no painel, até ser entregue. `entry`: a entrada dela
/// na fila do servidor, quando aparece; a regra vale só para essa entrada, não para outra mensagem de mesmo texto.
pub(super) struct IdleSend { key: SessionKey, text: String, since: Instant, known: HashSet<String>, entry: Option<String>, working_since: Option<Instant> }

/// O mesmo texto, sem contar espaço e quebra de linha: o servidor pode devolver a mensagem com outro espaçamento.
pub(super) fn same_text(a: &str, b: &str) -> bool { a.split_whitespace().eq(b.split_whitespace()) }

/// O destino local espera a linha do servidor enquanto o envio não foi respondido (`sent`: há quanto tempo foi) e
/// mais um pouco depois da resposta.
pub(super) fn local_expired(sent: Option<Duration>) -> bool { sent.is_some_and(|since| since > SETTLE) }

/// Quanto as animações em curso recuam o relógio depois de um quadro que levou `gap`: tudo menos um quadro normal.
pub(super) fn stall_shift(gap: Duration) -> Duration { if gap > STALL { gap - FRAME } else { Duration::ZERO } }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase { Waiting, Flying(f32), Landed, Over }

impl Phase {
    fn name(self) -> &'static str {
        match self { Self::Waiting => "waiting", Self::Flying(_) => "flying", Self::Landed => "landed", Self::Over => "over" }
    }
}

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

/// Fração da altura de uma peça que desliza, no progresso `t` (0 a 1, já com a curva) de um movimento que partiu de
/// `from`: abrindo vai até 1, fechando até 0. Inverter no meio parte de onde estava, sem salto.
pub(super) fn panel_fraction(from: f32, opening: bool, t: f32) -> f32 {
    let to = if opening { 1. } else { 0. };
    from + (to - from) * t.clamp(0., 1.)
}

/// A mensagem enviada com a sessão parada fica na conversa enquanto a sessão não trabalha, e no instante em que ela
/// começa a trabalhar (`working_for`), que é a própria entrega. Trabalhando há mais que isso com a mensagem ainda na
/// fila, o turno é de outra: ela vai para o painel.
pub(super) fn keeps_in_chat(working_for: Option<Duration>) -> bool { working_for.is_none_or(|d| d < HANDOFF) }

/// O que fazer com a bolha que apareceu na conversa sem viagem esperando por ela.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PanelExit { Fly, NeverListed, AlreadyLanded, NotDrawnInPanel, NotPlain }

/// A bolha só voa do painel "Na fila" para a conversa se o painel do servidor já listou a entrada dela e deixou de
/// listar (`left_panel`), se ainda não pousou nesta conversa (um voo por mensagem), se a linha dela estava de fato
/// desenhada no painel (nunca de uma medida velha) e se é texto simples.
pub(super) fn panel_exit(left_panel: bool, landed: bool, drawn_in_panel: bool, plain: bool) -> PanelExit {
    if !left_panel { PanelExit::NeverListed } else if landed { PanelExit::AlreadyLanded } else if !drawn_in_panel { PanelExit::NotDrawnInPanel }
    else if !plain { PanelExit::NotPlain } else { PanelExit::Fly }
}

/// Por que a linha `queued-…` não aparece na conversa: a entrada dela está no painel do servidor, ou a linha local do
/// painel ainda a representa (o destino do envio é a fila e o servidor ainda não a listou).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Hidden { Listed, Local }

/// `listed`: o painel do servidor lista a entrada; `kept_in_chat`: foi enviada com a sessão parada; `landing`: é o
/// destino de uma viagem; `local_row`: a linha local do painel é desta mensagem.
pub(super) fn queued_row(listed: bool, kept_in_chat: bool, landing: bool, local_row: bool) -> Option<Hidden> {
    if landing || listed && kept_in_chat { None } else if listed { Some(Hidden::Listed) } else if local_row { Some(Hidden::Local) } else { None }
}

/// Quanto da altura a linha que acabou de chegar já abriu (`raw`: o tempo da abertura, de 0 a 1): só com a lista
/// colada no fim e com animação; fora disso, e depois do fim, a linha é a de sempre.
pub(super) fn row_open(raw: f32, pinned: bool, reduce_motion: bool) -> Option<f32> {
    (raw < 1. && pinned && !reduce_motion).then(|| motion::ROW_OPEN.ease(raw))
}

fn between(from: Pixels, to: Pixels, t: f32) -> Pixels { from + (to - from) * t }

/// Grava onde o elemento pai foi desenhado; o pai precisa ser `relative`.
pub(super) fn record(places: &Rc<Places>, pick: fn(&Places) -> &Cell<Option<Bounds<Pixels>>>) -> impl IntoElement {
    let places = places.clone();
    canvas(move |bounds, _, _| pick(&places).set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
}

fn tracing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HANGAR_NATIVE_FLIGHT_TRACE").is_some())
}

/// Só para diagnosticar a viagem: `HANGAR_NATIVE_FLIGHT_TRACE=1` escreve no stderr cada mudança de estado dela, com os
/// ms desde a primeira linha. Sem a variável, nada é formatado nem escrito; linha vazia não sai.
pub(super) fn trace(line: impl FnOnce() -> String) {
    if !tracing() { return; }
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let (at, line) = (ms(*EPOCH.get_or_init(Instant::now)), line());
    if !line.is_empty() { eprintln!("flight {at:>9.1} {line}"); }
}

/// O aviso de ação da faixa de baixo apareceu, mudou ou sumiu; só o começo do texto, que é da interface.
pub(super) fn trace_note(places: &Places, note: Option<&str>) {
    if !tracing() || places.note.borrow().as_deref() == note { return; }
    trace(|| match note {
        Some(text) => format!("bottom note shown: {}", text.chars().take(60).collect::<String>()),
        None => "bottom note gone".into(),
    });
    *places.note.borrow_mut() = note.map(str::to_owned);
}

fn ms(since: Instant) -> f64 { since.elapsed().as_secs_f64() * 1000. }

/// Id de linha no registro, curto.
fn short(id: &str) -> String { id.chars().take(22).collect() }

fn xy(at: Point<Pixels>) -> String { format!("({:.0},{:.0})", f32::from(at.x), f32::from(at.y)) }

fn remember<T>(list: &mut Vec<T>, item: T) {
    list.push(item);
    if list.len() > REMEMBERED { list.remove(0); }
}

/// Uma peça da faixa de baixo que abre e fecha pela altura. A altura cheia vem de uma conta de quem desenha, nunca de
/// medida: a raiz soma a mudança deste quadro à altura da faixa (que é a do quadro anterior) e a conversa acompanha no
/// mesmo quadro.
#[derive(Default)]
pub(super) struct Slide {
    /// Falso na conversa recém-aberta: o primeiro desenho adota o estado sem animar.
    settled: bool,
    open: bool,
    /// A fração de onde partiu, se abre, e desde quando.
    moving: Option<(f32, bool, Instant)>,
    /// A altura com que foi desenhada no último quadro, e a que a raiz já contou para este.
    drawn: Option<f32>,
    planned: Option<f32>,
}

impl Slide {
    pub(super) fn reset(&mut self) { *self = Self::default(); }

    /// O deslize em curso começa `by` mais tarde, nunca depois de `now`.
    fn delay(&mut self, by: Duration, now: Instant) -> bool {
        let Some((_, _, at)) = self.moving.as_mut() else { return false };
        *at = (*at + by).min(now);
        true
    }

    fn fraction(&self) -> f32 {
        match self.moving {
            Some((from, opening, at)) => panel_fraction(from, opening, motion::RESIZE.ease(motion::RESIZE.raw(at))),
            None => if self.open { 1. } else { 0. },
        }
    }

    fn sliding(&self, full: f32) -> Option<f32> {
        self.moving.is_some_and(|(_, _, at)| motion::RESIZE.raw(at) < 1.).then(|| full * self.fraction())
    }

    /// Na raiz: quanto a altura muda do último quadro para este.
    pub(super) fn step(&mut self, full: f32) -> f32 {
        self.planned = self.sliding(full);
        match (self.drawn, self.planned) { (Some(before), Some(now)) => now - before, _ => 0. }
    }

    /// No desenho: `live` diz se a peça existe agora. Devolve a altura deste quadro enquanto ela desliza (a que a raiz
    /// contou, quando ela contou); `None` com ela parada, inteira ou ausente.
    pub(super) fn height(&mut self, live: bool, full: f32, animate: bool) -> Option<f32> {
        let planned = self.planned.take();
        if !self.settled { (self.settled, self.open, self.moving) = (true, live, None); }
        if live != self.open {
            // Inverter no meio parte da fração em que estava.
            let from = self.fraction();
            (self.open, self.moving) = (live, animate.then(|| (from, live, Instant::now())));
        }
        if self.moving.is_some_and(|(_, _, at)| motion::RESIZE.raw(at) >= 1.) { self.moving = None; }
        self.drawn = self.sliding(full).map(|now| planned.unwrap_or(now));
        self.drawn
    }
}

impl Hangar {
    /// No Enter: o destino nasce aqui, sem esperar o servidor. `to_queue`: a sessão trabalha e a mensagem vai para o
    /// painel "Na fila", que ganha a linha local dela; senão ela ganha a bolha local no fim da conversa. `idle`: a
    /// sessão estava parada, e a mensagem não aparece no painel enquanto não for entregue. `known`: as mensagens que
    /// já existiam. A cópia voa até o destino local na hora, se há o que animar.
    pub(super) fn send_takeoff(&mut self, text: &str, idle: bool, to_queue: bool, mut known: HashSet<String>, window: &Window, cx: &mut Context<Self>) {
        self.send_abort("new enter", cx);
        let text = text.trim();
        let Some(key) = self.selected_key() else { return };
        if text.is_empty() { return; }
        // Mensagem nova, mesmo com o texto de uma que já pousou, tem o voo dela.
        self.send_landed.retain(|landed| landed != text);
        known.extend(self.chat.queue_panel().entries.iter().map(|e| e.id.clone()));
        if idle && !to_queue {
            remember(&mut self.idle_sends, IdleSend { key: key.clone(), text: text.to_owned(), since: Instant::now(), known: known.clone(), entry: None, working_since: None });
        }
        trace(|| format!("local destination born: {} chars={} idle={idle}", if to_queue { "queue panel row" } else { "bubble at the end of the conversation" },
            text.chars().count()));
        let local = Some(Local { key, text: text.to_owned(), since: Instant::now(), known, sent: None, fresh: 0 });
        if to_queue { self.local_queue = local; } else { self.local_send = local; }
        let input = self.flight_places.input.get();
        if cx.reduce_motion() || !window.is_window_active() || self.in_tab_preview || !flies(text) || input.is_none() {
            trace(|| format!("takeoff refused source=field reduce_motion={} window_active={} tab_preview={} flies={} field_measured={}",
                cx.reduce_motion(), window.is_window_active(), self.in_tab_preview, flies(text), input.is_some()));
        } else if let Some(input) = input {
            let mut guess = None;
            let target = if to_queue {
                // O lugar da linha sai da conta das medidas do painel: a cópia parte já, sem esperar o desenho dele.
                self.flight_places.queue.set(None);
                guess = super::queue::row_place();
                Target::Queue
            } else {
                // A linha nasce fechada e sem lugar medido: abre ao longo do voo.
                self.flight_places.bubble.set(None);
                self.flight_places.row.set(0.);
                Target::Bubble(LOCAL_SEND.into())
            };
            trace(|| format!("takeoff source=field origin={} idle={idle} chars={} target={target:?} place_by_fixed_sizes={}", xy(input.origin),
                text.chars().count(), guess.map_or("none".into(), |place| xy(place.origin))));
            self.send_flight = Some(Flight { text: text.to_owned(), from: input.origin, left: Instant::now(), from_queue: false,
                target: Some((target, Instant::now())), guess, seen: Cell::default() });
        }
        if !to_queue {
            self.sync_tail_rows(cx);
            // A bolha em que a cópia pousa abre com o voo; sem voo, entra com o fade de qualquer linha nova.
            if self.send_flight.is_some() { self.arrived.remove(LOCAL_SEND); }
            self.redraw(Area::Conversation, cx);
        }
        // O campo esconde o texto de exemplo enquanto a cópia está por cima dele, e o painel ganha a linha local.
        self.redraw(Area::Bottom, cx);
    }

    /// No clique do Orientar: o texto sai da linha do painel, a única na tela, antes de a bolha ser desenhada.
    pub(super) fn send_takeoff_from_queue(&mut self, text: &str, cx: &mut Context<Self>) {
        self.send_abort("steer click", cx);
        let shown = self.queue.shown_text.as_deref() == Some(text.trim()) && flies(text);
        if cx.reduce_motion() || !super::chrome::window_active() || !shown {
            trace(|| format!("takeoff refused source=panel(click) reduce_motion={} window_active={} only_row_with_this_text={shown} chars={}",
                cx.reduce_motion(), super::chrome::window_active(), text.trim().chars().count()));
            return;
        }
        let Some(queue) = self.flight_places.queue.get() else {
            trace(|| "takeoff refused source=panel(click): panel row place not measured".into());
            return;
        };
        let from = point(queue.origin.x + px(QUEUE_PAD), queue.origin.y);
        trace(|| format!("takeoff source=panel(click) origin={} chars={} target=unset, waits for the bubble", xy(from), text.trim().chars().count()));
        self.send_flight = Some(Flight { text: text.trim().to_owned(), from, left: Instant::now(), from_queue: true, target: None, guess: None, seen: Cell::default() });
        self.redraw(Area::Bottom, cx);
    }

    /// O servidor respondeu ao envio. `delivered`: a mensagem foi entregue direto, sem passar pela fila, e a linha
    /// local do painel sai na hora. Senão o destino local ganha o prazo para a linha do servidor chegar, e a tela
    /// confere de novo quando ele vence.
    pub(super) fn send_settled(&mut self, key: &SessionKey, text: &str, delivered: bool, cx: &mut Context<Self>) {
        let same = |local: &Option<Local>| local.as_ref().is_some_and(|l| l.key == *key && same_text(&l.text, text));
        if !same(&self.local_send) && !same(&self.local_queue) { return; }
        trace(|| format!("send answered by the server: delivered={delivered} bubble_local={} queue_row_local={}", same(&self.local_send), same(&self.local_queue)));
        if same(&self.local_queue) && delivered { self.local_queue_released("the message was delivered without waiting in the queue", cx); }
        for local in [&mut self.local_send, &mut self.local_queue].into_iter().flatten().filter(|l| l.key == *key && same_text(&l.text, text)) {
            local.sent = Some(Instant::now());
        }
        self.redraw(Area::Bottom, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE + FRAME).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.local_send.is_none() && this.local_queue.is_none() { return; }
                    // A entrada do servidor que chegou nesse meio é a troca normal, feita pelo painel.
                    let listed = this.chat.queue_panel().entries.iter().any(|e| this.local_queue.as_ref().is_some_and(|l| !l.known.contains(&e.id)));
                    if !listed && this.local_queue.as_ref().is_some_and(|l| local_expired(l.sent.map(|at| at.elapsed()))) {
                        this.local_queue_released("the server answered and no entry came", cx);
                    }
                    this.sync_tail_rows(cx);
                    this.redraw(Area::Conversation, cx);
                    this.redraw(Area::Bottom, cx);
                });
            }
        }).detach();
    }

    /// Quanto da altura a linha `id`, que acabou de chegar, já abriu; `None` quando ela é a linha de sempre.
    fn row_opening(&self, id: &str, cx: &App) -> Option<f32> {
        row_open(motion::ROW_OPEN.raw(*self.arrived.get(id)?), self.follow_pinned(), cx.reduce_motion())
    }

    /// A linha `id` entrou agora num lugar que não existia (não tomou o de outra que saiu): além do fade, abre a altura.
    pub(super) fn row_opens(&mut self, id: &str, cx: &App) {
        if self.row_opening(id, cx).is_none() { return; }
        trace(|| format!("arrived row {} opens its height from 0 (list at the end)", short(id)));
        self.flight_places.opening.borrow_mut().insert(id.to_owned(), 0.);
    }

    /// No desenho da linha que chegou: ela abre de altura 0 até a dela no começo do fade, em vez de entrar inteira e
    /// empurrar a conversa num quadro só. A altura inteira é medida com a linha ainda fechada.
    pub(super) fn open_arriving_row(&self, id: &str, row: Div, cx: &App) -> Div {
        let places = self.flight_places.clone();
        let Some(full) = places.opening.borrow().get(id).copied() else { return row };
        let Some(open) = self.row_opening(id, cx) else { return row };
        let id = id.to_owned();
        div().relative().overflow_hidden().h(px(full * open))
            .child(div().absolute().top_0().left_0().right_0().child(row)
                .child(canvas(move |bounds, _, _| {
                    if let Some(full) = places.opening.borrow_mut().get_mut(&id) { *full = f32::from(bounds.size.height); }
                }, |_, _, _, _| {}).absolute().inset_0()))
    }

    /// Na raiz, a cada quadro: a lista mede de novo as linhas que estão abrindo e a conversa cede o lugar aos poucos. A
    /// que acabou de abrir é medida mais uma vez, inteira, e sai da conta: com a conversa parada isto não faz nada.
    pub(super) fn open_arriving_rows(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.flight_places.opening.borrow().keys().cloned().collect();
        if ids.is_empty() { return; }
        self.follow_content_changed(cx);
        for id in ids {
            if self.row_opening(&id, cx).is_none() {
                let full = self.flight_places.opening.borrow_mut().remove(&id);
                trace(|| format!("arrived row {} open: height {:.0}", short(&id), full.unwrap_or(0.)));
            }
            if let Some(row) = self.row_ids.iter().position(|row| *row == id) { self.list_state.remeasure_items(row..row + 1); }
        }
        self.redraw(Area::Conversation, cx);
    }

    /// Na raiz, a cada quadro: um quadro que demorou (a tela parada) não come a animação. O painel, a nota e a viagem
    /// em curso recuam o relógio pelo tempo perdido e seguem de onde estavam.
    pub(super) fn unstall(&mut self) {
        let now = Instant::now();
        let Some(gap) = self.flight_places.frame.replace(Some(now)).map(|last| now - last) else { return };
        let lost = stall_shift(gap);
        if lost.is_zero() { return; }
        let mut moving = self.queue.delay(lost, now) | self.note_slide.delay(lost, now);
        if let Some(flight) = &mut self.send_flight {
            flight.left = (flight.left + lost).min(now);
            if let Some((_, at)) = &mut flight.target { *at = (*at + lost).min(now); }
            moving = true;
        }
        if moving { trace(|| format!("frame took {:.0}ms: what was moving goes on from where it was", gap.as_secs_f64() * 1000.)); }
    }

    /// O envio falhou: o destino local some (o texto volta ao campo por quem trata a falha).
    pub(super) fn send_failed(&mut self, key: &SessionKey, text: &str, cx: &mut Context<Self>) {
        let same = |local: &Option<Local>| local.as_ref().is_some_and(|l| l.key == *key && l.text == text.trim());
        if !same(&self.local_send) && !same(&self.local_queue) { return; }
        trace(|| "local destination dropped: the send failed".into());
        if same(&self.local_send) { self.local_send = None; }
        if same(&self.local_queue) { self.local_queue = None; }
        self.idle_sends.retain(|send| !(send.key == *key && send.text == text.trim()));
        self.send_abort("send failed", cx);
        self.sync_tail_rows(cx);
        self.redraw(Area::Conversation, cx);
        self.redraw(Area::Bottom, cx);
    }

    /// A bolha local aparece no fim da conversa aberta até a linha do servidor (a `queued-…` ou a gravada) com o mesmo
    /// texto estar na tela: aí ela sai na mesma troca de linhas, e a outra toma o lugar.
    pub(super) fn local_send_shown(&mut self) -> bool {
        let Some(local) = &self.local_send else { return false };
        let current = self.selected_key().as_ref() == Some(&local.key);
        let panel = self.chat.queue_panel();
        let arrived = current.then(|| self.chat.events.iter().rev().find(|e| e.kind == "user_msg" && !local.known.contains(&e.id)
            && e.text.as_deref().is_some_and(|text| same_text(text, &local.text)) && !self.queue_hides(&e.id, &panel)).map(|e| e.id.clone())).flatten();
        let expired = local_expired(local.sent.map(|at| at.elapsed()));
        drop(panel);
        if arrived.is_none() && !expired { return current; }
        trace(|| match &arrived {
            Some(id) => format!("local bubble replaced by the server row {} (no flight, no fade)", short(id)),
            None => "local bubble dropped: no server row within the limit".into(),
        });
        self.local_send = None;
        false
    }

    /// O texto da linha local do painel, enquanto a entrada do servidor não chegou (`server`: as entradas dele agora)
    /// nem a mensagem foi direto para a conversa.
    pub(super) fn local_queue_text(&mut self, server: &[QueueEntry]) -> Option<String> {
        let key = self.selected_key();
        let local = self.local_queue.as_mut()?;
        if key.as_ref() != Some(&local.key) { return None; }
        // As entradas que o servidor pôs na fila depois do Enter: uma delas é esta mensagem.
        let fresh: Vec<&QueueEntry> = server.iter().filter(|e| !local.known.contains(&e.id)).collect();
        let age = local.since.elapsed();
        // Pelo texto; com o envio já respondido e uma entrada nova só, é ela, mesmo que o servidor tenha mudado o texto.
        let in_panel = fresh.iter().find(|e| same_text(&e.text, &local.text))
            .or_else(|| fresh.first().filter(|_| fresh.len() == 1 && local.sent.is_some())).map(|e| e.id.clone());
        let in_chat = self.chat.events.iter().rev().any(|e| e.kind == "user_msg" && !e.queued() && !local.known.contains(&e.id)
            && e.text.as_deref().is_some_and(|text| same_text(text, &local.text)));
        // O prazo depois da resposta é conferido fora do desenho (`send_settled`), que refaz as linhas da conversa.
        if in_panel.is_none() && !in_chat {
            if fresh.len() != local.fresh {
                local.fresh = fresh.len();
                trace(|| format!("local queue row kept after={:.0}ms: server entries={} new since enter={:?} (none has this text, chars={})",
                    age.as_secs_f64() * 1000., server.len(), fresh.iter().map(|e| format!("{} chars={}", short(&e.id), e.text.trim().chars().count())).collect::<Vec<_>>(),
                    local.text.chars().count()));
            }
            return Some(local.text.clone());
        }
        trace(|| match &in_panel {
            Some(id) => format!("local queue row replaced by the server entry {} after={:.0}ms (no flight; it brings its actions)", short(id), age.as_secs_f64() * 1000.),
            None => "local queue row dropped: the message went straight to the conversation".into(),
        });
        self.local_queue = None;
        None
    }

    /// Há um destino local mostrando este envio: a bolha apagada de "enviando" da faixa de baixo não aparece junto.
    pub(super) fn send_carries(&self, text: &str) -> bool {
        let key = self.selected_key();
        [&self.local_send, &self.local_queue].into_iter().flatten().any(|local| Some(&local.key) == key.as_ref() && local.text == text.trim())
    }

    /// Acerta cada envio feito com a sessão parada: qual entrada da fila é a dele, desde quando a sessão trabalha com
    /// ela ainda lá, e se já acabou (a entrada saiu da fila). Quando a sessão começa a trabalhar, marca a hora de
    /// conferir de novo: passado o instante da própria entrega, a mensagem que continuar na fila vai para o painel.
    pub(super) fn idle_tick(&mut self, cx: &mut Context<Self>) {
        if self.idle_sends.is_empty() { return; }
        let Some(key) = self.selected_key() else { return };
        let (working, panel) = (self.chat.state.state == "working", self.chat.queue_panel());
        let mut handoff = false;
        self.idle_sends.retain_mut(|send| {
            if send.key != key { return true; }
            if let Some(id) = &send.entry {
                if !panel.entries.iter().any(|e| e.id == *id) {
                    trace(|| format!("idle send done: its entry {} left the server queue", short(id)));
                    return false;
                }
            } else {
                let entry = panel.entries.iter().find(|e| !send.known.contains(&e.id) && same_text(&e.text, &send.text)).map(|e| e.id.clone());
                match &entry {
                    Some(id) => trace(|| format!("idle send is the server entry {}: it stays in the conversation, out of the queue panel", short(id))),
                    // Nunca passou pela fila: foi entregue direto.
                    None if send.since.elapsed() > LOCAL_MAX => return false,
                    None => {}
                }
                send.entry = entry;
            }
            if working && send.working_since.is_none() { handoff |= send.entry.is_some(); }
            send.working_since = if working { send.working_since.or_else(|| Some(Instant::now())) } else { None };
            true
        });
        drop(panel);
        if !handoff { return; }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HANDOFF).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    // Quem continua na fila com a sessão trabalhando passa da conversa para o painel: as linhas são
                    // refeitas inteiras, porque nenhum evento mudou.
                    trace(|| "idle send handoff check: what is still queued while the session works goes to the queue panel".into());
                    this.chat.invalidate();
                    this.sync_rows(cx);
                    this.redraw(Area::Bottom, cx);
                    cx.notify();
                });
            }
        }).detach();
    }

    /// A entrada `id` da fila é de uma mensagem enviada com a sessão parada e ainda por entregar: fica na conversa,
    /// fora do painel "Na fila".
    pub(super) fn send_keeps_in_chat(&self, id: &str) -> bool {
        let Some(key) = self.selected_key() else { return false };
        self.idle_sends.iter().any(|send| send.key == key && send.entry.as_deref() == Some(id) && keeps_in_chat(send.working_since.map(|at| at.elapsed())))
    }

    /// A linha `queued-…` some da conversa enquanto a entrada dela está no painel; a que foi enviada com a sessão
    /// parada, e a que é destino de uma viagem, continuam na conversa.
    pub(super) fn queue_hides(&self, id: &str, panel: &QueuePanel) -> bool { self.queue_hidden(id, panel).is_some() }

    /// Se a linha `queued-…` fica fora da conversa, e por quê. Enviada com a sessão trabalhando, ela fica escondida
    /// desde que chega: a linha local do painel a representa até o servidor listar a entrada.
    pub(super) fn queue_hidden(&self, id: &str, panel: &QueuePanel) -> Option<Hidden> {
        let raw = id.strip_prefix("queued-")?;
        let listed = panel.entries.iter().any(|e| e.id == raw);
        let local_row = !listed && self.local_queue.as_ref().is_some_and(|local| self.selected_key().as_ref() == Some(&local.key)
            && !local.known.contains(id) && !local.known.contains(raw)
            && self.chat.position(id).and_then(|i| self.chat.events[i].text.as_deref()).is_some_and(|text| same_text(text, &local.text)));
        queued_row(listed, listed && self.send_keeps_in_chat(raw), self.send_targets_bubble(id), local_row)
    }

    /// A linha local do painel saiu sem a entrada do servidor: a `queued-…` que ela segurava volta à conversa, e as
    /// linhas são refeitas inteiras, porque nenhum evento mudou.
    fn local_queue_released(&mut self, why: &str, cx: &mut Context<Self>) {
        if self.local_queue.take().is_none() { return; }
        trace(|| format!("local queue row dropped: {why}"));
        self.chat.invalidate();
        self.sync_rows(cx);
        self.redraw(Area::Conversation, cx);
        self.redraw(Area::Bottom, cx);
    }

    /// Há uma viagem (a do Orientar) esperando a bolha de `text` aparecer na conversa.
    pub(super) fn send_expects(&self, text: &str) -> bool {
        self.send_flight.as_ref().is_some_and(|f| f.target.is_none() && f.from_queue && f.text == text.trim() && f.left.elapsed() < TAKEOFF_WAIT)
    }

    /// Larga a viagem em curso: a bolha ou a linha que esperava a cópia, invisível, é redesenhada e aparece.
    fn send_abort(&mut self, why: &str, cx: &mut Context<Self>) {
        let Some(flight) = self.send_flight.take() else { return };
        trace(|| format!("abort why={why} after={:.0}ms source={} target={} row_was_invisible={}", ms(flight.left),
            if flight.from_queue { "panel" } else { "field" }, flight.target.as_ref().map_or("unset".into(), |(target, _)| format!("{target:?}")),
            flight.seen.get().hidden));
        self.redraw(Area::Conversation, cx);
        self.redraw(Area::Bottom, cx);
    }

    /// A bolha do usuário `id` passou a aparecer na conversa sem ser o destino de ninguém: pousa nela a viagem do
    /// Orientar que a esperava, ou a que sai agora do painel "Na fila" (a mensagem deixou a fila sozinha). Devolve se
    /// a viagem ficou com a linha, que então não entra com o fade.
    pub(super) fn send_lands_on_bubble(&mut self, id: &str, text: &str, left_panel: bool, cx: &mut Context<Self>) -> bool {
        if cx.reduce_motion() || !self.follow_pinned() || !super::chrome::window_active() {
            trace(|| format!("bubble {} appeared, no flight: reduce_motion={} list_pinned={} window_active={}", short(id),
                cx.reduce_motion(), self.follow_pinned(), super::chrome::window_active()));
            return false;
        }
        let text = text.trim();
        if !self.send_expects(text) {
            let queue = self.flight_places.queue.get().filter(|_| self.queue.shown_text.as_deref() == Some(text));
            let exit = panel_exit(left_panel, self.send_landed.iter().any(|landed| landed == text), queue.is_some(), flies(text));
            let Some(queue) = queue.filter(|_| exit == PanelExit::Fly) else {
                trace(|| format!("bubble {} appeared, no flight: {exit:?} (flight in course={})", short(id),
                    self.send_flight.as_ref().map_or("none".into(), |f| format!("{:?}", f.target.as_ref().map(|(target, _)| target)))));
                return false;
            };
            let from = point(queue.origin.x + px(QUEUE_PAD), queue.origin.y);
            // Outra viagem em curso dá a vez: o que ela escondia aparece.
            self.send_abort("panel flight takes over", cx);
            trace(|| format!("takeoff source=panel(left by itself) origin={} chars={}", xy(from), text.chars().count()));
            self.send_flight = Some(Flight { text: text.to_owned(), from, left: Instant::now(), from_queue: true, target: None, guess: None, seen: Cell::default() });
        }
        // A linha nasce fechada e sem lugar medido: abre ao longo do voo.
        self.flight_places.bubble.set(None);
        self.flight_places.row.set(0.);
        if let Some(flight) = &mut self.send_flight {
            trace(|| format!("target set: bubble {} after={:.0}ms source={}", short(id), ms(flight.left), if flight.from_queue { "panel" } else { "field" }));
            flight.target = Some((Target::Bubble(id.to_owned()), Instant::now()));
        }
        true
    }

    fn send_target_place(&self) -> Option<Bounds<Pixels>> {
        let flight = self.send_flight.as_ref()?;
        match &flight.target.as_ref()?.0 {
            Target::Bubble(_) => self.flight_places.bubble.get(),
            // A medida, quando a linha já foi desenhada; antes, a conta pelas medidas fixas do painel.
            Target::Queue => self.flight_places.queue.get().or(flight.guess),
        }
    }

    fn send_phase(&self) -> Option<Phase> {
        let flight = self.send_flight.as_ref()?;
        Some(phase(flight.left.elapsed(), flight.target.as_ref().map(|(_, at)| at.elapsed()), self.send_target_place().is_some(), motion::FLIGHT.total()))
    }

    /// A linha da conversa em que a viagem pousa.
    pub(super) fn send_bubble_target(&self) -> Option<&str> {
        match &self.send_flight.as_ref()?.target { Some((Target::Bubble(row), _)) => Some(row), _ => None }
    }

    /// A viagem pousa na bolha `id`: ela grava o próprio lugar.
    pub(super) fn send_targets_bubble(&self, id: &str) -> bool { self.send_bubble_target() == Some(id) }

    /// A linha do destino trocou de id (a bolha local virou a `queued-…`, ou esta a mensagem gravada): a viagem segue
    /// para a mesma bolha. Só o destino muda: a origem, o tempo já passado e o último lugar medido ficam, e a linha
    /// nova grava o dela no primeiro desenho. Recomeçar o voo aqui faria a cópia atravessar a tela de novo.
    pub(super) fn send_follows_bubble(&mut self, id: &str, text: &str) -> bool {
        let Some(flight) = self.send_flight.as_mut().filter(|f| f.text == text.trim()) else {
            trace(|| format!("target row gone, candidate {} refused: no flight with this text", short(id)));
            return false;
        };
        let Some((Target::Bubble(row), at)) = &mut flight.target else { return false };
        let (raw, left) = (motion::FLIGHT.raw(*at), flight.left);
        trace(|| format!("target swapped: {} -> {} flight_fraction={raw:.2} after={:.0}ms (same flight goes on, nothing restarts)", short(row), short(id), ms(left)));
        *row = id.to_owned();
        true
    }

    /// Quanto da altura a linha da bolha `id` já abriu, de 0 a 1, enquanto a cópia não chega nela; a bolha fica
    /// invisível lá dentro. `None` fora da viagem ou depois do pouso: a linha é a de sempre.
    pub(super) fn send_row_growth(&self, id: &str) -> Option<f32> {
        if !self.send_targets_bubble(id) { return None; }
        let growth = match self.send_phase()? {
            Phase::Waiting => Some(0.),
            Phase::Flying(raw) => Some(motion::FLIGHT.ease(raw)),
            Phase::Landed | Phase::Over => None,
        };
        if let Some(flight) = self.send_flight.as_ref().filter(|f| f.seen.get().hidden != growth.is_some()) {
            flight.seen.set(Seen { hidden: growth.is_some(), ..flight.seen.get() });
            trace(|| format!("row {} drawn {} after={:.0}ms row_height={:.0}", short(id),
                if growth.is_some() { "invisible (closed, opening with the flight)" } else { "visible" }, ms(flight.left), self.flight_places.row.get()));
        }
        growth
    }

    /// O texto da linha `id` do painel fica invisível: a cópia ainda não chegou na linha local, ou já saiu da linha
    /// (a única do painel, `single`) para a conversa.
    pub(super) fn send_hides_queue_text(&self, id: &str, single: bool) -> bool {
        let Some(flight) = &self.send_flight else { return false };
        let arriving = id == LOCAL_QUEUE && matches!(flight.target, Some((Target::Queue, _)))
            && matches!(self.send_phase(), Some(Phase::Waiting | Phase::Flying(_)));
        arriving || flight.from_queue && single
    }

    /// A cópia saiu do campo e ainda pode estar por cima dele: o texto de exemplo espera.
    pub(super) fn send_over_field(&self) -> bool { self.send_flight.as_ref().is_some_and(|f| !f.from_queue) }

    /// A bolha local de quem acabou de enviar: a mesma caixa da bolha gravada, para a troca não mudar a altura.
    pub(super) fn render_local_send(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.local_send.as_ref().map(|local| super::safe_markdown(&local.text)).unwrap_or_default();
        let view = self.text_view(LOCAL_SEND, LOCAL_SEND, text, cx);
        let landing = self.send_targets_bubble(LOCAL_SEND).then(|| self.flight_places.clone());
        div().w_full().flex().flex_col().gap_2().items_end()
            .child(user_bubble(conversation_text(div().flex().flex_col().gap_2(), true).child(chat_text(&view, cx)))
                // A bolha em que a mensagem enviada pousa diz onde está.
                .when_some(landing, |el, places| el.relative().child(record(&places, |p| &p.bubble))))
            // A faixa de hora e copiar da linha gravada, vazia: a altura não muda quando ela chega.
            .child(div().h(px(24.)))
            .into_any_element()
    }

    /// A cópia em viagem, por cima de tudo, em coordenadas da janela. Chamado no desenho da raiz, que pede os quadros.
    pub(super) fn render_send_flight(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let phase = self.send_phase()?;
        let to = self.send_target_place();
        let flight = self.send_flight.as_ref()?;
        let to_queue = matches!(flight.target, Some((Target::Queue, _)));
        let (text, start, from_queue) = (SharedString::from(flight.text.clone()), flight.from, flight.from_queue);
        // Só registro: a fase que mudou e o destino medido pela primeira vez.
        let mut seen = flight.seen.get();
        if seen.phase != Some(phase.name()) {
            trace(|| format!("phase {} -> {} after={:.0}ms since_target={} target={}", seen.phase.unwrap_or("none"), phase.name(), ms(flight.left),
                flight.target.as_ref().map_or("unset".into(), |(_, at)| format!("{:.0}ms", ms(*at))),
                flight.target.as_ref().map_or("unset".into(), |(target, _)| format!("{target:?}"))));
            seen.phase = Some(phase.name());
        }
        // A linha do painel posta pela conta das medidas fixas (escrita na saída) ainda escreve a medida quando ela chega.
        if let (Some(to), false) = (to.filter(|_| !to_queue || self.flight_places.queue.get().is_some()), seen.measured) {
            trace(|| format!("target measured: {} at {} size {:.0}x{:.0} after={:.0}ms copy_origin={}",
                flight.target.as_ref().map_or("unset".into(), |(target, _)| format!("{target:?}")), xy(to.origin), f32::from(to.size.width),
                f32::from(to.size.height), ms(flight.left), xy(start)));
            seen.measured = true;
        }
        flight.seen.set(seen);
        let over = match (&flight.target, to) {
            (None, _) => "deadline: the destination did not appear",
            (Some(_), None) => "deadline: the destination was not measured",
            (Some(_), Some(_)) => "landed",
        };
        if phase == Phase::Over {
            // Pousou: esta mensagem não voa de novo, mesmo que a linha dela troque de id ou volte do painel.
            if over == "landed" && !to_queue { remember(&mut self.send_landed, text.to_string()); }
            self.send_abort(over, cx);
            return None;
        }
        motion::request_frame(window, cx);
        // A linha da bolha abre a cada quadro do voo: a lista a mede de novo e a conversa cede o lugar aos poucos.
        if let Some(row) = self.send_bubble_target().and_then(|id| self.row_ids.iter().position(|row| row == id)) {
            self.follow_content_changed(cx);
            self.list_state.remeasure_items(row..row + 1);
            self.redraw(Area::Conversation, cx);
        }
        if to_queue && phase == Phase::Landed { self.redraw(Area::Bottom, cx); }
        // Texto novo no campo: a cópia parada por cima dele atrapalharia quem já digita a próxima.
        if to.is_none() && !from_queue && !self.composer.read(cx).value().is_empty() { return None; }
        let t = match phase { Phase::Flying(raw) => motion::FLIGHT.ease(raw), Phase::Landed => 1., _ => 0. };
        let (pad_x, pad_y) = if to_queue { (QUEUE_PAD, 0.) } else { BUBBLE_PAD };
        // O texto parte de onde estava: o canto da cópia recua o que o destino tem de recuo.
        let from = point(start.x - px(pad_x), start.y - px(pad_y));
        let end = to.map_or(from, |to| to.origin);
        let at = point(between(from.x, end.x, t), between(from.y, end.y, t));
        let copy = if to_queue {
            // Do tamanho do texto do campo ao da linha do painel, ao longo do caminho.
            div().px(px(pad_x)).text_size(px(16. - 3.5 * t)).text_color(theme::text()).truncate()
                .when_some(to, |el, to| el.w(to.size.width)).child(text)
        } else {
            conversation_text(div(), true).px(px(pad_x)).py(px(pad_y)).rounded(px(18.))
                .bg(theme::user_bubble().opacity(t)).text_color(theme::text())
                .when_some(to, |el, to| el.w(to.size.width)).child(text)
        };
        Some(deferred(anchored().position_mode(AnchoredPositionMode::Window).position(at).child(copy)).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    // Nome a nome: o `*` traria o `test` do gpui por cima do atributo padrão.
    use super::{Hidden, PanelExit, Phase, flies, keeps_in_chat, local_expired, panel_exit, panel_fraction, phase, queued_row, row_open, same_text, stall_shift};
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
        // Com o destino local, o voo começa no Enter: o tempo desde a saída e desde o destino é o mesmo.
        assert_eq!(phase(ms(160), Some(ms(160)), true, FLIGHT), Phase::Flying(0.5));
        assert_eq!(phase(ms(330), Some(ms(330)), true, FLIGHT), Phase::Landed);
        assert_eq!(phase(ms(370), Some(ms(370)), true, FLIGHT), Phase::Over);
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

    #[test]
    fn a_message_sent_while_idle_stays_out_of_the_queue_panel_until_another_turn_owns_the_session() {
        assert!(keeps_in_chat(None), "a sessão ainda não trabalha: pode estar acordando");
        assert!(keeps_in_chat(Some(ms(300))), "começou agora: é a própria entrega");
        assert!(!keeps_in_chat(Some(ms(1000))), "trabalha há um segundo com ela na fila: o turno é de outra");
    }

    #[test]
    fn the_local_destination_waits_for_the_server_however_long_the_send_takes() {
        assert!(!local_expired(None), "envio ainda sem resposta: o servidor já levou 27 s para pôr a mensagem na fila");
        assert!(!local_expired(Some(ms(4000))));
        assert!(local_expired(Some(ms(5001))), "respondido há mais que o prazo e a linha dele não veio");
    }

    #[test]
    fn the_server_entry_is_the_local_row_whatever_the_spacing() {
        assert!(same_text("terceira mensagem,  vai\npara a fila ", "terceira mensagem, vai para a fila"));
        assert!(!same_text("terceira mensagem", "terceira mensagem, vai para a fila"));
    }

    #[test]
    fn a_stalled_frame_does_not_eat_the_animation() {
        assert_eq!(stall_shift(ms(16)), Duration::ZERO);
        assert_eq!(stall_shift(ms(33)), Duration::ZERO, "tela a 30 quadros por segundo não é parada");
        // A tela ficou 266 ms sem quadro logo depois do Enter: o deslize de 240 ms segue como se fosse um quadro.
        assert_eq!(stall_shift(ms(266)), ms(250));
    }

    #[test]
    fn a_message_flies_out_of_the_panel_once_and_only_from_a_row_on_screen() {
        assert_eq!(panel_exit(true, false, true, true), PanelExit::Fly);
        assert_eq!(panel_exit(false, false, true, true), PanelExit::NeverListed, "a linha chegou antes de o painel do servidor listar a entrada");
        assert_eq!(panel_exit(true, true, true, true), PanelExit::AlreadyLanded, "já pousou: nunca um segundo voo");
        assert_eq!(panel_exit(true, false, false, true), PanelExit::NotDrawnInPanel, "sem a linha na tela não há origem");
        assert_eq!(panel_exit(true, false, true, false), PanelExit::NotPlain);
    }

    #[test]
    fn a_message_sent_to_the_queue_never_shows_as_a_bubble_while_the_panel_holds_it() {
        // A linha `queued-…` chegou 13 ms antes de o painel do servidor listar a entrada: a linha local a segura.
        assert_eq!(queued_row(false, false, false, true), Some(Hidden::Local));
        assert_eq!(queued_row(true, false, false, false), Some(Hidden::Listed));
        assert_eq!(queued_row(true, true, false, false), None, "enviada com a sessão parada: fica na conversa");
        assert_eq!(queued_row(true, false, true, false), None, "destino de uma viagem: aparece para a cópia pousar");
        assert_eq!(queued_row(false, false, false, false), None, "fora da fila e sem linha local: é bolha da conversa");
    }

    #[test]
    fn an_arriving_row_opens_its_height_only_while_the_list_is_at_the_end() {
        assert!(row_open(0., true, false).is_some_and(|open| open < 0.01), "nasce fechada");
        assert!(row_open(0.5, true, false).is_some_and(|open| open > 0.5 && open < 1.), "abre rápido no começo, como o fade");
        assert_eq!(row_open(1., true, false), None, "aberta: a linha de sempre, sem pedir quadro");
        assert_eq!(row_open(0.5, false, false), None, "a pessoa lê acima: a altura não anima");
        assert_eq!(row_open(0.5, true, true), None);
    }
}
