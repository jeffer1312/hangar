//! Configuração do controle do Windows no servidor conectado.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, Disclosure, Page};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{IndexPath, WindowExt, select::{Select, SelectEvent, SelectState}, searchable_list::SearchableListItem};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
struct AgentExe { path: String, exists: bool, size: u64 }

#[derive(Clone, Deserialize)]
struct ConfigFile { enabled: bool }

#[derive(Clone, Deserialize)]
struct Cliproxy { preset_url: String }

#[derive(Clone, Deserialize)]
struct ComputerTarget { name: String, path: String, transport: String, host: String }

impl SearchableListItem for ComputerTarget {
    type Value = String;
    fn title(&self) -> SharedString {
        format!("{} · {}", self.name, if self.transport == "local" { tr("computer_control_target_local") } else { self.host.clone() }).into()
    }
    fn value(&self) -> &String { &self.path }
}

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
    targets: Vec<ComputerTarget>,
    ssh_hosts: Vec<String>,
    local_available: bool,
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
    target_picker: Option<Entity<SelectState<Vec<ComputerTarget>>>>,
    picker_subscription: Option<Subscription>,
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
    target_dialog: Option<Entity<TargetDialog>>,
}

#[derive(Clone, Copy)]
pub(super) enum Write { Save, Install }

pub(super) enum ComputerReply {
    Loaded(u64, bool, Result<Value, Failure>),
    Written(u64, Write, Result<Value, Failure>),
    Target(EntityId, u64, TargetAction, Result<Value, Failure>),
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum TargetAction { Test, Setup, Create }

struct TargetDialog {
    owner: WeakEntity<Hangar>,
    connection: u64,
    project_dir: String,
    local_available: bool,
    ssh_hosts: Vec<String>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    proxy: Entity<InputState>,
    timeout: Entity<InputState>,
    ssh: bool,
    busy: Option<TargetAction>,
    seq: u64,
    test: Option<Result<(), String>>,
    setup: Option<String>,
    setup_error: Option<String>,
    copied: bool,
    error: Option<String>,
    setup_open: bool,
    setup_generation: u64,
    test_setup_generation: u64,
    advanced_open: bool,
    body_scroll: ScrollHandle,
    prompt_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl TargetDialog {
    fn new(owner: Entity<Hangar>, connection: u64, project_dir: String, local_available: bool,
        ssh_hosts: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("delphi-02"));
        let proxy = cx.new(|cx| InputState::new(window, cx));
        let timeout = cx.new(|cx| InputState::new(window, cx));
        let mut subscriptions = [&host, &proxy].map(|input| cx.subscribe_in(input, window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) { this.test = None; this.setup = None; this.copied = false; cx.notify(); }
            })).into_iter().collect::<Vec<_>>();
        subscriptions.push(cx.observe(&owner, |_, _, cx| cx.notify()));
        Self { owner: owner.downgrade(), connection, project_dir, local_available, ssh_hosts, name, host, proxy, timeout,
            ssh: true, busy: None, seq: 0, test: None, setup: None, setup_error: None, copied: false, error: None,
            setup_open: false, setup_generation: 0, test_setup_generation: 0, advanced_open: false,
            body_scroll: ScrollHandle::new(), prompt_scroll: ScrollHandle::new(), _subscriptions: subscriptions }
    }

    fn name(&self, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_owned();
        if !typed.is_empty() || !self.ssh { return typed; }
        self.host.read(cx).value().trim().rsplit('@').next().unwrap_or_default().to_lowercase()
            .chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') { c } else { '-' })
            .collect::<String>().trim_matches(['-', '.', '_']).chars().take(41).collect()
    }

    fn start(&mut self, action: TargetAction, cx: &mut Context<Self>) {
        if self.busy.is_some() { return; }
        let host = self.host.read(cx).value().trim().to_owned();
        if self.ssh && host.is_empty() && action != TargetAction::Setup { return; }
        let timeout = self.timeout.read(cx).value().trim().to_owned();
        let seconds = if action != TargetAction::Create || timeout.is_empty() { None } else { match timeout.parse::<u16>().ok().filter(|n| (1..=600).contains(n)) {
            Some(n) => Some(n), None => { self.error = Some(tr("computer_timeout_invalid")); cx.notify(); return; }
        }};
        let body = match action {
            TargetAction::Test => Some(json!({"host": host, "proxy_command": self.proxy.read(cx).value().trim()})),
            TargetAction::Setup => None,
            TargetAction::Create => Some(json!({"project_dir": self.project_dir, "name": self.name(cx),
                "transport": if self.ssh { "ssh" } else { "local" }, "host": host,
                "proxy_command": self.proxy.read(cx).value().trim(), "request_timeout": seconds})),
        };
        let me = cx.entity_id();
        let seq = self.seq + 1;
        let sent = self.owner.update(cx, |owner, _| {
            if owner.connection != self.connection { return false; }
            owner.request_computer_target(me, seq, action, body, host)
        }).unwrap_or(false);
        if !sent { self.error = Some(tr("settings_offline")); cx.notify(); return; }
        self.seq = seq;
        self.busy = Some(action);
        self.error = None;
        match action {
            TargetAction::Test => { self.test = None; self.test_setup_generation = self.setup_generation; },
            TargetAction::Setup => { self.setup = None; self.setup_error = None; self.copied = false; self.setup_open = true; },
            TargetAction::Create => {},
        }
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let disconnected = self.owner.upgrade().is_none_or(|owner| owner.read(cx).connection != self.connection);
        if self.busy.is_none() || disconnected {
            let me = cx.entity_id();
            let _ = self.owner.update(cx, |owner, _| {
                if owner.computer.target_dialog.as_ref().is_some_and(|dialog| dialog.entity_id() == me) {
                    owner.computer.target_dialog = None;
                }
            });
            window.close_dialog(cx);
        }
    }
}

impl Render for TargetDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy.is_some();
        if self.owner.upgrade().is_none_or(|owner| owner.read(cx).connection != self.connection) {
            return div().flex().flex_col().gap_3().child(tr("settings_offline"))
                .child(Button::new("computer-target-close-offline").outline().small().label(tr("close"))
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))).into_any_element();
        }
        let host = self.host.read(cx).value().trim().to_owned();
        let name = self.name(cx);
        let owner = cx.entity().downgrade();
        let mut body = div().flex().flex_col().gap_3()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr("computer_control_target_kind")))
            .child(div().flex().gap_2()
                .child(Radio::new("computer-target-ssh").label(tr("computer_control_target_ssh")).checked(self.ssh).disabled(busy)
                    .on_change(cx.listener(|this, _, _, cx| { this.ssh = true; this.test = None; cx.notify(); })))
                .child(Radio::new("computer-target-local").label(tr("computer_control_target_local")).checked(!self.ssh)
                    .disabled(busy || !self.local_available)
                    .on_change(cx.listener(|this, _, _, cx| { this.ssh = false; this.test = None; cx.notify(); }))));
        if !self.local_available { body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_local_hint"))); }
        if self.ssh {
            body = body.child(input_row("computer_control_target_host", &self.host, busy))
                .child(div().flex().items_center().gap_2()
                    .child(Button::new("computer-target-test").outline().small()
                        .label(tr(if self.busy == Some(TargetAction::Test) { "computer_control_target_testing" } else { "computer_control_target_test" }))
                        .loading(self.busy == Some(TargetAction::Test)).disabled(busy || host.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.start(TargetAction::Test, cx))))
                    .children(self.ssh_hosts.iter().filter(|h| h.as_str() != host).take(3).cloned().map(|suggestion| {
                        let selected = suggestion.clone();
                        Button::new(SharedString::from(format!("computer-host-{suggestion}"))).ghost().small().label(suggestion)
                            .disabled(busy).on_click(cx.listener(move |this, _, window, cx| {
                                this.host.update(cx, |input, cx| input.set_value(selected.clone(), window, cx));
                            }))
                    })))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_host_hint")));
            if let Some(test) = &self.test {
                body = body.child(div().id("computer-target-test-result").role(Role::Status).text_sm().whitespace_normal()
                    .text_color(if test.is_ok() { theme::text() } else { theme::danger() })
                    .child(match test { Ok(()) => tr("computer_control_target_test_ok"),
                        Err(detail) => tr("computer_control_target_test_fail").replace("{detail}", detail) }));
            }
            let open = self.setup_open;
            let toggle = owner.clone();
            body = body.child(Disclosure::new("computer-setup-toggle", open, tr("computer_control_setup_title"), false)
                .on_change(move |open, cx| { let _ = toggle.update(cx, |this, cx| {
                    this.setup_open = open; this.setup_generation += 1; cx.notify();
                }); }));
            if open {
                body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_setup_hint")));
                if let Some(prompt) = &self.setup {
                    let copy = prompt.clone();
                    body = body.child(scrolled("computer-setup-prompt", &self.prompt_scroll, 170.,
                        div().font_family(theme::MONO).text_xs()
                            .child(TextView::markdown("computer-setup-text", crate::conversation::fenced(prompt)).selectable(true).scrollable(false))))
                        .child(div().flex().gap_2()
                            .child(Button::new("computer-setup-copy").outline().small().label(tr(if self.copied { "computer_control_setup_copied" } else { "computer_control_setup_copy" }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())); this.copied = true; cx.notify();
                                })))
                            .child(Button::new("computer-setup-regenerate").ghost().small().label(tr("computer_control_setup_regenerate"))
                                .disabled(busy).on_click(cx.listener(|this, _, _, cx| this.start(TargetAction::Setup, cx)))));
                } else {
                    body = body.child(Button::new("computer-setup-generate").outline().small().label(tr("computer_control_setup_generate"))
                        .loading(self.busy == Some(TargetAction::Setup)).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.start(TargetAction::Setup, cx))));
                }
                if let Some(error) = &self.setup_error {
                    body = body.child(div().id("computer-setup-error").role(Role::Alert).text_sm()
                        .text_color(theme::danger()).whitespace_normal().child(error.clone()));
                }
            }
        }
        let advanced = self.advanced_open;
        let toggle = owner.clone();
        body = body.child(input_row("computer_control_target_name", &self.name, busy))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_name_hint")))
            .child(Disclosure::new("computer-target-advanced", advanced, tr("computer_control_target_advanced"), false)
                .on_change(move |open, cx| { let _ = toggle.update(cx, |this, cx| { this.advanced_open = open; cx.notify(); }); }));
        if advanced {
            if self.ssh { body = body.child(input_row("computer_control_target_proxy", &self.proxy, busy))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_proxy_hint"))); }
            body = body.child(input_row("computer_control_target_timeout", &self.timeout, busy))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("computer_control_target_timeout_hint")));
        }
        if let Some(error) = &self.error { body = body.child(div().id("computer-target-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone())); }
        div().id("computer-target-dialog").flex().flex_col().gap_3()
            .child(scrolled("computer-target-body", &self.body_scroll, 520., body))
            .child(div().flex().justify_end().gap_2()
                .child(Button::new("computer-target-cancel").ghost().small().label(tr("cancel")).disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
                .child(Button::new("computer-target-create").primary().small()
                    .label(tr(if self.busy == Some(TargetAction::Create) { "computer_control_target_creating" } else { "computer_control_target_create" }))
                    .loading(self.busy == Some(TargetAction::Create)).disabled(busy || name.is_empty() || (self.ssh && host.is_empty()))
                    .on_click(cx.listener(|this, _, _, cx| this.start(TargetAction::Create, cx))))).into_any_element()
    }
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
            ComputerForm { project_dir, agent_config, target_picker: None, picker_subscription: None, _subscriptions: subscriptions.into() }
        });
        form.project_dir.update(cx, |input, cx| input.set_value(state.project_dir.clone(), window, cx));
        form.agent_config.update(cx, |input, cx| input.set_value(state.agent_config.clone(), window, cx));
        Self::computer_picker(form, state, window, cx);
    }

    fn computer_picker(form: &mut ComputerForm, state: &ComputerState, window: &mut Window, cx: &mut Context<Self>) {
        form.target_picker = None;
        form.picker_subscription = None;
        if state.targets.is_empty() { return; }
        let current = form.agent_config.read(cx).value().to_string();
        let selected = state.targets.iter().position(|target| target.path == current)
            .map(IndexPath::new);
        let picker = cx.new(|cx| SelectState::new(state.targets.clone(), selected, window, cx));
        let subscription = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<ComputerTarget>>, window, cx| {
            if let SelectEvent::Confirm(Some(path)) = event {
                if let Some(form) = &this.computer.form {
                    form.agent_config.update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                }
                cx.notify();
            }
        });
        form.target_picker = Some(picker);
        form.picker_subscription = Some(subscription);
    }

    fn open_computer_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.computer.state.ok() else { return };
        let Some(form) = &self.computer.form else { return };
        if self.computer.busy.is_some() || self.computer.state.loading { return; }
        let project_dir = form.project_dir.read(cx).value().trim().to_owned();
        let (local_available, ssh_hosts) = (state.local_available, state.ssh_hosts.clone());
        let hangar = cx.entity();
        let owner = hangar.downgrade();
        let dialog = cx.new(|cx| TargetDialog::new(hangar, self.connection, project_dir, local_available, ssh_hosts, window, cx));
        self.computer.target_dialog = Some(dialog.clone());
        window.open_dialog(cx, move |surface, _, cx| {
            let busy = dialog.read(cx).busy.is_some();
            let (owner, me) = (owner.clone(), dialog.entity_id());
            surface.w(px(600.)).title(tr("computer_control_new_target")).child(dialog.clone())
                .keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(super::machines::enter_to_focused)
                .on_close(move |_, _, cx| { let _ = owner.update(cx, |this, _| {
                    if this.computer.target_dialog.as_ref().is_some_and(|d| d.entity_id() == me) { this.computer.target_dialog = None; }
                }); })
        });
        let host = self.computer.target_dialog.as_ref().unwrap().read(cx).host.clone();
        host.update(cx, |input, cx| input.focus(window, cx));
    }

    fn request_computer_target(&self, dialog: EntityId, seq: u64, action: TargetAction, body: Option<Value>, host: String) -> bool {
        let Some(api) = self.api.clone() else { return false };
        let done = self.computer_send_later();
        self.runtime.spawn(async move {
            let result = match action {
                TargetAction::Test => api.server_send(reqwest::Method::POST, &["computer-control", "test-host"], body, 25).await,
                TargetAction::Setup => api.server_read(&["computer-control", "windows-setup"], &[("host", &host)], 15).await,
                TargetAction::Create => api.server_send(reqwest::Method::POST, &["computer-control", "targets"], body, 25).await,
            };
            done(ComputerReply::Target(dialog, seq, action, result)).await;
        });
        true
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
                    else if let Some(form) = &mut self.computer.form { Self::computer_picker(form, state, window, cx); }
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
            ComputerReply::Target(id, seq, action, result) => {
                let Some(dialog) = self.computer.target_dialog.clone().filter(|d| d.entity_id() == id && d.read(cx).seq == seq) else { return };
                match action {
                    TargetAction::Test => {
                        let test = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| {
                            let ok = v.get("ok").and_then(Value::as_bool).ok_or_else(|| tr("invalid_response"))?;
                            if ok { Ok(()) } else { Err(v.get("detail").and_then(Value::as_str).unwrap_or_default().to_owned()) }
                        });
                        dialog.update(cx, |d, cx| {
                            d.busy = None;
                            if d.test_setup_generation == d.setup_generation { d.setup_open = test.is_err(); }
                            d.test = Some(test);
                            cx.notify();
                        });
                    }
                    TargetAction::Setup => {
                        let prompt = result.map_err(|e| Self::fetch_failure(&e))
                            .and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response")));
                        dialog.update(cx, |d, cx| { d.busy = None; match prompt { Ok(value) => d.setup = Some(value), Err(error) => d.setup_error = Some(error) }; cx.notify(); });
                    }
                    TargetAction::Create => {
                        let created = result.map_err(|e| Self::fetch_failure(&e)).and_then(parse_state);
                        match created {
                            Ok(state) => {
                                let name = dialog.read(cx).name(cx);
                                if let Some(path) = state.targets.iter().find(|target| target.name == name).map(|target| target.path.clone()) {
                                    self.computer.state.seq += 1;
                                    self.computer.state.loading = false;
                                    if let Some(form) = &mut self.computer.form {
                                        form.agent_config.update(cx, |input, cx| input.set_value(path, window, cx));
                                        Self::computer_picker(form, &state, window, cx);
                                    }
                                    self.computer.state.value = Some(Ok(state));
                                    self.computer.target_dialog = None;
                                    window.close_dialog(cx);
                                } else {
                                    dialog.update(cx, |d, cx| { d.busy = None; d.error = Some(tr("invalid_response")); cx.notify(); });
                                    self.load_computer(false, cx);
                                }
                            }
                            Err(error) => {
                                dialog.update(cx, |d, cx| { d.busy = None; d.error = Some(error); cx.notify(); });
                                self.load_computer(false, cx);
                            }
                        }
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
        let target = if let Some(picker) = &form.target_picker {
            div().flex().flex_col().gap_1().child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr("computer_control_target")))
                .child(Select::new(picker).small().disabled(busy || !self.computer.enabled).accessibility_label(tr("computer_control_target")))
        } else { input_row("computer_control_target", &form.agent_config, busy || !self.computer.enabled) };
        page = page.child(self.mark(div().flex().items_end().gap_2()
                .child(div().flex_1().min_w_0().child(target))
                .child(Button::new("computer-new-target").outline().small().label(tr("computer_control_new_target"))
                    .disabled(busy || !self.computer.enabled).on_click(cx.listener(|this, _, window, cx| this.open_computer_target(window, cx)))), "computer_control_target"))
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
