//! Sonho da memória: uma máquina escolhida relê toda noite as conversas novas de cada projeto e corrige a memória do
//! Claude. A escolha da máquina vale para todas as máquinas da pessoa, por isso vai a cada uma delas.
use super::*;
use super::super::git::{Kind as LineKind, line_row};
use super::super::vault::{alert, muted};
use crate::editdiff::{self, Op};
use gpui_kit::component::switch::Switch;
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};

/// Linhas do diff desenhadas por arquivo: uma memória enorme travaria a folha.
const LINES_MAX: usize = 400;

#[derive(Clone, Deserialize)]
struct Config {
    enabled: bool,
    #[serde(default)]
    api_key: String,
    hour: String,
    project_cap_usd: f64,
    month_cap_usd: Option<f64>,
    #[serde(default)]
    machine: String,
    #[serde(default)]
    user_memory: bool,
    #[serde(default)]
    this_machine: String,
}

#[derive(Clone, Deserialize)]
struct Project {
    id: String,
    name: String,
    #[serde(default)]
    machines: Vec<String>,
    #[serde(default)]
    new_sessions: u64,
    enabled: bool,
    mode: String,
    last_dream_at: Option<String>,
    #[serde(default)]
    last_cost_usd: f64,
    last_run_id: Option<String>,
    last_error: Option<String>,
    #[serde(default)]
    needs_confirm: bool,
}

#[derive(Clone, Default, Deserialize)]
struct Month {
    #[serde(default)]
    key_usd: f64,
    #[serde(default)]
    account_usd: f64,
}

#[derive(Clone, Deserialize)]
struct Suggestion {
    text: String,
    #[serde(default)]
    conflicts_with: Value,
}

#[derive(Clone, Deserialize)]
struct Status {
    projects: Vec<Project>,
    #[serde(default)]
    month: Month,
    running: Option<String>,
    #[serde(default)]
    suggestions: Vec<Suggestion>,
}

#[derive(Clone, Deserialize)]
struct RunDetail {
    projects: Vec<RunProject>,
}

#[derive(Clone, Deserialize)]
struct RunProject {
    project: String,
    places: Vec<Place>,
}

#[derive(Clone, Deserialize)]
struct Place {
    machine: String,
    root: String,
    #[serde(default)]
    before: BTreeMap<String, String>,
    #[serde(default)]
    after: BTreeMap<String, String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct UndoReply {
    results: Vec<UndoResult>,
}

#[derive(Deserialize)]
struct UndoResult {
    #[serde(default)]
    machine: String,
    ok: bool,
}

/// Uma máquina da pessoa: o id dela só se sabe pela leitura da configuração dela.
struct Target {
    label: String,
    active: bool,
    api: Option<Api>,
    config: Option<Config>,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Key,
    Hour,
    ProjectCap,
    MonthCap,
}

struct DreamPage {
    api: Api,
    runtime: Arc<Runtime>,
    targets: Vec<Target>,
    config: Option<Config>,
    status: Option<Status>,
    loading: bool,
    load_seq: u64,
    error: Option<String>,
    busy: HashSet<String>,
    field_error: Option<(Field, String)>,
    unconfirmed: Vec<String>,
    inputs: Vec<(Field, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

struct Changes {
    /// Nome de cada máquina pelo id, como a pessoa a conhece.
    labels: HashMap<String, String>,
    detail: Option<Result<Vec<RunProject>, String>>,
    /// Projeto, lugar e arquivo escolhidos para ver o antes e o depois.
    pick: Option<(usize, usize, String)>,
}

impl Hangar {
    pub(super) fn render_memory_dream(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return muted(tr("settings_offline")).into_any_element();
        };
        let (label, runtime) = (self.server_label(cx), self.runtime.clone());
        let this = &*self;
        // Vive enquanto a página é desenhada: reabrir a página relê tudo.
        let page = window.use_keyed_state(SharedString::from(format!("memory-dream-{}", api.identity())), cx, |window, cx| {
            DreamPage::new(api.clone(), runtime, this.dream_targets(label), window, cx)
        });
        page.update(cx, |page, cx| {
            // Outro token no mesmo endereço: o que foi lido com o anterior não vale.
            if !page.api.same_credential(&api) {
                page.api = api;
                page.load(window, cx);
            }
        });
        div().child(page).into_any_element()
    }

    /// As máquinas da pessoa (sem convite, desligada nem par externo); num servidor só sem lista, a ativa.
    fn dream_targets(&self, active_label: String) -> Vec<Target> {
        let own: Vec<&ServerEntry> = self.servers.iter().filter(|s| !s.invite && !s.disabled && !s.ephemeral).collect();
        if own.is_empty() {
            return vec![Target { label: active_label, active: true, api: self.api.clone(), config: None }];
        }
        own.into_iter()
            .map(|s| {
                let key = servers::norm(&s.address);
                Target {
                    label: s.label.clone(),
                    active: self.is_active_key(&key),
                    api: self.machine_api(&key).or_else(|| Api::new(&s.address, &s.token).ok()),
                    config: None,
                }
            })
            .collect()
    }
}

impl DreamPage {
    fn new(api: Api, runtime: Arc<Runtime>, targets: Vec<Target>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut inputs = Vec::new();
        let mut subscriptions = Vec::new();
        for field in [Field::Key, Field::Hour, Field::ProjectCap, Field::MonthCap] {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx);
                match field {
                    // Vira pontos, para a chave colada não sair em captura de tela.
                    Field::Key => state.masked(true),
                    Field::Hour => state.mask_pattern("99:99"),
                    _ => state.validate(|text, _| text.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')),
                }
            });
            subscriptions.push(cx.subscribe_in(&input, window, move |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit(field, window, cx);
                }
            }));
            inputs.push((field, input));
        }
        let mut page = Self {
            api,
            runtime,
            targets,
            config: None,
            status: None,
            loading: false,
            load_seq: 0,
            error: None,
            busy: HashSet::new(),
            field_error: None,
            unconfirmed: Vec::new(),
            inputs,
            _subscriptions: subscriptions,
        };
        page.load(window, cx);
        page.load_machines(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_seq += 1;
        let seq = self.load_seq;
        self.loading = true;
        self.error = None;
        let api = self.api.clone();
        run(
            &self.runtime,
            async move {
                let (config, status) =
                    futures::join!(api.server_read(&["dream", "config"], &[], 20), api.server_read(&["dream", "status"], &[], 20));
                Ok((parse::<Config>(config), parse::<Status>(status)))
            },
            window,
            cx,
            move |this, result: Result<_, Failure>, window, cx| {
                if seq != this.load_seq {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((Ok(config), Ok(status))) => {
                        if let Some(target) = this.targets.iter_mut().find(|t| t.active) {
                            target.config = Some(config.clone());
                        }
                        this.config = Some(config);
                        this.status = Some(status);
                        this.show_config(window, cx);
                        this.load_status(window, cx);
                    }
                    Ok((Err(error), _) | (_, Err(error))) | Err(error) => this.error = Some(Hangar::fetch_failure(&error)),
                }
            },
        );
        cx.notify();
    }

    /// O id de cada outra máquina, para o seletor; máquina fora do ar só fica de fora dele.
    fn load_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let apis: Vec<(usize, Api)> =
            self.targets.iter().enumerate().filter(|(_, t)| !t.active).filter_map(|(n, t)| t.api.clone().map(|api| (n, api))).collect();
        if apis.is_empty() {
            return;
        }
        run(
            &self.runtime,
            async move {
                Ok(futures::future::join_all(apis.into_iter().map(|(n, api)| async move {
                    (n, parse::<Config>(api.server_read(&["dream", "config"], &[], 10).await).ok())
                }))
                .await)
            },
            window,
            cx,
            |this, result: Result<Vec<(usize, Option<Config>)>, Failure>, window, cx| {
                for (n, config) in result.unwrap_or_default() {
                    if let Some(target) = this.targets.get_mut(n) {
                        target.config = config;
                    }
                }
                this.load_status(window, cx);
            },
        );
    }

    /// A máquina que sonha guarda o estado: projetos, rodadas, Sonhar agora e Desfazer vão para ela.
    /// Enquanto as outras máquinas não responderam, vale a ativa.
    fn dreamer_api(&self) -> Api {
        let machine = self.config.as_ref().map(|c| c.machine.as_str()).unwrap_or("");
        self.targets
            .iter()
            .find(|t| !machine.is_empty() && t.config.as_ref().is_some_and(|c| c.this_machine == machine))
            .and_then(|t| t.api.clone())
            .unwrap_or_else(|| self.api.clone())
    }

    /// Projetos da máquina que sonha, quando ela não é a ativa (a ativa já veio no `load`).
    fn load_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api = self.dreamer_api();
        if api.identity() == self.api.identity() {
            return;
        }
        let seq = self.load_seq;
        run(
            &self.runtime,
            async move { Ok(parse::<Status>(api.server_read(&["dream", "status"], &[], 20).await)) },
            window,
            cx,
            move |this, result: Result<_, Failure>, _, cx| {
                if seq != this.load_seq {
                    return;
                }
                match result {
                    Ok(Ok(status)) => this.status = Some(status),
                    Ok(Err(error)) | Err(error) => this.error = Some(Hangar::fetch_failure(&error)),
                }
                cx.notify();
            },
        );
    }

    fn input(&self, field: Field) -> &Entity<InputState> {
        &self.inputs.iter().find(|(f, _)| *f == field).expect("campo criado no new").1
    }

    /// Campos com o valor gravado, menos o que a pessoa está digitando agora.
    fn show_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.config.clone() else { return };
        for (field, input) in self.inputs.clone() {
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let value = match field {
                Field::Key => String::new(),
                Field::Hour => config.hour.clone(),
                Field::ProjectCap => number(config.project_cap_usd),
                Field::MonthCap => config.month_cap_usd.map(number).unwrap_or_default(),
            };
            input.update(cx, |state, cx| {
                if field == Field::Key {
                    let hint = if config.api_key.is_empty() { tr("server_secret_paste") } else { config.api_key.clone() };
                    state.set_placeholder(hint, window, cx);
                }
                if state.value() != value.as_str() {
                    state.set_value(value, window, cx);
                }
            });
        }
    }

    fn commit(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.config.clone() else { return };
        let text = self.input(field).read(cx).value().trim().to_owned();
        let invalid = |this: &mut Self, key: &str, cx: &mut Context<Self>| {
            this.field_error = Some((field, t(key)));
            cx.notify();
        };
        let body = match field {
            // Vazio mantém a chave gravada: o servidor só mostra a versão mascarada dela.
            Field::Key if text.is_empty() => return,
            Field::Key => json!({ "api_key": text }),
            Field::Hour if text == config.hour => return,
            Field::Hour if !valid_hour(&text) => return invalid(self, "dream_invalid_hour", cx),
            Field::Hour => json!({ "hour": text }),
            Field::MonthCap if text.is_empty() => {
                if config.month_cap_usd.is_none() {
                    return;
                }
                json!({ "month_cap_usd": null })
            }
            Field::ProjectCap | Field::MonthCap => {
                let Some(value) = cap(&text) else { return invalid(self, "dream_invalid_cap", cx) };
                let (key, current) = if field == Field::ProjectCap {
                    ("project_cap_usd", Some(config.project_cap_usd))
                } else {
                    ("month_cap_usd", config.month_cap_usd)
                };
                if current == Some(value) {
                    return;
                }
                json!({ key: value })
            }
        };
        self.save(body, Some(field), window, cx);
    }

    /// Grava na máquina da tela. Erro de campo fica ao lado dele; o do interruptor, num aviso.
    fn save(&mut self, body: Value, field: Option<Field>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert("config".into()) {
            return;
        }
        if field.is_some() {
            self.field_error = None;
        }
        let api = self.api.clone();
        run(
            &self.runtime,
            async move { parse::<Config>(api.server_send(reqwest::Method::PUT, &["dream", "config"], Some(body), 20).await) },
            window,
            cx,
            move |this, result, window, cx| {
                this.busy.remove("config");
                match result {
                    Ok(config) => {
                        if let Some(target) = this.targets.iter_mut().find(|t| t.active) {
                            target.config = Some(config.clone());
                        }
                        this.config = Some(config);
                        // A chave gravada sai do campo: Enter seguido do foco saindo a mandaria duas vezes.
                        if field == Some(Field::Key) {
                            this.input(Field::Key).clone().update(cx, |state, cx| state.set_value("", window, cx));
                        }
                        this.show_config(window, cx);
                    }
                    Err(error) => {
                        let text = match field {
                            Some(Field::Hour) if error.code.as_deref() == Some("dream_invalid_config") => t("dream_invalid_hour"),
                            Some(_) if error.code.as_deref() == Some("dream_invalid_config") => t("dream_invalid_cap"),
                            _ => failure_text(&error),
                        };
                        match field {
                            Some(field) => this.field_error = Some((field, text)),
                            None => window.push_notification(Notification::error(text), cx),
                        }
                    }
                }
            },
        );
        cx.notify();
    }

    /// A máquina que sonha vai a todas as máquinas: cada uma decide sozinha se é ela.
    fn choose_machine(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert("machine".into()) {
            return;
        }
        let sends: Vec<(usize, Api)> = self.targets.iter().enumerate().filter_map(|(n, t)| t.api.clone().map(|api| (n, api))).collect();
        self.unconfirmed = self.targets.iter().filter(|t| t.api.is_none()).map(|t| t.label.clone()).collect();
        run(
            &self.runtime,
            async move {
                Ok(futures::future::join_all(sends.into_iter().map(|(n, api)| {
                    let body = json!({ "machine": id });
                    async move { (n, parse::<Config>(api.server_send(reqwest::Method::PUT, &["dream", "config"], Some(body), 20).await)) }
                }))
                .await)
            },
            window,
            cx,
            |this, result: Result<Vec<(usize, Result<Config, Failure>)>, Failure>, window, cx| {
                this.busy.remove("machine");
                for (n, reply) in result.unwrap_or_default() {
                    match reply {
                        Ok(config) => this.targets[n].config = Some(config),
                        Err(_) => this.unconfirmed.push(this.targets[n].label.clone()),
                    }
                }
                this.load(window, cx);
            },
        );
        cx.notify();
    }

    /// Ação da pessoa no servidor: falha vira aviso; sucesso relê o estado.
    #[allow(clippy::too_many_arguments)]
    fn act(
        &mut self,
        busy: String,
        method: reqwest::Method,
        path: Vec<String>,
        query: Vec<(String, String)>,
        body: Option<Value>,
        done: impl FnOnce(&mut Self, Value, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.busy.insert(busy.clone()) {
            return;
        }
        let api = self.dreamer_api();
        run(
            &self.runtime,
            async move {
                let path: Vec<&str> = path.iter().map(String::as_str).collect();
                let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                api.server_send_query(method, &path, &query, body, 30).await
            },
            window,
            cx,
            move |this, result, window, cx| {
                this.busy.remove(&busy);
                match result {
                    Ok(value) => {
                        done(this, value, window, cx);
                        this.load(window, cx);
                    }
                    Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
                }
            },
        );
        cx.notify();
    }

    fn run_now(&mut self, project: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let body = project.as_ref().map_or_else(|| json!({}), |id| json!({ "project": id }));
        let busy = format!("run:{}", project.unwrap_or_default());
        self.act(busy, reqwest::Method::POST, path(&["dream", "run"]), Vec::new(), Some(body), |_, _, _, _| {}, window, cx);
    }

    fn update_project(&mut self, id: &str, body: Value, window: &mut Window, cx: &mut Context<Self>) {
        let query = vec![("id".to_owned(), id.to_owned())];
        self.act(format!("project:{id}"), reqwest::Method::PUT, path(&["dream", "project"]), query, Some(body), |_, _, _, _| {}, window, cx);
    }

    /// Desfaz só o projeto da linha: a rodada da noite cobre vários.
    fn undo(&mut self, run_id: String, project: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = path(&["dream", "runs", &run_id, "undo"]);
        let query = vec![("project".to_owned(), project.clone())];
        self.act(format!("undo:{run_id}:{project}"), reqwest::Method::POST, path, query, None, |this, value, window, cx| {
            let Ok(reply) = serde_json::from_value::<UndoReply>(value) else {
                window.push_notification(Notification::error(tr("invalid_response")), cx);
                return;
            };
            let failed: Vec<&UndoResult> = reply.results.iter().filter(|r| !r.ok).collect();
            if failed.is_empty() {
                window.push_notification(Notification::success(t("dream_undo_done")), cx);
            }
            for result in failed {
                let machine = this.machine_label(&result.machine);
                window.push_notification(Notification::warning(tr_shared("dream_undo_failed", &[("machine", &machine)])), cx);
            }
        }, window, cx);
    }

    fn dismiss(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let body = json!({ "index": index });
        self.act(format!("dismiss:{index}"), reqwest::Method::POST, path(&["dream", "suggestions", "dismiss"]), Vec::new(), Some(body),
            |_, _, _, _| {}, window, cx);
    }

    fn open_changes(&mut self, run_id: String, project: String, window: &mut Window, cx: &mut Context<Self>) {
        let (api, runtime) = (self.dreamer_api(), self.runtime.clone());
        let labels = self
            .targets
            .iter()
            .filter_map(|t| t.config.as_ref().map(|c| (c.this_machine.clone(), t.label.clone())))
            .collect();
        let changes = cx.new(|cx| Changes::new(api, runtime, run_id, project, labels, window, cx));
        window.open_dialog(cx, move |d, _, _| popup::dialog(d).title(t("dream_changes")).w(px(760.)).max_w_full().child(changes.clone()));
    }

    fn machine_label(&self, id: &str) -> String {
        self.targets
            .iter()
            .find(|t| t.config.as_ref().is_some_and(|c| c.this_machine == id))
            .map(|t| t.label.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    /// Dica da linha com o erro do campo logo abaixo, onde a pessoa está olhando.
    fn described(&self, hint: Option<&str>, field: Field) -> Div {
        let error = self.field_error.as_ref().filter(|(f, _)| *f == field).map(|(_, e)| e.clone());
        div().flex().flex_col().gap(px(2.)).children(hint.map(t)).children(error.map(|e| alert(SharedString::from("dream-field-error"), e)))
    }

    fn field_control(&self, field: Field, width: f32, label: &str) -> AnyElement {
        div()
            .w(px(width))
            .child(Input::new(self.input(field)).small().disabled(self.busy.contains("config")).aria_label(t(label)))
            .into_any_element()
    }

    fn machine_picker(&self, config: &Config, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .targets
            .iter()
            .find(|t| t.config.as_ref().is_some_and(|c| !c.this_machine.is_empty() && c.this_machine == config.machine))
            .map(|t| t.label.clone())
            .unwrap_or_else(|| if config.machine.is_empty() { "—".to_owned() } else { config.machine.clone() });
        let choices: Vec<(String, String)> = self
            .targets
            .iter()
            .filter_map(|t| t.config.as_ref().filter(|c| !c.this_machine.is_empty()).map(|c| (t.label.clone(), c.this_machine.clone())))
            .collect();
        let (weak, chosen) = (cx.entity().downgrade(), config.machine.clone());
        Button::new("dream-machine")
            .outline()
            .small()
            .max_w(px(220.))
            .accessibility_label(t("dream_machine"))
            .disabled(self.busy.contains("machine") || choices.is_empty())
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().min_w_0().truncate().child(current))
                    .child(chrome::small_icon(IconName::ChevronDown, 12., theme::muted())),
            )
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = sidebar::menu_style(menu).min_w(px(200.));
                for (label, id) in choices.clone() {
                    let (weak, on) = (weak.clone(), id == chosen);
                    menu = menu.item(PopupMenuItem::new(label).checked(on).on_click(move |_, window, cx| {
                        if !on {
                            let _ = weak.update(cx, |page, cx| page.choose_machine(id.clone(), window, cx));
                        }
                    }));
                }
                menu
            })
            .into_any_element()
    }

    /// "Quem paga": uma silhueta só, o escolhido com o fundo de destaque, como os segmentados das Configurações.
    fn mode_picker(&self, project: &Project, busy: bool, cx: &mut Context<Self>) -> Div {
        let modes = [("key", "dream_mode_key"), ("account", "dream_mode_account")];
        div().flex().rounded(px(6.)).border_1().border_color(theme::border_strong()).overflow_hidden().children(modes.iter().enumerate().map(
            |(n, &(mode, key))| {
                let (on, id) = (project.mode == mode, project.id.clone());
                Button::new(SharedString::from(format!("dream-mode-{}-{mode}", project.id)))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if on { theme::accent_dim() } else { transparent_black() })
                            .foreground(if on { theme::accent_text() } else { theme::muted() })
                            .hover(theme::hover())
                            .active(theme::hover()),
                    )
                    .small()
                    .h(px(28.))
                    .px(px(11.))
                    .rounded(px(0.))
                    .text_color(if on { theme::accent_text() } else { theme::muted() })
                    .when(on, |el| el.bg(theme::accent_dim()))
                    .when(n == 0, |el| el.border_r_1().border_color(theme::border_strong()))
                    .selected(on)
                    .disabled(busy)
                    .label(t(key))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !on {
                            this.update_project(&id, json!({ "mode": mode }), window, cx);
                        }
                    }))
            },
        ))
    }

    fn project_row(&self, project: &Project, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.contains(&format!("project:{}", project.id));
        let running = status.running.is_some() || self.busy.iter().any(|b| b.starts_with("run:"));
        let machines = project.machines.iter().map(|m| self.machine_label(m)).collect::<Vec<_>>().join(", ");
        let last = project
            .last_dream_at
            .as_deref()
            .map(|at| tr_shared("dream_last", &[("when", &local_time(at)), ("cost", &usd(project.last_cost_usd))]))
            .unwrap_or_else(|| t("dream_never"));
        let facts = [machines, tr_shared("dream_new_sessions", &[("n", &project.new_sessions.to_string())]), last]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let (toggle_id, run_id) = (project.id.clone(), project.id.clone());
        let enabled = project.enabled;
        let switch = Switch::new(SharedString::from(format!("dream-project-{}", project.id)))
            .checked(enabled)
            .disabled(busy)
            .accessibility_label(format!("{}, {}", t("dream_project_enabled"), project.name))
            .on_click(cx.listener(move |this, on: &bool, window, cx| this.update_project(&toggle_id, json!({ "enabled": *on }), window, cx)));
        // Projeto sem git é por pasta: só junta máquinas quando a pessoa diz qual é o mesmo.
        let pair = project.needs_confirm.then(|| {
            let others: Vec<(String, String)> = status
                .projects
                .iter()
                .filter(|p| p.id != project.id && p.id.starts_with("dir:"))
                .map(|p| (p.id.clone(), format!("{} · {}", p.name, p.machines.iter().map(|m| self.machine_label(m)).collect::<Vec<_>>().join(", "))))
                .collect();
            let (weak, id) = (cx.entity().downgrade(), project.id.clone());
            Button::new(SharedString::from(format!("dream-pair-{}", project.id)))
                .ghost()
                .small()
                .icon(IconName::Link)
                .label(t("dream_confirm_pair"))
                .tooltip(t("dream_confirm_pair_hint"))
                .disabled(busy || others.is_empty())
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let mut menu = sidebar::menu_style(menu).min_w(px(240.));
                    for (other, label) in others.clone() {
                        let (weak, id) = (weak.clone(), id.clone());
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |page, cx| page.update_project(&id, json!({ "alias_of": other }), window, cx));
                        }));
                    }
                    menu
                })
        });
        let last_run = project.last_run_id.clone();
        let changes = last_run.clone().map(|run| {
            let project = project.id.clone();
            Button::new(SharedString::from(format!("dream-changes-{}", project)))
                .ghost()
                .small()
                .icon(IconName::FileText)
                .label(t("dream_changes"))
                .on_click(cx.listener(move |this, _, window, cx| this.open_changes(run.clone(), project.clone(), window, cx)))
        });
        let undo = last_run.map(|run| {
            let project = project.id.clone();
            Button::new(SharedString::from(format!("dream-undo-{project}")))
                .ghost()
                .small()
                .icon(IconName::Undo2)
                .label(t("dream_undo"))
                .disabled(self.busy.contains(&format!("undo:{run}:{project}")))
                .on_click(cx.listener(move |this, _, window, cx| this.undo(run.clone(), project.clone(), window, cx)))
        });
        div()
            .id(SharedString::from(format!("dream-row-{}", project.id)))
            .mt(px(-1.))
            .border_t_1()
            .border_color(theme::border())
            .px_4()
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if enabled { theme::text() } else { theme::muted() })
                                    .truncate()
                                    .child(project.name.clone()),
                            )
                            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(facts)),
                    )
                    .child(switch),
            )
            .children(project.last_error.as_deref().map(|e| alert(SharedString::from(format!("dream-error-{}", project.id)), error_text(e))))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().text_xs().text_color(theme::muted()).child(t("dream_mode")))
                    .child(self.mode_picker(project, busy, cx))
                    .children(pair)
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("dream-run-{}", project.id)))
                            .outline()
                            .small()
                            .icon(IconName::Play)
                            .label(t("dream_run_now"))
                            .disabled(running || !enabled)
                            .on_click(cx.listener(move |this, _, window, cx| this.run_now(Some(run_id.clone()), window, cx))),
                    )
                    .children(changes)
                    .children(undo),
            )
            .into_any_element()
    }
}

impl Render for DreamPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(t("dream_title")))
            .child(muted(t("dream_lead")))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(8.))
                        .child(alert("dream-load-error", t("dream_load_error")))
                        .child(muted(error))
                        .child(
                            Button::new("dream-retry")
                                .outline()
                                .small()
                                .label(tr("retry"))
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                        ),
                )
            });
        let (Some(config), Some(status)) = (self.config.clone(), self.status.clone()) else {
            return page.when(self.error.is_none(), |el| el.child(div().id("dream-loading").role(Role::Status).child(muted(tr("loading")))));
        };
        let busy_config = self.busy.contains("config");
        let enabled = Switch::new("dream-enabled")
            .checked(config.enabled)
            .disabled(busy_config)
            .accessibility_label(t("dream_enabled"))
            .on_click(cx.listener(|this, on: &bool, window, cx| this.save(json!({ "enabled": *on }), None, window, cx)));
        let user_memory = Switch::new("dream-user-memory")
            .checked(config.user_memory)
            .disabled(busy_config)
            .accessibility_label(t("dream_user_memory"))
            .on_click(cx.listener(|this, on: &bool, window, cx| this.save(json!({ "user_memory": *on }), None, window, cx)));
        // Quem falhou ao gravar e quem ainda aponta para outra máquina: as duas não seguem a escolha.
        let mut unconfirmed = self.unconfirmed.clone();
        for target in &self.targets {
            if target.config.as_ref().is_some_and(|c| c.machine != config.machine) && !unconfirmed.contains(&target.label) {
                unconfirmed.push(target.label.clone());
            }
        }
        let machine_desc = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(t("dream_machine_hint"))
            .children(unconfirmed.iter().map(|label| div().text_color(theme::danger()).child(tr_shared("dream_machine_unconfirmed", &[("server", label)]))));
        let key_spent = match config.month_cap_usd {
            Some(cap) => format!("{}/{}", usd(status.month.key_usd), usd(cap)),
            None => usd(status.month.key_usd),
        };
        let spent = tr_shared("dream_month_value", &[("key", &key_spent), ("account", &usd(status.month.account_usd))]);
        let settings = settings_box()
            .child(setting_row(false, IconName::Moon, t("dream_enabled"), div().child(t("dream_enabled_hint")), true, enabled.into_any_element()))
            .child(setting_row(false, IconName::User, t("dream_user_memory"), div().child(t("dream_user_memory_hint")), true, user_memory.into_any_element()))
            .child(setting_row(false, IconName::Server, t("dream_machine"), machine_desc, true, self.machine_picker(&config, cx)))
            .child(setting_row(false, IconName::Key, t("dream_key"), self.described(Some("dream_key_hint"), Field::Key), true,
                self.field_control(Field::Key, 220., "dream_key")))
            .child(setting_row(false, IconName::Clock, t("dream_hour"), self.described(None, Field::Hour), true,
                self.field_control(Field::Hour, 88., "dream_hour")))
            .child(setting_row(false, IconName::Coins, t("dream_project_cap"), self.described(Some("dream_project_cap_hint"), Field::ProjectCap),
                true, self.field_control(Field::ProjectCap, 96., "dream_project_cap")))
            .child(setting_row(false, IconName::Wallet, t("dream_month_cap"), self.described(Some("dream_month_cap_hint"), Field::MonthCap),
                true, self.field_control(Field::MonthCap, 96., "dream_month_cap")))
            .child(setting_row(false, IconName::Wallet, t("dream_month_spent"), div(), true,
                div().text_sm().text_color(theme::text()).child(spent).into_any_element()));
        let running = status.running.is_some() || self.busy.iter().any(|b| b.starts_with("run:"));
        let actions = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("dream-refresh")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .accessibility_label(tr("reload"))
                    .tooltip(tr("reload"))
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
            )
            .child(
                Button::new("dream-run-all")
                    .primary()
                    .small()
                    .icon(IconName::Play)
                    .label(t(if status.running.is_some() { "dream_running" } else { "dream_run_now" }))
                    .disabled(running || status.projects.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.run_now(None, window, cx))),
            )
            .into_any_element();
        let projects = if status.projects.is_empty() {
            div()
                .py(px(40.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(chrome::small_icon(IconName::Folder, 28., theme::faint()))
                .child(muted(t("dream_projects_empty")))
        } else {
            settings_box().children(status.projects.iter().map(|p| self.project_row(p, &status, cx)))
        };
        let suggestions = (!status.suggestions.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .child(section_head(IconName::Lightbulb, t("dream_suggestions"), Some(t("dream_suggestions_hint")), None, px(0.)))
                .child(settings_box().children(status.suggestions.iter().enumerate().map(|(n, s)| {
                    let conflict = s.conflicts_with.as_str().filter(|c| !c.is_empty());
                    div()
                        .mt(px(-1.))
                        .border_t_1()
                        .border_color(theme::border())
                        .px_4()
                        .py(px(12.))
                        .flex()
                        .items_start()
                        .gap(px(14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(div().text_sm().whitespace_normal().child(s.text.clone()))
                                .children(conflict.map(|line| {
                                    div()
                                        .text_xs()
                                        .text_color(theme::muted())
                                        .whitespace_normal()
                                        .child(tr_shared("dream_suggestion_conflict", &[("line", line)]))
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("dream-dismiss-{n}")))
                                .ghost()
                                .small()
                                .label(t("dream_dismiss"))
                                .disabled(self.busy.contains(&format!("dismiss:{n}")))
                                .on_click(cx.listener(move |this, _, window, cx| this.dismiss(n, window, cx))),
                        )
                })))
        });
        page.child(settings)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(section_head(IconName::Folder, t("dream_projects"), None, Some(actions), px(0.)))
                    .child(projects),
            )
            .children(suggestions)
    }
}

impl Changes {
    fn new(api: Api, runtime: Arc<Runtime>, run_id: String, project: String, labels: HashMap<String, String>, window: &mut Window,
        cx: &mut Context<Self>) -> Self {
        run(
            &runtime,
            async move { parse::<RunDetail>(api.server_read(&["dream", "runs", &run_id], &[], 20).await) },
            window,
            cx,
            move |this, result, _, _| {
                // A rodada da noite traz todos os projetos; a linha mostra só o dela.
                this.detail = Some(
                    result.map_err(|e| failure_text(&e)).map(|detail| detail.projects.into_iter().filter(|p| p.project == project).collect()),
                );
            },
        );
        Self { labels, detail: None, pick: None }
    }
}

impl Render for Changes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div().id("dream-changes").max_h(px(560.)).overflow_y_scroll().flex().flex_col().gap(px(14.));
        let projects = match &self.detail {
            None => return body.child(div().id("dream-changes-loading").role(Role::Status).child(muted(tr("loading")))),
            Some(Err(error)) => return body.child(alert("dream-changes-error", error.clone())),
            Some(Ok(projects)) => projects.clone(),
        };
        body.children(projects.iter().enumerate().flat_map(|(pi, project)| {
            project.places.iter().enumerate().map(move |(li, place)| (pi, li, place.clone())).collect::<Vec<_>>()
        }).map(|(pi, li, place)| {
            let files = file_changes(&place);
            let picked = self.pick.as_ref().filter(|(p, l, _)| *p == pi && *l == li).map(|(_, _, f)| f.clone());
            let machine = self.labels.get(&place.machine).unwrap_or(&place.machine);
            let head = format!("{machine} · {}", place.root);
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).truncate().child(head))
                .children(place.error.clone().map(|e| alert(SharedString::from(format!("dream-place-error-{pi}-{li}")), error_text(&e))))
                .children(files.into_iter().map(|(file, key)| {
                    let on = picked.as_deref() == Some(file.as_str());
                    let chosen = file.clone();
                    Button::new(SharedString::from(format!("dream-file-{pi}-{li}-{file}")))
                        .ghost()
                        .small()
                        .justify_start()
                        .selected(on)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).child(file))
                                .child(div().text_xs().text_color(match key {
                                    "dream_file_added" => theme::success(),
                                    "dream_file_removed" => theme::removed(),
                                    _ => theme::muted(),
                                }).child(t(key))),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick = if on { None } else { Some((pi, li, chosen.clone())) };
                            cx.notify();
                        }))
                }))
                .children(picked.map(|file| {
                    let old = place.before.get(&file).map(String::as_str).unwrap_or_default();
                    let new = place.after.get(&file).map(String::as_str).unwrap_or_default();
                    div()
                        .id(SharedString::from(format!("dream-diff-{pi}-{li}")))
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme::border())
                        .children(editdiff::diff(old, new).into_iter().take(LINES_MAX).map(|line| {
                            let kind = match line.op {
                                Op::Add => LineKind::Add,
                                Op::Del => LineKind::Del,
                                Op::Same => LineKind::Context,
                            };
                            line_row(kind, line.old, line.new, SharedString::from(line.text), true)
                        }))
                }))
        }))
    }
}

fn t(key: &str) -> String {
    tr_shared(key, &[])
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

fn parse<T: DeserializeOwned>(value: Result<Value, Failure>) -> Result<T, Failure> {
    value.and_then(|v| serde_json::from_value(v).map_err(|_| Failure::local(tr("invalid_response"))))
}

/// Código do servidor ("dream_memory_changed: vps, notebook") na frase dele, com o resto como detalhe.
fn error_text(code: &str) -> String {
    let (head, rest) = code.split_once(':').map_or((code.trim(), ""), |(h, r)| (h.trim(), r.trim()));
    let key = format!("dream_error_{head}");
    let text = tr_shared(&key, &[("detail", rest)]);
    if text == key { tr_shared("dream_error_generic", &[("detail", code)]) } else { text }
}

fn failure_text(error: &Failure) -> String {
    match error.code.as_deref() {
        // A mensagem traz o detalhe (`dream_memory_changed: vps, notebook`); o código sozinho o perde.
        Some(code) if code.starts_with("dream_") => error_text(Some(error.detail.as_str()).filter(|d| d.starts_with(code)).unwrap_or(code)),
        _ => Hangar::fetch_failure(error),
    }
}

fn valid_hour(text: &str) -> bool {
    text.split_once(':').is_some_and(|(h, m)| {
        h.len() == 2 && m.len() == 2 && h.parse::<u8>().is_ok_and(|h| h < 24) && m.parse::<u8>().is_ok_and(|m| m < 60)
    })
}

fn cap(text: &str) -> Option<f64> {
    text.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.)
}

fn number(value: f64) -> String {
    format!("{value}")
}

fn usd(value: f64) -> String {
    format!("{value:.2}")
}

fn local_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|d| d.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string())
        .unwrap_or_else(|_| at.to_owned())
}

/// Arquivos que o sonho mexeu num lugar, com a etiqueta de cada um; os iguais ficam de fora.
fn file_changes(place: &Place) -> Vec<(String, &'static str)> {
    let names: BTreeSet<&String> = place.before.keys().chain(place.after.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| match (place.before.get(name), place.after.get(name)) {
            (None, Some(_)) => Some((name.clone(), "dream_file_added")),
            (Some(_), None) => Some((name.clone(), "dream_file_removed")),
            (Some(a), Some(b)) if a != b => Some((name.clone(), "dream_file_changed")),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Nomes explícitos: o `super::*` traz um `test` do gpui que encobre o `#[test]` padrão.
    use super::{cap, error_text, file_changes, tr_shared, valid_hour, Place};

    #[test]
    fn hour_and_cap_validation() {
        assert!(valid_hour("03:30") && valid_hour("23:59"));
        assert!(!valid_hour("24:00") && !valid_hour("3:30") && !valid_hour("03:6") && !valid_hour(""));
        assert_eq!(cap("2,5"), Some(2.5));
        assert_eq!(cap("0"), None);
        assert_eq!(cap("abc"), None);
    }

    #[test]
    fn error_codes_become_their_sentence_or_the_generic_one() {
        let known = error_text("dream_memory_changed: vps, notebook");
        assert!(known.contains("vps, notebook") && !known.starts_with("dream_"), "{known}");
        let unknown = error_text("dream_whatever: x");
        assert_eq!(unknown, tr_shared("dream_error_generic", &[("detail", "dream_whatever: x")]));
    }

    #[test]
    fn file_changes_skip_unchanged_files() {
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        let place = Place {
            machine: "vps".into(),
            root: "/r".into(),
            before: map(&[("a.md", "1"), ("b.md", "2"), ("c.md", "3")]),
            after: map(&[("a.md", "1"), ("b.md", "x"), ("d.md", "4")]),
            error: None,
        };
        assert_eq!(
            file_changes(&place),
            vec![("b.md".into(), "dream_file_changed"), ("c.md".into(), "dream_file_removed"), ("d.md".into(), "dream_file_added")]
        );
    }
}
