//! Pedidos do cofre de uma máquina dona (evento `vault` da lista dela): aprovar o uso de uma senha por um agente, ou
//! cadastrar a senha que ele pediu. Um cartão por pedido, ao lado do aviso do chaveiro; a resposta vai à própria dona,
//! com o `interaction_id` do cartão em que a pessoa agiu. A senha digitada só sai daqui no corpo da resposta.
use super::*;
use super::vault::{Policy, vault_failure};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind { Approve, Entry }

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Request {
    id: String,
    interaction_id: String,
    kind: Kind,
    origin: String,
    #[serde(default)]
    entry_name: Option<String>,
    #[serde(default)]
    label: Option<String>,
    reason: String,
    #[serde(default, rename = "use")]
    usage: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    placeholder: Option<String>,
}

#[derive(serde::Deserialize)]
struct Event { #[serde(default)] requests: Vec<Request> }

/// Pedidos do evento `vault`; `None` = corpo que não se lê (o anterior fica).
pub(super) fn parse(data: &Value) -> Option<Vec<Request>> { serde_json::from_value::<Event>(data.clone()).ok().map(|e| e.requests) }

#[derive(Clone, Copy)]
enum Decision { Allow, Deny, Save { keep: bool }, Decline }

/// Clique que responde o cartão `id`. Pela referência fraca: o `cx.listener` devolvido de uma closure ficava preso ao empréstimo do `cx`.
fn answer_on(id: &str, decision: Decision, cx: &mut Context<VaultRequests>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static + use<> {
    let (id, requests) = (id.to_owned(), cx.entity().downgrade());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        let _ = requests.update(cx, |this, cx| this.answer(id.clone(), decision, window, cx));
    }
}

struct EntryFields { password: Entity<InputState>, name: Entity<InputState> }

struct Card { request: Request, fields: Option<EntryFields>, policy: Policy, sending: bool, error: Option<String> }

impl Card {
    fn new(request: Request, window: &mut Window, cx: &mut Context<VaultRequests>) -> Self {
        let fields = (request.kind == Kind::Entry).then(|| {
            let hint = request.placeholder.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| tr("vault_request_password"));
            let label = request.label.clone().unwrap_or_default();
            EntryFields {
                password: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint)),
                name: cx.new(|cx| InputState::new(window, cx).default_value(label)),
            }
        });
        Self { request, fields, policy: Policy::Ask, sending: false, error: None }
    }
}

/// Os cartões de uma máquina. Entidade própria: o diálogo da barra de abas mostra a mesma.
pub(super) struct VaultRequests { api: Option<Api>, runtime: Arc<Runtime>, machine: String, cards: Vec<Card> }

impl VaultRequests {
    /// Pedido igual (mesmo id e mesma interação) guarda o que já foi digitado; pedido trocado nasce de novo.
    fn apply(&mut self, api: Option<Api>, machine: String, list: Vec<Request>, window: &mut Window, cx: &mut Context<Self>) {
        (self.api, self.machine) = (api, machine);
        let mut old = std::mem::take(&mut self.cards);
        self.cards = list.into_iter().map(|request| {
            match old.iter().position(|c| c.request.id == request.id && c.request.interaction_id == request.interaction_id) {
                Some(at) => { let mut card = old.swap_remove(at); card.request = request; card }
                None => Card::new(request, window, cx),
            }
        }).collect();
        cx.notify();
    }

    pub(super) fn count(&self) -> usize { self.cards.len() }

    fn answer(&mut self, id: String, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|c| c.request.id == id).filter(|c| !c.sending) else { return };
        let interaction = card.request.interaction_id.clone();
        let body = match decision {
            Decision::Allow => json!({"interaction_id": interaction, "decision": "allow"}),
            Decision::Deny => json!({"interaction_id": interaction, "decision": "deny"}),
            Decision::Decline => json!({"interaction_id": interaction, "decision": "decline"}),
            Decision::Save { keep } => {
                let Some(fields) = &card.fields else { return };
                let password = fields.password.read(cx).value().to_string();
                if password.is_empty() { card.error = Some(tr("vault_password_required")); cx.notify(); return; }
                let mut body = json!({"interaction_id": interaction, "decision": "save", "password": password, "keep": keep});
                let name = fields.name.read(cx).value().trim().to_owned();
                if !name.is_empty() { body["name"] = json!(name); }
                if keep { body["policy"] = json!(card.policy); }
                body
            }
        };
        let Some(api) = self.api.clone() else {
            card.error = Some(tr("network_error"));
            cx.notify();
            return;
        };
        (card.sending, card.error) = (true, None);
        let (done, result) = tokio::sync::oneshot::channel();
        let path = id.clone();
        // O chaveiro da dona pode demorar a abrir no `save`.
        self.runtime.spawn(async move {
            let _ = done.send(api.server_send(reqwest::Method::POST, &["vault", "requests", &path], Some(body), 45).await);
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update(cx, |this, cx| {
                match result {
                    // Quem tira o cartão de vez é o evento da dona; aqui ele só sai da frente.
                    Ok(_) => this.cards.retain(|c| c.request.id != id),
                    // Recusa mantém o texto digitado (chaveiro trancado no `save`, por exemplo).
                    Err(error) => if let Some(card) = this.cards.iter_mut().find(|c| c.request.id == id) {
                        (card.sending, card.error) = (false, Some(vault_failure(&error)));
                    },
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let r = &card.request;
        let id = r.id.clone();
        let off = card.sending;
        let button = |suffix: &str, label: String| request_button(suffix, &id, label, off);
        let on = |decision: Decision, cx: &mut Context<Self>| answer_on(&id, decision, cx);
        let (title, body) = match r.kind {
            Kind::Approve => {
                let allow = button("allow", tr("vault_request_allow")).primary().on_click(on(Decision::Allow, cx));
                let deny = button("deny", tr("vault_request_deny")).outline().on_click(on(Decision::Deny, cx));
                (approve_title(&r.origin, r.entry_name.as_deref().unwrap_or_default()),
                    approve_body(r.usage.as_deref(), r.target.as_deref().unwrap_or_default(), allow, deny))
            }
            Kind::Entry => {
                let label = r.label.clone().unwrap_or_default();
                let body = div().flex().flex_col().gap_1();
                let body = match &card.fields {
                    None => body,
                    Some(fields) => {
                        let policy = card.policy;
                        let radio = |suffix: &str, value: Policy, cx: &mut Context<Self>| {
                            let id = id.clone();
                            Radio::new(SharedString::from(format!("vault-request-policy-{suffix}-{id}"))).label(value.label()).checked(policy == value).disabled(off)
                                .on_change(cx.listener(move |this: &mut Self, _, _, cx| {
                                    if let Some(card) = this.cards.iter_mut().find(|c| c.request.id == id) { card.policy = value; cx.notify(); }
                                }))
                        };
                        body.child(Input::new(&fields.password).xsmall().mask_toggle().disabled(off).aria_label(tr("vault_field_password")))
                            .child(Input::new(&fields.name).xsmall().disabled(off).aria_label(tr("vault_field_name")))
                            .child(div().id(SharedString::from(format!("vault-request-policy-{id}"))).role(Role::RadioGroup).aria_label(tr("vault_field_policy")).flex().flex_col().gap_1().text_xs()
                                .child(radio("ask", Policy::Ask, cx))
                                .child(radio("free", Policy::Free, cx)))
                            .child(button("keep", tr("vault_request_keep")).primary().on_click(on(Decision::Save { keep: true }, cx)))
                            .child(button("once", tr("vault_request_once")).outline().on_click(on(Decision::Save { keep: false }, cx)))
                            .child(button("decline", tr("vault_request_decline")).ghost().on_click(on(Decision::Decline, cx)))
                    }
                };
                (tr("vault_request_entry_title").replace("{origin}", &r.origin).replace("{label}", &label), body)
            }
        };
        request_card(&id, &self.machine, title, &r.reason, body, card.error.clone()).into_any_element()
    }
}

fn request_line(text: String) -> Div { div().text_xs().text_color(theme::muted()).whitespace_normal().child(text) }

/// Botão de um cartão de pedido, nas larguras do cartão: `suffix` é a ação, `id` o pedido.
pub(crate) fn request_button(suffix: &str, id: &str, label: String, off: bool) -> Button {
    Button::new(SharedString::from(format!("vault-request-{suffix}-{id}"))).xsmall().w_full().label(label).disabled(off)
}

pub(crate) fn approve_title(origin: &str, entry: &str) -> String {
    tr("vault_request_approve_title").replace("{origin}", origin).replace("{entry}", entry)
}

/// Bloco do comando num cartão: a linha que o agente quer rodar, em fonte fixa.
pub(crate) fn request_command(text: String) -> Div {
    div().p_1().rounded(px(4.)).bg(theme::boxed()).font_family(theme::MONO).text_xs().whitespace_normal().child(text)
}

/// Corpo do cartão de aprovação: para que a senha vai ser usada, o comando e os dois botões. `usage` é `fill`, `exec` ou `file`.
pub(crate) fn approve_body(usage: Option<&str>, target: &str, allow: Button, deny: Button) -> Div {
    let line = match usage {
        Some("fill") => tr("vault_request_use_fill").replace("{target}", target),
        Some("exec") => tr("vault_request_use_exec"),
        Some("file") => tr("vault_request_use_file"),
        _ => String::new(),
    };
    let command = (usage == Some("exec") && !target.is_empty()).then(|| request_command(target.to_owned()));
    div().flex().flex_col().gap_1()
        .when(!line.is_empty(), |el| el.child(request_line(line)))
        .children(command)
        .child(div().flex().gap_1().child(allow).child(deny))
}

/// A casca de um cartão de pedido: de que máquina veio, o que pede, o motivo, o corpo e a falha da última resposta.
pub(crate) fn request_card(id: &str, machine: &str, title: String, reason: &str, body: Div, error: Option<String>) -> Stateful<Div> {
    let heading = div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning()).whitespace_normal()
        .child(format!("{} · {machine}", tr("vault_request_heading")));
    div().id(SharedString::from(format!("vault-request-{id}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.)).bg(theme::inset())
        .flex().flex_col().gap_1().role(Role::Group).aria_label(title.clone())
        .child(heading)
        .child(div().text_xs().text_color(theme::text()).whitespace_normal().child(title))
        .when(!reason.is_empty(), |el| el.child(request_line(tr("vault_request_reason").replace("{reason}", reason))))
        .child(body)
        .children(error.map(|error| div().id(SharedString::from(format!("vault-request-error-{id}"))).role(Role::Alert)
            .text_xs().text_color(theme::danger()).whitespace_normal().child(error)))
}

impl Render for VaultRequests {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cards.is_empty() { return div().p_2().text_sm().text_color(theme::muted()).child(tr("vault_request_none")); }
        let cards: Vec<AnyElement> = self.cards.iter().map(|card| self.render_card(card, cx)).collect();
        div().flex().flex_col().children(cards)
    }
}

impl Hangar {
    /// Pedidos novos do cofre de uma máquina (evento `vault`; `None` na queda do stream, que os reenvia ao voltar). Devolve se mudou.
    pub(super) fn set_vault_requests(&mut self, key: &str, data: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let list = match data {
            None => Vec::new(),
            Some(data) => match parse(data) { Some(list) => list, None => return false },
        };
        if list.is_empty() { return self.vault.remove(key).is_some(); }
        let (api, machine) = (self.machine_api(key), self.vault_machine_label(key, cx));
        let runtime = self.runtime.clone();
        let entity = self.vault.entry(key.to_owned())
            .or_insert_with(|| cx.new(|_| VaultRequests { api: None, runtime, machine: String::new(), cards: Vec::new() }))
            .clone();
        entity.update(cx, |requests, cx| requests.apply(api, machine, list, window, cx));
        true
    }

    fn vault_machine_label(&self, key: &str, cx: &App) -> String {
        match self.server_entry(key) {
            Some(entry) if !entry.label.is_empty() => entry.label.clone(),
            Some(entry) => servers::default_label(&entry.address),
            None => self.server_label(cx),
        }
    }

    /// Os cartões de uma máquina na barra lateral, se houver pedido esperando.
    pub(super) fn render_vault_requests(&self, key: &str, cx: &App) -> Option<AnyElement> {
        self.vault.get(key).filter(|e| e.read(cx).count() > 0).map(|e| e.clone().into_any_element())
    }

    /// Na barra das abas não cabe o cartão: uma linha por máquina que abre os cartões num diálogo.
    pub(super) fn render_vault_chips(&self, cx: &App) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<&String> = self.vault.keys().collect();
        keys.sort_by_key(|key| (**key != active, (*key).clone()));
        keys.into_iter().filter_map(|key| {
            let entity = self.vault.get(key)?.clone();
            let count = entity.read(cx).count();
            if count == 0 { return None; }
            let machine = self.vault_machine_label(key, cx);
            let label = tr("vault_request_chip").replace("{n}", &count.to_string()).replace("{machine}", &machine);
            let title = format!("{} · {machine}", tr("vault_request_heading"));
            Some(Button::new(SharedString::from(format!("vault-chip-{key}"))).xsmall().outline().flex_shrink_0().icon(IconName::KeyRound)
                .label(label)
                .on_click(move |_, window, cx| {
                    let (entity, title) = (entity.clone(), title.clone());
                    window.open_dialog(cx, move |dialog, _, _| popup::dialog(dialog).w(px(420.)).title(title.clone()).child(entity.clone()));
                })
                .into_any_element())
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn vault_event_reads_both_kinds_and_rejects_a_broken_body() {
        let data = json!({"state": "open", "requests": [
            {"id": "a", "interaction_id": "i1", "kind": "approve", "origin": "casa::web", "entry_name": "GitHub", "reason": "login",
                "use": "fill", "target": "github.com", "expires_at": 1},
            {"id": "b", "interaction_id": "i2", "kind": "entry", "origin": "casa::web", "label": "Token", "reason": "deploy", "expires_at": 1}]});
        let list = parse(&data).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].kind, list[0].usage.as_deref()), (Kind::Approve, Some("fill")));
        assert_eq!((list[1].kind, list[1].label.as_deref()), (Kind::Entry, Some("Token")));
        assert_eq!(parse(&json!({"state": null, "requests": []})), Some(Vec::new()));
        assert_eq!(parse(&json!({"requests": [{"id": "x"}]})), None);
    }
}
