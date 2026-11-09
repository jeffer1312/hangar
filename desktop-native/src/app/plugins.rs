//! Página Plugins do servidor conectado: os plugins do Claude Code da máquina, o que dá para instalar e os marketplaces.
//! Entidade própria, como o Cofre: as respostas voltam a ela, e ela some ao sair das Configurações.
use super::*;
use super::settings::{Disclosure, Page, settings_box};
use super::side::{notice, notice_button};
use super::accounts::fold;
use super::harness::grouped_number;
use super::vault::{alert, chip, muted};
use gpui_kit::component::switch::Switch;

// A CLI baixa repositórios nas mudanças longas; o servidor tem o teto dele um pouco abaixo destes.
const READ_SECONDS: u64 = 30;
const AVAILABLE_SECONDS: u64 = 70;
const SHORT_SECONDS: u64 = 50;
const LONG_SECONDS: u64 = 240;
const MARKETPLACE_REMOVE_SECONDS: u64 = 70;

/// Só estes 409 do `/recarregar` são "sessão ocupada"; os outros (conta que não reconciliou) têm motivo próprio.
const SESSION_BUSY_CODES: [&str; 5] =
    ["erro_sessao_iniciando", "erro_sessao_esperando_resposta", "erro_sessao_trabalhando", "erro_fila_pendente", "session_transfer_busy"];

fn session_busy(error: &Failure) -> bool {
    error.status == Some(409) && error.code.as_deref().is_some_and(|code| SESSION_BUSY_CODES.contains(&code))
}

const TAB_INSTALLED: usize = 0;
const TAB_DISCOVER: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter { All, Enabled, Disabled, Updates }

impl Filter {
    const ALL: [Filter; 4] = [Filter::All, Filter::Enabled, Filter::Disabled, Filter::Updates];

    fn keeps(self, plugin: &InstalledPlugin) -> bool {
        match self {
            Filter::All => true,
            Filter::Enabled => plugin.enabled,
            Filter::Disabled => !plugin.enabled,
            Filter::Updates => has_update(plugin),
        }
    }
}

/// Só a versão nova conhecida pelo disco vira pílula; a fonte externa só descobre ao atualizar.
fn has_update(plugin: &InstalledPlugin) -> bool { plugin.update_known && plugin.available_version.is_some() }

/// `query` já passou pelo `fold`: dobra uma vez por desenho, não por item.
fn found(name: &str, description: Option<&str>, query: &str) -> bool {
    query.is_empty() || fold(name).contains(query) || description.is_some_and(|d| fold(d).contains(query))
}

fn shown<'a>(plugins: &'a [InstalledPlugin], filter: Filter, query: &str) -> Vec<&'a InstalledPlugin> {
    let query = fold(query.trim());
    plugins.iter().filter(|p| filter.keeps(p) && found(&p.name, p.description.as_deref(), &query)).collect()
}

/// O mesmo formato que o servidor aceita: `dono/repo`, URL `https://` ou caminho absoluto.
fn source_ok(source: &str) -> bool {
    let part = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let windows_path = source.len() > 2 && source.as_bytes()[0].is_ascii_alphabetic() && source[1..].starts_with(":\\");
    !source.starts_with('-') && (source.starts_with("https://") || source.starts_with('/') || windows_path
        || source.split_once('/').is_some_and(|(owner, repo)| part(owner) && part(repo)))
}

/// Sessões que guardam o estado antigo dos plugins: Claude vivas (sem provedor na linha é Claude), nem mortas nem adormecidas.
fn live_claude(sessions: &[SessionInfo]) -> Vec<String> {
    sessions.iter().filter(|s| (s.provider.is_empty() || s.provider == "claude") && s.state != "dead" && !s.asleep)
        .map(|s| s.name.clone()).collect()
}

fn in_zone<Tz: chrono::TimeZone>(iso: &str, zone: &Tz) -> Option<chrono::DateTime<Tz>> {
    chrono::DateTime::parse_from_rfc3339(iso).ok().map(|t| t.with_timezone(zone))
}

fn date_text<Tz: chrono::TimeZone>(at: &chrono::DateTime<Tz>) -> String where Tz::Offset: std::fmt::Display {
    at.format(if crate::i18n::english() { "%m/%d/%Y" } else { "%d/%m/%Y" }).to_string()
}

/// Data no fuso de quem olha; o que não for ISO aparece como veio, cortado no dia.
fn day(iso: &str) -> String {
    in_zone(iso, &chrono::Local).map(|t| date_text(&t)).unwrap_or_else(|| iso.chars().take(10).collect())
}

/// `now` traz o fuso: "hoje" é o dia de quem olha, não o do servidor.
fn checked_label<Tz: chrono::TimeZone>(checked_at: Option<&str>, now: &chrono::DateTime<Tz>) -> String where Tz::Offset: std::fmt::Display {
    match checked_at.and_then(|iso| in_zone(iso, &now.timezone())) {
        None => tr("plugins_never_checked"),
        Some(at) if at.date_naive() == now.date_naive() => tr("plugins_checked_today").replace("{time}", &at.format("%H:%M").to_string()),
        Some(at) => tr("plugins_checked_on").replace("{date}", &date_text(&at)),
    }
}

fn inventory_labels(inventory: &PluginInventory) -> Vec<String> {
    [(inventory.skills, "plugins_inventory_skills", Some("plugins_inventory_skills_one")),
        (inventory.agents, "plugins_inventory_agents", Some("plugins_inventory_agents_one")),
        (inventory.hooks, "plugins_inventory_hooks", Some("plugins_inventory_hooks_one")),
        (inventory.mcp, "plugins_inventory_mcp", None), (inventory.lsp, "plugins_inventory_lsp", None)]
        .into_iter().filter_map(|(n, key, one)| {
            let n = n.filter(|n| *n > 0)?;
            Some(match one { Some(one) if n == 1 => tr(one), _ => tr(key).replace("{n}", &n.to_string()) })
        }).collect()
}

fn row_box(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).role(Role::ListItem).mt(px(-1.)).border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().items_center().gap(px(14.))
}

fn text_block(name: Div, meta: String, description: Option<String>) -> Div {
    div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
        .child(name)
        .child(div().truncate().text_size(px(12.5)).text_color(theme::muted()).child(meta))
        .when_some(description.filter(|d| !d.is_empty()), |el, d| el.child(div().text_size(px(12.5)).text_color(theme::faint()).whitespace_normal().child(d)))
}

/// Faixa depois de uma mudança: quem ainda está no estado anterior e o que deu errado ao reabrir.
struct Reopen {
    action: String,
    names: Vec<String>,
    busy: bool,
    failures: Vec<String>,
}

pub(super) struct PluginsPage {
    api: Api,
    runtime: Arc<Runtime>,
    hangar: WeakEntity<Hangar>,
    /// Número da leitura: resposta de uma leitura anterior não pisa na nova.
    seq: u64,
    tab: usize,
    filter: Filter,
    state: Option<Result<PluginsState, String>>,
    available: Option<Result<Vec<AvailablePlugin>, String>>,
    /// Pedido na primeira abertura do detalhe; uma mudança limpa, porque a versão pode ter trocado.
    details: HashMap<String, Result<PluginDetails, String>>,
    pending_details: HashSet<String>,
    /// Sobe a cada mudança: detalhe pedido antes dela chega velho e é descartado.
    details_generation: u64,
    open: Option<String>,
    /// Plugins (`id`) e marketplaces (`mkt:<nome>`) com mudança em andamento.
    busy: HashSet<String>,
    installed_now: HashSet<String>,
    checking: bool,
    error: Option<String>,
    source_error: Option<String>,
    reopen: Option<Reopen>,
    query: Entity<InputState>,
    discover_query: Entity<InputState>,
    source: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl PluginsPage {
    fn new(api: Api, runtime: Arc<Runtime>, hangar: WeakEntity<Hangar>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |hint: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        let query = field(tr("plugins_filter_placeholder"), window, cx);
        let discover_query = field(tr("plugins_discover_placeholder").replace("{n}", "…"), window, cx);
        let source = field(tr("plugins_marketplace_placeholder"), window, cx);
        let redraw = |_: &mut Self, _: &Entity<InputState>, event: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
            if matches!(event, InputEvent::Change) { cx.notify(); }
        };
        let _subscriptions = vec![
            cx.subscribe_in(&query, window, redraw),
            cx.subscribe_in(&discover_query, window, redraw),
            cx.subscribe_in(&source, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_marketplace(window, cx),
                InputEvent::Change => { this.source_error = None; cx.notify(); }
                _ => {}
            }),
        ];
        let mut page = Self { api, runtime, hangar, seq: 0, tab: TAB_INSTALLED, filter: Filter::All, state: None, available: None,
            details: HashMap::new(), pending_details: HashSet::new(), details_generation: 0, open: None, busy: HashSet::new(), installed_now: HashSet::new(), checking: false, error: None,
            source_error: None, reopen: None, query, discover_query, source, _subscriptions };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.seq += 1;
        let seq = self.seq;
        (self.state, self.error) = (None, None);
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_read(&["plugins"], &[], READ_SECONDS).await }, window, cx, move |this: &mut Self, result, _, _| {
            if seq != this.seq { return; }
            this.state = Some(result.map_err(|e| Hangar::fetch_failure(&e))
                .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response"))));
        });
        cx.notify();
    }

    fn load_available(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.available = None;
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_read(&["plugins", "available"], &[], AVAILABLE_SECONDS).await }, window, cx,
            |this: &mut Self, result, window, cx| {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| v.get("plugins").cloned()
                    .and_then(|p| serde_json::from_value::<Vec<AvailablePlugin>>(p).ok()).ok_or_else(|| tr("invalid_response")));
                if let Ok(list) = &list {
                    let hint = tr("plugins_discover_placeholder").replace("{n}", &grouped_number(list.len() as u64));
                    this.discover_query.update(cx, |input, cx| input.set_placeholder(hint, window, cx));
                }
                this.available = Some(list);
            });
        cx.notify();
    }

    fn select_tab(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == TAB_DISCOVER && self.available.as_ref().is_none_or(|a| a.is_err()) { self.load_available(window, cx); }
        cx.notify();
    }

    fn toggle_detail(&mut self, id: String, cx: &mut Context<Self>) {
        self.open = if self.open.as_ref() == Some(&id) { None } else { Some(id) };
        cx.notify();
    }

    /// Detalhe aberto e ainda não pedido: pede uma vez; a resposta fica até a próxima mudança.
    fn request_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.open.clone().filter(|id| !self.details.contains_key(id) && !self.pending_details.contains(id)) else { return };
        self.pending_details.insert(id.clone());
        let (api, path, generation) = (self.api.clone(), id.clone(), self.details_generation);
        run(&self.runtime, async move { api.server_read(&["plugins", &path, "details"], &[], READ_SECONDS).await }, window, cx,
            move |this: &mut Self, result, _, _| {
                this.pending_details.remove(&id);
                if generation != this.details_generation { return; }
                this.details.insert(id, result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response"))));
            });
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking { return; }
        (self.checking, self.error) = (true, None);
        let api = self.api.clone();
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["plugins", "check"], None, LONG_SECONDS).await }, window, cx,
            |this: &mut Self, result, _, _| {
                this.checking = false;
                match result.map(serde_json::from_value::<PluginsState>) {
                    Ok(Ok(state)) => this.state = Some(Ok(state)),
                    Ok(Err(_)) => this.error = Some(tr("invalid_response")),
                    Err(error) => this.error = Some(Hangar::fetch_failure(&error)),
                }
            });
        cx.notify();
    }

    /// Toda mudança devolve o estado relido do disco; `done` recebe a resposta inteira (com o `result`).
    #[allow(clippy::too_many_arguments)]
    fn change(&mut self, busy: String, method: reqwest::Method, path: Vec<String>, body: Option<Value>, seconds: u64,
        done: impl FnOnce(&mut Self, &Value, &mut Window, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert(busy.clone()) { return; }
        self.error = None;
        let api = self.api.clone();
        run(&self.runtime, async move {
            let parts: Vec<&str> = path.iter().map(String::as_str).collect();
            api.server_send(method, &parts, body, seconds).await
        }, window, cx, move |this: &mut Self, result, window, cx| {
            this.busy.remove(&busy);
            match result {
                Ok(value) => match serde_json::from_value::<PluginsState>(value.clone()) {
                    Ok(state) => {
                        this.state = Some(Ok(state));
                        this.details.clear();
                        this.details_generation += 1;
                        done(this, &value, window, cx);
                    }
                    Err(_) => this.error = Some(tr("invalid_response")),
                },
                Err(error) => this.error = Some(Hangar::fetch_failure(&error)),
            }
        });
        cx.notify();
    }

    /// A faixa só aparece com alguém para reabrir.
    fn changed(&mut self, action: String, cx: &mut Context<Self>) {
        let names = self.hangar.upgrade().map(|h| live_claude(&h.read(cx).sessions)).unwrap_or_default();
        self.reopen = (!names.is_empty()).then(|| Reopen { action, names, busy: false, failures: Vec::new() });
    }

    fn plugin_path(id: &str, action: Option<&str>) -> Vec<String> {
        ["plugins", id].into_iter().chain(action).map(str::to_owned).collect()
    }

    /// As linhas guardam só o `id`; o plugin sai do estado na hora do clique.
    fn installed(&self, id: &str) -> Option<InstalledPlugin> {
        self.state.as_ref()?.as_ref().ok()?.plugins.iter().find(|p| p.id == id).cloned()
    }

    fn set_enabled(&mut self, plugin: &InstalledPlugin, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let name = plugin.name.clone();
        let path = Self::plugin_path(&plugin.id, Some(if on { "enable" } else { "disable" }));
        self.change(plugin.id.clone(), reqwest::Method::POST, path, None, SHORT_SECONDS, move |this, _, _, cx| {
            this.changed(tr(if on { "plugins_action_enabled" } else { "plugins_action_disabled" }).replace("{name}", &name), cx);
        }, window, cx);
    }

    fn update(&mut self, plugin: &InstalledPlugin, window: &mut Window, cx: &mut Context<Self>) {
        let (name, known, fallback) = (plugin.name.clone(), plugin.update_known, plugin.available_version.clone());
        let path = Self::plugin_path(&plugin.id, Some("update"));
        self.change(plugin.id.clone(), reqwest::Method::POST, path, None, LONG_SECONDS, move |this, value, window, cx| {
            let result = value.get("result").cloned().and_then(|r| serde_json::from_value::<PluginOperation>(r).ok());
            if result.as_ref().is_some_and(|r| r.outcome == "up_to_date") {
                window.push_notification(Notification::info(tr("plugins_update_up_to_date").replace("{name}", &name)), cx);
                return;
            }
            let version = result.and_then(|r| r.new_version).or(fallback).unwrap_or_default();
            // Na fonte externa o resultado só existe aqui: a tela não tinha pílula para mostrar.
            if !known { window.push_notification(Notification::info(tr("plugins_update_done").replace("{name}", &name).replace("{version}", &version)), cx); }
            this.changed(tr("plugins_action_updated").replace("{name}", &name).replace("{version}", &version), cx);
        }, window, cx);
    }

    fn confirm_remove(&mut self, plugin: &InstalledPlugin, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let (id, name) = (plugin.id.clone(), plugin.name.clone());
        let title = tr("plugins_remove_confirm").replace("{name}", &name);
        chrome::confirm_alert(window, cx, title, String::new(), tr("plugins_remove"), ButtonVariant::Danger, move |window, cx| {
            let (id, name) = (id.clone(), name.clone());
            let _ = page.update(cx, |page, cx| {
                if page.open.as_ref() == Some(&id) { page.open = None; }
                page.change(id.clone(), reqwest::Method::DELETE, Self::plugin_path(&id, None), None, SHORT_SECONDS, move |this, _, _, cx| {
                    this.changed(tr("plugins_action_removed").replace("{name}", &name), cx);
                }, window, cx);
            });
            true
        });
    }

    fn install(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.change(id.clone(), reqwest::Method::POST, Self::plugin_path(&id, Some("install")), None, LONG_SECONDS, move |this, _, _, cx| {
            this.installed_now.insert(id);
            this.changed(tr("plugins_action_installed").replace("{name}", &name), cx);
        }, window, cx);
    }

    fn add_marketplace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.source.read(cx).value().trim().to_owned();
        if !source_ok(&source) { self.source_error = Some(tr("plugins_marketplace_format_error")); cx.notify(); return; }
        self.source_error = None;
        let path = vec!["plugins".to_owned(), "marketplaces".to_owned()];
        // O `result` não traz o nome: ele sai da lista nova contra a de antes.
        let before: HashSet<String> = self.state.as_ref().and_then(|s| s.as_ref().ok())
            .map(|s| s.marketplaces.iter().map(|m| m.name.clone()).collect()).unwrap_or_default();
        self.change("mkt:+".into(), reqwest::Method::POST, path, Some(json!({"source": source.clone()})), LONG_SECONDS, move |this, _, window, cx| {
            this.source.update(cx, |input, cx| input.set_value("", window, cx));
            let name = this.state.as_ref().and_then(|s| s.as_ref().ok())
                .and_then(|s| s.marketplaces.iter().find(|m| !before.contains(&m.name)).map(|m| m.name.clone())).unwrap_or(source);
            this.changed(tr("plugins_action_marketplace_added").replace("{name}", &name), cx);
        }, window, cx);
    }

    fn marketplace_path(name: &str, action: Option<&str>) -> Vec<String> {
        ["plugins", "marketplaces", name].into_iter().chain(action).map(str::to_owned).collect()
    }

    fn update_marketplace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = Self::marketplace_path(&name, Some("update"));
        self.change(format!("mkt:{name}"), reqwest::Method::POST, path, None, LONG_SECONDS, move |this, _, _, cx| {
            this.changed(tr("plugins_action_marketplace_updated").replace("{name}", &name), cx);
        }, window, cx);
    }

    fn set_auto_update(&mut self, name: String, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = Self::marketplace_path(&name, Some("auto-update"));
        // Atualizar sozinho não muda nada carregado nas sessões: sem faixa.
        self.change(format!("mkt:{name}"), reqwest::Method::PUT, path, Some(json!({"enabled": on})), SHORT_SECONDS, |_, _, _, _| {}, window, cx);
    }

    fn confirm_remove_marketplace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let title = tr("plugins_marketplace_remove_confirm").replace("{name}", &name);
        chrome::confirm_alert(window, cx, title, String::new(), tr("plugins_remove"), ButtonVariant::Danger, move |window, cx| {
            let name = name.clone();
            let _ = page.update(cx, |page, cx| {
                let path = Self::marketplace_path(&name, None);
                page.change(format!("mkt:{name}"), reqwest::Method::DELETE, path, None, MARKETPLACE_REMOVE_SECONDS, move |this, _, _, cx| {
                    this.changed(tr("plugins_action_marketplace_removed").replace("{name}", &name), cx);
                }, window, cx);
            });
            true
        });
    }

    /// Uma sessão por vez: duas reabrindo juntas disputariam a mesma conta.
    fn reopen_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reopen) = self.reopen.as_mut().filter(|r| !r.busy) else { return };
        reopen.busy = true;
        let (api, names) = (self.api.clone(), reopen.names.clone());
        run(&self.runtime, async move {
            let mut out = Vec::new();
            for name in names {
                let result = api.act(&name, &["recarregar"], None, false, crate::api::REOPEN_SECONDS).await;
                out.push((name, result));
            }
            Ok(out)
        }, window, cx, |this: &mut Self, result: Result<Vec<(String, Result<Value, Failure>)>, Failure>, window, cx| {
            let Some(reopen) = this.reopen.as_mut().filter(|r| r.busy) else { return };
            reopen.busy = false;
            let (mut names, mut failures, mut warnings) = (Vec::new(), Vec::new(), Vec::new());
            for (name, result) in result.unwrap_or_default() {
                match result {
                    Ok(value) => warnings.extend(controls::reopen_warnings(&value).map(|w| format!("{name}: {w}"))),
                    Err(error) => {
                        failures.push(if session_busy(&error) { tr("plugins_reopen_refused").replace("{name}", &name) }
                            else { format!("{name}: {}", Hangar::fetch_failure(&error)) });
                        names.push(name);
                    }
                }
            }
            if !warnings.is_empty() { window.push_notification(Notification::warning(warnings.join("\n")), cx); }
            if names.is_empty() { this.reopen = None; } else { (reopen.names, reopen.failures) = (names, failures); }
        });
        cx.notify();
    }

    fn retry(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id).outline().small().label(tr("plugins_retry")).on_click(cx.listener(|this, _, window, cx| this.load(window, cx)))
    }

    fn render_reopen(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reopen = self.reopen.as_ref()?;
        let n = reopen.names.len();
        let text = if n == 1 { tr("plugins_reopen_banner_one").replace("{action}", &reopen.action) }
            else { tr("plugins_reopen_banner").replace("{action}", &reopen.action).replace("{n}", &n.to_string()) };
        let label = if reopen.busy { tr("plugins_reopening") } else if n == 1 { tr("plugins_reopen") } else { tr("plugins_reopen_all").replace("{n}", &n.to_string()) };
        Some(div().id("plugins-reopen").role(Role::Status).child(notice(text, false)
            .children(reopen.failures.iter().map(|f| div().pl(px(23.)).text_size(px(11.)).text_color(theme::warning()).whitespace_normal().child(f.clone())))
            .child(div().pl(px(23.)).flex().gap_2()
                .child(notice_button("plugins-reopen-go", label, false, cx).disabled(reopen.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.reopen_all(window, cx))))
                .child(notice_button("plugins-reopen-later", tr("plugins_reopen_later"), false, cx).disabled(reopen.busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.reopen = None; cx.notify(); })))))
            .into_any_element())
    }

    fn render_installed(&self, state: &PluginsState, cx: &mut Context<Self>) -> AnyElement {
        let updates = state.plugins.iter().filter(|p| has_update(p)).count();
        let labels = [tr("plugins_filter_all"), tr("plugins_filter_enabled"), tr("plugins_filter_disabled"),
            tr("plugins_filter_updates").replace("{n}", &updates.to_string())];
        let selected = Filter::ALL.iter().position(|f| *f == self.filter).unwrap_or(0);
        let segments = TabBar::new("plugins-filter").segmented().small().selected_index(selected)
            .children(labels.into_iter().map(|l| Tab::new().label(l)))
            .on_click(cx.listener(|this, ix: &usize, _, cx| { this.filter = Filter::ALL[*ix]; cx.notify(); }));
        let bar = div().flex().flex_wrap().items_center().gap_2()
            .child(div().flex_1().min_w(px(180.)).child(Input::new(&self.query).small().cleanable(true).aria_label(tr("plugins_filter_placeholder"))))
            .child(segments);
        let query = self.query.read(cx).value().to_string();
        let list = shown(&state.plugins, self.filter, &query);
        let body = if state.plugins.is_empty() {
            settings_box().px_4().py(px(18.)).child(muted(tr("plugins_empty_installed"))).into_any_element()
        } else if list.is_empty() {
            let key = if self.filter == Filter::Updates && query.trim().is_empty() { "plugins_all_latest" } else { "plugins_empty_filter" };
            settings_box().px_4().py(px(18.)).child(muted(tr(key))).into_any_element()
        } else {
            settings_box().id("plugins-installed").role(Role::List).aria_label(tr("plugins_tab_installed"))
                .children(list.into_iter().flat_map(|p| {
                    let open = self.open.as_ref() == Some(&p.id);
                    std::iter::once(self.render_plugin(p, open, cx)).chain(open.then(|| self.render_detail(p, cx)))
                })).into_any_element()
        };
        div().flex().flex_col().gap_3().child(bar).child(body).into_any_element()
    }

    fn render_plugin(&self, plugin: &InstalledPlugin, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = plugin.id.clone();
        let busy = self.busy.contains(&id);
        let name = div().flex().items_center().gap_2()
            .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(if plugin.enabled { theme::text() } else { theme::muted() })
                .child(plugin.name.clone()))
            .when_some(plugin.available_version.clone().filter(|_| has_update(plugin)), |el, v| el
                .child(chip(tr("plugins_available").replace("{version}", &v), theme::accent_text(), theme::accent_dim())));
        let meta = tr("plugins_row_meta").replace("{version}", &plugin.version).replace("{date}", &day(&plugin.last_updated))
            .replace("{marketplace}", &plugin.marketplace);
        let page = cx.entity().downgrade();
        let (toggle_id, switch_id, update_id) = (id.clone(), id.clone(), id.clone());
        row_box(SharedString::from(format!("plugins-row-{id}"))).aria_label(plugin.name.clone()).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_detail(toggle_id.clone(), cx)))
            .child(text_block(name, meta, plugin.description.clone()))
            .when(has_update(plugin), |el| el.child(Button::new(SharedString::from(format!("plugins-update-{id}"))).outline().small()
                .label(tr(if busy { "plugins_updating" } else { "plugins_update" })).loading(busy).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| if let Some(p) = this.installed(&update_id) { this.update(&p, window, cx) }))))
            .child(Switch::new(SharedString::from(format!("plugins-switch-{id}"))).checked(plugin.enabled).disabled(busy)
                .accessibility_label(format!("{} · {}", tr(if plugin.enabled { "plugins_disable" } else { "plugins_enable" }), plugin.name))
                .on_click(cx.listener(move |this, on: &bool, window, cx| if let Some(p) = this.installed(&switch_id) { this.set_enabled(&p, *on, window, cx) })))
            .child(Disclosure::new(format!("plugins-open-{id}"), open, String::new(), true).name(plugin.name.clone())
                // A seta é o caminho do teclado; sem parar aqui, o clique nela também chegaria à linha e desfaria a troca.
                .on_change(move |_, cx| {
                    cx.stop_propagation();
                    let id = id.clone();
                    let _ = page.update(cx, |page, cx| page.toggle_detail(id, cx));
                }))
            .into_any_element()
    }

    fn render_detail(&self, plugin: &InstalledPlugin, cx: &mut Context<Self>) -> AnyElement {
        let id = plugin.id.clone();
        let busy = self.busy.contains(&id);
        let key = |k: &str| div().text_sm().text_color(theme::muted()).child(tr(k));
        let value = |text: String| div().text_sm().whitespace_normal().child(text);
        let version = match (&plugin.available_version, plugin.update_known) {
            (Some(v), true) => format!("{} → {}", plugin.version, tr("plugins_available").replace("{version}", v)),
            (_, true) => format!("{} ({})", plugin.version, tr("plugins_detail_latest")),
            (_, false) => format!("{} · {}", plugin.version, tr("plugins_external_note")),
        };
        let (content, tokens): (AnyElement, AnyElement) = match self.details.get(&id) {
            None => (muted(tr("loading")).into_any_element(), muted(tr("loading")).into_any_element()),
            Some(Err(error)) => (alert(SharedString::from(format!("plugins-detail-error-{id}")), error.clone()).into_any_element(), div().into_any_element()),
            Some(Ok(details)) => (
                div().flex().flex_wrap().gap_1().children(inventory_labels(&details.inventory).into_iter()
                    .map(|l| chip(l, theme::muted(), theme::inset()))).into_any_element(),
                value(details.always_on_tokens.map(|n| tr("plugins_detail_tokens").replace("{n}", &grouped_number(n))).unwrap_or_else(|| "—".into())).into_any_element(),
            ),
        };
        let line = |label: &str, value: AnyElement| div().flex().items_start().gap_4()
            .child(key(label).w(px(130.)).flex_shrink_0()).child(div().flex_1().min_w_0().child(value));
        let grid = div().flex().flex_col().gap_1()
            .child(line("plugins_detail_version", value(version).into_any_element()))
            .child(line("plugins_detail_updated", value(day(&plugin.last_updated)).into_any_element()))
            .child(line("plugins_detail_marketplace", value(plugin.marketplace.clone()).into_any_element()))
            .child(line("plugins_detail_content", content))
            .child(line("plugins_detail_always_on", tokens));
        let update_label = match (&plugin.available_version, plugin.update_known) {
            _ if busy => tr("plugins_updating"),
            (Some(v), true) => tr("plugins_update_to").replace("{version}", v),
            _ => tr("plugins_update"),
        };
        let (update_id, switch_id, remove_id) = (id.clone(), id.clone(), id.clone());
        let on = plugin.enabled;
        // Fonte externa também mostra Atualizar: a versão nova só se descobre pedindo.
        let can_update = has_update(plugin) || !plugin.update_known;
        let actions = div().flex().flex_wrap().items_center().gap_2()
            .when(can_update, |el| el.child(Button::new(SharedString::from(format!("plugins-detail-update-{id}")))
                .map(|b| if has_update(plugin) { b.primary() } else { b.outline() }).small().label(update_label).loading(busy).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| if let Some(p) = this.installed(&update_id) { this.update(&p, window, cx) }))))
            .child(Button::new(SharedString::from(format!("plugins-detail-toggle-{id}"))).outline().small()
                .label(tr(if on { "plugins_disable" } else { "plugins_enable" })).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| if let Some(p) = this.installed(&switch_id) { this.set_enabled(&p, !on, window, cx) })))
            .child(div().flex_1())
            .child(Button::new(SharedString::from(format!("plugins-detail-remove-{id}"))).danger().outline().small().label(tr("plugins_remove")).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| if let Some(p) = this.installed(&remove_id) { this.confirm_remove(&p, window, cx) })));
        div().id(SharedString::from(format!("plugins-detail-{id}"))).role(Role::Group).aria_label(plugin.name.clone())
            .mt(px(-1.)).border_t_1().border_color(theme::border()).bg(theme::inset()).px_4().py(px(14.)).flex().flex_col().gap_3()
            .child(grid).child(actions)
            .into_any_element()
    }

    fn render_discover(&self, state: &PluginsState, cx: &mut Context<Self>) -> AnyElement {
        let input = Input::new(&self.discover_query).small().cleanable(true).aria_label(tr("plugins_tab_discover"));
        let list = match &self.available {
            None => return div().flex().flex_col().gap_3().child(input)
                .child(div().id("plugins-available-loading").role(Role::Status).child(muted(tr("plugins_loading")))).into_any_element(),
            Some(Err(error)) => return div().flex().flex_col().gap_3().child(input).child(alert("plugins-available-error", error.clone()))
                .child(div().child(Button::new("plugins-available-retry").outline().small().label(tr("plugins_retry"))
                    .on_click(cx.listener(|this, _, window, cx| this.load_available(window, cx))))).into_any_element(),
            Some(Ok(list)) => list,
        };
        let query = self.discover_query.read(cx).value().to_string();
        let folded = fold(query.trim());
        let hits: Vec<&AvailablePlugin> = list.iter().filter(|p| found(&p.name, p.description.as_deref(), &folded)).collect();
        let installed: HashSet<&str> = state.plugins.iter().map(|i| i.id.as_str()).collect();
        let body = if list.is_empty() {
            settings_box().px_4().py(px(18.)).child(muted(tr("plugins_discover_none"))).into_any_element()
        } else if hits.is_empty() {
            settings_box().px_4().py(px(18.)).child(muted(tr("plugins_discover_empty").replace("{query}", query.trim()))).into_any_element()
        } else {
            settings_box().id("plugins-available").role(Role::List).aria_label(tr("plugins_tab_discover")).children(hits.into_iter().map(|p| {
                let id = p.id.clone();
                let done = self.installed_now.contains(&id) || installed.contains(id.as_str());
                let busy = self.busy.contains(&id);
                let meta = [p.version.clone(), Some(p.marketplace.clone())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let (target, target_name) = (id.clone(), p.name.clone());
                row_box(SharedString::from(format!("plugins-available-{id}"))).aria_label(p.name.clone())
                    .child(text_block(div().truncate().font_weight(FontWeight::MEDIUM).child(p.name.clone()), meta, p.description.clone()))
                    .child(if done { chip(tr("plugins_installed"), theme::success_text(), theme::success().opacity(0.12)).into_any_element() } else {
                        Button::new(SharedString::from(format!("plugins-install-{id}"))).primary().small()
                            .label(tr(if busy { "plugins_installing" } else { "plugins_install" })).loading(busy).disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.install(target.clone(), target_name.clone(), window, cx))).into_any_element()
                    })
                    .into_any_element()
            })).into_any_element()
        };
        div().flex().flex_col().gap_3().child(input).child(body).into_any_element()
    }

    fn render_marketplaces(&self, state: &PluginsState, cx: &mut Context<Self>) -> AnyElement {
        let adding = self.busy.contains("mkt:+");
        let bar = div().flex().items_center().gap_2()
            .child(div().flex_1().min_w_0().child(Input::new(&self.source).small().disabled(adding).aria_label(tr("plugins_marketplace_placeholder"))))
            .child(Button::new("plugins-marketplace-add").primary().small().label(tr("plugins_marketplace_add")).loading(adding).disabled(adding)
                .on_click(cx.listener(|this, _, window, cx| this.add_marketplace(window, cx))));
        let body = if state.marketplaces.is_empty() {
            settings_box().px_4().py(px(18.)).child(muted(tr("plugins_marketplaces_empty"))).into_any_element()
        } else {
            settings_box().id("plugins-marketplaces").role(Role::List).aria_label(tr("plugins_tab_marketplaces"))
                .children(state.marketplaces.iter().map(|m| self.render_marketplace(m, cx))).into_any_element()
        };
        div().flex().flex_col().gap_3().child(bar)
            .children(self.source_error.clone().map(|e| alert("plugins-source-error", e)))
            .child(body).into_any_element()
    }

    fn render_marketplace(&self, marketplace: &PluginMarketplace, cx: &mut Context<Self>) -> AnyElement {
        let name = marketplace.name.clone();
        let busy = self.busy.contains(&format!("mkt:{name}"));
        let source = if marketplace.builtin { tr("plugins_marketplace_builtin") } else { marketplace.source_label.clone() };
        let downloaded = marketplace.last_updated.as_deref().map(|d| tr("plugins_marketplace_downloaded").replace("{date}", &day(d)));
        let (auto_name, update_name, remove_name) = (name.clone(), name.clone(), name.clone());
        row_box(SharedString::from(format!("plugins-marketplace-{name}"))).aria_label(name.clone())
            .child(text_block(div().truncate().font_weight(FontWeight::MEDIUM).child(name.clone()), source, downloaded))
            // O embutido é da Anthropic: o servidor recusa qualquer ação nele.
            .when(!marketplace.builtin, |el| el
                .child(div().flex_shrink_0().flex().items_center().gap_2()
                    .child(div().text_sm().text_color(if marketplace.auto_update_editable { theme::muted() } else { theme::faint() })
                        .child(tr("plugins_marketplace_auto_update")))
                    .child(Switch::new(SharedString::from(format!("plugins-auto-{name}"))).checked(marketplace.auto_update)
                        .disabled(busy || !marketplace.auto_update_editable)
                        .accessibility_label(format!("{} · {name}", tr("plugins_marketplace_auto_update")))
                        .on_click(cx.listener(move |this, on: &bool, window, cx| this.set_auto_update(auto_name.clone(), *on, window, cx)))))
                .child(Button::new(SharedString::from(format!("plugins-marketplace-update-{name}"))).outline().small()
                    .label(tr(if busy { "plugins_updating" } else { "plugins_update" })).loading(busy).disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.update_marketplace(update_name.clone(), window, cx))))
                .child(Button::new(SharedString::from(format!("plugins-marketplace-remove-{name}"))).danger().outline().small()
                    .label(tr("plugins_remove")).disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm_remove_marketplace(remove_name.clone(), window, cx)))))
            .into_any_element()
    }
}

impl Render for PluginsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.request_details(window, cx);
        let top = div().flex().flex_col()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Plugins.title()))
            .child(div().mt(px(6.)).text_color(theme::muted()).whitespace_normal().child(tr("plugins_subtitle")));
        let checked = self.state.as_ref().and_then(|s| s.as_ref().ok()).map(|s| checked_label(s.checked_at.as_deref(), &chrono::Local::now()));
        let tabs = TabBar::new("plugins-tabs").underline().small().selected_index(self.tab)
            .children([Tab::new().label(tr("plugins_tab_installed")), Tab::new().label(tr("plugins_tab_discover")), Tab::new().label(tr("plugins_tab_marketplaces"))])
            .on_click(cx.listener(|this, ix: &usize, window, cx| this.select_tab(*ix, window, cx)));
        let bar = div().flex().flex_wrap().items_center().gap_3()
            .child(div().flex_1().min_w_0().child(tabs))
            .children(checked.map(|text| div().id("plugins-checked").role(Role::Status).text_sm().text_color(theme::muted()).child(text)))
            .child(Button::new("plugins-check").outline().small().label(tr(if self.checking { "plugins_checking" } else { "plugins_check" }))
                .loading(self.checking).disabled(self.checking || self.state.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))));
        let mut page = div().flex().flex_col().gap_4().child(top).child(bar).children(self.render_reopen(cx));
        if let Some(error) = self.error.clone() { page = page.child(alert("plugins-error", error)); }
        let state = match &self.state {
            None => return page.child(div().id("plugins-loading").role(Role::Status).child(muted(tr("plugins_loading")))),
            Some(Err(error)) => { let error = error.clone(); return page.child(alert("plugins-state-error", error)).child(div().child(self.retry("plugins-retry", cx))); }
            Some(Ok(state)) => state,
        };
        let body = match self.tab {
            TAB_INSTALLED => self.render_installed(state, cx),
            TAB_DISCOVER => self.render_discover(state, cx),
            _ => self.render_marketplaces(state, cx),
        };
        page.child(body)
    }
}

impl Hangar {
    /// A página é refeita a cada abertura e a cada conexão.
    pub(super) fn render_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return div().text_color(theme::muted()).child(tr("settings_offline")).into_any_element();
        };
        if self.plugins_page.as_ref().is_none_or(|(connection, _)| *connection != self.connection) {
            let (runtime, hangar) = (self.runtime.clone(), cx.entity().downgrade());
            let page = cx.new(|cx| PluginsPage::new(api, runtime, hangar, window, cx));
            self.plugins_page = Some((self.connection, page));
        }
        self.plugins_page.as_ref().map(|(_, page)| page.clone().into_any_element()).unwrap_or_else(|| div().into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn plugin(name: &str, enabled: bool, available: Option<&str>, known: bool) -> InstalledPlugin {
        InstalledPlugin { id: format!("{name}@m"), name: name.into(), marketplace: "m".into(), version: "1.0.0".into(),
            last_updated: "2026-11-07T10:00:00Z".into(), enabled, available_version: available.map(str::to_owned), update_known: known,
            description: Some(format!("faz {name}")) }
    }

    #[test]
    fn settings_page_plugins_has_both_translations() {
        let dicts = [include_str!("../../../messages/pt.json"), include_str!("../../../messages/en.json")]
            .map(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).unwrap());
        let keys = ["settings_page_plugins", "plugins_inventory_skills", "plugins_inventory_skills_one", "plugins_inventory_agents",
            "plugins_inventory_agents_one", "plugins_inventory_hooks", "plugins_inventory_hooks_one", "plugins_inventory_mcp", "plugins_inventory_lsp"];
        let missing: Vec<&str> = keys.into_iter().filter(|k| dicts.iter().any(|d| !d.contains_key(&format!("native_{k}")))).collect();
        assert!(missing.is_empty(), "sem native_* em pt.json/en.json: {missing:?}");
    }

    #[test]
    fn filter_counts_only_known_updates_and_matches_name_or_description() {
        let list = [plugin("caveman", true, Some("4.2.0"), true), plugin("humanizer", false, None, true), plugin("ext", true, None, false)];
        let names = |filter, query| shown(&list, filter, query).iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(Filter::Updates, ""), ["caveman"]);
        assert_eq!(names(Filter::Disabled, ""), ["humanizer"]);
        assert_eq!(names(Filter::Enabled, ""), ["caveman", "ext"]);
        assert_eq!(names(Filter::All, "FAZ HUM"), ["humanizer"]);
        assert!(!has_update(&plugin("ext", true, Some("2"), false)));
    }

    #[test]
    fn marketplace_source_follows_the_server_rule() {
        for ok in ["JuliusBrussee/caveman", "https://gitlab.example/a/b.git", "/home/me/mkt", "C:\\mkt"] { assert!(source_ok(ok), "{ok}"); }
        for bad in ["", "caveman", "-x/y", "a/b/c", "http://x", "dono/re po"] { assert!(!source_ok(bad), "{bad}"); }
    }

    #[test]
    fn reopen_counts_live_claude_sessions_only() {
        let session = |name: &str, provider: &str, state: &str, asleep: bool| SessionInfo {
            name: name.into(), provider: provider.into(), state: state.into(), asleep, ..Default::default() };
        let sessions = [session("a", "claude", "idle", false), session("b", "", "working", false), session("c", "codex", "idle", false),
            session("d", "claude", "dead", false), session("e", "claude", "idle", true)];
        assert_eq!(live_claude(&sessions), ["a", "b"]);
    }

    #[test]
    fn numbers_and_check_time_follow_the_language() {
        crate::i18n::english_in_this_thread(false);
        assert_eq!((grouped_number(90), grouped_number(1366), grouped_number(1_234_567)), ("90".into(), "1.366".into(), "1.234.567".into()));
        use chrono::TimeZone;
        // Relógio e fuso fixos (UTC-3): a suíte não depende do fuso da máquina.
        let zone = chrono::FixedOffset::west_opt(3 * 3600).unwrap();
        let now = zone.with_ymd_and_hms(2026, 11, 9, 10, 0, 0).unwrap();
        assert_eq!(checked_label(Some("2026-11-09T12:52:00Z"), &now), "Conferido hoje às 09:52");
        // 02:00 UTC ainda é o dia anterior em UTC-3.
        assert_eq!(checked_label(Some("2026-11-09T02:00:00Z"), &now), "Conferido em 08/11/2026");
        assert_eq!(checked_label(None, &now), tr("plugins_never_checked"));
        crate::i18n::english_in_this_thread(true);
        assert_eq!(grouped_number(1366), "1,366");
    }

    #[test]
    fn inventory_uses_singular_and_skips_zero() {
        crate::i18n::english_in_this_thread(false);
        let inventory = PluginInventory { skills: Some(1), agents: Some(3), hooks: Some(0), mcp: Some(2), lsp: None };
        assert_eq!(inventory_labels(&inventory), ["1 skill", "3 agentes", "2 MCP"]);
    }
}
