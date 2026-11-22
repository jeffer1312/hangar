//! Instalar o app do Hangar no iPhone, iPad e Android, com o print de cada toque. No iOS o servidor desta máquina
//! instala o SideStore pelo cabo (`/api/device-install/*`); o resto a pessoa faz no aparelho, guiada pelos passos.
//! No Android é só o APK, pelo QR.
use super::*;
use super::device::Remote;
use super::settings::{section_head, segments, settings_box};
use crate::api::device_install::{Device, DeviceError, Progress, Scan};

pub(super) const APK_URL: &str = "https://github.com/jeffer1312/hangar/releases/download/mobile-latest/hangar.apk";
/// Pasta dos prints no `AppAssets`; o arquivo é o `image` do passo.
const ASSET_DIR: &str = "install-guide/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GuideStep { pub(super) image: Option<&'static str>, pub(super) title_key: &'static str, pub(super) why_key: &'static str }

pub(super) struct GuideSection { pub(super) title_key: &'static str, pub(super) steps: &'static [GuideStep], source_qr: bool }

macro_rules! step {
    ($id:literal) => { GuideStep { image: None, title_key: concat!("devices_", $id), why_key: concat!("devices_", $id, "_why") } };
    ($id:literal, $image:literal) => { GuideStep { image: Some($image), title_key: concat!("devices_", $id), why_key: concat!("devices_", $id, "_why") } };
}

/// Prints que ainda não existem: o passo aparece só com o texto.
pub(super) const PENDING_CAPTURE: &[&str] = &["ios-00-confiar-computador.png", "ios-08b-reiniciar.png", "ios-08c-ativar.png", "ios-08d-codigo.png"];

/// Antes da instalação pelo cabo: confiar no computador e, sem ele, o serviço da Apple.
pub(super) const IOS_BEFORE: [GuideStep; 2] = [step!("ios_trust", "ios-00-confiar-computador.png"), step!("ios_service")];

/// Depois que o SideStore entrou no aparelho, nesta ordem.
pub(super) const IOS_AFTER: [GuideSection; 5] = [
    GuideSection { title_key: "devices_sec_devmode", source_qr: false, steps: &[
        step!("ios06", "ios-06-privacidade-seguranca.png"), step!("ios07", "ios-07-modo-desenvolvimento.png"),
        step!("ios08", "ios-08-chave-modo-desenvolvimento.png"), step!("ios08b", "ios-08b-reiniciar.png"),
        step!("ios08c", "ios-08c-ativar.png"), step!("ios08d", "ios-08d-codigo.png"),
    ] },
    GuideSection { title_key: "devices_sec_profile", source_qr: false, steps: &[
        step!("ios01", "ios-01-abrir-ajustes.png"), step!("ios02", "ios-02-geral.png"), step!("ios03", "ios-03-gestao-vpn-dispositivo.png"),
        step!("ios04", "ios-04-app-do-desenvolvedor.png"), step!("ios05", "ios-05-perfil-confiavel.png"),
    ] },
    GuideSection { title_key: "devices_sec_vpn", source_qr: false, steps: &[
        step!("ios09", "ios-09-abrir-app-store.png"), step!("ios10", "ios-10-buscar-na-app-store.png"),
        step!("ios11", "ios-11-digitar-localdevvpn.png"), step!("ios12", "ios-12-obter-localdevvpn.png"),
        step!("ios13", "ios-13-confirmar-instalacao.png"), step!("ios14", "ios-14-abrir-localdevvpn.png"),
        step!("ios15", "ios-15-assistente-next.png"), step!("ios16", "ios-16-assistente-next-2.png"),
        step!("ios17", "ios-17-assistente-next-3.png"), step!("ios18", "ios-18-assistente-comecar.png"),
        step!("ios19", "ios-19-conectar.png"), step!("ios20", "ios-20-permitir-vpn.png"),
        step!("ios21", "ios-21-codigo-do-ipad.png"), step!("ios22", "ios-22-vpn-conectada.png"),
    ] },
    GuideSection { title_key: "devices_sec_sidestore", source_qr: false, steps: &[
        step!("ios23", "ios-23-abrir-sidestore.png"), step!("ios24", "ios-24-sidestore-rede-local.png"),
        step!("ios25", "ios-25-sidestore-notificacoes.png"), step!("ss01", "ios-sidestore-01-boas-vindas.png"),
        step!("ss02", "ios-sidestore-02-arquivo-pareamento.png"), step!("ss03", "ios-sidestore-03-arquivos.png"),
        step!("ss04", "ios-sidestore-04-pasta-sidestore.png"), step!("ss07", "ios-sidestore-07-pasta-sidestore.png"),
        step!("ss08", "ios-sidestore-08-localdevvpn.png"), step!("ss09", "ios-sidestore-09-apple-id.png"),
        step!("ss10", "ios-sidestore-10-formulario-apple-id.png"), step!("ss11", "ios-sidestore-11-metodo-verificacao.png"),
        step!("ss12", "ios-sidestore-12-permitir-inicio-sessao.png"), step!("ss13", "ios-sidestore-13-codigo-apple.png"),
        step!("ss14", "ios-sidestore-14-digitar-codigo.png"), step!("ss17", "ios-sidestore-17-tudo-pronto.png"),
        step!("ss20", "ios-sidestore-20-sidestore-pronto.png"),
    ] },
    GuideSection { title_key: "devices_sec_source", source_qr: true, steps: &[
        step!("ss30", "ios-sidestore-30-adicionar-fonte.png"), step!("ss31", "ios-sidestore-31-confirmar-fonte.png"),
        step!("ss32", "ios-sidestore-32-obter-hangar.png"), step!("ss33", "ios-sidestore-33-confirmar-instalar.png"),
        step!("ss34", "ios-sidestore-34-hangar-instalado-pela-fonte.png"), step!("ss24", "ios-sidestore-24-hangar-aberto.png"),
        step!("ss23", "ios-sidestore-23-hangar-instalado.png"),
    ] },
];

/// Só se o SideStore der erro: -80045/-45001 no login pedem o anisette refeito; certificado revogado
/// (pelo Hangar ou outra ferramenta) pede sair sem guardar o certificado e entrar de novo.
pub(super) const IOS_RESERVE: [GuideStep; 2] = [step!("ios_reserve"), step!("ios_signout", "ios-sidestore-39-sair-sem-certificado.png")];

pub(super) const ANDROID: [GuideStep; 20] = [
    step!("a01", "android-01-abrir-navegador.png"), step!("a02", "android-02-baixando.png"), step!("a03", "android-03-abrir-arquivo.png"),
    step!("a04", "android-04-aviso-fonte.png"), step!("a05", "android-05-permitir-fonte.png"), step!("a06", "android-06-instalar.png"),
    step!("a07", "android-07-instalando.png"), step!("a08", "android-08-abrir-app.png"), step!("a09", "android-09-app-aberto.png"),
    step!("a10", "android-10-deslizar-para-cima.png"), step!("a11", "android-11-abrir-configuracoes.png"), step!("a12", "android-12-apps.png"),
    step!("a13", "android-13-hangar.png"), step!("a14", "android-14-notificacoes.png"), step!("a15", "android-15-ligar-notificacoes.png"),
    step!("a16", "android-16-voltar.png"), step!("a17", "android-17-uso-da-bateria.png"), step!("a18", "android-18-segundo-plano.png"),
    step!("a19", "android-19-sem-restricoes.png"), step!("a20", "android-20-pronto.png"),
];

/// O que a pessoa precisa saber antes de entregar o Apple ID.
const WARNINGS: [&str; 4] = ["devices_warn_push", "devices_warn_limits", "devices_warn_anisette", "devices_warn_502"];
/// Títulos de seção, desenhados pelo `heading` (que traduz por dentro).
#[cfg(test)]
const HEADINGS: [&str; 5] = ["devices_android_steps", "devices_before_you_start", "devices_device_heading", "devices_login_heading", "devices_reserve_heading"];

/// Códigos que o servidor devolve nestas rotas; cada um tem frase própria com o que fazer.
#[cfg(test)]
const ERROR_CODES: &[&str] = &[
    "device_install_wrong_code", "device_install_code_timeout", "device_install_app_limit", "device_install_appid_limit",
    "device_install_cert_limit", "device_install_disconnected", "device_install_locked", "device_not_trusted", "device_service_missing",
    "device_install_login_refused", "device_install_apple_busy", "device_install_anisette_unreachable", "device_install_download_failed",
    "device_install_download_mismatch", "device_install_sign_failed", "device_install_not_installed", "device_error",
    "device_install_forbidden", "device_install_busy", "device_install_unknown_job", "device_install_not_waiting", "connection_failed",
    "server_outdated", "unauthorized", "device_install_bad_request", "device_install_cancelled",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage { Login, AwaitingCode, CheckingLimit, Downloading, Signing, AwaitingCertChoice, Installing, Pairing, Verifying, Done, Failed }

impl Stage {
    /// As etapas que a tela lista, na ordem do servidor. A escolha de certificado só acontece com a
    /// conta lotada: fica fora da lista e conta como a assinatura.
    pub(super) const SHOWN: [Stage; 8] = [Stage::Login, Stage::AwaitingCode, Stage::CheckingLimit, Stage::Downloading, Stage::Signing,
        Stage::Installing, Stage::Pairing, Stage::Verifying];

    pub(super) fn parse(stage: &str) -> Option<Stage> {
        Some(match stage {
            "login" => Stage::Login, "awaiting_code" => Stage::AwaitingCode, "checking_limit" => Stage::CheckingLimit,
            "downloading" => Stage::Downloading, "signing" => Stage::Signing, "awaiting_cert_choice" => Stage::AwaitingCertChoice,
            "installing" => Stage::Installing, "pairing" => Stage::Pairing,
            "verifying" => Stage::Verifying, "done" => Stage::Done, "failed" => Stage::Failed,
            _ => return None,
        })
    }

    pub(super) fn label_key(self) -> &'static str {
        match self {
            Stage::Login => "devices_stage_login", Stage::AwaitingCode => "devices_stage_awaiting_code", Stage::CheckingLimit => "devices_stage_checking_limit",
            Stage::Downloading => "devices_stage_downloading", Stage::Signing => "devices_stage_signing",
            Stage::AwaitingCertChoice => "devices_stage_awaiting_cert_choice", Stage::Installing => "devices_stage_installing",
            Stage::Pairing => "devices_stage_pairing", Stage::Verifying => "devices_stage_verifying", Stage::Done => "devices_stage_done",
            Stage::Failed => "devices_stage_failed",
        }
    }

    fn running(self) -> bool { !matches!(self, Stage::Done | Stage::Failed) }

    /// O servidor só cancela antes de instalar: depois, o SideStore ficaria sem pareamento.
    pub(super) fn cancellable(self) -> bool { self.running() && !matches!(self, Stage::Installing | Stage::Pairing | Stage::Verifying) }
}

/// Em que ponto do iOS a tela está.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase { Searching, Service, Plug, Trust, Login, Running, Done, Failed }

/// `stage`: a etapa do trabalho em curso, se houver; ela vence o que a lista de aparelhos diz.
pub(super) fn ios_phase(scan: Option<&Scan>, selected: Option<&Device>, stage: Option<Stage>) -> Phase {
    match stage {
        Some(Stage::Done) => return Phase::Done,
        Some(Stage::Failed) => return Phase::Failed,
        Some(_) => return Phase::Running,
        None => {}
    }
    let Some(scan) = scan else { return Phase::Searching };
    if scan.service != "ok" { return Phase::Service; }
    if scan.devices.is_empty() { return Phase::Plug; }
    if selected.is_some_and(|d| d.trusted) { Phase::Login } else { Phase::Trust }
}

/// Os passos de antes da instalação que valem nesta fase.
pub(super) fn before_steps(phase: Phase) -> &'static [GuideStep] {
    match phase {
        Phase::Service => &IOS_BEFORE[1..],
        Phase::Plug | Phase::Trust => &IOS_BEFORE[..1],
        _ => &[],
    }
}

/// Os passos no aparelho só depois que o SideStore entrou nele: o Modo de Desenvolvimento antes não tem o que abrir.
pub(super) fn after_sections(phase: Phase) -> &'static [GuideSection] {
    if phase == Phase::Done { &IOS_AFTER } else { &[] }
}

/// Código sem frase própria (ou erro que o servidor nem mandou).
fn unknown_error(code: &str) -> String { tr("devices_err_unknown").replace("{code}", code) }

/// A frase do código com o que fazer; o detalhe do servidor (certificados, motivo da Apple) vai junto.
pub(super) fn error_message(error: &DeviceError) -> String {
    let key = format!("devices_err_{}", error.code);
    let text = tr(&key);
    let text = if text == key { unknown_error(&error.code) } else { text };
    match error.detail.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(detail) => format!("{text} ({detail})"),
        None => text,
    }
}

fn qr_svg(text: &str) -> Option<Arc<Image>> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build();
    Some(Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Platform { #[default] Ios, Android }

/// Consultas seguidas (uma por segundo) que podem cair antes de dar o trabalho por perdido.
const PROGRESS_TOLERATED_FAILURES: u32 = 5;

struct Job { id: String, progress: Option<Progress>, lost: Option<DeviceError>, failures: u32 }

impl Job {
    fn new(id: String) -> Self { Job { id, progress: None, lost: None, failures: 0 } }
    /// Só queda de rede (ou resposta ilegível) se repete; recusa do servidor não muda tentando de novo.
    fn record(&mut self, result: Result<Progress, DeviceError>) {
        match result {
            Ok(progress) => (self.progress, self.failures) = (Some(progress), 0),
            Err(error) if error.code == "connection_failed" && self.failures + 1 < PROGRESS_TOLERATED_FAILURES => self.failures += 1,
            Err(error) => self.lost = Some(error),
        }
    }
    fn stage(&self) -> Stage {
        if self.lost.is_some() { return Stage::Failed; }
        self.progress.as_ref().and_then(|p| Stage::parse(&p.stage)).unwrap_or(Stage::Login)
    }
    fn error(&self) -> Option<&DeviceError> { self.lost.as_ref().or_else(|| self.progress.as_ref()?.error.as_ref()) }
}

#[derive(Default)]
pub(in crate::app) struct Devices {
    platform: Platform,
    scan: Remote<Scan>,
    selected: Option<String>,
    trusting: Option<String>,
    trust_error: Option<String>,
    apple_id: Option<Entity<InputState>>,
    password: Option<Entity<InputState>>,
    code: Option<Entity<InputState>>,
    starting: bool,
    form_error: Option<String>,
    job: Option<Job>,
    code_sending: bool,
    code_error: Option<String>,
    certs_sending: bool,
    certs_error: Option<String>,
    cancelling: bool,
    /// O `/start` disse que o aparelho já tem instalação rodando (outra tela ou tela reaberta).
    busy: bool,
    qr: Vec<(String, Arc<Image>)>,
    /// O print aberto em tamanho maior; um clique no print abre e fecha.
    enlarged: Option<&'static str>,
}

pub(super) enum DevicesReply {
    Scanned(u64, Result<Scan, DeviceError>),
    Trusted(String, Result<(), DeviceError>),
    Started(Result<String, DeviceError>),
    Progress(String, Result<Progress, DeviceError>),
    Code(Result<(), DeviceError>),
    Certs(Result<(), DeviceError>),
    Cancelled(Result<(), DeviceError>),
}

impl Hangar {
    fn devices_send_later(&self) -> impl Fn(DevicesReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Devices(reply) }).await; })
        }
    }

    fn devices_api(&self) -> Option<Api> { self.api.clone().filter(Api::is_loopback) }

    pub(super) fn devices_opened(&mut self, cx: &mut Context<Self>) {
        if self.devices.job.is_none() && !self.devices.scan.loading { self.scan_devices(cx); }
    }

    // A senha e o código não ficam montados fora da página.
    pub(super) fn devices_page_left(&mut self) { (self.devices.password, self.devices.code) = (None, None); }

    fn scan_devices(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        let seq = self.devices.scan.start();
        let done = self.devices_send_later();
        self.runtime.spawn(async move { done(DevicesReply::Scanned(seq, api.device_scan().await)).await });
        cx.notify();
    }

    fn trust_device(&mut self, udid: String, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        if self.devices.trusting.is_some() { return; }
        (self.devices.trusting, self.devices.trust_error) = (Some(udid.clone()), None);
        let done = self.devices_send_later();
        self.runtime.spawn(async move { let result = api.device_trust(&udid).await; done(DevicesReply::Trusted(udid, result)).await });
        cx.notify();
    }

    fn start_device_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        let d = &mut self.devices;
        if d.starting || d.job.is_some() { return; }
        let Some(udid) = d.selected.clone() else { return };
        let (Some(apple_id), Some(password)) = (d.apple_id.clone(), d.password.clone()) else { return };
        let email = apple_id.read(cx).value().trim().to_owned();
        let secret = password.read(cx).value().to_string();
        if email.is_empty() || secret.is_empty() { d.form_error = Some(tr("devices_fields_required")); cx.notify(); return; }
        // A senha sai do campo antes do pedido: só o pedido a carrega, e ela some com ele.
        password.update(cx, |input, cx| input.set_value("", window, cx));
        (d.starting, d.form_error) = (true, None);
        let done = self.devices_send_later();
        self.runtime.spawn(async move { done(DevicesReply::Started(api.device_start(&udid, &email, secret).await)).await });
        cx.notify();
    }

    fn poll_progress(&mut self) {
        let Some(api) = self.devices_api() else { return };
        let Some(job) = self.devices.job.as_ref().filter(|j| j.stage().running()) else { return };
        let id = job.id.clone();
        let done = self.devices_send_later();
        self.runtime.spawn(async move { let result = api.device_progress(&id).await; done(DevicesReply::Progress(id, result)).await });
    }

    fn send_device_code(&mut self, resend: bool, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        let Some(id) = self.devices.job.as_ref().map(|j| j.id.clone()) else { return };
        let code = self.devices.code.as_ref().map(|c| c.read(cx).value().trim().to_owned()).unwrap_or_default();
        if !resend && code.is_empty() { return; }
        if self.devices.code_sending { return; }
        (self.devices.code_sending, self.devices.code_error) = (true, None);
        let done = self.devices_send_later();
        self.runtime.spawn(async move {
            let result = api.device_code(&id, (!resend).then_some(code.as_str())).await;
            done(DevicesReply::Code(result)).await
        });
        cx.notify();
    }

    /// `revoke`: revoga todos os certificados mostrados e segue; senão desiste do trabalho.
    fn send_device_certs(&mut self, revoke: bool, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        let Some(job) = self.devices.job.as_ref() else { return };
        if self.devices.certs_sending { return; }
        let id = job.id.clone();
        let serials = job.progress.as_ref().and_then(|p| p.certs.as_ref()).map(|c| c.iter().map(|c| c.serial.clone()).collect::<Vec<_>>());
        let choice = if revoke { match serials { Some(s) if !s.is_empty() => Some(s), _ => return } } else { None };
        (self.devices.certs_sending, self.devices.certs_error) = (true, None);
        let done = self.devices_send_later();
        self.runtime.spawn(async move { done(DevicesReply::Certs(api.device_certs(&id, choice).await)).await });
        cx.notify();
    }

    fn confirm_revoke_certs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("devices_certs_revoke"), tr("devices_certs_warning"),
            tr("devices_certs_revoke"), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.send_device_certs(true, cx));
                true
            });
    }

    /// Pelo aparelho, não pelo `job`: serve também ao trabalho que esta tela não começou.
    fn cancel_device_install(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.devices_api() else { return };
        let Some(udid) = self.devices.selected.clone() else { return };
        if self.devices.cancelling { return; }
        (self.devices.cancelling, self.devices.form_error) = (true, None);
        let done = self.devices_send_later();
        self.runtime.spawn(async move { done(DevicesReply::Cancelled(api.device_cancel(&udid).await)).await });
        cx.notify();
    }

    fn restart_device_install(&mut self, cx: &mut Context<Self>) {
        let d = &mut self.devices;
        (d.job, d.code_error, d.form_error, d.certs_error) = (None, None, None, None);
        self.scan_devices(cx);
    }

    pub(super) fn receive_devices(&mut self, reply: DevicesReply, window: &mut Window, cx: &mut Context<Self>) {
        let d = &mut self.devices;
        match reply {
            DevicesReply::Scanned(seq, result) => {
                if !d.scan.finish(seq, result.map_err(|e| error_message(&e))) { return; }
                let devices = d.scan.ok().map(|s| s.devices.as_slice()).unwrap_or_default();
                // O escolhido some do cabo: passa ao primeiro confiado (ou ao primeiro).
                if !devices.iter().any(|dev| Some(&dev.udid) == d.selected.as_ref()) {
                    d.selected = devices.iter().find(|dev| dev.trusted).or(devices.first()).map(|dev| dev.udid.clone());
                }
            }
            DevicesReply::Trusted(udid, result) => {
                if d.trusting.as_deref() != Some(udid.as_str()) { return; }
                d.trusting = None;
                match result {
                    Ok(()) => d.selected = Some(udid),
                    Err(error) => d.trust_error = Some(error_message(&error)),
                }
                self.scan_devices(cx);
            }
            DevicesReply::Started(result) => {
                d.starting = false;
                match result {
                    Ok(id) => { (d.job, d.busy) = (Some(Job::new(id)), false); self.poll_progress(); }
                    Err(error) => { d.busy = error.code == "device_install_busy"; d.form_error = Some(error_message(&error)); }
                }
            }
            DevicesReply::Cancelled(result) => {
                d.cancelling = false;
                match result {
                    // O trabalho desta tela chega a `failed` pela consulta; o de fora só libera o botão.
                    Ok(()) => { d.busy = false; d.form_error = None; }
                    // Terminou sozinho antes do pedido: nada mais segura o aparelho.
                    Err(error) if error.code == "device_install_not_waiting" => { d.busy = false; d.form_error = None; }
                    Err(error) => d.form_error = Some(error_message(&error)),
                }
            }
            DevicesReply::Progress(id, result) => {
                let Some(job) = d.job.as_mut().filter(|j| j.id == id) else { return };
                job.record(result);
                if job.stage().running() {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let _ = this.update(cx, |this, _| if this.devices.job.as_ref().is_some_and(|j| j.id == id) { this.poll_progress() });
                    }).detach();
                }
            }
            DevicesReply::Code(result) => {
                d.code_sending = false;
                match result {
                    Ok(()) => if let Some(code) = &d.code { code.update(cx, |input, cx| input.set_value("", window, cx)); },
                    Err(error) => d.code_error = Some(error_message(&error)),
                }
            }
            DevicesReply::Certs(result) => {
                d.certs_sending = false;
                if let Err(error) = result { d.certs_error = Some(error_message(&error)); }
            }
        }
        cx.notify();
    }

    fn qr(&mut self, text: &str) -> Option<Arc<Image>> {
        if let Some((_, image)) = self.devices.qr.iter().find(|(t, _)| t == text) { return Some(image.clone()); }
        let image = qr_svg(text)?;
        self.devices.qr.push((text.to_owned(), image.clone()));
        Some(image)
    }

    pub(super) fn render_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = div().flex().flex_col().child(self.page_top("settings_page_devices", tr("devices_lead")));
        if self.devices_api().is_none() {
            return page.child(settings_box().mt(px(20.)).child(note(tr("devices_local_only"), theme::muted()))).into_any_element();
        }
        let d = &mut self.devices;
        if d.apple_id.is_none() { d.apple_id = Some(cx.new(|cx| InputState::new(window, cx).placeholder("nome@icloud.com"))); }
        if d.password.is_none() { d.password = Some(cx.new(|cx| InputState::new(window, cx).masked(true))); }
        if d.code.is_none() { d.code = Some(cx.new(|cx| InputState::new(window, cx).placeholder("123456"))); }
        let platform = d.platform;
        let picker = segments("devices-platform", &[tr("devices_ios"), tr("devices_android")], platform as usize, 2, false, String::new(),
            |this: &mut Hangar, n, _, cx| { this.devices.platform = if n == 0 { Platform::Ios } else { Platform::Android }; cx.notify(); }, cx);
        let page = page.child(div().mt(px(20.)).flex().child(picker));
        match platform {
            Platform::Android => page.child(self.render_android(cx)).into_any_element(),
            Platform::Ios => page.child(self.render_ios(cx)).into_any_element(),
        }
    }

    fn render_android(&mut self, cx: &mut Context<Self>) -> Div {
        let qr = self.qr(APK_URL);
        div().flex().flex_col()
            .child(settings_box().mt(px(20.)).child(qr_block(qr, tr("devices_apk_qr"), APK_URL)))
            .child(self.heading("devices_android_steps"))
            .child(steps_box(&ANDROID, 1, self.devices.enlarged, cx))
    }

    fn render_ios(&mut self, cx: &mut Context<Self>) -> Div {
        let d = &self.devices;
        let stage = d.job.as_ref().map(Job::stage);
        let selected = d.scan.ok().and_then(|s| s.devices.iter().find(|dev| Some(&dev.udid) == d.selected.as_ref()));
        let phase = ios_phase(d.scan.ok(), selected, stage);
        let warnings = settings_box().children(WARNINGS
            .map(|key| div().px_4().py(px(10.)).flex().items_start().gap(px(10.))
                .child(div().pt(px(2.)).child(chrome::small_icon(IconName::Info, 14., theme::muted())))
                .child(div().flex_1().min_w_0().text_size(px(13.)).whitespace_normal().child(tr(key)))));
        let mut page = div().flex().flex_col().child(self.heading("devices_before_you_start")).child(warnings)
            .child(self.heading("devices_device_heading"));
        page = match phase {
            Phase::Running | Phase::Done | Phase::Failed => page.child(self.render_job(phase, cx)),
            _ => page.child(self.render_device_box(phase, cx)),
        };
        let before = before_steps(phase);
        let enlarged = self.devices.enlarged;
        if !before.is_empty() { page = page.child(div().mt(px(12.)).child(steps_box(before, 1, enlarged, cx))); }
        if phase == Phase::Login { page = page.child(self.heading("devices_login_heading")).child(self.render_login(cx)); }
        let sections = after_sections(phase);
        if !sections.is_empty() {
            let source = self.devices.job.as_ref().and_then(|j| j.progress.as_ref()).map(|p| format!("sidestore://source?url={}", p.source_url));
            let source_qr = source.as_deref().and_then(|s| self.qr(s));
            let mut number = 1;
            for section in sections {
                page = page.child(self.heading(section.title_key));
                if section.source_qr && let Some(link) = &source {
                    page = page.child(settings_box().mb(px(12.)).child(qr_block(source_qr.clone(), tr("devices_source_qr"), link)));
                }
                page = page.child(steps_box(section.steps, number, enlarged, cx));
                number += section.steps.len();
            }
            page = page.child(self.heading("devices_reserve_heading")).child(steps_box(&IOS_RESERVE, number, enlarged, cx));
        }
        page
    }

    fn render_device_box(&mut self, phase: Phase, cx: &mut Context<Self>) -> Div {
        let d = &self.devices;
        let search = Button::new("devices-search").outline().small().icon(IconName::RefreshCw).label(tr("devices_search_again"))
            .loading(d.scan.loading).disabled(d.scan.loading || d.trusting.is_some())
            .on_click(cx.listener(|this, _, _, cx| { this.devices.trust_error = None; this.scan_devices(cx); }));
        let head = section_head(IconName::Smartphone, tr("devices_device_heading"), None, Some(search.into_any_element()), px(16.));
        let mut body = settings_box().child(head);
        match (phase, &d.scan.value) {
            (_, Some(Err(error))) => body = body.child(note(error.clone(), theme::danger())),
            (Phase::Searching, _) => body = body.child(div().px_4().pb(px(16.)).flex().items_center().gap(px(10.)).text_size(px(13.)).text_color(theme::muted())
                .child(chrome::Spinner::new("devices-searching", IconName::LoaderCircle, px(15.), theme::accent())).child(tr("devices_searching"))),
            (Phase::Service, _) => body = body.child(note(tr("devices_service_missing"), theme::warning_text())),
            (Phase::Plug, _) => body = body.child(note(tr("devices_none"), theme::muted())),
            _ => {
                let devices = d.scan.ok().map(|s| s.devices.clone()).unwrap_or_default();
                for device in devices {
                    let on = d.selected.as_deref() == Some(device.udid.as_str());
                    let waiting = d.trusting.as_deref() == Some(device.udid.as_str());
                    let udid = device.udid.clone();
                    let action = if device.trusted {
                        Button::new(SharedString::from(format!("devices-use-{}", device.udid))).outline().small().label(tr("devices_use"))
                            .selected(on).toggled(on).on_click(cx.listener(move |this, _, _, cx| { this.devices.selected = Some(udid.clone()); cx.notify(); }))
                    } else {
                        Button::new(SharedString::from(format!("devices-trust-{}", device.udid))).primary().small().label(tr("devices_trust"))
                            .loading(waiting).disabled(d.trusting.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.trust_device(udid.clone(), cx)))
                    };
                    let status = match &device.error {
                        Some(error) => error_message(error),
                        None if device.trusted => tr("devices_trusted"),
                        None => tr("devices_not_trusted"),
                    };
                    body = body.child(div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().items_center().gap(px(12.))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                            .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).truncate().child(device.name.clone()))
                            .child(div().text_size(px(12.)).text_color(theme::muted())
                                .child(format!("iOS {} · {status}", device.ios_version))))
                        .child(action));
                    if waiting { body = body.child(note(tr("devices_trust_waiting"), theme::muted())); }
                }
            }
        }
        if let Some(error) = &d.trust_error { body = body.child(note(error.clone(), theme::danger())); }
        body
    }

    fn render_login(&mut self, cx: &mut Context<Self>) -> Div {
        let d = &self.devices;
        let (Some(apple_id), Some(password)) = (&d.apple_id, &d.password) else { return div() };
        let field = |label: String, input: &Entity<InputState>| div().flex().flex_col().gap(px(6.))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label.clone()))
            .child(Input::new(input).disabled(d.starting).aria_label(label));
        settings_box().p_4().gap(px(14.))
            .child(field(tr("devices_apple_id"), apple_id))
            .child(field(tr("devices_password"), password))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr("devices_password_note")))
            .when_some(d.form_error.clone(), |el, error| el.child(div().id("devices-form-error").role(Role::Alert).text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().gap(px(8.)).child(Button::new("devices-install").primary().small().label(tr("devices_install"))
                .loading(d.starting).disabled(d.starting || d.cancelling)
                .on_click(cx.listener(|this, _, window, cx| this.start_device_install(window, cx))))
                .when(d.busy, |el| el.child(cancel_button(d.cancelling, cx))))
    }

    fn render_job(&mut self, phase: Phase, cx: &mut Context<Self>) -> Div {
        let d = &self.devices;
        let Some(job) = &d.job else { return div() };
        let stage = job.stage();
        let listed = if stage == Stage::AwaitingCertChoice { Stage::Signing } else { stage };
        let reached = Stage::SHOWN.iter().position(|s| *s == listed).unwrap_or(Stage::SHOWN.len());
        let mut body = settings_box().child(section_head(IconName::Download, tr("devices_progress_heading"), None, None, px(16.)));
        if phase != Phase::Failed {
            body = body.child(div().px_4().pb(px(14.)).flex().flex_col().gap(px(8.)).children(Stage::SHOWN.iter().enumerate().map(|(n, s)| {
                let (icon, color) = if phase == Phase::Done || n < reached { (Some(IconName::CircleCheck), theme::success()) }
                    else if n == reached { (None, theme::accent()) } else { (Some(IconName::Circle), theme::faint()) };
                let mark = match icon {
                    Some(icon) => chrome::small_icon(icon, 15., color).into_any_element(),
                    None => chrome::Spinner::new(SharedString::from(format!("devices-stage-{n}")), IconName::LoaderCircle, px(15.), color).into_any_element(),
                };
                div().flex().items_center().gap(px(10.)).text_size(px(13.))
                    .text_color(if n <= reached || phase == Phase::Done { theme::text() } else { theme::muted() })
                    .child(mark).child(tr(s.label_key()))
            })));
        }
        if stage == Stage::AwaitingCode && let Some(code) = &d.code {
            let wrong = job.error().map(error_message);
            body = body.child(div().border_t_1().border_color(theme::border()).p_4().flex().flex_col().gap(px(10.))
                .child(div().text_size(px(13.)).whitespace_normal().child(tr("devices_code_prompt")))
                .when_some(wrong.or(d.code_error.clone()), |el, error| el.child(div().id("devices-code-error").role(Role::Alert).text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error)))
                .child(div().flex().items_end().gap(px(8.))
                    .child(div().w(px(160.)).child(Input::new(code).disabled(d.code_sending).aria_label(tr("devices_code"))))
                    .child(Button::new("devices-code-send").primary().small().label(tr("devices_code_send")).loading(d.code_sending).disabled(d.code_sending)
                        .on_click(cx.listener(|this, _, _, cx| this.send_device_code(false, cx))))
                    .child(Button::new("devices-code-resend").ghost().small().label(tr("devices_code_resend")).disabled(d.code_sending)
                        .on_click(cx.listener(|this, _, _, cx| this.send_device_code(true, cx))))));
        }
        if stage == Stage::AwaitingCertChoice {
            let certs = job.progress.as_ref().and_then(|p| p.certs.clone()).unwrap_or_default();
            body = body.child(div().border_t_1().border_color(theme::border()).p_4().flex().flex_col().gap(px(10.))
                .child(div().text_size(px(13.)).whitespace_normal().child(tr("devices_certs_prompt")))
                .children(certs.into_iter().map(|c| div().flex().flex_col().gap(px(2.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(format!("{} · {}", c.name, c.machine_name)))
                    .child(div().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).child(c.serial))))
                .child(div().text_size(px(12.5)).text_color(theme::warning_text()).whitespace_normal().child(tr("devices_certs_warning")))
                .when_some(d.certs_error.clone(), |el, error| el.child(div().id("devices-certs-error").role(Role::Alert).text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error)))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("devices-certs-revoke").danger().small().label(tr("devices_certs_revoke")).loading(d.certs_sending).disabled(d.certs_sending)
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_revoke_certs(window, cx))))
                    .child(Button::new("devices-certs-cancel").ghost().small().label(tr("devices_certs_cancel")).disabled(d.certs_sending)
                        .on_click(cx.listener(|this, _, _, cx| this.send_device_certs(false, cx))))));
        }
        if stage.cancellable() {
            body = body.child(div().px_4().pb(px(16.)).flex().child(cancel_button(d.cancelling, cx)))
                .when_some(d.form_error.clone(), |el, error| el.child(note(error, theme::danger())));
        }
        match phase {
            Phase::Done => body.child(note(tr("devices_done"), theme::success_text())),
            Phase::Failed => {
                let error = job.error().map(error_message).unwrap_or_else(|| unknown_error("failed"));
                body.child(div().id("devices-job-error").role(Role::Alert).child(note(error, theme::danger())))
                    .child(div().px_4().pb(px(16.)).flex().child(Button::new("devices-retry").outline().small().icon(IconName::RotateCcw)
                        .label(tr("devices_retry")).on_click(cx.listener(|this, _, _, cx| this.restart_device_install(cx)))))
            }
            _ => body,
        }
    }
}

fn cancel_button(cancelling: bool, cx: &mut Context<Hangar>) -> Button {
    Button::new("devices-cancel").outline().small().icon(IconName::Close).label(tr("devices_cancel"))
        .loading(cancelling).disabled(cancelling)
        .on_click(cx.listener(|this, _, _, cx| this.cancel_device_install(cx)))
}

fn note(text: String, color: Hsla) -> Div {
    div().px_4().pb(px(16.)).text_size(px(13.)).text_color(color).whitespace_normal().child(text)
}

fn qr_block(qr: Option<Arc<Image>>, label: String, link: &str) -> Div {
    div().p_4().flex().items_center().gap(px(16.))
        .child(div().flex_shrink_0().size(px(168.)).p(px(8.)).rounded(px(10.)).bg(gpui_kit::white())
            .children(qr.map(|qr| img(qr).size_full())))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.))
            .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).whitespace_normal().child(label))
            .child(div().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(link.to_owned())))
}

/// Uma linha por passo: número, o que tocar, o porquê e o print à direita (quando já existe), pequeno até o clique.
fn steps_box(steps: &[GuideStep], first: usize, enlarged: Option<&'static str>, cx: &mut Context<Hangar>) -> Div {
    settings_box().children(steps.iter().enumerate().map(|(n, step)| {
        let image = step.image.filter(|image| !PENDING_CAPTURE.contains(image));
        let shot = image.map(|image| {
            let (w, h) = if enlarged == Some(image) { (440., 640.) } else { (150., 216.) };
            div().id(SharedString::from(format!("devices-shot-{image}"))).flex_shrink_0().w(px(w)).h(px(h)).cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.devices.enlarged = if this.devices.enlarged == Some(image) { None } else { Some(image) };
                    cx.notify();
                }))
                .child(img(format!("{ASSET_DIR}{image}")).size_full().object_fit(ObjectFit::Contain))
        });
        div().when(n > 0, |el| el.border_t_1().border_color(theme::border())).px_4().py(px(14.)).flex().items_start().gap(px(14.))
            .child(div().size(px(24.)).flex_shrink_0().rounded_full().bg(theme::accent_dim()).flex().items_center().justify_center()
                .text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::accent_text()).child((first + n).to_string()))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).whitespace_normal().child(tr(step.title_key)))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(tr(step.why_key))))
            .children(shot)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::settings::Page;
    use core::prelude::v1::test;

    fn all_steps() -> Vec<GuideStep> {
        IOS_BEFORE.iter().chain(IOS_AFTER.iter().flat_map(|s| s.steps)).chain(IOS_RESERVE.iter()).chain(ANDROID.iter()).copied().collect()
    }

    fn scan(service: &str, devices: &[(&str, bool)]) -> Scan {
        Scan { service: service.into(), devices: devices.iter().map(|(udid, trusted)| Device { udid: (*udid).into(), name: "iPad".into(),
            ios_version: "26.5".into(), trusted: *trusted, error: None }).collect() }
    }

    #[test]
    fn devices_progress_retries_only_network_drops_and_only_for_a_while() {
        let err = |code: &str| Err(DeviceError { code: code.into(), detail: None });
        for code in ["device_install_unknown_job", "device_install_forbidden", "server_outdated", "unauthorized"] {
            let mut job = Job::new("j".into());
            job.record(err(code));
            assert_eq!(job.stage(), Stage::Failed, "{code}");
        }
        let mut job = Job::new("j".into());
        for _ in 0..PROGRESS_TOLERATED_FAILURES - 1 { job.record(err("connection_failed")); }
        assert!(job.lost.is_none());
        job.record(Ok(Progress { stage: "downloading".into(), error: None, source_url: String::new(), certs: None }));
        assert_eq!(job.failures, 0);
        for _ in 0..PROGRESS_TOLERATED_FAILURES { job.record(err("connection_failed")); }
        assert_eq!(job.lost.as_ref().map(|e| e.code.as_str()), Some("connection_failed"));
    }

    #[test]
    fn devices_page_only_shows_on_the_local_server() {
        let local = Api::new("http://127.0.0.1:8765", "t").unwrap();
        let remote = Api::new("http://192.168.0.5:8765", "t").unwrap();
        assert!(Page::shown(Some(&local)).any(|p| p == Page::Devices));
        assert!(!Page::shown(Some(&remote)).any(|p| p == Page::Devices));
        assert!(!Page::shown(None).any(|p| p == Page::Devices));
        // As outras páginas não dependem da conexão.
        assert_eq!(Page::shown(Some(&remote)).count() + 1, Page::sections().count());
    }

    #[test]
    fn devices_missing_service_shows_its_step_before_the_login() {
        let missing = scan("missing", &[]);
        let phase = ios_phase(Some(&missing), None, None);
        assert_eq!(phase, Phase::Service);
        assert_eq!(before_steps(phase), &IOS_BEFORE[1..]);
        assert!(IOS_BEFORE[1].image.is_none());
        let empty = scan("ok", &[]);
        assert_eq!(ios_phase(Some(&empty), None, None), Phase::Plug);
        let listed = scan("ok", &[("a", false), ("b", true)]);
        assert_eq!(ios_phase(Some(&listed), listed.devices.first(), None), Phase::Trust);
        assert_eq!(before_steps(Phase::Trust), &IOS_BEFORE[..1]);
        assert_eq!(ios_phase(Some(&listed), listed.devices.get(1), None), Phase::Login);
        assert!(before_steps(Phase::Login).is_empty());
    }

    #[test]
    fn devices_every_stage_maps_to_a_step() {
        let all = ["login", "awaiting_code", "checking_limit", "downloading", "signing", "installing", "pairing", "verifying", "awaiting_cert_choice",
            "done", "failed"];
        let parsed: Vec<Stage> = all.iter().map(|s| Stage::parse(s).unwrap_or_else(|| panic!("{s} sem etapa"))).collect();
        let keys: HashSet<&str> = parsed.iter().map(|s| s.label_key()).collect();
        assert_eq!(keys.len(), all.len(), "duas etapas com o mesmo rótulo");
        assert_eq!(Stage::parse("whatever"), None);
        let listed = scan("ok", &[("a", true)]);
        for stage in &parsed[..9] { assert_eq!(ios_phase(Some(&listed), listed.devices.first(), Some(*stage)), Phase::Running, "{stage:?}"); }
        assert_eq!(ios_phase(Some(&listed), listed.devices.first(), Some(Stage::Done)), Phase::Done);
        assert_eq!(ios_phase(Some(&listed), listed.devices.first(), Some(Stage::Failed)), Phase::Failed);
        // O trabalho em curso vence o aparelho que saiu do cabo.
        assert_eq!(ios_phase(Some(&scan("ok", &[])), None, Some(Stage::Signing)), Phase::Running);
    }

    #[test]
    fn devices_cancel_only_before_the_install() {
        let cancellable: Vec<Stage> = ["login", "awaiting_code", "checking_limit", "downloading", "signing", "awaiting_cert_choice",
            "installing", "pairing", "verifying", "done", "failed"].iter().filter_map(|s| Stage::parse(s)).filter(|s| s.cancellable()).collect();
        assert_eq!(cancellable, [Stage::Login, Stage::AwaitingCode, Stage::CheckingLimit, Stage::Downloading, Stage::Signing, Stage::AwaitingCertChoice]);
    }

    #[test]
    fn devices_cert_choice_reads_the_listed_certificates() {
        let p: Progress = serde_json::from_value(serde_json::json!({"stage": "awaiting_cert_choice", "error": null, "source_url": "u",
            "certs": [{"name": "iPhone Developer", "machine_name": "SideStore", "serial": "AAA111"}]})).unwrap();
        assert_eq!(Stage::parse(&p.stage), Some(Stage::AwaitingCertChoice));
        assert_eq!(p.certs.unwrap()[0].serial, "AAA111");
        let old: Progress = serde_json::from_value(serde_json::json!({"stage": "signing", "error": null, "source_url": "u"})).unwrap();
        assert_eq!(old.certs, None);
    }

    #[test]
    fn devices_developer_mode_comes_after_done() {
        for phase in [Phase::Searching, Phase::Service, Phase::Plug, Phase::Trust, Phase::Login, Phase::Running, Phase::Failed] {
            assert!(after_sections(phase).is_empty(), "{phase:?} mostrou os passos de depois");
            assert!(!before_steps(phase).iter().any(|s| s.title_key.starts_with("devices_ios0")), "{phase:?}");
        }
        let after = after_sections(Phase::Done);
        assert_eq!(after.first().map(|s| s.title_key), Some("devices_sec_devmode"));
        assert_eq!(after[0].steps[0].image, Some("ios-06-privacidade-seguranca.png"));
    }

    #[test]
    fn devices_every_key_has_both_translations() {
        let dicts = [include_str!("../../../messages/pt.json"), include_str!("../../../messages/en.json")]
            .map(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).unwrap());
        let mut keys: Vec<String> = all_steps().iter().flat_map(|s| [s.title_key, s.why_key]).map(str::to_owned).collect();
        keys.extend(IOS_AFTER.iter().map(|s| s.title_key.to_owned()));
        keys.extend(Stage::SHOWN.iter().chain([&Stage::AwaitingCertChoice, &Stage::Done, &Stage::Failed]).map(|s| s.label_key().to_owned()));
        keys.extend(ERROR_CODES.iter().map(|c| format!("devices_err_{c}")));
        keys.extend(WARNINGS.iter().chain(&HEADINGS).map(|k| (*k).to_owned()));
        keys.push("settings_page_devices".into());
        keys.push("devices_cancel".into());
        let missing: Vec<&String> = keys.iter().filter(|k| dicts.iter().any(|d| !d.contains_key(&format!("native_{k}")))).collect();
        assert!(missing.is_empty(), "sem native_* em pt.json/en.json: {missing:?}");
    }

    #[test]
    fn devices_every_image_exists_or_is_pending() {
        use gpui_kit::AssetSource;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("install-guide");
        for image in all_steps().iter().filter_map(|s| s.image) {
            if PENDING_CAPTURE.contains(&image) {
                assert!(!dir.join(image).exists(), "{image} já existe: sai do PENDING_CAPTURE");
                continue;
            }
            assert!(dir.join(image).is_file(), "{image} não existe em assets/install-guide nem no PENDING_CAPTURE");
            assert!(matches!(crate::AppAssets.load(&format!("{ASSET_DIR}{image}")), Ok(Some(_))), "{image} fora do AppAssets");
        }
    }
}
