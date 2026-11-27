//! WhatsApp no servidor conectado (leitura e envio de rascunho aprovado pelo dono): aceite do risco, instalação, pareamento do
//! celular e o que já foi guardado.
//! Fala só com `/api/whatsapp*` (rotas do dono); o serviço e os dados ficam no servidor.
use super::*;
use super::accounts::{step_item, step_states};
use super::device::Remote;
use super::harness::grouped_number;
use super::plugins::date_text;
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
const TOOLS: &str = "whatsapp_chats · whatsapp_read · whatsapp_search · whatsapp_request_media · whatsapp_draft";

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
    /// `installed`, `updated` ou `already_latest`, depois de terminar sem erro.
    #[serde(default)]
    outcome: Option<Value>,
}

/// A versão que falhou ao trocar e por quê: a mesma tag não é tentada de novo sozinha.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct UpdateFailed {
    #[serde(default)]
    tag: String,
    #[serde(default)]
    code: String,
}

/// Versão mais nova que a instalada, achada pela última procura.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct UpdateInfo {
    latest: String,
    /// A versão nova muda o formato do arquivo de mensagens (ou não se sabe): só entra com confirmação.
    #[serde(default)]
    migration: bool,
    #[serde(default)]
    failed: Option<UpdateFailed>,
}

/// Valor fora destes três faz a leitura falhar: resposta inválida, não tela inventada.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Service { Running, Stopped, Absent }

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct WaState {
    installed: bool,
    #[serde(default)]
    accepted_at: Option<String>,
    service: Service,
    #[serde(default)]
    job: Option<Job>,
    #[serde(default)]
    status: Option<Status>,
    #[serde(default)]
    qr_svg: Option<String>,
    /// Com o serviço parado, `whatsapp_schema_newer`: os dados são de um binário mais novo.
    #[serde(default)]
    status_error: Option<String>,
    #[serde(default)]
    installed_tag: Option<String>,
    /// Hora da última procura, com ou sem versão nova.
    #[serde(default)]
    update_checked_at: Option<String>,
    #[serde(default)]
    update: Option<UpdateInfo>,
}

fn parse_state(value: Value) -> Result<WaState, String> {
    serde_json::from_value(value).map_err(|_| tr("invalid_response"))
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
enum View { Off, Working, Stopped, BackupFailed, SchemaNewer, SchemaNewerLatest, Unreachable, Unpaired, Pairing, Connected, Outdated, Unknown }

/// Em ordem: job em curso, desinstalado, serviço de pé (pelo `status`), dados mais novos e parado.
fn view_of(state: &WaState) -> View {
    let working = state.job.as_ref().is_some_and(|job| !job.done);
    let schema_newer = state.status_error.as_deref() == Some("whatsapp_schema_newer");
    match (working, state.installed, state.service, state.status.as_ref()) {
        (true, ..) => View::Working,
        (_, false, ..) => View::Off,
        (_, _, Service::Running, None) => View::Unreachable,
        (_, _, Service::Running, Some(status)) => match status.state.as_str() {
            "connected" => View::Connected,
            "outdated" => View::Outdated,
            "pairing" => View::Pairing,
            "unpaired" | "logged_out" => View::Unpaired,
            _ => View::Unknown,
        },
        // Religar não adianta: só um binário mais novo abre esses dados. Atualizar que não achou outro também não.
        _ if schema_newer && already_latest(state) => View::SchemaNewerLatest,
        _ if schema_newer => View::SchemaNewer,
        // A cópia do arquivo não foi gravada e nada foi convertido: religar a mesma versão resolve quando houver espaço.
        _ if state.status_error.as_deref() == Some("whatsapp_backup_failed") => View::BackupFailed,
        _ => View::Stopped,
    }
}

/// O pedido de QR não começou: a releitura diz desvinculado sem pedido em voo nem a espera logo depois dele.
fn pair_not_started(view: View, pairing_in_flight: bool, awaiting: bool) -> bool {
    view == View::Unpaired && !pairing_in_flight && !awaiting
}

/// A última atualização terminou sem achar release mais nova que a instalada.
fn already_latest(state: &WaState) -> bool {
    state.job.as_ref().is_some_and(|job| job.done && job.kind == "update" && job.outcome.as_ref().and_then(Value::as_str) == Some("already_latest"))
}

/// Instalar ou religar vale com o aceite gravado no servidor ou marcado agora.
fn install_accepted(state: &WaState, accepted_now: bool) -> bool {
    accepted_now || state.accepted_at.is_some()
}

/// Sem aceite gravado, a tela pede o aceite antes de religar.
fn asks_accept(state: &WaState) -> bool {
    state.accepted_at.is_none()
}

/// O job do dono que terminou com erro: o tipo dele (`install`/`update`) e o código. O da passada automática
/// (`auto_update`) aparece pela `update.failed`, nunca como esta faixa.
fn failed_job(state: &WaState) -> Option<(String, String)> {
    let job = state.job.as_ref().filter(|job| job.done && matches!(job.kind.as_str(), "install" | "update"))?;
    let code = job.error.as_ref().and_then(code_of)?;
    Some((job.kind.clone(), code))
}

/// A linha da versão no cartão de conectado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionRow { Current, Failed, Available, Migration }

/// Só com a versão instalada conhecida; quem chama já é o cartão de conectado.
fn version_row(state: &WaState) -> Option<VersionRow> {
    state.installed_tag.as_ref()?;
    Some(match &state.update {
        None => VersionRow::Current,
        // A tentativa falhou: quem oferece de novo é a faixa do topo; a linha só procura.
        Some(update) if update.failed.is_some() => VersionRow::Failed,
        Some(update) if update.migration => VersionRow::Migration,
        Some(_) => VersionRow::Available,
    })
}

/// A troca que falhou e ficou marcada, sem job em curso nem a faixa do job do dono: tag, código e a versão que roda.
/// Só com o serviço de pé: a frase diz que a instalada continua rodando; parado, a vista de parado já explica.
fn update_failure(state: &WaState) -> Option<(String, String, String)> {
    if state.service != Service::Running || state.job.as_ref().is_some_and(|job| !job.done) || failed_job(state).is_some() { return None; }
    let failed = state.update.as_ref()?.failed.as_ref().filter(|f| !f.tag.is_empty() && !f.code.is_empty())?;
    Some((failed.tag.clone(), failed.code.clone(), state.installed_tag.clone()?))
}

/// A versão nova e a instalada, para o texto da confirmação.
fn migration_tags(state: &WaState) -> Option<(String, String)> {
    Some((state.update.as_ref()?.latest.clone(), state.installed_tag.clone()?))
}

/// O que a releitura diz da atualização que esta tela pediu, lida depois da resposta do POST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnUpdate { Running, AskMigration, Finished }

fn own_update_step(state: &WaState) -> OwnUpdate {
    match state.job.as_ref() {
        Some(job) if !job.done => OwnUpdate::Running,
        Some(job) if job.kind == "update" && job.outcome.as_ref().and_then(Value::as_str) == Some("needs_confirm") => OwnUpdate::AskMigration,
        _ => OwnUpdate::Finished,
    }
}

/// Atualizar recusado com `whatsapp_busy`: com um job em curso a página o acompanha; sem ele, um envio segura a vez.
fn update_refused_text(state: &WaState) -> Option<String> {
    (view_of(state) != View::Working).then(|| tr("whatsapp_update_busy"))
}

fn should_poll(state: &WaState, awaiting_pair: bool) -> bool {
    matches!(view_of(state), View::Working | View::Pairing) || (awaiting_pair && view_of(state) == View::Unpaired)
}

/// Só algarismos, de 10 a 15 (código do país, DDD e número); o `+`, espaços e traços somem.
fn phone_digits(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let other = text.chars().any(|c| !c.is_ascii_digit() && !matches!(c, '+' | ' ' | '-' | '(' | ')' | '.'));
    (!other && (10..=15).contains(&digits.len())).then_some(digits)
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

/// Segundos de espera por ação. Desvincular dá 30 s ao binário no servidor: a tela espera mais, senão estoura
/// antes. Remover ainda para, desregistra e apaga o serviço depois do logout.
fn act_timeout(act: Act) -> u64 {
    match act { Act::Uninstall => 90, Act::Logout | Act::Wipe => 45, _ => 30 }
}

pub(super) enum WhatsAppReply {
    Loaded(u64, Result<Value, Failure>),
    Acted(Act, Result<Value, Failure>),
}

#[derive(Default)]
pub(in crate::app) struct WhatsApp {
    state: Remote<WaState>,
    /// Uma ação por vez: a resposta dela é a única em voo (trocar de servidor recria tudo e descarta a velha).
    busy: Option<Act>,
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
    /// Atualizar sem confirmação pedido nesta tela: se o job parar em `needs_confirm`, a confirmação abre nesta janela.
    own_update: Option<AnyWindowHandle>,
    /// O último Atualizar voltou `whatsapp_busy`: a releitura decide se há job para acompanhar ou se avisa.
    update_refused: bool,
}

impl Drop for WhatsApp {
    fn drop(&mut self) { if let Some(task) = self.poll.take() { task.abort(); } }
}

impl WhatsApp {
    /// O pedido de pareamento acabou: QR escondido, formulário do número fechado, sem espera.
    fn reset_pairing(&mut self) {
        (self.qr_wanted, self.by_phone, self.awaiting_pair, self.phone_error) = (false, false, None, None);
    }
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
        w.reset_pairing();
        // O aviso de tirar o aparelho no celular fica até outra ação: a pessoa pode não ter lido ainda.
        if w.busy.is_none() { w.error = None; }
        self.whatsapp_read(false, cx);
    }

    /// Fora da página: a releitura para e o QR sai da memória, o desenho e o SVG do último estado.
    pub(super) fn whatsapp_page_left(&mut self) {
        let w = &mut self.whatsapp;
        if let Some(task) = w.poll.take() { task.abort(); }
        w.state.seq += 1;
        w.state.loading = false;
        (w.qr, w.qr_wanted) = (None, false);
        (w.own_update, w.update_refused) = (None, false);
        if let Some(Ok(state)) = w.state.value.as_mut() { state.qr_svg = None; }
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
        (w.busy, w.error) = (Some(act), None);
        if act != Act::Wipe { w.unlink_on_phone = false; }
        let path = match act { Act::Install => "install", Act::Update => "update", Act::Pair => "pair", Act::Logout => "logout",
            Act::Wipe => "wipe", Act::Uninstall => "uninstall" };
        let done = self.whatsapp_send_later();
        self.runtime.spawn(async move {
            done(WhatsAppReply::Acted(act, api.server_send(reqwest::Method::POST, &["whatsapp", path], body, act_timeout(act)).await)).await
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
                    let awaiting = w.awaiting_pair.is_some_and(|until| Instant::now() < until);
                    match view_of(state) {
                        View::Connected => w.reset_pairing(),
                        // O pedido falhou ou a espera passou sem pareamento: o botão de pedir o QR volta.
                        view if pair_not_started(view, w.busy == Some(Act::Pair), awaiting) => (w.qr_wanted, w.awaiting_pair) = (false, None),
                        View::Unpaired => {}
                        _ => w.awaiting_pair = None,
                    }
                }
                let mut ask = None;
                // Só a releitura feita depois da resposta do POST fala do job que ele abriu.
                if let (Ok(state), None) = (&result, w.busy) {
                    if std::mem::take(&mut w.update_refused) { if let Some(text) = update_refused_text(state) { w.error = Some(text); } }
                    if let Some(handle) = w.own_update {
                        match own_update_step(state) {
                            OwnUpdate::Running => {}
                            OwnUpdate::AskMigration => { w.own_update = None; ask = migration_tags(state).map(|_| handle); }
                            OwnUpdate::Finished => w.own_update = None,
                        }
                    }
                }
                let poll = result.as_ref().is_ok_and(|state| should_poll(state, w.awaiting_pair.is_some_and(|until| Instant::now() < until)));
                w.state.finish(seq, result);
                w.poll = None;
                if poll && self.settings == Some(Page::WhatsApp) && !self.window_tray.hidden { self.whatsapp_read(true, cx); }
                // Fora do empréstimo de quem recebeu a resposta: o diálogo abre na janela onde o clique aconteceu.
                if let Some(handle) = ask.filter(|_| self.settings == Some(Page::WhatsApp)) {
                    let this = cx.entity().downgrade();
                    cx.defer(move |cx| { let _ = cx.update_window(handle, |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.confirm_whatsapp_migration(window, cx));
                    }); });
                }
            }
            WhatsAppReply::Acted(act, result) => {
                let w = &mut self.whatsapp;
                w.busy = None;
                match result {
                    Ok(value) => {
                        if act == Act::Wipe { w.unlink_on_phone = value.get("logout").and_then(Value::as_str) == Some("not_done"); }
                        if act == Act::Pair { w.awaiting_pair = Some(Instant::now() + PAIR_GRACE); }
                    }
                    // Outro job já em curso: a página passa a acompanhar esse; sem job, a releitura avisa.
                    Err(error) if act == Act::Update && error.code.as_deref() == Some("whatsapp_busy") => (w.update_refused, w.own_update) = (true, None),
                    Err(error) if act == Act::Install && error.code.as_deref() == Some("whatsapp_busy") => {}
                    Err(error) => {
                        if act == Act::Update { w.own_update = None; }
                        w.error = Some(failure_text(&error));
                    }
                }
                // A ação pode ter chegado antes da queda; a releitura mostra o estado real. Fora da página não relê:
                // a reabertura já consulta o servidor, e o QR não volta à memória com a página fechada.
                if self.settings == Some(Page::WhatsApp) { self.whatsapp_read(false, cx); }
            }
        }
        cx.notify();
    }

    fn whatsapp_install(&mut self, cx: &mut Context<Self>) {
        let accepted = self.whatsapp.accepted || self.whatsapp.state.ok().is_some_and(|s| s.accepted_at.is_some());
        if !accepted { return; }
        self.whatsapp_act(Act::Install, Some(json!({"accept": true})), cx);
    }

    /// Atualizar sem confirmação: se a versão nova mudar o formato, o job para em `needs_confirm` e esta tela pergunta.
    fn whatsapp_update(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.whatsapp.busy.is_some() { return; }
        self.whatsapp.own_update = Some(window.window_handle());
        self.whatsapp_act(Act::Update, None, cx);
    }

    /// `connection`: a do servidor que mostrou a confirmação; trocada no meio, a troca não vai a outro servidor.
    fn whatsapp_update_confirmed(&mut self, connection: u64, cx: &mut Context<Self>) {
        if connection != self.connection {
            self.whatsapp.error = Some(tr("whatsapp_migration_other_server"));
            cx.notify();
            return;
        }
        if self.whatsapp.busy.is_some() { return; }
        self.whatsapp.own_update = None;
        self.whatsapp_act(Act::Update, Some(json!({"confirm_migration": true})), cx);
    }

    /// O diálogo de sempre (o do Apagar dados), com o botão primário: a troca guarda a cópia e volta sozinha se falhar.
    fn confirm_whatsapp_migration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((latest, installed)) = self.whatsapp.state.ok().and_then(migration_tags) else { return };
        let body = format!("{}\n\n{}", tr("whatsapp_migration_body").replace("{tag}", &latest).replace("{installed}", &installed),
            tr("whatsapp_migration_backup").replace("{installed}", &installed));
        let (this, connection) = (cx.entity().downgrade(), self.connection);
        chrome::confirm_alert(window, cx, tr("whatsapp_migration_title").replace("{tag}", &latest), body, tr("whatsapp_update"),
            ButtonVariant::Primary, move |_, cx| { let _ = this.update(cx, |this, cx| this.whatsapp_update_confirmed(connection, cx)); true });
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
        self.whatsapp.reset_pairing();
        cx.notify();
    }

    fn confirm_whatsapp_logout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("whatsapp_disconnect_title"), tr("whatsapp_disconnect_body"), tr("whatsapp_disconnect"),
            ButtonVariant::Danger, move |_, cx| { let _ = this.update(cx, |this, cx| this.whatsapp_act(Act::Logout, None, cx)); true });
    }

    fn confirm_whatsapp_wipe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.whatsapp.state.ok().is_some_and(|s| s.service == Service::Running);
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
        // A releitura para com a janela na bandeja; de volta à vista, retoma.
        let awaiting = self.whatsapp.awaiting_pair.is_some_and(|until| Instant::now() < until);
        if self.whatsapp.poll.is_none() && self.whatsapp.busy.is_none() && self.whatsapp.state.ok().is_some_and(|s| should_poll(s, awaiting)) {
            self.whatsapp_read(true, cx);
        }
        let state = match &self.whatsapp.state.value {
            Some(Ok(state)) => state,
            Some(Err(error)) => return page.child(alert_line("whatsapp-load-error", error.clone()))
                .child(div().flex().child(Button::new("whatsapp-retry-load").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.whatsapp.error = None; this.whatsapp_read(false, cx); })))).into_any_element(),
            None if self.whatsapp.state.loading => return page.child(div().id("whatsapp-loading").role(Role::Status).text_sm()
                .text_color(theme::muted()).child(tr("loading"))).into_any_element(),
            None => return page.into_any_element(),
        };
        let view = view_of(state);
        if let Some((kind, code)) = failed_job(state).filter(|_| view != View::Working) {
            let text = tr(if kind == "update" { "whatsapp_update_failed" } else { "whatsapp_install_failed" })
                .replace("{reason}", &code_text("whatsapp_job_err_", &code));
            let busy = self.whatsapp.busy.is_some();
            // Reinstalar exige o aceite, o gravado no servidor ou o marcado agora.
            let accepted = kind == "update" || install_accepted(state, self.whatsapp.accepted);
            let retry = Button::new("whatsapp-retry-job").outline().small().label(tr("server_retry")).disabled(busy || !accepted)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if kind == "update" { this.whatsapp_update(window, cx); } else { this.whatsapp_install(cx); }
                }));
            page = page.child(alert_row("whatsapp-job-failed", text, retry));
        }
        if let Some((tag, code, installed)) = update_failure(state).filter(|_| view != View::Working) {
            let text = tr("whatsapp_auto_update_failed").replace("{tag}", &tag)
                .replace("{reason}", &code_text("whatsapp_job_err_", &code)).replace("{installed}", &installed);
            let migration = state.update.as_ref().is_some_and(|update| update.migration);
            let busy = self.whatsapp.busy.is_some();
            let retry = Button::new("whatsapp-retry-auto-update").outline().small().label(tr("server_retry"))
                .loading(self.whatsapp.busy == Some(Act::Update)).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if migration { this.confirm_whatsapp_migration(window, cx); } else { this.whatsapp_update(window, cx); }
                }));
            page = page.child(alert_row("whatsapp-auto-update-failed", text, retry));
        }
        page = match view {
            View::Off => page.children(self.render_whatsapp_off(cx)),
            View::Working => page.child(render_working(state)),
            View::Stopped => page.children(self.render_whatsapp_stopped(state, tr("whatsapp_stopped_body"), cx)),
            View::BackupFailed => page.children(self.render_whatsapp_stopped(state, tr("whatsapp_status_err_backup_failed"), cx)),
            View::SchemaNewer => page.child(self.render_whatsapp_problem("whatsapp-schema-newer", tr("whatsapp_schema_newer_title"),
                tr("whatsapp_schema_newer_body"), Some(Act::Update), cx)),
            // Botão secundário: só procura de novo, porque a versão posterior pode sair com o job ainda na memória.
            View::SchemaNewerLatest => page.child(problem_box("whatsapp-schema-newer-latest", tr("whatsapp_schema_newer_title"),
                tr("whatsapp_schema_newer_latest_body"))
                .child(div().pt_1().flex().child(Button::new("whatsapp-check-again").outline().small().label(tr("whatsapp_check_again"))
                    .loading(self.whatsapp.busy == Some(Act::Update)).disabled(self.whatsapp.busy.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.whatsapp_update(window, cx)))))),
            View::Unreachable => page.child(self.render_whatsapp_problem("whatsapp-unreachable", tr("whatsapp_unreachable_title"),
                tr("whatsapp_unreachable_body"), None, cx)),
            View::Unknown => page.child(self.render_whatsapp_problem("whatsapp-unknown", tr("whatsapp_unreachable_title"), tr("invalid_response"), None, cx)),
            View::Outdated => page.child(self.render_whatsapp_problem("whatsapp-outdated", tr("whatsapp_outdated_title"), tr("whatsapp_outdated_body"),
                Some(Act::Update), cx)),
            View::Unpaired | View::Pairing => page.child(self.render_whatsapp_pair(state, view, cx)),
            View::Connected => page.children(self.render_whatsapp_connected(state, cx)),
        };
        if state.installed && view != View::Working { page = page.child(self.render_whatsapp_actions(view, cx)); }
        page.into_any_element()
    }

    fn render_whatsapp_off(&self, cx: &mut Context<Self>) -> Vec<Div> {
        let busy = self.whatsapp.busy.is_some();
        let intro = self.mark(settings_box().p_4().gap_2()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_intro_title")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_intro"))), "whatsapp_intro_title");
        let accepted = self.whatsapp.accepted;
        let installing = self.whatsapp.busy == Some(Act::Install);
        let install = div().flex().child(Button::new("whatsapp-install").primary().small().label(tr("whatsapp_install"))
            .loading(installing).disabled(busy || !accepted)
            .on_click(cx.listener(|this, _, _, cx| this.whatsapp_install(cx))));
        vec![intro, self.render_whatsapp_accept(cx), install]
    }

    /// A caixa "Entendo os riscos" com o texto dos riscos, a mesma do desligado.
    fn render_whatsapp_accept(&self, cx: &mut Context<Self>) -> Div {
        let risk = risk_box(Checkbox::new("whatsapp-accept").checked(self.whatsapp.accepted).disabled(self.whatsapp.busy.is_some())
            .label(tr("whatsapp_accept"))
            .on_click(cx.listener(|this, checked: &bool, _, cx| { this.whatsapp.accepted = *checked; cx.notify(); })));
        self.mark(div().child(risk), "whatsapp_risk_title")
    }

    /// Parado. Sem aceite gravado (a migração instala sem gravá-lo), religar pede o mesmo aceite do desligado.
    fn render_whatsapp_stopped(&self, state: &WaState, body: String, cx: &mut Context<Self>) -> Vec<Div> {
        let busy = self.whatsapp.busy.is_some();
        let retry = Button::new("whatsapp-restart").outline().small().label(tr("server_retry"))
            .loading(self.whatsapp.busy == Some(Act::Install)).disabled(busy || !install_accepted(state, self.whatsapp.accepted))
            .on_click(cx.listener(|this, _, _, cx| this.whatsapp_install(cx)));
        let problem = div().child(problem_box("whatsapp-stopped", tr("whatsapp_stopped_title"), body)
            .child(div().pt_1().flex().child(retry)));
        if asks_accept(state) { vec![problem, self.render_whatsapp_accept(cx)] } else { vec![problem] }
    }

    fn render_whatsapp_problem(&self, id: &'static str, title: String, body: String, act: Option<Act>, cx: &mut Context<Self>) -> Stateful<Div> {
        let busy = self.whatsapp.busy.is_some();
        let action = match act {
            Some(Act::Update) => Button::new("whatsapp-update").primary().small().label(tr("whatsapp_update"))
                .loading(self.whatsapp.busy == Some(Act::Update)).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.whatsapp_update(window, cx))),
            Some(_) => Button::new("whatsapp-restart").outline().small().label(tr("server_retry"))
                .loading(self.whatsapp.busy == Some(Act::Install)).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_install(cx))),
            None => Button::new("whatsapp-reload").outline().small().label(tr("server_retry")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.whatsapp_read(false, cx))),
        };
        problem_box(id, title, body).child(div().pt_1().flex().child(action))
    }

    fn render_whatsapp_pair(&self, state: &WaState, view: View, cx: &mut Context<Self>) -> Stateful<Div> {
        let w = &self.whatsapp;
        let busy = w.busy.is_some();
        let status = state.status.as_ref();
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let mut card = pair_card();
        if status.is_some_and(|s| s.state == "logged_out") { card = card.child(muted(tr("whatsapp_logged_out"))); }
        if let Some(code) = status.and_then(|s| s.error.as_ref()).and_then(code_of) {
            card = card.child(alert_row("whatsapp-pair-error", code_text("whatsapp_status_err_", &code),
                Button::new("whatsapp-pair-retry").outline().small().label(tr("server_retry")).disabled(busy)
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
                    .child(Button::new("whatsapp-show-qr").primary().small().label(tr("whatsapp_pair_show"))
                        .loading(w.busy == Some(Act::Pair)).disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_qr(window, cx))))
                    .child(Button::new("whatsapp-by-phone").ghost().small().label(tr("whatsapp_pair_by_phone")).disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_phone(window, cx)))));
        } else {
            let qr = w.qr.as_ref().map(|(_, image)| image.clone()).filter(|_| view == View::Pairing);
            card = card.child(qr_row(qr, Button::new("whatsapp-by-phone").ghost().small().label(tr("whatsapp_pair_by_phone")).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.whatsapp_by_phone(window, cx)))));
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

    fn render_whatsapp_connected(&self, state: &WaState, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(status) = &state.status else { return Vec::new() };
        let t = &status.totals;
        let mut summary = connected_summary(ConnectedFacts {
            device: status.device.as_deref(), device_name: status.device_name.as_deref(),
            last_seconds: status.last_message_at.as_deref().and_then(seconds_since), since: status.since.as_deref(),
            messages: t.messages, chats: t.chats, groups: t.groups, media_saved: t.media_saved,
        });
        if let Some(row) = version_row(state) { summary = summary.child(self.render_whatsapp_version(state, row, cx)); }
        let media = settings_box().id("whatsapp-media").p_4().gap_1()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_media_title")))
            .child(div().pb_1().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_media_desc")))
            .child(info_line(true, tr("whatsapp_media_phone_only").replace("{n}", &grouped_number(t.media_phone_only)), hint(tr("whatsapp_media_phone_only_hint"))))
            .child(info_line(false, tr("whatsapp_media_lost").replace("{n}", &grouped_number(t.media_lost)), hint(tr("whatsapp_media_lost_hint"))));
        vec![summary.into_any_element(), self.mark(div().child(media), "whatsapp_media_title").into_any_element(),
            self.mark(div().child(usage_box()), "whatsapp_usage_title").into_any_element()]
    }

    /// A versão instalada abaixo dos números; versão nova não é falha, então não vira faixa.
    fn render_whatsapp_version(&self, state: &WaState, row: VersionRow, cx: &mut Context<Self>) -> Div {
        let busy = self.whatsapp.busy.is_some();
        let latest = state.update.as_ref().map(|update| update.latest.clone()).unwrap_or_default();
        let checked = state.update_checked_at.as_deref().and_then(seconds_since).map(super::side::ago);
        let mut head = div().flex().flex_wrap().items_center().gap_1().text_sm()
            .child(tr("whatsapp_version_label"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(state.installed_tag.clone().unwrap_or_default()));
        if matches!(row, VersionRow::Available | VersionRow::Migration) {
            head = head.child(div().text_color(theme::muted()).child("·"))
                .child(div().text_color(theme::success()).child(tr("whatsapp_update_available").replace("{tag}", &latest)));
        }
        let (note, color) = match row {
            VersionRow::Current => (checked.map_or_else(|| tr("whatsapp_version_never"), |ago| tr("whatsapp_version_current").replace("{ago}", &ago)),
                theme::muted()),
            VersionRow::Failed => (checked.map_or_else(|| tr("whatsapp_version_never"), |ago| tr("whatsapp_version_checked").replace("{ago}", &ago)),
                theme::muted()),
            VersionRow::Available => (tr("whatsapp_update_quick"), theme::muted()),
            VersionRow::Migration => (tr("whatsapp_update_migration"), theme::warning_text()),
        };
        let button = match row {
            VersionRow::Current | VersionRow::Failed => Button::new("whatsapp-check-update").outline().small().label(tr("whatsapp_check_update"))
                .on_click(cx.listener(|this, _, window, cx| this.whatsapp_update(window, cx))),
            VersionRow::Available => Button::new("whatsapp-update-now").primary().small().label(tr("whatsapp_update"))
                .on_click(cx.listener(|this, _, window, cx| this.whatsapp_update(window, cx))),
            VersionRow::Migration => Button::new("whatsapp-update-ask").primary().small().label(tr("whatsapp_update_ask"))
                .on_click(cx.listener(|this, _, window, cx| this.confirm_whatsapp_migration(window, cx))),
        }.loading(self.whatsapp.busy == Some(Act::Update)).disabled(busy);
        div().mt_2().pt_3().border_t_1().border_color(theme::border()).flex().items_center().gap_3()
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(head)
                .child(div().text_xs().text_color(color).whitespace_normal().child(note)))
            .child(div().flex_shrink_0().child(button))
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

// Partes de vista que a página e a miniatura da novidade (`crate::news`) desenham iguais.

/// O aviso dos riscos com a caixa do aceite, que quem chama monta (com ou sem clique).
pub(crate) fn risk_box(accept: Checkbox) -> Stateful<Div> {
    let risks = [("whatsapp_risk_unofficial_title", "whatsapp_risk_unofficial"), ("whatsapp_risk_send_title", "whatsapp_risk_send"),
        ("whatsapp_risk_local_title", "whatsapp_risk_local"), ("whatsapp_risk_who_title", "whatsapp_risk_who")];
    let warning = theme::warning();
    div().id("whatsapp-risk").p_4().rounded(px(14.)).border_1().border_color(warning.alpha(0.3)).bg(warning.alpha(0.08))
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
        // A borda do kit some sobre o âmbar: a opção ganha caixa própria, como nos alertas do app.
        .child(div().mt_2().px(px(10.)).py(px(8.)).rounded(px(8.)).border_1().border_color(theme::border_strong()).bg(theme::inset())
            .child(accept))
}

/// A caixa do pareamento com o título; o conteúdo entra depois.
pub(crate) fn pair_card() -> Stateful<Div> {
    settings_box().id("whatsapp-pair").p_4().gap_3()
        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_pair_title")))
}

/// O QR ao lado dos passos; sem QR, o quadrado diz que está esperando o código.
pub(crate) fn qr_row(qr: Option<Arc<Image>>, by_phone: Button) -> Div {
    let square = div().flex_shrink_0().size(px(176.)).p(px(8.)).rounded(px(10.)).bg(gpui_kit::white()).flex().items_center().justify_center()
        .child(match qr {
            Some(qr) => img(qr).size_full().into_any_element(),
            None => div().text_size(px(12.5)).text_color(gpui_kit::black()).child(tr("whatsapp_pair_waiting")).into_any_element(),
        });
    let steps = ["whatsapp_pair_step1", "whatsapp_pair_step2", "whatsapp_pair_step3"];
    div().flex().flex_wrap().items_start().gap(px(18.))
        .child(div().id("whatsapp-qr").child(square))
        .child(div().flex_1().min_w(px(220.)).flex().flex_col().gap_2()
            .children(steps.iter().enumerate().map(|(n, key)| div().flex().items_start().gap_2().text_sm()
                .child(div().w(px(16.)).flex_shrink_0().text_color(theme::muted()).child(format!("{}.", n + 1)))
                .child(div().flex_1().min_w_0().whitespace_normal().child(tr(key)))))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("whatsapp_pair_qr_note")))
            .child(div().flex().child(by_phone)))
}

/// O que o resumo de conectado mostra; `device` já vem mascarado do servidor.
pub(crate) struct ConnectedFacts<'a> {
    pub device: Option<&'a str>,
    pub device_name: Option<&'a str>,
    pub last_seconds: Option<f64>,
    /// Data RFC 3339 da mensagem mais antiga guardada.
    pub since: Option<&'a str>,
    pub messages: u64,
    pub chats: u64,
    pub groups: u64,
    pub media_saved: u64,
}

pub(crate) fn connected_summary(c: ConnectedFacts<'_>) -> Stateful<Div> {
    let headline = match c.device.filter(|d| !d.is_empty()) {
        Some(device) => tr("whatsapp_connected").replace("{device}", device),
        None => tr("whatsapp_connected_plain"),
    };
    let last = match c.last_seconds {
        Some(seconds) => tr("whatsapp_last_message").replace("{ago}", &super::side::ago(seconds)),
        None => tr("whatsapp_no_message"),
    };
    let last = match c.device_name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("{} {last}", tr("whatsapp_device_name").replace("{name}", name)),
        None => last,
    };
    let since = c.since.and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).map(|at| date_text(&at.with_timezone(&chrono::Local)));
    let messages = match since { Some(date) => tr("whatsapp_total_messages").replace("{date}", &date), None => tr("whatsapp_total_messages_plain") };
    let number = |value: u64, label: String| div().flex_1().min_w(px(140.)).p_3().rounded(px(9.)).border_1().border_color(theme::border())
        .flex().flex_col().gap(px(2.))
        .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(grouped_number(value)))
        .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(label));
    settings_box().id("whatsapp-connected").p_4().gap_2()
        .child(div().flex().items_center().gap_2()
            .child(div().size(px(9.)).rounded_full().flex_shrink_0().bg(theme::success()))
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(headline)))
        .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(last))
        .child(div().pt_2().flex().flex_wrap().gap(px(10.))
            .child(number(c.messages, messages))
            .child(number(c.chats, tr("whatsapp_total_chats").replace("{groups}", &grouped_number(c.groups))))
            .child(number(c.media_saved, tr("whatsapp_total_media"))))
}

/// Como as sessões leem: as ferramentas e a skill.
pub(crate) fn usage_box() -> Stateful<Div> {
    settings_box().id("whatsapp-usage").p_4().gap_1()
        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("whatsapp_usage_title")))
        .child(info_line(true, tr("whatsapp_usage_tools"), div().text_xs().font_family(theme::MONO).text_color(theme::muted()).whitespace_normal().child(TOOLS)))
        .child(info_line(false, tr("whatsapp_usage_skill"), hint(tr("whatsapp_usage_skill_hint"))))
}

fn info_line(first: bool, title: String, hint: Div) -> Div {
    div().py(px(9.)).when(!first, |el| el.border_t_1().border_color(theme::border()))
        .flex().flex_col().gap(px(2.)).child(div().text_sm().child(title)).child(hint)
}

/// O aviso de um problema, sem ação: quem tem ação a acrescenta embaixo.
fn problem_box(id: &'static str, title: String, body: String) -> Stateful<Div> {
    settings_box().id(id).role(Role::Alert).p_4().gap_2()
        .child(div().flex().items_center().gap_2()
            .child(chrome::small_icon(IconName::TriangleAlert, 15., theme::warning()))
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title)))
        .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(body))
}

fn hint(text: String) -> Div { div().text_xs().text_color(theme::muted()).whitespace_normal().child(text) }

fn alert_line(id: &'static str, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text)
}

/// A falha em vermelho com a ação que tenta de novo ao lado.
fn alert_row(id: &'static str, text: String, action: Button) -> Stateful<Div> {
    div().id(id).role(Role::Alert).flex().items_center().gap_3()
        .child(div().flex_1().min_w_0().text_sm().text_color(theme::danger()).whitespace_normal().child(text))
        .child(action)
}

fn notice(id: &'static str, color: Hsla, icon: IconName, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Status).w_full().px(px(12.)).py(px(10.)).flex().items_start().gap(px(10.)).rounded(px(10.)).border_1()
        .border_color(color.alpha(0.3)).bg(color.alpha(0.08)).text_sm()
        .child(chrome::small_icon(icon, 15., color))
        .child(div().flex_1().min_w_0().whitespace_normal().child(text))
}

/// A instalação ou atualização em curso, com as etapas na ordem do `step` do servidor.
fn render_working(state: &WaState) -> Stateful<Div> {
    let job = state.job.as_ref();
    // A passada automática também troca o programa: a mesma vista do Atualizar.
    let update = job.is_some_and(|job| matches!(job.kind.as_str(), "update" | "auto_update"));
    let phases = step_states(&STEPS, job.map_or("", |job| job.step.as_str()));
    settings_box().id("whatsapp-working").role(Role::Status).p_4().gap_3()
        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr(if update { "whatsapp_updating_title" } else { "whatsapp_installing_title" })))
        .children(STEPS.iter().zip(phases).enumerate()
            .map(|(n, ((_, label), phase))| step_item(&format!("whatsapp-step-{n}"), tr(label), phase)))
}

#[cfg(test)]
mod tests {
    use super::{act_timeout, asks_accept, code_of, code_text, failed_job, install_accepted, migration_tags, own_update_step, pair_not_started, parse_state,
        phone_digits, should_poll, step_states, update_failure, update_refused_text, version_row, view_of, Act, CliproxyItem, OwnUpdate, VersionRow, View,
        STEPS};
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
        assert!(matches!(step_states(&STEPS, "service")[..], [CliproxyItem::Done, CliproxyItem::Running, CliproxyItem::Waiting]));
        assert!(matches!(step_states(&STEPS, "whatever")[..], [CliproxyItem::Running, CliproxyItem::Waiting, CliproxyItem::Waiting]));
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

    /// Parado sem aceite gravado (instalado pela migração): a tela pede o aceite e só religa com ele marcado.
    #[test]
    fn stopped_without_a_recorded_acceptance_asks_for_it() {
        let mut raw = base("stopped", None);
        raw["accepted_at"] = serde_json::Value::Null;
        let unaccepted = state(raw);
        assert_eq!(view_of(&unaccepted), View::Stopped);
        assert!(asks_accept(&unaccepted));
        assert!(!install_accepted(&unaccepted, false), "religar fica desabilitado");
        assert!(install_accepted(&unaccepted, true), "marcado agora, religa");
        let accepted = state(base("stopped", None));
        assert!(!asks_accept(&accepted) && install_accepted(&accepted, false), "com aceite gravado nada muda");
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
    fn stopped_on_newer_data_asks_for_the_update() {
        let with = |service: &str, error: serde_json::Value| {
            let mut value = base(service, None);
            value["status_error"] = error;
            view_of(&state(value))
        };
        assert_eq!(with("stopped", json!("whatsapp_schema_newer")), View::SchemaNewer);
        assert_eq!(with("stopped", json!(null)), View::Stopped);
        assert_eq!(with("stopped", json!("whatsapp_outro")), View::Stopped);
        // De pé, o `status_error` é de outra coisa (resposta sem assinatura, sem resposta).
        assert_eq!(with("running", json!("whatsapp_untrusted")), View::Unreachable);
    }

    #[test]
    fn an_update_that_found_nothing_newer_stops_offering_the_update() {
        let after = |kind: &str, outcome: serde_json::Value| {
            let mut value = base("stopped", None);
            value["status_error"] = json!("whatsapp_schema_newer");
            value["job"] = json!({"kind": kind, "step": "download", "done": true, "error": null, "outcome": outcome});
            view_of(&state(value))
        };
        assert_eq!(after("update", json!("already_latest")), View::SchemaNewerLatest);
        assert_eq!(after("update", json!("updated")), View::SchemaNewer);
        assert_eq!(after("install", json!("already_latest")), View::SchemaNewer);
        let mut stopped = base("stopped", None);
        stopped["job"] = json!({"kind": "update", "step": "download", "done": true, "error": null, "outcome": "already_latest"});
        assert_eq!(view_of(&state(stopped)), View::Stopped, "parado por outro motivo continua oferecendo religar");
    }

    fn connected(tag: Option<&str>, update: serde_json::Value) -> serde_json::Value {
        let mut value = base("running", Some(json!({"state": "connected"})));
        value["installed_tag"] = json!(tag);
        value["update_checked_at"] = json!("2026-11-26T10:00:00Z");
        value["update"] = update;
        value
    }

    fn newer(migration: bool, failed: serde_json::Value) -> serde_json::Value {
        json!({"latest": "v0.3.1", "migration": migration, "checked_at": "2026-11-26T10:00:00Z", "failed": failed, "last_error": null})
    }

    /// A linha da versão só existe conectado e com a tag instalada; o estado dela sai do `update`.
    #[test]
    fn the_version_row_follows_the_update_check() {
        let row = |value: serde_json::Value| version_row(&state(value));
        assert_eq!(row(connected(Some("v0.3.0"), json!(null))), Some(VersionRow::Current));
        assert_eq!(row(connected(Some("v0.3.0"), newer(false, json!(null)))), Some(VersionRow::Available));
        assert_eq!(row(connected(Some("v0.1.1"), newer(true, json!(null)))), Some(VersionRow::Migration));
        let failed = json!({"tag": "v0.3.1", "code": "whatsapp_not_started"});
        assert_eq!(row(connected(Some("v0.3.0"), newer(false, failed.clone()))), Some(VersionRow::Failed), "quem tenta de novo é a faixa");
        assert_eq!(row(connected(Some("v0.1.1"), newer(true, failed))), Some(VersionRow::Failed));
        assert_eq!(row(connected(None, newer(false, json!(null)))), None, "sem a tag instalada, nada");
    }

    /// Servidor antigo, sem os campos novos: lê igual e não mostra linha.
    #[test]
    fn an_older_server_without_the_version_fields_still_reads() {
        let s = state(base("running", Some(json!({"state": "connected"}))));
        assert_eq!(version_row(&s), None);
        assert_eq!(update_failure(&s), None);
    }

    /// A falha da troca marcada vira a linha vermelha do topo, com a versão que segue rodando; só uma faixa por vez.
    #[test]
    fn a_marked_update_failure_shows_once_and_only_without_a_job_running() {
        let failed = json!({"tag": "v0.3.1", "code": "whatsapp_update_restored"});
        let s = state(connected(Some("v0.3.0"), newer(false, failed.clone())));
        assert_eq!(update_failure(&s), Some(("v0.3.1".into(), "whatsapp_update_restored".into(), "v0.3.0".into())));
        // `last_error` (falha antes de mexer no serviço) não abre faixa.
        let mut quiet = newer(false, json!(null));
        quiet["last_error"] = json!({"tag": "v0.3.1", "code": "whatsapp_download_failed"});
        assert_eq!(update_failure(&state(connected(Some("v0.3.0"), quiet))), None);
        let mut running = connected(Some("v0.3.0"), newer(false, failed.clone()));
        running["job"] = json!({"kind": "auto_update", "step": "service", "done": false, "error": null, "outcome": null});
        assert_eq!(update_failure(&state(running)), None, "job em curso");
        let mut manual = connected(Some("v0.3.0"), newer(false, failed.clone()));
        manual["job"] = json!({"kind": "update", "step": "start", "done": true, "error": {"code": "whatsapp_not_started"}, "outcome": null});
        let manual = state(manual);
        assert!(failed_job(&manual).is_some());
        assert_eq!(update_failure(&manual), None, "a faixa do job do dono já diz");
        assert_eq!(update_failure(&state(connected(None, newer(false, failed.clone())))), None, "sem a versão que roda");
        // `update_data_migrated` deixa a nova registrada e parada: "continua rodando" seria falso.
        for service in ["stopped", "absent"] {
            let mut stopped = connected(Some("v0.3.0"), newer(true, json!({"tag": "v0.3.1", "code": "whatsapp_update_data_migrated"})));
            stopped["service"] = json!(service);
            assert_eq!(update_failure(&state(stopped)), None, "{service}");
        }
    }

    /// A passada automática nunca abre a faixa "A atualização parou"; enquanto anda, a vista é a de atualizando.
    #[test]
    fn the_automatic_job_never_opens_the_owner_banner() {
        let mut value = connected(Some("v0.3.0"), json!(null));
        value["job"] = json!({"kind": "auto_update", "step": "start", "done": true, "error": {"code": "whatsapp_not_started"}, "outcome": null});
        assert_eq!(failed_job(&state(value.clone())), None);
        value["job"] = json!({"kind": "auto_update", "step": "service", "done": false, "error": null, "outcome": null});
        let running = state(value);
        assert_eq!(view_of(&running), View::Working);
        assert!(should_poll(&running, false));
    }

    /// A confirmação só abre sozinha para o Atualizar desta tela que parou em `needs_confirm`.
    #[test]
    fn needs_confirm_of_the_owner_update_asks_for_the_migration() {
        let with_job = |kind: &str, done: bool, outcome: serde_json::Value| {
            let mut value = connected(Some("v0.1.1"), newer(true, json!(null)));
            value["job"] = json!({"kind": kind, "step": "download", "done": done, "error": null, "outcome": outcome});
            own_update_step(&state(value))
        };
        assert_eq!(with_job("update", false, json!(null)), OwnUpdate::Running);
        assert_eq!(with_job("update", true, json!("needs_confirm")), OwnUpdate::AskMigration);
        assert_eq!(with_job("update", true, json!("updated")), OwnUpdate::Finished);
        assert_eq!(with_job("auto_update", true, json!("needs_confirm")), OwnUpdate::Finished, "job de outra origem");
        assert_eq!(own_update_step(&state(connected(Some("v0.1.1"), json!(null)))), OwnUpdate::Finished);
        assert_eq!(migration_tags(&state(connected(Some("v0.1.1"), newer(true, json!(null))))), Some(("v0.3.1".into(), "v0.1.1".into())));
        assert_eq!(migration_tags(&state(connected(Some("v0.1.1"), json!(null)))), None, "sem versão nova, sem diálogo");
    }

    /// Atualizar recusado pela vez: com job em curso a página o acompanha; sem ele, avisa.
    #[test]
    fn a_busy_refusal_without_a_job_is_said() {
        crate::i18n::english_in_this_thread(false);
        assert_eq!(update_refused_text(&state(connected(Some("v0.3.0"), json!(null)))), Some(crate::i18n::tr("whatsapp_update_busy")));
        let mut running = connected(Some("v0.3.0"), json!(null));
        running["job"] = json!({"kind": "auto_update", "step": "download", "done": false, "error": null, "outcome": null});
        assert_eq!(update_refused_text(&state(running)), None);
    }

    /// Cópia do arquivo que não foi gravada: vista de parado com a frase própria, que só religa.
    #[test]
    fn a_failed_backup_stops_with_its_own_view() {
        let mut value = base("stopped", None);
        value["status_error"] = json!("whatsapp_backup_failed");
        assert_eq!(view_of(&state(value.clone())), View::BackupFailed);
        value["service"] = json!("running");
        assert_eq!(view_of(&state(value)), View::Unreachable, "de pé, o código é de outra coisa");
    }

    #[test]
    fn every_update_failure_has_its_own_phrase() {
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            let generic = crate::i18n::tr("whatsapp_job_err_unknown");
            let generic = generic.split('{').next().unwrap_or_default();
            for code in ["update_restored", "update_data_migrated", "backup_failed", "not_started", "update_no_rollback", "restore_pending"] {
                let text = code_text("whatsapp_job_err_", &format!("whatsapp_{code}"));
                assert!(!text.starts_with(generic), "{code} (english={english}) caiu na frase genérica: {text}");
            }
        }
    }

    /// O servidor dá 30 s ao binário para desvincular (`UNLINK_TIMEOUT` do `hangar-whatsapp`).
    #[test]
    fn unlinking_waits_past_the_server_deadline() {
        for act in [Act::Logout, Act::Wipe, Act::Uninstall] { assert!(act_timeout(act) > 30, "{act:?}"); }
        // Remover soma o logout (até 30 s) a parar, desregistrar e apagar o serviço.
        assert_eq!(act_timeout(Act::Uninstall), 90);
        assert_eq!(act_timeout(Act::Install), 30);
    }

    #[test]
    fn pairing_polls_and_unpaired_only_right_after_asking() {
        let pairing = state(base("running", Some(json!({"state": "pairing"}))));
        let unpaired = state(base("running", Some(json!({"state": "unpaired"}))));
        assert!(should_poll(&pairing, false));
        assert!(!should_poll(&unpaired, false));
        assert!(should_poll(&unpaired, true));
    }

    /// Desvinculado depois do pedido que falhou, ou da espera que passou, devolve o botão de pedir o QR.
    #[test]
    fn the_qr_request_comes_back_when_pairing_did_not_start() {
        assert!(pair_not_started(View::Unpaired, false, false));
        assert!(!pair_not_started(View::Unpaired, true, false), "pedido ainda em voo");
        assert!(!pair_not_started(View::Unpaired, false, true), "logo depois do pedido o estado ainda vem desvinculado");
        assert!(!pair_not_started(View::Pairing, false, false));
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
    fn every_refusal_the_server_sends_has_its_own_phrase() {
        let codes = ["accept_required", "busy", "off", "untrusted", "wipe_partial", "wipe_failed", "service_starting",
            "service_state_unknown", "not_installed", "unreachable", "accept_write_failed", "not_paired", "outdated", "remove_failed",
            "servico_falhou", "bad_reply", "error", "owner_only"];
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            let generic = crate::i18n::tr("whatsapp_err_unknown");
            let generic = generic.split('(').next().unwrap_or_default();
            for code in codes {
                let text = code_text("whatsapp_err_", &format!("whatsapp_{code}"));
                assert!(!text.starts_with(generic), "{code} (english={english}) caiu na frase genérica: {text}");
            }
        }
        assert_eq!(code_text("whatsapp_err_", "whatsapp_novo"), crate::i18n::tr("whatsapp_err_unknown").replace("{code}", "whatsapp_novo"));
    }
}
