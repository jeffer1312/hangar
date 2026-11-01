//! Página "Sessões": a ocultação automática é uma configuração só, gravada em todas as máquinas da pessoa.
use super::*;
use super::settings::{Page, settings_box};
use gpui_kit::component::switch::Switch;

/// Prazo pela tela, em horas inteiras: de 1 h a 7 dias (a rota aceita minutos para a verificação manual).
const MAX_HOURS: u32 = 168;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct AutoHide { enabled: bool, after_minutes: u32 }

impl AutoHide {
    fn hours(self) -> u32 { ((self.after_minutes + 30) / 60).clamp(1, MAX_HOURS) }
}

/// Uma máquina da pessoa: a leitura (`None` esperando) e o resultado da última gravação.
struct Machine { key: String, label: String, read: Option<Result<AutoHide, String>>, wrote: Option<Result<(), String>> }

#[derive(Default)]
pub(super) struct AutoHidePage {
    generation: u64,
    connection: u64,
    machines: Vec<Machine>,
    /// O que a tela mostra; segue a referência até a pessoa mexer.
    draft: Option<AutoHide>,
    edited: bool,
    hours_text: String,
    /// Máquinas com a gravação ainda sem resposta.
    saving: usize,
    hours: Option<(Entity<InputState>, Subscription)>,
}

pub(super) struct AutoHideReply { generation: u64, key: String, write: bool, result: Result<Value, Failure> }

impl AutoHidePage {
    /// A configuração mais comum entre as leituras boas; no empate, a da primeira máquina da lista (a ativa vem antes).
    /// Mesma regra do `autoHideDiverges` do core.
    fn reference(&self) -> Option<AutoHide> {
        let ok: Vec<AutoHide> = self.machines.iter().filter_map(|m| m.read.as_ref()?.as_ref().ok().copied()).collect();
        let mut best: Option<(AutoHide, usize)> = None;
        for v in &ok {
            let n = ok.iter().filter(|o| *o == v).count();
            if best.is_none_or(|(_, top)| n > top) { best = Some((*v, n)); }
        }
        best.map(|(v, _)| v)
    }

    /// Máquinas lidas com configuração diferente da referência; leitura que falhou aparece como erro, não aqui.
    fn differs(&self) -> Vec<&str> {
        let Some(reference) = self.reference() else { return Vec::new() };
        self.machines.iter().filter(|m| matches!(&m.read, Some(Ok(v)) if *v != reference)).map(|m| m.label.as_str()).collect()
    }

    /// O que gravar; `None` com as horas digitadas fora do limite.
    fn value(&self) -> Option<AutoHide> {
        let mut value = self.draft?;
        if value.enabled { value.after_minutes = self.hours_text.trim().parse::<u32>().ok().filter(|h| (1..=MAX_HOURS).contains(h))? * 60; }
        Some(value)
    }

    fn dirty(&self) -> bool { self.value().is_some_and(|v| Some(v) != self.reference()) }
}

impl Hangar {
    /// As máquinas da pessoa (sem convite, desligada nem par externo), a da tela primeiro; num servidor só sem lista, a ativa.
    fn auto_hide_machines(&self, cx: &App) -> Vec<(String, String, Result<Api, Failure>)> {
        let mut own: Vec<&servers::ServerEntry> = self.servers.iter().filter(|s| !s.invite && !s.disabled && !s.ephemeral).collect();
        if own.is_empty() {
            return self.api.clone().filter(|_| !self.active_invite()).map(|api| vec![(self.active_key(), self.server_label(cx), Ok(api))]).unwrap_or_default();
        }
        own.sort_by_key(|s| !self.is_active_key(&servers::norm(&s.address)));
        own.into_iter().map(|s| {
            let key = servers::norm(&s.address);
            let api = self.machine_api(&key).map(Ok).unwrap_or_else(|| Api::new(&s.address, &s.token));
            (key, s.label.clone(), api)
        }).collect()
    }

    /// Página aberta: relê todas as máquinas, para o aviso de diferença valer o que está gravado agora.
    pub(super) fn auto_hide_opened(&mut self, cx: &mut Context<Self>) {
        let machines = self.auto_hide_machines(cx);
        let generation = self.auto_hide.generation + 1;
        self.auto_hide = AutoHidePage { generation, connection: self.connection, ..Default::default() };
        for (key, label, api) in machines {
            self.auto_hide.machines.push(Machine { key: key.clone(), label, read: None, wrote: None });
            self.request_auto_hide(key, api, None);
        }
        cx.notify();
    }

    fn request_auto_hide(&self, key: String, api: Result<Api, Failure>, value: Option<AutoHide>) {
        let (tell, generation) = (self.sidebar_tell(), self.auto_hide.generation);
        self.runtime.spawn(async move {
            let result = match (api, value) {
                (Err(error), _) => Err(error),
                (Ok(api), None) => api.server_read(&["session-auto-hide"], &[], 15).await,
                (Ok(api), Some(value)) => api.server_send(reqwest::Method::PUT, &["session-auto-hide"], Some(json!(value)), 15).await,
            };
            tell.send(sidebar::SidebarReply::AutoHide(AutoHideReply { generation, key, write: value.is_some(), result })).await;
        });
    }

    /// Salvar e "Aplicar em todas": o mesmo valor em todas as máquinas, cada uma com o próprio resultado.
    fn save_auto_hide(&mut self, cx: &mut Context<Self>) {
        let Some(value) = self.auto_hide.value().filter(|_| self.auto_hide.saving == 0) else { return };
        let machines = self.auto_hide_machines(cx);
        let page = &mut self.auto_hide;
        for m in &mut page.machines { m.wrote = None; }
        page.saving = machines.iter().filter(|(key, ..)| page.machines.iter().any(|m| &m.key == key)).count();
        for (key, _, api) in machines {
            if self.auto_hide.machines.iter().any(|m| m.key == key) { self.request_auto_hide(key, api, Some(value)); }
        }
        cx.notify();
    }

    pub(super) fn receive_auto_hide(&mut self, reply: AutoHideReply, window: &mut Window, cx: &mut Context<Self>) {
        let AutoHideReply { generation, key, write, result } = reply;
        if generation != self.auto_hide.generation { return; }
        let parsed = result.map_err(|error| Self::visibility_failure(&error))
            .and_then(|value| serde_json::from_value::<AutoHide>(value).map_err(|_| tr("invalid_response")));
        let page = &mut self.auto_hide;
        let Some(machine) = page.machines.iter_mut().find(|m| m.key == key) else { return };
        if write {
            page.saving = page.saving.saturating_sub(1);
            match parsed {
                Ok(value) => { machine.read = Some(Ok(value)); machine.wrote = Some(Ok(())); }
                Err(error) => machine.wrote = Some(Err(error)),
            }
        } else {
            machine.read = Some(parsed);
        }
        if !page.edited && let Some(reference) = page.reference() {
            page.draft = Some(reference);
            page.hours_text = reference.hours().to_string();
            // O texto guardado antes do `set_value` faz o aviso de mudança do campo não contar como edição.
            if let Some((input, _)) = &page.hours {
                let text = page.hours_text.clone();
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        cx.notify();
    }

    fn edit_auto_hide(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.auto_hide;
        page.edited = true;
        for m in &mut page.machines { m.wrote = None; }
        cx.notify();
    }

    pub(super) fn render_sessions_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Reconectou com a página aberta: as respostas da conexão anterior não chegam mais.
        if self.auto_hide.connection != self.connection { self.auto_hide_opened(cx); }
        let mut page = div().flex().flex_col().gap_4().child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Sessions.title()));
        let hidden = self.hidden_count(cx);
        let see_hidden = (hidden > 0).then(|| div().flex().child(Button::new("auto-hide-see-hidden").outline().small()
            .label(tr("sidebar_see_hidden").replace("{n}", &hidden.to_string()))
            .on_click(cx.listener(|this, _, window, cx| this.show_hidden_group(window, cx)))));
        let s = &self.auto_hide;
        if s.machines.is_empty() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).children(see_hidden).into_any_element();
        }
        let line = |id: String, text: String, color: Hsla| div().id(SharedString::from(id)).text_sm().text_color(color).whitespace_normal().child(text);
        let read_errors: Vec<_> = s.machines.iter().filter_map(|m| match &m.read {
            Some(Err(error)) => Some(line(format!("auto-hide-read-{}", m.key), format!("{}: {error}", m.label), theme::danger()).role(Role::Alert)),
            _ => None,
        }).collect();
        if s.draft.is_none() {
            if s.machines.iter().any(|m| m.read.is_none()) {
                return page.child(div().id("auto-hide-loading").role(Role::Status).aria_label(tr("loading"))
                    .text_sm().text_color(theme::muted()).child(tr("loading"))).into_any_element();
            }
            return page.children(read_errors).child(div().flex().child(Button::new("auto-hide-retry").outline().small().label(tr("server_retry"))
                .on_click(cx.listener(|this, _, _, cx| this.auto_hide_opened(cx))))).children(see_hidden).into_any_element();
        }
        if s.hours.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(self.auto_hide.hours_text.clone()));
            let sub = cx.subscribe_in(&input, window, |this: &mut Hangar, input, event: &InputEvent, _, cx| {
                if !matches!(event, InputEvent::Change) { return; }
                let text = input.read(cx).value().to_string();
                if text == this.auto_hide.hours_text { return; }
                this.auto_hide.hours_text = text;
                this.edit_auto_hide(cx);
            });
            self.auto_hide.hours = Some((input, sub));
        }
        let s = &self.auto_hide;
        let (Some(draft), Some((input, _))) = (s.draft, &s.hours) else { return page.into_any_element() };
        let busy = s.saving > 0;
        let value = s.value();
        let description = value.filter(|v| v.enabled).map(|v| tr("settings_auto_hide_hours").replace("{n}", &(v.after_minutes / 60).to_string()));
        let control = div().flex().items_center().gap_3()
            .when(draft.enabled, |el| el.child(div().w(px(72.)).child(Input::new(input).id("auto-hide-hours").small().disabled(busy)
                .accessibility_id("auto-hide-hours").aria_label(tr("settings_auto_hide_label")))))
            .child(Switch::new("auto-hide-enabled").checked(draft.enabled).disabled(busy).accessibility_label(tr("settings_auto_hide_label"))
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    if this.auto_hide.saving > 0 { return; }
                    if let Some(draft) = &mut this.auto_hide.draft { draft.enabled = *on; }
                    this.edit_auto_hide(cx);
                })));
        page = page.child(settings_box().child(self.row(IconName::EyeOff, "settings_auto_hide_label", description, true, control.into_any_element())));
        let s = &self.auto_hide;
        if value.is_none() {
            page = page.child(line("auto-hide-invalid".into(), tr_shared("session_auto_hide_invalid", &[]), theme::danger()).role(Role::Alert));
        }
        let differs = s.differs();
        if !differs.is_empty() {
            page = page.child(div().flex().items_center().gap_3()
                .child(line("auto-hide-differs".into(), tr("settings_auto_hide_differs").replace("{servers}", &differs.join(", ")), theme::warning()).flex_1())
                .child(Button::new("auto-hide-apply-all").outline().small().label(tr("settings_auto_hide_apply_all")).loading(busy)
                    .disabled(busy || value.is_none()).on_click(cx.listener(|this, _, _, cx| this.save_auto_hide(cx)))));
        }
        let results: Vec<_> = s.machines.iter().filter_map(|m| match &m.wrote {
            Some(Ok(())) => Some(line(format!("auto-hide-saved-{}", m.key), tr("settings_auto_hide_saved_on").replace("{server}", &m.label), theme::success()).role(Role::Status)),
            Some(Err(error)) => Some(line(format!("auto-hide-failed-{}", m.key),
                tr("settings_auto_hide_not_saved_on").replace("{server}", &m.label).replace("{error}", error), theme::danger()).role(Role::Alert)),
            None => None,
        }).collect();
        page.children(read_errors).children(results)
            .child(div().flex().items_center().justify_end().gap_3()
                .children(see_hidden)
                .child(div().flex_1())
                .child(Button::new("auto-hide-save").primary().small().label(tr(if busy { "server_saving" } else { "server_save" })).loading(busy)
                    .disabled(busy || !s.dirty()).on_click(cx.listener(|this, _, _, cx| this.save_auto_hide(cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoHide, AutoHidePage, Machine};

    fn machine(label: &str, read: Option<Result<AutoHide, String>>) -> Machine {
        Machine { key: label.into(), label: label.into(), read, wrote: None }
    }

    #[test]
    fn auto_hide_differs_names_machines_unlike_the_majority() {
        let (off, on) = (AutoHide { enabled: false, after_minutes: 180 }, AutoHide { enabled: true, after_minutes: 180 });
        let mut page = AutoHidePage { machines: vec![machine("nb", None), machine("vps", Some(Ok(off))), machine("dp", Some(Ok(on))),
            machine("win", Some(Err("x".into())))], ..Default::default() };
        assert_eq!(page.reference(), Some(off), "empate: a primeira que respondeu");
        assert_eq!(page.differs(), ["dp"], "a que falhou não entra");
        page.machines.push(machine("lin", Some(Ok(on))));
        assert_eq!(page.reference(), Some(on), "a mais comum vence a primeira");
        assert_eq!(page.differs(), ["vps"]);
    }

    #[test]
    fn auto_hide_hours_outside_the_screen_limits_cannot_be_saved() {
        let mut page = AutoHidePage { draft: Some(AutoHide { enabled: true, after_minutes: 180 }), ..Default::default() };
        for typed in ["", "0", "169", "três"] {
            page.hours_text = typed.into();
            assert!(page.value().is_none(), "{typed}");
        }
        page.hours_text = "168".into();
        assert_eq!(page.value().map(|v| v.after_minutes), Some(10080));
    }
}
