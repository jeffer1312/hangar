//! Painel direito da sessão: estado, contexto, limites, projeto e atalhos. Tudo lido de fontes do
//! backend (stream, lista, rotas); nenhuma ação sai sem gesto, e desconhecido aparece como tal.
use super::*;
use crate::status::StatusFields;

const MIN_WIDTH: f32 = 240.;
const MAX_WIDTH: f32 = 480.;
const SIDEBAR: f32 = 270.;
// Largura que a conversa mantém; abaixo disso o painel sai de cena em vez de espremer o texto.
const CHAT_MIN: f32 = 540.;
const COST_EVERY: u64 = 30;
const DIFF_MAX: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shortcut {
    Send { label: String, text: String, direct: bool, confirm: bool },
    Shell { label: String, command: String, confirm: bool },
    Attach,
}

impl Shortcut {
    fn confirm(&self) -> bool { matches!(self, Shortcut::Send { confirm: true, .. } | Shortcut::Shell { confirm: true, .. }) }
    fn label(&self) -> String {
        match self { Shortcut::Send { label, .. } | Shortcut::Shell { label, .. } => label.clone(), Shortcut::Attach => tr("attach") }
    }
}

#[derive(Clone, Debug)]
pub(super) struct GitFile { path: String, code: String, added: Option<i64>, removed: Option<i64> }

#[derive(Clone, Debug)]
struct Cost { usd: Option<f64>, has_usage: bool, missing: Vec<String> }

pub(super) struct Side {
    pub open: bool,
    width: f32,
    drag: Option<(f32, f32)>,
    shortcuts: Option<Result<Vec<Shortcut>, String>>,
    // Custo do Codex: o último valor fica visível quando uma leitura falha; o erro vai junto.
    cost: Option<(SessionKey, Option<Cost>, Option<String>)>,
    cost_task: Option<(SessionKey, JoinHandle<()>)>,
    cost_gen: u64,
    files: Option<(SessionKey, Option<Result<Vec<GitFile>, String>>)>,
    diff: Option<(SessionKey, String, Option<Result<(String, bool), String>>)>,
    reloading: HashSet<SessionKey>,
}

impl Default for Side {
    fn default() -> Self {
        Self { open: true, width: 320., drag: None, shortcuts: None, cost: None, cost_task: None, cost_gen: 0,
            files: None, diff: None, reloading: HashSet::new() }
    }
}

impl Side {
    pub fn reset_server(&mut self) {
        self.shortcuts = None;
        self.stop_cost();
        self.cost = None;
        self.on_select();
        self.reloading.clear();
    }

    pub fn on_select(&mut self) {
        self.files = None;
        self.diff = None;
    }

    fn stop_cost(&mut self) {
        if let Some((_, task)) = self.cost_task.take() { task.abort(); }
        self.cost_gen += 1;
    }

    pub fn receive_config(&mut self, result: Result<Value, String>) {
        self.shortcuts = Some(result.map(|config| parse_shortcuts(
            config.pointer("/campos/shortcuts/valor").and_then(Value::as_str).unwrap_or(""))));
    }

    // Largura efetiva: nunca tira da conversa menos que CHAT_MIN; sem espaço, o painel não aparece.
    fn fitted(&self, viewport: f32) -> Option<f32> {
        let room = viewport - SIDEBAR - CHAT_MIN;
        (room >= MIN_WIDTH).then(|| self.width.clamp(MIN_WIDTH, MAX_WIDTH).min(room))
    }
}

/// Porta de `resolveShortcuts`: vazio ou inválido volta ao conjunto nativo; item ruim sai sozinho.
/// Dos internos só "anexos" existe aqui; terminal, navegador, modo e rodar são módulos à parte.
fn parse_shortcuts(raw: &str) -> Vec<Shortcut> {
    let defaults = vec![Shortcut::Attach];
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw.trim()) else { return defaults; };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool);
    let mut seen = HashSet::new();
    items.iter().filter_map(|item| {
        let id = text(item, "id")?;
        if !seen.insert(id) { return None; }
        let confirm = flag(item, "confirm").unwrap_or(false);
        match item.get("type").and_then(Value::as_str)? {
            "send_text" => Some(Shortcut::Send { label: text(item, "label")?, text: text(item, "text")?, direct: flag(item, "send_direct").unwrap_or(true), confirm }),
            "shell" => Some(Shortcut::Shell { label: text(item, "label")?, command: text(item, "command")?, confirm }),
            "internal" if item.get("action").and_then(Value::as_str) == Some("anexos") => Some(Shortcut::Attach),
            _ => None,
        }
    }).collect()
}

/// Tokens como o painel web: milhar arredondado em "k", milhão com uma casa, menos de mil cru.
pub(super) fn tokens(n: f64) -> String {
    if n >= 1e6 { format!("{}M", trim_zero(format!("{:.1}", n / 1e6))) }
    else if n >= 1e3 { format!("{}k", (n / 1e3).round()) }
    else { format!("{}", n.round()) }
}

fn trim_zero(s: String) -> String { s.strip_suffix(".0").map(str::to_owned).unwrap_or(s) }

fn money(usd: f64) -> String { format!("US$ {usd:.2}") }

fn duration(ms: f64) -> String {
    let s = ms / 1000.;
    if s < 10. { format!("{s:.1}s") } else if s < 60. { format!("{}s", s.round()) }
    else if s < 3600. { format!("{}m{:02}s", (s / 60.).floor(), (s % 60.).floor()) }
    else { format!("{}h{:02}m", (s / 3600.).floor(), ((s % 3600.) / 60.).floor()) }
}

fn ago(seconds: f64) -> String {
    let s = seconds.max(0.);
    if s < 60. { tr("ago_now") }
    else if s < 3600. { tr("ago_min").replace("{n}", &(s / 60.).floor().to_string()) }
    else if s < 86_400. { tr("ago_h").replace("{n}", &(s / 3600.).floor().to_string()) }
    else { tr("ago_d").replace("{n}", &(s / 86_400.).floor().to_string()) }
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

/// Aviso do painel (`.aviso` do web): alerta em âmbar, grave em vermelho.
fn notice(text: String, serious: bool) -> Div {
    let color = if serious { theme::danger() } else { theme::warning() };
    div().flex().flex_col().gap_2().px_3().py_2().rounded(px(12.)).border_1().border_color(color.opacity(0.34)).bg(color.opacity(0.10))
        .child(div().flex().items_start().gap_2()
            .child(chrome::small_icon(IconName::Info, 15., color))
            .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::muted()).child(text)))
}

fn notice_button(id: &'static str, label: String, serious: bool, cx: &App) -> Button {
    let color = if serious { theme::danger() } else { theme::warning() };
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(color).hover(color.opacity(0.14)).active(color.opacity(0.2)))
        .xsmall().rounded_full().border_1().border_color(color.opacity(0.45)).px(px(10.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(label))
}

fn agent_label(provider: &str) -> String {
    let mut chars = provider.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub(super) fn stats_line(stats: &Stats) -> String {
    let mut parts = vec![
        tr(if stats.turns == 1 { "stats_turn" } else { "stats_turns" }).replace("{n}", &stats.turns.to_string()),
        tr(if stats.steps == 1 { "stats_step" } else { "stats_steps" }).replace("{n}", &stats.steps.to_string()),
        tr("stats_tokens").replace("{in}", &tokens(stats.in_tok as f64)).replace("{out}", &tokens(stats.out_tok as f64)),
    ];
    if let Some(ms) = stats.llm_ms.filter(|v| *v > 0.) {
        parts.push(tr("stats_llm").replace("{d}", &duration(ms)));
        if let Some(tool) = stats.tool_ms.filter(|v| *v > 0.) { parts.push(tr("stats_tools").replace("{d}", &duration(tool))); }
    }
    if let Some(rate) = stats.tok_s.filter(|v| *v > 0.) { parts.push(tr("stats_rate").replace("{n}", &rate.round().to_string())); }
    if let Some(ms) = stats.ttft_ms.filter(|v| *v > 0.) { parts.push(tr("stats_ttft").replace("{d}", &duration(ms))); }
    if let Some(cache) = stats.cache_pct { parts.push(tr("stats_cache").replace("{n}", &cache.round().to_string())); }
    parts.join(" · ")
}

impl Hangar {
    pub(super) fn toggle_side(&mut self, cx: &mut Context<Self>) {
        self.side.open = !self.side.open;
        if !self.side.open { self.side.stop_cost(); }
        cx.notify();
    }

    pub(super) fn drag_side(&mut self, x: f32, pressed: bool, cx: &mut Context<Self>) {
        let Some((start_x, start_width)) = self.side.drag else { return; };
        if !pressed { self.side.drag = None; cx.notify(); return; }
        self.side.width = (start_width + start_x - x).clamp(MIN_WIDTH, MAX_WIDTH);
        cx.notify();
    }

    pub(super) fn side_dragging(&self) -> bool { self.side.drag.is_some() }

    pub(super) fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.side.drag.take().is_some() { cx.notify(); }
    }

    // Custo do Codex: só com o painel visível e a sessão aberta; troca de sessão cancela a leitura em curso.
    fn sync_cost(&mut self, visible: bool) {
        let want = self.selected_key().filter(|_| visible && self.provider().0 == "codex" && self.chat_online);
        if self.side.cost_task.as_ref().map(|(key, _)| key) == want.as_ref() { return; }
        self.side.stop_cost();
        let (Some(key), Some(api)) = (want, self.api.clone()) else { return; };
        if self.side.cost.as_ref().is_some_and(|(owner, ..)| owner != &key) { self.side.cost = None; }
        let (connection, tx, generation, name) = (self.connection, self.tx.clone(), self.side.cost_gen, key.name.clone());
        let owner = key.clone();
        let task = self.runtime.spawn(async move {
            loop {
                let result = api.read(&name, &["cost"], &[], 25).await;
                if tx.send(Envelope { connection, selection: None, payload: Payload::Reply(owner.clone(), Reply::Cost(generation), result) }).await.is_err() { return; }
                tokio::time::sleep(Duration::from_secs(COST_EVERY)).await;
            }
        });
        self.side.cost_task = Some((key, task));
    }

    fn load_files(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        self.side.files = Some((key.clone(), None));
        self.side.diff = None;
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["git", "files"], &[], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::GitFiles, result) }).await;
        });
        cx.notify();
    }

    // POST só de leitura: o backend confere que o caminho está na lista de alterados.
    fn open_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        if self.side.diff.as_ref().is_some_and(|(owner, current, _)| owner == &key && current == &path) { self.side.diff = None; cx.notify(); return; }
        self.side.diff = Some((key.clone(), path.clone(), None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["git", "diff"], Some(json!({"path": path})), false, 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Diff(path), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn prefill(&mut self, text: &str, protect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).value().to_string();
        if protect && !current.trim().is_empty() && current != text {
            self.confirm = Some(Confirm::Prefill(text.to_owned()));
            cx.notify();
            return;
        }
        self.composer.update(cx, |input, cx| { input.set_value(text.to_owned(), window, cx); input.focus(window, cx); });
        cx.notify();
    }

    pub(super) fn run_shortcut(&mut self, shortcut: Shortcut, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        if shortcut.confirm() && !confirmed {
            self.confirm = Some(Confirm::Shortcut(shortcut.label(), shortcut));
            cx.notify();
            return;
        }
        match shortcut {
            Shortcut::Attach => self.pick_files(cx),
            Shortcut::Send { text, direct: false, .. } => self.prefill(&text, true, window, cx),
            Shortcut::Send { text, .. } => {
                if !self.can_send() || self.delivery.pending(&key) || self.uploading.contains_key(&key) {
                    self.action_feedback.insert(key, (tr("shortcut_busy"), true));
                } else {
                    let known = self.known_user_ids();
                    self.deliver(key, text, String::new(), false, known, cx);
                }
            }
            Shortcut::Shell { label, command, .. } => {
                let Some(api) = self.api.clone() else { return; };
                self.action_feedback.insert(key.clone(), (tr("shortcut_started").replace("{label}", &label), false));
                let (connection, tx) = (self.connection, self.tx.clone());
                self.runtime.spawn(async move {
                    let result = api.act(&key.name, &["shortcut-shell"], Some(json!({"command": command})), false, 30).await;
                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Shell(label), result) }).await;
                });
            }
        }
        cx.notify();
    }

    fn reload_allowed(&self) -> bool {
        self.chat_online && self.chat.state.state == "idle"
            && self.selected_key().is_some_and(|key| !self.side.reloading.contains(&key))
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.reload_allowed() { cx.notify(); return; }
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return; };
        self.side.reloading.insert(key.clone());
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["recarregar"], None, false, 60).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Reload, result) }).await;
        });
        cx.notify();
    }

    fn read_failure(error: &Failure) -> String {
        if error.uncertain && error.status.is_none() { tr("network_error") } else { Self::fetch_failure(error) }
    }

    pub(super) fn receive_reply(&mut self, key: SessionKey, reply: Reply, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            Reply::Cost(generation) => {
                if generation != self.side.cost_gen { return; }
                let previous = self.side.cost.take().filter(|(owner, ..)| owner == &key).and_then(|(_, cost, _)| cost);
                self.side.cost = Some(match result {
                    Ok(value) => (key, Some(Cost {
                        usd: value.get("cost_usd").and_then(Value::as_f64),
                        has_usage: value.get("has_usage").and_then(Value::as_bool).unwrap_or(false),
                        missing: value.get("missing_models").and_then(Value::as_array)
                            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
                    }), None),
                    Err(error) => (key, previous, Some(Self::failure(&error))),
                });
            }
            Reply::GitFiles => {
                let Some((owner, slot)) = self.side.files.as_mut() else { return; };
                if owner != &key { return; }
                *slot = Some(result.map_err(|error| Self::failure(&error)).map(|value| {
                    let mut files: Vec<GitFile> = value.get("files").and_then(Value::as_array).map(|list| list.iter().filter_map(|f| Some(GitFile {
                        path: f.get("path")?.as_str()?.to_owned(),
                        code: f.get("code").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
                        added: f.get("added").and_then(Value::as_i64),
                        removed: f.get("removed").and_then(Value::as_i64),
                    })).collect()).unwrap_or_default();
                    files.sort_by_key(|f| std::cmp::Reverse(f.added.unwrap_or(0) + f.removed.unwrap_or(0)));
                    files
                }));
            }
            Reply::Diff(path) => {
                let Some((owner, current, slot)) = self.side.diff.as_mut() else { return; };
                if owner != &key || current != &path { return; }
                *slot = Some(match result {
                    Ok(value) => Ok((value.get("diff").and_then(Value::as_str).unwrap_or("").to_owned(), value.get("truncated").and_then(Value::as_bool).unwrap_or(false))),
                    Err(error) => Err(Self::read_failure(&error)),
                });
            }
            Reply::Shell(label) => {
                let note = match result {
                    Ok(_) => (tr("shortcut_launched").replace("{label}", &label), false),
                    Err(error) if matches!(error.status, Some(404 | 405)) => (tr("shortcut_shell_unsupported"), true),
                    Err(error) => (format!("{label}: {}", Self::failure(&error)), true),
                };
                self.action_feedback.insert(key, note);
            }
            Reply::Reload => {
                self.side.reloading.remove(&key);
                let note = match result { Ok(_) => (tr("reload_sent"), false), Err(error) => (Self::failure(&error), true) };
                self.action_feedback.insert(key, note);
            }
            other => self.receive_control(key, other, result, window, cx),
        }
    }

    fn loop_text(&self) -> Option<String> {
        let state = &self.chat.state;
        let session = self.selected.as_ref()?;
        let status = state.loop_status.clone().or_else(|| session.loop_status.clone())?;
        let iter = state.loop_iter.or(session.loop_iter);
        let max = state.loop_max.or(session.loop_max);
        let count = match (iter, max) { (Some(i), Some(m)) => format!(" {i}/{m}"), (Some(i), None) => format!(" {i}"), _ => String::new() };
        let known = ["running", "paused_awaiting", "done_claimed", "done", "stopped", "exhausted", "failed"];
        let label = if known.contains(&status.as_str()) { tr(&format!("loop_{status}")) } else { status };
        Some(format!("{}{count} · {label}", tr("loop")))
    }

    fn render_context(&mut self, status: Option<&StatusFields>, cx: &mut Context<Self>) -> AnyElement {
        let (provider, _) = self.provider();
        let provider = provider.to_owned();
        // `.sec-agora` do web: o número grande do contexto à esquerda, o custo à direita, a barra embaixo.
        let pct = status.and_then(|s| s.ctx_pct);
        let used = status.and_then(|s| s.ctx_used).map(tokens);
        let total = status.and_then(|s| s.ctx_total).map(tokens);
        let window_text = match (used, total) {
            (Some(u), Some(t)) => tr("side_ctx_of").replace("{used}", &u).replace("{total}", &t),
            (None, Some(t)) => t,
            _ => String::new(),
        };
        let pct_color = match pct { Some(p) if p >= 90. => theme::danger(), Some(p) if p >= 70. => theme::warning(), Some(_) => theme::text(), None => theme::faint() };
        let cost = if provider == "codex" {
            let owned = self.selected_key().and_then(|key| self.side.cost.as_ref().filter(|(owner, ..)| owner == &key));
            match owned {
                None => Some((tr("side_cost_loading"), None)),
                Some((_, Some(c), error)) => {
                    let value = match c.usd { Some(usd) => money(usd), None => "—".into() };
                    let note = if !c.has_usage { Some(tr("side_cost_no_usage")) }
                        else if !c.missing.is_empty() { Some(tr("side_cost_missing").replace("{models}", &c.missing.join(", "))) }
                        else { Some(tr("side_cost_estimate")) };
                    Some((value, error.clone().map(|e| tr("side_cost_stale").replace("{reason}", &e)).or(note)))
                }
                Some((_, None, error)) => Some(("—".into(), error.clone())),
            }
        } else { status.and_then(|s| s.cost_usd).map(|usd| (money(usd), Some(tr("side_cost_session")))) };
        let cost = cost.unwrap_or_else(|| ("—".into(), None));
        let mut line = Vec::new();
        if self.chat.state.state != "working" {
            if let Some(at) = self.selected.as_ref().and_then(|s| s.last_activity) { line.push(tr("side_idle_for").replace("{t}", &ago(now_seconds() - at))); }
        }
        if let (Some(tin), Some(tout)) = (status.and_then(|s| s.turn_in), status.and_then(|s| s.turn_out)) {
            line.push(tr("side_last_turn_line").replace("{in}", &tokens(tin)).replace("{out}", &tokens(tout)));
        }
        if let Some(time) = status.and_then(|s| s.session_time.clone()) { line.push(tr("side_session_time_line").replace("{t}", &time)); }
        let mut body = div().flex().flex_col()
            .child(div().flex().items_end().justify_between().gap_3()
                .child(div().flex().flex_col().flex_shrink_0()
                    .child(div().whitespace_nowrap().text_size(px(44.)).line_height(px(48.)).font_weight(FontWeight::SEMIBOLD).text_color(pct_color)
                        .child(pct.map(|p| format!("{}%", p.round())).unwrap_or_else(|| "—".into())))
                    .child(div().flex().gap_1().text_xs().text_color(theme::faint()).child(tr("side_ctx_label"))
                        .when(!window_text.is_empty(), |el| el.child("·").child(div().font_family(crate::theme::MONO).child(window_text)))))
                .child(div().flex().flex_col().items_end().min_w_0()
                    .child(div().whitespace_nowrap().text_sm().font_weight(FontWeight::SEMIBOLD).child(cost.0))
                    .when_some(cost.1, |el, note| el.child(div().max_w(px(140.)).whitespace_nowrap().truncate().text_right().text_size(px(11.)).text_color(theme::faint()).child(note)))));
        body = match pct {
            Some(p) => body.child(div().mt_3().child(chrome::meter(p))),
            None => body.child(div().mt_3().text_xs().text_color(theme::faint()).child(tr("side_ctx_unknown"))),
        };
        if !line.is_empty() {
            body = body.child(div().mt_2().flex().flex_col().gap(px(2.)).text_size(px(11.)).text_color(theme::faint()).font_family(crate::theme::MONO)
                .children(line.into_iter().map(|l| div().whitespace_nowrap().truncate().child(l))));
        }
        if let Some(limits) = self.render_limits(status) { body = body.child(limits); }
        let _ = cx;
        body.into_any_element()
    }

    /// Aviso do contexto cheio, no molde de `.aviso` do web: borda e fundo de alerta, texto e botão em pílula.
    fn render_ctx_warning(&self, status: Option<&StatusFields>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let provider = self.provider().0;
        let pct = status.and_then(|s| s.ctx_pct).filter(|p| *p >= 60. && matches!(provider, "claude" | "codex"))?;
        let serious = pct >= 85.;
        Some(notice(tr(if serious { "side_ctx_serious" } else { "side_ctx_attention" }), serious)
            .child(div().flex().child(notice_button("side-compact", tr("side_compact"), serious, cx)
                .on_click(cx.listener(|this, _, window, cx| this.fill_command("compact", true, window, cx)))))
            .into_any_element())
    }

    fn render_limits(&self, status: Option<&StatusFields>) -> Option<AnyElement> {
        let limited = self.chat.state.limited.or(self.selected.as_ref().and_then(|s| s.limited)) == Some(true);
        let reset = self.chat.state.limit_reset.clone().or_else(|| self.selected.as_ref().and_then(|s| s.limit_reset.clone()));
        let windows: Vec<(String, f64, Option<String>)> = status.map(|s| [
            (tr("limit_5h"), s.five_hour_pct, s.five_hour_reset.clone()),
            (tr("limit_7d"), s.weekly_pct, s.weekly_reset.clone()),
            (tr("limit_30d"), s.monthly_pct, s.monthly_reset.clone()),
        ].into_iter().filter_map(|(label, pct, reset)| Some((label, pct?, reset))).collect()).unwrap_or_default();
        if !limited && windows.is_empty() { return None; }
        // RateChips em barras: legenda e número em cima, barra, "reseta" embaixo; duas colunas.
        Some(div().mt_4().flex().flex_col().gap_2()
            .when(limited, |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::limited())
                .child(reset.map(|r| tr("side_limited_until").replace("{reset}", &r)).unwrap_or_else(|| tr("side_limited")))))
            .child(div().flex().flex_wrap().gap_3().children(windows.into_iter().map(|(label, pct, reset)| {
                div().flex_1().min_w(px(118.)).flex().flex_col().gap_1()
                    .child(div().flex().justify_between().text_xs()
                        .child(div().font_family(crate::theme::MONO).text_color(theme::faint()).child(label))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{}%", pct.round()))))
                    .child(chrome::meter(pct))
                    .when_some(reset, |el, r| el.child(div().text_size(px(11.)).text_color(theme::faint()).truncate().child(tr("side_resets").replace("{reset}", &r))))
            })))
            .into_any_element())
    }

    fn render_project(&mut self, status: Option<&StatusFields>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.selected.clone()?;
        let key = self.selected_key()?;
        let repo = status.and_then(|s| s.repo.clone());
        if repo.is_none() && session.git_dirty.is_none() && session.git_added.is_none() { return None; }
        let mut body = div().flex().flex_col().gap_2().child(chrome::section_label(tr("side_project")));
        if let Some(repo) = repo {
            let branch = status.and_then(|s| s.branch.clone()).unwrap_or_default();
            let dirty = status.and_then(|s| s.dirty) == Some(true);
            body = body.child(div().flex().items_center().gap_1().min_w_0().text_xs().font_family(crate::theme::MONO).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted())
                .child(div().min_w_0().truncate().child(format!("{repo} · {branch}")))
                .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*"))));
        }
        let changes = match (session.git_added, session.git_removed) {
            (Some(a), Some(r)) if a + r > 0 => div().flex().gap_2().text_xs()
                .child(div().font_family(crate::theme::MONO).text_color(theme::success()).child(format!("+{a}")))
                .child(div().font_family(crate::theme::MONO).text_color(theme::danger()).child(format!("−{r}")))
                .child(div().text_color(theme::faint()).child(tr("side_changes_tree"))).into_any_element(),
            _ if session.git_dirty.is_some_and(|n| n > 0) => div().text_xs().text_color(theme::faint()).child(tr("side_changes_local")).into_any_element(),
            _ => div().text_xs().text_color(theme::faint()).child(if session.git_dirty.is_some() { tr("side_changes_none") } else { String::new() }).into_any_element(),
        };
        body = body.child(changes);
        let open = self.side.files.as_ref().is_some_and(|(owner, _)| owner == &key);
        body = body.child(div().flex().gap_1()
            .child(Button::new("side-files").xsmall().ghost().selected(open).label(tr(if open { "side_files_reload" } else { "side_files" }))
                .on_click(cx.listener(|this, _, _, cx| this.load_files(cx))))
            .when(open, |el| el.child(Button::new("side-files-close").xsmall().ghost().label(tr("close"))
                .on_click(cx.listener(|this, _, _, cx| { this.side.files = None; this.side.diff = None; cx.notify(); })))));
        if open {
            let listing = match self.side.files.as_ref().and_then(|(_, slot)| slot.as_ref()) {
                None => div().text_xs().text_color(theme::muted()).child(tr("side_files_loading")).into_any_element(),
                Some(Err(reason)) => div().text_xs().text_color(theme::warning()).child(reason.clone()).into_any_element(),
                Some(Ok(files)) if files.is_empty() => div().text_xs().text_color(theme::muted()).child(tr("side_files_empty")).into_any_element(),
                Some(Ok(files)) => {
                    let current = self.side.diff.as_ref().map(|(_, path, _)| path.clone());
                    div().id("side-files-list").max_h(px(220.)).overflow_y_scroll().flex().flex_col()
                        .children(files.iter().enumerate().map(|(n, file)| {
                            let path = file.path.clone();
                            let counts = match (file.added, file.removed) { (Some(a), Some(r)) => format!("+{a} −{r}"), _ => file.code.clone() };
                            Button::new(SharedString::from(format!("side-file-{n}"))).ghost().xsmall().w_full().flex_shrink_0().selected(current.as_deref() == Some(file.path.as_str()))
                                .child(div().flex_1().min_w_0().truncate().font_family(crate::theme::MONO).child(file.path.clone()))
                                .child(div().flex_shrink_0().text_color(theme::muted()).child(counts))
                                .on_click(cx.listener(move |this, _, _, cx| this.open_diff(path.clone(), cx)))
                        })).into_any_element()
                }
            };
            body = body.child(listing);
        }
        if let Some((owner, path, slot)) = self.side.diff.clone().filter(|(owner, ..)| owner == &key) {
            let _ = owner;
            let content = match slot {
                None => div().text_xs().text_color(theme::muted()).child(tr("side_diff_loading")).into_any_element(),
                Some(Err(reason)) => div().text_xs().text_color(theme::warning()).child(reason).into_any_element(),
                Some(Ok((diff, truncated))) if diff.trim().is_empty() => div().text_xs().text_color(theme::muted())
                    .child(tr(if truncated { "side_diff_truncated" } else { "side_diff_empty" })).into_any_element(),
                Some(Ok((diff, truncated))) => {
                    let (shown, clipped) = conversation::clip(&diff, DIFF_MAX);
                    let view = self.text_view(&format!("side-diff:{path}"), "__side__", conversation::fenced(shown), cx);
                    div().flex().flex_col().gap_1()
                        .child(div().id("side-diff").max_h(px(360.)).overflow_y_scroll().text_xs().child(TextView::new(&view).selectable(true).scrollable(false)))
                        .when(truncated || clipped, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("side_diff_truncated"))))
                        .into_any_element()
                }
            };
            body = body.child(div().flex().flex_col().gap_1().p_2().rounded(px(12.)).bg(theme::inset()).border_1().border_color(theme::border())
                .child(div().text_xs().font_family(crate::theme::MONO).truncate().child(path)).child(content));
        }
        Some(body.into_any_element())
    }

    fn render_shortcuts(&self, readable: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = match self.side.shortcuts.as_ref()? {
            Ok(list) if list.is_empty() => return None,
            Ok(list) => list.clone(),
            Err(reason) => return Some(div().mx_4().my_3().child(div().text_xs().text_color(theme::warning())
                .child(tr("side_shortcuts_failed").replace("{reason}", reason))).into_any_element()),
        };
        let busy = self.selected_key().is_some_and(|key| self.uploading.contains_key(&key));
        // `.ctx-actions` do web: faixa embutida, cada atalho com ícone em cima e rótulo embaixo.
        Some(div().flex_shrink_0().mx_4().my_3().p(px(2.)).flex().flex_wrap().gap(px(2.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::inset())
            .children(list.into_iter().enumerate().map(|(n, shortcut)| {
                let icon = match &shortcut { Shortcut::Attach => IconName::Paperclip, Shortcut::Shell { .. } => IconName::SquareTerminal, Shortcut::Send { .. } => IconName::SquareSlash };
                let label = shortcut.label();
                Button::new(SharedString::from(format!("shortcut-{n}")))
                    .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted()).hover(theme::raised()).active(theme::raised()))
                    .flex_1().min_w(px(44.)).min_h(px(50.)).rounded(px(9.)).pt(px(6.)).pb(px(5.)).px(px(2.))
                    .tooltip(label.clone()).accessibility_label(label.clone()).disabled(!readable || busy)
                    .child(div().flex().flex_col().items_center().gap(px(3.))
                        .child(chrome::small_icon(icon, 18., theme::muted()))
                        .child(div().max_w(px(96.)).truncate().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).child(label)))
                    .on_click(cx.listener(move |this, _, window, cx| this.run_shortcut(shortcut.clone(), false, window, cx)))
            })).into_any_element())
    }

    pub(super) fn render_side(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewport = f32::from(window.viewport_size().width);
        let width = self.side.fitted(viewport).filter(|_| self.side.open && self.selected.is_some());
        let readable = self.selected.as_ref().is_some_and(|s| s.readable());
        self.sync_cost(width.is_some() && readable);
        let width = width?;
        let session = self.selected.clone()?;
        let status = self.status();
        let state = if self.chat_online && !self.chat.state.state.is_empty() { self.chat.state.state.clone() } else { session.state.clone() };
        let detail = self.chat.state.label.clone().or(session.label.clone()).filter(|l| !l.trim().is_empty());
        // Cabeçalho do painel do web: nome em mono, detalhe do estado embaixo, selo de estado à direita.
        let header = div().flex_shrink_0().relative().min_h(px(64.)).pl_4().pr(px(56.)).py_2().flex().items_center().justify_between().gap_3()
            .border_b_1().border_color(theme::border()).bg(theme::header_band())
            .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                .child(div().truncate().font_family(crate::theme::MONO).text_sm().font_weight(FontWeight::SEMIBOLD).child(session.name.clone()))
                .when_some(detail, |el, d| el.child(div().truncate().text_xs().text_color(theme::faint()).child(d)))
                .when_some(self.loop_text(), |el, text| el.child(div().truncate().text_xs().text_color(theme::accent()).child(text))))
            .child(chrome::state_chip(&state, tr(&format!("chip_{state}")), true))
            .child(div().absolute().top_2().right_2().child(chrome::icon_button("side-toggle", IconName::PanelRightClose, tr("side_hide"), cx)
                .size(px(36.)).on_click(cx.listener(|this, _, _, cx| this.toggle_side(cx)))));
        let mut content = div().flex().flex_col();
        if readable {
            let motive = self.chat.state.recarregar_motivo.clone().filter(|_| self.provider().1 && self.provider().0 == "claude");
            let mut notices = Vec::new();
            if let Some(motive) = motive {
                let reloading = self.selected_key().is_some_and(|key| self.side.reloading.contains(&key));
                notices.push(notice(tr("reload_hint").replace("{reason}", &tr(if motive == "config" { "reload_reason_config" } else { "reload_reason_other" })), false)
                    .child(div().text_size(px(11.)).text_color(theme::faint()).child(tr(if reloading { "reload_running" } else if self.reload_allowed() { "reload_ready" } else { "reload_wait" })))
                    .child(div().flex().child(notice_button("side-reload", tr("reload"), false, cx).disabled(!self.reload_allowed())
                        .on_click(cx.listener(|this, _, _, cx| { this.confirm = Some(Confirm::Reload); cx.notify(); }))))
                    .into_any_element());
            }
            notices.extend(self.render_ctx_warning(status.as_ref(), cx));
            content = content.child(div().mx_4().pt_4().pb_3().child(self.render_context(status.as_ref(), cx)))
                .when(!notices.is_empty(), |el| el.child(div().mx_4().mb_2().flex().flex_col().gap_2().children(notices)));
            if let Some(project) = self.render_project(status.as_ref(), cx) {
                content = content.child(div().mx_4().pt_4().pb_3().border_t_1().border_color(theme::border()).child(project));
            }
        }
        let actions = if readable { self.render_shortcuts(readable, cx) } else { None };
        let queued = if readable { self.queued_count() } else { 0 };
        let handle = div().id("side-resize").absolute().left_0().top_0().bottom_0().w(px(6.)).cursor_col_resize()
            .hover(|el| el.bg(theme::accent_dim()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.side.drag = Some((f32::from(event.position.x), width));
                cx.stop_propagation();
                cx.notify();
            }));
        let server = self.address.read(cx).value().trim_start_matches("http://").trim_start_matches("https://").to_string();
        Some(div().w(px(width)).h_full().flex_shrink_0().relative().p_3()
            .child(div().size_full().flex().flex_col().rounded(px(24.)).border_1().border_color(theme::border()).bg(theme::chrome())
                .shadow(theme::panel_shadow()).overflow_hidden()
                .child(header)
                .when_some(actions, |el, actions| el.child(actions))
                .child(div().id("side-scroll").flex_1().min_h_0().overflow_y_scroll().child(content))
                .child(div().flex_shrink_0().px_4().py_3().flex().items_center().justify_between().gap_2().border_t_1().border_color(theme::border()).text_size(px(11.))
                    .child(div().min_w_0().truncate().text_color(theme::faint()).child(format!("{} · {server}", agent_label(&session.provider))))
                    .when(queued > 0, |el| el.child(div().flex_shrink_0().text_color(theme::muted()).child(tr("side_queued").replace("{n}", &queued.to_string()))))))
            .child(handle)
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{Shortcut, Side, duration, parse_shortcuts, tokens};

    #[test]
    fn shortcuts_fall_back_and_drop_bad_items() {
        assert_eq!(parse_shortcuts(""), vec![Shortcut::Attach]);
        assert_eq!(parse_shortcuts("{quebrado"), vec![Shortcut::Attach]);
        let raw = r#"[{"id":"a","type":"send_text","label":"Relatório","text":"/relatorio","send_direct":false,"confirm":true},
            {"id":"a","type":"shell","label":"dup","command":"x"},{"id":"b","type":"shell","label":"Build","command":"make"},
            {"id":"c","type":"send_text","label":"","text":"x"},{"id":"t","type":"internal","action":"terminal"}]"#;
        assert_eq!(parse_shortcuts(raw), vec![
            Shortcut::Send { label: "Relatório".into(), text: "/relatorio".into(), direct: false, confirm: true },
            Shortcut::Shell { label: "Build".into(), command: "make".into(), confirm: false },
        ]);
    }

    #[test]
    fn token_and_duration_formats() {
        assert_eq!((tokens(590.), tokens(40_400.), tokens(1_000_000.), tokens(1_250_000.)), ("590".into(), "40k".into(), "1M".into(), "1.3M".into()));
        assert_eq!((duration(1_500.), duration(42_000.), duration(125_000.)), ("1.5s".into(), "42s".into(), "2m05s".into()));
    }

    #[test]
    fn panel_never_squeezes_the_chat() {
        let side = Side::default();
        assert_eq!(side.fitted(1180.), Some(320.));
        assert_eq!(side.fitted(1000.), None);
        assert_eq!(side.fitted(1060.), Some(250.));
    }
}
