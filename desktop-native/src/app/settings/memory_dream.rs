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
    /// `None` = o servidor não conseguiu contar; o fato fica de fora.
    #[serde(default)]
    new_sessions: Option<u64>,
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

/// O andamento da rodada, como o `GET /api/dream/events` manda.
#[derive(Clone, Deserialize)]
struct Progress {
    run_id: String,
    started_at: String,
    last_activity_at: String,
    #[serde(default)]
    spent_usd: f64,
    #[serde(default)]
    stopping: bool,
    current: Option<usize>,
    projects: Vec<ProjectProgress>,
}

#[derive(Clone, Deserialize)]
struct ProjectProgress {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    calls_total: usize,
    #[serde(default)]
    calls_done: usize,
    #[serde(default)]
    cost_usd: f64,
    error: Option<String>,
    notes: Option<NoteCounts>,
}

#[derive(Clone, Copy, Deserialize)]
struct NoteCounts {
    new: usize,
    changed: usize,
    removed: usize,
}

#[derive(Clone, Deserialize)]
struct CallMeta {
    n: usize,
    kind: String,
    label: String,
    model: String,
    status: String,
    started_at: String,
    ended_at: Option<String>,
    cost_usd: Option<f64>,
    error: Option<String>,
    #[serde(default)]
    steps: usize,
}

#[derive(Clone, Deserialize)]
struct CallList {
    calls: Vec<CallMeta>,
}

#[derive(Clone, Deserialize)]
struct CallDetail {
    meta: CallMeta,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Step {
    Text { text: String },
    Tool { tool: String, target: Option<String>, detail: Option<String> },
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
    /// Código do servidor quando o lugar não voltou.
    #[serde(default)]
    error: Option<String>,
}

/// Uma máquina da pessoa: o id dela só se sabe pela leitura da configuração dela.
struct Target {
    label: String,
    active: bool,
    api: Option<Api>,
    config: Option<Config>,
    /// Por que a configuração dela não veio: sem isso ela pareceria só não ter sonho.
    error: Option<String>,
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
    /// A rodada no ar (pelo `events` da máquina que sonha) e a última que acabou com a página aberta.
    progress: Option<Progress>,
    finished: Option<Progress>,
    /// Endereço da máquina que o `events` acompanha e a tarefa que o lê.
    watching: Option<String>,
    stream: Option<tokio::task::JoinHandle<()>>,
    calls: Option<WeakEntity<Calls>>,
}

impl Drop for DreamPage {
    fn drop(&mut self) {
        if let Some(task) = self.stream.take() {
            task.abort();
        }
    }
}

/// "Como sonhou": as chamadas de um projeto numa rodada e o detalhe da escolhida.
struct Calls {
    api: Api,
    runtime: Arc<Runtime>,
    run_id: String,
    project: String,
    /// Rodada ainda no ar: a lista se refaz a cada andamento.
    live: bool,
    list: Option<Result<Vec<CallMeta>, String>>,
    pick: Option<usize>,
    detail: Option<Result<CallDetail, String>>,
    show_prompt: bool,
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
            return vec![Target { label: active_label, active: true, api: self.api.clone(), config: None, error: None }];
        }
        own.into_iter()
            .map(|s| {
                let key = servers::norm(&s.address);
                Target {
                    label: s.label.clone(),
                    active: self.is_active_key(&key),
                    api: self.machine_api(&key).or_else(|| Api::new(&s.address, &s.token).ok()),
                    config: None,
                    error: None,
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
            progress: None,
            finished: None,
            watching: None,
            stream: None,
            calls: None,
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
                        this.watch(window, cx);
                    }
                    Ok((Err(error), _) | (_, Err(error))) | Err(error) => this.error = Some(Hangar::fetch_failure(&error)),
                }
            },
        );
        cx.notify();
    }

    /// O id de cada outra máquina, para o seletor; máquina fora do ar fica de fora dele, com o motivo à vista.
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
                    (n, parse::<Config>(api.server_read(&["dream", "config"], &[], 10).await))
                }))
                .await)
            },
            window,
            cx,
            |this, result: Result<Vec<(usize, Result<Config, Failure>)>, Failure>, window, cx| {
                match result {
                    Ok(replies) => {
                        for (n, reply) in replies {
                            let Some(target) = this.targets.get_mut(n) else { continue };
                            (target.config, target.error) = match reply {
                                Ok(config) => (Some(config), None),
                                Err(error) => (None, Some(Hangar::fetch_failure(&error))),
                            };
                        }
                    }
                    Err(error) => {
                        let text = Hangar::fetch_failure(&error);
                        for target in this.targets.iter_mut().filter(|t| !t.active && t.api.is_some()) {
                            target.error = Some(text.clone());
                        }
                    }
                }
                this.load_status(window, cx);
                this.watch(window, cx);
                cx.notify();
            },
        );
    }

    /// Acompanha o andamento na máquina que sonha; trocar de máquina troca a conexão.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api = self.dreamer_api();
        if self.watching.as_deref() == Some(api.identity().as_str()) {
            return;
        }
        if let Some(task) = self.stream.take() {
            task.abort();
        }
        self.watching = Some(api.identity());
        let (tx, rx) = async_channel::unbounded::<Option<Progress>>();
        self.stream = Some(self.runtime.spawn(progress_stream(api, tx)));
        // A tarefa abortada solta o `tx`: o `recv` falha e este laço acaba junto.
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(progress) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.on_progress(progress, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_progress(&mut self, progress: Option<Progress>, window: &mut Window, cx: &mut Context<Self>) {
        let ended = progress.is_none() && self.progress.is_some();
        if ended {
            self.finished = self.progress.take();
        } else if progress.is_some() {
            self.finished = None;
        }
        self.progress = progress;
        if let Some(calls) = self.calls.as_ref().and_then(WeakEntity::upgrade) {
            let live = self.progress.as_ref().map(|p| p.run_id.clone());
            calls.update(cx, |calls, cx| calls.refresh(live.as_deref(), window, cx));
        }
        // Acabou: os projetos ganham a data, o custo e o erro novos.
        if ended {
            self.load(window, cx);
        }
        cx.notify();
    }

    /// O andamento do projeto na rodada no ar ou na que acabou de terminar.
    fn row_state(&self, id: &str) -> Option<&ProjectProgress> {
        self.progress.as_ref().or(self.finished.as_ref())?.projects.iter().find(|p| p.id == id)
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.act("stop".into(), reqwest::Method::POST, path(&["dream", "stop"]), Vec::new(), None, |_, _, _, _| {}, window, cx);
    }

    fn open_calls(&mut self, run_id: String, project: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.progress.as_ref().is_some_and(|p| p.run_id == run_id);
        // O id da rodada começa pela hora em UTC (`20261115-085314-5081`).
        let when = chrono::NaiveDateTime::parse_from_str(run_id_prefix(&run_id), "%Y%m%d-%H%M%S")
            .ok()
            .map(|t| t.and_utc().with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string());
        let (api, runtime) = (self.dreamer_api(), self.runtime.clone());
        let calls = cx.new(|cx| Calls::new(api, runtime, run_id, project, live, window, cx));
        self.calls = Some(calls.downgrade());
        let mut title = format!("{} · {name}", t("dream_calls_title"));
        if let Some(when) = when {
            title = format!("{title} · {}", tr_shared("dream_calls_run", &[("when", &when)]));
        }
        window.open_dialog(cx, move |d, _, _| popup::dialog(d).title(title.clone()).w(px(820.)).max_w_full().child(calls.clone()));
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
                        Ok(config) => (this.targets[n].config, this.targets[n].error) = (Some(config), None),
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
                let text = match &result.error {
                    Some(code) => format!("{machine}: {}", error_text(code)),
                    None => tr_shared("dream_undo_failed", &[("machine", &machine)]),
                };
                window.push_notification(Notification::warning(text), cx);
            }
        }, window, cx);
    }

    fn dismiss(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let body = json!({ "index": index });
        self.act(format!("dismiss:{index}"), reqwest::Method::POST, path(&["dream", "suggestions", "dismiss"]), Vec::new(), Some(body),
            |_, _, _, _| {}, window, cx);
    }

    /// `project` `None` = a rodada inteira (o bloco "Sonhou").
    fn open_changes(&mut self, run_id: String, project: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Bloco acima de Projetos: o sonho no ar (projeto, conversa, barra, gasto e Parar) ou como a última rodada terminou.
    fn progress_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(p) = &self.progress {
            let active: Vec<&ProjectProgress> = p.projects.iter().filter(|x| x.status != "skipped").collect();
            let current = p.current.and_then(|i| p.projects.get(i));
            let n = current.and_then(|c| active.iter().position(|x| x.id == c.id)).map_or(0, |i| i + 1);
            let title = tr_shared("dream_progress_running", &[("n", &n.to_string()), ("total", &active.len().to_string())]);
            let sub = current.map(progress_line).unwrap_or_default();
            let stop = Button::new("dream-stop")
                .outline()
                .small()
                .icon(IconName::Square)
                .label(t(if p.stopping { "dream_progress_stopping" } else { "dream_progress_stop" }))
                .disabled(p.stopping || self.busy.contains("stop"))
                .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)));
            let foot = tr_shared(
                "dream_progress_foot",
                &[("elapsed", &span(since(&p.started_at))), ("cost", &usd(p.spent_usd)), ("idle", &span(since(&p.last_activity_at)))],
            );
            let bar = div()
                .h(px(6.))
                .w_full()
                .rounded(px(3.))
                .bg(theme::hover())
                .overflow_hidden()
                .child(div().h_full().w(relative(fraction(p))).rounded(px(3.)).bg(theme::accent()));
            let card = card_head(pulse_dot("card", 10.), title, sub, stop.into_any_element())
                .child(bar)
                .child(div().text_xs().text_color(theme::muted()).child(foot));
            return Some(card.id("dream-progress").role(Role::Status).into_any_element());
        }
        let f = self.finished.as_ref()?;
        let active: Vec<&ProjectProgress> = f.projects.iter().filter(|x| x.status != "skipped").collect();
        let done = active.iter().filter(|x| x.status == "done").count();
        let first_error = active.iter().find_map(|x| x.error.clone());
        let card = match first_error.filter(|_| done == 0) {
            Some(error) => {
                let retry = Button::new("dream-retry-round")
                    .primary()
                    .small()
                    .label(t("dream_progress_retry"))
                    .disabled(self.busy.iter().any(|b| b.starts_with("run:")))
                    .on_click(cx.listener(|this, _, window, cx| this.run_now(None, window, cx)));
                let icon = chrome::small_icon(IconName::CircleX, 18., theme::danger()).into_any_element();
                card_head(icon, t("dream_progress_failed"), error_text(&error), retry.into_any_element())
            }
            None => {
                let run_id = f.run_id.clone();
                let changes = Button::new("dream-round-changes")
                    .outline()
                    .small()
                    .icon(IconName::FileText)
                    .label(t("dream_changes"))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_changes(run_id.clone(), None, window, cx)));
                let sub = tr_shared(
                    "dream_progress_done_sub",
                    &[
                        ("done", &done.to_string()),
                        ("total", &active.len().to_string()),
                        ("cost", &usd(f.spent_usd)),
                        ("duration", &span(between(&f.started_at, &f.last_activity_at))),
                    ],
                );
                let sub = match self.config.as_ref().filter(|c| c.enabled) {
                    Some(c) => format!("{sub} · {}", tr_shared("dream_progress_next", &[("hour", &c.hour)])),
                    None => sub,
                };
                let icon = chrome::small_icon(IconName::CircleCheck, 18., theme::success()).into_any_element();
                card_head(icon, tr_shared("dream_progress_done", &[("ago", &span(since(&f.last_activity_at)))]), sub, changes.into_any_element())
            }
        };
        Some(card.id("dream-progress").role(Role::Status).into_any_element())
    }

    fn project_row(&self, project: &Project, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.contains(&format!("project:{}", project.id));
        let running = self.progress.is_some() || status.running.is_some() || self.busy.iter().any(|b| b.starts_with("run:"));
        let machines = project.machines.iter().map(|m| self.machine_label(m)).collect::<Vec<_>>().join(", ");
        let live = self.row_state(&project.id).cloned();
        let sessions = project.new_sessions.map(|n| tr_shared("dream_new_sessions", &[("n", &n.to_string())])).unwrap_or_default();
        let joined = |parts: &[&str]| parts.iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
        let facts: AnyElement = match live.as_ref() {
            Some(p) if p.status == "queued" => joined(&[machines.as_str(), sessions.as_str(), t("dream_row_queued").as_str()]).into_any_element(),
            Some(p) if is_active(&p.status) => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(format!("{machines} ·"))
                .child(pulse_dot(&format!("row-{}", project.id), 7.))
                .child(div().text_color(theme::accent_text()).child(row_activity(p)))
                .into_any_element(),
            Some(p) if p.status == "done" => {
                let done = tr_shared("dream_row_done", &[("notes", &notes_summary(p.notes)), ("cost", &usd(p.cost_usd))]);
                joined(&[machines.as_str(), done.as_str()]).into_any_element()
            }
            _ => facts(machines, project.new_sessions, project.last_dream_at.as_deref().map(|at| (local_time(at), project.last_cost_usd)))
                .into_any_element(),
        };
        // Durante a rodada vale o erro dela; o `last_error` de antes seria velho.
        let error = match live.as_ref() {
            Some(p) => p.error.clone(),
            None => project.last_error.clone(),
        };
        let calls_run = match live.as_ref() {
            Some(p) if p.status != "queued" && p.status != "skipped" => self.progress.as_ref().or(self.finished.as_ref()).map(|r| r.run_id.clone()),
            _ => project.last_run_id.clone(),
        };
        let calls = calls_run.map(|run| {
            let (id, name) = (project.id.clone(), project.name.clone());
            Button::new(SharedString::from(format!("dream-calls-{}", project.id)))
                .ghost()
                .small()
                .icon(IconName::Activity)
                .label(t("dream_calls_open"))
                .on_click(cx.listener(move |this, _, window, cx| this.open_calls(run.clone(), id.clone(), name.clone(), window, cx)))
        });
        let (toggle_id, run_id) = (project.id.clone(), project.id.clone());
        let enabled = project.enabled;
        let switch = project_switch(&project.id, &project.name, enabled, busy)
            .on_click(cx.listener(move |this, on: &bool, window, cx| this.update_project(&toggle_id, json!({ "enabled": *on }), window, cx)));
        let (weak, mode_id) = (cx.entity().downgrade(), project.id.clone());
        let picker = mode_picker(&project.id, &project.mode, busy, Some(move |mode: &'static str, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.update_project(&mode_id, json!({ "mode": mode }), window, cx));
        }), cx);
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
            changes_button(&project)
                .on_click(cx.listener(move |this, _, window, cx| this.open_changes(run.clone(), Some(project.clone()), window, cx)))
        });
        let undo = last_run.map(|run| {
            let project = project.id.clone();
            undo_button(&project, self.busy.contains(&format!("undo:{run}:{project}")))
                .on_click(cx.listener(move |this, _, window, cx| this.undo(run.clone(), project.clone(), window, cx)))
        });
        let run = run_button(SharedString::from(format!("dream-run-{}", project.id)), false)
            .outline()
            .disabled(running || !enabled)
            .on_click(cx.listener(move |this, _, window, cx| this.run_now(Some(run_id.clone()), window, cx)));
        row_frame(&project.id)
            .child(row_head(project.name.clone(), enabled, facts, switch))
            .children(error.as_deref().map(|e| alert(SharedString::from(format!("dream-error-{}", project.id)), error_text(e))))
            .child(row_bar(picker, pair.map(IntoElement::into_any_element), run, changes, undo).children(calls))
            .into_any_element()
    }
}

// Partes da página que a animação da novidade (`news/memory_dream.rs`) também desenha: quem chama liga o clique.

/// O que se sabe do projeto numa linha: máquinas, conversas novas (quando contadas) e o último sonho (quando e quanto custou).
pub(crate) fn facts(machines: String, new_sessions: impl Into<Option<u64>>, last: Option<(String, f64)>) -> String {
    let last = last
        .map(|(when, cost)| tr_shared("dream_last", &[("when", &when), ("cost", &usd(cost))]))
        .unwrap_or_else(|| t("dream_never"));
    let sessions = new_sessions.into().map(|n| tr_shared("dream_new_sessions", &[("n", &n.to_string())])).unwrap_or_default();
    [machines, sessions, last]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn project_switch(project_id: &str, name: &str, enabled: bool, busy: bool) -> Switch {
    Switch::new(SharedString::from(format!("dream-project-{project_id}")))
        .checked(enabled)
        .disabled(busy)
        .accessibility_label(format!("{}, {}", t("dream_project_enabled"), name))
}

/// "Quem paga": uma silhueta só, o escolhido com o fundo de destaque, como os segmentados das Configurações.
/// Sem `on_pick` só é desenhado, fora da ordem do Tab.
pub(crate) fn mode_picker<F>(project_id: &str, mode: &str, busy: bool, on_pick: Option<F>, cx: &App) -> Div
where
    F: Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
{
    let modes = [("key", "dream_mode_key"), ("account", "dream_mode_account")];
    div().flex().rounded(px(6.)).border_1().border_color(theme::border_strong()).overflow_hidden().children(modes.iter().enumerate().map(
        |(n, &(choice, key))| {
            let on = mode == choice;
            Button::new(SharedString::from(format!("dream-mode-{project_id}-{choice}")))
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
                .map(|el| match on_pick.clone() {
                    Some(pick) => el.on_click(move |_, window, cx| {
                        if !on {
                            pick(choice, window, cx);
                        }
                    }),
                    None => el.tab_stop(false),
                })
        },
    ))
}

/// Sonhar agora; `running` troca o rótulo por Sonhando…. Quem chama escolhe a variante (a da seção é a principal).
pub(crate) fn run_button(id: impl Into<ElementId>, running: bool) -> Button {
    Button::new(id).small().icon(IconName::Play).label(t(if running { "dream_running" } else { "dream_run_now" }))
}

pub(crate) fn changes_button(project_id: &str) -> Button {
    Button::new(SharedString::from(format!("dream-changes-{project_id}"))).ghost().small().icon(IconName::FileText).label(t("dream_changes"))
}

pub(crate) fn undo_button(project_id: &str, busy: bool) -> Button {
    Button::new(SharedString::from(format!("dream-undo-{project_id}"))).ghost().small().icon(IconName::Undo2).label(t("dream_undo")).disabled(busy)
}

pub(crate) fn refresh_button(loading: bool) -> Button {
    Button::new("dream-refresh").ghost().small().icon(IconName::RefreshCw).accessibility_label(tr("reload")).tooltip(tr("reload")).disabled(loading)
}

/// Cabeçalho "Projetos" com as ações da seção (recarregar e sonhar todos). Sonhando, o bloco de andamento
/// acima toma o lugar do "Sonhar agora".
pub(crate) fn projects_head(refresh: Button, run_all: Option<Button>) -> Div {
    let actions = div().flex().items_center().gap_2().child(refresh).children(run_all).into_any_element();
    section_head(IconName::Folder, t("dream_projects"), None, Some(actions), px(0.))
}

pub(crate) fn row_frame(project_id: &str) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("dream-row-{project_id}")))
        .mt(px(-1.))
        .border_t_1()
        .border_color(theme::border())
        .px_4()
        .py(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
}

/// Nome do projeto e os fatos dele, com o interruptor à direita.
pub(crate) fn row_head(name: String, enabled: bool, facts: impl IntoElement, switch: Switch) -> Div {
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
                .child(div().font_weight(FontWeight::MEDIUM).text_color(if enabled { theme::text() } else { theme::muted() }).truncate().child(name))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(facts)),
        )
        .child(switch)
}

/// Barra de baixo da linha: quem paga à esquerda, as ações à direita.
pub(crate) fn row_bar(picker: Div, pair: Option<AnyElement>, run: Button, changes: Option<Button>, undo: Option<Button>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().text_xs().text_color(theme::muted()).child(t("dream_mode")))
        .child(picker)
        .children(pair)
        .child(div().flex_1())
        .child(run)
        .children(changes)
        .children(undo)
}

/// Um arquivo da lista "O que mudou", com a etiqueta (nova, alterada, removida) na cor dela.
pub(crate) fn file_button(id: impl Into<ElementId>, file: String, key: &str, picked: bool) -> Button {
    Button::new(id).ghost().small().justify_start().selected(picked).child(
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).child(file))
            .child(
                div()
                    .text_xs()
                    .text_color(match key {
                        "dream_file_added" => theme::success(),
                        "dream_file_removed" => theme::removed(),
                        _ => theme::muted(),
                    })
                    .child(t(key)),
            ),
    )
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
            .children(unconfirmed.iter().map(|label| div().text_color(theme::danger()).child(tr_shared("dream_machine_unconfirmed", &[("server", label)]))))
            .children(self.targets.iter().filter_map(|t| {
                t.error.as_ref().map(|e| div().text_color(theme::danger()).whitespace_normal().child(format!("{}: {e}", t.label)))
            }));
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
        let running = self.progress.is_some() || status.running.is_some() || self.busy.iter().any(|b| b.starts_with("run:"));
        let refresh = refresh_button(self.loading).on_click(cx.listener(|this, _, window, cx| this.load(window, cx)));
        let run_all = self.progress.is_none().then(|| {
            run_button("dream-run-all", status.running.is_some())
                .primary()
                .disabled(running || status.projects.is_empty())
                .on_click(cx.listener(|this, _, window, cx| this.run_now(None, window, cx)))
        });
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
            .children(self.progress_card(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(projects_head(refresh, run_all))
                    .child(projects),
            )
            .children(suggestions)
    }
}

impl Changes {
    fn new(api: Api, runtime: Arc<Runtime>, run_id: String, project: Option<String>, labels: HashMap<String, String>, window: &mut Window,
        cx: &mut Context<Self>) -> Self {
        run(
            &runtime,
            async move { parse::<RunDetail>(api.server_read(&["dream", "runs", &run_id], &[], 20).await) },
            window,
            cx,
            move |this, result, _, _| {
                // A rodada da noite traz todos os projetos; a linha mostra só o dela.
                this.detail = Some(result.map_err(|e| failure_text(&e)).map(|detail| {
                    detail.projects.into_iter().filter(|p| project.as_ref().is_none_or(|id| &p.project == id)).collect()
                }));
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
                    file_button(SharedString::from(format!("dream-file-{pi}-{li}-{file}")), file, key, on)
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

impl Calls {
    fn new(api: Api, runtime: Arc<Runtime>, run_id: String, project: String, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut calls = Self { api, runtime, run_id, project, live, list: None, pick: None, detail: None, show_prompt: false };
        calls.load_list(window, cx);
        calls
    }

    fn load_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (api, run_id, project) = (self.api.clone(), self.run_id.clone(), self.project.clone());
        run(
            &self.runtime,
            async move { parse::<CallList>(api.server_read(&["dream", "runs", &run_id, "calls"], &[("project", &project)], 20).await) },
            window,
            cx,
            |this, result, window, cx| match result {
                Ok(list) => {
                    // Sem escolha, abre a chamada que está trabalhando; senão a primeira.
                    if this.pick.is_none() {
                        this.pick = list.calls.iter().find(|c| c.status == "running").or(list.calls.first()).map(|c| c.n);
                        this.load_detail(window, cx);
                    }
                    this.list = Some(Ok(list.calls));
                }
                Err(error) => this.list = Some(Err(failure_text(&error))),
            },
        );
    }

    fn load_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(n) = self.pick else { return };
        let (api, run_id, project) = (self.api.clone(), self.run_id.clone(), self.project.clone());
        run(
            &self.runtime,
            async move {
                let n = n.to_string();
                parse::<CallDetail>(api.server_read(&["dream", "runs", &run_id, "calls", &n], &[("project", &project)], 20).await)
            },
            window,
            cx,
            move |this, result, _, _| {
                if this.pick == Some(n) {
                    this.detail = Some(result.map_err(|e| failure_text(&e)));
                }
            },
        );
    }

    /// Andamento novo: relê a lista e, se a escolhida ainda trabalha, o detalhe dela.
    fn refresh(&mut self, live_run: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        self.live = live_run == Some(self.run_id.as_str());
        let picked_running = self.detail.as_ref().and_then(|d| d.as_ref().ok()).is_none_or(|d| d.meta.status == "running");
        self.load_list(window, cx);
        if picked_running {
            self.load_detail(window, cx);
        }
    }

    fn choose(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.pick = Some(n);
        self.detail = None;
        self.show_prompt = false;
        self.load_detail(window, cx);
        cx.notify();
    }
}

impl Render for Calls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div().id("dream-calls").h(px(520.)).flex().gap(px(14.));
        let calls = match &self.list {
            None => return body.child(div().id("dream-calls-loading").role(Role::Status).child(muted(tr("loading")))),
            Some(Err(error)) => return body.child(alert("dream-calls-error", error.clone())),
            Some(Ok(calls)) => calls.clone(),
        };
        if calls.is_empty() && !self.live {
            return body.child(muted(t("dream_calls_empty")));
        }
        let labels: Vec<&str> = calls.iter().map(|c| c.label.as_str()).collect();
        // O organizador só começa depois dos analisadores: enquanto isso aparece esperando.
        let waiting = self.live && self.project != "user" && calls.iter().all(|c| c.kind == "analyzer");
        let rows: Vec<AnyElement> = calls
            .iter()
            .map(|c| {
                let on = self.pick == Some(c.n);
                let n = c.n;
                let sub = match c.status.as_str() {
                    "running" => tr_shared("dream_call_running", &[("elapsed", &span(since(&c.started_at)))]),
                    "failed" => c.error.as_deref().map(error_text).unwrap_or_default(),
                    _ => call_meta(c),
                };
                Button::new(SharedString::from(format!("dream-call-{n}")))
                    .ghost()
                    .w_full()
                    .h_auto()
                    .justify_start()
                    .selected(on)
                    .accessibility_label(call_title(&c.label, &labels))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(n, window, cx)))
                    .child(
                        div()
                            .w_full()
                            .py(px(4.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .text_sm()
                                    .text_color(theme::text())
                                    .child(status_mark(&c.status, &format!("call-{n}")))
                                    .child(call_title(&c.label, &labels)),
                            )
                            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().text_left().child(sub)),
                    )
                    .into_any_element()
            })
            .collect();
        let organizer = waiting.then(|| {
            div()
                .px_3()
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().flex().items_center().gap(px(7.)).text_sm().child(status_mark("waiting", "organizer")).child(t("dream_call_organizer")))
                .child(div().text_xs().text_color(theme::muted()).child(t("dream_call_waiting")))
        });
        let list = div()
            .id("dream-calls-list")
            .w(relative(0.38))
            .flex_shrink_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(rows)
            .children(organizer);
        let detail = match &self.detail {
            None if self.pick.is_some() => div().id("dream-call-loading").role(Role::Status).child(muted(tr("loading"))).into_any_element(),
            None => div().into_any_element(),
            Some(Err(error)) => alert("dream-call-error", error.clone()).into_any_element(),
            Some(Ok(detail)) => {
                let meta = &detail.meta;
                let head = match meta.status.as_str() {
                    "running" => div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_xs()
                        .text_color(theme::muted())
                        .child(pulse_dot("detail", 7.))
                        .child(format!("{} · {}", model_name(&meta.model), tr_shared("dream_call_running", &[("elapsed", &span(since(&meta.started_at)))]))),
                    _ => div().text_xs().text_color(theme::muted()).child(format!("{} · {}", model_name(&meta.model), call_meta(meta))),
                };
                let prompt_open = self.show_prompt;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(head)
                    .children(meta.error.as_deref().map(|e| alert("dream-call-failed", error_text(e))))
                    .children(detail.steps.iter().map(step_view))
                    .child(
                        Button::new("dream-call-prompt")
                            .ghost()
                            .small()
                            .justify_start()
                            .label(t("dream_call_prompt"))
                            .selected(prompt_open)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_prompt = !prompt_open;
                                cx.notify();
                            })),
                    )
                    .when(prompt_open, |el| el.child(code_box(detail.prompt.clone())))
                    .into_any_element()
            }
        };
        body.child(list)
            .child(div().w(px(1.)).h_full().bg(theme::border()))
            .child(div().id("dream-call-detail").flex_1().min_w_0().overflow_y_scroll().child(detail))
    }
}

/// Cabeçalho dos blocos de andamento: ícone no quadradinho, título, a linha de baixo e a ação à direita.
fn card_head(icon: AnyElement, title: String, sub: String, action: AnyElement) -> Div {
    settings_box().px_4().py(px(14.)).gap(px(10.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(
                div()
                    .size(px(36.))
                    .flex_shrink_0()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::inset())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(sub)),
            )
            .child(action),
    )
}

/// Bolinha que pulsa: algo está trabalhando agora.
fn pulse_dot(id: &str, size: f32) -> AnyElement {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme::accent())
        .with_animation(SharedString::from(format!("dream-pulse-{id}")), Animation::new(Duration::from_millis(1200)).repeat(), |el, t| {
            el.opacity(0.3 + 0.7 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos()))
        })
        .into_any_element()
}

/// Marca da chamada na lista: pulsando, feita, falhou ou esperando.
fn status_mark(status: &str, id: &str) -> AnyElement {
    match status {
        "running" => pulse_dot(id, 7.),
        "done" => chrome::small_icon(IconName::Check, 13., theme::success()).into_any_element(),
        "failed" => chrome::small_icon(IconName::X, 13., theme::danger()).into_any_element(),
        _ => chrome::small_icon(IconName::Circle, 11., theme::muted()).into_any_element(),
    }
}

fn step_view(step: &Step) -> AnyElement {
    match step {
        Step::Text { text } => div().text_sm().text_color(theme::text()).whitespace_normal().child(text.clone()).into_any_element(),
        Step::Tool { tool, target, detail } => {
            let (icon, verb) = match tool.as_str() {
                "Read" => (IconName::FileText, t("dream_call_read")),
                "Write" => (IconName::Pencil, t("dream_call_wrote")),
                "Edit" | "MultiEdit" => (IconName::Pencil, t("dream_call_edited")),
                "Glob" | "Grep" => (IconName::Search, t("dream_call_searched")),
                other => (IconName::Activity, other.to_owned()),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .text_xs()
                        .text_color(theme::muted())
                        .child(chrome::small_icon(icon, 12., theme::muted()))
                        .child(verb)
                        .children(target.clone().map(|target| {
                            div().px(px(5.)).rounded(px(4.)).bg(theme::inset()).font_family(theme::MONO).text_color(theme::text()).child(target)
                        })),
                )
                .children(detail.clone().map(code_box))
                .into_any_element()
        }
    }
}

fn code_box(text: String) -> Div {
    div()
        .rounded(px(8.))
        .border_1()
        .border_color(theme::border())
        .bg(theme::inset())
        .p(px(10.))
        .text_xs()
        .font_family(theme::MONO)
        .whitespace_normal()
        .child(text)
}

fn is_active(status: &str) -> bool {
    matches!(status, "reading" | "analyzing" | "organizing" | "writing")
}

/// Nome do projeto no andamento; o `user` é a passada da memória do usuário.
fn display_name(p: &ProjectProgress) -> String {
    if p.id == "user" { t("dream_progress_user") } else { p.name.clone() }
}

/// O que o projeto em andamento está fazendo, com o nome dele ("hangar · conversa 34 de 85").
fn progress_line(p: &ProjectProgress) -> String {
    let project = display_name(p);
    let analyzers = p.calls_total.saturating_sub(1);
    match p.status.as_str() {
        "reading" => tr_shared("dream_progress_reading", &[("project", &project)]),
        "analyzing" => tr_shared(
            "dream_progress_analyzing",
            &[("project", &project), ("done", &p.calls_done.min(analyzers).to_string()), ("total", &analyzers.to_string())],
        ),
        "organizing" => tr_shared("dream_progress_organizing", &[("project", &project)]),
        "writing" => tr_shared("dream_progress_writing", &[("project", &project)]),
        _ => project,
    }
}

/// A linha do projeto enquanto ele sonha: a contagem das conversas; nas outras fases, a fase.
fn row_activity(p: &ProjectProgress) -> String {
    let analyzers = p.calls_total.saturating_sub(1);
    match p.status.as_str() {
        "analyzing" => tr_shared("dream_row_running", &[("done", &p.calls_done.min(analyzers).to_string()), ("total", &analyzers.to_string())]),
        _ => progress_line(p),
    }
}

fn notes_summary(notes: Option<NoteCounts>) -> String {
    let Some(n) = notes else { return t("dream_notes_none") };
    let parts: Vec<String> = [("dream_notes_new", n.new), ("dream_notes_changed", n.changed), ("dream_notes_removed", n.removed)]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(key, count)| tr_shared(key, &[("n", &count.to_string())]))
        .collect();
    if parts.is_empty() { t("dream_notes_none") } else { parts.join(", ") }
}

/// Quanto da rodada andou: projetos acabados contam inteiros, o atual pela parte das chamadas.
fn fraction(p: &Progress) -> f32 {
    let active: Vec<&ProjectProgress> = p.projects.iter().filter(|x| x.status != "skipped").collect();
    if active.is_empty() {
        return 0.;
    }
    let sum: f32 = active
        .iter()
        .map(|x| match x.status.as_str() {
            "done" | "failed" | "stopped" => 1.,
            "queued" => 0.,
            _ if x.calls_total > 0 => x.calls_done as f32 / x.calls_total as f32,
            _ => 0.,
        })
        .sum();
    (sum / active.len() as f32).clamp(0., 1.)
}

fn since(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at).map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0)).unwrap_or(0)
}

fn run_id_prefix(run_id: &str) -> &str {
    run_id.get(..15).unwrap_or(run_id)
}

fn between(from: &str, to: &str) -> i64 {
    match (chrono::DateTime::parse_from_rfc3339(from), chrono::DateTime::parse_from_rfc3339(to)) {
        (Ok(a), Ok(b)) => (b - a).num_seconds().max(0),
        _ => 0,
    }
}

fn span(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// "6 passos · 42 s · US$ 0.04" de uma chamada que acabou.
fn call_meta(c: &CallMeta) -> String {
    let duration = c.ended_at.as_deref().map(|end| span(between(&c.started_at, end))).unwrap_or_default();
    tr_shared("dream_call_meta", &[("steps", &c.steps.to_string()), ("duration", &duration), ("cost", &usd(c.cost_usd.unwrap_or(0.)))])
}

/// Nome da chamada pelo arquivo que ela leu: `s3-p0.txt` é a conversa 4 (com a parte, quando ela tem mais de uma),
/// `memory-vps-1.txt` as notas daquela máquina.
fn call_title(label: &str, labels: &[&str]) -> String {
    match label {
        "organizer" => return t("dream_call_organizer"),
        "user" => return t("dream_progress_user"),
        _ => {}
    }
    if let Some(machine) = label.strip_prefix("memory-").and_then(|r| r.strip_suffix(".txt")).and_then(|r| r.rsplit_once('-')).map(|(m, _)| m) {
        return tr_shared("dream_call_notes_from", &[("machine", machine)]);
    }
    let parsed = label.strip_prefix('s').and_then(|r| r.strip_suffix(".txt")).and_then(|r| r.split_once("-p"));
    let Some((Ok(session), Ok(part))) = parsed.map(|(s, p)| (s.parse::<usize>(), p.parse::<usize>())) else { return label.to_owned() };
    let prefix = format!("s{session}-p");
    if labels.iter().filter(|l| l.starts_with(&prefix)).count() > 1 {
        tr_shared("dream_call_conversation_part", &[("n", &(session + 1).to_string()), ("part", &(part + 1).to_string())])
    } else {
        tr_shared("dream_call_conversation", &[("n", &(session + 1).to_string())])
    }
}

/// `claude-haiku-5-5` → `Haiku`.
fn model_name(model: &str) -> String {
    let family = model.split('-').nth(1).unwrap_or(model);
    let mut chars = family.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// Lê o `events` do sonho e manda cada andamento; caiu, volta com espera crescente.
async fn progress_stream(api: Api, tx: async_channel::Sender<Option<Progress>>) {
    use eventsource_stream::Eventsource;
    use futures::StreamExt;
    let mut delay = 1u64;
    loop {
        crate::api::route::ensure(&api).await;
        if let Ok(response) = api.server_events(&["dream", "events"]).await {
            delay = 1;
            // O servidor manda um comentário a cada 10 s, que o leitor de eventos não repassa: o prazo fica nos bytes.
            let bytes = futures::stream::unfold(Box::pin(response.bytes_stream()), |mut stream| async move {
                match tokio::time::timeout(Duration::from_secs(25), stream.next()).await {
                    Ok(Some(chunk)) => Some((chunk, stream)),
                    _ => None,
                }
            });
            let mut events = Box::pin(bytes.eventsource());
            while let Some(Ok(item)) = events.next().await {
                if item.event != "progress" {
                    continue;
                }
                let Ok(progress) = serde_json::from_str::<Option<Progress>>(&item.data) else { continue };
                if tx.send(progress).await.is_err() {
                    return;
                }
            }
            crate::api::route::forget(&api);
        }
        if tx.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
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
    use super::{
        call_title, cap, error_text, facts, file_changes, fraction, model_name, notes_summary, span, t, tr_shared, valid_hour, NoteCounts, Place,
        Progress, ProjectProgress,
    };

    #[test]
    fn calls_are_named_by_what_they_read() {
        let labels = ["s0-p0.txt", "s1-p0.txt", "s1-p1.txt", "memory-vps-2.txt", "organizer"];
        assert_eq!(call_title("s0-p0.txt", &labels), tr_shared("dream_call_conversation", &[("n", "1")]));
        assert_eq!(call_title("s1-p1.txt", &labels), tr_shared("dream_call_conversation_part", &[("n", "2"), ("part", "2")]));
        assert_eq!(call_title("memory-vps-2.txt", &labels), tr_shared("dream_call_notes_from", &[("machine", "vps")]));
        assert_eq!(call_title("organizer", &labels), t("dream_call_organizer"));
        assert_eq!(call_title("estranho", &labels), "estranho");
        assert_eq!(model_name("claude-haiku-5-5"), "Haiku");
    }

    #[test]
    fn spans_notes_and_bar() {
        assert_eq!((span(8), span(660), span(3900)), ("8 s".into(), "11 min".into(), "1 h 5 min".into()));
        assert_eq!(notes_summary(Some(NoteCounts { new: 0, changed: 0, removed: 0 })), t("dream_notes_none"));
        let both = notes_summary(Some(NoteCounts { new: 2, changed: 1, removed: 0 }));
        assert!(both.contains('2') && both.contains('1'), "{both}");
        let project = |status: &str, done: usize| ProjectProgress {
            id: status.into(),
            name: status.into(),
            status: status.into(),
            calls_total: 4,
            calls_done: done,
            cost_usd: 0.,
            error: None,
            notes: None,
        };
        let progress = Progress {
            run_id: "r".into(),
            started_at: String::new(),
            last_activity_at: String::new(),
            spent_usd: 0.,
            stopping: false,
            current: Some(1),
            projects: vec![project("done", 4), project("analyzing", 2), project("queued", 0), project("skipped", 0)],
        };
        assert!((fraction(&progress) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unknown_session_count_is_left_out() {
        let counted = tr_shared("dream_new_sessions", &[("n", "3")]);
        assert_eq!(facts("vps".into(), 3u64, None), format!("vps · {counted} · {}", t("dream_never")));
        assert_eq!(facts("vps".into(), None::<u64>, None), format!("vps · {}", t("dream_never")));
    }

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
