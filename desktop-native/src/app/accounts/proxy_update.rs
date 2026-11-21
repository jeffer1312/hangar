//! O CLIProxyAPI desta máquina no topo de Contas: versão, de quem é, se responde, contas e sessões nele, e as ações
//! (Atualizar o do Hangar, Deixar o Hangar cuidar do de fora, Conectar conta). Lê `GET /api/engines/cliproxy/install`
//! ao abrir e recarregar a página; falha da leitura vai ao log, só a ação que falha vira aviso.
use super::*;
use super::cliproxy::{ItemState, code_text, step_item};
use super::keys::provider_badge;

/// Quanto o "atualizado" fica na tela antes de sumir.
const DONE_FOR: Duration = Duration::from_secs(4);
/// Leituras seguidas que falham durante a troca antes de desistir de acompanhar (uma por segundo).
const POLL_FAILURES: u32 = 60;
/// As três etapas que a atualização informa, na ordem do `step` do servidor.
const UPDATE_STEPS: [(&str, &str); 3] = [("download", "cliproxy_update_step_download"), ("service", "cliproxy_update_step_service"),
    ("start", "cliproxy_update_step_start")];
/// As quatro etapas da entrega ao Hangar.
const ADOPT_STEPS: [(&str, &str); 4] = [("download", "cliproxy_adopt_step_download"), ("config", "cliproxy_adopt_step_config"),
    ("service", "cliproxy_adopt_step_service"), ("start", "cliproxy_adopt_step_start")];

/// As duas ações que reiniciam o proxy: trocar a versão do Hangar, ou passar o de fora para ele.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action { Update, Adopt }

impl Action {
    fn kind(self) -> &'static str { match self { Self::Update => "update", Self::Adopt => "adopt" } }
    fn of(kind: &str) -> Option<Self> { match kind { "update" => Some(Self::Update), "adopt" => Some(Self::Adopt), _ => None } }
    pub(super) fn steps(self) -> &'static [(&'static str, &'static str)] { match self { Self::Update => &UPDATE_STEPS, Self::Adopt => &ADOPT_STEPS } }
    /// A chave do texto: `cliproxy_update_<x>` ou `cliproxy_adopt_<x>`.
    fn key(self, rest: &str) -> String { tr(&format!("cliproxy_{}_{rest}", self.kind())) }
}

#[derive(Clone, Deserialize)]
struct JobError { code: String }

#[derive(Clone, Deserialize)]
struct Job { #[serde(default)] kind: String, #[serde(default)] step: String, done: bool, error: Option<JobError> }

/// Se o proxy de fora pode passar para o Hangar; recusado, o código diz o motivo.
#[derive(Clone, Default, Deserialize)]
pub(super) struct Handover { #[serde(default)] pub(super) ok: bool, #[serde(default)] pub(super) code: String }

#[derive(Clone, Default, Deserialize)]
pub(super) struct ProxyInstall {
    #[serde(default)] state: String,
    version: Option<String>,
    latest: Option<String>,
    #[serde(default)] update_available: bool,
    #[serde(default)] running: bool,
    base_url: Option<String>,
    accounts: Option<usize>,
    sessions: Option<usize>,
    handover: Option<Handover>,
    job: Option<Job>,
}

impl ProxyInstall {
    /// A atualização ou a entrega, em curso ou acabada; a instalação do assistente fica de fora.
    fn action_job(&self) -> Option<(Action, &Job)> { self.job.as_ref().and_then(|j| Some((Action::of(&j.kind)?, j))) }
    /// A versão que a linha do modelo mostra: só a do proxy que o Hangar instalou.
    pub(super) fn managed_version(&self) -> Option<&str> { self.version.as_deref().filter(|_| self.state == "managed") }
}

enum Outcome { Done(Action, String, Instant), Failed(String), Interrupted(Action) }

#[derive(Default)]
pub(super) struct ProxyUpdate {
    pub(super) install: Option<ProxyInstall>,
    seq: u64,
    reading: bool,
    /// A ação saiu daqui: só então o fim da tarefa vira faixa.
    asked: Option<Action>,
    posting: Option<Action>,
    /// O servidor pediu confirmação: a releitura traz o número de sessões e abre o diálogo.
    confirm: Option<Action>,
    outcome: Option<Outcome>,
    poll: Option<JoinHandle<()>>,
}

pub(in crate::app) enum UpdateReply {
    Read(u64, Result<Value, Failure>),
    Posted(Action, Result<Value, Failure>),
}

/// A etapa de cada linha: antes da atual, feita; a atual, girando; depois, esperando.
fn step_states(steps: &[(&str, &str)], step: &str) -> Vec<ItemState> {
    let at = steps.iter().position(|(name, _)| *name == step).unwrap_or(0);
    (0..steps.len()).map(|i| if i < at { ItemState::Done } else if i == at { ItemState::Running } else { ItemState::Waiting }).collect()
}

/// O texto da falha pelo código da tarefa: só a volta que deu certo diz que o proxy segue funcionando.
fn failure_text(action: Action, code: &str, install: &ProxyInstall) -> String {
    let (new, old) = (install.latest.clone().unwrap_or_default(), install.version.clone().unwrap_or_default());
    match (action, code) {
        (_, "cliproxy_nao_subiu") => action.key("failed_back").replace("{new}", &new).replace("{old}", &old),
        (Action::Update, "cliproxy_atualizacao_sem_volta") | (Action::Adopt, "cliproxy_entrega_sem_volta") =>
            action.key("failed_lost").replace("{old}", &old),
        (_, other) => action.key("failed").replace("{reason}", &code_text(other)),
    }
}

/// Texto com o trecho `{new}` em negrito.
fn with_bold(template: &str, bold: &str, rest: &[(&str, &str)]) -> StyledText {
    let mut text = template.to_owned();
    for (key, value) in rest { text = text.replace(key, value); }
    let at = text.find("{new}");
    let plain = text.replacen("{new}", bold, 1);
    let strong = HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() };
    StyledText::new(plain).with_highlights(at.map(|at| (at..at + bold.len(), strong)))
}

/// Pergunta antes de reiniciar o proxy com sessões nele; `then` faz a ação confirmada.
pub(super) fn confirm_restart(action: Action, sessions: Option<usize>, window: &mut Window, cx: &mut Context<Hangar>,
    then: fn(&mut Hangar, &mut Context<Hangar>)) {
    let body = match sessions {
        Some(1) => action.key("confirm_one"),
        Some(n) => action.key("confirm_many").replace("{n}", &n.to_string()),
        None => action.key("confirm_some"),
    };
    let (title, ok) = (action.key("confirm_title"), action.key("confirm_ok"));
    let weak = cx.entity().downgrade();
    window.open_dialog(cx, move |dialog, window, _| {
        let weak = weak.clone();
        popup::dialog(dialog).w(rems(28.).to_pixels(window.rem_size())).title(title.clone())
            .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(body.clone()))
            .footer(div().h_flex().justify_end().gap_2()
                .child(Button::new("cliproxy-restart-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                .child(Button::new("cliproxy-restart-confirm").primary().label(ok.clone())
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = weak.update(cx, |this, cx| then(this, cx));
                    })))
            .on_ok(super::super::machines::enter_to_focused)
    });
}

impl Hangar {
    fn update_send(&self) -> impl Fn(UpdateReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let done = self.accounts_send_later();
        move |reply| done(AccountsReply::ProxyUpdate(reply))
    }

    /// Lê o estado da instalação; com a atualização ou a entrega em curso, relê a cada segundo até terminar.
    pub(super) fn load_proxy_install(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let u = &mut self.accounts.proxy;
        // A tarefa em curso já é relida pelo acompanhamento.
        if u.reading || u.poll.is_some() { return; }
        u.seq += 1;
        u.reading = true;
        let (seq, done) = (u.seq, self.update_send());
        self.runtime.spawn(async move { done(UpdateReply::Read(seq, api.server_read(&["engines", "cliproxy", "install"], &[], 20).await)).await });
        cx.notify();
    }

    fn poll_proxy_update(&mut self, cx: &mut Context<Self>) {
        if self.accounts.proxy.poll.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        let done = self.update_send();
        let seq = { let u = &mut self.accounts.proxy; u.seq += 1; u.seq };
        self.accounts.proxy.poll = Some(self.runtime.spawn(async move {
            let mut failures = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let read = api.server_read(&["engines", "cliproxy", "install"], &[], 20).await;
                // O servidor pode reiniciar no meio da troca: leitura que falha espera a próxima volta, até um teto.
                if read.is_err() {
                    failures += 1;
                    if failures < POLL_FAILURES { continue; }
                } else { failures = 0; }
                let running = matches!(&read, Ok(v) if v.pointer("/job/done").and_then(Value::as_bool) == Some(false));
                done(UpdateReply::Read(seq, read)).await;
                if !running { break; }
            }
        }));
        cx.notify();
    }

    fn post_proxy_action(&mut self, action: Action, confirm: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let u = &mut self.accounts.proxy;
        if u.posting.is_some() { return; }
        (u.posting, u.outcome) = (Some(action), None);
        let done = self.update_send();
        let body = json!({"confirm": confirm});
        self.runtime.spawn(async move {
            done(UpdateReply::Posted(action, api.server_send(reqwest::Method::POST, &["engines", "cliproxy", action.kind()], Some(body), 30).await)).await
        });
        cx.notify();
    }

    /// O "feito" some sozinho depois de `DONE_FOR`.
    fn proxy_done(&mut self, action: Action, tag: String, cx: &mut Context<Self>) {
        self.accounts.proxy.outcome = Some(Outcome::Done(action, tag, Instant::now()));
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(DONE_FOR).await;
            let _ = weak.update(cx, |this, cx| {
                if matches!(this.accounts.proxy.outcome, Some(Outcome::Done(..))) { this.accounts.proxy.outcome = None; }
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn receive_proxy_update(&mut self, reply: UpdateReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            UpdateReply::Read(seq, result) => {
                let u = &mut self.accounts.proxy;
                if seq != u.seq { return; }
                u.reading = false;
                let install = match result.map_err(|e| Self::failure(&e))
                    .and_then(|v| serde_json::from_value::<ProxyInstall>(v).map_err(|_| tr("invalid_response"))) {
                    Ok(install) => install,
                    Err(error) => {
                        // Leitura em segundo plano: a página segue sem a linha, e a falha fica no log.
                        eprintln!("[contas] estado do CLIProxyAPI não veio: {error}");
                        u.poll = None;
                        // Mas a releitura que o clique pediu (confirmação) ou que acompanhava a troca é da ação.
                        let asked = u.confirm.take().or(u.asked.take());
                        if let Some(action) = asked {
                            u.install = None;
                            u.outcome = Some(Outcome::Failed(action.key("failed").replace("{reason}", &error)));
                        }
                        cx.notify();
                        return;
                    }
                };
                if install.action_job().is_some_and(|(_, job)| !job.done) {
                    u.install = Some(install);
                    self.poll_proxy_update(cx);
                    return;
                }
                let finished = match (install.action_job(), u.asked) {
                    (Some((action, job)), Some(asked)) if action == asked => {
                        u.asked = None;
                        match &job.error {
                            None => Some((action, install.version.clone().unwrap_or_default())),
                            Some(error) if error.code == "erro_cliproxy_sessoes_ativas" => { u.outcome = Some(Outcome::Interrupted(action)); None }
                            Some(error) => { u.outcome = Some(Outcome::Failed(failure_text(action, &error.code, &install))); None }
                        }
                    }
                    // O servidor reiniciou e perdeu a tarefa: o estado relido diz se a troca entrou.
                    (None, Some(asked)) => {
                        u.asked = None;
                        let entered = match asked {
                            Action::Update => !install.update_available,
                            Action::Adopt => install.state == "managed",
                        };
                        install.version.clone().filter(|_| entered && install.state == "managed").map(|tag| (asked, tag))
                    }
                    _ => None,
                };
                let u = &mut self.accounts.proxy;
                u.poll = None;
                let confirm = u.confirm.take().map(|action| (action, install.sessions));
                u.install = Some(install);
                if let Some((action, tag)) = finished { self.proxy_done(action, tag, cx); }
                // Ninguém mais no proxy desde a recusa: segue direto, sem perguntar. Contagem que não veio pergunta
                // sem número, em vez de reenviar às cegas.
                match confirm {
                    Some((action, Some(0))) => self.post_proxy_action(action, false, cx),
                    Some((Action::Update, sessions)) =>
                        confirm_restart(Action::Update, sessions, window, cx, |this, cx| this.post_proxy_action(Action::Update, true, cx)),
                    Some((Action::Adopt, sessions)) =>
                        confirm_restart(Action::Adopt, sessions, window, cx, |this, cx| this.post_proxy_action(Action::Adopt, true, cx)),
                    None => {}
                }
            }
            UpdateReply::Posted(action, result) => {
                let u = &mut self.accounts.proxy;
                u.posting = None;
                match result {
                    Ok(_) => { u.asked = Some(action); self.poll_proxy_update(cx); }
                    Err(error) => match error.code.as_deref() {
                        Some("erro_cliproxy_sessoes_ativas") => { u.confirm = Some(action); u.reading = false; self.load_proxy_install(cx); }
                        // Outra tarefa em curso: acompanha a que está rodando.
                        Some("erro_cliproxy_ocupado") => { u.asked = Some(action); self.poll_proxy_update(cx); }
                        Some(code) => u.outcome = Some(Outcome::Failed(action.key("failed").replace("{reason}", &code_text(code)))),
                        None => u.outcome = Some(Outcome::Failed(action.key("failed").replace("{reason}", &Self::failure(&error)))),
                    },
                }
            }
        }
        cx.notify();
    }

    /// A linha do CLIProxyAPI no topo de Contas; `None` sem proxy nesta máquina ou com o estado ainda não lido.
    pub(super) fn proxy_card(&self, cx: &mut Context<Self>) -> Option<Div> {
        let u = &self.accounts.proxy;
        let install = u.install.as_ref().filter(|i| matches!(i.state.as_str(), "managed" | "external"))?;
        let external = install.state == "external";
        let running_job = install.action_job().filter(|(_, job)| !job.done);
        let busy = u.posting.is_some() || running_job.is_some();
        let can_adopt = external && install.handover.as_ref().is_some_and(|h| h.ok);
        let adopt = can_adopt.then(|| adopt_button("cliproxy-adopt").loading(u.posting == Some(Action::Adopt)).disabled(busy)
            .on_click(cx.listener(|this, _, _, cx| this.post_proxy_action(Action::Adopt, false, cx))));
        let connect = connect_button("cliproxy-connect").disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.open_cliproxy_wizard(window, cx)));
        let view = CardView {
            version: install.version.clone(), external, address: install.base_url.as_deref().filter(|_| install.running).map(|b| host(b).to_owned()),
            accounts: install.accounts, sessions: install.sessions,
        };
        let retry = |action: Action, cx: &mut Context<Self>| Button::new("cliproxy-action-retry").small().outline().icon(IconName::RefreshCw)
            .label(action.key("retry")).loading(u.posting == Some(action)).disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.post_proxy_action(action, false, cx)));
        let new = install.latest.clone().unwrap_or_default();
        let body: Option<Div> = if let Some((action, job)) = running_job {
            Some(running_banner(action, &new, &job.step))
        } else {
            match &u.outcome {
                Some(Outcome::Done(action, tag, at)) if at.elapsed() < DONE_FOR => Some(done_banner(*action, tag)),
                Some(Outcome::Failed(text)) => {
                    // A entrega que falhou deixa o proxy de fora; a atualização, o do Hangar.
                    let action = if external { Action::Adopt } else { Action::Update };
                    Some(banner(theme::danger(), IconName::CircleAlert, text.clone()).child(div().flex_shrink_0().child(retry(action, cx))))
                }
                _ => None,
            }.or_else(|| {
                let interrupted = |action: Action| matches!(u.outcome, Some(Outcome::Interrupted(a)) if a == action);
                if external {
                    let handover = install.handover.as_ref()?;
                    Some(match (handover.ok, interrupted(Action::Adopt)) {
                        (true, true) => card_note(tr("cliproxy_adopt_interrupted"), theme::muted()),
                        (true, false) => card_note(tr("cliproxy_card_external_note"), theme::muted()),
                        (false, _) => card_note(code_text(&handover.code), theme::warning()),
                    })
                } else if install.update_available {
                    let button = update_button("cliproxy-update").loading(u.posting == Some(Action::Update)).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.post_proxy_action(Action::Update, false, cx)));
                    Some(update_available_banner(&new, &install.version.clone().unwrap_or_default(), interrupted(Action::Update), button))
                } else { None }
            })
        };
        Some(proxy_card_view(&view, adopt.into_iter().chain([connect]).map(IntoElement::into_any_element).collect(), body))
    }
}

/// O que a linha do CLIProxyAPI mostra: dados simples, para a tela de Contas e a animação da novidade.
pub(crate) struct CardView {
    pub(crate) version: Option<String>,
    pub(crate) external: bool,
    /// Onde responde; `None` = parado.
    pub(crate) address: Option<String>,
    pub(crate) accounts: Option<usize>,
    pub(crate) sessions: Option<usize>,
}

/// A linha do CLIProxyAPI: selo, nome, versão, de quem é, se responde, contas e sessões, os botões e o que vem embaixo.
pub(crate) fn proxy_card_view(view: &CardView, actions: Vec<AnyElement>, body: Option<Div>) -> Div {
    let tag = |text: String, color: Hsla| div().flex_shrink_0().px(px(7.)).py(px(1.)).rounded(px(6.)).border_1()
        .border_color(color.alpha(0.35)).text_size(px(11.5)).text_color(color).child(text);
    let origin = if view.external { tag(tr("cliproxy_card_external"), theme::warning()) } else { tag(tr("cliproxy_card_managed"), theme::accent()) };
    // Versão e origem quebram juntas para a linha de baixo, nunca uma em cada linha.
    let tags = div().flex_shrink_0().flex().items_center().gap(px(8.)).children(view.version.clone().map(|v| tag(v, theme::muted()))).child(origin);
    let title = div().flex().flex_wrap().items_center().gap(px(8.)).min_w_0()
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("CLIProxyAPI"))
        .child(tags);
    let dot = |color: Hsla| div().size(px(7.)).flex_shrink_0().rounded_full().bg(color);
    let status = match &view.address {
        Some(address) => div().flex().items_center().gap(px(6.)).child(dot(theme::success()))
            .child(tr("cliproxy_card_running").replace("{url}", address)),
        None => div().flex().items_center().gap(px(6.)).child(dot(theme::faint())).child(tr("cliproxy_card_stopped")),
    };
    let count = |n: usize, one: &str, many: &str| if n == 1 { tr(one) } else { tr(many).replace("{n}", &n.to_string()) };
    let meta = div().flex().flex_wrap().items_center().gap_x(px(14.)).gap_y(px(4.)).text_size(px(12.5)).text_color(theme::muted())
        .child(status)
        .children(view.accounts.map(|n| count(n, "cliproxy_card_accounts_one", "cliproxy_card_accounts_many")))
        .children(view.sessions.filter(|n| *n > 0).map(|n| count(n, "cliproxy_card_sessions_one", "cliproxy_card_sessions_many")));
    // Sem largura para nome, selos e botões na mesma linha, os botões descem: o texto nunca fica por baixo deles.
    let head = div().flex().flex_wrap().items_center().gap(px(12.))
        .child(provider_badge("http://127.0.0.1:8317", "CLIProxyAPI", 36.))
        .child(div().flex_1().min_w(px(240.)).flex().flex_col().gap(px(5.)).child(title).child(meta))
        .child(div().flex_shrink_0().flex().items_center().gap(px(8.)).children(actions));
    settings_box().p(px(16.)).gap(px(12.)).child(head).children(body)
}

/// Texto embaixo da linha, separado por um fio.
pub(crate) fn card_note(text: String, color: Hsla) -> Div {
    div().pt(px(10.)).border_t_1().border_color(theme::border()).text_size(px(12.5)).text_color(color).whitespace_normal().child(text)
}

/// O "Deixar o Hangar cuidar"; quem chama liga o clique (a animação da novidade não liga).
pub(crate) fn adopt_button(id: &'static str) -> Button {
    Button::new(id).small().primary().label(tr("cliproxy_adopt_button"))
}

pub(crate) fn connect_button(id: &'static str) -> Button {
    Button::new(id).small().outline().label(tr("cliproxy_card_connect"))
}

/// Faixa do andamento da entrega, para a animação da novidade.
pub(crate) fn adopt_running_banner(new: &str, step: &str) -> Div { running_banner(Action::Adopt, new, step) }

/// Faixa da entrega que deu certo.
pub(crate) fn adopt_done_banner(tag: &str) -> Div { done_banner(Action::Adopt, tag) }

/// O botão Atualizar da faixa; quem chama liga o clique (a animação da novidade não liga).
pub(crate) fn update_button(id: &'static str) -> Button {
    Button::new(id).small().primary().icon(IconName::RefreshCw).label(tr("cliproxy_update_button"))
}

/// Faixa da versão nova, com o botão que quem chama montou.
pub(crate) fn update_available_banner(new: &str, old: &str, interrupted: bool, button: impl IntoElement) -> Div {
    let text = with_bold(&tr("cliproxy_update_available"), new, &[("{old}", old)]);
    let body = div().flex().flex_col().gap(px(4.)).child(text)
        .when(interrupted, |body| body.child(div().text_color(theme::muted()).child(tr("cliproxy_update_interrupted"))));
    let icon = chrome::small_icon(IconName::ArrowUp, 15., theme::accent()).into_any_element();
    banner_with(theme::accent(), icon, body.into_any_element()).child(div().flex_shrink_0().child(button))
}

/// Faixa do andamento, com as etapas e a atual girando.
fn running_banner(action: Action, new: &str, step: &str) -> Div {
    let steps = action.steps();
    let body = div().flex().flex_col().gap(px(8.))
        .child(action.key("running").replace("{new}", new))
        .child(div().flex().flex_col().gap(px(7.)).children(steps.iter().zip(step_states(steps, step)).enumerate()
            .map(|(i, ((_, key), state))| step_item(&format!("cliproxy-{}-step-{i}", action.kind()), tr(key).replace("{new}", new), state))));
    let spinner = chrome::Spinner::new(SharedString::from(format!("cliproxy-{}-running", action.kind())), IconName::LoaderCircle, px(15.), theme::accent());
    banner_with(theme::accent(), spinner.into_any_element(), body.into_any_element()).items_start()
}

/// Faixa do andamento da atualização, para a animação da novidade.
pub(crate) fn update_running_banner(new: &str, step: &str) -> Div { running_banner(Action::Update, new, step) }

/// As etapas da entrega em curso, uma por linha: o assistente as mostra no passo Instalar.
pub(super) fn adopt_items(new: &str, step: &str) -> Vec<Div> {
    ADOPT_STEPS.iter().zip(step_states(&ADOPT_STEPS, step)).enumerate()
        .map(|(index, ((_, key), state))| step_item(&format!("cliproxy-wizard-adopt-{index}"), tr(key).replace("{new}", new), state))
        .collect()
}

fn done_banner(action: Action, tag: &str) -> Div {
    banner(theme::success(), IconName::CircleCheck, action.key("done").replace("{new}", tag))
}

/// Faixa do fim que deu certo.
pub(crate) fn update_done_banner(tag: &str) -> Div { done_banner(Action::Update, tag) }

/// O texto da falha da entrega no assistente.
pub(super) fn adopt_failure_text(code: &str, latest: &str, version: &str) -> String {
    let install = ProxyInstall { latest: Some(latest.to_owned()), version: Some(version.to_owned()), ..Default::default() };
    failure_text(Action::Adopt, code, &install)
}

#[cfg(test)]
mod tests {
    use super::{Action, ItemState, ProxyInstall, failure_text, step_states, ADOPT_STEPS, UPDATE_STEPS};
    use crate::i18n::tr;
    use serde_json::{Value, json};

    fn install(value: Value) -> ProxyInstall { serde_json::from_value(value).unwrap() }

    #[test]
    fn steps_before_the_current_one_are_done() {
        assert!(matches!(step_states(&UPDATE_STEPS, "download")[..], [ItemState::Running, ItemState::Waiting, ItemState::Waiting]));
        assert!(matches!(step_states(&UPDATE_STEPS, "service")[..], [ItemState::Done, ItemState::Running, ItemState::Waiting]));
        assert!(matches!(step_states(&ADOPT_STEPS, "service")[..], [ItemState::Done, ItemState::Done, ItemState::Running, ItemState::Waiting]));
    }

    #[test]
    fn only_a_rollback_that_worked_says_the_proxy_still_works() {
        let i = install(json!({"state": "managed", "version": "v8.0.79", "latest": "v8.1.0"}));
        assert_eq!(failure_text(Action::Update, "cliproxy_nao_subiu", &i),
            tr("cliproxy_update_failed_back").replace("{new}", "v8.1.0").replace("{old}", "v8.0.79"));
        assert_eq!(failure_text(Action::Update, "cliproxy_atualizacao_sem_volta", &i), tr("cliproxy_update_failed_lost").replace("{old}", "v8.0.79"));
        let ext = install(json!({"state": "external", "version": "v8.0.56", "latest": "v8.0.79"}));
        assert_eq!(failure_text(Action::Adopt, "cliproxy_nao_subiu", &ext),
            tr("cliproxy_adopt_failed_back").replace("{new}", "v8.0.79").replace("{old}", "v8.0.56"));
        assert_eq!(failure_text(Action::Adopt, "cliproxy_entrega_sem_volta", &ext), tr("cliproxy_adopt_failed_lost").replace("{old}", "v8.0.56"));
    }

    #[test]
    fn only_the_proxy_the_hangar_installed_shows_its_version() {
        assert_eq!(install(json!({"state": "managed", "version": "v8.0.79"})).managed_version(), Some("v8.0.79"));
        assert_eq!(install(json!({"state": "external", "version": "v8.0.56"})).managed_version(), None);
        let job = install(json!({"state": "managed", "job": {"kind": "install", "step": "start", "done": false}}));
        assert!(job.action_job().is_none(), "a instalação do assistente não é atualização nem entrega");
        let adopt = install(json!({"state": "external", "job": {"kind": "adopt", "step": "config", "done": false}}));
        assert!(matches!(adopt.action_job(), Some((Action::Adopt, _))));
    }
}
