//! Pedidos de aprovação de escrita das integrações MCP (evento SSE `integrations_approvals`): um cartão por
//! pedido, na conversa da sessão que pediu ou flutuando, respondido na máquina dona com `interaction_id`.
use super::*;
use super::integrations::integrations_failure;

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Request {
    pub id: String,
    pub interaction_id: String,
    pub origin: String,
    pub integration_id: String,
    pub integration_name: String,
    pub tool: String,
    #[serde(default)]
    pub arguments_preview: String,
    #[serde(default)]
    pub reason: String,
    pub expires_at: f64,
}

#[derive(serde::Deserialize)]
struct Event {
    requests: Vec<Request>,
}

/// Pedidos do evento `integrations_approvals`; `None` = corpo quebrado (os cartões anteriores ficam).
pub(super) fn parse(data: &Value) -> Option<Vec<Request>> {
    let event = serde_json::from_value::<Event>(data.clone()).ok()?;
    let valid = event.requests.iter().all(|r| {
        !r.id.trim().is_empty()
            && !r.interaction_id.trim().is_empty()
            && !r.integration_id.trim().is_empty()
            && !r.tool.trim().is_empty()
            && r.expires_at.is_finite()
    });
    valid.then_some(event.requests)
}

#[derive(Clone, Copy)]
enum Decision {
    AllowOnce,
    AllowAlways,
    Deny,
}

impl Decision {
    fn wire(self) -> &'static str {
        match self {
            Decision::AllowOnce => "allow_once",
            Decision::AllowAlways => "allow_always",
            Decision::Deny => "deny",
        }
    }
}

fn answer_on(
    id: &str,
    decision: Decision,
    cx: &mut Context<IntegrationRequests>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static + use<> {
    let (id, requests) = (id.to_owned(), cx.entity().downgrade());
    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        let _ = requests.update(cx, |this, cx| this.answer(id.clone(), decision, window, cx));
    }
}

struct Card {
    request: Request,
    sending: bool,
    error: Option<String>,
}

/// Os cartões de aprovação pendentes de uma máquina dona.
pub(super) struct IntegrationRequests {
    api: Option<Api>,
    runtime: Arc<Runtime>,
    machine: String,
    cards: Vec<Card>,
}

impl IntegrationRequests {
    fn apply(
        &mut self,
        api: Option<Api>,
        machine: String,
        list: Vec<Request>,
        cx: &mut Context<Self>,
    ) {
        (self.api, self.machine) = (api, machine);
        let mut old = std::mem::take(&mut self.cards);
        self.cards = list
            .into_iter()
            .map(|request| {
                match old
                    .iter()
                    .position(|c| c.request.id == request.id && c.request.interaction_id == request.interaction_id)
                {
                    Some(at) => {
                        let mut card = old.swap_remove(at);
                        card.request = request;
                        card
                    }
                    None => Card {
                        request,
                        sending: false,
                        error: None,
                    },
                }
            })
            .collect();
        cx.notify();
    }

    pub(super) fn count(&self) -> usize {
        self.cards.len()
    }

    fn render_where(
        &self,
        keep: &dyn Fn(&str) -> bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.cards
            .iter()
            .filter(|c| keep(&c.request.origin))
            .map(|card| self.render_card(card, cx))
            .collect()
    }

    fn answer(
        &mut self,
        id: String,
        decision: Decision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self
            .cards
            .iter_mut()
            .find(|c| c.request.id == id)
            .filter(|c| !c.sending)
        else {
            return;
        };
        let body = json!({
            "interaction_id": card.request.interaction_id,
            "decision": decision.wire(),
        });
        let Some(api) = self.api.clone() else {
            card.error = Some(tr_shared("integrations_approval_no_server", &[]));
            cx.notify();
            return;
        };
        (card.sending, card.error) = (true, None);
        let (done, result) = tokio::sync::oneshot::channel();
        let path = id.clone();
        self.runtime.spawn(async move {
            let _ = done.send(
                api.server_send(
                    reqwest::Method::POST,
                    &["integrations", "approvals", &path],
                    Some(body),
                    30,
                )
                .await,
            );
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = result
                .await
                .unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update(cx, |this, cx| {
                match result {
                    // O evento da dona limpa em definitivo; aqui removemos logo após o 200.
                    Ok(_) => this.cards.retain(|c| c.request.id != id),
                    Err(error) => {
                        if let Some(card) = this.cards.iter_mut().find(|c| c.request.id == id) {
                            (card.sending, card.error) = (false, Some(integrations_failure(&error)));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let r = &card.request;
        let id = r.id.clone();
        let off = card.sending;
        let btn = |suffix: &str, label: String| {
            Button::new(SharedString::from(format!("integration-request-{suffix}-{id}")))
                .xsmall()
                .w_full()
                .label(label)
                .disabled(off)
        };
        let on = |decision: Decision, cx: &mut Context<Self>| answer_on(&id, decision, cx);
        let allow_once = btn("allow-once", tr_shared("integrations_approval_allow_once", &[]))
            .primary()
            .on_click(on(Decision::AllowOnce, cx));
        let allow_always = btn("allow-always", tr_shared("integrations_approval_allow_always", &[]))
            .outline()
            .on_click(on(Decision::AllowAlways, cx));
        let deny = btn("deny", tr_shared("integrations_approval_deny", &[]))
            .danger()
            .outline()
            .on_click(on(Decision::Deny, cx));

        let heading = div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::warning())
            .whitespace_normal()
            .child(format!(
                "{} · {}",
                tr_shared("integrations_approval_heading", &[]),
                self.machine
            ));
        let title = tr_shared("integrations_approval_title", &[]);
        let meta = |label_key: &str, value: String, mono: bool| {
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .text_xs()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(theme::faint())
                        .child(format!("{}:", tr_shared(label_key, &[]))),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(theme::text())
                        .when(mono, |el| el.font_family(theme::MONO))
                        .whitespace_normal()
                        .child(value),
                )
        };
        let half = || div().flex_1().min_w_0();

        div()
            .id(SharedString::from(format!("integration-request-{id}")))
            .flex_shrink_0()
            .mx(px(8.))
            .my(px(4.))
            .p_2()
            .rounded(px(8.))
            .bg(theme::inset())
            .flex()
            .flex_col()
            .gap_1()
            .role(Role::Group)
            .aria_label(title.clone())
            .child(heading)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .whitespace_normal()
                    .child(title),
            )
            .child(meta("integrations_approval_integration", r.integration_name.clone(), false))
            .child(meta("integrations_approval_tool", r.tool.clone(), true))
            .child(meta("integrations_approval_origin", r.origin.clone(), false))
            .when(!r.reason.trim().is_empty(), |el| {
                el.child(meta("integrations_approval_reason", r.reason.clone(), false))
            })
            .when(!r.arguments_preview.trim().is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::faint())
                                .child(format!("{}:", tr_shared("integrations_approval_args", &[]))),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("integration-request-args-{id}")))
                                .max_h(px(140.))
                                .overflow_y_scroll()
                                .p_1()
                                .rounded(px(4.))
                                .bg(theme::boxed())
                                .font_family(theme::MONO)
                                .text_xs()
                                .whitespace_normal()
                                .child(r.arguments_preview.clone()),
                        ),
                )
            })
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().flex().gap_1().child(half().child(allow_once)).child(half().child(deny)))
                    .child(allow_always),
            )
            .children(card.error.clone().map(|error| {
                div()
                    .id(SharedString::from(format!("integration-request-error-{id}")))
                    .role(Role::Alert)
                    .text_xs()
                    .text_color(theme::danger())
                    .whitespace_normal()
                    .child(error)
            }))
            .into_any_element()
    }
}

impl Render for IntegrationRequests {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cards.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(theme::muted())
                .child(tr_shared("integrations_approval_lost", &[]));
        }
        let cards: Vec<AnyElement> = self
            .cards
            .iter()
            .map(|card| self.render_card(card, cx))
            .collect();
        div().flex().flex_col().children(cards)
    }
}

impl Hangar {
    /// Atualiza pedidos de aprovação das integrações para uma máquina (`None` na queda do stream).
    pub(super) fn set_integration_requests(
        &mut self,
        key: &str,
        data: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> bool {
        let list = match data {
            None => Vec::new(),
            Some(data) => match parse(data) {
                Some(list) => list,
                None => {
                    crate::log_line(
                        "integracoes: evento integrations_approvals ilegivel; os cartoes anteriores ficam",
                    );
                    return false;
                }
            },
        };
        if list.is_empty() {
            return self.integrations.remove(key).is_some();
        }
        let (api, machine) = (self.machine_api(key), self.vault_machine_label(key, cx));
        let runtime = self.runtime.clone();
        let entity = self
            .integrations
            .entry(key.to_owned())
            .or_insert_with(|| {
                cx.new(|_| IntegrationRequests {
                    api: None,
                    runtime,
                    machine: String::new(),
                    cards: Vec::new(),
                })
            })
            .clone();
        entity.update(cx, |requests, cx| requests.apply(api, machine, list, cx));
        self.ensure_identities();
        true
    }

    fn integration_origin_matches(&self, key: &str, origin: &str) -> bool {
        if self.files_expanded() {
            return false;
        }
        let Some(name) = self.selected.as_ref().map(|s| s.name.as_str()) else {
            return false;
        };
        let open = self.open_server();
        if open != key {
            return false;
        }
        match self.identity_slot(&open) {
            Some(super::grouping::IdSlot::Known(id)) => {
                origin == format!("{id}::{name}") || (!origin.contains("::") && origin == name)
            }
            _ if !self.multi_server() => {
                origin == name || origin.ends_with(&format!("::{name}"))
            }
            _ => false,
        }
    }

    fn integration_cards(&self, session_only: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<String> = self.integrations.keys().cloned().collect();
        keys.sort_by_key(|key| (*key != active, key.clone()));
        keys.into_iter()
            .flat_map(|key| {
                let entity = self.integrations[&key].clone();
                entity.update(cx, |requests, cx| {
                    requests.render_where(
                        &|origin: &str| self.integration_origin_matches(&key, origin) == session_only,
                        cx,
                    )
                })
            })
            .collect()
    }

    /// Cartões da sessão aberta, acima da caixa de mensagem.
    pub(super) fn render_integration_session(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cards = self.integration_cards(true, cx);
        (!cards.is_empty()).then(|| {
            div()
                .flex_shrink_0()
                .w_full()
                .max_w(px(760.))
                .mx_auto()
                .flex()
                .flex_col()
                .children(cards)
                .into_any_element()
        })
    }

    /// Cartões de outras sessões ou quando a conversa está coberta: flutuam sobre a janela.
    #[allow(dead_code)]
    pub(super) fn render_integration_float(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.render_integration_float_at(0., cx)
    }

    /// Versão com deslocamento horizontal quando o popover do cofre também está aberto no mesmo canto.
    pub(super) fn render_integration_float_at(
        &self,
        extra_right: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let cards = self.integration_cards(false, cx);
        if cards.is_empty() {
            return None;
        }
        let cards = div()
            .id("integrations-float")
            .max_h(px(520.))
            .overflow_y_scroll()
            .py_1()
            .flex()
            .flex_col()
            .children(cards);
        Some(
            div()
                .absolute()
                .top(px(52.))
                .right(px(16. + extra_right))
                .occlude()
                .child(chrome::popover(cards.into_any_element(), true))
                .into_any_element(),
        )
    }

    /// Botão compacto na barra de abas/trilho que abre os cartões num diálogo.
    pub(super) fn render_integration_chips(&self, rail: bool, cx: &App) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<&String> = self.integrations.keys().collect();
        keys.sort_by_key(|key| (**key != active, (*key).clone()));
        keys.into_iter()
            .filter_map(|key| {
                let entity = self.integrations.get(key)?.clone();
                let count = entity.read(cx).count();
                if count == 0 {
                    return None;
                }
                let machine = self.vault_machine_label(key, cx);
                let label = tr_shared(
                    "integrations_approval_chip",
                    &[("n", &count.to_string()), ("machine", &machine)],
                );
                let title = format!(
                    "{} · {machine}",
                    tr_shared("integrations_approval_heading", &[])
                );
                let button = Button::new(SharedString::from(format!("integration-chip-{key}")))
                    .xsmall()
                    .outline()
                    .flex_shrink_0()
                    .icon(IconName::Plug);
                let button = if rail {
                    button
                        .label(count.to_string())
                        .tooltip(label.clone())
                        .accessibility_label(label)
                } else {
                    button.label(label)
                };
                Some(
                    button
                        .on_click(move |_, window, cx| {
                            let (entity, title) = (entity.clone(), title.clone());
                            window.open_dialog(cx, move |dialog, _, _| {
                                popup::dialog(dialog)
                                    .w(px(420.))
                                    .title(title.clone())
                                    .child(entity.clone())
                            });
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn integrations_approvals_event_parses_valid_and_rejects_broken_payloads() {
        let valid = json!({
            "requests": [
                {
                    "id": "req-1",
                    "interaction_id": "int-1",
                    "origin": "casa::sessao-1",
                    "integration_id": "github",
                    "integration_name": "GitHub",
                    "tool": "create_issue",
                    "arguments_preview": "{\"title\":\"Bug\"}",
                    "reason": "Abrir issue",
                    "expires_at": 1700000000.0
                }
            ]
        });
        let list = parse(&valid).expect("evento válido");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].integration_name, "GitHub");
        assert_eq!(list[0].tool, "create_issue");
        assert_eq!(list[0].origin, "casa::sessao-1");

        assert_eq!(parse(&json!({"requests": []})), Some(Vec::new()));
        assert_eq!(parse(&json!({})), None);
        assert_eq!(parse(&json!({"requests": [{"id": "x"}]})), None);
        assert_eq!(
            parse(&json!({
                "requests": [{
                    "id": "",
                    "interaction_id": "int-1",
                    "origin": "casa::s",
                    "integration_id": "github",
                    "integration_name": "GitHub",
                    "tool": "create_issue",
                    "expires_at": 1.0
                }]
            })),
            None
        );
    }
}
