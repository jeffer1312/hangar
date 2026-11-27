//! Seletor de modelos no estilo do T3 Code, no lugar do menu simples de modelo: trilho com os favoritos, o agente
//! da sessão e o Codex que pode receber a conversa; busca sempre à vista; estrela de favorito e Ctrl+1…9 por linha;
//! "Mostrar mais" quando a lista é longa; ao lado, o raciocínio do modelo em foco; embaixo, a velocidade.
//! Escolher um modelo do Codex numa sessão Claude pede a troca de agente (`POST /conta` com `when_idle`): com a
//! sessão trabalhando, o servidor espera o turno acabar, e o que for enviado no meio vai ao modelo novo.
use super::*;
use hangar_api::session::HandoffState;

pub(super) const FAVORITES: &str = "favorites";
/// Linhas da aba antes do "Mostrar mais".
const SHORT: usize = 6;
/// Largura do trilho e da coluna de raciocínio.
const RAIL: f32 = 50.;
const REASONING: f32 = 240.;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Pick { Apply, Handoff { credential: String, model: String, effort: Option<String> }, More }

/// Conta Codex que recebe a conversa na troca: `credential` vai no pedido, `account` lê o catálogo.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct HandoffAccount { pub(in crate::app) credential: String, pub(in crate::app) account: String, pub(in crate::app) label: String }

/// Catálogo do Codex lido para a troca, da sessão e da conta em que foi pedido; `read` separa a leitura desta
/// abertura da de uma abertura anterior que ainda volta.
pub(in crate::app) struct HandoffCatalog { key: SessionKey, account: HandoffAccount, read: u64, catalog: Option<Result<Value, String>> }

fn provider_label(provider: &str) -> String { super::super::orchestration::provider_name(provider).to_owned() }

/// Nome curto do modelo para o divisor e a faixa da troca: `claude-opus-5-5` vira "Opus 5.5", `gpt-6.2-sol` vira
/// "GPT-6.2-Sol". Sem modelo, o nome do agente.
pub(crate) fn model_display(provider: &str, model: &str) -> String {
    let model = model.trim();
    if model.is_empty() { return provider_label(provider); }
    // O Claude já tem o rótulo da linha de status; o resto ganha maiúscula por palavra, com "GPT" inteiro.
    if provider == "claude" { return crate::status::model_label(model); }
    let (base, wide) = match model.strip_suffix("[1m]") { Some(base) => (base, true), None => (model, false) };
    let shown = base.split('-').map(|p| if p.eq_ignore_ascii_case("gpt") { "GPT".to_owned() } else { capitalized(p) }).collect::<Vec<_>>().join("-");
    if wide { format!("{shown} 1M") } else { shown }
}

/// Atalho da linha `n` (1…9) no jeito da plataforma.
fn jump_label(n: usize) -> String { if cfg!(target_os = "macos") { format!("⌘{n}") } else { format!("Ctrl+{n}") } }

/// A tecla do atalho como o `Kbd` do T3: fundo leve, sem borda, na fonte do texto.
fn jump_key(n: usize) -> Div {
    div().flex_shrink_0().h(px(20.)).min_w(px(20.)).px(px(4.)).flex().items_center().justify_center().rounded(px(4.))
        .bg(theme::text().opacity(0.07)).text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(jump_label(n))
}

/// O que um desenho do seletor monta uma vez só.
#[derive(Default)]
struct PickerView { own: Vec<Choice>, handoff: Vec<Choice>, tab: String, has_favorites: bool }

/// Linha de modelo: agente, nome, visto da atual, estrela e atalho. A tela e a animação da novidade usam a mesma.
pub(crate) fn row_view(id: SharedString, provider: &str, name: AnyElement, current: bool, lit: bool, star: Option<AnyElement>, jump: Option<usize>) -> Button {
    let more = provider.is_empty();
    // Como no T3: a atual só leva o visto, sem fundo; o fundo é da linha em foco. Visto colado no nome, estrela e atalho
    // na ponta.
    popup::row(id, false).aria_selected(current).py(px(8.))
        .when(lit, |el| el.bg(theme::text().opacity(0.09)))
        .child(div().w_full().flex().items_center().gap(px(10.))
            .when(!more, |el| el.child(chrome::provider_glyph(provider, 18.)))
            .child(div().flex_1().min_w_0().flex().items_center().gap(px(6.)).child(div().min_w_0().child(name))
                .when(current, |el| el.child(chrome::small_icon(IconName::Check, 15., theme::muted()))))
            .children(star)
            .children(jump.map(jump_key)))
}

/// A estrela de favorito, acesa ou não.
pub(crate) fn star_icon(favorite: bool) -> Icon { chrome::small_icon(IconName::Star, 13., if favorite { theme::warning() } else { theme::faint() }) }

/// Aba do trilho com a barra de destaque quando está à vista.
pub(crate) fn rail_tab(button: Button, selected: bool) -> Div {
    div().relative().child(button)
        .when(selected, |el| el.child(div().absolute().right(px(-5.)).top(px(9.)).w(px(3.)).h(px(20.)).rounded_full().bg(theme::accent())))
}

pub(crate) fn rail_button(id: &str, selected: bool, label: String, child: AnyElement) -> Button {
    Button::new(SharedString::from(format!("picker-tab-{id}"))).ghost().w(px(38.)).h(px(38.)).rounded(px(9.)).selected(selected)
        .tooltip(label.clone()).accessibility_label(label).child(child)
}

pub(crate) fn rail_view(tabs: Vec<AnyElement>) -> Div {
    div().w(px(RAIL)).flex_none().flex().flex_col().items_center().gap(px(6.)).py(px(6.)).border_r_1().border_color(theme::border()).children(tabs)
}

/// Nível de raciocínio na coluna ao lado da lista.
pub(crate) fn effort_row(label: &str, on: bool) -> Button {
    popup::row(SharedString::from(format!("picker-effort-{label}")), on).py(px(8.))
        .child(div().w_full().flex().items_center().gap_2()
            .child(div().flex_1().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::NORMAL).child(effort_label(label)))
            .when(on, |el| el.child(chrome::small_icon(IconName::Check, 15., theme::muted()))))
}

pub(crate) fn reasoning_view(header: String, rows: Vec<AnyElement>) -> Div {
    let empty = rows.is_empty();
    div().w(px(REASONING)).flex_none().flex().flex_col().gap(px(2.)).pl(px(6.)).border_l_1().border_color(theme::border())
        // Cabeçalho do T3: "Raciocínio" à esquerda, o modelo em foco à direita, com a linha embaixo.
        .child(div().mx(px(4.)).px(px(4.)).pt(px(8.)).pb(px(8.)).mb(px(4.)).flex().items_center().gap_2().border_b_1().border_color(theme::border())
            .child(div().flex_none().text_size(px(15.)).font_weight(FontWeight::NORMAL).text_color(theme::muted()).child(tr("new_chat_reasoning")))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(theme::faint()).text_right().child(header)))
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
        // Cada abertura relê as contas e o catálogo do Codex: uma falha não fica presa, e a conta de mais folga é a de agora.
        self.controls.handoff = None;
        // Leitura que falhou não marca carregado: a próxima abertura relê, e nenhuma estrela grava por cima.
        if self.controls.favorites.is_none() { self.controls.favorites = crate::appearance::favorite_models(); }
        self.controls.providers.reset();
        if self.provider().0 == "claude" { self.load_session_engines(cx); }
        if self.handoff_source_now().is_some() { self.ensure_session_accounts(cx); }
    }

    /// Lê o catálogo do Codex da conta de mais folga quando ele falta: as contas chegam depois da abertura, e quem as
    /// espera é o desenho do seletor.
    pub(super) fn picker_poll(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        // A abertura relê as contas: até a leitura chegar, a lista antiga pode trazer uma conta que ficou cheia.
        if self.session_accounts().loading { return; }
        let accounts = self.handoff_accounts();
        if let Some(chosen) = self.controls.handoff.as_ref().filter(|h| h.key == key) {
            // Catálogo já pedido nesta sessão vale enquanto a conta dele seguir apta; a que saiu da lista leva o catálogo junto.
            if accounts.as_ref().is_none_or(|list| list.iter().any(|a| a.credential == chosen.account.credential)) { return; }
            self.controls.handoff = None;
        }
        if self.handoff_source_now().is_none() { return; }
        let Some(account) = accounts.and_then(|list| list.into_iter().next()) else { return };
        let Some(api) = self.session_api() else { return };
        static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let read = READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let codex_account = account.account.clone();
        self.controls.handoff = Some(HandoffCatalog { key: key.clone(), account, read, catalog: None });
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.server_read(&["model-options"], &[("provider", "codex"), ("codex_account", codex_account.as_str())], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::HandoffCatalog(read), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_handoff_catalog(&mut self, key: SessionKey, read: u64, result: Result<Value, Failure>) {
        let Some(handoff) = self.controls.handoff.as_mut().filter(|h| h.key == key && h.read == read) else { return };
        handoff.catalog = Some(result.map_err(|error| Self::failure(&error)));
    }

    /// A resposta é da leitura do catálogo do Codex que o seletor ainda espera: uma trocada é descartada.
    pub(in crate::app) fn handoff_read_current(&self, key: &SessionKey, read: u64) -> bool {
        self.controls.handoff.as_ref().is_some_and(|h| h.key == *key && h.read == read)
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
            // Leitura das contas que falhou deixa a aba aberta: é nela que a falha aparece.
            let usable = self.handoff_accounts().is_some_and(|list| !list.is_empty()) || self.session_accounts_notice().is_some();
            tabs.push(("codex".into(), usable));
        }
        tabs
    }

    /// O que um desenho do seletor precisa, montado uma vez: as linhas do agente e do Codex, a aba e se há favoritos.
    fn picker_view(&self, catalog: &Value) -> PickerView {
        let (own, handoff) = (self.choices(Ctl::Model, catalog), self.handoff_rows());
        let favorites = self.favorite_list();
        let has_favorites = own.iter().chain(&handoff).any(|c| favorites.contains(&c.key));
        // A aba à vista: a escolhida, ou os favoritos quando algum está na lista, ou o agente da sessão.
        // Sem favorito sobrando, a aba de Favoritos sai do trilho e a lista volta ao agente da sessão.
        // A aba do Codex sem conta apta (a última saiu da lista) ou sem troca possível volta ao agente da sessão: o
        // esqueleto fica só para a leitura a caminho.
        let codex_gone = self.provider().0 != "codex"
            && (self.handoff_source_now().is_none()
                || (self.handoff_accounts().is_some_and(|list| list.is_empty()) && self.session_accounts_notice().is_none()));
        let tab = self.controls.picker_tab.clone().filter(|tab| tab != FAVORITES || has_favorites).filter(|tab| tab != "codex" || !codex_gone)
            .unwrap_or_else(|| if has_favorites { FAVORITES.into() } else { self.provider().0.to_owned() });
        PickerView { own, handoff, tab, has_favorites }
    }

    /// As linhas do seletor: busca em todos os agentes; senão a aba, favoritos primeiro, e o "Mostrar mais".
    pub(super) fn picker_rows(&self, catalog: &Value, cx: &App) -> Vec<Choice> { self.picker_rows_of(self.picker_view(catalog), cx) }

    fn picker_rows_of(&self, view: PickerView, cx: &App) -> Vec<Choice> {
        let PickerView { own, handoff, tab, .. } = view;
        let query = self.ctl_search.read(cx).value().trim().to_owned();
        let favorites = self.favorite_list();
        if !query.is_empty() {
            keep(own.into_iter().chain(handoff).collect(), &query)
        } else if tab == FAVORITES {
            let mut rows: Vec<Choice> = own.into_iter().chain(handoff).filter(|c| favorites.contains(&c.key)).collect();
            rows.sort_by_key(|c| favorites.iter().position(|f| *f == c.key));
            rows
        } else {
            let mut rows = if tab == "codex" && self.provider().0 != "codex" { handoff } else { own };
            favorites_first(&mut rows, favorites);
            shortened(rows, self.controls.picker_more)
        }
    }

    /// Ctrl+N: a linha `n` da lista à vista.
    pub(super) fn picker_jump(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, choices, _)) = self.open_choices(cx) else { return };
        if choices.get(n - 1).is_none_or(|c| c.pick == Pick::More) { return; }
        self.pick_ctl(Some(n - 1), window, cx);
    }

    fn toggle_favorite(&mut self, key: String, cx: &mut Context<Self>) {
        // Favoritos que não leram: gravar a lista desta estrela apagaria os do disco.
        if self.controls.favorites.is_none() { self.controls.favorites = crate::appearance::favorite_models(); }
        if self.controls.favorites.is_none() { return; }
        // A aba à vista fica: a primeira estrela não leva a pessoa para os Favoritos.
        if self.controls.picker_tab.is_none() {
            let tab = self.controls.open.as_ref().and_then(|o| o.catalog.as_ref()).and_then(|c| c.as_ref().ok())
                .map(|catalog| self.picker_view(catalog).tab);
            self.controls.picker_tab = tab;
        }
        let Some(list) = self.controls.favorites.as_mut() else { return };
        match list.iter().position(|f| *f == key) { Some(at) => { list.remove(at); } None => list.push(key) }
        let saved = list.clone();
        // Estrelas rápidas gravam em ordem: só a lista mais nova chega ao disco.
        static LATEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let version = LATEST.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.runtime.spawn_blocking(move || {
            let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            if LATEST.load(std::sync::atomic::Ordering::SeqCst) == version { crate::appearance::save_favorite_models(&saved); }
        });
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
            let result = api.act(&key.name, &["conta"], None, true, 15).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::HandoffCancel, result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_handoff_cancel(&mut self, key: SessionKey, result: Result<Value, Failure>) {
        if let Err(error) = result { self.action_feedback.insert(key, (Self::failure(&error), true)); }
    }

    /// Faixa sobre o compositor enquanto a troca espera, acontece ou acabou de falhar.
    pub(in crate::app) fn render_handoff_band(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.selected.as_ref()?;
        let handoff = session.handoff.as_ref()?;
        let live = self.status().and_then(|s| s.model);
        let from = model_display(&session.provider, session.model.as_deref().or(live.as_deref()).unwrap_or_default());
        let to = model_display(&handoff.provider, handoff.model.as_deref().unwrap_or_default());
        let route = route_view(&session.provider, from, &handoff.provider, to, 14.);
        let (title, color) = match handoff.state {
            HandoffState::Running => (tr_shared("handoff_running", &[]), theme::muted()),
            HandoffState::Failed => {
                let reason = handoff.code.as_deref().map(|code| crate::i18n::tr_web(code, &HashMap::new()).unwrap_or_else(|| code.to_owned())).unwrap_or_default();
                (tr_shared("handoff_failed", &[("reason", &reason)]), theme::warning())
            }
            HandoffState::Waiting => (tr_shared(if session.state == "working" { "handoff_waiting_turn" } else { "handoff_waiting" }, &[]), theme::muted()),
        };
        let waiting = handoff.state == HandoffState::Waiting;
        let icon = match handoff.state { HandoffState::Waiting => IconName::Hourglass, HandoffState::Failed => IconName::TriangleAlert, HandoffState::Running => IconName::LoaderCircle };
        let cancel = waiting.then(|| Button::new("handoff-cancel").small().ghost().label(tr_shared("handoff_cancel", &[]))
            .on_click(cx.listener(|this, _, _, cx| this.cancel_handoff(cx))));
        Some(band_view(icon, title, color, route, cancel).id("handoff-band").into_any_element())
    }

    /// Divisor da conversa onde o agente mudou: "Troca de contexto · Opus 5.5 → GPT-6.2-Sol".
    pub(in crate::app) fn render_handoff_line(&self, event_index: usize) -> AnyElement {
        let Some(handoff) = self.chat.events.get(event_index).and_then(|event| event.handoff.as_ref()) else { return div().into_any_element() };
        let (from, to) = (handoff.from_provider.as_deref().unwrap_or_default(), handoff.to_provider.as_deref().unwrap_or_default());
        let route = route_view(from, model_display(from, handoff.from_model.as_deref().unwrap_or_default()),
            to, model_display(to, handoff.to_model.as_deref().unwrap_or_default()), 12.5);
        divider_view(route).into_any_element()
    }

    fn picker_row(&self, n: usize, c: Choice, lit: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let (current, enabled, more) = (c.current, c.enabled, c.pick == Pick::More);
        let id = SharedString::from(format!("ctl-choice-{}", if more { "more" } else { c.key.as_str() }));
        // O clique leva a chave da linha desenhada: a lista pode ter mudado (favorito chegando muda a aba) até ele.
        let picked = c.key.clone();
        let favorite = !c.key.is_empty() && self.favorite_list().contains(&c.key);
        // Como no T3: a estrela só aparece acesa ou na linha em foco.
        let star = (!c.key.is_empty() && (favorite || lit)).then(|| {
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
        let name = div().id(SharedString::from(format!("picker-name-{}", c.key))).min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::NORMAL)
            .text_color(if more { theme::muted() } else { theme::text() }).child(c.label)
            .when(!detail.is_empty(), |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(detail.clone()).build(window, cx)));
        let provider = if more { String::new() } else { c.provider.clone() };
        row_view(id, &provider, name.into_any_element(), current, lit, star, (!more && n < 9).then_some(n + 1))
            .disabled(busy || !enabled)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && enabled && this.controls.highlight != Some(n) { this.controls.highlight = Some(n); cx.notify(); }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.pick_picker_row(&picked, window, cx)))
            .into_any_element()
    }

    /// A linha desenhada com a chave `key` (vazia = o "Mostrar mais"), procurada na lista de agora; sumida, nada.
    fn pick_picker_row(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, choices, _)) = self.open_choices(cx) else { return };
        let found = choices.iter().position(|c| if key.is_empty() { c.pick == Pick::More } else { c.key == key });
        if let Some(n) = found { self.pick_ctl(Some(n), window, cx); }
    }

    fn picker_rail(&self, view: &PickerView, cx: &mut Context<Self>) -> Div {
        let tab = view.tab.clone();
        let has_favorites = view.has_favorites;
        let button = |id: String, selected: bool, enabled: bool, label: String, child: AnyElement, cx: &mut Context<Self>| {
            let tab = id.clone();
            let button = rail_button(&id, selected, label, child).disabled(!enabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.controls.picker_tab.as_deref() == Some(super::providers::TAB) {
                        this.controls.providers.reset();
                        this.ctl_search.update(cx, |input, cx| input.set_value("", window, cx));
                    }
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
        use super::providers::TAB;
        if self.provider_picker_available() {
            let active = tab == TAB;
            let button = super::providers::tab_button(active)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.controls.picker_tab.as_deref() != Some(TAB) {
                        this.controls.picker_tab = Some(TAB.into());
                        this.open_provider_picker(window, cx);
                    }
                    cx.notify();
                }));
            items.push(rail_tab(button, active).into_any_element());
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
        // Nível de outro modelo troca o modelo junto (Claude e Codex): a resposta precisa refazer o catálogo de modelos.
        let ctl = if focus.is_some_and(|f| !f.current) { Ctl::Model } else { Ctl::Effort };
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
                    let model = focus.body.get("model").and_then(Value::as_str).unwrap_or_default();
                    levels.extend(codex_efforts(catalog, model).into_iter().map(|c| (c.label, c.current, Pick::Apply, c.path, c.body)));
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
                        _ => { this.apply_ctl(ctl, path.clone(), body.clone(), label.clone(), cx); }
                    }
                }))
                .into_any_element()
        }).collect::<Vec<_>>();
        reasoning_view(header, list)
    }

    /// O acesso da sessão no canto do rodapé, como o "Full access" do T3: a permissão do Codex ou o modo do Claude.
    /// Abre o painel dele no lugar do seletor.
    fn picker_access(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.ctl_list();
        let ctl = [Ctl::Permission, Ctl::Mode].into_iter().find(|c| list.contains(c))?;
        let value = self.ctl_label(ctl)?;
        let shown = match (self.provider().0, ctl) {
            ("claude", Ctl::Mode) => tr(&format!("mode_{value}")),
            ("codex", Ctl::Mode) => tr(&format!("codex_mode_{value}")),
            ("antigravity" | "kimi", Ctl::Mode) => shown(self.provider().0, Ctl::Mode, value),
            _ => value,
        };
        Some(Button::new("picker-access").ghost().small().disabled(!self.chat_online)
            .accessibility_label(format!("{}: {shown}", tr(&format!("ctl_{}", ctl.key()))))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(13.5)).text_color(theme::muted())
                .child(chrome::small_icon(IconName::KeyRound, 14., theme::faint())).child(shown)
                .child(chrome::small_icon(IconName::ChevronUp, 13., theme::faint())))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_ctl(ctl, false, cx);
                this.focus_ctl_panel(window, cx);
            }))
            .into_any_element())
    }

    /// Velocidade: Normal, Fast e Ultra Fast, cada uma liberada só quando o modelo atual a aceita.
    fn picker_speed(&self, catalog: &Value, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let speed = match self.provider().0 {
            "codex" => codex_fast_state(catalog, self.ctl_label(Ctl::Fast).as_deref()),
            "claude" => {
                let (Some(key), Some(session)) = (self.selected_key(), self.selected.as_ref()) else { return None; };
                self.controls.model_catalog(&key, session)?;
                if !catalog_engine_matches(catalog, session.engine.as_deref()) { return None; }
                claude_fast_state(catalog)?
            }
            _ => return None,
        };
        let (labels, hints) = speed_texts();
        let hint = if !speed.available() { tr("ctl_fast_unavailable") } else if speed.ultrafast || speed.tier == Some("ultrafast") {
            hints[speed.selected()].clone()
        } else { format!("{} {}", hints[speed.selected()], tr("ctl_speed_ultrafast_unavailable")) };
        let locked = busy || !self.chat_online;
        let enabled = if locked { [false; 3] } else { speed.enabled() };
        let off_note = tr(if speed.fast { "ctl_speed_ultrafast_unavailable" } else { "ctl_fast_unavailable" });
        // Escolha que não pode mudar agora continua à vista, apagada; desconhecida não mostra nenhuma.
        Some(div().min_w_0().max_w(px(320.)).flex().flex_col().gap_1()
            .child(crate::app::settings::segments_where("ctl-speed", &labels, &hints, speed.selected(), &enabled,
                (locked || !speed.available()) && speed.tier.is_some(), off_note, |this, n, _, cx| {
                    let tier = SPEED_TIERS[n];
                    this.apply_ctl(Ctl::Fast, vec!["service-tier"], json!({"service_tier": tier}), tier.into(), cx);
                }, cx))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(hint))
            .into_any_element())
    }

    /// O painel do seletor: trilho, busca e lista, raciocínio ao lado, velocidade embaixo.
    pub(super) fn render_model_picker(&self, catalog: Option<&Result<Value, String>>, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let searching = !self.ctl_search.read(cx).value().trim().is_empty();
        // O catálogo do agente que falhou ou não chegou afeta só as linhas dele: as do Codex da troca seguem.
        let none = Value::Null;
        let own = match catalog { Some(Ok(catalog)) => catalog, _ => &none };
        let view = self.picker_view(own);
        let tab = view.tab.clone();
        if tab == super::providers::TAB {
            let rail = self.picker_rail(&view, cx);
            return div().flex().child(rail).child(self.render_claude_provider_picker(busy, cx)).into_any_element();
        }
        let rail = (!searching).then(|| self.picker_rail(&view, cx));
        let rows = self.picker_rows_of(view, cx);
        let pick = self.ctl_highlight(&rows);
        if !rows.is_empty() && !self.controls.revealed.get() {
            if self.controls.scroll.bounds().size.height > px(0.) {
                self.controls.revealed.set(true);
                self.controls.scroll.scroll_to_item(pick);
            } else { window.request_animation_frame(); }
        }
        let handoff_tab = !searching && tab == "codex" && self.provider().0 != "codex";
        let handoff_state = self.handoff_catalog().map(|h| (&h.catalog, h.account.label.clone()));
        let focus_handoff = rows.get(pick).is_some_and(|c| matches!(c.pick, Pick::Handoff { .. }));
        let hint = (handoff_tab || focus_handoff)
            .then(|| handoff_state.as_ref().map(|(_, label)| label.clone())).flatten()
            .map(|label| div().px(px(8.)).pb(px(4.)).text_xs().text_color(theme::muted()).whitespace_normal()
                .child(tr("picker_handoff_hint").replace("{account}", &label)));
        // A releitura das contas que falhou deixa a lista anterior valendo para a troca; o aviso diz que ela pode estar velha.
        let accounts_problem = (handoff_tab || focus_handoff).then(|| self.session_accounts_notice()).flatten()
            .map(|reason| div().px(px(8.)).pb(px(4.)).text_xs().text_color(theme::danger()).whitespace_normal()
                .child(tr("accounts_refresh_failed").replace("{reason}", reason)));
        // Linha de troca em foco com o catálogo do Codex lido: os níveis dela aparecem mesmo com o do agente em erro.
        let handoff_levels = focus_handoff && matches!(handoff_state, Some((Some(Ok(_)), _)));
        let reasoning = (matches!(catalog, Some(Ok(_))) || handoff_tab || handoff_levels).then(|| self.picker_reasoning(own, &rows, busy, cx));
        // Carga ou erro do catálogo do agente: acima das linhas que chegaram, nunca no lugar delas; a lista à vista é a
        // mesma que o teclado e o clique percorrem.
        let own_state = (!handoff_tab && !matches!(catalog, Some(Ok(_)))).then(|| self.ctl_load_state(Ctl::Model, catalog, cx)).flatten();
        let list = match catalog {
            _ if handoff_tab && !matches!(handoff_state, Some((Some(Ok(_)), _))) => match handoff_state {
                Some((Some(Err(reason)), _)) => div().p(px(8.)).text_size(px(12.)).text_color(theme::danger())
                    .child(tr("ctl_failed").replace("{reason}", reason)).into_any_element(),
                // Leitura das contas que falhou, sem nada a caminho: o aviso em vermelho acima diz o motivo, e reabrir
                // o seletor relê.
                None if !self.session_accounts().loading && self.session_accounts_notice().is_some() => div().into_any_element(),
                _ => popup::skeleton("picker-handoff-loading", 4).into_any_element(),
            },
            _ if rows.is_empty() && own_state.is_some() => div().into_any_element(),
            _ if rows.is_empty() => div().px(px(8.)).py(px(24.)).text_size(px(12.)).text_color(theme::muted()).text_center()
                .child(tr(if searching { "ctl_no_results" } else if tab == FAVORITES { "picker_no_favorites" } else { "ctl_empty" })).into_any_element(),
            _ => div().id("ctl-list").flex_1().max_h(px(320.)).overflow_y_scroll().track_scroll(&self.controls.scroll).flex().flex_col()
                .children(rows.into_iter().enumerate().map(|(n, c)| self.picker_row(n, c, n == pick, busy, cx)))
                .into_any_element(),
        };
        // Busca do T3: sem caixa, com um traço fino na cor de destaque.
        let search = div().mx(px(4.)).mb(px(6.)).border_b_1().border_color(theme::accent())
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_ctl(-1, false, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_ctl(1, false, cx)))
            .child(Input::new(&self.ctl_search).appearance(false).h(px(40.)).text_size(px(16.)).aria_label(tr("picker_search"))
                .prefix(chrome::small_icon(IconName::Search, 16., theme::faint())));
        let center = div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).px(px(4.)).child(search).children(hint).children(accounts_problem).children(own_state).child(list);
        let (speed, context) = match catalog {
            Some(Ok(catalog)) => (self.picker_speed(catalog, busy, cx), self.render_context_footer(catalog, busy, cx)),
            _ => (None, None),
        };
        let refresh = (self.provider().0 == "claude" && !handoff_tab).then(|| Button::new("ctl-model-refresh").ghost().xsmall()
            .icon(IconName::RefreshCw).tooltip(tr("voice_models_refresh")).accessibility_label(tr("voice_models_refresh")).disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_ctl(Ctl::Model, true, cx);
                this.focus_ctl_panel(window, cx);
            })));
        let access = self.picker_access(cx);
        // Rodapé do T3: velocidade à esquerda, o acesso (permissão) à direita.
        let footer = (speed.is_some() || refresh.is_some() || access.is_some()).then(|| div().mt(px(4.)).pt(px(8.)).px(px(4.)).flex().items_center().gap_2()
            .border_t_1().border_color(theme::border())
            .children(speed).children(refresh).child(div().flex_1()).children(access));
        div().flex().flex_col()
            .child(div().flex().min_h(px(300.))
                .children(rail)
                .child(center)
                .children(reasoning))
            .children(footer)
            .children(context)
            .when(self.provider().0 == "claude" && self.provider_choices_failed(), |el| {
                el.child(div().px_2().pt_2().flex().items_center().gap_2()
                    .child(div().text_xs().text_color(theme::warning()).child(tr("claude_provider_read_failed")))
                    .child(Button::new("picker-provider-retry").ghost().small().label(tr("retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_session_engines(cx)))))
            })
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
