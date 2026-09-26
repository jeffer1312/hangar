//! Contas conhecidas pelo servidor e a política de orquestração, somente leitura nesta página.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{Page, settings_box};
use super::chrome::Skeleton;
use serde::Deserialize;

const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "kimi", "omp"];

#[derive(Deserialize)]
struct Policy { provider: String, conta: String, trocar: bool }

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
    #[serde(rename = "politica")]
    policy: Vec<Policy>,
    #[serde(rename = "inventario")]
    accounts: Vec<Account>,
}

#[derive(Default)]
pub(super) struct Orchestration { data: Remote<OrchestrationData> }

pub(super) struct OrchestrationReply(u64, Result<Value, Failure>);

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
        let seq = self.orchestration.data.seq;
        self.orchestration = Orchestration::default();
        self.orchestration.data.seq = seq;
        self.load_orchestration(cx);
    }

    fn load_orchestration(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.orchestration.data.start();
        let (tx, connection) = (self.tx.clone(), self.connection);
        self.runtime.spawn(async move {
            let result = api.server_read(&["orquestracao", "politica"], &[], 10).await;
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::Orchestration(OrchestrationReply(seq, result)) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_orchestration(&mut self, OrchestrationReply(seq, result): OrchestrationReply,
        cx: &mut Context<Self>) {
        let result = result.map_err(|error| Self::fetch_failure(&error)).and_then(|value| {
            serde_json::from_value(value).map_err(|_| tr("invalid_response"))
        });
        if self.orchestration.data.finish(seq, result) { cx.notify(); }
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
        if data.accounts.is_empty() {
            return page.child(div().text_sm().text_color(theme::muted())
                .child(tr("orchestration_empty"))).into_any_element();
        }
        for provider in PROVIDERS {
            let accounts: Vec<_> = data.accounts.iter().filter(|account| account.provider == provider).collect();
            if accounts.is_empty() { continue; }
            let rows = accounts.into_iter().map(|account| {
                let account_key = policy_account_key(&account.conta);
                let policy = data.policy.iter().find(|entry| entry.provider == account.provider && policy_account_key(&entry.conta) == account_key);
                let forbidden = !data.policy.is_empty() && policy.is_none();
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
                settings_box()
                    .p_3().mb_2().flex_row().items_center().gap_3().when(forbidden, |row| row.bg(theme::inset()))
                    .child(div().size(px(28.)).flex_shrink_0().rounded_full().bg(theme::raised())
                        .flex().items_center().justify_center().text_xs().font_weight(FontWeight::BOLD)
                        .text_color(if forbidden { theme::muted() } else { theme::provider(&account.provider).0 })
                        .child(initials(account.name())))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().font_weight(FontWeight::MEDIUM).truncate()
                            .when(forbidden, |name| name.text_color(theme::muted()))
                            .child(account.name().to_string()))
                        .child(div().text_sm().text_color(theme::muted()).truncate().child(subtitle)))
                    .when(account.reduced, |row| row.child(chip(tr("orchestration_reduced"), theme::muted(), theme::raised()).text_xs()))
                    .when_some(status, |row, (key, color)| row.child(chip(tr(key), color, theme::raised()).text_xs()))
            });
            page = page.child(div().flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_2().text_sm().font_weight(FontWeight::SEMIBOLD)
                    .child(chrome::provider_glyph(provider, 16.)).child(provider_name(provider)))
                .children(rows));
        }
        page.into_any_element()
    }
}
