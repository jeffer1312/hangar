//! WhatsApp só leitura no servidor conectado: aceite do risco, instalação, pareamento do celular e o que já foi guardado.
//! Fala só com `/api/whatsapp*` (rotas do dono); o serviço e os dados ficam no servidor.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, Page};
use serde::Deserialize;
use std::{cell::Cell, rc::Rc};

/// As etapas do job, na ordem do `step` do servidor.
const STEPS: [(&str, &str); 3] = [("download", "whatsapp_step_download"), ("service", "whatsapp_step_service"), ("start", "whatsapp_step_start")];
/// O job e o pareamento mudam sem aviso do servidor: a página relê neste ritmo enquanto um deles anda.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// Logo depois de pedir o código o estado ainda pode vir `unpaired`; a releitura continua por este tempo.
const PAIR_GRACE: Duration = Duration::from_secs(10);
/// As ferramentas que as sessões usam: nomes do catálogo MCP, iguais nos dois idiomas.
const TOOLS: &str = "whatsapp_chats · whatsapp_read · whatsapp_search · whatsapp_request_media";

/// Já aberta neste aparelho: lido do disco uma vez, no primeiro desenho da navegação.
static PAGE_SEEN: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();

fn page_seen() -> &'static std::sync::atomic::AtomicBool {
    PAGE_SEEN.get_or_init(|| std::sync::atomic::AtomicBool::new(crate::appearance::whatsapp_page_seen()))
}

/// O selo "novo" da navegação: some na primeira abertura da página.
pub(super) fn show_new_badge() -> bool { !page_seen().load(std::sync::atomic::Ordering::Relaxed) }

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Totals { messages: u64, chats: u64, groups: u64, media_saved: u64, media_phone_only: u64, media_lost: u64 }

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Status {
    #[serde(default)]
    state: String,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    device: Option<String>,
    /// Como o aparelho aparece na lista do celular; binário antigo não manda.
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    pair_code: Option<String>,
    #[serde(default)]
    totals: Totals,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    last_message_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Job {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    step: String,
    done: bool,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct WaState {
    installed: bool,
    #[serde(default)]
    accepted_at: Option<String>,
    service: String,
    #[serde(default)]
    job: Option<Job>,
    #[serde(default)]
    status: Option<Status>,
    #[serde(default)]
    qr_svg: Option<String>,
}

fn parse_state(value: Value) -> Result<WaState, String> {
    let state: WaState = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    if !matches!(state.service.as_str(), "running" | "stopped" | "absent") { return Err(tr("invalid_response")); }
    Ok(state)
}

/// O código de um erro, venha como texto ou como `{code}`.
fn code_of(error: &Value) -> Option<String> {
    match error {
        Value::String(code) => Some(code.clone()),
        Value::Object(fields) => fields.get("code").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }.filter(|code| !code.is_empty())
}

/// O que a página mostra, decidido só pela resposta do `GET /api/whatsapp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View { Off, Working, Stopped, Unreachable, Unpaired, Pairing, Connected, Outdated, Unknown }

fn view_of(state: &WaState) -> View {
    if state.job.as_ref().is_some_and(|job| !job.done) { return View::Working; }
    if !state.installed { return View::Off; }
    if state.service != "running" { return View::Stopped; }
    let Some(status) = &state.status else { return View::Unreachable };
    match status.state.as_str() {
        "connected" => View::Connected,
        "outdated" => View::Outdated,
        "pairing" => View::Pairing,
        "unpaired" | "logged_out" => View::Unpaired,
        _ => View::Unknown,
    }
}

/// O job que terminou com erro: o tipo dele (`install`/`update`) e o código.
fn failed_job(state: &WaState) -> Option<(String, String)> {
    let job = state.job.as_ref().filter(|job| job.done)?;
    let code = job.error.as_ref().and_then(code_of)?;
    Some((job.kind.clone(), code))
}

fn should_poll(state: &WaState, awaiting_pair: bool) -> bool {
    matches!(view_of(state), View::Working | View::Pairing) || (awaiting_pair && view_of(state) == View::Unpaired)
}

/// Índice da etapa em curso; etapa desconhecida conta como a primeira.
fn step_index(step: &str) -> usize { STEPS.iter().position(|(name, _)| *name == step).unwrap_or(0) }

/// Só algarismos, de 10 a 15 (código do país, DDD e número); o `+`, espaços e traços somem.
fn phone_digits(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let other = text.chars().any(|c| !c.is_ascii_digit() && !matches!(c, '+' | ' ' | '-' | '(' | ')' | '.'));
    (!other && (10..=15).contains(&digits.len())).then_some(digits)
}

/// Inteiro com o separador de milhar do idioma da tela.
fn grouped(n: u64, english: bool) -> String {
    let digits = n.to_string();
    let sep = if english { ',' } else { '.' };
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { out.push(sep); }
        out.push(c);
    }
    out
}

fn local_date(at: &str, english: bool) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&chrono::Local);
    Some(at.format(if english { "%m/%d/%Y" } else { "%d/%m/%Y" }).to_string())
}

fn seconds_since(at: &str) -> Option<f64> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some((chrono::Utc::now().timestamp_millis() - at.timestamp_millis()) as f64 / 1000.)
}

/// Frase de um código: a da página, se existir; senão a genérica com o código.
fn code_text(prefix: &str, code: &str) -> String {
    let key = format!("{prefix}{}", code.strip_prefix("whatsapp_").unwrap_or(code));
    let text = tr(&key);
    if text == key { tr(&format!("{prefix}unknown")).replace("{code}", code) } else { text }
}

fn failure_text(error: &Failure) -> String {
    match error.code.as_deref() {
        Some(code) if code.starts_with("whatsapp_") => code_text("whatsapp_err_", code),
        _ => Hangar::setting_failure(error),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act { Install, Update, Pair, Logout, Wipe, Uninstall }

pub(super) enum WhatsAppReply {
    Loaded(u64, Result<Value, Failure>),
    Acted(u64, Act, Result<Value, Failure>),
}

#[derive(Default)]
pub(in crate::app) struct WhatsApp {
    state: Remote<WaState>,
    busy: Option<Act>,
    write_seq: u64,
    error: Option<String>,
    accepted: bool,
    /// O QR dá acesso à conta: só aparece depois que a pessoa pede, nesta visita à página.
    qr_wanted: bool,
    by_phone: bool,
    phone: Option<Entity<InputState>>,
    phone_error: Option<String>,
    _phone_subscription: Option<Subscription>,
    /// O SVG recebido e a imagem dele: o QR só é redesenhado quando muda.
    qr: Option<(String, Arc<Image>)>,
    awaiting_pair: Option<Instant>,
    /// Apagou com o serviço parado: o aparelho continua na lista do celular.
    unlink_on_phone: bool,
    poll: Option<JoinHandle<()>>,
}

impl Drop for WhatsApp {
    fn drop(&mut self) { if let Some(task) = self.poll.take() { task.abort(); } }
}

impl Hangar {
    fn whatsapp_send_later(&self) -> impl Fn(WhatsAppReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::WhatsApp(reply) }).await; })
        }
    }

    pub(super) fn whatsapp_opened(&mut self, cx: &mut Context<Self>) {
        if !page_seen().swap(true, std::sync::atomic::Ordering::Relaxed) { self.runtime.spawn_blocking(crate::appearance::mark_whatsapp_page_seen); }
        let w = &mut self.whatsapp;
        (w.qr_wanted, w.by_phone, w.phone_error, w.awaiting_pair) = (false, false, None, None);
        if w.busy.is_none() { (w.error, w.unlink_on_phone) = (None, false); }
        self.whatsapp_read(false, cx);
    }

    /// Fora da página: a releitura para e o QR sai da memória.
    pub(super) fn whatsapp_page_left(&mut self) {
        let w = &mut self.whatsapp;
        if let Some(task) = w.poll.take() { task.abort(); }
        w.state.seq += 1;
        w.state.loading = false;
        (w.qr, w.qr_wanted) = (None, false);
    }

    fn whatsapp_read(&mut self, delay: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let w = &mut self.whatsapp;
        if let Some(task) = w.poll.take() { task.abort(); }
        w.state.seq += 1;
        // A releitura não volta ao "Carregando": o que está na tela fica até a resposta.
        if w.state.value.is_none() { w.state.loading = true; }
        let seq = w.state.seq;
        let done = self.whatsapp_send_later();
        self.whatsapp.poll = Some(self.runtime.spawn(async move {
            if delay { tokio::time::sleep(POLL_EVERY).await; }
            done(WhatsAppReply::Loaded(seq, api.server_read(&["whatsapp"], &[], 15).await)).await
        }));
        cx.notify();
    }

    fn whatsapp_act(&mut self, act: Act, body: Option<Value>, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.whatsapp.busy.is_some() { return; }
        let w = &mut self.whatsapp;
        w.write_seq += 1;
        let seq = w.write_seq;
        (w.busy, w.error) = (Some(act), None);
        if act != Act::Wipe { w.unlink_on_phone = false; }
        let path = match act { Act::Install => "install", Act::Update => "update", Act::Pair => "pair", Act::Logout => "logout",
            Act::Wipe => "wipe", Act::Uninstall => "uninstall" };
        let done = self.whatsapp_send_later();
        self.runtime.spawn(async move {
            done(WhatsAppReply::Acted(seq, act, api.server_send(reqwest::Method::POST, &["whatsapp", path], body, 30).await)).await
        });
        cx.notify();
    }

    pub(super) fn receive_whatsapp(&mut self, reply: WhatsAppReply, cx: &mut Context<Self>) {
        match reply {
            WhatsAppReply::Loaded(seq, result) => {
                if seq != self.whatsapp.state.seq { return; }
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_state);
                let w = &mut self.whatsapp;
                if let Ok(state) = &result {
                    w.qr = match (&state.qr_svg, w.qr.take()) {
                        (Some(svg), Some((old, image))) if *svg == old => Some((old, image)),
                        (Some(svg), _) => Some((svg.clone(), Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec())))),
                        (None, _) => None,
                    };
                    if view_of(state) != View::Unpaired { w.awaiting_pair = None; }
                    if view_of(state) == View::Connected { (w.qr_wanted, w.by_phone) = (false, false); }
                }
                let poll = result.as_ref().is_ok_and(|state| should_poll(state, w.awaiting_pair.is_some_and(|until| Instant::now() < until)));
                w.state.finish(seq, result);
                w.poll = None;
                if poll && self.settings == Some(Page::WhatsApp) && !self.window_tray.hidden { self.whatsapp_read(true, cx); }
            }
            WhatsAppReply::Acted(seq, act, result) => {
                let w = &mut self.whatsapp;
                if seq != w.write_seq { return; }
                w.busy = None;
                match result {
                    Ok(value) => {
                        if act == Act::Wipe { w.unlink_on_phone = value.get("logout").and_then(Value::as_str) == Some("not_done"); }
                        if act == Act::Pair { w.awaiting_pair = Some(Instant::now() + PAIR_GRACE); }
                    }
                    // Outro job já em curso: a página passa a acompanhar esse.
                    Err(error) if matches!(act, Act::Install | Act::Update) && error.code.as_deref() == Some("whatsapp_busy") => {}
                    Err(error) => w.error = Some(failure_text(&error)),
                }
                // A ação pode ter chegado antes da queda; a releitura mostra o estado real.
                self.whatsapp_read(false, cx);
            }
        }
        cx.notify();
    }

    fn whatsapp_install(&mut self, cx: &mut Context<Self>) {
        let accepted = self.whatsapp.accepted || self.whatsapp.state.ok().is_some_and(|s| s.accepted_at.is_some());
        if !accepted { return; }
        self.whatsapp_act(Act::Install, Some(json!({"accept": true})), cx);
    }

    fn whatsapp_pair(&mut self, phone: Option<String>, cx: &mut Context<Self>) {
        self.whatsapp.qr_wanted = phone.is_none();
        self.whatsapp_act(Act::Pair, phone.map(|phone| json!({"phone": phone})), cx);
    }

    fn whatsapp_request_code(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.whatsapp.phone else { return };
        match phone_digits(&input.read(cx).value()) {
            Some(phone) => { self.whatsapp.phone_error = None; self.whatsapp_pair(Some(phone), cx); }
            None => { self.whatsapp.phone_error = Some(tr("whatsapp_pair_phone_invalid")); cx.notify(); }
        }
    }

    fn whatsapp_by_phone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.whatsapp.phone.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("whatsapp_pair_phone_placeholder")));
            let subscription = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => { this.whatsapp.phone_error = None; cx.notify(); }
                InputEvent::PressEnter { .. } => this.whatsapp_request_code(cx),
                _ => {}
            });
            (self.whatsapp.phone, self.whatsapp._phone_subscription) = (Some(input), Some(subscription));
        }
        self.whatsapp.by_phone = true;
        if let Some(input) = &self.whatsapp.phone { input.update(cx, |input, cx| input.focus(window, cx)); }
        cx.notify();
    }

    fn whatsapp_cancel_pair(&mut self, cx: &mut Context<Self>) {
        let w = &mut self.whatsapp;
        (w.qr_wanted, w.by_phone, w.awaiting_pair, w.phone_error) = (false, false, None, None);
        cx.notify();
    }

    fn confirm_whatsapp_logout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("whatsapp_disconnect_title"), tr("whatsapp_disconnect_body"), tr("whatsapp_disconnect"),
            ButtonVariant::Danger, move |_, cx| { let _ = this.update(cx, |this, cx| this.whatsapp_act(Act::Logout, None, cx)); true });
    }

    fn confirm_whatsapp_wipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.whatsapp.state.ok().is_some_and(|s| s.service == "running");
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("whatsapp_wipe_title"), tr(if running { "whatsapp_wipe_body" } else { "whatsapp_wipe_body_stopped" }),
            tr("whatsapp_wipe_ok"), ButtonVariant::Danger,
            move |_, cx| { let _ = this.update(cx, |this, cx| this.whatsapp_act(Act::Wipe, Some(json!({"confirm": true})), cx)); true });
    }

    /// Remover oferece desconectar antes: depois de removido, só o celular tira o aparelho da lista.
    fn confirm_whatsapp_uninstall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let connected = self.whatsapp.state.ok().map(view_of) == Some(View::Connected);
        let logout = Rc::new(Cell::new(connected));
        let check = connected.then(|| (tr("whatsapp_remove_logout"), logout.clone()));
        let this = cx.entity().downgrade();
        chrome::confirm_alert_with(window, cx, tr("whatsapp_remove_title"), tr("whatsapp_remove_body"), check, tr("whatsapp_remove_ok"),
            ButtonVariant::Danger, move |_, cx| {
                let body = json!({"confirm": true, "logout": logout.get()});
                let _ = this.update(cx, |this, cx| this.whatsapp_act(Act::Uninstall, Some(body), cx));
                true
            });
    }

    pub(super) fn render_whatsapp(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::WhatsApp.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let mut page = div().flex().flex_col().gap_4().child(title);
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = self.whatsapp.error.clone() {
            page = page.child(alert_line("whatsapp-write-error", error));
        }
        if self.whatsapp.unlink_on_phone {
            page = page.child(notice("whatsapp-unlink-on-phone", theme::warning(), IconName::TriangleAlert, tr("whatsapp_unlink_on_phone")));
        }
        let state = match &self.whatsapp.state.value {
            Some(Ok(state)) => state.clone(),
            Some(Err(error)) => return page.child(alert_line("whatsapp-load-error", error.clone()))
                .child(div().flex().child(Button::new("whatsapp-retry-load").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.whatsapp.error = None; this.whatsapp_read(false, cx); })))).into_any_element(),
            None if self.whatsapp.state.loading => return page.child(div().id("whatsapp-loading").role(Role::Status).text_sm()
                .text_color(theme::muted()).child(tr("loading"))).into_any_element(),
            None => return page.into_any_element(),
        };
        let view = view_of(&state);
        // A releitura para com a janela na bandeja; de volta à vista, retoma.
        let awaiting = self.whatsapp.awaiting_pair.is_some_and(|until| Instant::now() < until);
        if self.whatsapp.poll.is_none() && self.whatsapp.busy.is_none() && should_poll(&state, awaiting) { self.whatsapp_read(true, cx); }
        if let Some((kind, code)) = failed_job(&state).filter(|_| view != View::Working) {
            let text = tr(if kind == "update" { "whatsapp_update_failed" } else { "whatsapp_install_failed" })
                .replace("{reason}", &code_text("whatsapp_job_err_", &code));
            let busy = self.whatsapp.busy.is_some();
            let retry = (view != View::Off).then(|| Button::new("whatsapp-retry-job").outline().small().label(tr("server_retry")).disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if kind == "update" { this.whatsapp_act(Act::Update, None, cx); } else { this.whatsapp_install(cx); }
                })));
            page = page.child(div().id("whatsapp-job-failed").role(Role::Alert).flex().items_center().gap_3()
                .child(div().flex_1().min_w_0().text_sm().text_color(theme::danger()).whitespace_normal().child(text))
                .children(retry));
        }
        page = match view {
            View::Off => page.children(self.render_whatsapp_off(cx)),
            View::Working => page.child(self.render_whatsapp_working(&state)),
            View::Stopped => page.child(self.render_whatsapp_problem("whatsapp-stopped", tr("whatsapp_stopped_title"), tr("whatsapp_stopped_body"),
                Some(Act::Install), cx)),
            View::Unreachable => page.child(self.render_whatsapp_problem("whatsapp-unreachable", tr("whatsapp_unreachable_title"),
                tr("whatsapp_unreachable_body"), None, cx)),
            View::Unknown => page.child(self.render_whatsapp_problem("whatsapp-unknown", tr("whatsapp_unreachable_title"), tr("invalid_response"), None, cx)),
            View::Outdated => page.child(self.render_whatsapp_problem("whatsapp-outdated", tr("whatsapp_outdated_title"), tr("whatsapp_outdated_body"),
                Some(Act::Update), cx)),
            View::Unpaired | View::Pairing => page.child(self.render_whatsapp_pair(&state, view, cx)),
            View::Connected => page.children(self.render_whatsapp_connected(&state)),
        };
        if state.installed && view != View::Working { page = page.child(self.render_whatsapp_actions(view, cx)); }
        page.into_any_element()
    }

    fn render_whatsapp_off(&self, cx: &mut Context<Self>) -> Vec<Div> {
        let busy = self.whatsapp.busy.is_some();
        let intro = self.mark(settings_box().p_4().gap_2()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_intro_title")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_intro"))), "whatsapp_intro_title");
        let risks = [("whatsapp_risk_unofficial_title", "whatsapp_risk_unofficial"), ("whatsapp_risk_readonly_title", "whatsapp_risk_readonly"),
            ("whatsapp_risk_local_title", "whatsapp_risk_local"), ("whatsapp_risk_who_title", "whatsapp_risk_who")];
        let warning = theme::warning();
        let accepted = self.whatsapp.accepted;
        let risk = div().id("whatsapp-risk").p_4().rounded(px(14.)).border_1().border_color(warning.alpha(0.3)).bg(warning.alpha(0.08))
            .flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_2()
                .child(chrome::small_icon(IconName::TriangleAlert, 15., theme::warning_text()))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning_text()).child(tr("whatsapp_risk_title"))))
            .children(risks.into_iter().map(|(lead, body)| {
                let lead = tr(lead);
                let text = format!("{lead} {}", tr(body));
                div().flex().items_start().gap_2()
                    .child(div().w(px(10.)).flex_shrink_0().text_sm().text_color(theme::muted()).child("•"))
                    .child(div().flex_1().min_w_0().text_sm().whitespace_normal().text_color(theme::muted())
                        .child(StyledText::new(text).with_highlights([(0..lead.len(),
                            HighlightStyle { color: Some(theme::text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })])))
            }))
            .child(div().pt_2().child(Checkbox::new("whatsapp-accept").checked(accepted).disabled(busy).label(tr("whatsapp_accept"))
                .on_click(cx.listener(|this, checked: &bool, _, cx| { this.whatsapp.accepted = *checked; cx.notify(); }))));
        let installing = self.whatsapp.busy == Some(Act::Install);
        let install = div().flex().child(Button::new("whatsapp-install").primary().small().label(tr("whatsapp_install"))
            .loading(installing).disabled(busy || !accepted)
            .on_click(cx.listener(|this, _, _, cx| this.whatsapp_install(cx))));
        vec![intro, self.mark(div().child(risk), "whatsapp_risk_title"), install]
    }

    fn render_whatsapp_working(&self, state: &WaState) -> Stateful<Div> {
        let job = state.job.as_ref();
        let update = job.is_some_and(|job| job.kind == "update");
        let current = job.map_or(0, |job| step_index(&job.step));
        settings_box().id("whatsapp-working").role(Role::Status).p_4().gap_3()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr(if update { "whatsapp_updating_title" } else { "whatsapp_installing_title" })))
            .children(STEPS.iter().enumerate().map(|(n, (_, label))| {
                let phase = if n < current { Step::Done } else if n == current { Step::Running } else { Step::Waiting };
                step_row(&format!("whatsapp-step-{n}"), tr(label), phase)
            }))
    }

    fn render_whatsapp_problem(&self, id: &'static str, title: String, body: String, act: Option<Act>, cx: &mut Context<Self>) -> Stateful<Div> {
        let busy = self.whatsapp.busy.is_some();
        let action = match act {
            Some(Act::Update) => Button::new("whatsapp-update").primary().small().label(tr("whatsapp_update"))
                .loading(self.whatsapp.busy == Some(Act::Update)).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_act(Act::Update, None, cx))),
            Some(_) => Button::new("whatsapp-restart").outline().small().label(tr("server_retry"))
                .loading(self.whatsapp.busy == Some(Act::Install)).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_install(cx))),
            None => Button::new("whatsapp-reload").outline().small().label(tr("server_retry")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_read(false, cx))),
        };
        settings_box().id(id).role(Role::Alert).p_4().gap_2()
            .child(div().flex().items_center().gap_2()
                .child(chrome::small_icon(IconName::TriangleAlert, 15., theme::warning()))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title)))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(body))
            .child(div().pt_1().flex().child(action))
    }

    fn render_whatsapp_pair(&self, state: &WaState, view: View, cx: &mut Context<Self>) -> Stateful<Div> {
        let w = &self.whatsapp;
        let busy = w.busy.is_some();
        let status = state.status.as_ref();
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let mut card = settings_box().id("whatsapp-pair").p_4().gap_3()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_pair_title")));
        if status.is_some_and(|s| s.state == "logged_out") { card = card.child(muted(tr("whatsapp_logged_out"))); }
        if let Some(code) = status.and_then(|s| s.error.as_ref()).and_then(code_of) {
            card = card.child(div().id("whatsapp-pair-error").role(Role::Alert).flex().items_center().gap_3()
                .child(div().flex_1().min_w_0().text_sm().text_color(theme::danger()).whitespace_normal().child(code_text("whatsapp_status_err_", &code)))
                .child(Button::new("whatsapp-pair-retry").outline().small().label(tr("server_retry")).disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.whatsapp.by_phone { this.whatsapp_request_code(cx); } else { this.whatsapp_by_qr(window, cx); }
                    }))));
        }
        let pair_code = status.and_then(|s| s.pair_code.clone()).filter(|_| view == View::Pairing);
        if w.by_phone {
            let input = w.phone.clone();
            card = card.child(muted(tr("whatsapp_pair_phone_hint")))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .children(input.map(|input| div().w(px(220.)).child(Input::new(&input).small().disabled(busy).aria_label(tr("whatsapp_pair_phone_hint")))))
                    .child(Button::new("whatsapp-request-code").primary().small().label(tr("whatsapp_pair_request_code"))
                        .loading(w.busy == Some(Act::Pair)).disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.whatsapp_request_code(cx)))))
                .children(w.phone_error.clone().map(|error| alert_line("whatsapp-phone-error", error)))
                .children(pair_code.map(|code| div().id("whatsapp-pair-code").flex().flex_col().gap_1()
                    .child(div().font_family(theme::MONO).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(code))
                    .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_pair_code_note")))))
                .child(div().flex().child(Button::new("whatsapp-back-qr").ghost().small().label(tr("whatsapp_pair_back_qr")).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.whatsapp.by_phone = false; cx.notify(); }))));
        } else if !w.qr_wanted {
            card = card.child(muted(tr("whatsapp_pair_show_hint")))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .child(Button::new("whatsapp-show-qr").primary().small().icon(IconName::QrCode).label(tr("whatsapp_pair_show"))
                        .loading(w.busy == Some(Act::Pair)).disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_qr(window, cx))))
                    .child(Button::new("whatsapp-by-phone").ghost().small().label(tr("whatsapp_pair_by_phone")).disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_phone(window, cx)))));
        } else {
            let qr = w.qr.as_ref().map(|(_, image)| image.clone()).filter(|_| view == View::Pairing);
            let square = div().flex_shrink_0().size(px(176.)).p(px(8.)).rounded(px(10.)).bg(gpui_kit::white()).flex().items_center().justify_center()
                .child(match qr {
                    Some(qr) => img(qr).size_full().into_any_element(),
                    None => div().text_size(px(12.5)).text_color(gpui_kit::black()).child(tr("whatsapp_pair_waiting")).into_any_element(),
                });
            let steps = ["whatsapp_pair_step1", "whatsapp_pair_step2", "whatsapp_pair_step3"];
            card = card.child(div().flex().flex_wrap().items_start().gap(px(18.))
                .child(div().id("whatsapp-qr").child(square))
                .child(div().flex_1().min_w(px(220.)).flex().flex_col().gap_2()
                    .children(steps.iter().enumerate().map(|(n, key)| div().flex().items_start().gap_2().text_sm()
                        .child(div().w(px(16.)).flex_shrink_0().text_color(theme::muted()).child(format!("{}.", n + 1)))
                        .child(div().flex_1().min_w_0().whitespace_normal().child(tr(key)))))
                    .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_pair_qr_note")))
                    .child(div().flex().child(Button::new("whatsapp-by-phone").ghost().small().label(tr("whatsapp_pair_by_phone")).disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_phone(window, cx)))))));
        }
        if w.qr_wanted || w.by_phone {
            card = card.child(div().flex().child(Button::new("whatsapp-cancel-pair").outline().small().label(tr("cancel")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_cancel_pair(cx)))));
        }
        card
    }

    /// Pede o QR. Já em pareamento, só mostra o que o servidor mandou: pedir de novo trocaria o código.
    fn whatsapp_by_qr(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.whatsapp.by_phone = false;
        let pairing = self.whatsapp.state.ok().map(view_of) == Some(View::Pairing) && self.whatsapp.qr.is_some();
        let failed = self.whatsapp.state.ok().and_then(|s| s.status.as_ref()).is_some_and(|s| s.error.is_some());
        if pairing && !failed {
            self.whatsapp.qr_wanted = true;
            self.whatsapp_read(false, cx);
        } else {
            self.whatsapp_pair(None, cx);
        }
    }

    fn render_whatsapp_connected(&self, state: &WaState) -> Vec<AnyElement> {
        let Some(status) = &state.status else { return Vec::new() };
        let english = crate::i18n::english();
        let t = &status.totals;
        let headline = match status.device.as_deref().filter(|d| !d.is_empty()) {
            Some(device) => tr("whatsapp_connected").replace("{device}", device),
            None => tr("whatsapp_connected_plain"),
        };
        let last = match status.last_message_at.as_deref().and_then(seconds_since) {
            Some(seconds) => tr("whatsapp_last_message").replace("{ago}", &super::side::ago(seconds)),
            None => tr("whatsapp_no_message"),
        };
        let last = match status.device_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => format!("{} {last}", tr("whatsapp_device_name").replace("{name}", name)),
            None => last,
        };
        let since = status.since.as_deref().and_then(|at| local_date(at, english));
        let messages = match since { Some(date) => tr("whatsapp_total_messages").replace("{date}", &date), None => tr("whatsapp_total_messages_plain") };
        let number = |value: u64, label: String| div().flex_1().min_w(px(140.)).p_3().rounded(px(9.)).border_1().border_color(theme::border())
            .flex().flex_col().gap(px(2.))
            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(grouped(value, english)))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(label));
        let summary = settings_box().id("whatsapp-connected").p_4().gap_2()
            .child(div().flex().items_center().gap_2()
                .child(div().size(px(9.)).rounded_full().flex_shrink_0().bg(theme::success()))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(headline)))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(last))
            .child(div().pt_2().flex().flex_wrap().gap(px(10.))
                .child(number(t.messages, messages))
                .child(number(t.chats, tr("whatsapp_total_chats").replace("{groups}", &grouped(t.groups, english))))
                .child(number(t.media_saved, tr("whatsapp_total_media"))));
        let line = |first: bool, title: String, hint: Div| div().py(px(9.)).when(!first, |el| el.border_t_1().border_color(theme::border()))
            .flex().flex_col().gap(px(2.)).child(div().text_sm().child(title)).child(hint);
        let hint = |text: String| div().text_xs().text_color(theme::muted()).whitespace_normal().child(text);
        let media = settings_box().id("whatsapp-media").p_4().gap_1()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_media_title")))
            .child(div().pb_1().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_media_desc")))
            .child(line(true, tr("whatsapp_media_phone_only").replace("{n}", &grouped(t.media_phone_only, english)), hint(tr("whatsapp_media_phone_only_hint"))))
            .child(line(false, tr("whatsapp_media_lost").replace("{n}", &grouped(t.media_lost, english)), hint(tr("whatsapp_media_lost_hint"))));
        let usage = settings_box().id("whatsapp-usage").p_4().gap_1()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_usage_title")))
            .child(line(true, tr("whatsapp_usage_tools"), div().text_xs().font_family(theme::MONO).text_color(theme::muted()).whitespace_normal().child(TOOLS)))
            .child(line(false, tr("whatsapp_usage_skill"), hint(tr("whatsapp_usage_skill_hint"))));
        vec![summary.into_any_element(), self.mark(div().child(media), "whatsapp_media_title").into_any_element(),
            self.mark(div().child(usage), "whatsapp_usage_title").into_any_element()]
    }

    fn render_whatsapp_actions(&self, view: View, cx: &mut Context<Self>) -> Div {
        let busy = self.whatsapp.busy.is_some();
        let row = div().flex().flex_wrap().items_center().gap_2()
            .when(view == View::Connected, |el| el.child(Button::new("whatsapp-disconnect").outline().small().label(tr("whatsapp_disconnect"))
                .loading(self.whatsapp.busy == Some(Act::Logout)).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.confirm_whatsapp_logout(window, cx)))))
            // Contorno de sempre com a tinta vermelha do mock: apaga sem volta, mas não é o caminho principal.
            .child(Button::new("whatsapp-wipe").small().label(tr("whatsapp_wipe"))
                .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::danger()).hover(theme::hover()).active(theme::hover()))
                .border_1().border_color(theme::border())
                .loading(self.whatsapp.busy == Some(Act::Wipe)).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.confirm_whatsapp_wipe(window, cx))))
            .child(Button::new("whatsapp-remove").ghost().small().label(tr("whatsapp_remove"))
                .loading(self.whatsapp.busy == Some(Act::Uninstall)).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.confirm_whatsapp_uninstall(window, cx))));
        self.mark(row, "whatsapp_remove")
    }
}

fn alert_line(id: &'static str, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text)
}

fn notice(id: &'static str, color: Hsla, icon: IconName, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Status).w_full().px(px(12.)).py(px(10.)).flex().items_start().gap(px(10.)).rounded(px(10.)).border_1()
        .border_color(color.alpha(0.3)).bg(color.alpha(0.08)).text_sm()
        .child(chrome::small_icon(icon, 15., color))
        .child(div().flex_1().min_w_0().whitespace_normal().child(text))
}

#[derive(Clone, Copy, PartialEq)]
enum Step { Done, Running, Waiting }

fn step_row(id: &str, label: String, phase: Step) -> Div {
    let (icon, color): (AnyElement, Hsla) = match phase {
        Step::Done => (chrome::small_icon(IconName::CircleCheck, 16., theme::success()).into_any_element(), theme::text()),
        Step::Running => (chrome::Spinner::new(SharedString::from(id.to_owned()), IconName::LoaderCircle, px(16.), theme::accent()).into_any_element(), theme::text()),
        Step::Waiting => (chrome::small_icon(IconName::Circle, 16., theme::faint()).into_any_element(), theme::muted()),
    };
    div().flex().items_center().gap(px(10.)).text_size(px(13.5)).text_color(color)
        .child(div().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(icon)).child(label)
}

#[cfg(test)]
mod tests {
    use super::{code_of, failed_job, grouped, parse_state, phone_digits, should_poll, step_index, view_of, View};
    use core::prelude::v1::test;
    use serde_json::json;

    fn state(value: serde_json::Value) -> super::WaState { parse_state(value).expect("valid state") }

    fn base(service: &str, status: Option<serde_json::Value>) -> serde_json::Value {
        json!({"installed": true, "accepted_at": "2026-11-22T10:00:00Z", "service": service, "job": null, "status": status, "qr_svg": null})
    }

    #[test]
    fn not_installed_is_off() {
        let s = state(json!({"installed": false, "accepted_at": null, "service": "absent", "job": null, "status": null, "qr_svg": null}));
        assert_eq!(view_of(&s), View::Off);
    }

    #[test]
    fn running_job_wins_over_everything() {
        let s = state(json!({"installed": false, "accepted_at": "x", "service": "absent",
            "job": {"kind": "install", "step": "service", "done": false, "error": null, "outcome": null}, "status": null, "qr_svg": null}));
        assert_eq!(view_of(&s), View::Working);
        assert!(should_poll(&s, false));
        assert_eq!(step_index("service"), 1);
        assert_eq!(step_index("whatever"), 0);
    }

    #[test]
    fn finished_job_with_error_is_reported_with_its_kind() {
        let s = state(json!({"installed": true, "accepted_at": "x", "service": "running",
            "job": {"kind": "update", "step": "download", "done": true, "error": {"code": "whatsapp_download_failed"}, "outcome": null},
            "status": {"state": "connected"}, "qr_svg": null}));
        assert_eq!(failed_job(&s), Some(("update".into(), "whatsapp_download_failed".into())));
        assert_eq!(view_of(&s), View::Connected);
        assert!(!should_poll(&s, false));
    }

    #[test]
    fn installed_service_states() {
        assert_eq!(view_of(&state(base("stopped", None))), View::Stopped);
        assert_eq!(view_of(&state(base("absent", None))), View::Stopped);
        assert_eq!(view_of(&state(base("running", None))), View::Unreachable);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "unpaired"}))))), View::Unpaired);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "logged_out"}))))), View::Unpaired);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "pairing"}))))), View::Pairing);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "outdated"}))))), View::Outdated);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "connected"}))))), View::Connected);
        assert_eq!(view_of(&state(base("running", Some(json!({"state": "novo"}))))), View::Unknown);
    }

    #[test]
    fn pairing_polls_and_unpaired_only_right_after_asking() {
        let pairing = state(base("running", Some(json!({"state": "pairing"}))));
        let unpaired = state(base("running", Some(json!({"state": "unpaired"}))));
        assert!(should_poll(&pairing, false));
        assert!(!should_poll(&unpaired, false));
        assert!(should_poll(&unpaired, true));
    }

    #[test]
    fn unknown_service_is_invalid() {
        assert!(parse_state(base("sleeping", None)).is_err());
        assert!(parse_state(json!({"service": "running"})).is_err());
    }

    #[test]
    fn error_code_from_text_or_object() {
        assert_eq!(code_of(&json!("pair_timeout")), Some("pair_timeout".into()));
        assert_eq!(code_of(&json!({"code": "pair_timeout"})), Some("pair_timeout".into()));
        assert_eq!(code_of(&json!({"code": ""})), None);
        assert_eq!(code_of(&json!(3)), None);
    }

    #[test]
    fn phone_keeps_only_digits_in_range() {
        assert_eq!(phone_digits("+55 (64) 99999-1234"), Some("5564999991234".into()));
        assert_eq!(phone_digits("64 9999"), None);
        assert_eq!(phone_digits("5564abc99991234"), None);
        assert_eq!(phone_digits("1234567890123456"), None);
    }

    #[test]
    fn thousands_follow_the_language() {
        assert_eq!(grouped(0, false), "0");
        assert_eq!(grouped(566, false), "566");
        assert_eq!(grouped(4195, false), "4.195");
        assert_eq!(grouped(76501, true), "76,501");
        assert_eq!(grouped(1234567, false), "1.234.567");
    }
}
