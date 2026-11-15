//! Avisos da CLI do Claude na conversa: uma linha discreta entre fios, que abre o detalhe, e os avisos que o dono
//! mandou ignorar em cada máquina (`/api/notice-ignores`, evento `notice_ignores` da lista).
use super::*;
use std::collections::BTreeSet;

/// O que a pessoa fez num aviso desta conversa: ele segue na tela (com Desfazer ou o erro) até ela ser recarregada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Acted {
    /// Ignorado aqui, com a chave: voltar a mostrar essa chave desfaz todas as linhas dela.
    Ignored(String),
    Failed(String),
}

/// Evento que o servidor marcou como aviso: o mapa presente, mesmo vazio.
pub(super) fn is_notice_line(event: &ChatEvent) -> bool { event.notice_meta.is_some() }

fn meta<'a>(event: &'a ChatEvent, field: &str) -> Option<&'a str> {
    event.notice_meta.as_ref()?.get(field)?.as_str()
}

/// A chave de ignorar; aviso de bloqueio real não tem.
pub(super) fn notice_key(event: &ChatEvent) -> Option<&str> { meta(event, "key") }

/// O texto do aviso sem o `⚙️` que o servidor põe para o cliente antigo.
fn plain(event: &ChatEvent) -> &str {
    let text = event.text.as_deref().unwrap_or("");
    text.strip_prefix("⚙️").map(str::trim_start).unwrap_or(text)
}

pub(super) fn notice_title(event: &ChatEvent) -> String {
    match meta(event, "subject") {
        Some(hook) => tr_shared("notice_hook_failed_line", &[("name", hook)]),
        None => plain(event).lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned(),
    }
}

/// "12 vezes", só quando o hook falhou mais de uma.
pub(super) fn notice_count(event: &ChatEvent) -> Option<String> {
    let count = event.notice_meta.as_ref()?.get("count")?.as_u64().filter(|count| *count > 1)?;
    Some(tr_shared("notice_count", &[("count", &count.to_string())]))
}

/// O que abre ao clicar: o detalhe do hook, ou o aviso inteiro quando ele não cabe na linha.
pub(super) fn notice_detail(event: &ChatEvent) -> Option<String> {
    let detail = event.hook_error.as_deref().unwrap_or_else(|| plain(event)).trim();
    (!detail.is_empty() && (event.hook_error.is_some() || detail != notice_title(event))).then(|| detail.to_owned())
}

pub(super) fn notice_hidden(event: &ChatEvent, ignored: &BTreeSet<String>) -> bool {
    notice_key(event).is_some_and(|key| ignored.contains(key))
}

/// Como uma chave ignorada aparece na lista das Configurações; a que este app não conhece aparece crua.
pub(super) fn notice_key_label(key: &str) -> String {
    if let Some(hook) = key.strip_prefix("hook:") { return tr_shared("notice_key_hook", &[("name", hook)]); }
    if let Some(name) = key.strip_prefix("note:notification:") { return tr_shared("notice_key_notification", &[("name", name)]); }
    if key.starts_with("note:startup:") { return tr_shared("notice_key_startup", &[]); }
    if key == "note:retry" { return tr_shared("notice_key_retry", &[]); }
    key.to_owned()
}

/// `{"keys":[…]}` da rota e do evento.
pub(super) fn parse_keys(data: &Value) -> Option<BTreeSet<String>> {
    data.get("keys")?.as_array()?.iter().map(|key| key.as_str().map(str::to_owned)).collect()
}

/// Guarda a lista de uma máquina; `Some(true)` quando mudou, `None` quando o evento não é uma lista.
pub(super) fn store(map: &mut HashMap<String, BTreeSet<String>>, server: &str, data: &Value) -> Option<bool> {
    let keys = parse_keys(data)?;
    if map.get(server) == Some(&keys) { return Some(false); }
    map.insert(server.to_owned(), keys);
    Some(true)
}

impl Hangar {
    fn ignored_here(&self) -> Option<&BTreeSet<String>> { self.notice_ignores.get(&self.open_server()) }

    /// Aviso ignorado nesta máquina some da conversa, menos o que a pessoa acabou de ignorar aqui (fica com Desfazer).
    pub(super) fn notice_row_hidden(&self, event: &ChatEvent) -> bool {
        is_notice_line(event) && !self.notice_acted.contains_key(&event.id)
            && self.ignored_here().is_some_and(|ignored| notice_hidden(event, ignored))
    }

    /// A lista de avisos ignorados mudou: as linhas da conversa aberta mudam de altura.
    pub(super) fn notice_ignores_changed(&mut self, server: &str, cx: &mut Context<Self>) {
        if server != self.open_server() { return; }
        self.follow_content_changed(cx);
        self.list_state.remeasure();
        self.redraw(panes::Area::Conversation, cx);
    }

    fn set_notice_ignored(&mut self, event_id: String, key: String, ignored: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.session_api() else { return };
        self.put_notice_ignore(api, self.open_server(), event_id, key, ignored, window, cx);
    }

    /// "Voltar a mostrar" das Configurações, na máquina ativa.
    fn show_notice_again(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.put_notice_ignore(api, self.active_key(), format!("settings:{key}"), key, false, window, cx);
    }

    /// `owner`: quem mostra o resultado, o id do evento na conversa ou `settings:{chave}` na lista das Configurações.
    fn put_notice_ignore(&mut self, api: Api, server: String, event_id: String, key: String, ignored: bool, window: &mut Window, cx: &mut Context<Self>) {
        let body = json!({"key": key, "ignored": ignored});
        run(&self.runtime, async move { api.server_send(reqwest::Method::PUT, &["notice-ignores"], Some(body), 15).await }, window, cx,
            move |this, result, _, cx| {
                match result {
                    Ok(list) => {
                        this.notice_acted.remove(&event_id);
                        if ignored { this.notice_acted.insert(event_id, Acted::Ignored(key)); }
                        // Voltou a mostrar (aqui ou nas Configurações): as linhas "ignorado · Desfazer" dessa chave saem.
                        else { this.notice_acted.retain(|_, acted| *acted != Acted::Ignored(key.clone())); }
                        store(&mut this.notice_ignores, &server, &list);
                    }
                    Err(error) => {
                        let why = format!("{} · {}", tr_shared("notice_ignore_failed", &[]), Hangar::fetch_failure(&error));
                        this.notice_acted.insert(event_id, Acted::Failed(why));
                    }
                }
                this.notice_ignores_changed(&server, cx);
            });
    }

    /// Avisos ignorados da máquina ativa, para as Configurações. `None` em servidor que não manda a lista.
    pub(super) fn render_ignored_notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let keys = self.notice_ignores.get(&self.active_key())?;
        let rows: Vec<Div> = keys.iter().map(|key| {
            let failed = match self.notice_acted.get(&format!("settings:{key}")) { Some(Acted::Failed(why)) => Some(why.clone()), _ => None };
            let put = key.clone();
            div().flex().flex_col().gap(px(2.))
                .child(div().min_h(px(30.)).flex().items_center().gap(px(8.))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(notice_key_label(key)))
                    .child(Button::new(SharedString::from(format!("notice-show-{key}"))).ghost().xsmall().label(tr_shared("notice_show_again", &[]))
                        .on_click(cx.listener(move |this, _, window, cx| this.show_notice_again(put.clone(), window, cx)))))
                .when_some(failed, |el, why| el.child(div().text_xs().text_color(theme::warning()).child(why)))
        }).collect();
        Some(div().pt(px(10.)).flex().flex_col().gap(px(4.))
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr_shared("notice_ignored_title", &[])))
            .map(|el| if rows.is_empty() { el.child(div().text_sm().text_color(theme::faint()).child(tr_shared("notice_ignored_empty", &[]))) }
                else { el.children(rows) })
            .into_any_element())
    }

    pub(super) fn render_notice_line(&self, event_index: usize, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(event) = self.chat.events.get(event_index) else { return div().into_any_element(); };
        let (id, key) = (event.id.clone(), notice_key(event).map(str::to_owned));
        let rule = || div().flex_1().h(px(1.)).bg(theme::border());
        let line = |content: AnyElement| div().w_full().min_h(px(27.)).flex().items_center().gap(px(8.)).child(rule()).child(content).child(rule());
        let small = |text: String, color: Hsla| div().flex_shrink_0().text_size(px(12.5)).text_color(color).child(text);
        // Acabou de ignorar: a linha fica, com o caminho de volta, até a conversa ser recarregada.
        if matches!(self.notice_acted.get(&id), Some(Acted::Ignored(_))) {
            let undo = key.map(|key| {
                let event_id = id.clone();
                Button::new(SharedString::from(format!("notice-undo-{id}"))).ghost().xsmall().label(tr_shared("comum_desfazer", &[]))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_notice_ignored(event_id.clone(), key.clone(), false, window, cx)))
            });
            return line(div().flex().items_center().gap(px(6.)).child(small(tr_shared("notice_ignored_line", &[]), theme::faint())).children(undo)
                .into_any_element()).into_any_element();
        }
        let (title, count, detail) = (notice_title(event), notice_count(event), notice_detail(event));
        let failed = match self.notice_acted.get(&id) { Some(Acted::Failed(why)) => Some(why.clone()), _ => None };
        let expandable = detail.is_some() || key.is_some();
        let open = expandable && self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let head = div().id(SharedString::from(format!("notice-{id}"))).flex().items_center().gap(px(7.)).min_w_0()
            .child(chrome::small_icon(IconName::TriangleAlert, 14., theme::warning()))
            .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(title))
            .when_some(count, |el, count| el.child(small(format!("· {count}"), theme::faint())))
            .when(expandable, |el| el.cursor_pointer()
                .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint()))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx))));
        let body = open.then(|| {
            let ignore = key.map(|key| {
                let event_id = id.clone();
                Button::new(SharedString::from(format!("notice-ignore-{id}"))).ghost().xsmall().label(tr_shared("notice_ignore", &[]))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_notice_ignored(event_id.clone(), key.clone(), true, window, cx)))
            });
            div().w_full().flex().flex_col().items_center().gap(px(7.)).pb(px(6.))
                .when_some(detail, |el, detail| el.child(div().max_w(px(900.)).px(px(12.)).py(px(9.)).rounded(px(8.)).bg(theme::inset())
                    .font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).child(detail)))
                .when_some(failed, |el, why| el.child(small(why, theme::warning())))
                .children(ignore)
        });
        div().w_full().flex().flex_col().child(line(head.into_any_element())).children(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O `super::*` traz o `test` do gpui, que não é o dos testes comuns.
    use core::prelude::v1::test;

    fn notice(text: &str, meta: Value) -> ChatEvent {
        ChatEvent { kind: "assistant_msg".into(), id: "local-1".into(), text: Some(text.into()), notice_meta: meta.as_object().cloned(), ..Default::default() }
    }

    fn hook(detail: &str, meta: Value) -> ChatEvent {
        ChatEvent { kind: "notice".into(), text: Some("hook_failed".into()), hook_error: Some(detail.into()), ..notice("", meta) }
    }

    #[test]
    fn notice_title_for_hook_uses_subject() {
        let event = hook("SessionStart:compact: sem pasta (falhou 12 vezes)", json!({"key":"hook:SessionStart:compact","subject":"SessionStart:compact","count":12}));
        assert_eq!(notice_title(&event), tr_shared("notice_hook_failed_line", &[("name", "SessionStart:compact")]));
        assert_eq!(notice_count(&event), Some(tr_shared("notice_count", &[("count", "12")])));
        assert_eq!(notice_detail(&event).as_deref(), Some("SessionStart:compact: sem pasta (falhou 12 vezes)"));
        assert_eq!(notice_count(&hook("Stop: x", json!({"key":"hook:Stop","subject":"Stop"}))), None, "uma falha só não tem total");
    }

    #[test]
    fn notice_title_for_cli_note_strips_the_gear_and_keeps_the_first_line() {
        let event = notice("⚙️ Servidor MCP que precisa de login: `slack`\nO plugin `x` não carregou: sem pasta", json!({"key":"note:startup:ab12cd34"}));
        assert_eq!(notice_title(&event), "Servidor MCP que precisa de login: `slack`");
        assert_eq!(notice_detail(&event).as_deref(), Some("Servidor MCP que precisa de login: `slack`\nO plugin `x` não carregou: sem pasta"));
        let short = notice("⚙️ O modelo recusou responder", json!({}));
        assert_eq!(notice_title(&short), "O modelo recusou responder");
        assert_eq!(notice_detail(&short), None, "aviso de uma linha não tem o que abrir");
    }

    #[test]
    fn notice_hidden_only_for_ignored_key() {
        let ignored: BTreeSet<String> = ["hook:Stop".to_owned()].into();
        assert!(notice_hidden(&hook("Stop: x", json!({"key":"hook:Stop","subject":"Stop"})), &ignored));
        assert!(!notice_hidden(&hook("Start: x", json!({"key":"hook:Start","subject":"Start"})), &ignored));
    }

    #[test]
    fn notice_without_key_is_never_hidden_and_has_no_ignore() {
        let event = notice("⚙️ Turno parado: a conversa chegou ao limite do contexto", json!({}));
        assert!(is_notice_line(&event) && notice_key(&event).is_none());
        assert!(!notice_hidden(&event, &["hook:Stop".to_owned(), String::new()].into()));
    }

    #[test]
    fn event_without_notice_meta_is_not_a_notice_line() {
        let answer = ChatEvent { kind: "assistant_msg".into(), id: "a1".into(), text: Some("⚙️ resposta comum".into()), ..Default::default() };
        assert!(!is_notice_line(&answer));
        // Aviso de hook do Codex, sem o mapa: segue no desenho antigo.
        let codex = ChatEvent { kind: "notice".into(), text: Some("hook_failed".into()), hook_error: Some("x".into()), ..Default::default() };
        assert!(!is_notice_line(&codex));
    }

    #[test]
    fn notice_key_label_names_each_kind_and_keeps_an_unknown_key() {
        assert_eq!(notice_key_label("hook:Stop"), tr_shared("notice_key_hook", &[("name", "Stop")]));
        assert_eq!(notice_key_label("hook:PostToolUse:Bash"), tr_shared("notice_key_hook", &[("name", "PostToolUse:Bash")]));
        assert_eq!(notice_key_label("note:startup:ab12cd34"), tr_shared("notice_key_startup", &[]));
        assert_eq!(notice_key_label("note:retry"), tr_shared("notice_key_retry", &[]));
        assert_eq!(notice_key_label("note:notification:update"), tr_shared("notice_key_notification", &[("name", "update")]));
        assert_eq!(notice_key_label("note:de-uma-versao-futura"), "note:de-uma-versao-futura");
        assert_ne!(notice_key_label("hook:Stop"), "notice_key_hook", "a chave de texto existe nas mensagens");
    }

    #[test]
    fn notice_ignores_store_reports_change_per_server() {
        let mut map = HashMap::new();
        assert_eq!(store(&mut map, "casa", &json!({"keys":["hook:Stop"]})), Some(true));
        assert_eq!(store(&mut map, "casa", &json!({"keys":["hook:Stop"]})), Some(false));
        assert_eq!(store(&mut map, "vps", &json!({"keys":[]})), Some(true));
        assert_eq!(store(&mut map, "casa", &json!({"machine":null})), None, "evento de outro formato não apaga a lista");
        assert_eq!(map["casa"], ["hook:Stop".to_owned()].into());
    }
}
