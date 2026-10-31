use super::*;
use super::session_numbers::{Entry, Selection};

#[derive(Default)]
pub(super) struct SessionPicker {
    selection: Selection,
    timer: Option<Task<()>>,
    cancelled: bool,
}

fn incarnation(session: &SessionInfo) -> Option<&str> {
    session.lifecycle_id.as_deref().or(session.jsonl.as_deref())
}

impl Hangar {
    fn session_numbers_allowed(&self, window: &mut Window, cx: &mut App) -> bool {
        window.is_window_active() && !self.connection_dialog && !window.has_active_dialog(cx)
            && (self.settings.is_none() || self.settings_live()) && self.costs.view.is_none()
            && self.worktrees.view.is_none() && !self.search.open && self.sidebar.editing.is_none()
            && !self.keyboard.is_editing()
    }

    /// Desiste da seleção até os modificadores mudarem: segurar de novo não reabre os números.
    fn abandon_session_numbers(&mut self, cx: &mut Context<Self>) {
        self.cancel_session_numbers(cx);
        self.session_picker.cancelled = true;
    }

    pub(super) fn cancel_session_numbers(&mut self, cx: &mut Context<Self>) {
        let shown = self.session_picker.selection.active();
        self.session_picker.timer = None;
        self.session_picker.selection.cancel();
        if shown { self.redraw(panes::Area::Nav, cx); }
    }

    pub(super) fn session_number_modifiers(&mut self, modifiers: Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        if modifiers != self.keyboard.hold_modifiers() {
            self.session_picker.cancelled = false;
            // Soltar o atalho com um número digitado é a confirmação, como Alt+Tab: não espera o prazo.
            if !self.session_picker.selection.input().is_empty() && self.session_numbers_allowed(window, cx) {
                self.session_picker.timer = None;
                let input = self.session_picker.selection.input().to_owned();
                let chosen = self.session_picker.selection.confirm();
                self.select_numbered_session(chosen, &input, window, cx);
            }
            self.cancel_session_numbers(cx);
            return;
        }
        if self.session_picker.cancelled || !self.session_numbers_allowed(window, cx) {
            self.cancel_session_numbers(cx);
            return;
        }
        if self.session_picker.selection.active() { return; }
        let entries = self.numbered_session_order(cx).into_iter().filter_map(|target| {
            let session = self.target_session(&target)?;
            Some(Entry { incarnation: incarnation(session).map(str::to_owned), target })
        }).collect();
        self.session_picker.selection.begin(entries);
        self.redraw(panes::Area::Nav, cx);
    }

    pub(super) fn session_number_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if event.prefer_character_input {
            self.abandon_session_numbers(cx);
            return false;
        }
        self.session_number_modifiers(window.modifiers(), window, cx);
        if !self.session_picker.selection.active() { return false; }
        let key = event.keystroke.key.as_str();
        if matches!(key, "escape" | "enter" | "backspace") && !self.session_picker.selection.claims_edit_keys() { return false; }
        if key == "escape" {
            self.abandon_session_numbers(cx);
            return true;
        }
        if key == "enter" {
            self.session_picker.timer = None;
            let input = self.session_picker.selection.input().to_owned();
            let chosen = self.session_picker.selection.confirm();
            self.select_numbered_session(chosen, &input, window, cx);
            return true;
        }
        if key == "backspace" {
            if !event.is_held {
                self.session_picker.selection.backspace(Instant::now());
                self.schedule_session_number(cx, window);
            }
            return true;
        }
        let stroke = KeybindingKeystroke::new_with_mapper(event.keystroke.clone(), false, cx.keyboard_mapper().as_ref());
        let layout = cx.keyboard_layout();
        let digit = super::session_numbers::event_digit(event.physical_digit, stroke.key(), [layout.id(), layout.name()]);
        // Outra tecla com os modificadores presos é atalho (Ctrl+Shift+T abre um diálogo, que não repassa a soltura
        // dos modificadores à raiz): os números saem da tela agora, em vez de ficarem presos até ele fechar.
        let Some(digit) = digit else {
            self.abandon_session_numbers(cx);
            return false;
        };
        if !event.is_held && self.session_picker.selection.push_digit(digit, Instant::now()) {
            self.schedule_session_number(cx, window);
        }
        true
    }

    fn schedule_session_number(&mut self, cx: &mut Context<Self>, window: &mut Window) {
        self.session_picker.timer = None;
        self.redraw(panes::Area::Nav, cx);
        let Some(deadline) = self.session_picker.selection.deadline() else { return; };
        let revision = self.session_picker.selection.revision();
        self.session_picker.timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(deadline.saturating_duration_since(Instant::now())).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.session_picker.selection.revision() != revision { return; }
                if window.modifiers() != this.keyboard.hold_modifiers() || !this.session_numbers_allowed(window, cx) {
                    this.cancel_session_numbers(cx);
                    return;
                }
                let input = this.session_picker.selection.input().to_owned();
                let chosen = this.session_picker.selection.finish_if_ready(Instant::now());
                this.select_numbered_session(chosen, &input, window, cx);
            });
        }));
    }

    fn select_numbered_session(&mut self, chosen: Option<Entry>, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        if input.is_empty() { return; }
        let Some(chosen) = chosen else {
            window.push_notification(Notification::warning(tr("keyboard_number_invalid").replace("{number}", input)), cx);
            self.redraw(panes::Area::Nav, cx);
            return;
        };
        let alive = self.target_session(&chosen.target).is_some_and(|session| {
            chosen.incarnation.as_deref().is_none_or(|captured| incarnation(session) == Some(captured))
        });
        if alive {
            self.open_target(&chosen.target, window, cx);
            self.tour_event(crate::tour::TourEvent::NumberedSession, cx);
        }
        else { window.push_notification(Notification::warning(tr("keyboard_number_unavailable").replace("{number}", input)), cx); }
        self.redraw(panes::Area::Nav, cx);
    }

    fn numbered_session_order(&self, cx: &App) -> Vec<sidebar::Target> {
        if !self.rail() { return self.visible_order(cx); }
        let rows = |key: &str, layout: sidebar::Layout<'_>| {
            layout.waiting.into_iter().chain(layout.groups.into_iter().flat_map(|group| group.sessions))
                .map(|session| sidebar::Target::new(key, &session.name)).collect::<Vec<_>>()
        };
        let active = self.active_key();
        if !self.multi_server() { return rows(&active, self.sidebar_layout(cx)); }
        let mut order = Vec::new();
        for server in self.servers.iter().filter(|server| !server.disabled) {
            let key = servers::norm(&server.address);
            if key == active { order.extend(rows(&key, self.sidebar_layout(cx))); }
            else if let Some(list) = self.remote.get(&key) { order.extend(rows(&key, self.remote_layout(&key, &list.sessions, cx))); }
        }
        order
    }

    pub(super) fn session_number_badge(&self, target: &sidebar::Target) -> Option<AnyElement> {
        let number = self.session_picker.selection.number(target)?.to_string();
        let matching = self.session_picker.selection.input() == number;
        Some(div().id(SharedString::from(format!("session-number-{}", target.id()))).flex_shrink_0()
            .px_1().rounded_sm().border_1().border_color(theme::border_strong())
            .bg(if matching { theme::accent_dim() } else { theme::raised() })
            .font_family(theme::MONO).text_xs().font_weight(FontWeight::SEMIBOLD)
            .text_color(if matching { theme::accent_text() } else { theme::text() })
            .child(number).into_any_element())
    }

    pub(super) fn session_number_label(&self, target: &sidebar::Target, label: String) -> String {
        match self.session_picker.selection.number(target) {
            Some(number) => format!("{} · {label}", tr("keyboard_number_label").replace("{number}", &number.to_string())),
            None => label,
        }
    }
}
