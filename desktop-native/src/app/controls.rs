//! Controles da sessão no compositor (modelo, esforço, modo, permissão) e os pedidos de plano que
//! mudam o modo. Catálogo vem das rotas de cada provider no gesto; nada é lido do terminal ao montar.
use super::*;
use gpui_kit::component::switch::Switch;

const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
const CLAUDE_MODES: [&str; 6] = ["plan", "auto", "manual", "acceptEdits", "bypassPermissions", "dontAsk"];
// Acima disto a lista ganha busca e só as linhas visíveis são montadas (catálogo do Pi/OMP tem dezenas).
const LONG_LIST: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Ctl { Model, Effort, Fast, Mode, Permission }

impl Ctl {
    fn key(self) -> &'static str {
        match self { Ctl::Model => "model", Ctl::Effort => "effort", Ctl::Fast => "fast", Ctl::Mode => "mode", Ctl::Permission => "permission" }
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
    pub(super) fn session(&self) -> &SessionKey { &self.key }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelIdentity {
    provider: String,
    engine: Option<String>,
    engine_account: Option<String>,
    account: Option<String>,
    lifecycle: Option<String>,
}

impl ModelIdentity {
    fn of(session: &SessionInfo) -> Self {
        Self { provider: session.provider.clone(), engine: session.engine.clone(), engine_account: session.engine_account.clone(),
            account: session.conta.clone(), lifecycle: session.lifecycle_id.clone() }
    }
}

#[derive(Default)]
pub(super) struct Controls {
    open: Option<Open>,
    busy: HashMap<SessionKey, Ctl>,
    // Rótulo aplicado com sucesso e o valor da fonte ao vivo naquele momento: vale até a fonte mudar.
    applied: HashMap<(SessionKey, Ctl), (String, Option<String>)>,
    // Catálogos confirmados de modelo, ciclo do Claude e permissão do Codex, por sessão.
    known: HashMap<(SessionKey, Ctl), Value>,
    // Cada abertura conserva só sua leitura; respostas antigas não alteram Fast confirmado.
    model_reads: HashMap<SessionKey, (ModelIdentity, tokio::sync::oneshot::Receiver<Result<Value, Failure>>)>,
    model_identities: HashMap<SessionKey, ModelIdentity>,
    model_actions: HashMap<SessionKey, Option<ModelIdentity>>,
    // Plano do Claude com terminal: metadados achados e o conteúdo aberto pelo gesto.
    plan: Option<(SessionKey, Option<Result<Value, String>>, Option<Result<Value, String>>)>,
    plans_done: HashSet<String>,
    planning: HashSet<SessionKey>,
    preselect: Option<(SessionKey, String)>,
    prethread_since: Option<Instant>,
    // Linha sob o teclado; `None` é a atual (ou a primeira livre) até ↑↓ ou o ponteiro moverem.
    highlight: Option<usize>,
    scroll: ScrollHandle,
    // A mesma rolagem na lista longa, que é virtualizada.
    long: UniformListScrollHandle,
    // O painel toma o foco ao abrir: ↑↓ Enter chegam a ele, e o Esc sobe até o braço da raiz.
    focus: Option<(FocusHandle, Subscription)>,
    // A lista rola até a linha destacada uma vez por abertura, no primeiro desenho com ela.
    revealed: std::cell::Cell<bool>,
    // O atalho de permissão pediu o ciclo que ainda não conhecia: a leitura que chegar anda tantos modos quantas
    // teclas foram dadas enquanto ela não vinha.
    cycle_after_read: Option<(SessionKey, usize)>,
    // Shift+Tab: o modo pedido aparece na pílula na hora. Tecla que chega com a troca em voo só anda o alvo, e a
    // resposta manda o que faltar; aplicado, o rótulo passa ao `applied` até o SSE trazer o modo novo.
    mode_target: HashMap<SessionKey, ModeTarget>,
}

// `answers`: os modos que as respostas deste alvo já trouxeram.
struct ModeTarget { mode: String, before: Option<String>, answers: Vec<String> }

impl ModeTarget {
    /// Se a resposta manda o pedido de novo: só quando o alvo andou com a troca em voo e o backend saiu do lugar. Modo
    /// que já tinha voltado é recusa, e insistir repetiria o pedido sem fim.
    fn resend_after(&mut self, answer: &str) -> bool {
        if self.mode == answer || self.answers.iter().any(|a| a == answer) { return false; }
        self.answers.push(answer.to_owned());
        true
    }
}

/// O modo `steps` posições à frente de `from` no ciclo; fora do ciclo, conta a partir do primeiro.
fn mode_ahead(modes: &[String], from: Option<&str>, steps: usize) -> Option<String> {
    if steps == 0 { return None; }
    let at = from.and_then(|f| modes.iter().position(|m| m == f)).unwrap_or(modes.len().saturating_sub(1));
    super::sidebar::wrap_step(Some(at), modes.len(), steps as isize).map(|ix| modes[ix].clone())
}

impl Controls {
    pub fn on_select(&mut self) {
        self.open = None;
        for key in self.model_actions.keys().chain(self.model_reads.keys()) {
            self.known.remove(&(key.clone(), Ctl::Model));
            self.applied.remove(&(key.clone(), Ctl::Model));
        }
        self.model_reads.clear();
        self.model_actions.values_mut().for_each(|identity| *identity = None);
        self.plan = None;
        self.prethread_since = Some(Instant::now());
    }

    pub(super) fn on_session_update(&mut self, key: &SessionKey, session: &SessionInfo) {
        let identity = ModelIdentity::of(session);
        if self.model_identities.get(key).is_some_and(|previous| previous != &identity) {
            self.known.remove(&(key.clone(), Ctl::Model));
            self.applied.remove(&(key.clone(), Ctl::Model));
            self.model_reads.remove(key);
            if let Some(action) = self.model_actions.get_mut(key) { *action = None; }
            if self.open.as_ref().is_some_and(|o| &o.key == key && o.ctl == Ctl::Model) { self.open = None; }
        }
        self.model_identities.insert(key.clone(), identity);
    }

    fn model_catalog(&self, key: &SessionKey, session: &SessionInfo) -> Option<&Value> {
        if self.model_identities.get(key) != Some(&ModelIdentity::of(session)) { return None; }
        self.known.get(&(key.clone(), Ctl::Model)).filter(|catalog| catalog_engine_matches(catalog, session.engine.as_deref()))
    }

    pub fn clear_plan_preview(&mut self) { self.plan = None; }
    pub fn picker_open(&self) -> bool { self.open.is_some() }

    fn remember_model_catalog(&mut self, key: &SessionKey, catalog: Option<&Value>) {
        self.known.remove(&(key.clone(), Ctl::Model));
        if let Some(catalog) = catalog { self.known.insert((key.clone(), Ctl::Model), catalog.clone()); }
    }

    fn take_model_catalog(&mut self, key: &SessionKey, identity: &ModelIdentity) -> Option<Result<Value, Failure>> {
        let (owner, receive) = self.model_reads.get_mut(key)?;
        if owner != identity {
            self.model_reads.remove(key);
            return None;
        }
        let result = receive.try_recv().ok()?;
        self.model_reads.remove(key);
        Some(result)
    }

    fn confirm_claude_fast(&mut self, key: &SessionKey, tier: &str) {
        if let Some(known) = self.known.get_mut(&(key.clone(), Ctl::Model)) { known["current"]["service_tier"] = json!(tier); }
        if let Some(open) = self.open.as_mut().filter(|o| &o.key == key && o.ctl == Ctl::Model)
            && let Some(Ok(catalog)) = open.catalog.as_mut() { catalog["current"]["service_tier"] = json!(tier); }
    }
}

// Modo conhecido aparece traduzido; valor que o backend inventar depois aparece cru.
fn shown(ctl: Ctl, value: String) -> String {
    match ctl {
        Ctl::Effort => effort_label(&value),
        Ctl::Fast if value == "priority" => tr("ctl_fast_on"),
        Ctl::Fast if value == "default" => tr("ctl_fast_off"),
        Ctl::Mode if CLAUDE_MODES.contains(&value.as_str()) => tr(&format!("mode_{value}")),
        Ctl::Mode if value == "default" => tr("codex_mode_default"),
        _ => value,
    }
}

pub(super) fn capitalized(label: &str) -> String {
    let mut chars = label.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn effort_label(label: &str) -> String {
    let value = label.to_ascii_lowercase();
    match value.as_str() {
        "low" | "medium" | "high" | "xhigh" | "max" | "ultra" => tr_shared(&format!("reasoning_level_{value}"), &[]),
        _ => capitalized(label),
    }
}

/// A linha de status do Claude escreve o modelo colado ("Opus5.5·1M"); na pílula ele sai com espaços ("Opus 5.5 · 1M").
/// Só palavra de três letras ou mais se solta do número: ids curtos como `k3` ficam como estão.
fn spaced(label: &str) -> String {
    let mut out = String::new();
    let mut letters = 0;
    for c in label.chars() {
        if c == '·' { out.push_str(" · "); letters = 0; continue; }
        if c.is_ascii_digit() && letters >= 3 { out.push(' '); }
        letters = if c.is_alphabetic() { letters + 1 } else { 0 };
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `avisos` de uma reabertura (motor que saiu do catálogo, registro não gravado), numa linha.
pub(super) fn reopen_warnings(value: &Value) -> Option<String> {
    let avisos: Vec<&str> = value.get("avisos").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    (!avisos.is_empty()).then(|| avisos.join(" · "))
}

/// O problema da tela pré-conversa: a frase da faixa pelo código; só texto sem frase própria aparece como veio.
fn prethread_problem(problem: &str) -> String {
    super::problem_text(problem).unwrap_or_else(|| problem.to_owned())
}

fn text(value: &Value, key: &str) -> String { value.get(key).and_then(Value::as_str).unwrap_or("").to_owned() }

fn catalog_engine_matches(catalog: &Value, engine: Option<&str>) -> bool {
    let engine = engine.filter(|engine| !engine.is_empty());
    if text(catalog, "kind") == "engine" {
        engine.is_some() && catalog.get("engine").and_then(Value::as_str) == engine
    } else { engine.is_none() }
}

pub(in crate::app) fn context_model(model: &str, on: bool) -> String {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    if on { format!("{base}[1m]") } else { base.to_owned() }
}

fn claude_context_model<'a>(catalog: &'a Value, model: &str) -> Option<&'a str> {
    if text(catalog, "kind") != "engine" || catalog.get("supports_fast").and_then(Value::as_bool) != Some(true) { return None; }
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    catalog.get("models")?.as_array()?.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(base)
        && m.get("supports_fast").and_then(Value::as_bool) == Some(true))?.get("id")?.as_str()
}

fn claude_context_state(catalog: &Value) -> Option<(&str, bool)> {
    let model = catalog.pointer("/current/model")?.as_str()?;
    Some((claude_context_model(catalog, model)?, model.ends_with("[1m]")))
}

fn engine_model_choice(catalog: &Value, id: &str) -> String {
    if claude_context_state(catalog).is_some_and(|(_, on)| on) && claude_context_model(catalog, id).is_some() {
        context_model(id, true)
    } else { id.to_owned() }
}

fn claude_fast_state(catalog: &Value) -> Option<(bool, bool)> {
    if text(catalog, "kind") != "engine" || catalog.get("supports_fast").and_then(Value::as_bool) != Some(true) { return None; }
    let tier = catalog.pointer("/current/service_tier").and_then(Value::as_str);
    let supported = claude_context_state(catalog).is_some();
    Some((tier == Some("priority"), supported && matches!(tier, Some("default" | "priority"))))
}

fn confirmed_fast_tier(response: &Value) -> Option<&str> {
    response.get("service_tier").and_then(Value::as_str)
        .filter(|tier| response.get("ok").and_then(Value::as_bool) == Some(true) && matches!(*tier, "default" | "priority"))
}

fn codex_fast_state(catalog: &Value, live: Option<&str>) -> (bool, bool) {
    let tier = live.or_else(|| catalog.pointer("/current/service_tier").and_then(Value::as_str));
    let on = tier == Some("priority");
    let model = catalog.pointer("/current/model").and_then(Value::as_str);
    let supported = catalog.get("models").and_then(Value::as_array).is_some_and(|models| models.iter()
        .find(|m| model.is_some() && m.get("model").and_then(Value::as_str) == model)
        .and_then(|m| m.get("serviceTiers")).and_then(Value::as_array)
        .is_some_and(|tiers| tiers.iter().any(|t| t.get("id").and_then(Value::as_str) == Some("priority")
            && t.get("hidden").and_then(Value::as_bool) != Some(true))));
    (on, on || (tier == Some("default") && supported))
}

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
        if ctl == Ctl::Mode && let Some(target) = self.controls.mode_target.get(&key) { return Some(target.mode.clone()); }
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
            Ctl::Fast if self.provider().0 == "codex" => self.chat.state.codex_service_tier.clone()
                .or_else(|| self.selected.as_ref().and_then(|s| s.codex_service_tier.clone())),
            Ctl::Fast => self.selected.as_ref().filter(|_| self.provider().0 == "claude")
                .and_then(|session| self.controls.model_catalog(&key, session))
                .filter(|v| claude_fast_state(v).is_some())
                .and_then(|v| v.pointer("/current/service_tier")).and_then(Value::as_str).map(str::to_owned),
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
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if let Some(session) = self.selected.clone() { self.controls.on_session_update(&key, &session); }
        if !probe && self.controls.open.as_ref().is_some_and(|o| o.key == key && o.ctl == ctl) { self.controls.open = None; cx.notify(); return; }
        self.command_panel = false;
        self.close_recent();
        let (provider, headless) = self.provider();
        let provider = provider.to_owned();
        if provider == "claude" && ctl == Ctl::Model && self.controls.busy.contains_key(&key) { return; }
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
            // `sondar` passa por todos os modos no terminal: timeout maior que a volta completa.
            let seconds = if probe { 60 } else { 45 };
            if provider == "claude" && ctl == Ctl::Model { self.read_claude_model_catalog(api, key, seconds); }
            else {
                let (connection, tx) = (self.connection, self.tx.clone());
                self.runtime.spawn(async move {
                    let result = api.read(&key.name, &path, &query, seconds).await;
                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Catalog(ctl), result) }).await;
                });
            }
        }
        cx.notify();
    }

    fn read_claude_model_catalog(&mut self, api: Api, key: SessionKey, seconds: u64) {
        let Some(identity) = self.controls.model_identities.get(&key).cloned() else { return; };
        let (send, receive) = tokio::sync::oneshot::channel();
        self.controls.model_reads.insert(key.clone(), (identity, receive));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["model", "options"], &[], seconds).await;
            if send.send(result).is_err() { return; }
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::Reply(key, Reply::Catalog(Ctl::Model), Ok(Value::Null)) }).await;
        });
    }

    /// Alt+Shift+P e Shift+Tab no campo do Claude, como o web: o próximo modo do ciclo que a sessão aceita. Ciclo ainda
    /// não lido é lido com a sonda, como a pílula, e o modo é aplicado quando a leitura chega.
    pub(super) fn cycle_permission(&mut self, cx: &mut Context<Self>) {
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if self.provider().0 != "claude" { return; }
        if let Some((reading, steps)) = &mut self.controls.cycle_after_read && *reading == key {
            *steps += 1;
            return;
        }
        if self.controls.cycle_after_read.is_some() || self.mode_blocked(&key) { return; }
        let known = self.controls.known.get(&(key.clone(), Ctl::Mode)).cloned();
        if known.as_ref().is_some_and(|v| v.get("modes").and_then(Value::as_array).is_some_and(|m| !m.is_empty())) {
            self.apply_next_mode(key, 1, cx);
            return;
        }
        self.controls.cycle_after_read = Some((key.clone(), 1));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["permission-modes"], &[("sondar", "1")], 60).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Catalog(Ctl::Mode), result) }).await;
        });
        cx.notify();
    }

    fn apply_next_mode(&mut self, key: SessionKey, steps: usize, cx: &mut Context<Self>) {
        let catalog = self.controls.known.get(&(key.clone(), Ctl::Mode)).cloned().unwrap_or(Value::Null);
        let modes: Vec<String> = catalog.get("modes").and_then(Value::as_array).map(|m| m.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        if modes.is_empty() {
            // Sessão sem terminal responde `sondavel:false` COM a lista: só lista vazia é o dontAsk sem volta.
            if catalog.get("sondavel").and_then(Value::as_bool) == Some(false) {
                let text = super::activity::web("permissao_sem_ciclo");
                self.action_feedback.insert(key.clone(), (text.clone(), true));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.action_feedback.get(&key).is_some_and(|(t, _)| *t == text) { this.action_feedback.remove(&key); cx.notify(); }
                    });
                }).detach();
                cx.notify();
            }
            return;
        }
        self.step_mode(key, &modes, steps, cx);
    }

    /// Shift+Tab no campo do Codex, como o web: alterna entre o modo padrão e o de plano.
    pub(super) fn toggle_codex_mode(&mut self, cx: &mut Context<Self>) {
        if self.provider().0 != "codex" { return; }
        let Some(key) = self.selected_key() else { return; };
        if self.mode_blocked(&key) { return; }
        self.step_mode(key, &["plan".into(), "default".into()], 1, cx);
    }

    /// Outra pílula aplicando segura o Shift+Tab; a troca de modo em voo, não: a tecla só anda o alvo.
    fn mode_blocked(&self, key: &SessionKey) -> bool { self.controls.busy.get(key).is_some_and(|ctl| *ctl != Ctl::Mode) }

    /// Anda `steps` modos a partir do que a pílula mostra. Com a troca em voo só o alvo anda; a resposta manda o resto.
    fn step_mode(&mut self, key: SessionKey, modes: &[String], steps: usize, cx: &mut Context<Self>) {
        let from = self.ctl_label(Ctl::Mode);
        let Some(mode) = mode_ahead(modes, from.as_deref(), steps).filter(|m| from.as_deref() != Some(m.as_str())) else { return; };
        let live = self.ctl_live(Ctl::Mode);
        self.controls.mode_target.entry(key.clone()).or_insert_with(|| ModeTarget { mode: String::new(), before: live, answers: Vec::new() }).mode = mode;
        if !self.controls.busy.contains_key(&key) { self.send_mode(key, cx); }
        cx.notify();
    }

    fn send_mode(&mut self, key: SessionKey, cx: &mut Context<Self>) {
        let Some(mode) = self.controls.mode_target.get(&key).map(|target| target.mode.clone()) else { return; };
        let path = if self.provider().0 == "codex" { vec!["codex", "mode"] } else { vec!["permission-mode"] };
        // Sem conexão o pedido não sai: a pílula volta ao modo real.
        if !self.apply_ctl(Ctl::Mode, path, json!({"mode": mode}), mode, cx) { self.controls.mode_target.remove(&key); }
    }

    /// Aplicado, o rótulo vai ao `applied` e segura o modo novo enquanto o SSE repete o de antes. Se o SSE nunca mudar
    /// (o modo voltou ao de antes no terminal), o prazo devolve a pílula à fonte ao vivo.
    fn settle_mode_target(&mut self, key: SessionKey, mode: String, cx: &mut Context<Self>) {
        let Some(target) = self.controls.mode_target.remove(&key) else { return; };
        self.controls.applied.insert((key.clone(), Ctl::Mode), (mode.clone(), target.before));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                let entry = (key, Ctl::Mode);
                if this.controls.applied.get(&entry).is_some_and(|(applied, _)| *applied == mode) {
                    this.controls.applied.remove(&entry);
                    cx.notify();
                }
            });
        }).detach();
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
                let confirmed = catalog.pointer("/current/model").and_then(Value::as_str).filter(|_| engine);
                let on = confirmed.map(|model| ids.iter().position(|(id, _)| id == model || Some(id.as_str()) == claude_context_model(catalog, model)))
                    .unwrap_or_else(|| claude_model_current(&ids, &current));
                models.iter().enumerate().map(|(n, m)| {
                    let id = text(m, "id");
                    let name = Some(text(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                    if engine { choice(name, String::new(), vec!["engine", "model"], json!({"model": engine_model_choice(catalog, &id)}), on == Some(n)) }
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

    /// Devolve se o pedido saiu.
    fn apply_ctl(&mut self, ctl: Ctl, path: Vec<&'static str>, body: Value, label: String, cx: &mut Context<Self>) -> bool {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return false; };
        if self.controls.busy.contains_key(&key) || !self.chat_online { return false; }
        if self.provider().0 == "claude" && matches!(ctl, Ctl::Model | Ctl::Fast) {
            let Some(session) = self.selected.clone() else { return false; };
            self.controls.on_session_update(&key, &session);
            let catalog = self.controls.model_catalog(&key, &session);
            if (path.first() == Some(&"engine") && catalog.is_none())
                || (ctl == Ctl::Fast && !catalog.and_then(claude_fast_state).is_some_and(|(_, available)| available)) { return false; }
            self.controls.model_reads.remove(&key);
            self.controls.model_actions.insert(key.clone(), Some(ModelIdentity::of(&session)));
        }
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
        true
    }

    pub(super) fn receive_control(&mut self, key: SessionKey, reply: Reply, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.selected.clone().filter(|_| self.selected_key().as_ref() == Some(&key));
        if let Some(session) = &current { self.controls.on_session_update(&key, session); }
        match reply {
            Reply::Catalog(ctl) => {
                let claude_catalog = ctl == Ctl::Model && result.as_ref().is_ok_and(Value::is_null);
                let result = if claude_catalog {
                    let Some(session) = &current else { self.controls.model_reads.remove(&key); return; };
                    let Some(result) = self.controls.take_model_catalog(&key, &ModelIdentity::of(session)) else { return; };
                    result.and_then(|catalog| if catalog_engine_matches(&catalog, session.engine.as_deref()) { Ok(catalog) }
                        else { Err(Failure::local(tr("request_changed"))) })
                } else { result };
                if ctl == Ctl::Model {
                    // Uma nova leitura vence a confirmação anterior, inclusive quando o motor mudou.
                    self.controls.remember_model_catalog(&key, result.as_ref().ok());
                }
                if claude_catalog && let Err(error) = &result {
                    if self.chat_auth_lost(error) { self.open_connection(window, cx); }
                    self.action_feedback.insert(key.clone(), (Self::failure(error), true));
                }
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
                if ctl == Ctl::Mode && self.controls.cycle_after_read.as_ref().is_some_and(|(reading, _)| *reading == key) {
                    let steps = self.controls.cycle_after_read.take().map_or(1, |(_, steps)| steps);
                    match &result {
                        // Outra sessão na tela: o modo calculado pelo ciclo desta não se aplica àquela.
                        Ok(_) if self.selected_key().as_ref() == Some(&key) => self.apply_next_mode(key.clone(), steps, cx),
                        Ok(_) => {}
                        Err(error) => { self.action_feedback.insert(key.clone(), (Self::failure(error), true)); }
                    }
                    cx.notify();
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
                let origin = self.controls.model_actions.remove(&key);
                let claude_model_action = origin.is_some();
                if let Some(identity) = origin {
                    if identity.is_none() || identity != current.as_ref().map(ModelIdentity::of) { return; }
                }
                let claude_fast = claude_model_action && ctl == Ctl::Fast;
                let result = result.and_then(|value| {
                    if claude_fast && confirmed_fast_tier(&value).is_none() { Err(Failure::local(tr("invalid_response"))) }
                    else { Ok(value) }
                });
                match result {
                    Ok(value) => {
                        // A resposta do backend é a verdade: Pi/Kimi devolvem o que ficou de fato.
                        let label = match ctl {
                            Ctl::Mode => Some(text(&value, "mode")).filter(|m| !m.is_empty()).or_else(|| Some(text(&value, "current")).filter(|m| !m.is_empty())).unwrap_or(label),
                            Ctl::Permission => Some(text(&value, "current")).filter(|m| !m.is_empty()).unwrap_or(label),
                            Ctl::Effort => value.get("thinking").or_else(|| value.get("effort")).and_then(Value::as_str).map(str::to_owned).unwrap_or(label),
                            Ctl::Fast => value.get("service_tier").and_then(Value::as_str).map(str::to_owned).unwrap_or(label),
                            Ctl::Model => value.pointer("/current/name").or_else(|| value.get("model")).and_then(Value::as_str).map(str::to_owned).unwrap_or(label),
                        };
                        if ctl == Ctl::Mode {
                            let behind = self.controls.mode_target.get_mut(&key).is_some_and(|target| target.resend_after(&label));
                            match (behind, current.is_some()) {
                                // Teclas que chegaram com a troca em voo: o que falta sai agora, sem aviso no meio do caminho.
                                (true, true) => { self.send_mode(key, cx); cx.notify(); return; }
                                (true, false) => { self.controls.mode_target.remove(&key); }
                                // Chegou ao alvo, ou o backend recusou: a pílula assenta no modo que ficou.
                                (false, _) => self.settle_mode_target(key.clone(), label.clone(), cx),
                            }
                        }
                        let pending = value.get("pending_confirm").is_some_and(|v| !v.is_null());
                        let partial = value.get("effort_error").is_some_and(|v| !v.is_null());
                        if claude_fast && !pending { self.controls.confirm_claude_fast(&key, &label); }
                        if ctl == Ctl::Model && !pending {
                            if claude_model_action {
                                // O backend pode conservar Fast na troca de GPT; só a nova leitura diz o tier que ficou.
                                if let Some(api) = self.session_api() { self.read_claude_model_catalog(api, key.clone(), 45); }
                            } else { self.controls.known.remove(&(key.clone(), Ctl::Model)); }
                        }
                        // No Codex, guardar Fast no rótulo deixaria um valor velho vencer o SSE.
                        if !pending && !matches!(ctl, Ctl::Mode | Ctl::Fast) {
                            self.controls.applied.insert((key.clone(), ctl), (label.clone(), before));
                        }
                        if matches!(ctl, Ctl::Mode | Ctl::Permission) {
                            if let Some(known) = self.controls.known.get_mut(&(key.clone(), ctl)) { known["current"] = json!(label); }
                        }
                        if self.controls.open.as_ref().is_some_and(|o| o.key == key && o.ctl == ctl) && !partial { self.controls.open = None; }
                        let note = if pending { (tr("ctl_pending_confirm"), true) }
                            else if partial { (tr("ctl_effort_not_applied"), true) }
                            else if let Some(warnings) = reopen_warnings(&value) { (warnings, true) }
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
                        if ctl == Ctl::Mode { self.controls.mode_target.remove(&key); }
                        self.action_feedback.insert(key, (text, true));
                    }
                }
            }
            Reply::PlanReview(generation) => { self.receive_review_file(key, generation, result); }
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
                if self.controls.preselect.as_ref().is_none_or(|(owner, pending)| owner != &key || pending != &snapshot) { return; }
                self.controls.preselect = None;
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

    /// Com modelo e esforço, os dois vão numa pílula só, e o esforço mora no rodapé do menu do modelo (o `pickers` do Zeron).
    fn ctl_paired(&self) -> bool {
        let list = self.ctl_list();
        list.contains(&Ctl::Model) && list.contains(&Ctl::Effort)
    }

    /// Seletores da sessão: modelo com esforço, e permissão, em pílulas; o modo com seta.
    pub(super) fn render_ctl_pills(&self, readable: bool, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<AnyElement>) {
        let Some(key) = self.selected_key().filter(|_| readable) else { return (Vec::new(), None); };
        let busy = self.controls.busy.get(&key).copied();
        let open = self.controls.open.as_ref().filter(|o| o.key == key).map(|o| o.ctl);
        let claude = self.provider().0 == "claude";
        let paired = self.ctl_paired();
        let (mut pills, mut mode) = (Vec::new(), None);
        for ctl in self.ctl_list() {
            if paired && ctl == Ctl::Effort { continue; }
            let name = tr(&format!("ctl_{}", ctl.key()));
            let value = self.ctl_label(ctl).map(|v| shown(ctl, v)).map(|v| if claude && ctl == Ctl::Model { spaced(&v) } else { v });
            if paired && ctl == Ctl::Model {
                let effort = self.ctl_label(Ctl::Effort).map(|e| effort_label(&e));
                let fast = self.ctl_label(Ctl::Fast).as_deref() == Some("priority");
                let applying = match busy { Some(Ctl::Model) => Some(name.clone()), Some(Ctl::Effort) => Some(tr("ctl_effort")), Some(Ctl::Fast) => Some(tr("ctl_fast")), _ => None };
                let text = applying.map(|what| tr("ctl_applying").replace("{what}", &what)).or(value.clone()).unwrap_or_else(|| name.clone());
                let aria = [Some(format!("{name}: {text}")), effort.clone().map(|e| format!("{}: {e}", tr("ctl_effort"))), fast.then(|| tr("ctl_fast"))]
                    .into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let id = SharedString::from(format!("ctl-{}", ctl.key()));
                pills.push(popup::anchor(div(), id.clone()).child(super::model_pill(id, text, effort.filter(|_| busy.is_none()), (fast && busy.is_none()).then(|| tr("ctl_fast")), cx)
                    .selected(open == Some(ctl)).disabled(!self.chat_online)
                    .tooltip(format!("{name} · {}", tr("ctl_effort"))).accessibility_label(aria)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_ctl(ctl, false, cx);
                        this.focus_ctl_panel(window, cx);
                    }))).into_any_element());
                continue;
            }
            // O modo pedido pelo Shift+Tab já aparece como valor; "aplicando" fica para a troca pelo painel.
            let shifting = ctl == Ctl::Mode && self.controls.mode_target.contains_key(&key);
            let text = if busy == Some(ctl) && !shifting { tr("ctl_applying").replace("{what}", &name) } else { value.clone().unwrap_or_else(|| name.clone()) };
            let id = SharedString::from(format!("ctl-{}", ctl.key()));
            let listener = cx.listener(move |this, _, window, cx| {
                this.open_ctl(ctl, false, cx);
                this.focus_ctl_panel(window, cx);
            });
            if ctl == Ctl::Mode {
                let plan = self.ctl_label(ctl).as_deref() == Some("plan");
                let aria = format!("{name}: {text}");
                mode = Some(popup::anchor(div(), id.clone()).child(super::menu_pill(id, text, plan, cx).selected(open == Some(ctl)).disabled(!self.chat_online)
                    .tooltip(name.clone()).accessibility_label(aria)
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
            let handle = cx.focus_handle().tab_stop(true);
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
        // Escolha no painel vence o rótulo segurado do último Shift+Tab.
        if ctl == Ctl::Mode && let Some(key) = self.selected_key() {
            self.controls.applied.remove(&(key.clone(), ctl));
            self.controls.mode_target.remove(&key);
        }
        self.apply_ctl(ctl, choice.path, choice.body, choice.label, cx);
    }

    // Um destaque só: o ponteiro move o do teclado. A atual leva o fundo accent e o tique; `inline` põe a origem ao lado
    // do nome, numa linha só.
    fn ctl_row(&self, n: usize, c: Choice, ctl: Ctl, lit: bool, busy: bool, inline: bool, cx: &mut Context<Self>) -> AnyElement {
        let (current, lit, enabled) = (c.current, lit && c.enabled, c.enabled);
        // Id pelo pedido que a linha faria: filtrando, a mesma linha muda de posição e não herda o estado de outra.
        let id = SharedString::from(format!("ctl-choice-{}", c.body));
        let shown = if ctl == Ctl::Effort { effort_label(&c.label) } else { c.label };
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

    /// Rodapé do menu do modelo: os níveis de esforço do modelo atual, como o "Raciocínio" da tela sem sessão. Trocar o
    /// esforço deixa o menu aberto; o catálogo lido na abertura envelhece, então o nível da linha de status vence.
    fn render_effort_footer(&self, catalog: &Value, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let levels = self.choices(Ctl::Effort, catalog);
        if levels.is_empty() { return None; }
        let now = self.ctl_label(Ctl::Effort).unwrap_or_default();
        let live = levels.iter().any(|c| c.label.eq_ignore_ascii_case(now.trim()));
        Some(div().flex().flex_col().gap(px(2.))
            .child(popup::separator())
            .child(popup::title(tr("new_chat_reasoning"), None))
            // Até 5 níveis numa linha, dividindo a largura; mais que isso quebram linha sem cortar o rótulo. O escolhido
            // aparece pelo realce, sem ícone que alargue o botão.
            .child({
                let many = levels.len() > 5;
                div().id("ctl-efforts").role(Role::Group).aria_label(tr("new_chat_reasoning")).px(px(4.)).pb(px(2.)).flex().gap(px(2.))
                .when(many, |el| el.flex_wrap())
                .children(levels.into_iter().map(|c| {
                    let on = if live { c.label.eq_ignore_ascii_case(now.trim()) } else { c.current };
                    let label = effort_label(&c.label);
                    Button::new(SharedString::from(format!("ctl-effort-{}", c.label))).ghost().xsmall().when(!many, |b| b.flex_1().min_w_0())
                        .selected(on).label(label)
                        .disabled(busy || !c.enabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if on { return; }
                            this.apply_ctl(Ctl::Effort, c.path.clone(), c.body.clone(), c.label.clone(), cx);
                        }))
                }))
            })
            .into_any_element())
    }

    fn render_fast_footer(&self, catalog: &Value, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (on, available) = match self.provider().0 {
            "codex" => codex_fast_state(catalog, self.ctl_label(Ctl::Fast).as_deref()),
            "claude" => {
                let (Some(key), Some(session)) = (self.selected_key(), self.selected.as_ref()) else { return None; };
                self.controls.model_catalog(&key, session)?;
                if !catalog_engine_matches(catalog, session.engine.as_deref()) { return None; }
                claude_fast_state(catalog)?
            },
            _ => return None,
        };
        Some(div().flex().items_center().gap_2().px_2().py_1()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().text_sm().child(tr("ctl_fast")))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal()
                    .child(tr(if available { "ctl_fast_hint" } else { "ctl_fast_unavailable" }))))
            .child(Switch::new("ctl-fast").small().checked(on).accessibility_label(tr("ctl_fast"))
                .disabled(busy || !self.chat_online || !available)
                .on_change(cx.listener(|this, on: &bool, _, cx| {
                    let tier = if *on { "priority" } else { "default" };
                    this.apply_ctl(Ctl::Fast, vec!["service-tier"], json!({"service_tier": tier}), tier.into(), cx);
                })))
            .into_any_element())
    }

    fn render_context_footer(&self, catalog: &Value, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.provider().0 != "claude" { return None; }
        let (Some(key), Some(session)) = (self.selected_key(), self.selected.as_ref()) else { return None; };
        self.controls.model_catalog(&key, session)?;
        if !catalog_engine_matches(catalog, session.engine.as_deref()) { return None; }
        let (_, on) = claude_context_state(catalog)?;
        Some(div().flex().items_center().gap_2().px_2().py_1()
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().text_sm().child(tr("create_context_title")))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("create_engine_context_help"))))
            .child(Switch::new("ctl-context").small().checked(on).accessibility_label(tr("create_context_title"))
                .disabled(busy || !self.chat_online)
                .on_change(cx.listener(|this, on: &bool, _, cx| {
                    let (Some(key), Some(session)) = (this.selected_key(), this.selected.as_ref()) else { return; };
                    let Some(catalog) = this.controls.model_catalog(&key, session) else { return; };
                    let Some((model, _)) = claude_context_state(catalog) else { return; };
                    let model = context_model(model, *on);
                    this.apply_ctl(Ctl::Model, vec!["engine", "model"], json!({"model": model}), model, cx);
                })))
            .into_any_element())
    }

    pub(super) fn render_ctl_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.selected_key()?;
        let open = self.controls.open.as_ref().filter(|o| o.key == key)?;
        let ctl = open.ctl;
        let (provider, headless) = self.provider();
        if provider == "claude" && ctl == Ctl::Model {
            let session = self.selected.as_ref()?;
            if self.controls.model_identities.get(&key) != Some(&ModelIdentity::of(session)) { return None; }
            if let Some(Ok(catalog)) = &open.catalog {
                if !catalog_engine_matches(catalog, session.engine.as_deref()) { return None; }
            }
        }
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
                let effort = (ctl == Ctl::Model && self.ctl_paired()).then(|| self.render_effort_footer(catalog, busy, cx)).flatten();
                div().flex().flex_col().gap(px(2.))
                    .children(search)
                    .child(list)
                    .children(effort)
                    .children((ctl == Ctl::Model).then(|| self.render_fast_footer(catalog, busy, cx)).flatten())
                    .children((ctl == Ctl::Model).then(|| self.render_context_footer(catalog, busy, cx)).flatten())
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
            let list_focused = this.controls.focus.as_ref().is_some_and(|(focus, _)| focus.is_focused(window))
                || this.ctl_search.read(cx).focus_handle(cx).is_focused(window);
            if !list_focused { return; }
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
        Some(div().p(px(popup::INSET)).rounded_md().bg(theme::popup_content_fill()).flex().flex_col().gap(px(2.))
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
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if self.headless_plan().map(|(current, _)| current) != Some(id) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        if self.delivery.pending(&key) || self.pending_count() > 0 || !self.controls.planning.insert(key.clone()) { return; }
        let target = self.chat.state.claude_previous_non_plan.clone().filter(|m| !m.is_empty() && m != "plan").unwrap_or_else(|| "acceptEdits".into());
        let request = tr("plan_request");
        self.action_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let outcome = match api.act(&key.name, &["permission-mode"], Some(json!({"mode": target})), false, crate::api::REOPEN_SECONDS).await {
                Err(error) => PlanOutcome::ModeFailed(Self::failure(&error)),
                Ok(_) => match api.send(&key.name, &request).await {
                    Ok(delivery) => PlanOutcome::Sent(delivery.delivered),
                    // Incerto pode ter chegado: voltar ao plano derrubaria a implementação em curso.
                    Err(error) if error.uncertain => PlanOutcome::SendFailed { reason: Self::failure(&error), uncertain: true, reverted: false },
                    Err(error) => {
                        let reverted = api.act(&key.name, &["permission-mode"], Some(json!({"mode": "plan"})), false, crate::api::REOPEN_SECONDS).await.is_ok();
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
        let ready = !running && self.can_send() && !self.delivery.pending(&key) && self.pending_count() == 0;
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
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if self.provider() != ("claude", false) { return; }
        self.controls.plan = Some((key.clone(), None, None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["plan-preview"], &[("content", "false")], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::PlanPreview(false), result) }).await;
        });
    }

    fn open_plan_preview(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
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
        Some(SessionKey { server: self.session_server()?, name: session.name.clone(), jsonl: String::new() })
    }

    fn prethread_snapshot(&self) -> Option<String> {
        let session = self.selected.as_ref()?;
        (session.state == "awaiting_input").then(|| json!([session.question, session.options, session.interaction_id]).to_string())
    }

    fn preselect(&mut self, option: usize, snapshot: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.prethread_key()) else { return; };
        if self.controls.preselect.is_some() { return; }
        if self.prethread_snapshot().as_deref() != Some(snapshot.as_str()) {
            self.action_feedback.insert(key, (tr("request_changed"), true));
            cx.notify();
            return;
        }
        let Some(body) = select_body(&snapshot, Some(option)) else { return; };
        self.controls.preselect = Some((key.clone(), snapshot.clone()));
        self.action_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["select"], Some(body), false, 30).await;
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
                let failed = session.state == "dead" || session.problema.as_deref().is_some_and(|p| !p.trim().is_empty());
                let opening = !failed && session.state != "awaiting_input";
                let label = session.label.clone().filter(|l| !l.trim().is_empty())
                    .or_else(|| session.question.clone().filter(|q| !q.trim().is_empty()))
                    .or_else(|| session.startup_steps.last().cloned())
                    .unwrap_or_else(|| tr(if session.state == "dead" { "chip_dead" } else { "prethread_waiting" }));
                let color = if failed { theme::warning() } else { theme::accent() };
                for step in session.startup_steps.iter().take(session.startup_steps.len().saturating_sub(1)) {
                    body = body.child(div().text_xs().text_color(theme::muted()).child(step.clone()));
                }
                body = body.child(div().flex().items_center().gap_2().text_sm().text_color(color)
                    .when(opening, |el| el.child(self.working_mark_slot(panes::Area::Conversation, "prethread-mark", 16., color)))
                    .child(div().flex_1().min_w_0().child(label.clone())));
                if opening && let Some(since) = self.controls.prethread_since {
                    body = body.child(div().h(px(20.)).flex().items_center().gap_2().text_xs().text_color(theme::muted())
                        .child(tr("prethread_elapsed"))
                        .child(self.elapsed_slot(panes::Area::Conversation, "prethread-elapsed", since, None)));
                }
                if let Some(problem) = session.problema.as_ref().filter(|p| !p.trim().is_empty() && *p != &label) {
                    body = body.child(div().text_sm().text_color(theme::warning()).child(prethread_problem(problem)));
                }
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
    use super::{Choice, ModeTarget, claude_effort_current, claude_model_current, codex_fast_state, keep, mode_ahead, only_match, prethread_problem, reopen_warnings, spaced, step_free};

    #[test]
    fn prethread_problem_shows_the_phrase_never_the_raw_code() {
        crate::i18n::set_language(crate::appearance::Language::Pt);
        assert_eq!(prethread_problem("headless_processo_caiu"), "O processo do Claude caiu; o próximo prompt tenta de novo");
        assert_eq!(prethread_problem("codex_limite_uso"), crate::i18n::tr_shared("problema_codex_limite_uso", &[]));
        assert_ne!(prethread_problem("codex_limite_uso"), "codex_limite_uso");
        // Texto livre do servidor, sem frase própria, aparece como veio.
        assert_eq!(prethread_problem("broken config"), "broken config");
    }

    #[test]
    fn reopen_warnings_join_what_the_server_sent() {
        assert_eq!(reopen_warnings(&serde_json::json!({"ok": true, "avisos": ["motor saiu", "registro não gravado"]})).as_deref(),
            Some("motor saiu · registro não gravado"));
        assert_eq!(reopen_warnings(&serde_json::json!({"ok": true})), None);
        assert_eq!(reopen_warnings(&serde_json::json!({"ok": true, "avisos": []})), None);
    }

    #[test]
    fn mode_reply_resends_only_while_the_backend_moves() {
        let mut target = ModeTarget { mode: "plan".into(), before: Some("manual".into()), answers: Vec::new() };
        // Teclas em voo: a resposta trouxe o primeiro passo, e o resto sai.
        assert!(target.resend_after("acceptEdits"));
        assert!(!target.resend_after("plan"));
        // Modo recusado: o backend devolve o mesmo modo de novo, e o pedido para em vez de repetir.
        let mut refused = ModeTarget { mode: "auto".into(), before: Some("manual".into()), answers: Vec::new() };
        assert!(refused.resend_after("manual"));
        assert!(!refused.resend_after("manual"));
    }

    #[test]
    fn shift_tab_steps_walk_the_cycle_and_wrap() {
        let cycle: Vec<String> = ["manual", "acceptEdits", "plan", "auto"].map(String::from).to_vec();
        assert_eq!(mode_ahead(&cycle, Some("manual"), 1).as_deref(), Some("acceptEdits"));
        // Três Shift+Tab rápidos viram um pedido só, três modos à frente.
        assert_eq!(mode_ahead(&cycle, Some("manual"), 3).as_deref(), Some("auto"));
        assert_eq!(mode_ahead(&cycle, Some("auto"), 1).as_deref(), Some("manual"));
        assert_eq!(mode_ahead(&cycle, Some("plan"), 6).as_deref(), Some("manual"));
        // Modo fora do ciclo (ou desconhecido): o primeiro Shift+Tab vai ao primeiro do ciclo.
        assert_eq!(mode_ahead(&cycle, None, 1).as_deref(), Some("manual"));
        assert_eq!(mode_ahead(&cycle, Some("dontAsk"), 2).as_deref(), Some("acceptEdits"));
        assert_eq!(mode_ahead(&[], Some("plan"), 1), None);
        // O Codex alterna entre plano e padrão; outro modo vai ao plano, como antes.
        let codex: Vec<String> = ["plan", "default"].map(String::from).to_vec();
        assert_eq!(mode_ahead(&codex, Some("plan"), 1).as_deref(), Some("default"));
        assert_eq!(mode_ahead(&codex, Some("default"), 1).as_deref(), Some("plan"));
        assert_eq!(mode_ahead(&codex, Some("full-auto"), 1).as_deref(), Some("plan"));
    }

    #[test]
    fn codex_fast_uses_the_selected_models_catalog_and_confirmed_tier() {
        let mut catalog = serde_json::json!({
            "current": {"model": "gpt-supported", "service_tier": "default"},
            "models": [
                {"model": "gpt-supported", "serviceTiers": [{"id": "priority", "name": "Fast"}]},
                {"model": "gpt-standard", "serviceTiers": []}
            ]
        });
        assert_eq!(codex_fast_state(&catalog, None), (false, true));
        assert_eq!(codex_fast_state(&catalog, Some("priority")), (true, true));
        catalog["current"]["model"] = serde_json::json!("gpt-standard");
        assert_eq!(codex_fast_state(&catalog, None), (false, false));
        assert_eq!(codex_fast_state(&catalog, Some("priority")), (true, true));
        catalog["current"]["service_tier"] = serde_json::json!("priority");
        assert_eq!(codex_fast_state(&catalog, Some("default")), (false, false));
    }

    #[test]
    fn codex_fast_stays_disabled_until_the_current_tier_is_known() {
        let catalog = serde_json::json!({
            "current": {"model": "gpt-supported"},
            "models": [{"model": "gpt-supported", "serviceTiers": [{"id": "priority"}]}]
        });
        assert_eq!(codex_fast_state(&catalog, None), (false, false));
        assert_eq!(codex_fast_state(&catalog, Some("default")), (false, true));
        assert_eq!(codex_fast_state(&serde_json::json!({}), Some("priority")), (true, true));
        let hidden = serde_json::json!({"current": {"model": "m"}, "models": [{"model": "m", "serviceTiers": [{"id": "priority", "hidden": true}]}]});
        assert_eq!(codex_fast_state(&hidden, Some("default")), (false, false));
    }

    #[test]
    fn claude_fast_requires_local_engine_capability_and_confirmed_model_tier() {
        let mut catalog = serde_json::json!({
            "kind": "engine", "supports_fast": true,
            "current": {"model": "gpt-supported", "service_tier": "default"},
            "models": [{"id": "gpt-supported", "supports_fast": true}, {"id": "other", "supports_fast": false}]
        });
        assert_eq!(super::claude_fast_state(&catalog), Some((false, true)));
        catalog["current"]["service_tier"] = serde_json::json!("priority");
        assert_eq!(super::claude_fast_state(&catalog), Some((true, true)));
        catalog["current"]["model"] = serde_json::json!("other");
        assert_eq!(super::claude_fast_state(&catalog), Some((true, false)));
        catalog["current"]["model"] = serde_json::json!("account/gpt-supported");
        assert_eq!(super::claude_fast_state(&catalog), Some((true, false)));
        catalog["current"]["model"] = serde_json::json!("gpt-supported");
        catalog["current"]["service_tier"] = serde_json::Value::Null;
        assert_eq!(super::claude_fast_state(&catalog), Some((false, false)));
        catalog["supports_fast"] = serde_json::json!(false);
        assert_eq!(super::claude_fast_state(&catalog), None);
        catalog["supports_fast"] = serde_json::json!(true);
        catalog["kind"] = serde_json::json!("claude");
        assert_eq!(super::claude_fast_state(&catalog), None);
    }

    #[test]
    fn engine_context_uses_confirmed_model_and_keeps_suffix_only_for_local_gpt() {
        let mut catalog = serde_json::json!({
            "kind":"engine", "supports_fast":true,
            "current":{"model":"gpt-one[1m]", "service_tier":"priority"},
            "models":[{"id":"gpt-one", "supports_fast":true}, {"id":"gpt-two", "supports_fast":true},
                {"id":"other", "supports_fast":false}, {"id":"opus"}, {"id":"opus[1m]"}]
        });
        assert_eq!(super::claude_context_state(&catalog), Some(("gpt-one", true)));
        assert_eq!(super::claude_fast_state(&catalog), Some((true, true)));
        assert_eq!(super::engine_model_choice(&catalog, "gpt-two"), "gpt-two[1m]");
        assert_eq!(serde_json::json!({"model":super::engine_model_choice(&catalog, "gpt-two")}), serde_json::json!({"model":"gpt-two[1m]"}));
        assert_eq!(super::engine_model_choice(&catalog, "other"), "other");
        assert_eq!(super::context_model("gpt-one[1m]", false), "gpt-one");
        assert_eq!(super::context_model("gpt-one[1m]", true), "gpt-one[1m]");
        catalog["current"]["model"] = serde_json::json!("gpt-one");
        assert_eq!(super::claude_context_state(&catalog), Some(("gpt-one", false)));
        assert_eq!(super::engine_model_choice(&catalog, "gpt-two"), "gpt-two");
        for invalid in ["account/gpt-one[1m]", "other[1m]", "gpt-one[1m][1m]"] {
            catalog["current"]["model"] = serde_json::json!(invalid);
            assert_eq!(super::claude_context_state(&catalog), None);
            assert_eq!(super::claude_fast_state(&catalog), Some((true, false)));
        }
        catalog["current"]["model"] = serde_json::json!("gpt-one[1m]");
        catalog["supports_fast"] = serde_json::json!(false);
        assert_eq!(super::claude_context_state(&catalog), None);
        assert_eq!(super::engine_model_choice(&catalog, "gpt-two"), "gpt-two");
        catalog["supports_fast"] = serde_json::json!(true);
        for kind in ["claude", "codex"] {
            catalog["kind"] = serde_json::json!(kind);
            assert_eq!(super::claude_context_state(&catalog), None);
            assert_eq!(super::engine_model_choice(&catalog, "opus[1m]"), "opus[1m]");
        }
        assert_eq!(claude_model_current(&[("opus".into(), false), ("opus[1m]".into(), false)], "opus[1m]"), Some(1));
    }

    #[test]
    fn claude_fast_confirmation_is_scoped_and_fresh_catalog_wins() {
        let key = super::SessionKey { server: "machine".into(), name: "session".into(), jsonl: "conversation".into() };
        let other = super::SessionKey { server: "another".into(), ..key.clone() };
        let catalog = serde_json::json!({"kind":"engine", "supports_fast":true,
            "current":{"model":"gpt", "service_tier":"default"}, "models":[{"id":"gpt", "supports_fast":true}]});
        let mut controls = super::Controls::default();
        controls.remember_model_catalog(&key, Some(&catalog));
        controls.remember_model_catalog(&other, Some(&catalog));
        controls.open = Some(super::Open { key: key.clone(), ctl: super::Ctl::Model, catalog: Some(Ok(catalog.clone())) });
        let response = serde_json::json!({"ok":true, "service_tier":"priority"});
        controls.confirm_claude_fast(&key, super::confirmed_fast_tier(&response).unwrap());
        assert_eq!(super::claude_fast_state(&controls.known[&(key.clone(), super::Ctl::Model)]), Some((true, true)));
        assert_eq!(super::claude_fast_state(controls.open.as_ref().unwrap().catalog.as_ref().unwrap().as_ref().unwrap()), Some((true, true)));
        assert_eq!(controls.known[&(other, super::Ctl::Model)], catalog);
        assert!(controls.applied.is_empty());
        controls.remember_model_catalog(&key, Some(&catalog));
        assert_eq!(super::claude_fast_state(&controls.known[&(key.clone(), super::Ctl::Model)]), Some((false, true)));
        controls.remember_model_catalog(&key, None);
        assert!(!controls.known.contains_key(&(key, super::Ctl::Model)));
        for invalid in [serde_json::json!({"ok":true}), serde_json::json!({"service_tier":"priority"}),
            serde_json::json!({"ok":false, "service_tier":"priority"}), serde_json::json!({"ok":true, "service_tier":"fast"})] {
            assert_eq!(super::confirmed_fast_tier(&invalid), None);
        }
    }

    #[test]
    fn replaced_model_reads_and_reads_before_fast_change_cannot_overwrite_confirmation() {
        let key = super::SessionKey { server: "machine".into(), name: "session".into(), jsonl: "conversation".into() };
        let mut controls = super::Controls::default();
        let identity = super::ModelIdentity::of(&super::SessionInfo { provider: "claude".into(), engine: Some("proxy".into()), ..Default::default() });
        let (old_send, old_receive) = tokio::sync::oneshot::channel();
        controls.model_reads.insert(key.clone(), (identity.clone(), old_receive));
        old_send.send(Ok(serde_json::json!({"current":{"service_tier":"default"}}))).unwrap();
        let (new_send, new_receive) = tokio::sync::oneshot::channel();
        controls.model_reads.insert(key.clone(), (identity.clone(), new_receive));
        // A notificação antiga já enfileirada não consome uma leitura nova ainda incompleta.
        assert!(controls.take_model_catalog(&key, &identity).is_none());
        let newest = serde_json::json!({"current":{"service_tier":"priority"}});
        new_send.send(Ok(newest.clone())).unwrap();
        assert_eq!(controls.take_model_catalog(&key, &identity).unwrap().unwrap(), newest);
        assert!(controls.take_model_catalog(&key, &identity).is_none());
        let (send, receive) = tokio::sync::oneshot::channel();
        controls.model_reads.insert(key.clone(), (identity.clone(), receive));
        controls.model_reads.remove(&key);
        assert!(send.send(Ok(serde_json::Value::Null)).is_err());
        assert!(controls.take_model_catalog(&key, &identity).is_none());
    }

    #[test]
    fn claude_model_refresh_keeps_confirmed_priority_and_unknown_stays_unknown() {
        let key = super::SessionKey { server: "machine".into(), name: "session".into(), jsonl: "conversation".into() };
        let session = super::SessionInfo { provider: "claude".into(), engine: Some("proxy".into()), ..Default::default() };
        let identity = super::ModelIdentity::of(&session);
        let mut controls = super::Controls::default();
        let mut catalog = serde_json::json!({"kind":"engine", "engine":"proxy", "supports_fast":true,
            "current":{"model":"gpt-one", "service_tier":"priority"},
            "models":[{"id":"gpt-one", "supports_fast":true}, {"id":"gpt-two", "supports_fast":true}]});
        controls.on_session_update(&key, &session);
        controls.remember_model_catalog(&key, Some(&catalog));
        let (send, receive) = tokio::sync::oneshot::channel();
        controls.model_reads.insert(key.clone(), (identity.clone(), receive));
        controls.on_session_update(&key, &session);
        assert_eq!(super::claude_fast_state(controls.model_catalog(&key, &session).unwrap()), Some((true, true)));
        assert!(controls.take_model_catalog(&key, &identity).is_none());
        catalog["current"]["model"] = serde_json::json!("gpt-two");
        send.send(Ok(catalog)).unwrap();
        let refreshed = controls.take_model_catalog(&key, &identity).unwrap().unwrap();
        controls.remember_model_catalog(&key, Some(&refreshed));
        assert_eq!(controls.model_catalog(&key, &session).unwrap().pointer("/current/model"), Some(&serde_json::json!("gpt-two")));
        assert_eq!(super::claude_fast_state(controls.model_catalog(&key, &session).unwrap()), Some((true, true)));
        let mut unknown = refreshed;
        unknown["current"]["service_tier"] = serde_json::Value::Null;
        controls.remember_model_catalog(&key, Some(&unknown));
        assert_eq!(super::claude_fast_state(controls.model_catalog(&key, &session).unwrap()), Some((false, false)));
        assert!(controls.model_catalog(&key, &session).unwrap().pointer("/current/service_tier").unwrap().is_null());
    }

    #[test]
    fn model_identity_changes_invalidate_catalog_reads_and_actions_even_after_switching_back() {
        let key = super::SessionKey { server: "machine".into(), name: "session".into(), jsonl: "conversation".into() };
        let session = super::SessionInfo { provider: "claude".into(), engine: Some("proxy".into()),
            engine_account: Some("account-one".into()), conta: Some("claude-one".into()), lifecycle_id: Some("first".into()), ..Default::default() };
        let identity = super::ModelIdentity::of(&session);
        let catalog = serde_json::json!({"kind":"engine", "engine":"proxy", "supports_fast":true,
            "current":{"model":"gpt", "service_tier":"priority"}, "models":[{"id":"gpt", "supports_fast":true}]});
        let mut changes = Vec::new();
        let mut changed = session.clone(); changed.engine = None; changes.push(changed);
        let mut changed = session.clone(); changed.engine = Some("another-proxy".into()); changes.push(changed);
        let mut changed = session.clone(); changed.engine_account = Some("account-two".into()); changes.push(changed);
        let mut changed = session.clone(); changed.conta = Some("claude-two".into()); changes.push(changed);
        let mut changed = session.clone(); changed.lifecycle_id = Some("second".into()); changes.push(changed);
        let mut changed = session.clone(); changed.provider = "codex".into(); changes.push(changed);
        for changed in changes {
            let mut controls = super::Controls::default();
            controls.on_session_update(&key, &session);
            controls.remember_model_catalog(&key, Some(&catalog));
            controls.open = Some(super::Open { key: key.clone(), ctl: super::Ctl::Model, catalog: Some(Ok(catalog.clone())) });
            controls.applied.insert((key.clone(), super::Ctl::Model), ("gpt".into(), None));
            controls.busy.insert(key.clone(), super::Ctl::Fast);
            controls.model_actions.insert(key.clone(), Some(identity.clone()));
            let (send, receive) = tokio::sync::oneshot::channel();
            controls.model_reads.insert(key.clone(), (identity.clone(), receive));
            send.send(Ok(catalog.clone())).unwrap();
            assert!(controls.model_catalog(&key, &changed).is_none());
            controls.on_session_update(&key, &changed);
            assert!(controls.model_catalog(&key, &changed).is_none());
            assert!(!controls.known.contains_key(&(key.clone(), super::Ctl::Model)));
            assert!(!controls.applied.contains_key(&(key.clone(), super::Ctl::Model)));
            assert!(controls.open.is_none());
            assert!(controls.take_model_catalog(&key, &identity).is_none());
            assert_eq!(controls.model_actions.get(&key), Some(&None));
            assert_eq!(controls.busy.get(&key), Some(&super::Ctl::Fast));
            controls.on_session_update(&key, &session);
            assert_eq!(controls.model_actions.remove(&key), Some(None));
            assert!(controls.model_catalog(&key, &session).is_none());
        }
        assert!(!super::catalog_engine_matches(&catalog, None));
        assert!(!super::catalog_engine_matches(&catalog, Some("another-proxy")));
        let mut controls = super::Controls::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        controls.model_reads.insert(key.clone(), (identity.clone(), receive));
        send.send(Ok(catalog)).unwrap();
        let mut different = identity.clone(); different.engine = None;
        assert!(controls.take_model_catalog(&key, &different).is_none());
        assert!(controls.take_model_catalog(&key, &identity).is_none());
        controls.model_actions.insert(key.clone(), Some(identity));
        controls.on_select();
        assert_eq!(controls.model_actions.remove(&key), Some(None));
    }

    #[test]
    fn leaving_before_fast_confirmation_or_model_refresh_drops_the_old_badge() {
        let key = super::SessionKey { server: "machine".into(), name: "session".into(), jsonl: "conversation".into() };
        let session = super::SessionInfo { provider: "claude".into(), engine: Some("proxy".into()), ..Default::default() };
        let identity = super::ModelIdentity::of(&session);
        for refreshing in [false, true] {
            let mut controls = super::Controls::default();
            let mut catalog = serde_json::json!({"kind":"engine", "engine":"proxy", "supports_fast":true,
                "current":{"model":"gpt", "service_tier":"priority"}, "models":[{"id":"gpt", "supports_fast":true}]});
            controls.on_session_update(&key, &session);
            controls.remember_model_catalog(&key, Some(&catalog));
            controls.applied.insert((key.clone(), super::Ctl::Model), ("gpt".into(), None));
            if refreshing {
                let (send, receive) = tokio::sync::oneshot::channel();
                controls.model_reads.insert(key.clone(), (identity.clone(), receive));
                send.send(Ok(catalog.clone())).unwrap();
            } else { controls.model_actions.insert(key.clone(), Some(identity.clone())); }
            controls.on_select();
            controls.on_session_update(&key, &session);
            assert!(controls.model_catalog(&key, &session).is_none());
            assert!(!controls.applied.contains_key(&(key.clone(), super::Ctl::Model)));
            assert!(controls.take_model_catalog(&key, &identity).is_none());
            assert_eq!(controls.model_actions.remove(&key), if refreshing { None } else { Some(None) });
            assert!(controls.model_catalog(&key, &session).is_none());
            catalog["current"]["service_tier"] = serde_json::json!("default");
            controls.remember_model_catalog(&key, Some(&catalog));
            assert_eq!(super::claude_fast_state(controls.model_catalog(&key, &session).unwrap()), Some((false, true)));
        }
    }

    #[test]
    fn claude_model_label_gets_its_spaces_back() {
        assert_eq!(spaced("Opus5.5·1M"), "Opus 5.5 · 1M");
        assert_eq!(spaced("Sonnet4.6"), "Sonnet 4.6");
        assert_eq!(spaced("Opus 5.5 · 1M"), "Opus 5.5 · 1M");
        assert_eq!(spaced("k3"), "k3");
    }

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
