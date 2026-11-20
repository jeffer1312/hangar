//! Seletor de modelos no estilo do T3 Code, no lugar do menu simples de modelo: trilho com os favoritos, o agente
//! da sessão e o Codex que pode receber a conversa; busca sempre à vista; estrela de favorito e Ctrl+1…9 por linha;
//! "Mostrar mais" quando a lista é longa; ao lado, o raciocínio do modelo em foco; embaixo, a velocidade.
//! Escolher um modelo do Codex numa sessão Claude pede a troca de agente (`POST /conta` com `when_idle`): com a
//! sessão trabalhando, o servidor espera o turno acabar, e o que for enviado no meio vai ao modelo novo.
use super::*;

pub(super) const FAVORITES: &str = "favorites";
/// Linhas da aba antes do "Mostrar mais".
const SHORT: usize = 6;
/// Largura do trilho e da coluna de raciocínio.
const RAIL: f32 = 44.;
const REASONING: f32 = 196.;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Pick { Apply, Handoff { credential: String, model: String, effort: Option<String> }, More }

/// Conta Codex que recebe a conversa na troca: `credential` vai no pedido, `account` lê o catálogo.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct HandoffAccount { pub(in crate::app) credential: String, pub(in crate::app) account: String, pub(in crate::app) label: String }

/// Catálogo do Codex lido para a troca, da sessão e da conta em que foi pedido.
pub(in crate::app) struct HandoffCatalog { key: SessionKey, account: HandoffAccount, catalog: Option<Result<Value, String>> }

fn provider_label(provider: &str) -> String {
    match provider { "claude" => "Claude".into(), "codex" => "Codex".into(), "omp" => "omp".into(), other => capitalized(other) }
}

/// Nome curto do modelo para o divisor e a faixa da troca: `claude-opus-5-5` vira "Opus 5.5", `gpt-6.2-sol` vira
/// "GPT-6.2-Sol". Sem modelo, o nome do agente.
pub(crate) fn model_display(provider: &str, model: &str) -> String {
    let model = model.trim();
    if model.is_empty() { return provider_label(provider); }
    let (base, wide) = match model.strip_suffix("[1m]") { Some(base) => (base, true), None => (model, false) };
    let shown = if provider == "claude" {
        let rest = base.strip_prefix("claude-").unwrap_or(base);
        // A data no fim do id (`20250929`) não entra no nome.
        let mut parts = rest.split('-').filter(|p| !(p.len() == 8 && p.bytes().all(|b| b.is_ascii_digit())));
        let family = parts.next().map(capitalized).unwrap_or_default();
        let version: Vec<&str> = parts.collect();
        if version.is_empty() { family }
        else if version.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) { format!("{family} {}", version.join(".")) }
        else { format!("{family} {}", version.iter().map(|p| capitalized(p)).collect::<Vec<_>>().join(" ")) }
    } else {
        base.split('-').map(|p| if p.eq_ignore_ascii_case("gpt") { "GPT".to_owned() } else { capitalized(p) }).collect::<Vec<_>>().join("-")
    };
    if wide { format!("{shown} 1M") } else { shown }
}

/// Atalho da linha `n` (1…9) no jeito da plataforma.
fn jump_label(n: usize) -> String { if cfg!(target_os = "macos") { format!("⌘{n}") } else { format!("Ctrl+{n}") } }

fn shortcut(text: String) -> Div {
    div().flex_none().px(px(5.)).py(px(1.)).rounded(px(5.)).bg(theme::inset())
        .font_family(theme::MONO).text_size(px(10.)).text_color(theme::faint()).child(text)
}

/// Linha de modelo: agente, nome, visto da atual, estrela e atalho. A tela e a animação da novidade usam a mesma.
pub(crate) fn row_view(id: SharedString, provider: &str, name: AnyElement, current: bool, lit: bool, star: Option<AnyElement>, jump: Option<usize>) -> Button {
    let more = provider.is_empty();
    popup::row(id, current)
        .when(lit && !current, |el| el.bg(theme::text().opacity(0.06)))
        .child(div().w_full().flex().items_center().gap_2()
            .when(!more, |el| el.child(chrome::provider_glyph(provider, 16.)))
            .child(div().flex_1().min_w_0().child(name))
            .when(current, |el| el.child(chrome::small_icon(IconName::Check, 15., theme::accent())))
            .children(star)
            .children(jump.map(|n| shortcut(jump_label(n)))))
}

/// A estrela de favorito, acesa ou não.
pub(crate) fn star_icon(favorite: bool) -> Icon { chrome::small_icon(IconName::Star, 13., if favorite { theme::accent() } else { theme::faint() }) }

/// Aba do trilho com a barra de destaque quando está à vista.
pub(crate) fn rail_tab(button: Button, selected: bool) -> Div {
    div().relative().child(button)
        .when(selected, |el| el.child(div().absolute().left(px(-5.)).top(px(9.)).w(px(3.)).h(px(16.)).rounded(px(2.)).bg(theme::accent())))
}

pub(crate) fn rail_button(id: &str, selected: bool, label: String, child: AnyElement) -> Button {
    Button::new(SharedString::from(format!("picker-tab-{id}"))).ghost().w(px(34.)).h(px(34.)).rounded(px(8.)).selected(selected)
        .tooltip(label.clone()).accessibility_label(label).child(child)
}

pub(crate) fn rail_view(tabs: Vec<AnyElement>) -> Div {
    div().w(px(RAIL)).flex_none().flex().flex_col().items_center().gap(px(4.)).py(px(4.)).border_r_1().border_color(theme::border()).children(tabs)
}

/// Nível de raciocínio na coluna ao lado da lista.
pub(crate) fn effort_row(label: &str, on: bool) -> Button {
    popup::row(SharedString::from(format!("picker-effort-{label}")), on)
        .child(div().w_full().flex().items_center().gap_2()
            .child(div().flex_1().min_w_0().truncate().text_sm().child(effort_label(label)))
            .when(on, |el| el.child(chrome::small_icon(IconName::Check, 15., theme::accent()))))
}

pub(crate) fn reasoning_view(header: String, rows: Vec<AnyElement>) -> Div {
    let empty = rows.is_empty();
    div().w(px(REASONING)).flex_none().flex().flex_col().gap(px(2.)).pl(px(6.)).border_l_1().border_color(theme::border())
        .child(div().px(px(8.)).pt(px(6.)).pb(px(4.)).flex().items_center().gap_2()
            .child(div().flex_none().text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(theme::faint()).child(tr("new_chat_reasoning").to_uppercase()))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme::faint()).text_right().child(header)))
        .when(empty, |el| el.child(div().px(px(8.)).py(px(6.)).text_xs().text_color(theme::muted()).child(tr("picker_no_reasoning"))))
        .children(rows)
}

/// "Opus 5.5 → GPT-6.2-Sol" com a marca de cada agente.
pub(crate) fn route_view(from_provider: &str, from: String, to_provider: &str, to: String, size: f32) -> Div {
    div().flex().items_center().gap(px(5.)).min_w_0()
        .child(chrome::provider_glyph(from_provider, 14.)).child(div().truncate().text_size(px(size)).child(from))
        .child(chrome::small_icon(IconName::ArrowRight, 12., theme::faint()))
        .child(chrome::provider_glyph(to_provider, 14.)).child(div().truncate().text_size(px(size)).child(to))
}

/// Faixa sobre o compositor da troca pedida.
pub(crate) fn band_view(icon: IconName, title: String, color: Hsla, route: Div, cancel: Option<Button>) -> Div {
    div().py_2().flex().items_center().gap_2().border_t_1().border_color(theme::border())
        .child(chrome::small_icon(icon, 14., color))
        .child(div().flex_none().text_sm().text_color(color).child(title))
        .child(div().flex_1().min_w_0().child(route))
        .children(cancel)
}

/// O divisor da conversa onde o agente mudou.
pub(crate) fn divider_view(route: Div) -> Div {
    let content = div().flex().items_center().gap(px(8.)).flex_none().text_color(theme::muted())
        .child(chrome::small_icon(IconName::RefreshCw, 13., theme::faint()))
        .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(tr_shared("handoff_divider", &[])))
        .child(route);
    super::super::notice_line::notice_rule_line(content)
}

/// Favoritos primeiro, cada grupo na ordem do agente; a ordem dos favoritos não muda a da lista.
fn favorites_first(rows: &mut [Choice], favorites: &[String]) { rows.sort_by_key(|c| !favorites.contains(&c.key)); }

/// A aba mostra `SHORT` linhas e o "Mostrar mais"; a atual fica à vista mesmo além do corte.
fn shortened(rows: Vec<Choice>, more: bool) -> Vec<Choice> {
    if more || rows.len() <= SHORT + 1 { return rows; }
    let current = rows.iter().position(|c| c.current);
    let (mut kept, mut hidden) = (Vec::new(), 0);
    for (n, row) in rows.into_iter().enumerate() {
        if n < SHORT || Some(n) == current { kept.push(row) } else { hidden += 1 }
    }
    if hidden > 0 {
        kept.push(Choice { label: tr("picker_show_more"), detail: String::new(), path: Vec::new(), body: json!({"more": true}),
            current: false, enabled: true, provider: String::new(), key: String::new(), pick: Pick::More });
    }
    kept
}

impl Hangar {
    fn handoff_source_now(&self) -> Option<(String, String)> {
        self.selected.as_ref().and_then(super::super::sidebar::handoff_source).map(|(life, jsonl)| (life.to_owned(), jsonl.to_owned()))
    }

    fn favorite_list(&self) -> &[String] { self.controls.favorites.as_deref().unwrap_or_default() }

    /// O seletor abriu: aba e lista curta voltam ao começo, os favoritos saem do disco na primeira vez e as contas Codex
    /// são lidas quando a sessão pode levar a conversa ao Codex.
    pub(super) fn picker_opened(&mut self, cx: &mut Context<Self>) {
        self.controls.picker_tab = None;
        self.controls.picker_more = false;
        if self.controls.favorites.is_none() { self.controls.favorites = Some(crate::appearance::favorite_models()); }
        if self.handoff_source_now().is_some() { self.ensure_session_accounts(cx); }
    }

    /// Lê o catálogo do Codex da conta de mais folga quando ele falta: as contas chegam depois da abertura, e quem as
    /// espera é o desenho do seletor.
    pub(super) fn picker_poll(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        if self.handoff_source_now().is_none() { return; }
        let Some(account) = self.handoff_accounts().and_then(|list| list.into_iter().next()) else { return };
        if self.controls.handoff.as_ref().is_some_and(|h| h.key == key && h.account.credential == account.credential) { return; }
        let Some(api) = self.session_api() else { return };
        let (credential, codex_account) = (account.credential.clone(), account.account.clone());
        self.controls.handoff = Some(HandoffCatalog { key: key.clone(), account, catalog: None });
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.server_read(&["model-options"], &[("provider", "codex"), ("codex_account", codex_account.as_str())], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::HandoffCatalog(credential), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_handoff_catalog(&mut self, key: SessionKey, credential: String, result: Result<Value, Failure>) {
        let Some(handoff) = self.controls.handoff.as_mut().filter(|h| h.key == key && h.account.credential == credential) else { return };
        handoff.catalog = Some(result.map_err(|error| Self::failure(&error)));
    }

    fn handoff_catalog(&self) -> Option<&HandoffCatalog> {
        let key = self.selected_key()?;
        self.controls.handoff.as_ref().filter(|h| h.key == key)
    }

    /// Os modelos do Codex que podem receber a conversa, como linhas de troca.
    fn handoff_rows(&self) -> Vec<Choice> {
        let Some(handoff) = self.handoff_catalog() else { return Vec::new() };
        let Some(Ok(catalog)) = &handoff.catalog else { return Vec::new() };
        catalog.get("models").and_then(Value::as_array).into_iter().flatten()
            .filter(|m| m.get("hidden").and_then(Value::as_bool) != Some(true))
            .filter_map(|m| {
                let id = text(m, "id");
                if id.is_empty() { return None; }
                let label = Some(text(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                let effort = Some(text(m, "default_effort")).filter(|e| !e.is_empty());
                Some(Choice { label, detail: text(m, "desc"), path: Vec::new(), body: json!({"handoff": id}), current: false, enabled: true,
                    provider: "codex".into(), key: format!("codex:{id}"),
                    pick: Pick::Handoff { credential: handoff.account.credential.clone(), model: id, effort } })
            }).collect()
    }

    /// Abas do trilho: o agente da sessão e, numa sessão Claude que pode trocar, o Codex (desligado sem conta conectada).
    fn picker_providers(&self) -> Vec<(String, bool)> {
        let own = self.provider().0.to_owned();
        let mut tabs = vec![(own, true)];
        if self.handoff_source_now().is_some() {
            tabs.push(("codex".into(), self.handoff_accounts().is_some_and(|list| !list.is_empty())));
        }
        tabs
    }

    /// A aba à vista: a escolhida, ou os favoritos quando algum está na lista, ou o agente da sessão.
    fn picker_tab(&self, own: &[Choice], handoff: &[Choice]) -> String {
        if let Some(tab) = &self.controls.picker_tab { return tab.clone(); }
        let favorites = self.favorite_list();
        if own.iter().chain(handoff).any(|c| favorites.contains(&c.key)) { FAVORITES.into() } else { self.provider().0.to_owned() }
    }

    /// As linhas do seletor: busca em todos os agentes; senão a aba, favoritos primeiro, e o "Mostrar mais".
    pub(super) fn picker_rows(&self, catalog: &Value, cx: &App) -> Vec<Choice> {
        let query = self.ctl_search.read(cx).value().trim().to_owned();
        let own = self.choices(Ctl::Model, catalog);
        let handoff = self.handoff_rows();
        if !query.is_empty() { return keep(own.into_iter().chain(handoff).collect(), &query); }
        let favorites = self.favorite_list();
        let tab = self.picker_tab(&own, &handoff);
        if tab == FAVORITES {
            let mut rows: Vec<Choice> = own.into_iter().chain(handoff).filter(|c| favorites.contains(&c.key)).collect();
            rows.sort_by_key(|c| favorites.iter().position(|f| *f == c.key));
            return rows;
        }
        let mut rows = if tab == "codex" && self.provider().0 != "codex" { handoff } else { own };
        favorites_first(&mut rows, favorites);
        shortened(rows, self.controls.picker_more)
    }

    /// Ctrl+N: a linha `n` da lista à vista.
    pub(super) fn picker_jump(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, choices, _)) = self.open_choices(cx) else { return };
        if choices.get(n - 1).is_none_or(|c| c.pick == Pick::More) { return; }
        self.pick_ctl(Some(n - 1), window, cx);
    }

    fn toggle_favorite(&mut self, key: String, cx: &mut Context<Self>) {
        // A aba à vista fica: a primeira estrela não leva a pessoa para os Favoritos.
        if self.controls.picker_tab.is_none() {
            let tab = self.controls.open.as_ref().and_then(|o| o.catalog.as_ref()).and_then(|c| c.as_ref().ok())
                .map(|catalog| self.picker_tab(&self.choices(Ctl::Model, catalog), &self.handoff_rows()));
            self.controls.picker_tab = tab;
        }
        let list = self.controls.favorites.get_or_insert_with(Vec::new);
        match list.iter().position(|f| *f == key) { Some(at) => { list.remove(at); } None => list.push(key) }
        let saved = list.clone();
        self.runtime.spawn_blocking(move || crate::appearance::save_favorite_models(&saved));
        cx.notify();
    }

    /// Pede a troca de agente: o servidor responde na hora e a linha da lista acompanha a espera.
    pub(super) fn request_handoff(&mut self, credential: String, model: String, effort: Option<String>, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        if self.controls.busy.contains_key(&key) || !self.chat_online { return; }
        let Some((life, jsonl)) = self.handoff_source_now() else {
            self.action_feedback.insert(key, (tr("handoff_unavailable"), true));
            cx.notify();
            return;
        };
        let body = json!({"credential_id": credential, "source_life": life, "source_jsonl": jsonl, "model": model, "effort": effort, "when_idle": true});
        self.close_controls();
        self.controls.busy.insert(key.clone(), Ctl::Model);
        self.action_feedback.remove(&key);
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["conta"], Some(body), false, 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Handoff, result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_handoff(&mut self, key: SessionKey, result: Result<Value, Failure>) {
        if self.controls.busy.get(&key) == Some(&Ctl::Model) { self.controls.busy.remove(&key); }
        let note = match result { Ok(_) => (tr("handoff_requested"), false), Err(error) => (Self::failure(&error), true) };
        self.action_feedback.insert(key, note);
    }

    fn cancel_handoff(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["conta"], None, true, 15).await.map(|_| Value::Null);
            let note = result.err().map(|error| Hangar::failure(&error));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::HandoffCancel(note), Ok(Value::Null)) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_handoff_cancel(&mut self, key: SessionKey, note: Option<String>) {
        if let Some(note) = note { self.action_feedback.insert(key, (note, true)); }
    }

    /// Faixa sobre o compositor enquanto a troca espera, acontece ou acabou de falhar.
    pub(in crate::app) fn render_handoff_band(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.selected.as_ref()?;
        let handoff = session.handoff.as_ref()?;
        let live = self.status().and_then(|s| s.model);
        let from = model_display(&session.provider, session.model.as_deref().or(live.as_deref()).unwrap_or_default());
        let to = model_display(&handoff.provider, handoff.model.as_deref().unwrap_or_default());
        let route = route_view(&session.provider, from, &handoff.provider, to, 14.);
        let (title, color) = match handoff.state.as_str() {
            "running" => (tr_shared("handoff_running", &[]), theme::muted()),
            "failed" => {
                let reason = handoff.code.as_deref().map(|code| crate::i18n::tr_web(code, &HashMap::new()).unwrap_or_else(|| code.to_owned())).unwrap_or_default();
                (tr_shared("handoff_failed", &[("reason", &reason)]), theme::warning())
            }
            _ => (tr_shared(if session.state == "working" { "handoff_waiting_turn" } else { "handoff_waiting" }, &[]), theme::muted()),
        };
        let waiting = handoff.state == "waiting";
        let icon = if waiting { IconName::Hourglass } else if handoff.state == "failed" { IconName::TriangleAlert } else { IconName::LoaderCircle };
        let cancel = waiting.then(|| Button::new("handoff-cancel").small().ghost().label(tr_shared("handoff_cancel", &[]))
            .on_click(cx.listener(|this, _, _, cx| this.cancel_handoff(cx))));
        Some(band_view(icon, title, color, route, cancel).id("handoff-band").into_any_element())
    }

    /// Divisor da conversa onde o agente mudou: "Troca de contexto · Opus 5.5 → GPT-6.2-Sol".
    pub(in crate::app) fn render_handoff_line(&self, event_index: usize) -> AnyElement {
        let Some(handoff) = self.chat.events.get(event_index).and_then(|event| event.handoff.as_ref()) else { return div().into_any_element() };
        let field = |name: &str| handoff.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
        let (from, to) = (field("from_provider"), field("to_provider"));
        let route = route_view(&from, model_display(&from, &field("from_model")), &to, model_display(&to, &field("to_model")), 12.5);
        divider_view(route).into_any_element()
    }

    fn picker_row(&self, n: usize, c: Choice, lit: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let (current, enabled, more) = (c.current, c.enabled, c.pick == Pick::More);
        let id = SharedString::from(format!("ctl-choice-{}", c.body));
        let favorite = !c.key.is_empty() && self.favorite_list().contains(&c.key);
        let star = (!c.key.is_empty()).then(|| {
            let key = c.key.clone();
            Button::new(SharedString::from(format!("picker-fav-{}", c.key))).ghost().xsmall()
                .accessibility_label(tr(if favorite { "picker_unfavorite" } else { "picker_favorite" }))
                .child(star_icon(favorite))
                .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.toggle_favorite(key.clone(), cx); }))
                .into_any_element()
        });
        // Só o nome na linha, como no T3; a descrição fica na dica do nome e na busca. A dica mora no nome: a linha já
        // ouve o hover para o destaque, e o GPUI não aceita dois no mesmo elemento.
        let detail = c.detail.clone();
        let name = div().id(SharedString::from(format!("picker-name-{}", c.body))).min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM)
            .text_color(if more { theme::muted() } else { theme::text() }).child(c.label)
            .when(!detail.is_empty(), |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(detail.clone()).build(window, cx)));
        let provider = if more { String::new() } else { c.provider.clone() };
        row_view(id, &provider, name.into_any_element(), current, lit, star, (!more && n < 9).then_some(n + 1))
            .disabled(busy || !enabled)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && enabled && this.controls.highlight != Some(n) { this.controls.highlight = Some(n); cx.notify(); }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.pick_ctl(Some(n), window, cx)))
            .into_any_element()
    }

    fn picker_rail(&self, own: &[Choice], handoff: &[Choice], cx: &mut Context<Self>) -> Div {
        let tab = self.picker_tab(own, handoff);
        let favorites = self.favorite_list();
        let has_favorites = own.iter().chain(handoff).any(|c| favorites.contains(&c.key));
        let button = |id: String, selected: bool, enabled: bool, label: String, child: AnyElement, cx: &mut Context<Self>| {
            let tab = id.clone();
            let button = rail_button(&id, selected, label, child).disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.controls.picker_tab = Some(tab.clone());
                    this.controls.picker_more = false;
                    this.controls.highlight = None;
                    this.controls.revealed.set(false);
                    cx.notify();
                }));
            // A aba à vista leva a barra de destaque na borda, como o trilho do T3.
            rail_tab(button, selected).into_any_element()
        };
        let mut items = Vec::new();
        if has_favorites {
            items.push(button(FAVORITES.into(), tab == FAVORITES, true, tr("picker_favorites"),
                chrome::small_icon(IconName::Star, 16., theme::accent()).into_any_element(), cx));
        }
        for (provider, enabled) in self.picker_providers() {
            let label = if provider != self.provider().0 && !enabled { tr("picker_handoff_no_account") } else { provider_label(&provider) };
            items.push(button(provider.clone(), tab == provider, enabled, label, chrome::provider_glyph(&provider, 20.).into_any_element(), cx));
        }
        rail_view(items)
    }

    /// Coluna de raciocínio do modelo em foco (o destacado; sem destaque, o atual).
    fn picker_reasoning(&self, catalog: &Value, rows: &[Choice], busy: bool, cx: &mut Context<Self>) -> Div {
        let pick = self.ctl_highlight(rows);
        let focus = rows.get(pick).filter(|c| c.pick != Pick::More).or_else(|| rows.iter().find(|c| c.current));
        let provider = self.provider().0;
        // (rótulo, ligado, ação): a ação aplica o esforço no agente da sessão ou pede a troca com ele.
        let mut levels: Vec<(String, bool, Pick, Vec<&'static str>, Value)> = Vec::new();
        let mut header = String::new();
        if let Some(focus) = focus {
            header = focus.label.clone();
            match &focus.pick {
                Pick::Handoff { credential, model, .. } => {
                    let entry = self.handoff_catalog().and_then(|h| h.catalog.as_ref()).and_then(|c| c.as_ref().ok())
                        .and_then(|c| c.get("models").and_then(Value::as_array)).and_then(|models| models.iter().find(|m| text(m, "id") == *model));
                    let default = entry.map(|m| text(m, "default_effort")).unwrap_or_default();
                    for effort in entry.and_then(|m| m.get("efforts").and_then(Value::as_array)).into_iter().flatten().filter_map(Value::as_str) {
                        levels.push((effort.to_owned(), effort == default,
                            Pick::Handoff { credential: credential.clone(), model: model.clone(), effort: Some(effort.to_owned()) }, Vec::new(), Value::Null));
                    }
                }
                Pick::Apply if provider == "codex" => {
                    let model = focus.body.get("model").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let now = catalog.get("current").cloned().unwrap_or(Value::Null);
                    let entry = catalog.get("models").and_then(Value::as_array).and_then(|models| models.iter().find(|m| text(m, "model") == model));
                    for e in entry.and_then(|m| m.get("efforts").and_then(Value::as_array)).into_iter().flatten() {
                        let value = text(e, "value");
                        if value.is_empty() { continue; }
                        let on = model == text(&now, "model") && value == text(&now, "effort");
                        levels.push((value.clone(), on, Pick::Apply, vec!["model"], json!({"model": model, "effort": value})));
                    }
                }
                // Claude: os níveis valem para qualquer modelo da lista (menos o Haiku, que não tem); noutro modelo, o nível
                // troca modelo e esforço no mesmo pedido.
                Pick::Apply if provider == "claude" && focus.path.first() == Some(&"model-effort") => {
                    if !focus.label.to_lowercase().contains("haiku") {
                        let now = self.ctl_label(Ctl::Effort).unwrap_or_default();
                        let model = focus.body.get("model").and_then(Value::as_str).unwrap_or_default().to_owned();
                        for effort in CLAUDE_EFFORTS {
                            let on = focus.current && claude_effort_current(&now) == Some(effort);
                            let body = if focus.current { json!({"effort": effort, "scope": "session"}) }
                                else { json!({"model": model, "effort": effort, "scope": "session"}) };
                            levels.push((effort.to_owned(), on, Pick::Apply, vec!["model-effort"], body));
                        }
                    }
                }
                Pick::Apply if focus.current && self.ctl_list().contains(&Ctl::Effort) => {
                    let now = self.ctl_label(Ctl::Effort).unwrap_or_default();
                    for c in self.choices(Ctl::Effort, catalog) {
                        let on = c.label.eq_ignore_ascii_case(now.trim()) || (now.trim().is_empty() && c.current);
                        levels.push((c.label, on, Pick::Apply, c.path, c.body));
                    }
                }
                _ => {}
            }
        }
        let list = levels.into_iter().map(|(label, on, pick, path, body)| {
            effort_row(&label, on).disabled(busy || !self.chat_online)
                .on_click(cx.listener(move |this, _, window, cx| {
                    match pick.clone() {
                        Pick::Handoff { credential, model, effort } => {
                            this.composer.update(cx, |input, cx| input.focus(window, cx));
                            this.request_handoff(credential, model, effort, cx);
                        }
                        _ if on => {}
                        _ => { this.apply_ctl(Ctl::Effort, path.clone(), body.clone(), label.clone(), cx); }
                    }
                }))
                .into_any_element()
        }).collect::<Vec<_>>();
        reasoning_view(header, list)
    }

    /// Velocidade como no T3: "Padrão" e "Rápido" lado a lado, quando o modelo atual tem o nível rápido.
    fn picker_speed(&self, catalog: &Value, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (on, available) = match self.provider().0 {
            "codex" => codex_fast_state(catalog, self.ctl_label(Ctl::Fast).as_deref()),
            "claude" => {
                let (Some(key), Some(session)) = (self.selected_key(), self.selected.as_ref()) else { return None; };
                self.controls.model_catalog(&key, session)?;
                if !catalog_engine_matches(catalog, session.engine.as_deref()) { return None; }
                claude_fast_state(catalog)?
            }
            _ => return None,
        };
        if !available { return None; }
        let option = |id: &'static str, label: String, selected: bool, tier: &'static str, cx: &mut Context<Self>| {
            Button::new(id).ghost().xsmall().selected(selected).label(label).disabled(busy || !self.chat_online)
                .when(tier == "priority", |b| b.icon(IconName::Zap))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if selected { return; }
                    this.apply_ctl(Ctl::Fast, vec!["service-tier"], json!({"service_tier": tier}), tier.into(), cx);
                }))
        };
        Some(div().flex().items_center().gap(px(2.)).p(px(2.)).rounded(px(8.)).bg(theme::inset())
            .child(option("picker-speed-default", tr("picker_speed_standard"), !on, "default", cx))
            .child(option("picker-speed-fast", tr("ctl_fast"), on, "priority", cx))
            .into_any_element())
    }

    /// O painel do seletor: trilho, busca e lista, raciocínio ao lado, velocidade embaixo.
    pub(super) fn render_model_picker(&self, catalog: Option<&Result<Value, String>>, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let searching = !self.ctl_search.read(cx).value().trim().is_empty();
        let (own, handoff) = match catalog { Some(Ok(catalog)) => (self.choices(Ctl::Model, catalog), self.handoff_rows()), _ => (Vec::new(), Vec::new()) };
        let tab = self.picker_tab(&own, &handoff);
        let rows = match catalog { Some(Ok(catalog)) => self.picker_rows(catalog, cx), _ => Vec::new() };
        let pick = self.ctl_highlight(&rows);
        if !rows.is_empty() && !self.controls.revealed.get() {
            if self.controls.scroll.bounds().size.height > px(0.) {
                self.controls.revealed.set(true);
                self.controls.scroll.scroll_to_item(pick);
            } else { window.request_animation_frame(); }
        }
        let handoff_tab = !searching && tab == "codex" && self.provider().0 != "codex";
        let handoff_state = self.handoff_catalog().map(|h| (&h.catalog, h.account.label.clone()));
        let list = match catalog {
            None => popup::skeleton("ctl-loading", 5).into_any_element(),
            Some(Err(reason)) => div().p(px(8.)).flex().flex_col().items_start().gap(px(6.)).text_size(px(12.)).text_color(theme::danger())
                .child(tr("ctl_failed").replace("{reason}", reason))
                .child(Button::new("ctl-retry").outline().xsmall().label(tr("ctl_retry")).on_click(cx.listener(move |this, _, window, cx| {
                    this.close_controls();
                    this.open_ctl(Ctl::Model, this.provider().0 == "claude", cx);
                    this.focus_ctl_panel(window, cx);
                })))
                .into_any_element(),
            Some(Ok(_)) if handoff_tab && !matches!(handoff_state, Some((Some(Ok(_)), _))) => match handoff_state {
                Some((Some(Err(reason)), _)) => div().p(px(8.)).text_size(px(12.)).text_color(theme::danger())
                    .child(tr("ctl_failed").replace("{reason}", reason)).into_any_element(),
                _ => popup::skeleton("picker-handoff-loading", 4).into_any_element(),
            },
            Some(Ok(_)) if rows.is_empty() => div().px(px(8.)).py(px(24.)).text_size(px(12.)).text_color(theme::muted()).text_center()
                .child(tr(if searching { "ctl_no_results" } else if tab == FAVORITES { "picker_no_favorites" } else { "ctl_empty" })).into_any_element(),
            Some(Ok(_)) => div().id("ctl-list").flex_1().max_h(px(320.)).overflow_y_scroll().track_scroll(&self.controls.scroll).flex().flex_col()
                .children(rows.iter().cloned().enumerate().map(|(n, c)| self.picker_row(n, c, n == pick, busy, cx)))
                .into_any_element(),
        };
        let hint = (handoff_tab || rows.get(pick).is_some_and(|c| matches!(c.pick, Pick::Handoff { .. })))
            .then(|| handoff_state.as_ref().map(|(_, label)| label.clone())).flatten()
            .map(|label| div().px(px(8.)).pb(px(4.)).text_xs().text_color(theme::muted()).whitespace_normal()
                .child(tr("picker_handoff_hint").replace("{account}", &label)));
        let search = div().px(px(4.)).pb(px(4.))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_ctl(-1, false, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_ctl(1, false, cx)))
            .child(Input::new(&self.ctl_search).h(px(32.)).aria_label(tr("picker_search"))
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint())));
        let center = div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).px(px(4.)).child(search).children(hint).child(list);
        let reasoning = match catalog { Some(Ok(catalog)) => Some(self.picker_reasoning(catalog, &rows, busy, cx)), _ => None };
        let (speed, context) = match catalog {
            Some(Ok(catalog)) => (self.picker_speed(catalog, busy, cx), self.render_context_footer(catalog, busy, cx)),
            _ => (None, None),
        };
        let refresh = (self.provider().0 == "claude" && !handoff_tab).then(|| Button::new("ctl-model-refresh").ghost().xsmall().label(tr("voice_models_refresh")).disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_ctl(Ctl::Model, true, cx);
                this.focus_ctl_panel(window, cx);
            })));
        let footer = (speed.is_some() || refresh.is_some()).then(|| div().mt(px(4.)).pt(px(6.)).px(px(4.)).flex().items_center().gap_2()
            .border_t_1().border_color(theme::border())
            .children(speed).child(div().flex_1()).children(refresh));
        div().flex().flex_col()
            .child(div().flex().min_h(px(260.))
                .when(!searching, |el| el.child(self.picker_rail(&own, &handoff, cx)))
                .child(center)
                .children(reasoning))
            .children(footer)
            .children(context)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob traz o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn row(key: &str, current: bool) -> Choice {
        Choice { label: key.into(), detail: String::new(), path: Vec::new(), body: json!({"model": key}), current, enabled: true,
            provider: "codex".into(), key: format!("codex:{key}"), pick: Pick::Apply }
    }

    #[test]
    fn model_names_read_like_the_t3_divider() {
        assert_eq!(model_display("claude", "claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_display("claude", "opus"), "Opus");
        assert_eq!(model_display("claude", "claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_display("claude", "claude-opus-5-5[1m]"), "Opus 5.5 1M");
        assert_eq!(model_display("codex", "gpt-6.2-sol"), "GPT-6.2-Sol");
        assert_eq!(model_display("codex", ""), "Codex");
    }

    #[test]
    fn long_tab_shows_six_rows_the_current_one_and_show_more() {
        let rows: Vec<Choice> = (0..10).map(|n| row(&format!("m{n}"), n == 8)).collect();
        let short = shortened(rows.clone(), false);
        let keys: Vec<&str> = short.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(&keys[..7], ["m0", "m1", "m2", "m3", "m4", "m5", "m8"]);
        assert_eq!(short.last().map(|c| c.pick.clone()), Some(Pick::More));
        assert_eq!(shortened(rows.clone(), true).len(), 10, "expandida mostra tudo");
        assert_eq!(shortened(rows[..7].to_vec(), false).len(), 7, "sete linhas cabem sem o corte");
    }

    #[test]
    fn favorites_come_first_keeping_the_agent_order() {
        let mut rows: Vec<Choice> = ["a", "b", "c", "d"].iter().map(|k| row(k, false)).collect();
        favorites_first(&mut rows, &["codex:d".into(), "codex:b".into()]);
        let keys: Vec<&str> = rows.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(keys, ["b", "d", "a", "c"]);
    }
}
