//! Leitura e aprovação do plano compartilham o pedido atual, sem alterar o transporte das escolhas.
use super::*;

#[derive(Default)]
pub(super) struct ReviewState {
    current: Option<(SessionKey, interaction::PlanReview)>,
    generation: u64,
    reading: bool,
    file: Option<Result<String, String>>,
    expanded: Option<ExpandedReview>,
    /// Campo do "o que mudar", aberto pela opção correspondente; pertence à chamada `tool_id`.
    feedback: Option<Feedback>,
}

struct Feedback { tool_id: String, input: Entity<InputState>, _change: Subscription }

#[derive(Clone)]
struct ExpandedReview { focus: FocusHandle, origin: Option<WeakFocusHandle>, inline_offset: Point<Pixels> }

impl ReviewState {
    pub(super) fn is_expanded(&self) -> bool { self.expanded.is_some() }
}

fn file_content(plan: &interaction::PlanReview, value: &Value) -> Option<String> {
    let path = value.get("path").and_then(Value::as_str)?;
    if let Some(expected) = &plan.path {
        if path != expected { return None; }
    } else if !plan.file_candidates.iter().any(|known| known == path) { return None; }
    value.get("markdown").and_then(Value::as_str).filter(|text| !text.trim().is_empty()).map(str::to_owned)
}

impl Hangar {
    pub(super) fn plan_review_available(&self) -> bool { self.plan_review.current.is_some() }

    fn current_plan_review(&self) -> Option<(SessionKey, interaction::PlanReview)> {
        if self.selected.as_ref().is_none_or(|session| !session.readable() || session.read_only()) || self.chat.ask.is_some() { return None; }
        Some((self.selected_key()?, interaction::plan_review(&self.chat.events, &self.chat.state, self.provider().0)?))
    }

    pub(super) fn sync_plan_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.current_plan_review();
        if self.plan_review.current == current { return; }
        let same_call = self.plan_review.current.as_ref().zip(current.as_ref()).is_some_and(|((old_key, old), (key, next))| old_key == key && old.tool_id == next.tool_id);
        if !same_call { self.close_plan_review(window, cx); self.plan_review.feedback = None; }
        self.plan_review.current = current;
        self.plan_review.generation += 1;
        self.plan_review.file = None;
        self.plan_review.reading = false;
        self.plan_scroll = Default::default();
        self.plan_view = None;
        if self.plan_review.current.as_ref().is_some_and(|(_, plan)| plan.plan.is_empty() && (plan.path.is_some() || !plan.file_candidates.is_empty())) {
            self.read_review_file(cx);
        }
    }

    fn read_review_file(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some((key, _))) = (self.session_api(), self.plan_review.current.clone()) else { return; };
        if self.plan_review.reading { return; }
        self.plan_review.generation += 1;
        self.plan_review.reading = true;
        self.plan_review.file = None;
        let (generation, connection, tx) = (self.plan_review.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["plan-preview"], &[("content", "true")], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::PlanReview(generation), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_review_file(&mut self, key: SessionKey, generation: u64, result: Result<Value, Failure>) {
        if generation != self.plan_review.generation || self.current_plan_review() != self.plan_review.current { return; }
        let Some((owner, plan)) = &self.plan_review.current else { return; };
        if owner != &key { return; }
        self.plan_review.reading = false;
        self.plan_review.file = Some(match result {
            Ok(value) => file_content(plan, &value).ok_or_else(|| tr("plan_review_unavailable")),
            Err(error) => Err(Self::failure(&error)),
        });
    }

    pub(super) fn close_plan_review(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(expanded) = self.plan_review.expanded.take() else { return false; };
        self.plan_scroll.1.set_offset(expanded.inline_offset);
        expanded.origin.and_then(|focus| focus.upgrade()).unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
        cx.notify();
        true
    }

    fn expand_plan_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current_plan_review() != self.plan_review.current || self.plan_review.current.is_none() { return; }
        let focus = cx.focus_handle();
        let origin = window.focused(cx).map(|focus| focus.downgrade());
        focus.focus(window, cx);
        self.plan_review.expanded = Some(ExpandedReview { focus, origin, inline_offset: self.plan_scroll.1.offset() });
        cx.notify();
    }

    fn review_action(&mut self, expected: &(SessionKey, interaction::PlanReview), action: Action, snapshot: String, cx: &mut Context<Self>) {
        if self.current_plan_review().as_ref() != Some(expected) {
            if let Some(key) = self.selected_key() { self.action_feedback.insert(key, (tr("request_changed"), true)); }
            cx.notify();
            return;
        }
        self.act(action, snapshot, cx);
    }

    fn toggle_plan_feedback(&mut self, tool_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.plan_review.feedback.as_ref().is_some_and(|f| f.tool_id == tool_id) { self.plan_review.feedback = None; cx.notify(); return; }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("plan_review_changes_placeholder")));
        // O botão Enviar depende do texto: cada edição redesenha o cartão; Enter envia como o botão.
        let change = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            // Shift+Enter (ou Ctrl/Cmd+Enter) não envia: só o Enter puro, como o compositor do subagente.
            InputEvent::PressEnter { secondary: false, shift: false } => this.send_plan_feedback(cx),
            InputEvent::PressEnter { .. } => {}
            _ => cx.notify(),
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.plan_review.feedback = Some(Feedback { tool_id, input, _change: change });
        cx.notify();
    }

    /// O mesmo envio do botão: opção "o que mudar" do plano exibido, com o texto do campo.
    fn send_plan_feedback(&mut self, cx: &mut Context<Self>) {
        let Some(expected) = self.plan_review.current.clone() else { return };
        let Some(input) = self.plan_review.feedback.as_ref().filter(|f| f.tool_id == expected.1.tool_id).map(|f| f.input.clone()) else { return };
        let text = input.read(cx).value().to_string();
        let option = self.chat.state.options.clone().unwrap_or_default().iter()
            .position(|option| interaction::plan_choice_key(option) == Some("plan_review_changes"));
        if text.trim().is_empty() { return; }
        // Sem a opção "o que mudar" no menu exibido, o pedido mudou: avisa em vez de engolir o texto.
        let Some(index) = option else {
            if let Some(key) = self.selected_key() { self.action_feedback.insert(key, (tr("request_changed"), true)); }
            cx.notify();
            return;
        };
        let snapshot = select_snapshot(&self.chat.state);
        self.review_action(&expected, Action::SelectText(index + 1, text), snapshot, cx);
    }

    pub(super) fn render_plan_review_card(&mut self, expanded: bool, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let expected = self.plan_review.current.clone()?;
        let (_, plan) = &expected;
        let inline_height = (f32::from(window.viewport_size().height) * 0.42).clamp(100., 440.);
        let detached = !expanded && self.plan_review.expanded.is_some();
        let text = if !plan.plan.is_empty() { Some(plan.plan.clone()) } else { self.plan_review.file.as_ref().and_then(|result| result.as_ref().ok()).cloned() };
        let body = if detached {
            div().h(px(inline_height)).flex_shrink(1.).min_h_0().into_any_element()
        } else if let Some(text) = text {
            let source = safe_markdown(&text);
            let view = match &self.plan_view {
                Some((cached, view)) if *cached == source => view.clone(),
                _ => { let view = cx.new(|cx| TextViewState::markdown(&source, cx)); self.plan_view = Some((source, view.clone())); view }
            };
            let document = div().p(px(22.)).child(TextView::new(&view).selectable(true).scrollable(false).code_block_actions(copy_code));
            let handle = &self.plan_scroll.1;
            // Fora da tela cheia o texto cede altura ao cabeçalho e às opções, que nunca saem da área da pergunta.
            div().relative().min_h_0().flex().flex_col().when(expanded, |el| el.flex_1().h_full()).when(!expanded, |el| el.flex_shrink(1.))
                .child(div().id(if expanded { "plan-review-expanded-scroll" } else { "plan-review-scroll" })
                    .when(expanded, |el| el.h_full()).when(!expanded, |el| el.h(px(inline_height)).flex_shrink(1.).min_h_0())
                    .overflow_y_scroll().track_scroll(handle).pr_3().child(document))
                .child(div().absolute().inset_0().child(Scrollbar::vertical(handle).mode(ScrollbarMode::Always)))
                .into_any_element()
        } else {
            let reason = if self.plan_review.reading { tr("plan_preview_loading") }
                else { self.plan_review.file.as_ref().and_then(|result| result.as_ref().err()).cloned().unwrap_or_else(|| tr("plan_review_unavailable")) };
            div().p_4().min_h(px(100.)).when(expanded, |el| el.flex_1()).flex().flex_col().items_start().gap_3()
                .child(div().text_sm().text_color(theme::muted()).child(reason))
                .when(!self.plan_review.reading && (plan.path.is_some() || !plan.file_candidates.is_empty()), |el| el.child(Button::new("plan-review-retry").small().outline().label(tr("ctl_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.read_review_file(cx)))))
                .into_any_element()
        };
        let head = div().p_3().flex_shrink_0().flex().items_center().gap_3().border_b_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("plan_review_title")))
                .child(div().text_xs().text_color(theme::muted()).child(tr("plan_review_hint"))))
            .child(Button::new(if expanded { "plan-review-back" } else { "plan-review-expand" }).small().outline().disabled(detached)
                .icon(if expanded { IconName::Minimize } else { IconName::Maximize }).label(tr(if expanded { "plan_review_back" } else { "plan_review_expand" }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if expanded { this.close_plan_review(window, cx); } else { this.expand_plan_review(window, cx); }
                })));
        let feedback = self.plan_review.feedback.as_ref().filter(|f| f.tool_id == plan.tool_id).map(|f| f.input.clone());
        let mut feedback_row = None;
        let mut choices = div().flex().flex_wrap().gap_2().items_center();
        for (index, option) in self.chat.state.options.clone().unwrap_or_default().iter().enumerate() {
            let key = interaction::plan_choice_key(option);
            let label = key.map(tr).unwrap_or_else(|| option.clone());
            let request = expected.clone();
            let snapshot = select_snapshot(&self.chat.state);
            let asks_text = key == Some("plan_review_changes");
            let (send_request, send_snapshot) = (request.clone(), snapshot.clone());
            let button = Button::new(SharedString::from(format!("plan-review-option-{expanded}-{index}")))
                .small().outline().h_auto().min_h(px(32.)).max_w_full().disabled(busy || detached)
                .accessibility_label(format!("{}. {option}", index + 1)).tooltip(option.clone())
                .child(div().whitespace_normal().py_1().child(format!("{}. {label}", index + 1)));
            choices = choices.child(if asks_text {
                let tool_id = plan.tool_id.clone();
                button.toggled(feedback.is_some()).on_click(cx.listener(move |this, _, window, cx| this.toggle_plan_feedback(tool_id.clone(), window, cx)))
            } else {
                button.on_click(cx.listener(move |this, _, _, cx| this.review_action(&request, Action::Select(index + 1), snapshot.clone(), cx)))
            });
            if let (true, Some(input), false) = (asks_text, feedback.as_ref(), detached) {
                let ready = !input.read(cx).value().trim().is_empty();
                let input_for_send = input.clone();
                feedback_row = Some(div().flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(input).disabled(busy)))
                    .child(Button::new(SharedString::from(format!("plan-review-send-{expanded}"))).small().primary().label(tr("send")).disabled(busy || !ready)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let text = input_for_send.read(cx).value().to_string();
                            this.review_action(&send_request, Action::SelectText(index + 1, text), send_snapshot.clone(), cx);
                        }))));
            }
        }
        let request = expected.clone();
        choices = choices.child(Button::new(if expanded { "plan-review-cancel-expanded" } else { "plan-review-cancel" }).small().ghost().disabled(busy || detached)
            .label(tr("cancel")).on_click(cx.listener(move |this, _, _, cx| this.review_action(&request, Action::Cancel, String::new(), cx))));
        // O texto cede antes (peso quase zero aqui); só sem texto a mostrar o rodapé encolhe e rola, sem cortar decisões.
        let footer = div().id("plan-review-footer").p_3().flex_shrink(0.001).min_h_0().overflow_y_scroll().border_t_1().border_color(theme::border()).flex().flex_col().gap_2()
            .child(div().flex().flex_wrap().gap_2().items_center()
                .child(div().flex_1().text_xs().text_color(theme::muted()).child(tr("plan_review_decision")))
                .when_some(plan.path.as_ref(), |el, path| el.child(div().max_w_full().text_xs().text_color(theme::muted()).truncate().child(composer::safe_name(path)))))
            .child(choices)
            .children(feedback_row);
        let space = self.question_space_height(true, window);
        Some(div().flex().flex_col().min_h_0().when(expanded, |el| el.size_full()).when(!expanded, |el| el.max_h(px(space)))
            .rounded(px(14.)).border_1().border_color(theme::ask_highlight().opacity(0.55)).bg(theme::boxed()).overflow_hidden()
            .child(head).child(body).child(footer).into_any_element())
    }

    pub(super) fn render_plan_review_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let focus = self.plan_review.expanded.as_ref()?.focus.clone();
        let busy = self.selected_key().as_ref().is_some_and(|key| self.flight.busy(key));
        let card = self.render_plan_review_card(true, busy, window, cx)?;
        let overlay=div().absolute().inset_0().p_4().bg(theme::background()).occlude()
            // O deferred muda a árvore de eventos: o vínculo precisa viajar com a própria camada.
            .when(self.in_tab_preview,|el|el
                .capture_any_mouse_down(cx.listener(|this,_,window,cx|this.enter_tab_preview_event(window,cx)))
                .capture_any_mouse_up(cx.listener(|this,_,window,cx|this.enter_tab_preview_event(window,cx)))
                .capture_key_down(cx.listener(|this,_,window,cx|this.enter_tab_preview_event(window,cx)))
                .capture_action(cx.listener(|this,_: &Escape,window,cx| {
                    this.enter_tab_preview_event(window,cx);this.close_plan_review(window,cx);cx.stop_propagation();
                })))
            .child(div().size_full().child(card).focus_trap("plan-review-focus", &focus));
        Some(deferred(overlay)
            .with_priority(gpui_kit::base::POPUP_PRIORITY).into_any_element())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn file_content_requires_the_path_proven_by_the_current_call() {
        let plan = interaction::PlanReview { tool_id: "p".into(), plan: String::new(), path: Some("/plans/p.md".into()), file_candidates: Vec::new() };
        assert_eq!(file_content(&plan, &json!({"path":"/plans/p.md","markdown":"# Atual"})), Some("# Atual".into()));
        assert!(file_content(&plan, &json!({"path":"/plans/old.md","markdown":"# Antigo"})).is_none());
        assert!(file_content(&plan, &json!({"path":"/plans/p.md","markdown":"  "})).is_none());
        assert!(file_content(&plan, &Value::Null).is_none());
    }

    #[test]
    fn file_content_accepts_a_confirmed_plan_even_after_another_markdown_edit() {
        let plan = interaction::PlanReview { tool_id: "p".into(), plan: String::new(), path: None,
            file_candidates: vec!["/repo/README.md".into(), "/plans/p.md".into()] };
        assert!(file_content(&plan, &json!({"path":"/plans/p.md","markdown":"# Atual"})).is_some());
    }
}
