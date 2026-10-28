//! Etiquetas de máquina e conta, gravadas no servidor de cada máquina (`/api/tags`): o evento `tags` da lista guardado por
//! máquina para a placa das abas, e o diálogo que edita a etiqueta da máquina ou de uma conta.
use super::*;
use super::device::Remote;

pub(super) const COLORS: [&str; 8] = ["slate", "blue", "green", "lime", "amber", "orange", "pink", "violet"];
pub(super) const MACHINE_ICONS: [&str; 6] = ["laptop", "monitor", "server", "house", "briefcase", "cloud"];
pub(super) const ACCOUNT_ICONS: [&str; 11] = ["credit-card", "provider", "key-round", "wallet", "user-round", "circle-user", "contact",
    "badge-check", "at-sign", "ticket", "id-card"];
const NAME_MAX: usize = 24;

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct Tag { pub(super) name: String, pub(super) icon: String, pub(super) color: Option<String> }

/// Conta como a lista a mostra: o nome já vem do servidor, cortado no `@`.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Badge { pub(super) name: String, pub(super) icon: String, pub(super) color: Option<String> }

/// Evento `tags` da lista do dono; as contas vêm pela `conta` da linha.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(super) struct Event {
    pub(super) machine_id: Option<String>,
    pub(super) machine: Option<Tag>,
    #[serde(default)]
    pub(super) accounts: HashMap<String, Badge>,
}

/// Guarda o evento da máquina `key`: `None` quando o corpo não se lê (fica o anterior), senão se mudou.
pub(super) fn store(map: &mut HashMap<String, Event>, key: &str, data: &Value) -> Option<bool> {
    let event = <Event as serde::Deserialize>::deserialize(data).ok()?;
    if map.get(key) == Some(&event) { return Some(false); }
    map.insert(key.to_owned(), event);
    Some(true)
}

pub(super) fn machine_icon(key: &str) -> IconName {
    match key {
        "laptop" => IconName::Laptop, "server" => IconName::Server, "house" => IconName::House,
        "briefcase" => IconName::Briefcase, "cloud" => IconName::Cloud, _ => IconName::Monitor,
    }
}

/// `provider` não tem ícone: quem desenha usa o logo do agente da conta.
pub(super) fn account_icon(key: &str) -> Option<IconName> {
    Some(match key {
        "provider" => return None,
        "key-round" => IconName::KeyRound, "wallet" => IconName::Wallet, "user-round" => IconName::UserRound,
        "circle-user" => IconName::CircleUser, "contact" => IconName::Contact, "badge-check" => IconName::BadgeCheck,
        "at-sign" => IconName::AtSign, "ticket" => IconName::Ticket, "id-card" => IconName::IdCard,
        _ => IconName::CreditCard,
    })
}

/// Logo do provider da conta; sem logo vetorial, o cartão.
pub(super) fn provider_mark(provider: &str, size: f32, color: Hsla) -> AnyElement {
    chrome::provider_logo(provider, size, color).map(IntoElement::into_any_element)
        .unwrap_or_else(|| chrome::small_icon(IconName::CreditCard, size, color).into_any_element())
}

/// A máquina só entra na placa quando a sessão roda em outra. Com os dois identificadores compara-os; sem eles, vale o
/// endereço de loopback da máquina da sessão.
pub(super) fn shows_machine(session_id: Option<&str>, local_id: Option<&str>, session_loopback: bool) -> bool {
    match (session_id.filter(|id| !id.is_empty()), local_id.filter(|id| !id.is_empty())) {
        (Some(session), Some(local)) => session != local,
        _ => !session_loopback,
    }
}

/// Nome da máquina na placa: o da etiqueta, senão o desta tela, senão o host.
pub(super) fn machine_name(tag: Option<&Tag>, label: &str, address: &str) -> String {
    match tag.map(|t| t.name.trim()).filter(|name| !name.is_empty()) {
        Some(name) => name.to_owned(),
        None if !label.is_empty() => label.to_owned(),
        None => servers::default_label(address),
    }
}

/// O que o diálogo edita: a etiqueta da máquina ou a de uma conta, pelo id de `/api/credenciais`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Subject { Machine, Account { id: String, provider: String } }

/// Ícone de quem ainda não tem etiqueta: o mesmo que a placa mostra sem ela.
fn default_icon(subject: &Subject) -> &'static str { if *subject == Subject::Machine { "monitor" } else { "credit-card" } }

/// O corpo do `PUT /api/tags`. Nome vazio grava a etiqueta sem nome: aí vale o nome local.
fn change(subject: &Subject, name: &str, icon: &str, color: Option<&str>) -> Value {
    match subject {
        Subject::Machine => json!({"machine": {"name": name.trim(), "icon": icon, "color": color}}),
        Subject::Account { id, .. } => json!({"account": {"id": id, "tag": {"icon": icon, "color": color}}}),
    }
}

/// A recusa do servidor em frase: 404 é servidor sem etiquetas, 409 é limite de contas.
fn save_failure(error: &Failure) -> String {
    match (error.status, error.code.as_deref()) {
        (Some(404), _) => tr_shared("tags_old_server", &[]),
        (Some(409), _) | (_, Some("tags_full")) => tr_shared("tags_full", &[]),
        _ => format!("{} · {}", tr_shared("tags_save_failed", &[]), Hangar::fetch_failure(error)),
    }
}

/// O diálogo de uma etiqueta. Entidade própria: o diálogo é desenhado com o `Hangar` em atualização. Lê e grava sozinho,
/// na máquina de `api`; quem mostra a etiqueta nova nas abas é o evento `tags` que o servidor manda na mudança.
pub(super) struct TagEditor {
    api: Api,
    runtime: Arc<Runtime>,
    subject: Subject,
    /// `Ok(None)` = servidor sem a rota.
    doc: Remote<Option<()>>,
    name: Entity<InputState>,
    icon: String,
    color: Option<String>,
    saving: bool,
    save_seq: u64,
    saved: bool,
    error: Option<String>,
    _subscription: Subscription,
}

impl TagEditor {
    fn new(api: Api, runtime: Arc<Runtime>, subject: Subject, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr_shared("tags_name_hint", &[])));
        let subscription = cx.subscribe(&name, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { this.saved = false; cx.notify(); }
        });
        let icon = default_icon(&subject).to_owned();
        let mut editor = Self { api, runtime, subject, doc: Remote::default(), name, icon, color: None, saving: false, save_seq: 0,
            saved: false, error: None, _subscription: subscription };
        editor.load(window, cx);
        editor
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.doc.start();
        let (api, (done, result)) = (self.api.clone(), tokio::sync::oneshot::channel());
        self.runtime.spawn(async move { let _ = done.send(api.server_read(&["tags"], &[], 15).await); });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update_in(cx, |this, window, cx| { this.loaded(seq, result, window, cx); cx.notify(); });
        }).detach();
        cx.notify();
    }

    fn loaded(&mut self, seq: u64, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        if seq != self.doc.seq { return; }
        let value = match result {
            Ok(doc) => {
                let (name, icon, color) = match &self.subject {
                    Subject::Machine => doc.get("machine").and_then(|t| serde_json::from_value::<Tag>(t.clone()).ok())
                        .map(|t| (t.name, Some(t.icon), t.color)).unwrap_or_default(),
                    Subject::Account { id, .. } => doc.get("accounts").and_then(|accounts| accounts.get(id))
                        .map(|t| (String::new(), t.get("icon").and_then(Value::as_str).map(str::to_owned),
                            t.get("color").and_then(Value::as_str).map(str::to_owned))).unwrap_or_default(),
                };
                self.icon = icon.unwrap_or_else(|| default_icon(&self.subject).to_owned());
                self.color = color;
                self.name.update(cx, |input, cx| input.set_value(name, window, cx));
                self.saved = false;
                Ok(Some(()))
            }
            Err(error) if error.status == Some(404) => Ok(None),
            Err(error) => Err(format!("{} · {}", tr_shared("tags_load_failed", &[]), Hangar::fetch_failure(&error))),
        };
        self.doc.finish(seq, value);
    }

    fn pick_icon(&mut self, icon: &str, cx: &mut Context<Self>) { self.icon = icon.to_owned(); self.saved = false; cx.notify(); }
    fn pick_color(&mut self, color: Option<&str>, cx: &mut Context<Self>) { self.color = color.map(str::to_owned); self.saved = false; cx.notify(); }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.doc.ok() != Some(&Some(())) { return; }
        let name: String = self.name.read(cx).value().trim().chars().take(NAME_MAX).collect();
        let body = change(&self.subject, &name, &self.icon, self.color.as_deref());
        self.save_seq += 1;
        let seq = self.save_seq;
        (self.saving, self.saved, self.error) = (true, false, None);
        let (api, (done, result)) = (self.api.clone(), tokio::sync::oneshot::channel());
        self.runtime.spawn(async move { let _ = done.send(api.server_send(reqwest::Method::PUT, &["tags"], Some(body), 15).await); });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update_in(cx, |this, _, cx| {
                if seq != this.save_seq { return; }
                this.saving = false;
                match result {
                    Ok(_) => this.saved = true,
                    // Servidor que perdeu a rota no meio (voltou para uma versão antiga) passa a dizer isso no lugar do editor.
                    Err(error) if error.status == Some(404) => this.doc.set(Ok(None)),
                    Err(error) => this.error = Some(save_failure(&error)),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}

impl Render for TagEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div().id("tag-editor").flex().flex_col().gap(px(14.)).text_size(px(13.));
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);
        let loading = || div().id("tag-editor-loading").role(Role::Status).child(muted(tr("loading")));
        match &self.doc.value {
            None => return body.child(loading()),
            Some(Ok(None)) => return body.child(muted(tr_shared("tags_old_server", &[]))),
            Some(Err(error)) => return body
                .child(div().id("tag-editor-load-error").role(Role::Alert).text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(div().child(Button::new("tag-editor-retry").outline().small().label(tr("retry")).disabled(self.doc.loading)
                    .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))))),
            Some(Ok(Some(()))) if self.doc.loading => return body.child(loading()),
            Some(Ok(Some(()))) => {}
        }
        let title = |key: &str| div().font_weight(FontWeight::MEDIUM).child(tr_shared(key, &[]));
        let swatch_ring = |on: bool| if on { theme::accent() } else { theme::border() };
        let provider = match &self.subject { Subject::Account { provider, .. } => provider.clone(), Subject::Machine => String::new() };
        let icons: &[&str] = if self.subject == Subject::Machine { &MACHINE_ICONS } else { &ACCOUNT_ICONS };
        let tint = self.color.as_deref().and_then(theme::tag_color).unwrap_or_else(theme::text);
        let icon_cells = icons.iter().map(|key| {
            let (key, on) = (*key, self.icon == *key);
            let label = tr_shared(&format!("tags_icon_{}", key.replace('-', "_")), &[]);
            let mark = match (&self.subject, account_icon(key)) {
                (Subject::Machine, _) => chrome::small_icon(machine_icon(key), 16., tint).into_any_element(),
                (_, Some(icon)) => chrome::small_icon(icon, 16., tint).into_any_element(),
                (_, None) => provider_mark(&provider, 16., tint),
            };
            Button::new(SharedString::from(format!("tag-icon-{key}"))).ghost().small().selected(on).disabled(self.saving)
                .border_1().border_color(swatch_ring(on)).tooltip(label.clone()).accessibility_label(label).child(mark)
                .on_click(cx.listener(move |this, _, _, cx| this.pick_icon(key, cx)))
        });
        let auto = tr_shared("tags_color_auto", &[]);
        let color_cells = std::iter::once((None, auto)).chain(COLORS.iter().map(|key| (Some(*key), tr_shared(&format!("tags_color_{key}"), &[]))))
            .map(|(key, label)| {
                let on = self.color.as_deref() == key;
                let dot = match key.and_then(theme::tag_color) {
                    Some(color) => div().size(px(14.)).rounded_full().bg(color),
                    // "Automática": anel vazado, a cor sai do hash da máquina ou da conta.
                    None => div().size(px(14.)).rounded_full().border_2().border_color(theme::muted()),
                };
                Button::new(SharedString::from(format!("tag-color-{}", key.unwrap_or("auto")))).ghost().small().selected(on).disabled(self.saving)
                    .border_1().border_color(swatch_ring(on)).tooltip(label.clone()).accessibility_label(label).child(dot)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_color(key, cx)))
            });
        let status = if self.saving { Some((tr_shared("tags_saving", &[]), theme::muted())) }
            else if self.saved { Some((tr_shared("tags_saved", &[]), theme::success())) } else { None };
        body
            .when(self.subject == Subject::Machine, |el| el.child(div().flex().flex_col().gap(px(6.)).child(title("tags_name"))
                .child(Input::new(&self.name).small().disabled(self.saving).aria_label(tr_shared("tags_name", &[])))
                .child(muted(tr_shared("tags_name_hint", &[])))))
            .child(div().flex().flex_col().gap(px(6.)).child(title("tags_icon"))
                .child(div().id("tag-icons").role(Role::Group).aria_label(tr_shared("tags_icon", &[])).flex().flex_wrap().gap(px(6.)).children(icon_cells)))
            .child(div().flex().flex_col().gap(px(6.)).child(title("tags_color"))
                .child(div().id("tag-colors").role(Role::Group).aria_label(tr_shared("tags_color", &[])).flex().flex_wrap().gap(px(6.)).children(color_cells)))
            .children(self.error.clone().map(|error| div().id("tag-editor-error").role(Role::Alert).text_color(theme::danger())
                .whitespace_normal().child(error)))
            .child(div().flex().items_center().justify_end().gap(px(10.))
                .children(status.map(|(text, color)| div().id("tag-editor-status").role(Role::Status).text_size(px(12.5)).text_color(color).child(text)))
                .child(Button::new("tag-editor-save").primary().small().label(tr_shared(if self.saving { "tags_saving" } else { "tags_save" }, &[]))
                    .loading(self.saving).disabled(self.saving).on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))))
    }
}

impl Hangar {
    /// Abre o editor da etiqueta num diálogo, gravando na máquina de `api`.
    pub(super) fn open_tag_editor(&mut self, api: Api, subject: Subject, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let runtime = self.runtime.clone();
        let editor = cx.new(|cx| TagEditor::new(api, runtime, subject, window, cx));
        window.open_dialog(cx, move |dialog, _, _| popup::dialog(dialog).w(px(480.)).title(title.clone()).child(editor.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn plate_shows_the_machine_only_for_another_machine() {
        // Mesmo identificador: a sessão é desta máquina, mesmo por um endereço que não é loopback.
        assert!(!shows_machine(Some("casa"), Some("casa"), false));
        assert!(shows_machine(Some("casa"), Some("notebook"), true));
        // Sem um dos identificadores (servidor sem CP_SERVER_ID), vale o loopback.
        assert!(!shows_machine(None, Some("notebook"), true));
        assert!(shows_machine(Some("casa"), None, false));
        assert!(!shows_machine(Some(""), Some(""), true));
    }

    #[test]
    fn plate_names_the_machine_by_tag_then_local_label_then_host() {
        let tag = |name: &str| Tag { name: name.into(), icon: "house".into(), color: None };
        assert_eq!(machine_name(Some(&tag("Casa")), "pc-sala", "https://casa.tail.ts.net"), "Casa");
        assert_eq!(machine_name(Some(&tag("  ")), "pc-sala", "https://casa.tail.ts.net"), "pc-sala");
        assert_eq!(machine_name(None, "", "https://casa.tail.ts.net"), "casa");
    }

    #[test]
    fn plate_reads_the_event_and_keeps_the_previous_one_on_a_broken_body() {
        let mut map = HashMap::new();
        let event = json!({"machine_id": "casa", "machine": {"name": "Casa", "icon": "house", "color": "green"},
            "accounts": {"claude:/home/j/.claude-200-2": {"name": "claude-200-2", "icon": "provider", "color": null}}});
        assert_eq!(store(&mut map, "k", &event), Some(true));
        assert_eq!(store(&mut map, "k", &event), Some(false));
        assert_eq!(store(&mut map, "k", &json!({"machine": 3})), None);
        let kept = &map["k"];
        assert_eq!(kept.accounts["claude:/home/j/.claude-200-2"].name, "claude-200-2");
        assert!(account_icon("provider").is_none());
        assert!(matches!(machine_icon(&kept.machine.as_ref().unwrap().icon), IconName::House));
    }

    #[test]
    fn plate_editor_sends_only_the_edited_tag() {
        assert_eq!(change(&Subject::Machine, " Casa ", "house", None), json!({"machine": {"name": "Casa", "icon": "house", "color": null}}));
        let account = Subject::Account { id: "codex:a".into(), provider: "codex".into() };
        assert_eq!(change(&account, "", "wallet", Some("pink")), json!({"account": {"id": "codex:a", "tag": {"icon": "wallet", "color": "pink"}}}));
    }
}
