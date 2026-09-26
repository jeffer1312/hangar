//! Controles da sessão no compositor (modelo, esforço, modo, permissão) e os pedidos de plano que
//! mudam o modo. Catálogo vem das rotas de cada provider no gesto; nada é lido do terminal ao montar.
use super::*;

const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
const CLAUDE_MODES: [&str; 6] = ["plan", "auto", "manual", "acceptEdits", "bypassPermissions", "dontAsk"];
// Acima disto a lista ganha busca e só as linhas visíveis são montadas (catálogo do Pi/OMP tem dezenas).
const LONG_LIST: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Ctl { Model, Effort, Mode, Permission }

impl Ctl {
    fn key(self) -> &'static str {
        match self { Ctl::Model => "model", Ctl::Effort => "effort", Ctl::Mode => "mode", Ctl::Permission => "permission" }
    }
}

struct Choice { label: String, detail: String, path: Vec<&'static str>, body: Value, current: bool, enabled: bool }

#[derive(Debug, PartialEq)]
pub(super) enum PlanOutcome {
    ModeFailed(String),
    // Pedido não entrou; `reverted` diz se o modo plano voltou.
    SendFailed { reason: String, uncertain: bool, reverted: bool },
    Sent(bool),
}

#[derive(Clone)]
pub(super) struct Open { key: SessionKey, ctl: Ctl, catalog: Option<Result<Value, String>> }

impl Open {
    /// Id do gatilho do painel, a pílula do controle.
    pub(super) fn anchor(&self) -> String { format!("ctl-{}", self.ctl.key()) }
}

#[derive(Default)]
pub(super) struct Controls {
    open: Option<Open>,
    busy: HashMap<SessionKey, Ctl>,
    // Rótulo aplicado com sucesso e o valor da fonte ao vivo naquele momento: vale até a fonte mudar.
    applied: HashMap<(SessionKey, Ctl), (String, Option<String>)>,
    // Ciclo do Claude com terminal (lido da rodapé do pane) e permissão do Codex, por sessão.
    known: HashMap<(SessionKey, Ctl), Value>,
    // Plano do Claude com terminal: metadados achados e o conteúdo aberto pelo gesto.
    plan: Option<(SessionKey, Option<Result<Value, String>>, Option<Result<Value, String>>)>,
    plans_done: HashSet<String>,
    planning: HashSet<SessionKey>,
    preselect: Option<SessionKey>,
    // Linha sob o teclado; `None` é a atual (ou a primeira livre) até ↑↓ ou o ponteiro moverem.
    highlight: Option<usize>,
    scroll: ScrollHandle,
    // A mesma rolagem na lista longa, que é virtualizada.
    long: UniformListScrollHandle,
    // O painel toma o foco ao abrir: ↑↓ Enter chegam a ele, e o Esc sobe até o braço da raiz.
    focus: Option<(FocusHandle, Subscription)>,
    // A lista rola até a linha destacada uma vez por abertura, no primeiro desenho com ela.
    revealed: std::cell::Cell<bool>,
}

impl Controls {
    pub fn on_select(&mut self) { self.open = None; self.plan = None; }
    pub fn clear_plan_preview(&mut self) { self.plan = None; }
}

// Modo conhecido aparece traduzido; valor que o backend inventar depois aparece cru.
fn shown(ctl: Ctl, value: String) -> String {
    match ctl {
        Ctl::Mode if CLAUDE_MODES.contains(&value.as_str()) => tr(&format!("mode_{value}")),
        Ctl::Mode if value == "default" => tr("codex_mode_default"),
        _ => value,
    }
}

fn capitalized(label: &str) -> String {
    let mut chars = label.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn text(value: &Value, key: &str) -> String { value.get(key).and_then(Value::as_str).unwrap_or("").to_owned() }

// A única linha com o nome exato; nome repetido entre providers não diz qual é a atual.
fn only_match(names: &[String], current: &str) -> Option<usize> {
    let current = current.trim();
    if current.is_empty() { return None; }
    let mut hits = names.iter().enumerate().filter(|(_, n)| n.trim().eq_ignore_ascii_case(current)).map(|(i, _)| i);
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

// Porta de `matchCurrent` do web: a linha de status vence o `active`, que envelhece no picker.
fn claude_model_current(models: &[(String, bool)], status: &str) -> Option<usize> {
    let active = || models.iter().position(|(_, a)| *a);
    let status = status.trim().to_lowercase();
    if status.is_empty() { return active(); }
    if let Some(i) = models.iter().position(|(id, _)| id.to_lowercase() == status) { return Some(i); }
    let million = status.contains("1m");
    let base = |id: &str| {
        let id = id.to_lowercase();
        match id.rfind('[') { Some(p) if id.ends_with(']') => id[..p].to_owned(), _ => id }
    };
    let candidates: Vec<usize> = models.iter().enumerate()
        .filter(|(_, (id, _))| id != "default" && !base(id).is_empty() && status.contains(&base(id))).map(|(i, _)| i).collect();
    candidates.iter().copied().find(|&i| models[i].0.to_lowercase().ends_with("[1m]") == million)
        .or_else(|| candidates.first().copied()).or_else(active)
}

// Exato, senão a abreviação da linha de status (`med` → medium).
fn claude_effort_current(status: &str) -> Option<&'static str> {
    let status = status.trim().to_lowercase();
    if status.is_empty() { return None; }
    CLAUDE_EFFORTS.iter().find(|e| **e == status).or_else(|| CLAUDE_EFFORTS.iter().find(|e| e.starts_with(&status))).copied()
}

// Como o web, sem caixa, sobre `origem/id nome`: acha pelo nome, pelo provider e pelo id que a statusline mostra.
fn keep(choices: Vec<Choice>, query: &str) -> Vec<Choice> {
    let query = query.trim().to_lowercase();
    if query.is_empty() { return choices; }
    choices.into_iter().filter(|c| {
        let model = c.body.get("model").and_then(Value::as_str).unwrap_or_default();
        format!("{}/{} {}", c.detail, model, c.label).to_lowercase().contains(&query)
    }).collect()
}

/// Busca do painel de controles: cada letra recomeça o destaque na lista filtrada.
pub(super) fn search_field(window: &mut Window, cx: &mut Context<Hangar>) -> Entity<InputState> {
    let search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("ctl_search")));
    cx.subscribe(&search, |this: &mut Hangar, _, event: &InputEvent, cx| {
        if !matches!(event, InputEvent::Change) { return; }
        this.controls.highlight = None;
        this.controls.revealed.set(false);
        cx.notify();
    }).detach();
    search
}

// ↑↓ andam só pelas linhas livres e dão a volta nas pontas; de fora da lista, o sentido escolhe a ponta.
fn step_free(free: &[bool], now: usize, step: isize) -> Option<usize> {
    let free: Vec<usize> = free.iter().enumerate().filter(|(_, f)| **f).map(|(n, _)| n).collect();
    let last = *free.last()?;
    Some(match free.iter().position(|&n| n == now) {
        Some(at) => free[(at as isize + step).rem_euclid(free.len() as isize) as usize],
        None if step > 0 => free[0],
        None => last,
    })
}

impl Hangar {
    pub(super) fn controls_open(&self) -> bool { self.controls.open.is_some() }
    pub(super) fn close_controls(&mut self) { self.controls.open = None; }
    pub(super) fn ctl_snapshot(&self) -> Option<Open> { self.controls.open.clone() }

    /// Desenha o painel de `open` mesmo já fechado: a saída animada mostra o que estava na tela.
    pub(super) fn render_ctl_panel_for(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let live = self.controls.open.replace(open);
        let panel = self.render_ctl_panel(window, cx);
        self.controls.open = live;
        panel
    }

    // Quais controles cada provider tem; o Claude esconde o esforço no Haiku, como o web.
    fn ctl_list(&self) -> Vec<Ctl> {
        match self.provider().0 {
            "claude" => {
                let haiku = self.ctl_label(Ctl::Model).is_some_and(|m| m.to_lowercase().contains("haiku"));
                [Some(Ctl::Model), (!haiku).then_some(Ctl::Effort), Some(Ctl::Mode)].into_iter().flatten().collect()
            }
            "codex" => vec![Ctl::Model, Ctl::Effort, Ctl::Mode, Ctl::Permission],
            "pi" | "omp" | "kimi" => vec![Ctl::Model, Ctl::Effort],
            _ => Vec::new(),
        }
    }

    fn ctl_label(&self, ctl: Ctl) -> Option<String> {
        let key = self.selected_key()?;
        let live = self.ctl_live(ctl);
        match self.controls.applied.get(&(key, ctl)) {
            Some((applied, before)) if *before == live => Some(applied.clone()),
            _ => live,
        }
    }

    fn ctl_live(&self, ctl: Ctl) -> Option<String> {
        let key = self.selected_key()?;
        let status = self.status();
        let from_status = match ctl {
            Ctl::Model => status.as_ref().and_then(|s| s.model.clone()),
            Ctl::Effort => status.as_ref().and_then(|s| s.effort.clone()),
            Ctl::Mode if self.provider().0 == "codex" => self.chat.state.codex_mode.clone(),
            Ctl::Mode => self.chat.state.claude_permission_mode.clone()
                .or_else(|| self.controls.known.get(&(key.clone(), ctl)).map(|v| text(v, "current")).filter(|s| !s.is_empty())),
            Ctl::Permission => self.controls.known.get(&(key, ctl)).map(|v| text(v, "current")).filter(|s| !s.is_empty()),
        };
        from_status
    }

    // Catálogo lido no gesto de abrir. O ciclo do Claude com terminal e a permissão do Codex com
    // terminal mexem na TUI para ler: esses só com um segundo clique explícito.
    pub(super) fn open_ctl(&mut self, ctl: Ctl, probe: bool, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if !probe && self.controls.open.as_ref().is_some_and(|o| o.key == key && o.ctl == ctl) { self.controls.open = None; cx.notify(); return; }
        self.command_panel = false;
        self.recent = None;
        let (provider, headless) = self.provider();
        let provider = provider.to_owned();
        let read: Option<(Vec<&'static str>, Vec<(&'static str, &'static str)>)> = match (provider.as_str(), ctl) {
            ("claude", Ctl::Model) => Some((vec!["model", "options"], vec![])),
            ("claude", Ctl::Effort) => None,
            ("claude", Ctl::Mode) if headless || probe => Some((vec!["permission-modes"], if probe { vec![("sondar", "1")] } else { vec![] })),
            ("claude", Ctl::Mode) => Some((vec!["permission-modes"], vec![])),
            ("codex", Ctl::Model | Ctl::Effort) => Some((vec!["models"], vec![])),
            ("codex", Ctl::Mode) => None,
            ("codex", Ctl::Permission) if headless || probe => Some((vec!["codex-permissions"], vec![])),
            ("codex", Ctl::Permission) => None,
            ("pi" | "omp", _) => Some((vec!["pi", "models"], vec![])),
            ("kimi", _) => Some((vec!["kimi", "models"], vec![])),
            _ => return,
        };
        let loaded = read.is_none().then(|| Ok(Value::Null));
        self.controls.open = Some(Open { key: key.clone(), ctl, catalog: loaded });
        self.controls.highlight = None;
        self.controls.scroll = ScrollHandle::new();
        self.controls.revealed.set(false);
        if let Some((path, query)) = read {
            let (connection, tx) = (self.connection, self.tx.clone());
            // `sondar` passa por todos os modos no terminal: timeout maior que a volta completa.
            let seconds = if probe { 60 } else { 45 };
            self.runtime.spawn(async move {
                let result = api.read(&key.name, &path, &query, seconds).await;
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Catalog(ctl), result) }).await;
            });
        }
        cx.notify();
    }

    fn choices(&self, ctl: Ctl, catalog: &Value) -> Vec<Choice> {
        let key = self.selected_key();
        let (provider, _) = self.provider();
        let current = self.ctl_label(ctl).unwrap_or_default();
        let choice = |label: String, detail: String, path: Vec<&'static str>, body: Value, on: bool| Choice { label, detail, path, body, current: on, enabled: true };
        let list = |v: &Value, k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        match (provider, ctl) {
            ("claude", Ctl::Model) => {
                let engine = text(catalog, "kind") == "engine";
                let models = list(catalog, "models");
                let ids: Vec<(String, bool)> = models.iter().map(|m| (text(m, "id"), m.get("active").and_then(Value::as_bool) == Some(true))).collect();
                let on = claude_model_current(&ids, &current);
                models.iter().enumerate().map(|(n, m)| {
                    let id = text(m, "id");
                    let name = Some(text(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                    if engine { choice(name, String::new(), vec!["engine", "model"], json!({"model": id}), on == Some(n)) }
                    else { choice(name, text(m, "desc"), vec!["model-effort"], json!({"model": id, "scope": "session"}), on == Some(n)) }
                }).collect()
            }
            ("claude", Ctl::Effort) => {
                let on = claude_effort_current(&current);
                CLAUDE_EFFORTS.iter().map(|e| choice((*e).into(), String::new(), vec!["model-effort"],
                    json!({"effort": e, "scope": "session"}), on == Some(*e))).collect()
            }
            ("claude", Ctl::Mode) => {
                let cycle: Vec<String> = list(catalog, "modes").iter().filter_map(|m| m.as_str().map(str::to_owned)).collect();
                let known_cycle = key.and_then(|k| self.controls.known.get(&(k, Ctl::Mode)))
                    .map(|v| list(v, "modes").iter().filter_map(|m| m.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
                let cycle = if cycle.is_empty() { known_cycle } else { cycle };
                // Ciclo ainda não lido: tudo fica indisponível até a leitura explícita, sem afirmar o motivo.
                let unknown = cycle.is_empty();
                CLAUDE_MODES.iter().map(|m| {
                    let enabled = cycle.iter().any(|c| c == m);
                    Choice { enabled, detail: if unknown || enabled { String::new() } else { tr("mode_not_in_cycle") },
                        ..choice(tr(&format!("mode_{m}")), String::new(), vec!["permission-mode"], json!({"mode": m}), current == *m) }
                }).collect()
            }
            ("codex", Ctl::Model) => {
                let now = catalog.get("current").cloned().unwrap_or(Value::Null);
                list(catalog, "models").iter().map(|m| {
                    let model = text(m, "model");
                    let efforts: Vec<String> = list(m, "efforts").iter().map(|e| text(e, "value")).filter(|e| !e.is_empty()).collect();
                    // Mesmo modelo mantém o esforço atual; outro leva o padrão dele.
                    let effort = if model == text(&now, "model") && !text(&now, "effort").is_empty() { text(&now, "effort") }
                        else { Some(text(m, "defaultEffort")).filter(|e| !e.is_empty()).or_else(|| efforts.first().cloned()).unwrap_or_default() };
                    let label = Some(text(m, "displayName")).filter(|n| !n.is_empty()).unwrap_or_else(|| model.clone());
                    let mut body = json!({"model": model});
                    if !effort.is_empty() { body["effort"] = json!(effort); }
                    choice(label, text(m, "description"), vec!["model"], body, model == text(&now, "model"))
                }).collect()
            }
            ("codex", Ctl::Effort) => {
                let now = catalog.get("current").cloned().unwrap_or(Value::Null);
                let model = text(&now, "model");
                list(catalog, "models").iter().find(|m| text(m, "model") == model).map(|m| list(m, "efforts").iter().map(|e| {
                    let value = text(e, "value");
                    choice(value.clone(), text(e, "description"), vec!["model"], json!({"model": model, "effort": value}), value == text(&now, "effort"))
                }).collect()).unwrap_or_default()
            }
            ("codex", Ctl::Mode) => ["default", "plan"].iter().map(|m| choice(tr(&format!("codex_mode_{m}")), String::new(), vec!["codex", "mode"],
                json!({"mode": m}), current == *m)).collect(),
            ("codex", Ctl::Permission) => {
                let modes = list(catalog, "modes");
                let now = text(catalog, "current");
                let on = modes.iter().position(|m| m.get("atual").and_then(Value::as_bool) == Some(true))
                    .or_else(|| modes.iter().position(|m| !now.is_empty() && text(m, "nome") == now));
                modes.iter().enumerate().map(|(n, m)| {
                    let name = text(m, "nome");
                    choice(name.clone(), text(m, "desc"), vec!["codex-permissions"], json!({"mode": name}), on == Some(n))
                }).collect()
            }
            ("pi" | "omp", Ctl::Model) => {
                let now = catalog.get("current").cloned().unwrap_or(Value::Null);
                list(catalog, "models").iter().map(|m| {
                    let (provider, id) = (text(m, "provider"), text(m, "id"));
                    let name = Some(text(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                    let on = provider == text(&now, "provider") && id == text(&now, "id");
                    choice(name, provider.clone(), vec!["pi", "model"], json!({"provider": provider, "model": id}), on)
                }).collect()
            }
            ("pi" | "omp", Ctl::Effort) => list(catalog, "levels").iter().filter_map(Value::as_str).map(|level| choice(level.into(), String::new(),
                vec!["pi", "model"], json!({"effort": level}), text(catalog, "thinking") == level)).collect(),
            ("kimi", Ctl::Model) => {
                let models = list(catalog, "models");
                let names: Vec<String> = models.iter().map(|m| Some(text(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| text(m, "alias"))).collect();
                let on = only_match(&names, &current);
                models.iter().zip(names).enumerate().map(|(n, (m, name))| {
                    choice(name, text(m, "provider"), vec!["kimi", "model"], json!({"model": text(m, "alias")}), on == Some(n))
                }).collect()
            }
            ("kimi", Ctl::Effort) => {
                let model = self.ctl_label(Ctl::Model).unwrap_or_default();
                let model = model.trim();
                list(catalog, "models").iter().find(|m| !model.is_empty() && text(m, "name").trim().eq_ignore_ascii_case(model))
                    .map(|m| list(m, "efforts").iter().filter_map(Value::as_str).map(|e| choice(e.into(), String::new(), vec!["kimi", "model"],
                        json!({"effort": e}), e.eq_ignore_ascii_case(current.trim()))).collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn apply_ctl(&mut self, ctl: Ctl, path: Vec<&'static str>, body: Value, label: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if self.controls.busy.contains_key(&key) || !self.chat_online { return; }
        self.controls.busy.insert(key.clone(), ctl);
        self.action_feedback.remove(&key);
        let before = self.ctl_live(ctl);
        let (connection, tx) = (self.connection, self.tx.clone());
        // Acima dos 40 s que o backend espera o Claude sem terminal trocar de modelo.
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &path, Some(body), false, 60).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Applied(ctl, label, before), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_control(&mut self, key: SessionKey, reply: Reply, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            Reply::Catalog(ctl) => {
                if let Ok(value) = &result {
                    if matches!(ctl, Ctl::Mode | Ctl::Permission) && !value.is_null() {
                        // Lista vazia nunca apaga um ciclo já conhecido.
                        let keep = value.get("modes").and_then(Value::as_array).is_none_or(|m| m.is_empty())
                            && self.controls.known.contains_key(&(key.clone(), ctl));
                        if !keep { self.controls.known.insert((key.clone(), ctl), value.clone()); }
                    }
                    if value.get("restaurado").and_then(Value::as_bool) == Some(false) {
                        self.action_feedback.insert(key.clone(), (tr("mode_probe_not_restored"), true));
                    }
                }
                let Some(open) = self.controls.open.as_mut().filter(|o| o.key == key && o.ctl == ctl) else { return; };
                open.catalog = Some(result.map_err(|error| Self::failure(&error)));
                // Lista longa chegando com o painel no foco: quem abriu já pode digitar a busca.
                let panel = self.controls.focus.as_ref().is_some_and(|(handle, _)| handle.is_focused(window));
                if panel && self.open_choices(cx).is_some_and(|(_, _, long)| long) {
                    self.ctl_search.update(cx, |input, cx| input.focus(window, cx));
                }
            }
            Reply::Applied(ctl, label, before) => {
                if self.controls.busy.get(&key) == Some(&ctl) { self.controls.busy.remove(&key); }
                match result {
                    Ok(value) => {
                        // A resposta do backend é a verdade: Pi/Kimi devolvem o que ficou de fato.
                        let label = match ctl {
                            Ctl::Mode => Some(text(&value, "mode")).filter(|m| !m.is_empty()).or_else(|| Some(text(&value, "current")).filter(|m| !m.is_empty())).unwrap_or(label),
                            Ctl::Permission => Some(text(&value, "current")).filter(|m| !m.is_empty()).unwrap_or(label),
                            Ctl::Effort => value.get("thinking").or_else(|| value.get("effort")).and_then(Value::as_str).map(str::to_owned).unwrap_or(label),
                            Ctl::Model => value.pointer("/current/name").or_else(|| value.get("model")).and_then(Value::as_str).map(str::to_owned).unwrap_or(label),
                        };
                        let pending = value.get("pending_confirm").is_some_and(|v| !v.is_null());
                        let partial = value.get("effort_error").is_some_and(|v| !v.is_null());
                        if !pending && !matches!(ctl, Ctl::Mode) {
                            self.controls.applied.insert((key.clone(), ctl), (label.clone(), before));
                        }
                        if matches!(ctl, Ctl::Mode | Ctl::Permission) {
                            if let Some(known) = self.controls.known.get_mut(&(key.clone(), ctl)) { known["current"] = json!(label); }
                        }
                        if self.controls.open.as_ref().is_some_and(|o| o.key == key && o.ctl == ctl) && !partial { self.controls.open = None; }
                        let note = if pending { (tr("ctl_pending_confirm"), true) }
                            else if partial { (tr("ctl_effort_not_applied"), true) }
                            else { (tr("ctl_applied").replace("{what}", &tr(&format!("ctl_{}", ctl.key()))).replace("{value}", &shown(ctl, label)), false) };
                        self.action_feedback.insert(key, note);
                    }
                    Err(error) => {
                        let text = match (error.uncertain, error.status) {
                            (true, None) => tr("action_uncertain"),
                            (true, Some(_)) => format!("{} {}", tr(&error.detail), tr("action_uncertain")),
                            _ => Self::failure(&error),
                        };
                        // Não aplicou: o rótulo fica com o valor real, e a leitura do Codex é refeita.
                        self.controls.applied.remove(&(key.clone(), ctl));
                        self.action_feedback.insert(key, (text, true));
                    }
                }
            }
            Reply::PlanPreview(content) => {
                let Some((owner, meta, body)) = self.controls.plan.as_mut() else { return; };
                if owner != &key { return; }
                let result = match result {
                    Err(error) if error.status == Some(404) => Ok(Value::Null),
                    other => other.map_err(|error| Self::failure(&error)),
                };
                if content { *body = Some(result); } else { *meta = Some(result); }
            }
            Reply::PreSelect(snapshot) => {
                if self.controls.preselect.as_ref() == Some(&key) { self.controls.preselect = None; }
                let _ = snapshot;
                let note = match result {
                    Ok(_) => (tr("option_sent"), false),
                    Err(error) if error.uncertain => (tr("action_uncertain"), true),
                    Err(error) => (Self::failure(&error), true),
                };
                self.action_feedback.insert(key, note);
            }
            _ => {}
        }
    }

    /// Seletores da sessão como no compositor do web: modelo, esforço e permissão em pílulas; o modo com seta.
    pub(super) fn render_ctl_pills(&self, readable: bool, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<AnyElement>) {
        let Some(key) = self.selected_key().filter(|_| readable) else { return (Vec::new(), None); };
        let busy = self.controls.busy.get(&key).copied();
        let open = self.controls.open.as_ref().filter(|o| o.key == key).map(|o| o.ctl);
        let claude = self.provider().0 == "claude";
        let (mut pills, mut mode) = (Vec::new(), None);
        for ctl in self.ctl_list() {
            let name = tr(&format!("ctl_{}", ctl.key()));
            let value = self.ctl_label(ctl).map(|v| shown(ctl, v));
            let text = if busy == Some(ctl) { tr("ctl_applying").replace("{what}", &name) } else { value.clone().unwrap_or_else(|| name.clone()) };
            let id = SharedString::from(format!("ctl-{}", ctl.key()));
            let listener = cx.listener(move |this, _, window, cx| {
                this.open_ctl(ctl, false, cx);
                this.focus_ctl_panel(window, cx);
            });
            if ctl == Ctl::Mode {
                let plan = self.ctl_label(ctl).as_deref() == Some("plan");
                mode = Some(popup::anchor(div(), id.clone()).child(chrome::pill_button(id, cx).pl(px(10.)).gap(px(6.)).selected(open == Some(ctl)).disabled(!self.chat_online)
                    .tooltip(name.clone()).accessibility_label(format!("{name}: {text}"))
                    .child(div().max_w(px(160.)).truncate().text_xs().text_color(if plan { theme::accent() } else { theme::muted() })
                        .when(plan, |el| el.font_weight(FontWeight::SEMIBOLD)).child(text))
                    .child(chrome::small_icon(IconName::ChevronDown, 12., theme::faint()))
                    .on_click(listener)).into_any_element());
                continue;
            }
            let glyph = ctl == Ctl::Effort && claude;
            pills.push(popup::anchor(div(), id.clone()).child(chrome::pill_button(id, cx).gap(px(4.)).selected(open == Some(ctl)).disabled(!self.chat_online)
                .tooltip(name.clone()).accessibility_label(format!("{name}: {text}"))
                .when(glyph, |el| el.child(div().text_size(px(10.)).text_color(theme::faint()).child("✦")))
                .child(div().max_w(px(130.)).truncate().text_xs().font_weight(FontWeight::SEMIBOLD).child(text))
                .on_click(listener)).into_any_element());
        }
        (pills, mode)
    }

    fn focus_ctl_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.controls_open() { return; }
        if self.controls.focus.is_none() {
            let handle = cx.focus_handle();
            let out = cx.on_focus_out(&handle, window, |this, _, window, cx| {
                // O painel saiu da árvore com o foco (clique fora, troca de sessão, sessão ilegível): o foco volta à raiz.
                let search = this.ctl_search.read(cx).focus_handle(cx);
                let stranded = window.focused(cx).is_none_or(|f| f == search || this.controls.focus.as_ref().is_some_and(|(h, _)| *h == f));
                if window.is_window_active() && stranded { this.root_focus.focus(window, cx); }
            });
            self.controls.focus = Some((handle, out));
        }
        self.ctl_search.update(cx, |input, cx| input.set_value("", window, cx));
        if let Some((handle, _)) = &self.controls.focus { handle.clone().focus(window, cx); }
    }

    /// Opções do painel aberto, quando a lista já chegou; na lista longa, só as que passam na busca.
    fn open_choices(&self, cx: &App) -> Option<(Ctl, Vec<Choice>, bool)> {
        let open = self.controls.open.as_ref()?;
        let Some(Ok(catalog)) = &open.catalog else { return None; };
        let all = self.choices(open.ctl, catalog);
        let long = all.len() > LONG_LIST;
        let query = if long { self.ctl_search.read(cx).value().to_string() } else { String::new() };
        Some((open.ctl, keep(all, &query), long))
    }

    fn ctl_highlight(&self, choices: &[Choice]) -> usize {
        self.controls.highlight.filter(|&n| choices.get(n).is_some_and(|c| c.enabled))
            .or_else(|| choices.iter().position(|c| c.current && c.enabled))
            .or_else(|| choices.iter().position(|c| c.enabled)).unwrap_or(0)
    }

    // `edge`: Home/End, a primeira ou a última livre conforme o sentido.
    fn move_ctl(&mut self, step: isize, edge: bool, cx: &mut Context<Self>) {
        let Some((_, choices, long)) = self.open_choices(cx) else { return; };
        let free: Vec<bool> = choices.iter().map(|c| c.enabled).collect();
        let now = if edge { usize::MAX } else { self.ctl_highlight(&choices) };
        let Some(next) = step_free(&free, now, step) else { return; };
        self.controls.highlight = Some(next);
        if long { self.controls.long.scroll_to_item(next, ScrollStrategy::Nearest); } else { self.controls.scroll.scroll_to_item(next); }
        cx.notify();
    }

    /// Clique ou Enter numa linha (`None` = a destacada). A atual só fecha: reaplicar dispararia a troca de novo.
    /// O foco volta ao campo, para quem trocou seguir digitando.
    fn pick_ctl(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ctl, choices, _)) = self.open_choices(cx) else { return; };
        if self.selected_key().is_some_and(|key| self.controls.busy.contains_key(&key)) { return; }
        let n = row.unwrap_or_else(|| self.ctl_highlight(&choices));
        let Some(choice) = choices.into_iter().nth(n).filter(|c| c.enabled) else { return; };
        self.composer.update(cx, |input, cx| input.focus(window, cx));
        if choice.current { self.close_controls(); cx.notify(); return; }
        self.apply_ctl(ctl, choice.path, choice.body, choice.label, cx);
    }

    // Um destaque só: o ponteiro move o do teclado. A atual leva o fundo accent e o tique; `inline` põe a origem ao lado
    // do nome, numa linha só.
    fn ctl_row(&self, n: usize, c: Choice, ctl: Ctl, lit: bool, busy: bool, inline: bool, cx: &mut Context<Self>) -> AnyElement {
        let (current, lit, enabled) = (c.current, lit && c.enabled, c.enabled);
        // Id pelo pedido que a linha faria: filtrando, a mesma linha muda de posição e não herda o estado de outra.
        let id = SharedString::from(format!("ctl-choice-{}", c.body));
        let shown = if ctl == Ctl::Effort { capitalized(&c.label) } else { c.label };
        let name = div().truncate().text_sm().text_color(theme::text()).child(shown);
        let text = if inline {
            div().flex_1().min_w_0().flex().items_center().gap(px(6.)).child(name.flex_none().max_w_full().font_weight(FontWeight::MEDIUM))
                .when(!c.detail.is_empty(), |el| el.child(div().min_w_0().truncate().text_xs().text_color(theme::muted()).child(c.detail)))
        } else {
            div().flex_1().min_w_0().flex().flex_col().gap(px(1.)).child(name)
                .when(!c.detail.is_empty(), |el| el.child(div().truncate().text_xs().text_color(theme::muted()).child(c.detail)))
        };
        popup::row(id, current).disabled(busy || !enabled)
            // Véu do texto, como o do Zeron: `theme::hover()` sólido tem a cor do cartão no clássico escuro.
            .when(lit && !current, |el| el.bg(theme::text().opacity(0.06)))
            .child(div().w_full().flex().items_center().gap_2().child(text)
                .when(current, |el| el.child(chrome::small_icon(IconName::Check, 16., theme::accent()))))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && enabled && this.controls.highlight != Some(n) { this.controls.highlight = Some(n); cx.notify(); }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.pick_ctl(Some(n), window, cx)))
            .into_any_element()
    }

    pub(super) fn render_ctl_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.selected_key()?;
        let open = self.controls.open.as_ref().filter(|o| o.key == key)?;
        let ctl = open.ctl;
        let (provider, headless) = self.provider();
        let busy = self.controls.busy.contains_key(&key);
        let probe_note = match (provider, ctl) {
            ("claude", Ctl::Mode) if !headless => Some("mode_probe_hint"),
            ("codex", Ctl::Permission) if !headless => Some("codex_permission_probe_hint"),
            _ => None,
        };
        let body = match &open.catalog {
            None => popup::skeleton("ctl-loading", 5).into_any_element(),
            Some(Err(reason)) => div().p(px(8.)).flex().flex_col().items_start().gap(px(6.)).text_size(px(12.)).text_color(theme::danger())
                .child(tr("ctl_failed").replace("{reason}", reason))
                .child(Button::new("ctl-retry").outline().xsmall().label(tr("ctl_retry")).on_click(cx.listener(move |this, _, window, cx| {
                    this.close_controls();
                    this.open_ctl(ctl, false, cx);
                    this.focus_ctl_panel(window, cx);
                })))
                .into_any_element(),
            Some(Ok(catalog)) => {
                let (_, choices, long) = self.open_choices(cx).unwrap_or((ctl, Vec::new(), false));
                let needs_probe = probe_note.is_some() && (catalog.is_null() || choices.iter().all(|c| !c.enabled) || choices.is_empty());
                let pick = self.ctl_highlight(&choices);
                // A GPUI mede a área da lista depois de aplicar o `scroll_to_item`: no primeiro quadro o pedido se perderia.
                // Sem medida ainda, só pede outro quadro. A lista longa guarda o pedido até medir.
                if !choices.is_empty() && !self.controls.revealed.get() {
                    if long {
                        self.controls.revealed.set(true);
                        self.controls.long.scroll_to_item(pick, ScrollStrategy::Center);
                    } else if self.controls.scroll.bounds().size.height > px(0.) {
                        self.controls.revealed.set(true);
                        self.controls.scroll.scroll_to_item(pick);
                    } else { window.request_animation_frame(); }
                }
                let list = if choices.is_empty() && !needs_probe {
                    let searching = long && !self.ctl_search.read(cx).value().trim().is_empty();
                    div().px(px(8.)).py(px(24.)).text_size(px(12.)).text_color(theme::muted()).text_center()
                        .child(tr(if searching { "ctl_no_results" } else { "ctl_empty" })).into_any_element()
                } else if long {
                    // Só as linhas visíveis são montadas, todas numa linha só; o painel que sai desenha o `open` dele.
                    let shown = open.clone();
                    // O teto mora no contêiner: a medida da lista soma todas as linhas antes do `max_h` dela valer,
                    // e o cartão crescia até a borda da janela.
                    div().max_h(px(260.)).flex().flex_col().overflow_hidden().child(uniform_list("ctl-list-long", choices.len(), cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let live = this.controls.open.replace(shown.clone());
                        let rows = this.open_choices(cx).map(|(_, choices, _)| {
                            let (pick, busy) = (this.ctl_highlight(&choices), this.controls.busy.contains_key(&shown.key));
                            choices.into_iter().enumerate().skip(range.start).take(range.len())
                                .map(|(n, c)| this.ctl_row(n, c, ctl, n == pick, busy, true, cx)).collect()
                        }).unwrap_or_default();
                        this.controls.open = live;
                        rows
                    })).with_sizing_behavior(ListSizingBehavior::Infer).track_scroll(&self.controls.long)).into_any_element()
                } else {
                    // Filhos diretos são as linhas: o índice do `scroll_to_item` é o da opção.
                    div().id("ctl-list").max_h(px(260.)).overflow_y_scroll().track_scroll(&self.controls.scroll).flex().flex_col()
                        .children(choices.into_iter().enumerate().map(|(n, c)| self.ctl_row(n, c, ctl, n == pick, busy, ctl == Ctl::Model, cx)))
                        .into_any_element()
                };
                // As setas andam na lista antes do campo tentar mover o cursor; Enter e Esc o campo já deixa subir.
                let search = long.then(|| div().px(px(4.)).pb(px(4.))
                    .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_ctl(-1, false, cx)))
                    .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_ctl(1, false, cx)))
                    .child(Input::new(&self.ctl_search).h(px(32.)).aria_label(tr("ctl_search"))
                        .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))));
                div().flex().flex_col().gap(px(2.))
                    .children(search)
                    .child(list)
                    .when(ctl == Ctl::Effort, |el| el.child(popup::separator())
                        .child(div().px(px(8.)).pt(px(4.)).pb(px(2.)).text_xs().text_color(theme::muted()).child(tr("effort_hint"))))
                    .when_some(probe_note.filter(|_| needs_probe), |el, note| el.child(div().px(px(8.)).flex().items_center().gap_2()
                        .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).child(tr(note)))
                        .child(Button::new("ctl-probe").xsmall().flex_shrink_0().label(tr("ctl_probe")).disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_ctl(ctl, true, cx);
                                this.focus_ctl_panel(window, cx);
                            })))))
                    .into_any_element()
            }
        };
        let keys = cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "up" => this.move_ctl(-1, false, cx),
                "down" => this.move_ctl(1, false, cx),
                // Entrando de fora da lista: para a frente cai na primeira, para trás na última.
                "home" => this.move_ctl(1, true, cx),
                "end" => this.move_ctl(-1, true, cx),
                "enter" => this.pick_ctl(None, window, cx),
                _ => return,
            }
            cx.stop_propagation();
        });
        Some(div().p(px(popup::INSET)).rounded_md().bg(theme::raised()).flex().flex_col().gap(px(2.))
            .when_some(self.controls.focus.as_ref(), |el, focus| el.track_focus(&focus.0).on_key_down(keys))
            .child(popup::title(tr(&format!("ctl_{}_title", ctl.key())), Some("esc")))
            .child(body).into_any_element())
    }

    /// Plano do Claude sem terminal em modo plano, parado: a última resposta depois do último pedido real.
    pub(super) fn headless_plan(&self) -> Option<(String, String)> {
        let (provider, headless) = self.provider();
        if provider != "claude" || !headless || self.chat.state.claude_permission_mode.as_deref() != Some("plan") { return None; }
        if self.chat.state.state != "idle" || self.chat.state.claude_plan_pending.is_some() { return None; }
        for event in self.chat.events.iter().rev() {
            if event.kind == "user_msg" && !event.queued() { return None; }
            if event.kind == "assistant_msg" && !event.id.starts_with("local-") {
                let text = event.text.clone().unwrap_or_default();
                if !text.trim().is_empty() { return Some((event.id.clone(), text)); }
            }
        }
        None
    }

    // Sai do modo plano para o modo anterior, pede a implementação e, se o pedido falhar, tenta voltar ao plano.
    fn implement_headless(&mut self, id: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if self.headless_plan().map(|(current, _)| current) != Some(id) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        if self.delivery.pending(&key) || self.queued_count() > 0 || !self.controls.planning.insert(key.clone()) { return; }
        let target = self.chat.state.claude_previous_non_plan.clone().filter(|m| !m.is_empty() && m != "plan").unwrap_or_else(|| "acceptEdits".into());
        let request = tr("plan_request");
        self.action_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let outcome = match api.act(&key.name, &["permission-mode"], Some(json!({"mode": target})), false, 60).await {
                Err(error) => PlanOutcome::ModeFailed(Self::failure(&error)),
                Ok(_) => match api.send(&key.name, &request).await {
                    Ok(delivery) => PlanOutcome::Sent(delivery.delivered),
                    // Incerto pode ter chegado: voltar ao plano derrubaria a implementação em curso.
                    Err(error) if error.uncertain => PlanOutcome::SendFailed { reason: Self::failure(&error), uncertain: true, reverted: false },
                    Err(error) => {
                        let reverted = api.act(&key.name, &["permission-mode"], Some(json!({"mode": "plan"})), false, 60).await.is_ok();
                        PlanOutcome::SendFailed { reason: Self::failure(&error), uncertain: error.uncertain, reverted }
                    }
                },
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::HeadlessPlan(key, outcome) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_headless_plan(&mut self, key: SessionKey, outcome: PlanOutcome) {
        self.controls.planning.remove(&key);
        let note = match outcome {
            PlanOutcome::ModeFailed(reason) => (tr("plan_mode_failed").replace("{reason}", &reason), true),
            PlanOutcome::SendFailed { reason, uncertain: true, .. } => (tr("plan_send_uncertain").replace("{reason}", &reason), true),
            PlanOutcome::SendFailed { reason, reverted: true, .. } => (tr("plan_send_failed_reverted").replace("{reason}", &reason), true),
            PlanOutcome::SendFailed { reason, .. } => (tr("plan_send_failed_stuck").replace("{reason}", &reason), true),
            PlanOutcome::Sent(true) => (tr("plan_implement_sent"), false),
            PlanOutcome::Sent(false) => (tr("plan_implement_queued"), false),
        };
        self.action_feedback.insert(key, note);
    }

    pub(super) fn render_headless_plan(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, _) = self.headless_plan()?;
        if self.plans_dismissed.contains(&id) { return None; }
        let key = self.selected_key()?;
        let running = self.controls.planning.contains(&key);
        let ready = !running && self.can_send() && !self.delivery.pending(&key) && self.queued_count() == 0;
        let dismiss = id.clone();
        Some(div().py_2().flex().items_center().gap_2().border_t_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().text_sm().child(tr(if running { "plan_implementing" } else { "headless_plan_title" })))
            .child(Button::new("hplan-dismiss").small().ghost().label(tr("codex_plan_dismiss"))
                .on_click(cx.listener(move |this, _, _, cx| { this.plans_dismissed.insert(dismiss.clone()); cx.notify(); })))
            .child(Button::new("hplan-implement").small().primary().label(tr("codex_plan_implement")).disabled(!ready)
                .on_click(cx.listener(move |this, _, _, cx| this.implement_headless(id.clone(), cx))))
            .into_any_element())
    }

    /// Plano do Claude com terminal: só leitura do arquivo pelo `/plan-preview` (metadados na descoberta, conteúdo no gesto).
    pub(super) fn discover_plan(&mut self) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if self.provider() != ("claude", false) { return; }
        self.controls.plan = Some((key.clone(), None, None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["plan-preview"], &[("content", "false")], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::PlanPreview(false), result) }).await;
        });
    }

    fn open_plan_preview(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        let Some((owner, _, body)) = self.controls.plan.as_mut() else { return; };
        if owner != &key { return; }
        if body.is_some() { *body = None; cx.notify(); return; }
        // Erro vazio marca "carregando" até a resposta chegar.
        *body = Some(Err(String::new()));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["plan-preview"], &[("content", "true")], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::PlanPreview(true), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn render_plan_preview(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.selected_key()?;
        let (owner, meta, body) = self.controls.plan.clone()?;
        if owner != key { return None; }
        let meta = match meta? {
            Ok(Value::Null) => return None,
            Ok(meta) => meta,
            Err(reason) => return Some(div().py_1().text_xs().text_color(theme::warning()).child(tr("plan_preview_failed").replace("{reason}", &reason)).into_any_element()),
        };
        let path = text(&meta, "path");
        let marker = format!("{}:{}", text(&meta, "name"), path);
        if self.controls.plans_done.contains(&marker) { return None; }
        let title = Some(text(&meta, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| tr("plan_preview_title"));
        let expanded = body.is_some();
        let content = match body {
            None => None,
            Some(Err(reason)) if reason.is_empty() => Some(div().text_sm().text_color(theme::muted()).child(tr("plan_preview_loading")).into_any_element()),
            Some(Err(reason)) => Some(div().text_sm().text_color(theme::warning()).child(reason).into_any_element()),
            Some(Ok(Value::Null)) => Some(div().text_sm().text_color(theme::muted()).child(tr("plan_preview_gone")).into_any_element()),
            Some(Ok(value)) => {
                let source = safe_markdown(&text(&value, "markdown"));
                let view = self.text_view(&format!("plan-preview:{marker}"), "__plan__", source, cx);
                Some(div().id("plan-preview-body").max_h(px(320.)).overflow_y_scroll().p_3().rounded_md().bg(theme::raised())
                    .child(TextView::new(&view).selectable(true).scrollable(false)).into_any_element())
            }
        };
        let done = marker.clone();
        Some(div().py_2().flex().flex_col().gap_2().border_t_1().border_color(theme::border())
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().text_sm().truncate().child(tr("plan_preview_found").replace("{title}", &title)))
                    .when(!path.is_empty(), |el| el.child(div().text_xs().text_color(theme::muted()).truncate().child(path.clone()))))
                .child(Button::new("plan-preview-hide").small().outline().label(tr("plan_preview_hide"))
                    .on_click(cx.listener(move |this, _, _, cx| { this.controls.plans_done.insert(done.clone()); cx.notify(); })))
                .child(Button::new("plan-preview-open").small().toggled(expanded).label(tr(if expanded { "close" } else { "plan_preview_open" }))
                    .on_click(cx.listener(|this, _, _, cx| this.open_plan_preview(cx)))))
            .when_some(content, |el, content| el.child(content))
            .into_any_element())
    }

    /// Codex antes da thread: a pergunta (aprovação dos hooks) vem da lista e sai por `/select`.
    pub(super) fn prethread_key(&self) -> Option<SessionKey> {
        let session = self.selected.as_ref().filter(|s| !s.readable() && s.provider == "codex")?;
        Some(SessionKey { server: self.server.clone()?, name: session.name.clone(), jsonl: String::new() })
    }

    fn prethread_snapshot(&self) -> Option<String> {
        let session = self.selected.as_ref()?;
        (session.state == "awaiting_input").then(|| json!([session.question, session.options]).to_string())
    }

    fn preselect(&mut self, option: usize, snapshot: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.prethread_key()) else { return; };
        if self.controls.preselect.is_some() { return; }
        if self.prethread_snapshot().as_deref() != Some(snapshot.as_str()) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        self.controls.preselect = Some(key.clone());
        self.action_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["select"], Some(json!({"option": option})), false, 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::PreSelect(snapshot), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn render_prethread(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.prethread_key()?;
        let session = self.selected.as_ref()?;
        let note = self.action_feedback.get(&key).cloned();
        let busy = self.controls.preselect.is_some();
        let mut body = div().flex().flex_col().gap_2();
        match (self.prethread_snapshot(), session.question.clone(), session.options.clone().filter(|o| !o.is_empty())) {
            (Some(snapshot), Some(question), Some(options)) => {
                body = body.child(div().text_xs().text_color(theme::muted()).child(tr("prethread_header")))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(question));
                for (i, option) in options.iter().enumerate() {
                    let snap = snapshot.clone();
                    body = body.child(Button::new(SharedString::from(format!("prethread-{i}"))).w_full().disabled(busy || !self.list_online)
                        .label(format!("{}. {option}", i + 1))
                        .on_click(cx.listener(move |this, _, _, cx| this.preselect(i + 1, snap.clone(), cx))));
                }
            }
            _ => {
                let label = session.label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| tr("prethread_waiting"));
                body = body.child(div().text_sm().text_color(theme::muted()).child(label));
            }
        }
        Some(div().flex_1().p_6().flex().flex_col().gap_3()
            .child(div().text_sm().text_color(theme::muted()).child(tr("prethread_title")))
            .child(div().w_full().max_w(px(COLUMN)).p_4().rounded_lg().border_1().border_color(theme::border()).bg(theme::surface()).child(body))
            .when_some(note, |el, (note, warning)| el.child(div().text_xs().text_color(if warning { theme::warning() } else { theme::muted() }).child(note)))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{Choice, claude_effort_current, claude_model_current, keep, only_match, step_free};

    #[test]
    fn search_matches_name_origin_or_id_ignoring_case() {
        let row = |label: &str, detail: &str, id: &str| Choice { label: label.into(), detail: detail.into(), path: Vec::new(),
            body: serde_json::json!({"model": id}), current: false, enabled: true };
        let rows = || vec![row("GPT-5", "openai", "gpt-5"), row("Sonnet", "anthropic", "claude-sonnet"),
            row("gpt-oss", "groq", "oss-120b"), row("Modelo sintético 05", "google", "sint-05")];
        let labels = |q: &str| keep(rows(), q).into_iter().map(|c| c.label).collect::<Vec<_>>();
        assert_eq!(labels(" gpt "), ["GPT-5", "gpt-oss"]);
        assert_eq!(labels("ANTHROPIC"), ["Sonnet"]);
        assert_eq!(labels("sint-05"), ["Modelo sintético 05"]);
        assert_eq!(labels("openai/gpt"), ["GPT-5"]);
        assert_eq!(labels("").len(), 4);
        assert!(labels("zzz").is_empty());
    }

    #[test]
    fn arrows_skip_locked_rows_and_wrap() {
        let free = [true, false, true, true];
        assert_eq!((step_free(&free, 0, 1), step_free(&free, 3, 1), step_free(&free, 0, -1)), (Some(2), Some(0), Some(3)));
        assert_eq!((step_free(&free, usize::MAX, 1), step_free(&free, usize::MAX, -1)), (Some(0), Some(3)));
        assert_eq!(step_free(&[false, false], 0, 1), None);
    }

    fn names(list: &[&str]) -> Vec<String> { list.iter().map(|s| (*s).to_owned()).collect() }

    #[test]
    fn kimi_marks_exactly_one_row() {
        let k3 = names(&["K3", "K3-256k"]);
        assert_eq!((only_match(&k3, "K3"), only_match(&k3, "k3-256k")), (Some(0), Some(1)));
        let coding = names(&["K2.7 Coding", "K2.7 Coding Highspeed"]);
        assert_eq!(only_match(&coding, "K2.7 Coding"), Some(0));
        assert_eq!(only_match(&names(&["K3", "K3", "K3-256k"]), "K3"), None);
        assert_eq!(only_match(&k3, ""), None);
    }

    #[test]
    fn claude_status_beats_stale_active() {
        let picker = vec![("opus".to_owned(), true), ("fable".to_owned(), false)];
        assert_eq!(claude_model_current(&picker, "Fable 5"), Some(1));
        assert_eq!(claude_model_current(&picker, ""), Some(0));
        let window = vec![("default".to_owned(), false), ("opus".to_owned(), false), ("opus[1m]".to_owned(), false)];
        assert_eq!(claude_model_current(&window, "Opus5·1M"), Some(2));
        assert_eq!(claude_model_current(&window, "Opus 5"), Some(1));
        assert_eq!((claude_effort_current("med"), claude_effort_current("high"), claude_effort_current("")), (Some("medium"), Some("high"), None));
    }
}
