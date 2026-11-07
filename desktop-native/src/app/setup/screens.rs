//! As telas do assistente (mock `2026-10-06-instalador-grafico-mock.html`): lateral com as etapas, conteúdo rolável e rodapé
//! com o progresso, Voltar e a ação principal. Cores de `theme`, medidas pelos helpers rem do GPUI.
use super::*;
use super::codes::{self, Fix};
use super::failure::{self, AgentPanel, AgentPhase, Failure, OnFailureAction, PanelView, SendState};
use super::report;
use super::flow::{self, PasswordMode, PasswordProblem, PhoneOutcome, Primary, Screen, Status};
use super::marks::{End, ItemRow, State, Step};
use super::phone::Qr;
use super::precheck::{self, Check, CheckRow};
use super::system::AGENTS;
use super::wizard::{AppCopy, SetupWizard};
use gpui_kit::component::{progress::Progress as Bar, stepper::{Stepper, StepperItem}};
use std::rc::Rc;

/// Itens do Windows que o script não liga sozinho quando a regra está travada: o botão abre a página onde a pessoa liga
/// (Windows 11: "Para desenvolvedores" tem o Modo Desenvolvedor e a permissão de scripts do PowerShell).
const WINDOWS_SETTINGS_ITEMS: [&str; 2] = ["politica-scripts", "modo-desenvolvedor"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowMark { Ok, Doing, Queued, Warn, Failed }

impl From<State> for RowMark {
    fn from(state: State) -> Self {
        match state { State::Ok => Self::Ok, State::Doing => Self::Doing, State::Queued => Self::Queued, State::Pending => Self::Warn, State::Failed => Self::Failed }
    }
}

fn row_mark(id: &str, mark: RowMark) -> AnyElement {
    let slot = div().size_5().flex_shrink_0().rounded_full().flex().items_center().justify_center();
    match mark {
        RowMark::Ok => slot.bg(theme::success().opacity(0.12)).child(chrome::small_icon(IconName::Check, 12., theme::success())).into_any_element(),
        RowMark::Doing => slot.child(chrome::Spinner::new(SharedString::from(format!("setup-spin-{id}")), IconName::LoaderCircle, px(14.), theme::accent())).into_any_element(),
        RowMark::Queued => slot.border_1().border_color(theme::border()).into_any_element(),
        RowMark::Warn => slot.bg(theme::warning().opacity(0.12)).child(chrome::small_icon(IconName::CircleAlert, 12., theme::warning())).into_any_element(),
        RowMark::Failed => slot.bg(theme::danger().opacity(0.12)).child(chrome::small_icon(IconName::CircleAlert, 12., theme::danger())).into_any_element(),
    }
}

/// Uma linha das listas (mock `.row`): estado, nome com descrição e o rótulo ou botão à direita.
pub(crate) fn row(id: &str, mark: RowMark, name: String, detail: Option<String>, trailing: Option<AnyElement>) -> Stateful<Div> {
    div().id(SharedString::from(format!("setup-row-{id}"))).flex().items_center().gap_3().px_4().py_2p5()
        .child(row_mark(id, mark))
        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).whitespace_normal().child(name))
            .when_some(detail, |el, detail| el.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(detail))))
        .children(trailing)
}

/// O cartão das listas: um fio entre as linhas, nunca dois.
pub(crate) fn card(rows: Vec<Stateful<Div>>) -> Div {
    div().flex().flex_col().rounded_lg().border_1().border_color(theme::border()).bg(theme::boxed())
        .children(rows.into_iter().enumerate().map(|(ix, row)| row.when(ix > 0, |row| row.border_t_1().border_color(theme::border()))))
}

fn trailing_text(text: String) -> AnyElement {
    div().flex_shrink_0().text_xs().font_family(theme::MONO).text_color(theme::faint()).child(text).into_any_element()
}

fn state_label(state: State) -> String {
    match state {
        State::Queued => tr("setup_item_queued"), State::Doing => tr("setup_item_doing"), State::Ok => tr("setup_item_ok"),
        State::Pending => tr("setup_item_pending"), State::Failed => tr("setup_item_failed"),
    }
}

fn item_row(item: &ItemRow) -> Stateful<Div> {
    let name = if item.text.is_empty() { item.id.clone() } else { item.text.clone() };
    let needs_settings = cfg!(windows) && WINDOWS_SETTINGS_ITEMS.contains(&item.id.as_str()) && matches!(item.state, State::Failed | State::Pending);
    let trailing = if needs_settings {
        Button::new(SharedString::from(format!("setup-item-open-{}", item.id))).outline().small().icon(IconName::ExternalLink)
            .label(tr("setup_item_open_settings")).on_click(|_, _, cx| cx.open_url("ms-settings:developers")).into_any_element()
    } else { trailing_text(state_label(item.state)) };
    row(&item.id, item.state.into(), name, None, Some(trailing))
}

fn check_id(check: Check) -> &'static str {
    match check { Check::Git => "git", Check::Curl => "curl", Check::Internet => "internet", Check::Space => "space",
        Check::Winget => "winget", Check::Pkg => "pkg", Check::Sudo => "sudo", Check::Internal => "internal" }
}

fn check_row(check: &CheckRow) -> Stateful<Div> {
    let name = match check.check {
        Check::Git => tr("setup_check_git"), Check::Curl => tr("setup_check_curl"), Check::Internet => tr("setup_check_internet"),
        Check::Space => tr("setup_check_space").replace("{gb}", &(precheck::MIN_FREE / precheck::GB).to_string()),
        Check::Winget => tr("setup_check_winget"), Check::Pkg => tr("setup_check_pkg"), Check::Sudo => tr("setup_check_sudo"),
        Check::Internal => tr("setup_check_internal"),
    };
    let mark = if check.ok { RowMark::Ok } else if check.blocking { RowMark::Failed } else { RowMark::Warn };
    let label = if check.ok { tr("setup_check_found") }
        else if check.check == Check::Internal { tr("setup_check_failed") } else { tr("setup_check_missing") };
    // Bloqueio com código leva o passo a passo e o botão da tabela (spec: "bloqueio vira item com a frase e a saída").
    let failed = check.code.filter(|_| !check.ok);
    let detail = [(!check.detail.is_empty()).then(|| check.detail.clone()), failed.and_then(codes::help)].into_iter().flatten()
        .collect::<Vec<_>>().join("\n");
    let trailing = match failed {
        Some("sem-winget") => Button::new("setup-check-store").outline().small().icon(IconName::ExternalLink).label(Fix::OpenStore.label())
            .on_click(|_, _, cx| cx.open_url(codes::STORE_URL)).into_any_element(),
        _ => trailing_text(label),
    };
    row(check_id(check.check), mark, name, (!detail.is_empty()).then_some(detail), Some(trailing))
}

fn status_icon(status: Status) -> Option<IconName> {
    match status { Status::Done => Some(IconName::Check), Status::Doing => Some(IconName::LoaderCircle), Status::NeedsYou => Some(IconName::CircleAlert), Status::Pending => None }
}

fn screen_names(screen: Screen) -> (String, String) {
    match screen {
        Screen::Welcome => (tr("setup_step_welcome"), tr("setup_step_welcome_sub")),
        Screen::Prepare => (tr("setup_step_prepare"), tr("setup_step_prepare_sub")),
        Screen::Install => (tr("setup_step_install"), tr("setup_step_install_sub")),
        Screen::Tailscale => (tr("setup_step_tailscale"), tr("setup_step_tailscale_sub")),
        Screen::Phone => (tr("setup_step_phone"), tr("setup_step_phone_sub")),
        Screen::Done => (tr("setup_step_done"), tr("setup_step_done_sub")),
    }
}

fn screen_copy(screen: Screen) -> (String, String) {
    match screen {
        Screen::Welcome => (tr("setup_welcome_title"), tr("setup_welcome_lead")),
        Screen::Prepare => (tr("setup_prepare_title"), tr("setup_prepare_lead")),
        Screen::Install => (tr("setup_install_title"), tr("setup_install_lead")),
        Screen::Tailscale => (tr("setup_tailscale_title"), tr("setup_tailscale_lead")),
        Screen::Phone => (tr("setup_phone_title"), tr("setup_phone_lead")),
        Screen::Done => (tr("setup_done_title"), tr("setup_done_lead")),
    }
}

fn header(eyebrow: String, title: String, lead: String) -> Div {
    div().flex().flex_col().gap_2()
        .child(div().text_xs().font_family(theme::MONO).text_color(theme::accent_text()).child(eyebrow))
        .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_base().text_color(theme::muted()).max_w(rems(36.)).whitespace_normal().child(lead))
}

fn section_head(title: String, hint: String) -> Div {
    div().flex().items_center().justify_between().gap_4()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_xs().text_color(theme::faint()).child(hint))
}

fn muted_line(id: &'static str, spinner: bool, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Status).flex().items_center().gap_2().text_sm().text_color(theme::muted())
        .when(spinner, |el| el.child(chrome::Spinner::new(SharedString::from(format!("{id}-spin")), IconName::LoaderCircle, px(14.), theme::muted())))
        .child(text)
}

pub(super) const TAILSCALE_DNS: &str = "https://login.tailscale.com/admin/dns";

/// Aviso que pede a pessoa (mock `.callout`): ícone, título, linhas e as ações.
fn callout(id: &'static str, title: String, lines: Vec<String>, actions: Vec<AnyElement>) -> Stateful<Div> {
    div().id(id).role(Role::Alert).flex().gap_3().p_4().rounded_lg().border_1().border_color(theme::warning().opacity(0.3)).bg(theme::warning().opacity(0.06))
        .child(chrome::small_icon(IconName::CircleAlert, 16., theme::warning()))
        .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .children(lines.into_iter().map(|line| div().text_sm().text_color(theme::muted()).whitespace_normal().child(line)))
            .child(div().flex().flex_wrap().gap_2().pt_1().children(actions)))
}

fn tips(tailscale: bool) -> Div {
    let first = if tailscale { tr("setup_phone_tip_tailscale") } else { tr("setup_phone_tip_lan") };
    div().flex().flex_col().gap_2().children([first, tr("setup_phone_tip_scan"), tr("setup_phone_tip_done")].into_iter().enumerate()
        .map(|(n, text)| div().flex().items_start().gap_2()
            .child(div().size_5().flex_shrink_0().rounded_md().bg(theme::inset()).flex().items_center().justify_center()
                .text_xs().font_family(theme::MONO).text_color(theme::muted()).child((n + 1).to_string()))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(text))))
}

fn connect_offer() -> Div { div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("setup_phone_connect")) }

fn recheck_button(id: &'static str, cx: &mut Context<SetupWizard>) -> AnyElement {
    Button::new(id).outline().small().label(tr("setup_recheck")).on_click(cx.listener(|w, _, window, cx| w.retry(window, cx))).into_any_element()
}

impl SetupWizard {
    /// O aviso de login da Tailscale: o script pede o login ainda na preparação, então ele aparece em qualquer tela
    /// enquanto o link espera e a conta não conectou.
    fn login_notice(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let latest = self.runs.latest()?;
        let account_marked = self.runs.items(Step::Tailscale).iter().any(|i| i.id == "tailscale-conta" && i.state == State::Ok);
        if self.tailscale_running || account_marked { return None; }
        let url = latest.link("tailscale-login")?.to_owned();
        let mut actions = vec![Button::new("setup-tailscale-login-open").primary().small().icon(IconName::ExternalLink).label(tr("setup_tailscale_login"))
            .on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()];
        // Passou o teto do script sem login: "Conferir de novo" roda o script de novo, só depois do FIM (a pendência não o para).
        if self.runs.end().is_some() && (latest.pendings.iter().any(|(c, _)| c == "tailscale-login") || latest.error.as_deref() == Some("tailscale-login")) {
            actions.push(recheck_button("setup-tailscale-login-recheck", cx));
        }
        Some(callout("setup-tailscale-login", tr("setup_tailscale_login"), vec![tr("setup_tailscale_login_hint")], actions))
    }

    fn failure_handler(&self, cx: &mut Context<Self>) -> OnFailureAction {
        let this = cx.entity().downgrade();
        Rc::new(move |action, window, cx| { let _ = this.update(cx, |w, cx| w.failure_action(action, window, cx)); })
    }

    fn panel_view(&self) -> PanelView<'_> {
        let agent = self.agent.as_ref().map(|run| AgentPanel { agent: run.agent, phase: &run.phase,
            explanation: run.transcript.explanation.as_deref(), activity: &run.transcript.lines,
            notes: run.restored.as_ref().map(|r| report::restore_notes(r.head_moved.as_ref(), &r.errors, run.timed_out)).unwrap_or_default(),
            not_stopped: run.not_stopped });
        // O aviso da correção só aparece depois que o envio dela saiu de verdade.
        let fix_sent = self.fix_sent.is_some_and(|id| self.outbox.items.iter().any(|(n, state, _)| *n == id && *state == SendState::Sent));
        // Sem internet o agente também não roda: a oferta só daria um erro de API.
        let offline = self.failure.as_ref().is_some_and(|f| f.code.as_deref() == Some("sem-internet"));
        PanelView { report: self.report.as_deref(), send: self.send, locked: self.report_about.is_none(), refreshing: self.refreshing,
            agents: if offline { &[] } else { &self.agents_ready }, agent, fix_sent }
    }

    /// A abertura desfez um conserto que o app interrompeu ao cair: diz quais arquivos voltaram, nunca calado.
    fn recovered_notice(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let recovery = self.recovered.as_ref()?;
        let restored = &recovery.restored;
        let failed = !restored.errors.is_empty();
        let mut lines: Vec<String> = Vec::new();
        match &recovery.offer {
            Some(Ok(files)) => lines.push(tr("setup_agent_recover_offer").replace("{arquivos}", &files.join(", "))),
            Some(Err(why)) => lines.push(tr("setup_agent_recover_offer_unknown").replace("{erro}", why)),
            None if recovery.skipped => lines.push(tr("setup_agent_recover_skipped")),
            None => {}
        }
        if !restored.changed.is_empty() { lines.push(tr("setup_agent_recovered").replace("{arquivos}", &restored.changed.join(", "))); }
        // O que não voltou fica listado (`restore_notes`), e fechar o aviso pára as tentativas sozinhas.
        if failed { lines.push(tr("setup_agent_recover_failed")); }
        lines.extend(report::restore_notes(restored.head_moved.as_ref(), &restored.errors, false));
        if recovery.not_stopped { lines.push(tr("setup_agent_not_stopped")); }
        let title = if failed { tr("setup_agent_recover_failed_title") }
            else if recovery.skipped { tr("setup_agent_recover_skipped_title") } else { tr("setup_agent_recovered_title") };
        let close = Button::new("setup-agent-recovered-close").ghost().small().label(tr("close"))
            .on_click(cx.listener(|w, _, _, cx| w.dismiss_recovered(cx)));
        // Com um agente novo chamado, a anotação já é a dele: o botão sumiria com o conserto errado.
        let undo = (recovery.offer.is_some() && self.agent.is_none()).then(|| Button::new("setup-agent-undo-anyway").outline().small()
            .label(tr("setup_agent_recover_undo")).on_click(cx.listener(|w, _, _, cx| w.undo_anyway(cx))).into_any_element());
        Some(callout("setup-agent-recovered", title, lines, undo.into_iter().chain([close.into_any_element()]).collect()))
    }

    fn render_details(&self, id: &'static str, cx: &mut Context<Self>) -> Div {
        // Com a falha na tela, "Ver detalhes" mora no painel dela.
        if self.failure.as_ref().is_some_and(|f| f.screen == self.viewing) { return div(); }
        let lines: Vec<String> = self.runs.latest().map(|p| p.lines().cloned().collect()).unwrap_or_default();
        failure::details(id, self.details_open, &lines, cx.listener(|w, _, _, cx| { w.details_open = !w.details_open; cx.notify(); }))
    }

    fn render_side(&self, list: &[Screen], cx: &mut Context<Self>) -> Div {
        let view = self.view();
        let viewing = list.iter().position(|s| *s == self.viewing).unwrap_or(0);
        let items: Vec<StepperItem> = list.iter().map(|screen| {
            let status = flow::status(*screen, &view);
            let (label, sub) = screen_names(*screen);
            // O estado vai em palavra além do ícone: cor nunca sozinha.
            let sub = match status {
                Status::NeedsYou => tr("setup_state_needs_you"),
                Status::Doing if *screen != self.viewing => tr("setup_state_doing"),
                _ => sub,
            };
            StepperItem::new().disabled(!flow::reachable(list, *screen, &view))
                .when_some(status_icon(status), |item, icon| item.icon(icon))
                .child(div().flex().flex_col().gap_0p5()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                    .child(div().text_xs().text_color(if status == Status::NeedsYou { theme::warning_text() } else { theme::muted() }).child(sub)))
        }).collect();
        let (steps, this) = (list.to_vec(), cx.entity().downgrade());
        div().w_64().flex_shrink_0().h_full().flex().flex_col().gap_6().px_4().py_6().border_r_1().border_color(theme::border())
            .child(div().flex().items_center().gap_3().px_2()
                .child(svg().path(crate::HANGAR_MARK).size_8().text_color(theme::text()))
                .child(div().flex().flex_col()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Hangar"))
                    .child(div().text_xs().text_color(theme::faint()).child(tr("setup_brand")))))
            .child(Stepper::new("setup-steps").vertical().selected_index(viewing).items(items).on_click(move |ix, _, cx| {
                let Some(screen) = steps.get(*ix).copied() else { return };
                let _ = this.update(cx, |w, cx| if flow::reachable(&steps, screen, &w.view()) { w.go(screen, cx) });
            }))
            // Versão numa linha própria: na mesma linha do "Fechar" ela não encolhe e encosta no botão.
            .child(div().mt_auto().pt_3().border_t_1().border_color(theme::border()).flex().flex_col().items_start().gap_2()
                .child(div().text_xs().font_family(theme::MONO).text_color(theme::faint())
                    .child(format!("v{} · {}-{}", crate::update::CURRENT, std::env::consts::OS, std::env::consts::ARCH)))
                .child(Button::new("setup-close").ghost().xsmall().label(tr("setup_close")).on_click(cx.listener(|w, _, window, cx| w.close(window, cx)))))
    }

    fn render_footer(&self, list: &[Screen], cx: &mut Context<Self>) -> Div {
        let view = self.view();
        let ix = list.iter().position(|s| *s == self.viewing).unwrap_or(0);
        let total = list.len();
        let meta = if flow::status(Screen::Done, &view) == Status::Done { tr("setup_footer_finished") }
            else { tr("setup_footer_meta").replace("{n}", &(ix + 1).to_string()).replace("{total}", &total.to_string()) };
        let pct = ix as f32 * 100. / total.saturating_sub(1).max(1) as f32;
        let enabled = flow::primary_enabled(self.viewing, &view, self.can_start(cx));
        let label = match flow::primary(self.viewing, self.started) {
            Primary::Start => tr("setup_start"), Primary::Continue => tr("setup_continue"),
            Primary::PhoneConnected => tr("setup_phone_done"), Primary::OpenHangar => tr("setup_open_hangar"),
        };
        div().flex().items_center().gap_4().px_6().py_3().border_t_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().flex().items_center().gap_3()
                .child(div().w_32().child(Bar::new("setup-progress").value(pct).accessibility_label(meta.clone())))
                .child(div().text_xs().text_color(theme::faint()).child(meta)))
            .when(ix > 0, |el| el.child(Button::new("setup-back").ghost().label(tr("setup_back")).on_click(cx.listener(|w, _, _, cx| w.back(cx)))))
            .when(self.viewing == Screen::Phone, |el| el.child(Button::new("setup-phone-skip").outline().label(tr("setup_phone_skip")).disabled(!enabled)
                .on_click(cx.listener(|w, _, _, cx| w.finish_phone(PhoneOutcome::Skipped, cx)))))
            .child(Button::new("setup-primary").primary().label(label).disabled(!enabled).on_click(cx.listener(|w, _, window, cx| w.press_primary(window, cx))))
    }

    fn render_screen(&self, list: &[Screen], cx: &mut Context<Self>) -> Div {
        let n = list.iter().position(|s| *s == self.viewing).unwrap_or(0) + 1;
        let head = if self.viewing == Screen::Done { self.done_header() } else {
            let (title, lead) = screen_copy(self.viewing);
            header(tr("setup_eyebrow").replace("{n}", &n.to_string()).replace("{total}", &list.len().to_string()), title, lead)
        };
        let busy = self.agent.as_ref().is_some_and(|run| run.busy());
        let failure = self.failure.as_ref().filter(|f| f.screen == self.viewing).map(|f| {
            let mut lines: Vec<String> = self.runs.latest().map(|p| p.lines().cloned().collect()).unwrap_or_default();
            // Cada comando do agente aparece em "Ver detalhes" (spec), depois da saída do script.
            if let Some(run) = &self.agent {
                lines.push(format!("— {} —", run.agent.name()));
                lines.extend(run.transcript.lines.iter().cloned());
            }
            // Agente trabalhando: os botões da frase somem, nada sobe na pasta que ele edita.
            let shown = if busy { Failure { fixes: Vec::new(), ..f.clone() } } else { f.clone() };
            failure::failure_panel(&shown, self.details_open, &lines, self.failure_handler(cx))
        });
        let body = match self.viewing {
            Screen::Welcome => self.render_welcome(cx),
            Screen::Prepare => self.render_prepare(cx),
            Screen::Install => self.render_install(cx),
            Screen::Tailscale => self.render_tailscale(cx),
            Screen::Phone => self.render_phone(cx),
            Screen::Done => self.render_done(cx),
        };
        // Na tela 4 o aviso já vem no corpo; nas outras ele sobe enquanto o script espera o login.
        let login = (self.viewing != Screen::Tailscale && self.runs.end().is_none()).then(|| self.login_notice(cx)).flatten();
        // O relatório fica logo abaixo da falha: a pessoa lê o que sai antes de decidir.
        let after = failure.is_some().then(|| failure::after_panel(&self.panel_view(), self.failure_handler(cx)));
        // A reconferência roda nas telas do script, sem o painel da falha: o andamento do agente sobe junto.
        let rechecking = (failure.is_none() && self.agent.as_ref().is_some_and(|run| run.phase == AgentPhase::Rechecking))
            .then(|| failure::agent_block(&self.panel_view(), self.failure_handler(cx))).flatten();
        // O envio continua visível depois de sair da falha, até sair ou a pessoa fechar o aviso.
        let outbox = failure::outbox_lines(&self.outbox, self.failure_handler(cx));
        let recovered = self.recovered_notice(cx);
        // O envio fica no alto: depois de "Tentar de novo" a falha nova pode ser idêntica, e a linha é o sinal de que algo saiu.
        div().flex().flex_col().gap_6().child(head).children(outbox).children(recovered).children(failure).children(after)
            .children(rechecking).children(login).child(body)
    }

    fn agent_tile(&self, id: &'static str, name: &'static str, cx: &mut Context<Self>) -> Button {
        let on = self.agents.contains(&id);
        let (state, color) = if self.installed.contains(&id) { (tr("setup_agent_installed"), theme::success_text()) }
            else { (tr("setup_agent_will_install"), theme::muted()) };
        Button::new(SharedString::from(format!("setup-agent-{id}")))
            .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { theme::boxed() }).foreground(theme::text())
                .hover(theme::hover()).active(theme::hover()))
            .w_40().h_auto().p_3().rounded_lg().toggled(on).disabled(self.started).accessibility_label(format!("{name}: {state}"))
            .child(div().w_full().flex().flex_col().items_start().gap_2()
                .child(div().w_full().flex().items_center().justify_between()
                    .child(chrome::provider_glyph(id, 28.))
                    .child(if on { chrome::small_icon(IconName::Check, 14., theme::accent_text()).into_any_element() }
                        else { div().size_4().rounded_sm().border_1().border_color(theme::border()).into_any_element() }))
                .child(div().flex().items_center().gap_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(name)
                    .when(id == "claude", |el| el.child(div().text_xs().font_family(theme::MONO).text_color(theme::accent_text()).child(tr("setup_agents_default")))))
                .child(div().text_xs().text_color(color).child(state)))
            .on_click(cx.listener(move |w, _, _, cx| w.toggle_agent(id, cx)))
    }

    fn access_tile(&self, id: &'static str, outside: bool, icon: IconName, title: String, desc: String, cx: &mut Context<Self>) -> Button {
        let on = self.outside == outside;
        Button::new(id)
            .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { theme::boxed() }).foreground(theme::text())
                .hover(theme::hover()).active(theme::hover()))
            .flex_1().h_auto().p_4().rounded_lg().toggled(on).disabled(self.started).accessibility_label(format!("{title}. {desc}"))
            .child(div().w_full().flex().flex_col().items_start().gap_2()
                .child(chrome::small_icon(icon, 18., if on { theme::accent_text() } else { theme::muted() }))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(desc)))
            .on_click(cx.listener(move |w, _, _, cx| w.set_outside(outside, cx)))
    }

    fn render_password(&self, cx: &mut Context<Self>) -> Div {
        let head = section_head(tr("setup_password_title"), tr("setup_password_hint"));
        if self.password_mode == PasswordMode::Keep {
            return div().flex().flex_col().gap_3().child(head)
                .child(card(vec![row("password-kept", RowMark::Ok, tr("setup_password_kept"), Some(tr("setup_password_kept_hint")), None)]));
        }
        // O aviso só aparece depois de começar a digitar: campo vazio não é erro ainda.
        let typed = !self.password.read(cx).value().is_empty();
        let problem = self.password_problem(cx).filter(|_| typed);
        let field = |label: String, input: &Entity<InputState>| div().flex_1().flex().flex_col().gap_1()
            .child(div().text_sm().child(label.clone())).child(Input::new(input).aria_label(label));
        div().flex().flex_col().gap_3().child(head)
            .child(div().flex().gap_4()
                .child(Radio::new("setup-password-generate").label(tr("setup_password_generate")).checked(self.password_mode == PasswordMode::Generate)
                    .disabled(self.started).on_click(cx.listener(|w, _, _, cx| w.set_password_mode(PasswordMode::Generate, cx))))
                .child(Radio::new("setup-password-choose").label(tr("setup_password_choose")).checked(self.password_mode == PasswordMode::Choose)
                    .disabled(self.started).on_click(cx.listener(|w, _, _, cx| w.set_password_mode(PasswordMode::Choose, cx)))))
            .when(self.password_mode == PasswordMode::Choose, |el| el
                .child(div().flex().gap_3().child(field(tr("setup_password_field"), &self.password)).child(field(tr("setup_password_confirm"), &self.confirm)))
                .when_some(problem, |el, problem| el.child(div().id("setup-password-problem").role(Role::Alert).text_sm().text_color(theme::warning_text())
                    .child(match problem {
                        PasswordProblem::Short => tr("setup_password_short"),
                        PasswordProblem::Forbidden => tr("setup_password_forbidden"),
                        PasswordProblem::Mismatch => tr("setup_password_mismatch"),
                    }))))
    }

    fn render_precheck_summary(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.precheck {
            None => Some(muted_line("setup-checking", true, tr("setup_check_running")).into_any_element()),
            Some(rows) if precheck::blocked(rows) => Some(div().id("setup-blocked").role(Role::Alert).flex().flex_col().gap_2()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning_text()).child(tr("setup_check_blocked")))
                .child(card(rows.iter().filter(|r| r.blocking).map(check_row).collect()))
                .child(div().child(Button::new("setup-recheck").outline().small().label(tr("setup_check_again"))
                    .on_click(cx.listener(|w, _, window, cx| w.recheck(window, cx)))))
                .into_any_element()),
            Some(rows) if precheck::needs_git(rows) => Some(muted_line("setup-git-note", false, tr("setup_check_git_missing")).into_any_element()),
            Some(_) => None,
        }
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> Div {
        let tiles: Vec<Button> = AGENTS.iter().map(|&(id, name)| self.agent_tile(id, name, cx)).collect();
        let home = self.access_tile("setup-access-home", false, IconName::Wifi, tr("setup_access_home"), tr("setup_access_home_desc"), cx);
        let away = self.access_tile("setup-access-outside", true, IconName::Globe, tr("setup_access_outside"), tr("setup_access_outside_desc"), cx);
        div().flex().flex_col().gap_6()
            .child(div().flex().flex_col().gap_3()
                .child(section_head(tr("setup_agents_title"), tr("setup_agents_hint")))
                .child(div().flex().flex_wrap().gap_2().children(tiles))
                .when(self.agents.is_empty(), |el| el.child(div().id("setup-agents-need").role(Role::Alert).text_sm().text_color(theme::warning_text())
                    .child(tr("setup_agents_need")))))
            .child(div().flex().flex_col().gap_3()
                .child(section_head(tr("setup_access_title"), tr("setup_access_hint")))
                .child(div().flex().gap_3().child(home).child(away)))
            .child(self.render_password(cx))
            .children(self.render_precheck_summary(cx))
    }

    fn app_rows(&self, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let mut rows = Vec::new();
        match &self.app_copy {
            Some(AppCopy::Done(path)) => rows.push(row("app-copy", RowMark::Ok, tr("setup_app_copy"), Some(path.display().to_string()), None)),
            Some(AppCopy::Skipped) => rows.push(row("app-copy", RowMark::Warn, tr("setup_app_copy"), Some(tr("setup_app_copy_skipped")), None)),
            Some(AppCopy::Failed(why)) => rows.push(row("app-copy", RowMark::Failed, tr("setup_app_copy"), Some(tr("setup_app_copy_failed").replace("{erro}", why)),
                Some(Button::new("setup-app-copy-retry").outline().small().label(tr("setup_retry"))
                    .on_click(cx.listener(|w, _, _, cx| w.copy_app(cx))).into_any_element()))),
            None if self.finished => rows.push(row("app-copy", RowMark::Doing, tr("setup_app_copy"), None, None)),
            None => {}
        }
        match &self.connection {
            Some(Ok(address)) => rows.push(row("connection", RowMark::Ok, tr("setup_connection"), Some(address.clone()), None)),
            Some(Err(why)) => rows.push(row("connection", RowMark::Warn, tr("setup_connection"), Some(why.clone()), None)),
            None => {}
        }
        rows
    }

    fn render_prepare(&self, cx: &mut Context<Self>) -> Div {
        let mut rows: Vec<Stateful<Div>> = self.precheck.iter().flatten().map(check_row).collect();
        if let Some(text) = &self.preparing { rows.push(row("app-preparing", RowMark::Doing, text.clone(), None, None)); }
        rows.extend(self.runs.items(Step::Prepare).iter().map(item_row));
        div().flex().flex_col().gap_4()
            .when(cfg!(windows), |el| el.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("setup_prepare_uac"))))
            .when(!rows.is_empty(), |el| el.child(card(rows)))
            .child(self.render_details("setup-details-prepare", cx))
    }

    fn render_install(&self, cx: &mut Context<Self>) -> Div {
        let items = self.runs.items(Step::Install);
        let done = items.iter().filter(|i| i.state == State::Ok).count();
        let pct = if items.is_empty() { 0. } else { done as f32 * 100. / items.len() as f32 };
        let title = self.runs.install.as_ref().and_then(|p| p.doing_item()).filter(|i| i.step == Some(Step::Install)).map(|i| i.text.clone())
            .unwrap_or_else(|| if self.runs.install.is_none() { tr("setup_install_waiting") } else { String::new() });
        let mut rows: Vec<Stateful<Div>> = items.iter().map(item_row).collect();
        // Os itens da etapa do celular (`rede-local`, `firewall`) moram aqui, depois dos da instalação: a tela do celular é só o código.
        rows.extend(self.runs.items(Step::Phone).iter().map(item_row));
        rows.extend(self.app_rows(cx));
        div().flex().flex_col().gap_4()
            .child(div().flex().items_center().justify_between().gap_4()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_xs().font_family(theme::MONO).text_color(theme::faint()).child(format!("{}%", pct.round() as u32))))
            .child(Bar::new("setup-install-bar").value(pct).accessibility_label(tr("setup_step_install")))
            .when(!rows.is_empty(), |el| el.child(card(rows)))
            .child(self.render_details("setup-details-install", cx))
    }

    pub(super) fn render_tailscale(&self, cx: &mut Context<Self>) -> Div {
        let items = self.runs.items(Step::Tailscale);
        let account_marked = items.iter().any(|i| i.id == "tailscale-conta" && i.state == State::Ok);
        let mut rows: Vec<Stateful<Div>> = items.iter().map(item_row).collect();
        if self.tailscale_running && !account_marked { rows.push(row("tailscale-running", RowMark::Ok, tr("setup_tailscale_connected"), None, None)); }
        let has_code = |code: &str| self.runs.latest().is_some_and(|p| p.pendings.iter().any(|(c, _)| c == code) || p.error.as_deref() == Some(code));
        let login_callout = self.login_notice(cx);
        let ended = self.runs.end().is_some();
        let https_callout = has_code("tailscale-https").then(|| callout("setup-tailscale-https", tr("setup_tailscale_https_title"),
            vec![tr("setup_tailscale_https_lead"), tr("setup_tailscale_https_1"), tr("setup_tailscale_https_2")],
            vec![Button::new("setup-tailscale-https-open").primary().small().icon(IconName::ExternalLink).label(tr("setup_tailscale_open_settings"))
                    .on_click(|_, _, cx| cx.open_url(TAILSCALE_DNS)).into_any_element()]
                .into_iter().chain(ended.then(|| recheck_button("setup-tailscale-https-recheck", cx))).collect()));
        div().flex().flex_col().gap_4()
            .when(!rows.is_empty(), |el| el.child(card(rows)))
            .children(login_callout)
            .children(https_callout)
            .child(self.render_details("setup-details-tailscale", cx))
    }

    pub(super) fn render_phone(&self, cx: &mut Context<Self>) -> Div {
        if flow::status(Screen::Phone, &self.view()) == Status::Pending {
            return div().child(muted_line("setup-phone-waiting", false, tr("setup_phone_waiting")));
        }
        let retry = || Button::new("setup-phone-retry").outline().small().label(tr("setup_phone_retry"))
            .on_click(cx.listener(|w, _, _, cx| { w.qr = Qr::Idle; w.load_qr(cx); }));
        match &self.qr {
            Qr::Shown { url, tailscale, image, connect } => {
                let copy = url.clone();
                div().flex().items_center().gap_8()
                    // Fundo branco é o do QR, não decoração: câmera nenhuma lê o código sobre o tema escuro.
                    .child(div().id("setup-qr").role(Role::Image).aria_label(tr("setup_phone_qr_label")).size_48().flex_shrink_0().p_3()
                        .rounded_xl().bg(gpui_kit::white()).child(img(image.clone()).size_full()))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_3()
                        .child(div().text_xs().text_color(theme::muted()).child(tr("setup_phone_type")))
                        .child(div().flex().items_center().gap_2().px_3().py_2().rounded_lg().bg(theme::inset()).border_1().border_color(theme::border())
                            .child(div().flex_1().min_w_0().font_family(theme::MONO).text_sm().text_color(theme::accent_text()).whitespace_normal().child(url.clone()))
                            .child(Button::new("setup-phone-copy").ghost().small().icon(IconName::Copy).label(tr("setup_phone_copy"))
                                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))))
                        .child(tips(*tailscale))
                        .when(*connect, |el| el.child(connect_offer())))
            }
            Qr::Loading => div().child(muted_line("setup-phone-loading", true, tr("setup_phone_loading"))),
            // Idle na tela do celular é o código que ninguém pediu (retomada): sempre há como pedir.
            Qr::Idle => div().child(retry()),
            Qr::NoAddress { connect } => div().flex().flex_col().gap_2()
                .child(div().id("setup-phone-none").role(Role::Alert).text_sm().text_color(theme::warning_text()).whitespace_normal().child(tr("setup_phone_none")))
                .when(*connect, |el| el.child(connect_offer()))
                .child(div().child(retry())),
            Qr::Failed(why) => div().flex().flex_col().gap_2()
                .child(div().id("setup-phone-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(why.clone()))
                .child(div().child(retry())),
        }
    }

    fn done_header(&self) -> Div {
        let pending = matches!(self.runs.end(), Some(End::Pending));
        let (icon, color) = if pending { (IconName::CircleAlert, theme::warning()) } else { (IconName::CircleCheck, theme::success()) };
        div().flex().flex_col().gap_2()
            .child(div().size_12().rounded_xl().flex().items_center().justify_center().bg(color.opacity(0.1)).border_1().border_color(color.opacity(0.3))
                .child(chrome::small_icon(icon, 24., color)))
            .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child(if pending { tr("setup_done_title_pending") } else { tr("setup_done_title") }))
            .child(div().text_base().text_color(theme::muted()).whitespace_normal().child(tr("setup_done_lead")))
    }

    fn render_done(&self, cx: &mut Context<Self>) -> Div {
        let mut rows: Vec<Stateful<Div>> = self.runs.items(Step::Final).iter().map(item_row).collect();
        rows.extend(self.app_rows(cx));
        let on_action = self.failure_handler(cx);
        rows.extend(self.runs.latest().iter().flat_map(|p| p.pendings.iter()).map(|(code, text)| failure::pending_row(code, text, on_action.clone())));
        // Reconferência passou com o agente chamado: o que ele disse e o relatório que saiu ficam em "Tudo pronto" — também
        // quando ele terminou com erro e o relatório foi como "aberto".
        let fixed = self.agent.as_ref().is_some_and(|run| matches!(run.phase, AgentPhase::Done { .. }))
            .then(|| failure::after_panel(&self.panel_view(), on_action.clone()));
        div().flex().flex_col().gap_4()
            .when(self.runs.end().is_none(), |el| el.child(muted_line("setup-done-waiting", true, tr("setup_done_waiting"))))
            .when(!rows.is_empty(), |el| el.child(card(rows)))
            .children(fixed)
    }
}

impl Render for SetupWizard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = flow::screens(self.outside);
        div().id("setup").track_focus(&self.focus).size_full().flex().items_stretch()
            .child(self.render_side(&list, cx))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .child(div().id("setup-content").flex_1().min_h_0().overflow_y_scroll()
                    .child(div().px_10().pt_8().pb_6().max_w(rems(52.)).child(self.render_screen(&list, cx))))
                .child(self.render_footer(&list, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob da gpui_kit (via `use super::*`) traz um `test` que colide com o atributo padrão.
    use core::prelude::v1::test;

    #[test]
    fn row_marks_follow_the_item_state() {
        assert_eq!(RowMark::from(State::Pending), RowMark::Warn);
        assert_eq!(RowMark::from(State::Failed), RowMark::Failed);
        assert_eq!(RowMark::from(State::Queued), RowMark::Queued);
    }
}
