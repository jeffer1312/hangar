//! Configuração do controle do Windows no servidor conectado.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, Page};
use gpui_kit::component::checkbox::Checkbox;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
struct AgentExe { path: String, exists: bool, size: u64 }

#[derive(Clone, Deserialize)]
struct ConfigFile { enabled: bool }

#[derive(Clone, Deserialize)]
struct Cliproxy { preset_url: String }

#[derive(Clone, Deserialize)]
struct ComputerState {
    enabled: bool,
    mode: String,
    installed_tag: String,
    agent_exe: AgentExe,
    project_dir: String,
    agent_config: String,
    llm_url: String,
    llm_model: String,
    llm_effort: String,
    cliproxy: Cliproxy,
    files: Vec<ConfigFile>,
    #[serde(default)]
    migration_skipped: Vec<String>,
}

fn parse_state(value: Value) -> Result<ComputerState, String> {
    let state: ComputerState = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    if state.mode != "local" && state.mode != "package" { return Err(tr("invalid_response")); }
    Ok(state)
}

struct ComputerForm {
    project_dir: Entity<InputState>,
    agent_config: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(in crate::app) struct Computer {
    state: Remote<ComputerState>,
    form: Option<ComputerForm>,
    enabled: bool,
    busy: Option<Write>,
    write_seq: u64,
    error: Option<String>,
    note: Option<String>,
    migration_warning: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum Write { Save, Install }

pub(super) enum ComputerReply {
    Loaded(u64, bool, Result<Value, Failure>),
    Written(u64, Write, Result<Value, Failure>),
}

fn input_row(label: &str, input: &Entity<InputState>, disabled: bool) -> Div {
    div().flex().flex_col().gap_1()
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr(label)))
        .child(Input::new(input).disabled(disabled).aria_label(tr(label)))
}

impl Hangar {
    fn computer_send_later(&self) -> impl Fn(ComputerReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Computer(reply) }).await; })
        }
    }

    pub(super) fn computer_opened(&mut self, cx: &mut Context<Self>) {
        if self.computer.busy.is_none() {
            (self.computer.error, self.computer.note, self.computer.migration_warning) = (None, None, None);
            self.load_computer(true, cx);
        }
    }

    fn load_computer(&mut self, refill: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.computer.state.start();
        let done = self.computer_send_later();
        self.runtime.spawn(async move { done(ComputerReply::Loaded(seq, refill, api.server_read(&["computer-control"], &[], 10).await)).await });
        cx.notify();
    }

    fn fill_computer(&mut self, state: &ComputerState, window: &mut Window, cx: &mut Context<Self>) {
        self.computer.enabled = state.enabled;
        let form = self.computer.form.get_or_insert_with(|| {
            let project_dir = cx.new(|cx| InputState::new(window, cx));
            let agent_config = cx.new(|cx| InputState::new(window, cx));
            let subscriptions = [&project_dir, &agent_config].map(|input| cx.subscribe_in(input, window,
                |this: &mut Hangar, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) { this.save_computer(cx); }
                }));
            ComputerForm { project_dir, agent_config, _subscriptions: subscriptions.into() }
        });
        form.project_dir.update(cx, |input, cx| input.set_value(state.project_dir.clone(), window, cx));
        form.agent_config.update(cx, |input, cx| input.set_value(state.agent_config.clone(), window, cx));
    }

    fn save_computer(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(state), Some(form)) = (self.api.clone(), self.computer.state.ok(), self.computer.form.as_ref()) else { return };
        if self.computer.busy.is_some() || self.computer.state.loading { return; }
        let body = json!({
            "enabled": self.computer.enabled, "mode": state.mode,
            "project_dir": form.project_dir.read(cx).value().trim(),
            "agent_config": form.agent_config.read(cx).value().trim(),
            "llm_url": state.llm_url, "llm_model": state.llm_model, "llm_effort": state.llm_effort,
            "llm_key": null, "jev_key": null, "use_cliproxy_key": state.llm_url == state.cliproxy.preset_url,
        });
        self.write_computer(Write::Save, api, Some(body), cx);
    }

    fn install_computer(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.computer.busy.is_some() || self.computer.state.loading || self.computer.state.ok().is_none() { return; }
        self.write_computer(Write::Install, api, None, cx);
    }

    fn write_computer(&mut self, action: Write, api: Api, body: Option<Value>, cx: &mut Context<Self>) {
        self.computer.write_seq += 1;
        let seq = self.computer.write_seq;
        self.computer.state.seq += 1;
        (self.computer.busy, self.computer.error, self.computer.note, self.computer.migration_warning) = (Some(action), None, None, None);
        let done = self.computer_send_later();
        self.runtime.spawn(async move {
            let result = match action {
                Write::Save => api.server_send(reqwest::Method::PUT, &["computer-control"], body, 20).await,
                Write::Install => api.server_post(&["computer-control", "install"], 180).await,
            };
            done(ComputerReply::Written(seq, action, result)).await;
        });
        cx.notify();
    }

    pub(super) fn receive_computer(&mut self, reply: ComputerReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            ComputerReply::Loaded(seq, refill, result) => {
                if seq != self.computer.state.seq { return; }
                let result = result.map_err(|error| Self::fetch_failure(&error)).and_then(parse_state);
                if let Ok(state) = &result {
                    if refill || self.computer.form.is_none() { self.fill_computer(state, window, cx); }
                }
                self.computer.state.finish(seq, result);
            }
            ComputerReply::Written(seq, action, result) => {
                if seq != self.computer.write_seq { return; }
                self.computer.busy = None;
                match result.map_err(|error| Self::fetch_failure(&error)).and_then(parse_state) {
                    Ok(state) => {
                        self.computer.state.seq += 1;
                        self.computer.state.loading = false;
                        self.fill_computer(&state, window, cx);
                        self.computer.note = Some(match action {
                            Write::Install => tr("computer_control_installed").replace("{tag}", &state.installed_tag),
                            Write::Save if state.enabled => tr("computer_control_saved_on").replace("{n}", &state.files.iter().filter(|file| file.enabled).count().to_string()),
                            Write::Save => tr("computer_control_saved_off"),
                        });
                        if !state.migration_skipped.is_empty() {
                            self.computer.migration_warning = Some(tr("computer_control_migration_skipped")
                                .replace("{files}", &state.migration_skipped.join(", ")));
                        }
                        self.computer.state.value = Some(Ok(state));
                    }
                    Err(error) => {
                        self.computer.error = Some(error);
                        // A gravação pode ter chegado antes da queda; a releitura mostra o estado real.
                        self.load_computer(false, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_computer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut page = div().flex().flex_col().gap_4()
            .child(div().flex().items_center().gap_2()
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Windows.title()))
                .child(chip(tr("server_scope"), theme::muted(), theme::raised())))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_what")))
            .child(div().flex().flex_col().gap_1().children([
                "computer_control_step_tree", "computer_control_step_jev", "computer_control_step_llm", "computer_control_step_repeat",
            ].into_iter().enumerate().map(|(n, key)| div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(format!("{}. {}", n + 1, tr(key))))));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = &self.computer.error {
            page = page.child(div().id("computer-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        if self.computer.state.loading {
            return page.child(div().id("computer-loading").role(Role::Status).text_sm().text_color(theme::muted()).child(tr("loading"))).into_any_element();
        }
        let state = match &self.computer.state.value {
            Some(Ok(state)) => state,
            Some(Err(error)) => return page.child(div().id("computer-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).child(error.clone()))
                .child(div().flex().child(Button::new("computer-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.computer.error = None; this.load_computer(true, cx); })))).into_any_element(),
            None => return page.into_any_element(),
        };
        let Some(form) = &self.computer.form else { return page.into_any_element() };
        let busy = self.computer.busy.is_some();
        let package = state.mode == "package";
        let mode = if package { tr("computer_control_mode_package").replace("{tag}", &state.installed_tag) }
            else { tr("computer_control_mode_local").replace("{dir}", &state.project_dir) };
        let agent = if state.agent_exe.exists {
            tr("computer_control_agent_ok").replace("{path}", &state.agent_exe.path)
                .replace("{mb}", &format!("{:.1}", state.agent_exe.size as f64 / 1_048_576.))
        } else {
            tr(if package { "computer_control_agent_missing_package" } else { "computer_control_agent_missing_local" })
                .replace("{path}", &state.agent_exe.path)
        };
        page = page.child(self.mark(settings_box().p_3().gap_1()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(mode))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr(if package { "computer_control_mode_package_hint" } else { "computer_control_mode_local_hint" })))
            .child(div().id("computer-agent").role(if state.agent_exe.exists { Role::Status } else { Role::Alert })
                .text_sm().text_color(if state.agent_exe.exists { theme::muted() } else { theme::danger() })
                .whitespace_normal().child(agent))
            .child(div().flex().child(Button::new("computer-install").outline().small()
                .label(tr(if matches!(self.computer.busy, Some(Write::Install)) { "computer_control_installing" }
                    else if package { "computer_control_update" } else { "computer_control_install" }))
                .disabled(busy).on_click(cx.listener(|this, _, _, cx| this.install_computer(cx))))), "computer_control_install"))
            .child(self.mark(div().flex().flex_col().gap_1()
                .child(Checkbox::new("computer-enable").checked(self.computer.enabled).disabled(busy)
                    .label(tr("computer_control_enable"))
                    .on_change(cx.listener(|this, checked: &bool, _, cx| { this.computer.enabled = *checked; cx.notify(); })))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_enable_hint"))), "computer_control_enable"));
        if !package {
            page = page.child(self.mark(input_row("computer_control_dir", &form.project_dir, busy || !self.computer.enabled), "computer_control_dir"));
        }
        page = page.child(self.mark(input_row("computer_control_target", &form.agent_config, busy || !self.computer.enabled), "computer_control_target"))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_hint")))
            .child(div().flex().child(Button::new("computer-save").primary().small()
                .label(tr(if matches!(self.computer.busy, Some(Write::Save)) { "computer_control_saving" } else { "computer_control_save" }))
                .disabled(busy).on_click(cx.listener(|this, _, _, cx| this.save_computer(cx)))));
        if let Some(note) = &self.computer.note { page = page.child(div().id("computer-saved").role(Role::Status).text_sm().font_weight(FontWeight::SEMIBOLD).child(note.clone())); }
        if let Some(warning) = &self.computer.migration_warning {
            page = page.child(div().id("computer-migration-warning").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(warning.clone()));
        }
        page.into_any_element()
    }
}
