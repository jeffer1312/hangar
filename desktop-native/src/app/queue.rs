//! Painel "Na fila" acima do composer, como o `QueuedRunsControl` do T3: o que espera a vez no Hangar, com arrastar,
//! setas, editar no composer, orientar e apagar por mensagem.
use std::rc::Rc;

use gpui_kit::base::AccordionTrigger;

use super::*;
use crate::appearance::DeliveryMode;
use crate::queue_panel::{self as model, QueueEdit, QueueEntry};

/// Cerca de quatro linhas à vista; o resto rola dentro do painel.
const LIST_MAX: f32 = 4. * 28.;
/// Medidas fixas do painel: cabeçalho, linha, vão entre linhas, pé da lista, borda e o vão até o cartão do composer.
const HEAD_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 26.;
const ROW_GAP: f32 = 1.;
const LIST_PAD: f32 = 6.;
const BORDER: f32 = 1.;
const PANEL_GAP: f32 = 6.;
/// Recuo dos lados da lista e da linha, e a altura do texto dentro da linha.
const LIST_SIDE: f32 = 6.;
const ROW_SIDE: f32 = 4.;
const ROW_TEXT: f32 = 20.;

/// O que se arrasta pela alça.
#[derive(Clone)]
pub(super) struct QueueDrag { id: String, text: String }

/// O que acompanha o ponteiro durante o arrasto: o texto da mensagem numa linha.
pub(super) struct QueueDragChip { text: String }

impl Render for QueueDragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().max_w(px(320.)).px(px(10.)).py(px(4.)).rounded(px(8.)).bg(theme::raised()).border_1().border_color(theme::border_strong())
            .shadow(theme::popover_shadow()).text_size(px(12.5)).text_color(theme::text()).truncate()
            .child(conversation::one_line(&super::quotes::summary(&self.text), 80))
    }
}

/// Estado do painel na tela: a edição em curso (de qual conversa), recolhido e onde o arrasto soltaria.
#[derive(Default)]
pub(super) struct QueueUi {
    jev: HashMap<String, (Instant, u64, Option<Result<bool, String>>)>,
    jev_seq: u64,
    edit: Option<(SessionKey, QueueEdit)>,
    collapsed: bool,
    /// Linha sob o ponteiro no arrasto e se ele está na metade de baixo dela.
    drop: Option<(String, bool)>,
    /// Entrada que saiu pela alça; só vale enquanto há arrasto ativo.
    dragged: Option<String>,
    /// Texto da única mensagem que o painel mostra: de onde ela sai em viagem para a conversa.
    pub(super) shown_text: Option<String>,
    /// Falso na conversa recém-aberta: o primeiro desenho adota o estado do painel sem animar.
    settled: bool,
    open: bool,
    /// O painel deslizando: a fração de onde partiu, se abre, e desde quando.
    slide: Option<(f32, bool, Instant)>,
    /// As entradas do último desenho com o painel aberto: é o que ele mostra enquanto fecha.
    last: Rc<[QueueEntry]>,
    /// Linhas à vista e se o painel pode deslizar, como no último desenho: a raiz faz a conta da altura por eles.
    shape: (usize, bool),
    /// A altura deslizante com que o painel foi desenhado no último quadro, e a que a raiz já contou para este.
    drawn: Option<f32>,
    planned: Option<f32>,
}

impl QueueUi {
    /// Outra conversa abre com o painel aberto.
    pub(super) fn on_select(&mut self) {
        (self.collapsed, self.drop, self.dragged) = (false, None, None);
        (self.shown_text, self.settled) = (None, false);
        // O deslize era do painel da outra conversa: a raiz não conta altura por ele.
        (self.slide, self.drawn, self.planned) = (None, None, None);
    }

    /// Quanto da altura o painel mostra agora: 1 aberto, 0 fechado, entre os dois enquanto desliza.
    fn fraction(&self) -> f32 {
        match self.slide {
            Some((from, opening, at)) => {
                let spec = slide_spec(opening);
                send_flight::panel_fraction(from, opening, spec.ease(spec.raw(at)))
            }
            None => if self.open { 1. } else { 0. },
        }
    }

    /// O deslize em curso começa `by` mais tarde, nunca depois de `now`: um quadro que demorou não come a abertura.
    pub(super) fn delay(&mut self, by: Duration, now: Instant) -> bool {
        let Some((_, _, at)) = self.slide.as_mut() else { return false };
        *at = (*at + by).min(now);
        true
    }

    /// A altura do painel deslizando agora; `None` parado ou sem poder deslizar.
    fn sliding_height(&self) -> Option<f32> {
        let (rows, slides) = self.shape;
        (slides && self.slide.is_some_and(|(_, opening, at)| slide_spec(opening).raw(at) < 1.))
            .then(|| (panel_height(rows) + PANEL_GAP) * self.fraction())
    }
}

fn slide_spec(opening: bool) -> motion::Spec { if opening { motion::QUEUE_OPEN } else { motion::FLIGHT } }

/// Altura do painel com `rows` linhas à vista (0 recolhido ou sem mensagens), pelas medidas com que ele é desenhado:
/// abrir e fechar por uma altura calculada não espera a medida de um quadro para acertar a do seguinte.
fn panel_height(rows: usize) -> f32 {
    let list = if rows == 0 { 0. } else { (rows as f32 * ROW_HEIGHT + (rows - 1) as f32 * ROW_GAP + LIST_PAD).min(LIST_MAX) };
    2. * BORDER + HEAD_HEIGHT + list
}

/// Onde fica o texto da última linha do painel, colado acima do cartão do composer (`card`), pelas medidas com que ele
/// é desenhado: a linha local de quem acabou de enviar tem lugar antes de o painel ser desenhado.
fn row_place_above(card: Bounds<Pixels>) -> Bounds<Pixels> {
    let side = BORDER + LIST_SIDE + ROW_SIDE;
    let top = PANEL_GAP + BORDER + LIST_PAD + ROW_HEIGHT - (ROW_HEIGHT - ROW_TEXT) / 2.;
    Bounds::new(point(card.origin.x + px(side), card.origin.y - px(top)), size(card.size.width - px(2. * side), px(ROW_TEXT)))
}

/// O lugar da linha local do painel agora, pelo cartão do composer do último quadro; `None` sem ele na tela.
pub(super) fn row_place() -> Option<Bounds<Pixels>> {
    popup::anchor_peek(crate::tour::anchors::COMPOSER).map(row_place_above)
}

/// "Mandar direto": só Claude e Codex têm o Orientar por mensagem, e só com o agente trabalhando.
pub(super) fn sends_direct(enabled: bool, state: &str, provider: &str) -> bool {
    enabled && state == "working" && matches!(provider, "claude" | "codex")
}

/// Aviso de entrega parada no painel: é onde a mensagem presa espera (a bolha dela some da conversa).
pub(super) fn stalled_notice(panel: &model::QueuePanel, problem: Option<&str>, read_only: bool) -> bool {
    !panel.entries.is_empty() && problem == Some(super::STALLED) && !read_only
}

impl Hangar {
    fn queue_busy(&self) -> bool { self.selected_key().is_some_and(|key| self.flight.busy(&key)) }

    /// O envio desta conversa sai como Orientar, sem esperar no painel.
    pub(super) fn queue_sends_direct(&self) -> bool {
        sends_direct(appearance::get().delivery_mode() == DeliveryMode::Direct, &self.chat.state.state, self.provider().0)
    }

    pub(super) fn set_delivery_mode(&mut self, mode: DeliveryMode, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.set_delivery_mode(mode);
        self.apply_appearance(next, true, cx);
        cx.notify();
    }

    /// O servidor do Jev: o da conversa aberta; sem ela (Configurações pela tela inicial), o ativo.
    fn queue_jev_server(&self) -> Option<String> {
        self.selected_key().map(|key| key.server).or_else(|| self.server.clone())
    }

    pub(super) fn queue_jev_ready(&self) -> bool {
        self.queue_jev_server().and_then(|server| self.queue.jev.get(&server)).is_some_and(|(_, _, result)| matches!(result, Some(Ok(true))))
    }

    /// Por que o Jev deste servidor está ou não disponível: a dica da opção.
    pub(super) fn queue_jev_help(&self) -> String {
        let state = self.queue_jev_server().and_then(|server| self.queue.jev.get(&server)).map(|(_, _, state)| state);
        state.map(|state| match state {
            None => tr("queue_jev_loading"),
            Some(Err(error)) => tr("queue_jev_read_error").replace("{error}", error),
            Some(Ok(false)) => tr("queue_jev_setup"),
            Some(Ok(true)) => tr("queue_jev_help"),
        }).unwrap_or_else(|| tr("queue_jev_loading"))
    }

    pub(super) fn refresh_queue_jev(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.queue_jev_server() else { return };
        if self.queue.jev.get(&server).is_some_and(|(at, _, result)| result.is_none() || at.elapsed() < Duration::from_secs(30)) { return; }
        let Some(api) = self.api_for(&server) else { return };
        self.queue.jev_seq += 1;
        let seq = self.queue.jev_seq;
        self.queue.jev.insert(server.clone(), (Instant::now(), seq, None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.server_read(&["jev", "delivery"], &[], 10).await
                .and_then(|config| config["available"].as_bool().ok_or_else(|| Failure::local("invalid_response")))
                .map_err(|error| if error.status == Some(404) { tr("queue_jev_unsupported") } else { Hangar::fetch_failure(&error) });
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::QueueJev(server, seq, result) }).await;
        });
    }

    pub(super) fn receive_queue_jev(&mut self, server: String, seq: u64, result: Result<bool, String>) {
        if self.queue.jev.get(&server).is_some_and(|(_, current, _)| *current == seq) {
            self.queue.jev.insert(server, (Instant::now(), seq, Some(result)));
        }
    }

    pub(super) fn invalidate_queue_jev(&mut self, server: &str) { self.queue.jev.remove(server); }

    pub(super) fn queue_timing(&self, direct: bool, ordinary: bool, message: &str) -> SendTiming {
        if direct { return SendTiming::Direct; }
        let mode = appearance::get().delivery_mode();
        // "Na conversa" vale com e sem terminal: o Claude Code lê a mensagem na pausa seguinte nos dois.
        if ordinary && sends_direct(mode == DeliveryMode::Conversation, &self.chat.state.state, self.provider().0) {
            return SendTiming::Conversation;
        }
        if ordinary && mode == DeliveryMode::Jev && self.queue_jev_ready()
            && sends_direct(true, &self.chat.state.state, self.provider().0) {
            let latest = self.chat.events.iter().rposition(|e| e.kind == "user_msg" && !e.queued() && !e.id.starts_with("pending-") && peer_of(e).is_none());
            let task = latest.map(|index| {
                    let e = &self.chat.events[index];
                    let body = e.body();
                    let caption = composer::parse_marked(&body).map(|m| m.caption).unwrap_or(body);
                    let (_, task) = super::quotes::split_refs(&caption);
                    task.to_owned()
                }).unwrap_or_default();
            // Em um turno longo, o pedido pode estar fora da página carregada, mas as falas do agente continuam válidas.
            let recent_start = latest.map_or(0, |index| index + 1);
            let recent = self.chat.ask.as_ref().filter(|ask| ask.payload.is_async)
                .map(|ask| ask.payload.questions.iter().map(|q| q.question.as_str()).collect::<Vec<_>>().join("\n"))
                .or_else(|| self.chat.events[recent_start..].iter().rev()
                    .find(|e| e.kind == "assistant_msg" && !e.body().trim().is_empty()).map(|e| e.body()))
                .unwrap_or_default();
            let task = crate::delivery::timing_context(&task, &recent);
            let task = if message.chars().count() > 2000 { String::new() } else { task };
            return SendTiming::Jev { task, message: message.chars().take(2000).collect() };
        }
        SendTiming::Queued
    }

    pub(super) fn queue_edit_active(&self) -> bool {
        self.queue.edit.as_ref().is_some_and(|(key, _)| Some(key) == self.selected_key().as_ref())
    }

    fn editing_entry(&self) -> Option<&str> {
        self.queue.edit.as_ref().filter(|_| self.queue_edit_active()).map(|(_, edit)| edit.entry_id.as_str())
    }

    /// Rascunho da conversa que sai da tela: em edição de mensagem da fila, o de antes dela (a edição acaba aqui).
    pub(super) fn composer_draft(&mut self, cx: &App) -> String {
        match self.queue.edit.take() {
            Some((key, edit)) => { self.end_edit_refs(&key, false); edit.saved_draft }
            None => self.composer.read(cx).value().to_string(),
        }
    }

    fn begin_queue_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        let panel = self.chat.queue_panel();
        let Some(entry) = panel.entries.iter().find(|e| e.id == id) else { return };
        let draft = self.composer.read(cx).value().to_string();
        let current = self.queue.edit.as_ref().filter(|(k, _)| *k == key).map(|(_, e)| e);
        let edit = model::begin_edit(entry, &draft, current);
        // As citações e os elementos da mensagem vão para a bandeja; o campo recebe só o digitado.
        let (refs, text) = super::quotes::split_refs(&entry.text);
        drop(panel);
        self.begin_edit_refs(&key, refs);
        self.queue.edit = Some((key, edit));
        self.queue.collapsed = false;
        self.set_composer_at_end(text, window, cx);
        cx.notify();
    }

    /// Troca o texto do campo com o cursor no fim, pronto para continuar.
    fn set_composer_at_end(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let last = text.split('\n').count().saturating_sub(1) as u32;
        self.composer.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.set_cursor_position(gpui_kit::component::input::Position::new(last, u32::MAX), window, cx);
        });
    }

    /// Sai da edição (Cancelar, Esc ou atualização feita): o rascunho de antes volta ao composer.
    pub(super) fn finish_queue_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, edit)) = self.queue.edit.take() else { return };
        self.end_edit_refs(&key, false);
        self.set_composer_at_end(edit.saved_draft, window, cx);
        cx.notify();
    }

    /// "Atualizar" e o Enter em edição. Texto igual ao da fila (sem as bordas em branco, como o `queueEditPayload` do
    /// core) só sai da edição, sem pedido.
    pub(super) fn submit_queue_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, edit)) = self.queue.edit.clone() else { return };
        let text = self.compose_with_refs(&key, &self.composer.read(cx).value().to_string());
        if !model::can_update(&text) { return; }
        if text.trim() == edit.original.trim() { self.finish_queue_edit(window, cx); return; }
        self.act(Action::QueueEdit(edit.entry_id.clone(), text), edit.entry_id, cx);
    }

    /// A atualização voltou: sai da edição se ela ainda é a da mesma mensagem.
    pub(super) fn queue_edit_done(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_entry() == Some(id) { self.finish_queue_edit(window, cx); }
    }

    /// A lista nova pode ter levado a mensagem em edição: ela vira rascunho (com o rascunho vazio) e avisa.
    pub(super) fn queue_panel_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.queue_edit_active() { return; }
        let Some((key, edit)) = self.queue.edit.clone() else { return };
        let composer = self.composer.read(cx).value().to_string();
        let Some(text) = model::edit_left(&edit, &self.chat.queue_panel(), &composer) else { return };
        self.queue.edit = None;
        // Volta o texto editado só sem rascunho guardado (`edit_left`); as referências seguem o mesmo destino.
        self.end_edit_refs(&key, edit.saved_draft.trim().is_empty());
        self.set_composer_at_end(text, window, cx);
        self.action_feedback.insert(key, (tr_shared("queue_entry_gone", &[]), true));
        cx.notify();
    }

    fn reorder_queued(&mut self, id: &str, before: Option<String>, cx: &mut Context<Self>) {
        self.act(Action::QueueReorder(id.to_owned(), before), id.to_owned(), cx);
    }

    /// O Enviar do composer em edição: "Atualizar" com texto, na cor do Enviar.
    pub(super) fn queue_update_button(&self, enabled: bool, cx: &mut Context<Self>) -> Button {
        send_colored("send", enabled, cx).0.h(px(30.)).px(px(14.)).rounded_full().text_size(px(13.)).label(tr_shared("queue_panel_update", &[]))
            .accessibility_label(tr_shared("queue_panel_update_aria", &[]))
            .on_click(cx.listener(|this, _, window, cx| this.submit_queue_edit(window, cx)))
    }

    /// Linha acima do campo em edição.
    pub(super) fn queue_editing_note(&self) -> Option<Stateful<Div>> {
        self.queue_edit_active().then(|| div().id("queue-editing").text_xs().text_color(theme::accent())
            .child(tr_shared("queue_panel_editing", &[])))
    }

    /// Quanto a altura do painel deslizando muda do último quadro para este. A raiz soma isso à altura medida da faixa
    /// de baixo, que é a do quadro anterior, e o painel é desenhado com a mesma conta.
    pub(super) fn queue_slide_step(&mut self) -> f32 {
        self.queue.planned = self.queue.sliding_height();
        match (self.queue.drawn, self.queue.planned) { (Some(before), Some(now)) => now - before, _ => 0. }
    }

    pub(super) fn render_queue_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.idle_tick(cx);
        let (planned, _) = (self.queue.planned.take(), self.queue.drawn.take());
        // O Jev decide no Enter: a leitura dele não pode esperar o painel aparecer.
        if matches!(self.provider().0, "claude" | "codex") && appearance::get().delivery_mode() == DeliveryMode::Jev { self.refresh_queue_jev(cx); }
        let panel = self.chat.queue_panel();
        let read_only = self.selected.as_ref().is_some_and(|s| s.read_only());
        let stalled = stalled_notice(&panel, self.chat.state.problema.as_deref(), read_only);
        // A mensagem enviada com a sessão parada fica na conversa até ser entregue: não entra no painel.
        let (mut entries, actions): (Vec<QueueEntry>, bool) =
            (panel.entries.iter().filter(|e| !self.send_keeps_in_chat(&e.id)).cloned().collect(), panel.actions);
        drop(panel);
        // A que saiu do campo com a sessão trabalhando já tem a linha dela, local, até a do servidor tomar o lugar.
        if let Some(text) = self.local_queue_text(&entries) { entries.push(QueueEntry { id: send_flight::LOCAL_QUEUE.into(), text, ts: None }); }
        // Uma cópia por quadro, dividida pelas linhas e pelos handlers de setas e de soltar.
        let entries: Rc<[QueueEntry]> = entries.into();
        let live = !entries.is_empty();
        // O painel abre e fecha pela altura, para a conversa ganhar e perder o espaço aos poucos.
        let moving = !cx.reduce_motion();
        // Com o aviso de entrega parada a altura depende do texto dele: o painel não desliza.
        let slides = !stalled;
        if !self.queue.settled { (self.queue.settled, self.queue.open, self.queue.slide) = (true, live, None); }
        if live != self.queue.open {
            // Inverter no meio parte da fração em que o painel estava.
            let from = self.queue.fraction();
            // Sem deslize não há movimento para esperar: o painel abre e fecha no mesmo quadro.
            (self.queue.open, self.queue.slide) = (live, (moving && slides).then(|| (from, live, Instant::now())));
            send_flight::trace(|| format!("queue panel {} from fraction {from:.2} slides={} rows={} local_row={}", if live { "opens" } else { "closes" },
                moving && slides, entries.len(), entries.iter().any(|e| e.id == send_flight::LOCAL_QUEUE)));
        }
        if live { self.queue.last = entries.clone(); }
        if self.queue.slide.is_some_and(|(_, opening, at)| slide_spec(opening).raw(at) >= 1.) { self.queue.slide = None; }
        if !live && self.queue.slide.is_none() {
            // Sem painel na tela não há linha de onde uma mensagem saia em viagem: a medida velha não vale.
            self.queue.shown_text = None;
            self.flight_places.queue.set(None);
            return None;
        }
        // Fechando, ele mostra o que tinha, sem ações: a fila de verdade já esvaziou.
        let (entries, actions) = if live { (entries, actions) } else { (self.queue.last.clone(), false) };
        // Recolhido, nenhuma linha é desenhada: não há de onde uma mensagem saia em viagem.
        self.queue.shown_text = (entries.len() == 1 && !self.queue.collapsed).then(|| entries[0].text.trim().to_owned());
        // Só a linha única e a linha local gravam o lugar; com outras, a medida anterior não é de ninguém.
        if self.queue.collapsed || entries.len() != 1 && !entries.iter().any(|e| e.id == send_flight::LOCAL_QUEUE) { self.flight_places.queue.set(None); }
        let rows = if self.queue.collapsed { 0 } else { entries.len() };
        self.queue.shape = (rows, slides);
        let expanded = !self.queue.collapsed;
        let header_focus = window.use_keyed_state("queue-panel-header-focus", cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        let weak = cx.entity().downgrade();
        let header = AccordionTrigger::new("queue-panel-header").open(expanded).track_focus(&header_focus)
            .flex().items_center().gap(px(8.)).px(px(12.)).py(px(6.)).rounded(px(12.)).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .when(header_focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .aria_label(tr_shared(if expanded { "queue_panel_collapse" } else { "queue_panel_expand" }, &[]))
            .text_size(px(12.5)).text_color(theme::muted())
            .on_change(move |open, _, _, cx| { let _ = weak.update(cx, |this, cx| { this.queue.collapsed = !open; cx.notify(); }); })
            .child(chrome::small_icon(IconName::ListOrdered, 14., theme::faint()))
            .child(div().flex_1().min_w_0().child(tr("queued")))
            .child(div().font_family(theme::MONO).text_color(theme::faint()).child(entries.len().to_string()))
            .child(chrome::small_icon(if expanded { IconName::ChevronDown } else { IconName::ChevronUp }, 14., theme::faint()));
        let list = (expanded && !entries.is_empty()).then(|| {
            let rows: Vec<AnyElement> = (0..entries.len()).map(|i| self.render_queue_row(&entries, actions, i, window, cx)).collect();
            div().id("queue-panel-list").max_h(px(LIST_MAX)).overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).px(px(LIST_SIDE)).pb(px(LIST_PAD))
                .children(rows)
        });
        let top = div().h(px(HEAD_HEIGHT)).flex_shrink_0().flex().flex_wrap().items_center().gap(px(6.)).pr(px(10.))
            .child(div().flex_1().min_w_0().child(header));
        // Fora da lista: com o painel recolhido o aviso e o Reabrir continuam à vista.
        let stalled = stalled.then(|| div().id("queue-stalled").flex().flex_wrap().items_center().gap(px(8.)).px(px(12.)).pb(px(6.))
            .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(theme::warning())
                .child(tr_shared("problema_headless_delivery_stalled", &[])))
            .child(Button::new("queue-stalled-reopen").xsmall().label(tr_shared("reabrir_sessao_presa", &[]))
                .disabled(!self.reload_allowed()).on_click(cx.listener(|this, _, _, cx| this.reload(cx)))));
        // Mesmo material do cartão do composer, colado acima dele.
        let body = div().id("queue-panel").flex().flex_col().rounded(px(12.)).border_1().border_color(theme::border_strong())
            .bg(theme::boxed()).child(top).children(stalled).children(list);
        let body = popup::anchor(body, crate::tour::anchors::QUEUE_PANEL);
        let panel = chrome::glass_panel(body, px(12.));
        // A conta da raiz para este quadro, quando ela a fez; no primeiro quadro do deslize, a mesma conta aqui.
        let Some(height) = self.queue.sliding_height().map(|now| planned.unwrap_or(now)) else {
            return Some(div().mb(px(PANEL_GAP)).child(panel).into_any_element());
        };
        self.queue.drawn = Some(height);
        motion::request_frame(window, cx);
        // O painel fica preso ao pé da caixa que cresce ou encolhe: nasce do campo, com a linha da mensagem primeiro,
        // e volta para ele. O vão até o cartão do composer vai junto.
        Some(div().relative().overflow_hidden().h(px(height))
            .child(div().absolute().left_0().right_0().bottom(px(PANEL_GAP)).child(panel)).into_any_element())
    }

    fn render_queue_row(&mut self, entries: &Rc<[QueueEntry]>, actions: bool, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let entry = &entries[i];
        let (id, text) = (entry.id.clone(), entry.text.clone());
        // A linha local ainda não existe no servidor: sem alça nem botões até a entrada dele tomar o lugar.
        let (local, single) = (id == send_flight::LOCAL_QUEUE, entries.len() == 1);
        let actions = actions && !local;
        let busy = self.queue_busy();
        let editing = self.editing_entry() == Some(id.as_str());
        let dragging = cx.has_active_drag();
        let drop_line = self.queue.drop.as_ref().filter(|(over, _)| dragging && *over == id).map(|(_, after)| *after);
        let mut row = div().id(SharedString::from(format!("queue-row-{i}"))).relative().flex().items_center().gap(px(4.))
            .h(px(ROW_HEIGHT)).flex_shrink_0().px(px(ROW_SIDE)).rounded(px(8.)).when(editing, |el| el.bg(theme::accent_dim()));
        if let Some(after) = drop_line {
            let line = div().absolute().left_0().right_0().h(px(2.)).rounded_full().bg(theme::accent());
            row = row.child(if after { line.bottom_0() } else { line.top_0() });
        }
        if actions {
            let focus = window.use_keyed_state(SharedString::from(format!("queue-grip-{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true))
                .read(cx).clone();
            let (keys_id, keys_entries) = (id.clone(), entries.clone());
            let grip = div().id(SharedString::from(format!("queue-grip-{i}"))).track_focus(&focus).flex_shrink_0().p(px(3.)).rounded(px(5.))
                .when(!busy, |el| el.cursor_grab().hover(|el| el.bg(theme::hover())))
                .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
                .role(Role::Button).aria_label(tr_shared("queue_panel_reorder", &[]))
                .child(chrome::small_icon(IconName::GripVertical, 14., theme::faint()))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    let up = match event.keystroke.key.as_str() { "up" => true, "down" => false, _ => return };
                    cx.stop_propagation();
                    if this.queue_busy() { return; }
                    if let Some(before) = model::move_target(&keys_entries, &keys_id, up) { this.reorder_queued(&keys_id, before, cx); }
                }));
            let grip = if busy { grip } else {
                let weak = cx.entity().downgrade();
                grip.on_drag(QueueDrag { id: id.clone(), text: text.clone() }, move |drag, _, _, cx| {
                    let _ = weak.update(cx, |this, cx| { (this.queue.drop, this.queue.dragged) = (None, Some(drag.id.clone())); cx.notify(); });
                    cx.new(|_| QueueDragChip { text: drag.text.clone() })
                })
            };
            let (over, entries) = (id.clone(), entries.clone());
            let moved = id.clone();
            row = row.child(grip)
                .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<QueueDrag>, _, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let spot = inside.then(|| (moved.clone(), event.event.position.y > event.bounds.center().y));
                    let next = match spot {
                        Some(spot) => Some(spot),
                        None if this.queue.drop.as_ref().is_some_and(|(over, _)| *over == moved) => None,
                        None => return,
                    };
                    if this.queue.drop != next { this.queue.drop = next; cx.notify(); }
                }))
                .on_drop(cx.listener(move |this, drag: &QueueDrag, _, cx| {
                    let after = this.queue.drop.take().filter(|(spot, _)| *spot == over).is_some_and(|(_, after)| after);
                    this.queue.dragged = None;
                    cx.notify();
                    if this.queue_busy() { return; }
                    if let Some(before) = model::drop_target(&entries, &drag.id, &over, after) { this.reorder_queued(&drag.id, before, cx); }
                }))
                .when(dragging && self.queue.dragged.as_deref() == Some(id.as_str()), |el| el.opacity(0.5));
        }
        let full = super::quotes::summary(&text);
        let arriving = self.send_arrives_at_queue_row(&id);
        row = row.child(div().id(SharedString::from(format!("queue-text-{i}"))).flex_1().min_w_0().px(px(4.)).truncate()
            .text_size(px(12.5)).text_color(theme::text()).child(conversation::one_line(&full, 400))
            // A linha local e a que está sozinha na fila dizem onde estão: a mensagem enviada pousa na primeira, e sai da
            // segunda em viagem para a conversa.
            .when(local || single || arriving, |el| el.relative().child(send_flight::record(&self.flight_places, |p| &p.queue))
                .when(arriving || self.send_hides_queue_text(&id, single), |el| el.opacity(0.)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)));
        if !actions { return row.into_any_element(); }
        if editing {
            return row.child(Button::new("queue-cancel-edit").ghost().xsmall().label(tr("cancel"))
                .on_click(cx.listener(|this, _, window, cx| this.finish_queue_edit(window, cx)))).into_any_element();
        }
        let (edit, steer, remove) = (id.clone(), id.clone(), id);
        let steered = text.clone();
        row.child(Button::new(SharedString::from(format!("queue-edit-{i}"))).ghost().xsmall().icon(IconName::Pencil)
                .tooltip(tr_shared("queue_panel_edit", &[])).accessibility_label(tr_shared("queue_panel_edit", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.begin_queue_edit(&edit, window, cx))))
            .child(Button::new(SharedString::from(format!("queue-steer-{i}"))).ghost().xsmall().icon(IconName::CornerUpRight)
                .label(tr_shared("codex_orientar", &[])).tooltip(tr_shared("codex_orientar_ajuda", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    // A viagem até a conversa parte no clique: a bolha já nasce como destino dela.
                    this.send_takeoff_from_queue(&steered, cx);
                    this.act(Action::QueueSteer(steer.clone()), steer.clone(), cx)
                })))
            .child(Button::new(SharedString::from(format!("queue-remove-{i}"))).ghost().xsmall().icon(IconName::Close)
                .tooltip(tr_shared("queue_panel_remove", &[])).accessibility_label(tr_shared("queue_panel_remove", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.act(Action::QueueCancel(remove.clone()), remove.clone(), cx))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{panel_height, row_place_above, sends_direct, stalled_notice};
    use crate::api::dto::ChatEvent;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn the_local_row_has_its_place_above_the_composer_before_the_panel_is_drawn() {
        // O cartão do composer em (22, 735), com 1320 de largura: a linha medida na tela ficou em (33, 699), 1298 x 20.
        let card = Bounds::new(point(px(22.), px(735.)), size(px(1320.), px(90.)));
        assert_eq!(row_place_above(card), Bounds::new(point(px(33.), px(699.)), size(px(1298.), px(20.))));
    }

    #[test]
    fn the_panel_height_follows_the_rows_shown_up_to_the_list_ceiling() {
        assert_eq!(panel_height(0), 34., "recolhido ou sem mensagens: borda e cabeçalho");
        assert_eq!(panel_height(1), 34. + 26. + 6.);
        assert_eq!(panel_height(2), 34. + 2. * 26. + 1. + 6.);
        assert_eq!(panel_height(9), 34. + 112., "a lista para em quatro linhas e rola");
    }

    #[test]
    fn queue_panel_shows_the_stalled_delivery_with_reopen() {
        // A mensagem que espera (`delivered:false`) está no painel, e a bolha dela some da conversa.
        let waiting = ChatEvent { kind: "user_msg".into(), id: "queued-a".into(), text: Some("oi".into()), queued_delivered: Some(false), ..Default::default() };
        let panel = crate::queue_panel::current(None, std::slice::from_ref(&waiting)).into_owned();
        assert!(crate::queue_panel::hidden("queued-a", &panel));
        assert!(stalled_notice(&panel, Some("headless_delivery_stalled"), false));
        assert!(!stalled_notice(&panel, Some("headless_delivery_stalled"), true), "par só de leitura não reabre");
        assert!(!stalled_notice(&panel, None, false));
        assert!(!stalled_notice(&panel, Some("terminal_input_no_safe_delivery"), false));
        assert!(!stalled_notice(&crate::queue_panel::QueuePanel::default(), Some("headless_delivery_stalled"), false), "fila vazia, nada preso");
    }

    #[test]
    fn direct_send_only_while_claude_or_codex_works_with_the_option_on() {
        assert!(sends_direct(true, "working", "claude"));
        assert!(sends_direct(true, "working", "codex"));
        assert!(!sends_direct(false, "working", "claude"), "desligada, a mensagem espera na fila");
        assert!(!sends_direct(true, "idle", "claude"), "parado, o envio comum já entra na hora");
        assert!(!sends_direct(true, "working", "kimi"), "o Kimi não tem Orientar por mensagem");
    }
}
