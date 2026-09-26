//! Contas conhecidas pelo servidor e edição da política de orquestração.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{Page, settings_box};
use super::chrome::Skeleton;
use gpui_kit::component::switch::Switch;
use serde::Deserialize;

const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "kimi", "omp"];

#[derive(Clone, Deserialize)]
struct Policy { provider: String, conta: String, apelido: String, modelos: Vec<String>, trocar: bool }

#[derive(Deserialize)]
struct Account {
    provider: String,
    conta: String,
    apelido: String,
    modelos: Vec<Value>,
    reduced: bool,
}

impl Account {
    fn name(&self) -> &str { if self.apelido.is_empty() { &self.conta } else { &self.apelido } }
}

#[derive(Deserialize)]
struct OrchestrationData {
    arquivo: String,
    mtime: f64,
    #[serde(rename = "politica")]
    policy: Vec<Policy>,
    #[serde(rename = "inventario")]
    accounts: Vec<Account>,
}

struct Draft {
    provider: String,
    conta: String,
    ligada: bool,
    trocar: bool,
    modelos: Vec<String>,
    filter: Entity<InputState>,
    custom: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(super) struct Orchestration {
    data: Remote<OrchestrationData>,
    draft: Option<Draft>,
    saving: bool,
    save_seq: u64,
    error: Option<String>,
    reload_required: bool,
    saved: bool,
}

pub(super) enum OrchestrationReply {
    Loaded(u64, Result<Value, Failure>),
    Saved(u64, Policy, bool, Result<Value, Failure>),
}

fn provider_name(provider: &str) -> &'static str {
    match provider { "codex" => "Codex", "pi" => "Pi", "kimi" => "Kimi", "omp" => "OMP", _ => "Claude" }
}

fn initials(name: &str) -> String {
    let text: String = name.split(|ch: char| ch.is_whitespace() || matches!(ch, '_' | '-' | ':' | '/'))
        .filter(|part| !part.is_empty() && !["e", "and", "de", "da", "do", "&"].iter()
            .any(|word| part.eq_ignore_ascii_case(word)))
        .take(2).filter_map(|part| part.chars().next()).flat_map(char::to_uppercase).collect();
    if text.is_empty() { "?".into() } else { text }
}

// ponytail: nomes de contas locais usam acentos latinos; NFKD completo pede dependência fora desta Task.
fn policy_account_key(value: &str) -> String {
    let mut key = String::new();
    for word in value.trim().trim_matches('`').replace("**", "").split_whitespace() {
        if !key.is_empty() { key.push(' '); }
        for ch in word.chars().flat_map(char::to_lowercase) {
            let plain = match ch {
                'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' => 'i',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
                'ú' | 'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c', 'ñ' => 'n',
                '\u{0300}'..='\u{036f}' => continue,
                _ => ch,
            };
            key.push(plain);
        }
    }
    key
}

impl Hangar {
    pub(super) fn orchestration_opened(&mut self, cx: &mut Context<Self>) {
        if self.orchestration.saving { return; }
        let (seq, save_seq) = (self.orchestration.data.seq, self.orchestration.save_seq + 1);
        self.orchestration = Orchestration::default();
        self.orchestration.data.seq = seq;
        self.orchestration.save_seq = save_seq;
        self.load_orchestration(cx);
    }

    fn load_orchestration(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.orchestration.saving { return; }
        self.orchestration.error = None;
        self.orchestration.reload_required = false;
        self.orchestration.saved = false;
        let seq = self.orchestration.data.start();
        let (tx, connection) = (self.tx.clone(), self.connection);
        self.runtime.spawn(async move {
            let result = api.server_read(&["orquestracao", "politica"], &[], 10).await;
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::Orchestration(OrchestrationReply::Loaded(seq, result)) }).await;
        });
        cx.notify();
    }

    fn choose_orchestration(&mut self, provider: String, conta: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.orchestration.data.ok() else { return };
        if self.orchestration.saving || data.accounts.iter().all(|i| i.provider != provider || i.conta != conta) { return; }
        let policy = data.policy.iter().find(|p| p.provider == provider && policy_account_key(&p.conta) == policy_account_key(&conta));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("orchestration_filter_models")));
        let custom = cx.new(|cx| InputState::new(window, cx).placeholder(tr("orchestration_model_id")));
        let subscriptions = vec![cx.subscribe_in(&filter, window, |_this: &mut Hangar, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) { cx.notify(); }
        }), cx.subscribe_in(&custom, window, |this: &mut Hangar, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) { cx.notify(); }
            if matches!(event, InputEvent::PressEnter { .. }) { this.add_orchestration_model(window, cx); }
        })];
        self.orchestration.draft = Some(Draft { provider, conta, ligada: data.policy.is_empty() || policy.is_some(),
            trocar: policy.is_none_or(|p| p.trocar), modelos: policy.map_or_else(|| vec!["*".into()], |p| p.modelos.clone()),
            filter, custom, _subscriptions: subscriptions });
        self.orchestration.error = None;
        self.orchestration.saved = false;
        cx.notify();
    }

    fn add_orchestration_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.orchestration.draft.as_mut() else { return };
        let id = draft.custom.read(cx).value().trim().to_string();
        if id.is_empty() || id.contains(&['|', '\n', '\r'][..]) || draft.modelos.iter().any(|m| m == &id) { return; }
        if draft.modelos.iter().any(|m| m == "*") {
            draft.modelos = self.orchestration.data.ok().and_then(|data| data.accounts.iter()
                .find(|i| i.provider == draft.provider && i.conta == draft.conta))
                .map(|account| account.modelos.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned)).collect())
                .unwrap_or_default();
        }
        draft.modelos.push(id);
        draft.custom.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn toggle_orchestration_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.orchestration.draft.as_mut() else { return };
        let all = draft.modelos.iter().any(|m| m == "*");
        let catalog: Vec<String> = self.orchestration.data.ok().and_then(|data| data.accounts.iter()
            .find(|i| i.provider == draft.provider && i.conta == draft.conta))
            .map(|account| account.modelos.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned)).collect())
            .unwrap_or_default();
        if all { draft.modelos = catalog.into_iter().filter(|model| model != id).collect(); }
        else if draft.modelos.iter().any(|model| model == id) { draft.modelos.retain(|model| model != id); }
        else { draft.modelos.push(id.to_owned()); }
        cx.notify();
    }

    fn save_orchestration(&mut self, provider: String, conta: String, ligada: bool, trocar: bool,
        modelos: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.orchestration.data.ok() else { return };
        if self.orchestration.saving || self.orchestration.reload_required || self.orchestration.data.loading { return; }
        let Some(account) = data.accounts.iter().find(|i| i.provider == provider && i.conta == conta) else { return };
        let policy = Policy { provider, conta, apelido: account.apelido.clone(),
            modelos: if modelos.is_empty() { vec!["*".into()] } else { modelos }, trocar };
        let last_off = !ligada && (data.policy.is_empty() || data.policy.iter().all(|p|
            p.provider == policy.provider && policy_account_key(&p.conta) == policy_account_key(&policy.conta)));
        let first_on = ligada && data.policy.is_empty();
        if last_off {
            let weak = cx.entity().downgrade();
            chrome::confirm_alert(window, cx, tr("orchestration_unrestricted_title"),
                tr("orchestration_unrestricted_confirm"), tr("orchestration_save"), ButtonVariant::Danger,
                move |_, cx| { let _ = weak.update(cx, |this, cx| this.send_orchestration(policy.clone(), false, cx)); true });
        } else if first_on {
            let weak = cx.entity().downgrade();
            chrome::confirm_alert(window, cx, tr("orchestration_restrict_title"),
                tr("orchestration_restrict_confirm").replace("{n}", &data.accounts.len().saturating_sub(1).to_string()),
                tr("orchestration_save"), ButtonVariant::Danger,
                move |_, cx| { let _ = weak.update(cx, |this, cx| this.send_orchestration(policy.clone(), true, cx)); true });
        } else { self.send_orchestration(policy, ligada, cx); }
    }

    fn send_orchestration(&mut self, policy: Policy, ligada: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(data) = self.orchestration.data.ok() else { return };
        if self.orchestration.saving || self.orchestration.reload_required { return; }
        let body = json!({"provider": policy.provider, "apelido": policy.apelido, "modelos": policy.modelos,
            "trocar": policy.trocar, "ligada": ligada, "mtime": data.mtime});
        self.orchestration.save_seq += 1;
        let seq = self.orchestration.save_seq;
        self.orchestration.saving = true;
        self.orchestration.error = None;
        self.orchestration.saved = false;
        let (tx, connection) = (self.tx.clone(), self.connection);
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::PUT, &["orquestracao", "politica", &policy.conta], Some(body), 10).await;
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::Orchestration(OrchestrationReply::Saved(seq, policy, ligada, result)) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_orchestration(&mut self, reply: OrchestrationReply, cx: &mut Context<Self>) {
        match reply {
            OrchestrationReply::Loaded(seq, result) => {
                let parsed = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v|
                    serde_json::from_value(v).map_err(|_| tr("invalid_response")));
                if !self.orchestration.data.finish(seq, parsed) { return; }
                if let (Some(draft), Some(data)) = (self.orchestration.draft.as_mut(), self.orchestration.data.ok()) {
                    if let Some(policy) = data.policy.iter().find(|p| p.provider == draft.provider && policy_account_key(&p.conta) == policy_account_key(&draft.conta)) {
                        (draft.ligada, draft.trocar, draft.modelos) = (true, policy.trocar, policy.modelos.clone());
                    } else { (draft.ligada, draft.trocar, draft.modelos) = (data.policy.is_empty(), true, vec!["*".into()]); }
                }
            }
            OrchestrationReply::Saved(seq, policy, ligada, result) => {
                if seq != self.orchestration.save_seq { return; }
                self.orchestration.saving = false;
                match result {
                    Ok(value) => match value.get("mtime").and_then(Value::as_f64) {
                        Some(mtime) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
                            if let Some(Ok(data)) = &mut self.orchestration.data.value {
                                data.policy.retain(|p| p.provider != policy.provider || policy_account_key(&p.conta) != policy_account_key(&policy.conta));
                                if ligada { data.policy.push(policy.clone()); }
                                data.mtime = mtime;
                                self.orchestration.data.seq += 1;
                                if let Some(draft) = self.orchestration.draft.as_mut() {
                                    let current = data.policy.iter().find(|p| p.provider == draft.provider &&
                                        policy_account_key(&p.conta) == policy_account_key(&draft.conta));
                                    draft.ligada = data.policy.is_empty() || current.is_some();
                                    draft.trocar = current.is_none_or(|p| p.trocar);
                                    draft.modelos = current.map_or_else(|| vec!["*".into()], |p| p.modelos.clone());
                                }
                                self.orchestration.saved = true;
                            }
                        }
                        _ => { self.orchestration.error = Some(tr("invalid_response")); self.orchestration.reload_required = true; }
                    },
                    Err(error) => {
                        self.orchestration.reload_required = error.status == Some(409) || error.uncertain;
                        self.orchestration.error = Some(if error.status == Some(409) { tr("orchestration_file_changed") }
                            else { Self::fetch_failure(&error) });
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_orchestration(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Orchestration.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let page = div().flex().flex_col().gap_4().child(title)
            .child(self.mark(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("orchestration_intro")), "orchestration_intro"));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if self.orchestration.data.loading {
            return page.child(div().id("orchestration-loading").role(Role::Status).aria_label(tr("loading"))
                .children((0..4usize).map(|i| settings_box().id(("orchestration-skeleton", i))
                    .p_3().mb_2().flex_row().items_center().gap_3()
                    .child(Skeleton::new(("orchestration-avatar", i)).size(px(28.)).rounded_full())
                    .child(div().flex().flex_col().gap_2()
                        .child(Skeleton::new(("orchestration-name", i)).w(px(140.)).h(px(12.)))
                        .child(Skeleton::new(("orchestration-detail", i)).w(px(210.)).h(px(10.)))))))
                .into_any_element();
        }
        let data = match &self.orchestration.data.value {
            Some(Ok(data)) => data,
            Some(Err(error)) => return page
                .child(div().id("orchestration-load-error").role(Role::Alert).text_sm()
                    .text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(Button::new("orchestration-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_orchestration(cx))))
                .into_any_element(),
            None => return page.into_any_element(),
        };
        let mut page = page;
        if data.policy.is_empty() {
            page = page.child(self.mark(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("orchestration_unrestricted")), "orchestration_unrestricted")
                .id("orchestration-unrestricted").role(Role::Status));
        }
        page = page
            .when_some(self.orchestration.error.as_ref(), |el, error| el.child(div().id("orchestration-save-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone())))
            .when(self.orchestration.saved, |el| el.child(div().id("orchestration-saved").role(Role::Status).text_sm()
                .text_color(theme::success()).child(tr("orchestration_saved"))))
            .when(self.orchestration.reload_required, |el| el.child(Button::new("orchestration-reload")
                .outline().small().label(tr("orchestration_reload"))
                .on_click(cx.listener(|this, _, _, cx| this.load_orchestration(cx)))));
        if data.accounts.is_empty() {
            return page.child(div().text_sm().text_color(theme::muted())
                .child(tr("orchestration_empty"))).into_any_element();
        }
        let draft_dirty = self.orchestration.draft.as_ref().is_some_and(|draft| {
            let current = data.policy.iter().find(|p| p.provider == draft.provider &&
                policy_account_key(&p.conta) == policy_account_key(&draft.conta));
            draft.ligada != (data.policy.is_empty() || current.is_some()) || draft.trocar != current.is_none_or(|p| p.trocar)
                || draft.modelos != current.map_or_else(|| vec!["*".into()], |p| p.modelos.clone())
        });
        let mut list = div().w(relative(0.5)).min_w(px(300.)).flex_shrink_0().flex().flex_col().gap_2();
        for provider in PROVIDERS {
            let accounts: Vec<_> = data.accounts.iter().filter(|account| account.provider == provider).collect();
            if accounts.is_empty() { continue; }
            let rows = accounts.into_iter().map(|account| {
                let account_key = policy_account_key(&account.conta);
                let policy = data.policy.iter().find(|entry| entry.provider == account.provider && policy_account_key(&entry.conta) == account_key);
                let forbidden = !data.policy.is_empty() && policy.is_none();
                let selected = self.orchestration.draft.as_ref().is_some_and(|draft| draft.provider == account.provider && draft.conta == account.conta);
                let (provider, conta) = (account.provider.clone(), account.conta.clone());
                let (switch_provider, switch_conta) = (provider.clone(), conta.clone());
                let (switch_trocar, switch_models) = policy.map_or_else(|| (true, vec!["*".into()]),
                    |p| (p.trocar, p.modelos.clone()));
                let status = if data.policy.is_empty() { None }
                    else if forbidden { Some(("orchestration_forbidden", theme::danger())) }
                    else if policy.is_some_and(|entry| !entry.trocar) { Some(("orchestration_locked", theme::warning())) }
                    else { None };
                let mut subtitle = account.conta.clone();
                if !account.modelos.is_empty() {
                    subtitle.push_str(" · ");
                    subtitle.push_str(&if account.modelos.len() == 1 { tr("orchestration_model_one") }
                        else { tr("orchestration_model_many").replace("{n}", &account.modelos.len().to_string()) });
                }
                let body = div().flex().items_center().gap_2().min_w_0().w_full()
                    .child(div().size(px(28.)).flex_shrink_0().rounded_full().bg(theme::raised())
                        .flex().items_center().justify_center().text_xs().font_weight(FontWeight::BOLD)
                        .text_color(if forbidden { theme::muted() } else { theme::provider(&account.provider).0 })
                        .child(initials(account.name())))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().font_weight(FontWeight::MEDIUM).truncate()
                            .when(forbidden, |name| name.text_color(theme::muted()))
                            .child(account.name().to_string()))
                        .child(div().text_sm().text_color(theme::muted()).truncate().child(subtitle)));
                settings_box().p(px(6.)).mb_2().flex_row().items_center().gap_2()
                    .when(forbidden, |row| row.bg(theme::inset()))
                    .when(selected, |row| row.border_color(theme::accent()))
                    .child(Button::new(SharedString::from(format!("orchestration-select-{provider}-{conta}")))
                        .ghost().flex_1().min_w_0().child(body)
                        .disabled(self.orchestration.saving)
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_orchestration(provider.clone(), conta.clone(), window, cx))))
                    .when(account.reduced, |row| row.child(chip(tr("orchestration_reduced"), theme::muted(), theme::raised()).text_xs()))
                    .when_some(status, |row, (key, color)| row.child(chip(tr(key), color, theme::raised()).text_xs()))
                    .child(Switch::new(SharedString::from(format!("orchestration-toggle-{}-{}", account.provider, account.conta)))
                        .checked(!forbidden).disabled(self.orchestration.saving || draft_dirty || self.orchestration.reload_required)
                        .accessibility_label(account.name().to_owned())
                        .on_click(cx.listener(move |this, on: &bool, window, cx| this.save_orchestration(
                            switch_provider.clone(), switch_conta.clone(), *on, switch_trocar,
                            switch_models.clone(), window, cx))))
            });
            list = list.child(div().flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_2().text_sm().font_weight(FontWeight::SEMIBOLD)
                    .child(chrome::provider_glyph(provider, 16.)).child(provider_name(provider)))
                .children(rows));
        }
        let detail = self.render_orchestration_detail(data, cx);
        page = page.child(div().flex().flex_wrap().items_start().gap_4().child(list).child(detail));
        page.into_any_element()
    }

    fn render_orchestration_detail(&self, data: &OrchestrationData, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.orchestration.draft.as_ref() else {
            return div().flex_1().min_w(px(280.)).p_4().text_color(theme::muted())
                .child(tr("orchestration_choose_account")).into_any_element();
        };
        let Some(account) = data.accounts.iter().find(|i| i.provider == draft.provider && i.conta == draft.conta) else {
            return div().into_any_element();
        };
        let all = draft.modelos.iter().any(|m| m == "*");
        let mut models: Vec<(String, String)> = account.modelos.iter().filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_owned();
            Some((id.clone(), m.get("name").and_then(Value::as_str).unwrap_or(&id).to_owned()))
        }).collect();
        for id in &draft.modelos {
            if id != "*" && !models.iter().any(|m| &m.0 == id) { models.push((id.clone(), id.clone())); }
        }
        let filter = draft.filter.read(cx).value().trim().to_lowercase();
        let total = models.len();
        let marked = if all { total } else { draft.modelos.len() };
        let provider = draft.provider.clone();
        let conta = draft.conta.clone();
        let ligada = draft.ligada;
        let trocar = draft.trocar;
        let selected_models = draft.modelos.clone();
        let disabled = self.orchestration.saving || data.mtime.is_nan();
        settings_box().flex_1().min_w(px(280.)).p_4().gap_4()
            .child(div().flex().items_center().gap_3()
                .child(div().size(px(44.)).rounded_full().bg(theme::raised()).flex().items_center().justify_center()
                    .font_weight(FontWeight::BOLD).text_color(theme::provider(&account.provider).0).child(initials(account.name())))
                .child(div().flex().flex_col().min_w_0()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).truncate().child(account.name().to_owned()))
                    .child(div().text_sm().text_color(theme::muted()).truncate()
                        .child(format!("{} · {}", provider_name(&account.provider), account.conta)))))
            .child(div().flex().items_center().justify_between().gap_2()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(tr("orchestration_can_use"))
                    .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("orchestration_can_use_help"))))
                .child(Checkbox::new("orchestration-enabled").checked(ligada).disabled(disabled)
                    .accessibility_label(tr("orchestration_can_use"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(draft) = this.orchestration.draft.as_mut() { draft.ligada = *on; }
                        cx.notify();
                    }))))
            .child(div().flex().items_center().justify_between().gap_2()
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(tr("orchestration_can_switch"))
                    .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("orchestration_can_switch_help"))))
                .child(Checkbox::new("orchestration-switch").checked(trocar).disabled(disabled || !ligada)
                    .accessibility_label(tr("orchestration_can_switch"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(draft) = this.orchestration.draft.as_mut() { draft.trocar = *on; }
                        cx.notify();
                    }))))
            .child(div().flex().flex_col().gap_2()
                .child(div().flex().flex_wrap().items_center().justify_between().gap_2()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM)
                        .child(tr("orchestration_allowed_models").replace("{n}", &marked.to_string()).replace("{total}", &total.to_string())))
                    .child(div().flex().gap_1()
                        .child(Button::new("orchestration-all").ghost().small().label(tr("orchestration_mark_all"))
                            .disabled(disabled || !ligada).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(draft) = this.orchestration.draft.as_mut() { draft.modelos = vec!["*".into()]; }
                                cx.notify();
                            })))
                        .child(Button::new("orchestration-clear").ghost().small().label(tr("orchestration_clear"))
                            .disabled(disabled || !ligada).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(draft) = this.orchestration.draft.as_mut() { draft.modelos.clear(); }
                                cx.notify();
                            })))))
                .when(total > 6, |el| el.child(Input::new(&draft.filter).small().disabled(disabled || !ligada)
                    .aria_label(tr("orchestration_filter_models"))))
                .child(div().id("orchestration-models").max_h(px(300.)).overflow_y_scroll().flex().flex_col()
                    .children(models.into_iter().filter(|(id, name)| filter.is_empty() ||
                        id.to_lowercase().contains(&filter) || name.to_lowercase().contains(&filter)).map(|(id, name)| {
                        let checked = all || draft.modelos.iter().any(|m| m == &id);
                        Checkbox::new(SharedString::from(format!("orchestration-model-{id}"))).label(name)
                            .checked(checked).disabled(disabled || !ligada)
                            .on_click(cx.listener(move |this, _: &bool, _, cx| this.toggle_orchestration_model(&id, cx)))
                    }))))
            .when(account.reduced, |el| el.child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("orchestration_reduced_help"))))
            .child(div().flex().items_center().gap_2()
                .child(Input::new(&draft.custom).small().flex_1().disabled(disabled || !ligada)
                    .aria_label(tr("orchestration_model_id")))
                .child(Button::new("orchestration-add-model").outline().small().label(tr("orchestration_add"))
                    .disabled(disabled || !ligada || draft.custom.read(cx).value().trim().is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.add_orchestration_model(window, cx)))))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                .child(tr("orchestration_policy_path").replace("{arquivo}", &data.arquivo)))
            .child(Button::new("orchestration-save").primary().label(if self.orchestration.saving {
                tr("orchestration_saving") } else { tr("orchestration_save") })
                .disabled(disabled || self.orchestration.reload_required)
                .on_click(cx.listener(move |this, _, window, cx| this.save_orchestration(
                    provider.clone(), conta.clone(), ligada, trocar, selected_models.clone(), window, cx))))
            .into_any_element()
    }
}
