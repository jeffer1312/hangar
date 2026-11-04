//! Painel "Na fila" acima do composer, como o `QueuedRunsControl` do T3: o que espera a vez no Hangar, com arrastar,
//! setas, editar no composer, orientar e apagar por mensagem.
use std::rc::Rc;

use gpui_kit::base::AccordionTrigger;

use super::*;
use crate::queue_panel::{self as model, QueueEdit, QueueEntry};

/// Cerca de quatro linhas à vista; o resto rola dentro do painel.
const LIST_MAX: f32 = 4. * 28.;

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
}

impl QueueUi {
    /// Outra conversa abre com o painel aberto.
    pub(super) fn on_select(&mut self) { self.collapsed = false; self.drop = None; self.dragged = None; }
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
        sends_direct(appearance::get().steer_while_working, &self.chat.state.state, self.provider().0)
    }

    fn set_queue_direct(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.steer_while_working = on;
        if on { next.jev_delivery = false; }
        self.apply_appearance(next, true, cx);
        cx.notify();
    }

    fn set_queue_jev(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut next = appearance::get();
        next.jev_delivery = on;
        if on { next.steer_while_working = false; }
        self.apply_appearance(next, true, cx);
        cx.notify();
    }

    pub(super) fn queue_jev_ready(&self) -> bool {
        self.selected_key().and_then(|k| self.queue.jev.get(&k.server)).is_some_and(|(_, _, result)| matches!(result, Some(Ok(true))))
    }

    fn refresh_queue_jev(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        if self.queue.jev.get(&key.server).is_some_and(|(at, _, result)| result.is_none() || at.elapsed() < Duration::from_secs(30)) { return; }
        let Some(api) = self.api_for(&key.server) else { return };
        self.queue.jev_seq += 1;
        let seq = self.queue.jev_seq;
        self.queue.jev.insert(key.server.clone(), (Instant::now(), seq, None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.server_read(&["jev", "delivery"], &[], 10).await
                .and_then(|config| config["available"].as_bool().ok_or_else(|| Failure::local("invalid_response")))
                .map_err(|error| if error.status == Some(404) { tr("queue_jev_unsupported") } else { Hangar::fetch_failure(&error) });
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::QueueJev(key.server, seq, result) }).await;
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
        if ordinary && appearance::get().jev_delivery && self.queue_jev_ready()
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

    /// O painel; sem mensagem esperando só aparece com o "mandar direto" ligado e o agente trabalhando, porque aí nada entra
    /// na fila e esta é a única caixinha para desligá-lo.
    pub(super) fn render_queue_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.chat.queue_panel();
        let direct = appearance::get().steer_while_working;
        let supports = matches!(self.provider().0, "claude" | "codex");
        if panel.entries.is_empty() && !(supports && direct && self.chat.state.state == "working") { return None; }
        let read_only = self.selected.as_ref().is_some_and(|s| s.read_only());
        let stalled = stalled_notice(&panel, self.chat.state.problema.as_deref(), read_only);
        // Uma cópia por quadro, dividida pelas linhas e pelos handlers de setas e de soltar.
        let (entries, actions): (Rc<[QueueEntry]>, bool) = (panel.entries.iter().cloned().collect(), panel.actions);
        drop(panel);
        if supports { self.refresh_queue_jev(cx); }
        let jev_ready = self.queue_jev_ready();
        let jev_state = self.selected_key().and_then(|key| self.queue.jev.get(&key.server)).map(|(_, _, state)| state);
        let jev_configured = !matches!(jev_state, Some(Some(Ok(false))));
        let jev_help = jev_state.map(|state| match state {
            None => tr("queue_jev_loading"),
            Some(Err(error)) => tr("queue_jev_read_error").replace("{error}", error),
            Some(Ok(false)) => tr("queue_jev_setup"),
            Some(Ok(true)) => tr("queue_jev_help"),
        }).unwrap_or_else(|| tr("queue_jev_loading"));
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
            div().id("queue-panel-list").max_h(px(LIST_MAX)).overflow_y_scroll().flex().flex_col().gap(px(1.)).px(px(6.)).pb(px(6.))
                .children(rows)
        });
        // Fora do cabeçalho: clicar na caixinha não abre nem fecha o painel.
        let toggle = supports.then(|| Checkbox::new("queue-direct").small().label(tr("queue_panel_direct")).checked(direct)
            .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_queue_direct(*checked, cx))));
        let jev_toggle = (supports && jev_configured).then(||Checkbox::new("queue-jev").small().label(tr("queue_jev"))
            .checked(appearance::get().jev_delivery && jev_ready).disabled(!jev_ready)
            .tooltip(jev_help)
            .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_queue_jev(*checked, cx))));
        let top = div().flex().flex_wrap().items_center().gap(px(6.)).pr(px(10.))
            .child(div().flex_1().min_w_0().child(header)).children(toggle).children(jev_toggle);
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
        Some(div().mb(px(6.)).child(chrome::glass_panel(body, px(12.))).into_any_element())
    }

    fn render_queue_row(&mut self, entries: &Rc<[QueueEntry]>, actions: bool, i: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let entry = &entries[i];
        let (id, text) = (entry.id.clone(), entry.text.clone());
        let busy = self.queue_busy();
        let editing = self.editing_entry() == Some(id.as_str());
        let dragging = cx.has_active_drag();
        let drop_line = self.queue.drop.as_ref().filter(|(over, _)| dragging && *over == id).map(|(_, after)| *after);
        let mut row = div().id(SharedString::from(format!("queue-row-{i}"))).relative().flex().items_center().gap(px(4.))
            .min_h(px(26.)).px(px(4.)).rounded(px(8.)).when(editing, |el| el.bg(theme::accent_dim()));
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
        row = row.child(div().id(SharedString::from(format!("queue-text-{i}"))).flex_1().min_w_0().px(px(4.)).truncate()
            .text_size(px(12.5)).text_color(theme::text()).child(conversation::one_line(&full, 400))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)));
        if !actions { return row.into_any_element(); }
        if editing {
            return row.child(Button::new("queue-cancel-edit").ghost().xsmall().label(tr("cancel"))
                .on_click(cx.listener(|this, _, window, cx| this.finish_queue_edit(window, cx)))).into_any_element();
        }
        let (edit, steer, remove) = (id.clone(), id.clone(), id);
        row.child(Button::new(SharedString::from(format!("queue-edit-{i}"))).ghost().xsmall().icon(IconName::Pencil)
                .tooltip(tr_shared("queue_panel_edit", &[])).accessibility_label(tr_shared("queue_panel_edit", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.begin_queue_edit(&edit, window, cx))))
            .child(Button::new(SharedString::from(format!("queue-steer-{i}"))).ghost().xsmall().icon(IconName::CornerUpRight)
                .label(tr_shared("codex_orientar", &[])).tooltip(tr_shared("codex_orientar_ajuda", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.act(Action::QueueSteer(steer.clone()), steer.clone(), cx))))
            .child(Button::new(SharedString::from(format!("queue-remove-{i}"))).ghost().xsmall().icon(IconName::Close)
                .tooltip(tr_shared("queue_panel_remove", &[])).accessibility_label(tr_shared("queue_panel_remove", &[])).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.act(Action::QueueCancel(remove.clone()), remove.clone(), cx))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{sends_direct, stalled_notice};
    use crate::api::dto::ChatEvent;

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
