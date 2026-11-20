//! Versão nova do CLIProxyAPI instalado pelo Hangar: a faixa no topo de Contas, o Atualizar, a confirmação quando há
//! sessões no proxy e o andamento. Lê `GET /api/engines/cliproxy/install` ao abrir e recarregar a página; falha da
//! leitura vai ao log, só a atualização que falha vira aviso.
use super::*;
use super::cliproxy::{ItemState, code_text, step_item};

/// Quanto o "atualizado" fica na tela antes de sumir.
const DONE_FOR: Duration = Duration::from_secs(4);
/// As três etapas que a atualização informa, na ordem do `step` do servidor.
const UPDATE_STEPS: [(&str, &str); 3] = [("download", "cliproxy_update_step_download"), ("service", "cliproxy_update_step_service"),
    ("start", "cliproxy_update_step_start")];

#[derive(Clone, Deserialize)]
struct JobError { code: String }

#[derive(Clone, Deserialize)]
struct Job { #[serde(default)] kind: String, #[serde(default)] step: String, done: bool, error: Option<JobError> }

#[derive(Clone, Default, Deserialize)]
pub(super) struct ProxyInstall {
    #[serde(default)] state: String,
    version: Option<String>,
    latest: Option<String>,
    #[serde(default)] update_available: bool,
    sessions: Option<usize>,
    job: Option<Job>,
}

impl ProxyInstall {
    fn update_job(&self) -> Option<&Job> { self.job.as_ref().filter(|j| j.kind == "update") }
    /// A versão que a linha do modelo mostra: só a do proxy que o Hangar instalou.
    pub(super) fn managed_version(&self) -> Option<&str> { self.version.as_deref().filter(|_| self.state == "managed") }
}

enum Outcome { Done(String, Instant), Failed(String), Interrupted }

#[derive(Default)]
pub(super) struct ProxyUpdate {
    pub(super) install: Option<ProxyInstall>,
    seq: u64,
    reading: bool,
    /// O Atualizar saiu daqui: só então o fim da tarefa vira faixa.
    asked: bool,
    posting: bool,
    /// O servidor pediu confirmação: a releitura traz o número de sessões e abre o diálogo.
    confirm: bool,
    outcome: Option<Outcome>,
    poll: Option<JoinHandle<()>>,
}

pub(in crate::app) enum UpdateReply {
    Read(u64, Result<Value, Failure>),
    Posted(Result<Value, Failure>),
}

/// A etapa da atualização em cada linha: antes da atual, feita; a atual, girando; depois, esperando.
fn step_states(step: &str) -> [ItemState; 3] {
    let at = UPDATE_STEPS.iter().position(|(name, _)| *name == step).unwrap_or(0);
    std::array::from_fn(|i| if i < at { ItemState::Done } else if i == at { ItemState::Running } else { ItemState::Waiting })
}

/// O texto da falha pelo código da tarefa.
fn failure_text(code: &str, install: &ProxyInstall) -> String {
    let (new, old) = (install.latest.clone().unwrap_or_default(), install.version.clone().unwrap_or_default());
    match code {
        "cliproxy_nao_subiu" => tr("cliproxy_update_failed_back").replace("{new}", &new).replace("{old}", &old),
        "cliproxy_atualizacao_sem_volta" => tr("cliproxy_update_failed_lost").replace("{old}", &old),
        other => tr("cliproxy_update_failed").replace("{reason}", &code_text(other)),
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

impl Hangar {
    fn update_send(&self) -> impl Fn(UpdateReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let done = self.accounts_send_later();
        move |reply| done(AccountsReply::ProxyUpdate(reply))
    }

    /// Lê o estado da instalação; com a atualização em curso, relê a cada segundo até ela terminar.
    pub(super) fn load_proxy_install(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let u = &mut self.accounts.proxy;
        // A atualização em curso já é relida pelo acompanhamento.
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
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let read = api.server_read(&["engines", "cliproxy", "install"], &[], 20).await;
                let running = matches!(&read, Ok(v) if v.pointer("/job/done").and_then(Value::as_bool) == Some(false));
                done(UpdateReply::Read(seq, read)).await;
                if !running { break; }
            }
        }));
        cx.notify();
    }

    fn post_proxy_update(&mut self, confirm: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let u = &mut self.accounts.proxy;
        if u.posting { return; }
        (u.posting, u.outcome) = (true, None);
        let done = self.update_send();
        let body = json!({"confirm": confirm});
        self.runtime.spawn(async move {
            done(UpdateReply::Posted(api.server_send(reqwest::Method::POST, &["engines", "cliproxy", "update"], Some(body), 30).await)).await
        });
        cx.notify();
    }

    fn confirm_proxy_update(&mut self, sessions: usize, window: &mut Window, cx: &mut Context<Self>) {
        let body = if sessions == 1 { tr("cliproxy_update_confirm_one") }
            else { tr("cliproxy_update_confirm_many").replace("{n}", &sessions.to_string()) };
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let weak = weak.clone();
            popup::dialog(dialog).w(rems(28.).to_pixels(window.rem_size())).title(tr("cliproxy_update_confirm_title"))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(body.clone()))
                .footer(div().h_flex().justify_end().gap_2()
                    .child(Button::new("cliproxy-update-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("cliproxy-update-confirm").primary().label(tr("cliproxy_update_confirm_ok"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = weak.update(cx, |this, cx| this.post_proxy_update(true, cx));
                        })))
                .on_ok(super::super::machines::enter_to_focused)
        });
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
                        // Leitura em segundo plano: a página segue sem a faixa, e a falha fica no log.
                        eprintln!("[contas] estado do CLIProxyAPI não veio: {error}");
                        u.poll = None;
                        return;
                    }
                };
                if install.update_job().is_some_and(|job| !job.done) {
                    u.install = Some(install);
                    self.poll_proxy_update(cx);
                    return;
                }
                match install.update_job() {
                    Some(job) if u.asked => {
                        u.asked = false;
                        u.outcome = Some(match &job.error {
                            None => {
                                let tag = install.version.clone().unwrap_or_default();
                                let weak = cx.entity().downgrade();
                                cx.spawn(async move |_, cx| {
                                    cx.background_executor().timer(DONE_FOR).await;
                                    let _ = weak.update(cx, |this, cx| {
                                        if matches!(this.accounts.proxy.outcome, Some(Outcome::Done(..))) { this.accounts.proxy.outcome = None; }
                                        cx.notify();
                                    });
                                }).detach();
                                Outcome::Done(tag, Instant::now())
                            }
                            Some(error) if error.code == "erro_cliproxy_sessoes_ativas" => Outcome::Interrupted,
                            Some(error) => Outcome::Failed(failure_text(&error.code, &install)),
                        });
                    }
                    _ => {}
                }
                u.poll = None;
                let confirm = std::mem::take(&mut u.confirm).then(|| install.sessions.unwrap_or(0));
                u.install = Some(install);
                // Ninguém mais no proxy desde a recusa: atualiza direto, sem perguntar.
                match confirm {
                    Some(0) => self.post_proxy_update(false, cx),
                    Some(sessions) => self.confirm_proxy_update(sessions, window, cx),
                    None => {}
                }
            }
            UpdateReply::Posted(result) => {
                let u = &mut self.accounts.proxy;
                u.posting = false;
                match result {
                    Ok(_) => { u.asked = true; self.poll_proxy_update(cx); }
                    Err(error) => match error.code.as_deref() {
                        Some("erro_cliproxy_sessoes_ativas") => { u.confirm = true; u.reading = false; self.load_proxy_install(cx); }
                        // Outra instalação ou atualização em curso: acompanha a que está rodando.
                        Some("erro_cliproxy_ocupado") => { u.asked = true; self.poll_proxy_update(cx); }
                        Some(code) => u.outcome = Some(Outcome::Failed(tr("cliproxy_update_failed").replace("{reason}", &code_text(code)))),
                        None => u.outcome = Some(Outcome::Failed(tr("cliproxy_update_failed").replace("{reason}", &Self::failure(&error)))),
                    },
                }
            }
        }
        cx.notify();
    }

    /// A faixa do topo de Contas; `None` sem nada a dizer (em dia, proxy de fora ou estado ainda não lido).
    pub(super) fn proxy_update_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let u = &self.accounts.proxy;
        let install = u.install.as_ref()?;
        let posting = u.posting;
        let update = |button: Button, cx: &mut Context<Self>| button.loading(posting).disabled(posting)
            .on_click(cx.listener(|this, _, _, cx| this.post_proxy_update(false, cx)));
        let new = install.latest.clone().unwrap_or_default();
        if let Some(job) = install.update_job().filter(|j| !j.done) {
            return Some(update_running_banner(&new, &job.step));
        }
        match &u.outcome {
            Some(Outcome::Done(tag, at)) if at.elapsed() < DONE_FOR => return Some(update_done_banner(tag)),
            Some(Outcome::Failed(text)) => {
                let retry = Button::new("cliproxy-update-retry").small().outline().icon(IconName::RefreshCw).label(tr("cliproxy_update_retry"));
                return Some(banner(theme::danger(), IconName::CircleAlert, text.clone()).child(div().flex_shrink_0().child(update(retry, cx))));
            }
            _ => {}
        }
        if install.state != "managed" || !install.update_available { return None; }
        let old = install.version.clone().unwrap_or_default();
        Some(update_available_banner(&new, &old, matches!(u.outcome, Some(Outcome::Interrupted)), update(update_button("cliproxy-update"), cx)))
    }
}

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

/// Faixa do andamento, com as três etapas e a atual girando.
pub(crate) fn update_running_banner(new: &str, step: &str) -> Div {
    let body = div().flex().flex_col().gap(px(8.))
        .child(tr("cliproxy_update_running").replace("{new}", new))
        .child(div().flex().flex_col().gap(px(7.)).children(UPDATE_STEPS.iter().zip(step_states(step)).enumerate()
            .map(|(i, ((_, key), state))| step_item(&format!("cliproxy-update-step-{i}"), tr(key), state))));
    let spinner = chrome::Spinner::new("cliproxy-update-running", IconName::LoaderCircle, px(15.), theme::accent());
    banner_with(theme::accent(), spinner.into_any_element(), body.into_any_element()).items_start()
}

/// Faixa do fim que deu certo.
pub(crate) fn update_done_banner(tag: &str) -> Div {
    banner(theme::success(), IconName::CircleCheck, tr("cliproxy_update_done").replace("{new}", tag))
}

#[cfg(test)]
mod tests {
    use super::{ItemState, ProxyInstall, failure_text, step_states};
    use crate::i18n::tr;
    use serde_json::{Value, json};

    fn install(value: Value) -> ProxyInstall { serde_json::from_value(value).unwrap() }

    #[test]
    fn steps_before_the_current_one_are_done() {
        assert!(matches!(step_states("download"), [ItemState::Running, ItemState::Waiting, ItemState::Waiting]));
        assert!(matches!(step_states("service"), [ItemState::Done, ItemState::Running, ItemState::Waiting]));
        assert!(matches!(step_states("start"), [ItemState::Done, ItemState::Done, ItemState::Running]));
    }

    #[test]
    fn only_a_rollback_that_worked_says_the_proxy_still_works() {
        let i = install(json!({"state": "managed", "version": "v8.0.79", "latest": "v8.1.0"}));
        let back = failure_text("cliproxy_nao_subiu", &i);
        assert!(back.contains("v8.1.0") && back.contains("v8.0.79"));
        assert_eq!(back, tr("cliproxy_update_failed_back").replace("{new}", "v8.1.0").replace("{old}", "v8.0.79"));
        assert_eq!(failure_text("cliproxy_atualizacao_sem_volta", &i), tr("cliproxy_update_failed_lost").replace("{old}", "v8.0.79"));
    }

    #[test]
    fn only_the_proxy_the_hangar_installed_shows_its_version() {
        assert_eq!(install(json!({"state": "managed", "version": "v8.0.79"})).managed_version(), Some("v8.0.79"));
        assert_eq!(install(json!({"state": "external", "version": null})).managed_version(), None);
        let job = install(json!({"state": "managed", "job": {"kind": "install", "step": "start", "done": false}}));
        assert!(job.update_job().is_none(), "a instalação em curso não é atualização");
    }
}
