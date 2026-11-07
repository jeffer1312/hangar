//! Hangar Connect deste servidor: a ligação com a conta, o endereço público e os aparelhos que entram por ele.
//! Aparelho novo pedindo entrada vira cartão fora desta página (`render_vault_float`): é assim que o celular entra.
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, Page};

/// O serviço é um só: ninguém digita o endereço dele.
const SERVICE_URL: &str = "https://connect.hangar.dev.br";
/// A lista do dono não avisa de aparelho novo: quem pede entrada espera até isto pelo cartão.
pub(super) const POLL_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Status { enabled: bool, running: bool, host: Option<String>, error: Option<String> }

fn parse_status(value: Value) -> Result<Status, String> {
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).ok_or_else(|| tr("invalid_response"));
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(Status { enabled: flag("enabled")?, running: flag("running")?, host: text("host"), error: text("error") })
}

/// `running` é a supervisão ligada, `enabled` os dois filhos de pé: ligado e subindo ainda não é pronto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase { Unlinked, Stopped, Starting, Ready }

fn phase(status: &Status) -> Phase {
    match (status.running, status.enabled, status.host.is_some()) {
        (true, true, _) => Phase::Ready,
        (true, false, _) => Phase::Starting,
        (false, _, true) => Phase::Stopped,
        (false, _, false) => Phase::Unlinked,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeviceStatus { Pending, Approved, Denied }

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Device { device_id: String, name: String, status: DeviceStatus, last_seen: Option<u64> }

fn parse_devices(value: Value) -> Result<Vec<Device>, String> {
    let all: Vec<Device> = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    // Recusado só fica guardado para o aparelho ler a recusa.
    Ok(all.into_iter().filter(|device| device.status != DeviceStatus::Denied).collect())
}

enum DeviceView<'a> { Loading, Failed(&'a str), Empty, Loaded(&'a [Device]) }

/// O que a lista mostra. Numa releitura o valor anterior fica na tela: `Loading` é só a primeira.
fn device_view(list: &Remote<Vec<Device>>) -> DeviceView<'_> {
    match &list.value {
        None => DeviceView::Loading,
        Some(Err(error)) => DeviceView::Failed(error),
        Some(Ok(devices)) if devices.is_empty() => DeviceView::Empty,
        Some(Ok(devices)) => DeviceView::Loaded(devices),
    }
}

/// Recusa conhecida na língua da pessoa; o resto, a frase do servidor.
fn failure_text(error: &Failure) -> String {
    match error.code.as_deref() {
        Some("connect_login_refused") => tr_shared("connect_error_login", &[]),
        Some("connect_name_invalid" | "connect_service_refused") => tr_shared("connect_account_rules", &[]),
        Some("connect_service_busy") => tr_shared("connect_error_limit", &[]),
        Some("connect_service_unreachable") => tr_shared("connect_error_network", &[]),
        Some("connect_service_failed" | "connect_service_invalid") => tr_shared("connect_error_unavailable", &[]),
        Some("connect_not_linked") => tr_shared("connect_error_not_linked", &[]),
        Some("connect_already_linked") => tr("connect_error_already_linked"),
        Some("connect_machine_limit") => tr("connect_error_machine_limit"),
        Some("connect_preview_port_refused") => tr("connect_preview_port_refused"),
        _ => Hangar::fetch_failure(error),
    }
}

fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

fn parse_preview(value: Value) -> Result<String, String> {
    value.get("url").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response"))
}

/// Devolve a senha e já esvazia o campo. `set_value` limpa também o desfazer: o Ctrl+Z não a traz de volta.
fn take_password(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> String {
    let password = input.read(cx).value().to_string();
    input.update(cx, |input, cx| input.set_value("", window, cx));
    password
}

fn submit_on_enter(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Hangar>) -> Subscription {
    cx.subscribe_in(input, window, |this: &mut Hangar, _, event: &InputEvent, window, cx| {
        if let InputEvent::PressEnter { .. } = event { this.submit_link(window, cx); }
    })
}

struct LinkForm { account: Entity<InputState>, password: Entity<InputState>, machine: Entity<InputState>, preview_port: Entity<InputState>, _enter: [Subscription; 2] }

impl LinkForm {
    fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let account = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let machine = cx.new(|cx| InputState::new(window, cx));
        let preview_port = cx.new(|cx| InputState::new(window, cx));
        let _enter = [submit_on_enter(&password, window, cx), submit_on_enter(&machine, window, cx)];
        Self { account, password, machine, preview_port, _enter }
    }
}

#[derive(Default)]
pub(in crate::app) struct Connect {
    status: Remote<Status>,
    devices: Remote<Vec<Device>>,
    saving: bool,
    error: Option<String>,
    revoking: Option<String>,
    device_error: Option<String>,
    form: Option<LinkForm>,
    preview_url: Option<String>,
    preview_busy: bool,
    preview_error: Option<String>,
}

pub(super) enum ConnectReply {
    Loaded(u64, Result<Value, Failure>),
    Saved(Result<Value, Failure>),
    Devices(u64, Result<Value, Failure>),
    Revoked(Result<Value, Failure>),
    PreviewOpened(Result<Value, Failure>),
    PreviewClosed(Result<Value, Failure>),
}

#[derive(Clone, Debug, PartialEq)]
struct Request { device: Device, sending: bool, error: Option<String> }

/// Os pendentes de `devices`, guardando o envio e a falha do cartão que já estava na tela.
fn merge(old: &[Request], devices: &[Device]) -> Vec<Request> {
    devices.iter().filter(|device| device.status == DeviceStatus::Pending).map(|device| {
        let seen = old.iter().find(|card| card.device.device_id == device.device_id);
        Request { device: device.clone(), sending: seen.is_some_and(|card| card.sending), error: seen.and_then(|card| card.error.clone()) }
    }).collect()
}

/// Por máquina, chave `servers::norm`: sobrevive à troca do ativo, ao contrário de `Connect`.
#[derive(Default)]
pub(in crate::app) struct Notices { pending: HashMap<String, Vec<Request>>, refused: HashSet<String> }

impl Notices {
    fn card(&mut self, machine: &str, device: &str) -> Option<&mut Request> {
        self.pending.get_mut(machine)?.iter_mut().find(|card| card.device.device_id == device)
    }
}

/// Fora de `ConnectReply` porque cada máquina responde por si: a troca do ativo não descarta a resposta.
pub(super) enum NoticeReply {
    Listed(String, Result<Value, Failure>),
    Answered { machine: String, device: String, result: Result<Value, Failure> },
}

fn device_card(card: &Request, machine: &str, approve: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    deny: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let id = &card.device.device_id;
    let button = |action: &str, key: &str| Button::new(SharedString::from(format!("connect-{action}-{id}"))).xsmall().w_full()
        .label(tr_shared(key, &[])).disabled(card.sending);
    div().id(SharedString::from(format!("connect-request-{id}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.)).bg(theme::inset())
        .flex().flex_col().gap_1().role(Role::Group).aria_label(card.device.name.clone())
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning()).whitespace_normal()
            .child(tr_shared("connect_device_pending", &[("machine", machine)])))
        .child(div().text_sm().text_color(theme::text()).whitespace_normal().child(card.device.name.clone()))
        .child(request_pair(button("approve", "connect_device_approve").primary().on_click(approve),
            button("deny", "connect_device_deny").outline().on_click(deny)))
        .children(card.error.clone().map(|error| div().id(SharedString::from(format!("connect-request-error-{id}"))).role(Role::Alert)
            .text_xs().text_color(theme::danger()).whitespace_normal().child(error)))
}

impl Hangar {
    fn connect_send_later(&self) -> impl Fn(ConnectReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Connect(reply) }).await; })
        }
    }

    // Senha digitada e não enviada não fica no campo fora da página.
    pub(super) fn connect_page_left(&mut self) { self.connect.form = None; }

    /// Conexão nova: um "Ligar" em voo da anterior deixaria a página travada, e o formulário dela montado.
    pub(super) fn connect_reset(&mut self) {
        self.connect = Connect::default();
        self.connect_notices.refused.clear();
    }

    pub(super) fn connect_opened(&mut self, cx: &mut Context<Self>) {
        (self.connect.error, self.connect.device_error) = (None, None);
        self.load_connect(cx);
        self.load_connect_devices(cx);
    }

    fn load_connect(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.connect.status.start();
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::Loaded(seq, api.server_read(&["connect"], &[], 10).await)).await });
        cx.notify();
    }

    fn load_connect_devices(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.connect.devices.start();
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::Devices(seq, api.server_read(&["connect", "devices"], &[], 10).await)).await });
        cx.notify();
    }

    fn submit_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connect.saving { return; }
        let Some(form) = &self.connect.form else { return };
        let password = take_password(&form.password, window, cx);
        let account = form.account.read(cx).value().trim().to_string();
        let machine_name = form.machine.read(cx).value().trim().to_string();
        if account.is_empty() || password.is_empty() || machine_name.is_empty() {
            self.connect.error = Some(tr("connect_fields_required"));
            cx.notify();
            return;
        }
        let Some(api) = self.api.clone() else { return };
        (self.connect.saving, self.connect.error) = (true, None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let body = json!({"service_url": SERVICE_URL, "account": account, "password": password, "machine_name": machine_name});
            // Ligar pode baixar o frpc e o Caddy na primeira vez: prazo longo.
            done(ConnectReply::Saved(api.server_send(reqwest::Method::POST, &["connect", "link"], Some(body), 180).await)).await
        });
        cx.notify();
    }

    fn confirm_unlink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("connect_off_title"), tr("connect_off_body"), tr("connect_off"), ButtonVariant::Danger, move |_, cx| {
            let _ = this.update(cx, |this, cx| this.unlink_connect(cx));
            true
        });
    }

    fn unlink_connect(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.connect.saving { return; }
        (self.connect.saving, self.connect.error) = (true, None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::Saved(api.server_send(reqwest::Method::DELETE, &["connect"], None, 30).await)).await });
        cx.notify();
    }

    fn start_preview(&mut self, cx: &mut Context<Self>) {
        if self.connect.preview_busy { return; }
        let Some(form) = &self.connect.form else { return };
        let Some(port) = parse_port(&form.preview_port.read(cx).value().to_string()) else {
            self.connect.preview_error = Some(tr("connect_preview_port_invalid"));
            cx.notify();
            return;
        };
        let Some(api) = self.api.clone() else { return };
        (self.connect.preview_busy, self.connect.preview_error) = (true, None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            // A primeira prévia reinicia o `frpc` e o Caddy: prazo longo.
            done(ConnectReply::PreviewOpened(api.server_send(reqwest::Method::POST, &["connect", "preview"], Some(json!({"port": port})), 60).await)).await
        });
        cx.notify();
    }

    fn stop_preview(&mut self, cx: &mut Context<Self>) {
        if self.connect.preview_busy { return; }
        let Some(api) = self.api.clone() else { return };
        (self.connect.preview_busy, self.connect.preview_error) = (true, None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            done(ConnectReply::PreviewClosed(api.server_send(reqwest::Method::DELETE, &["connect", "preview"], None, 15).await)).await
        });
        cx.notify();
    }

    fn confirm_revoke(&mut self, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let machine = self.vault_machine_label(&self.active_key(), cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_device_revoke_title", &[("name", &name)]), tr_shared("connect_device_revoke_body", &[("machine", &machine)]),
            tr_shared("connect_device_revoke", &[]), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.revoke_device(device.clone(), cx));
                true
            });
    }

    fn revoke_device(&mut self, device: String, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.connect.revoking.is_some() { return; }
        (self.connect.revoking, self.connect.device_error) = (Some(device.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            done(ConnectReply::Revoked(api.server_send(reqwest::Method::DELETE, &["connect", "devices", &device], None, 15).await)).await
        });
        cx.notify();
    }

    pub(super) fn receive_connect(&mut self, reply: ConnectReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            ConnectReply::Loaded(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_status);
                if !self.connect.status.finish(seq, result) { return; }
            }
            ConnectReply::Saved(result) => {
                self.connect.saving = false;
                match result.map_err(|error| failure_text(&error)).and_then(parse_status) {
                    Ok(status) => self.connect.status.set(Ok(status)),
                    // O envio pode ter estourado o prazo com o servidor já ligado: relê em vez de supor.
                    Err(error) => { self.connect.error = Some(error); self.load_connect(cx); }
                }
                self.load_connect_devices(cx);
            }
            ConnectReply::Devices(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_devices);
                if let Ok(devices) = &result { self.set_pending(&self.active_key(), devices); }
                if !self.connect.devices.finish(seq, result) { return; }
            }
            ConnectReply::Revoked(result) => {
                self.connect.revoking = None;
                self.connect.device_error = result.err().map(|error| failure_text(&error));
                self.load_connect_devices(cx);
            }
            ConnectReply::PreviewOpened(result) => {
                self.connect.preview_busy = false;
                match result.map_err(|error| failure_text(&error)).and_then(parse_preview) {
                    Ok(url) => self.connect.preview_url = Some(url),
                    Err(error) => self.connect.preview_error = Some(error),
                }
            }
            ConnectReply::PreviewClosed(result) => {
                self.connect.preview_busy = false;
                match result {
                    Ok(_) => self.connect.preview_url = None,
                    Err(error) => self.connect.preview_error = Some(failure_text(&error)),
                }
            }
        }
        if self.connect.form.is_none() && self.settings == Some(Page::Connect) {
            self.connect.form = Some(LinkForm::new(window, cx));
        }
        cx.notify();
    }

    /// Devolve se os cartões da máquina mudaram.
    fn set_pending(&mut self, machine: &str, devices: &[Device]) -> bool {
        let old = self.connect_notices.pending.remove(machine).unwrap_or_default();
        let cards = merge(&old, devices);
        let changed = cards != old;
        if !cards.is_empty() { self.connect_notices.pending.insert(machine.to_owned(), cards); }
        changed
    }

    /// Máquinas que o cartão acompanha. Convite não é do dono, e a rota de aparelhos recusaria a cada rodada.
    fn connect_machines(&self) -> Vec<(String, Api)> {
        let mut machines: Vec<(String, Api)> = self.remote.iter().filter(|(_, list)| list.online)
            .filter_map(|(key, list)| Some((key.clone(), list.api.clone()?))).collect();
        machines.extend(self.api.clone().map(|api| (self.active_key(), api)));
        machines.retain(|(key, _)| !self.connect_notices.refused.contains(key) && !self.server_entry(key).is_some_and(|entry| entry.invite));
        machines
    }

    fn read_pending(&self, machine: String, api: Api) {
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = api.server_read(&["connect", "devices"], &[], 8).await;
            let _ = tx.send(Envelope { connection: 0, selection: None, payload: Payload::ConnectNotice(NoticeReply::Listed(machine, result)) }).await;
        });
    }

    /// Com a página aberta, relê também o estado: ligado e subindo vira pronto sem ninguém clicar.
    pub(super) fn connect_tick(&mut self, cx: &mut Context<Self>) {
        let machines = self.connect_machines();
        self.connect_notices.pending.retain(|key, _| machines.iter().any(|(machine, _)| machine == key));
        for (machine, api) in machines { self.read_pending(machine, api); }
        if self.settings == Some(Page::Connect) && !self.connect.saving {
            self.load_connect(cx);
            self.load_connect_devices(cx);
        }
    }

    fn confirm_approve(&mut self, machine: String, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let label = self.vault_machine_label(&machine, cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_device_approve_title", &[("name", &name)]), tr_shared("connect_device_approve_body", &[("machine", &label)]),
            tr_shared("connect_device_approve", &[]), ButtonVariant::Primary, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.answer_device(machine.clone(), device.clone(), true, cx));
                true
            });
    }

    fn answer_device(&mut self, machine: String, device: String, approve: bool, cx: &mut Context<Self>) {
        let api = self.machine_api(&machine);
        let Some(card) = self.connect_notices.card(&machine, &device).filter(|card| !card.sending) else { return };
        let Some(api) = api else {
            card.error = Some(tr("network_error"));
            cx.notify();
            return;
        };
        (card.sending, card.error) = (true, None);
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = if approve { api.server_send(reqwest::Method::POST, &["connect", "devices", &device, "approve"], None, 15).await }
                else { api.server_send(reqwest::Method::DELETE, &["connect", "devices", &device], None, 15).await };
            let _ = tx.send(Envelope { connection: 0, selection: None, payload: Payload::ConnectNotice(NoticeReply::Answered { machine, device, result }) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_connect_notice(&mut self, reply: NoticeReply, cx: &mut Context<Self>) {
        match reply {
            NoticeReply::Listed(machine, result) => match result.map(parse_devices) {
                Ok(Ok(devices)) => if self.set_pending(&machine, &devices) { cx.notify(); },
                Ok(Err(_)) => crate::log_line("connect: lista de aparelhos ilegível; os cartões anteriores ficam"),
                // Sem a rota ou sem o login do dono, repetir a cada rodada só encheria as falhas recentes.
                Err(error) if matches!(error.status, Some(401 | 403 | 404)) => {
                    crate::log_line(&format!("connect: máquina sem a lista de aparelhos ({:?}); sem cartão nesta conexão", error.status));
                    self.connect_notices.refused.insert(machine.clone());
                    if self.connect_notices.pending.remove(&machine).is_some() { cx.notify(); }
                }
                // Acompanhamento: a falha fica nas recentes do servidor, sem aviso na tela.
                Err(_) => {}
            },
            NoticeReply::Answered { machine, device, result } => {
                match result {
                    Ok(_) => if let Some(cards) = self.connect_notices.pending.get_mut(&machine) { cards.retain(|card| card.device.device_id != device); },
                    Err(error) => if let Some(card) = self.connect_notices.card(&machine, &device) { (card.sending, card.error) = (false, Some(failure_text(&error))); },
                }
                if let Some(api) = self.machine_api(&machine) { self.read_pending(machine.clone(), api); }
                if self.is_active_key(&machine) && self.settings == Some(Page::Connect) { self.load_connect_devices(cx); }
                cx.notify();
            }
        }
    }

    /// Os cartões de aparelho novo, a máquina ativa primeiro; `only` restringe a uma máquina.
    pub(super) fn connect_request_cards(&self, only: Option<&str>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active = self.active_key();
        let mut keys: Vec<&String> = self.connect_notices.pending.keys().filter(|key| only.is_none_or(|only| only == key.as_str())).collect();
        keys.sort_by_key(|key| (**key != active, (*key).clone()));
        let mut cards = Vec::new();
        for key in keys {
            let label = self.vault_machine_label(key, cx);
            for card in &self.connect_notices.pending[key] {
                let (machine, device, name) = (key.clone(), card.device.device_id.clone(), card.device.name.clone());
                let approve = cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_approve(machine.clone(), device.clone(), name.clone(), window, cx));
                let (machine, device) = (key.clone(), card.device.device_id.clone());
                let deny = cx.listener(move |this, _: &ClickEvent, _, cx| this.answer_device(machine.clone(), device.clone(), false, cx));
                cards.push(device_card(card, &label, approve, deny).into_any_element());
            }
        }
        cards
    }

    fn render_device_row(&self, device: &Device, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let seen = device.last_seen.map_or_else(|| tr("connect_never_seen"),
            |at| tr("connect_last_seen").replace("{when}", &super::side::ago(super::side::now_seconds() - at as f64)));
        let (id, name) = (device.device_id.clone(), device.name.clone());
        div().flex().items_center().gap_2().px_3().py_2().when(!first, |el| el.border_t_1().border_color(theme::border()))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_sm().whitespace_normal().child(device.name.clone()))
                .child(div().text_xs().text_color(theme::muted()).child(seen)))
            .child(Button::new(SharedString::from(format!("connect-revoke-{id}"))).outline().small().flex_shrink_0()
                .label(tr_shared("connect_device_revoke", &[])).disabled(self.connect.revoking.is_some())
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_revoke(id.clone(), name.clone(), window, cx))))
            .into_any_element()
    }

    fn render_connect_devices(&self, cx: &mut Context<Self>) -> Div {
        let key = self.active_key();
        let machine = self.vault_machine_label(&key, cx);
        let section = div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr_shared("connect_devices_title", &[("machine", &machine)])))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("connect_devices_help", &[])))
            .children(self.connect.device_error.clone().map(|error| div().id("connect-device-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .children(self.connect_request_cards(Some(&key), cx));
        match device_view(&self.connect.devices) {
            DeviceView::Loading => section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))),
            DeviceView::Failed(error) => section
                .child(div().id("connect-devices-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))
                .child(div().child(Button::new("connect-devices-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_connect_devices(cx))))),
            DeviceView::Empty => section.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr_shared("connect_devices_empty", &[]))),
            DeviceView::Loaded(devices) => {
                let rows: Vec<AnyElement> = devices.iter().filter(|device| device.status == DeviceStatus::Approved).enumerate()
                    .map(|(at, device)| self.render_device_row(device, at == 0, cx)).collect();
                section.when(!rows.is_empty(), |el| el.child(settings_box().children(rows)))
            }
        }
    }

    /// Parar fica sempre à mão: a prévia mora no servidor, e o app reaberto não sabe se há uma.
    fn render_connect_preview(&self, cx: &mut Context<Self>) -> Option<Div> {
        let form = self.connect.form.as_ref()?;
        let busy = self.connect.preview_busy;
        let link = self.connect.preview_url.clone().map(|url| {
            let copied = url.clone();
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().font_family(theme::MONO).text_sm().truncate().child(url))
                .child(Button::new("connect-preview-copy").outline().small().flex_shrink_0().label(tr("connect_preview_copy"))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))))
        });
        Some(div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_preview_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_preview_help")))
            .child(super::sync::sync_field(tr("connect_preview_port"), &form.preview_port, busy))
            .children(self.connect.preview_error.clone().map(|error| div().id("connect-preview-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("connect-preview-start").primary().small().label(tr("connect_preview_start")).loading(busy).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.start_preview(cx))))
                .child(Button::new("connect-preview-stop").outline().small().label(tr("connect_preview_stop")).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.stop_preview(cx)))))
            .children(link))
    }

    pub(super) fn render_connect(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Connect.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let mut page = div().flex().flex_col().gap_4().child(title)
            .child(div().id("connect-draft").role(Role::Alert).text_sm().font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::danger()).whitespace_normal().child(tr("connect_draft")))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_intro")));
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
        }
        if let Some(error) = &self.connect.error {
            page = page.child(div().id("connect-write-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        let status = match &self.connect.status.value {
            Some(Ok(status)) => status.clone(),
            Some(Err(error)) => return page.child(div().id("connect-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).child(error.clone()))
                .child(Button::new("connect-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.connect.error = None; this.load_connect(cx); }))).into_any_element(),
            None => return page.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))).into_any_element(),
        };
        // A subida que falhou volta no envio e também no estado: uma frase só na tela.
        if let Some(error) = status.error.as_ref().filter(|error| self.connect.error.as_ref() != Some(*error)) {
            page = page.child(div().id("connect-run-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        let (state, saving) = (phase(&status), self.connect.saving);
        let linked = state != Phase::Unlinked;
        if linked {
            let label = match state { Phase::Ready => "connect_on", Phase::Starting => "connect_starting", _ => "connect_stopped" };
            page = page.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr(label)))
                .children(status.host.clone().map(|host| div().font_family(theme::MONO).text_sm().truncate().child(format!("https://{host}"))));
        }
        if matches!(state, Phase::Unlinked | Phase::Stopped) && let Some(form) = &self.connect.form {
            let help = if linked { tr("connect_stopped_help") } else { format!("{} {}", tr("connect_link_help"), tr_shared("connect_account_rules", &[])) };
            let label = if saving { "connect_turning_on" } else if linked { "connect_turn_on_again" } else { "connect_turn_on" };
            page = page.child(super::sync::sync_field(tr_shared("connect_account_name", &[]), &form.account, saving))
                .child(super::sync::sync_field(tr("connect_password"), &form.password, saving))
                .child(super::sync::sync_field(tr("connect_machine_name"), &form.machine, saving))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(help))
                .child(div().child(Button::new("connect-on").primary().small().label(tr(label)).loading(saving).disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_link(window, cx)))));
        }
        if linked {
            let open = (state == Phase::Ready).then(|| status.host.clone()).flatten().map(|host| {
                let address = format!("https://{host}");
                Button::new("connect-open").outline().small().label(tr("connect_open")).on_click(move |_, _, cx| cx.open_url(&address))
            });
            page = page.child(div().flex().items_center().gap_2()
                .children(open)
                .child(Button::new("connect-refresh").outline().small().label(tr("connect_refresh"))
                    .on_click(cx.listener(|this, _, _, cx| { this.load_connect(cx); this.load_connect_devices(cx); })))
                .child(Button::new("connect-off").outline().small().label(tr("connect_off")).disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_unlink(window, cx)))));
        }
        if state == Phase::Ready {
            page = page.children(self.render_connect_preview(cx));
        }
        // Desligar guarda as credenciais dos aparelhos: quem ficou com uma ainda pode ser revogado.
        if linked || self.connect.devices.ok().is_some_and(|devices| !devices.is_empty()) {
            page = page.child(self.render_connect_devices(cx));
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{device_card, device_view, merge, parse_devices, parse_port, parse_preview, parse_status, phase, take_password, Device, DeviceStatus, DeviceView, InputState, Phase, Remote, Request};
    use crate::i18n::tr_shared;
    // O nome explícito vence qualquer `test` trazido por glob.
    use core::prelude::v1::test;
    use gpui::{Context, Entity, IntoElement, Render, TestAppContext, Window, div, prelude::*};
    use serde_json::json;

    fn device(id: &str, status: DeviceStatus) -> Device {
        Device { device_id: id.to_owned(), name: format!("Pixel {id}"), status, last_seen: None }
    }

    struct Field { input: Entity<InputState> }

    impl Render for Field {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div() }
    }

    #[gpui::test]
    fn link_form_never_keeps_the_password_after_submit(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| Field { input: cx.new(|cx| InputState::new(window, cx).masked(true)) });
        let input = view.update(cx, |view, _| view.input.clone());
        let taken = cx.update(|window, cx| {
            input.update(cx, |input, cx| input.set_value("segredo", window, cx));
            take_password(&input, window, cx)
        });
        assert_eq!(taken, "segredo");
        assert!(cx.update(|_, cx| input.read(cx).value().to_string()).is_empty());
    }

    #[test]
    fn device_list_handles_empty_and_error_states() {
        let mut list = Remote::<Vec<Device>>::default();
        assert!(matches!(device_view(&list), DeviceView::Loading));
        let seq = list.start();
        assert!(matches!(device_view(&list), DeviceView::Loading));
        list.finish(seq, Err("fora do ar".to_owned()));
        assert!(matches!(device_view(&list), DeviceView::Failed("fora do ar")));
        let seq = list.start();
        list.finish(seq, parse_devices(json!([{"device_id": "a", "name": "Velho", "status": "denied", "last_seen": null}])));
        assert!(matches!(device_view(&list), DeviceView::Empty), "recusado não aparece");
        let seq = list.start();
        list.finish(seq, parse_devices(json!([{"device_id": "b", "name": "Pixel", "status": "approved", "last_seen": 1700000000}])));
        assert!(matches!(device_view(&list), DeviceView::Loaded(rows) if rows.len() == 1 && rows[0].last_seen == Some(1700000000)));
        // Releitura em curso não apaga a lista da tela.
        list.start();
        assert!(matches!(device_view(&list), DeviceView::Loaded(_)));
    }

    struct Card { card: Request }

    impl Render for Card {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { device_card(&self.card, "mesa", |_, _, _| {}, |_, _, _| {}) }
    }

    #[gpui::test]
    fn pending_device_shows_an_approval_card(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let name = "iPhone {machine} <b>da Ana</b>";
        let card = Request { device: Device { name: name.to_owned(), ..device("a", DeviceStatus::Pending) }, sending: false, error: None };
        let (_, cx) = cx.add_window_view(|window, _| { window.retain_a11y_tree(true); Card { card } });
        cx.run_until_parked();
        let snapshot = cx.update(|window, cx| { let _ = window.draw(cx); window.a11y_snapshot(None).unwrap() });
        assert!(snapshot.contains(name), "o nome sai como o servidor mandou: {snapshot}");
        assert!(snapshot.contains(&tr_shared("connect_device_pending", &[("machine", "mesa")])));
        assert!(snapshot.contains(&tr_shared("connect_device_approve", &[])));
        assert!(snapshot.contains(&tr_shared("connect_device_deny", &[])));
    }

    #[test]
    fn only_pending_devices_become_cards_and_a_refresh_keeps_their_failure() {
        let devices = [device("a", DeviceStatus::Pending), device("b", DeviceStatus::Approved)];
        let cards = merge(&[], &devices);
        assert_eq!(cards.iter().map(|card| card.device.device_id.as_str()).collect::<Vec<_>>(), ["a"]);
        let failed = [Request { error: Some("recusado".to_owned()), ..cards[0].clone() }];
        assert_eq!(merge(&failed, &devices)[0].error.as_deref(), Some("recusado"));
        assert!(merge(&failed, &[]).is_empty(), "aparelho decidido em outro lugar sai da tela");
    }

    #[test]
    fn preview_form_reads_the_port_and_the_link() {
        assert_eq!(parse_port(" 3000 "), Some(3000));
        for bad in ["", "0", "65536", "porta"] {
            assert_eq!(parse_port(bad), None, "{bad}");
        }
        assert_eq!(parse_preview(json!({"url": "https://a/?hangar_preview=b"})).as_deref(), Ok("https://a/?hangar_preview=b"));
        assert!(parse_preview(json!({})).is_err());
    }

    #[test]
    fn linked_and_starting_is_not_ready() {
        let at = |running: bool, enabled: bool, host: Option<&str>| {
            phase(&parse_status(json!({"enabled": enabled, "running": running, "host": host, "error": null})).unwrap())
        };
        assert_eq!(at(false, false, None), Phase::Unlinked);
        assert_eq!(at(false, false, Some("mesa.conta.connect.hangar.dev.br")), Phase::Stopped);
        assert_eq!(at(true, false, Some("mesa.conta.connect.hangar.dev.br")), Phase::Starting);
        assert_eq!(at(true, true, Some("mesa.conta.connect.hangar.dev.br")), Phase::Ready);
        assert!(parse_status(json!({"enabled": true})).is_err(), "formato antigo não passa por ligado");
    }
}
