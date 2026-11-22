use super::*;
use crate::app::accounts::ProviderChoice;
use gpui_kit::component::input::Input;
use hangar_api::session::{ClaudeProviderSource, ClaudeProviderSwitch};

pub(crate) const TAB: &str = "claude-providers";

pub(crate) fn tab_button(active: bool) -> Button {
    super::picker::rail_button(TAB, active, tr("claude_provider_picker"),
        chrome::small_icon(IconName::Cpu, 18., theme::accent()).into_any_element())
}

pub(crate) fn account_row(id: &str, label: &str, detail: &str, picked: bool, current: bool, disabled: bool) -> Button {
    Button::new(SharedString::from(format!("claude-provider-{id}"))).ghost().w_full().h_auto().selected(picked).disabled(disabled)
        .child(div().w_full().flex().items_center().justify_between().gap_2().py_1()
            .child(div().min_w_0().flex().flex_col().items_start()
                .child(div().truncate().child(label.to_owned()))
                .child(div().text_xs().truncate().text_color(theme::muted()).child(detail.to_owned())))
            .when(current, |el| el.child(div().text_xs().text_color(theme::accent()).child(tr("claude_provider_current")))))
}

pub(crate) fn model_row(model: &str, selected: bool) -> Button {
    Button::new(SharedString::from(format!("claude-provider-model-{model}"))).ghost().w_full()
        .selected(selected).label(super::picker::model_display("claude", model))
}

pub(crate) fn confirmation(account: &str, model: &str) -> Div {
    div().p_3().flex().flex_col().gap_2()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("claude_provider_confirm_title")))
        .child(div().text_sm().child(tr("claude_provider_summary").replace("{account}", account)
            .replace("{model}", &super::picker::model_display("claude", model))))
        .child(div().text_sm().text_color(theme::muted()).child(tr("claude_provider_history")))
}

#[derive(Default)]
pub(super) struct ProviderPicker {
    request: u64,
    source: Option<ClaudeProviderSource>,
    account: Option<ProviderChoice>,
    models: Option<Result<Vec<String>, String>>,
    model: Option<String>,
    confirming: bool,
    notice: Option<String>,
}

impl ProviderPicker {
    pub(super) fn reset(&mut self) {
        *self = Self { request: self.request.wrapping_add(1), ..Self::default() };
    }
    fn chosen(&self) -> Option<(&ProviderChoice, &str)> {
        let account = self.account.as_ref()?;
        account.account.as_ref()?;
        let model = self.model.as_deref()?;
        self.models.as_ref()?.as_ref().ok()?.iter().any(|id| id == model).then_some((account, model))
    }
}

fn source_of(session: &SessionInfo) -> Option<ClaudeProviderSource> {
    if session.provider != "claude" || session.read_only() || session.guest_kind.is_some() { return None; }
    Some(ClaudeProviderSource { lifecycle_id:session.lifecycle_id.clone()?, jsonl:session.jsonl.clone()?, headless:session.headless,
        engine:session.engine.clone(), engine_account:session.engine_account.clone(), engine_binding:session.engine_binding.clone() })
}

impl Hangar {
    pub(super) fn provider_picker_available(&self) -> bool {
        self.selected.as_ref().and_then(source_of).is_some() && self.provider_choices().is_some()
    }

    pub(super) fn open_provider_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controls.providers.reset();
        self.controls.providers.source = self.selected.as_ref().and_then(source_of);
        self.ctl_search.update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn load_provider_models(&mut self, choice: ProviderChoice, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(key), Some(account)) = (self.session_api(), self.selected_key(), choice.account.clone()) else { return; };
        let state = &mut self.controls.providers;
        state.request = state.request.wrapping_add(1);
        let serial = state.request;
        state.account = Some(choice.clone());
        state.model = None;
        state.models = None;
        state.notice = None;
        state.confirming = false;
        self.ctl_search.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
        let connection = self.connection;
        let task = self.runtime.spawn(async move {
            let response = api.server_read(&["model-options"], &[("provider", "claude"), ("engine", &choice.engine),
                ("engine_account", &account), ("engine_binding", &choice.binding)], 20).await.map_err(|e| Self::fetch_failure(&e))?;
            if response["engine"] != choice.engine || response["engine_account"] != account || response["engine_binding"] != choice.binding {
                return Err(tr("claude_provider_changed"));
            }
            let models = response["models"].as_array().ok_or_else(|| tr("invalid_response"))?;
            Ok(models.iter().filter_map(|row| row["id"].as_str().map(str::to_owned)).collect::<Vec<_>>())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(tr("claude_provider_read_failed")));
            let _ = this.update(cx, |this, cx| {
                if this.connection == connection && this.selected_key().as_ref() == Some(&key)
                    && this.controls.picker_tab.as_deref() == Some(TAB) && this.controls.providers.request == serial {
                    this.controls.providers.models = Some(result);
                    cx.notify();
                }
            });
        }).detach();
        cx.notify();
    }

    fn apply_provider(&mut self, cx: &mut Context<Self>) {
        if !self.controls_open() || self.controls.picker_tab.as_deref() != Some(TAB) { return; }
        let state = &self.controls.providers;
        let (Some((choice, model)), Some(source), Some(target), true) = (state.chosen(), state.source.clone(), self.selected_target(), state.confirming) else { return; };
        let valid = self.provider_choices().is_some_and(|choices| choices.contains(choice));
        let request = ClaudeProviderSwitch { source, engine:choice.engine.clone(), engine_account:choice.account.clone().unwrap_or_default(),
            engine_binding:choice.binding.clone(), model:model.to_owned() };
        if valid && self.switch_claude_provider(target, choice.label.clone(), request, cx) {
            self.controls.open = None;
        } else {
            self.controls.providers.notice = Some(tr("claude_provider_changed"));
        }
        cx.notify();
    }

    pub(super) fn render_claude_provider_picker(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        let state = &self.controls.providers;
        let current_label = self.selected.as_ref().and_then(|s| s.engine.as_deref()).and_then(|engine| {
            self.provider_choices()?.into_iter().find(|choice| choice.engine == engine).map(|choice| choice.label)
        });
        let footer = div().border_t_1().border_color(theme::border()).pt_2().px_2().flex().items_center().justify_between().gap_2()
            .child(div().min_w_0().truncate().text_xs().text_color(theme::muted()).child(tr("claude_provider_in_use").replace("{name}", current_label.as_deref().unwrap_or("Claude"))));
        let idle = self.selected.as_ref().is_some_and(|session| session.state == "idle");
        let notice = state.notice.as_ref().map(|text| div().px_2().pb_2().text_sm().text_color(theme::warning()).child(text.clone()));
        if state.confirming && let Some((choice, model)) = state.chosen() {
            let back = Button::new("claude-provider-back").ghost().small().label(tr("claude_provider_back"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.controls.providers.confirming = false;
                    this.controls.providers.notice = None;
                    this.ctl_search.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }));
            let cancel = Button::new("claude-provider-cancel").outline().small().label(tr("cancel"))
                .on_click(cx.listener(|this, _, _, cx| { this.controls.open = None; cx.notify(); }));
            let apply = Button::new("claude-provider-confirm").primary().small().label(tr("claude_provider_confirm")).disabled(busy || !idle)
                .on_click(cx.listener(|this, _, _, cx| this.apply_provider(cx)));
            return div().flex_1().min_w_0().flex().flex_col().justify_between().min_h(px(300.))
                .child(confirmation(&format!("{} · {}", choice.label, choice.detail), model).children(notice))
                .child(footer.child(div().flex().items_center().gap_2().child(back).child(cancel).child(apply)));
        }
        let choices = self.provider_choices().unwrap_or_default();
        let accounts = div().w(px(225.)).flex_shrink_0().flex().flex_col().gap_1().px_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("claude_providers")))
            .child(div().text_xs().text_color(theme::muted()).child(tr("claude_provider_choose_account")))
            .child(div().id("claude-provider-accounts").flex_1().min_h_0().overflow_y_scroll()
                .when(choices.is_empty(), |el| el.child(div().py_2().text_sm().text_color(theme::muted()).child(tr("claude_provider_empty"))))
                .children(choices.into_iter().map(|choice| {
                    let current = state.source.as_ref().is_some_and(|s| s.engine.as_deref() == Some(choice.engine.as_str()) && s.engine_binding.as_deref() == Some(choice.binding.as_str()));
                    let picked = state.account.as_ref() == Some(&choice);
                    account_row(&choice.engine, &choice.label, &choice.detail, picked, current, busy || choice.account.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| this.load_provider_models(choice.clone(), window, cx)))
                })));
        let mut models = div().flex_1().min_w_0().flex().flex_col().gap_1().border_l_1().border_color(theme::border()).px_2();
        if let Some(account) = &state.account {
            models = models.child(div().font_weight(FontWeight::SEMIBOLD).child(tr("claude_provider_models").replace("{name}", &account.label)))
                .child(Input::new(&self.ctl_search).appearance(false).aria_label(tr("picker_search")));
            models = match &state.models {
                None => models.child(popup::skeleton("claude-provider-models-loading", 3)),
                Some(Err(error)) => {
                    let account = account.clone();
                    models.child(div().text_sm().text_color(theme::warning()).child(error.clone()))
                        .child(Button::new("claude-provider-retry").ghost().small().label(tr("retry"))
                            .on_click(cx.listener(move |this, _, window, cx| this.load_provider_models(account.clone(), window, cx))))
                }
                Some(Ok(catalog)) => {
                    let query = self.ctl_search.read(cx).value().trim().to_lowercase();
                    let visible: Vec<_> = catalog.iter().filter(|model| model.to_lowercase().contains(&query)).collect();
                    models.child(div().id("claude-provider-models").flex_1().min_h_0().overflow_y_scroll()
                        .when(visible.is_empty(), |el| el.child(div().py_2().text_sm().text_color(theme::muted()).child(tr("claude_provider_no_models"))))
                        .children(visible.into_iter().map(|model| {
                            let model = model.clone();
                            model_row(&model, state.model.as_ref() == Some(&model))
                                .on_click(cx.listener(move |this, _, _, cx| { this.controls.providers.model = Some(model.clone()); cx.notify(); }))
                        })))
                }
            };
        } else {
            models = models.child(div().text_sm().text_color(theme::muted()).child(tr("claude_provider_choose_account")));
        }
        let apply = Button::new("claude-provider-review").primary().small().label(tr("claude_provider_apply")).disabled(busy || !idle || state.chosen().is_none())
            .on_click(cx.listener(|this, _, _, cx| { this.controls.providers.confirming = true; cx.notify(); }));
        div().flex_1().min_w_0().flex().flex_col().child(div().h(px(300.)).flex().child(accounts).child(models))
            .children(notice).child(footer.child(apply))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_model_must_be_present_in_the_selected_accounts_catalog() {
        let mut state = super::ProviderPicker {
            account: Some(super::ProviderChoice { engine:"fixture".into(), label:"Fixture".into(), detail:String::new(), account:Some("plus".into()), binding:"bound".into() }),
            models:Some(Ok(vec!["gpt-plus".into()])), ..Default::default()
        };
        assert!(state.chosen().is_none());
        state.model = Some("gpt-pro".into());
        assert!(state.chosen().is_none());
        state.model = Some("gpt-plus".into());
        assert_eq!(state.chosen().map(|(_, model)| model), Some("gpt-plus"));
        state.reset();
        assert!(state.chosen().is_none());
    }
}
