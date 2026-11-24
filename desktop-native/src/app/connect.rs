//! Hangar Connect deste servidor: a ligação com a conta, o endereço público e os aparelhos da conta.
//! Aparelho novo é liberado uma vez na conta, em "Aparelhos da conta", e passa a entrar em todas as máquinas dela.
//! O que entrou só nesta máquina, sem a conta, fica em "Aparelhos desta máquina".
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, Page};

/// O serviço é um só: ninguém digita o endereço dele.
const SERVICE_URL: &str = "https://connect.hangar.dev.br";
/// Ninguém avisa de aparelho novo na conta: com a página aberta, quem pede liberação aparece em até isto.
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

/// `Gone` junta o revogado (lápide que a conta guarda para as máquinas) e um estado que este app ainda não conhece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeviceStatus { Pending, Approved, #[serde(other)] Gone }

/// Serviço antigo não manda o tipo: todo mundo é aparelho.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeviceKind { Machine, #[default] #[serde(other)] Device }

/// Uma linha de `GET /api/connect/account-devices`: celular ou PC da conta. O PC tem o nome da máquina no serviço.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Device { device_id: String, name: String, status: DeviceStatus, #[serde(default)] kind: DeviceKind, created_at: f64 }

fn parse_devices(value: Value) -> Result<Vec<Device>, String> {
    let mut all: Vec<Device> = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    all.retain(|device| device.status != DeviceStatus::Gone);
    // O pedido esperando liberação é o que pede ação.
    all.sort_by_key(|device| device.status != DeviceStatus::Pending);
    Ok(all)
}

/// Os botões da linha: pendente é Liberar/Recusar, liberado é Tirar acesso. Esta máquina não se tira da conta por aqui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowActions { Answer, Revoke, This }

/// O PC da conta tem como nome, na lista, o `machine_id` do vínculo: é por ele que esta máquina se reconhece, nunca pelo rótulo.
fn row_actions(device: &Device, this_machine_id: &str) -> RowActions {
    match device.status {
        DeviceStatus::Approved if device.kind == DeviceKind::Machine && device.name == this_machine_id => RowActions::This,
        DeviceStatus::Pending => RowActions::Answer,
        _ => RowActions::Revoke,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum LocalStatus { Pending, Approved, #[serde(other)] Denied }

/// Uma linha de `GET /api/connect/devices`: aparelho aprovado ou esperando nesta máquina.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct LocalDevice {
    device_id: String, name: String, status: LocalStatus, #[serde(default)] last_seen: Option<u64>,
    /// O código que o celular mostra, pronto do servidor; servidor antigo não manda.
    #[serde(default)] install_code: Option<String>,
    /// Entrou pela conta: aparece em "Aparelhos da conta", não aqui.
    #[serde(default)] account_device: Option<String>,
}

/// Só os que não vieram pela conta; recusado só fica guardado para o aparelho ler a recusa. Pendente primeiro.
fn parse_local_devices(value: Value) -> Result<Vec<LocalDevice>, String> {
    let mut all: Vec<LocalDevice> = serde_json::from_value(value).map_err(|_| tr("invalid_response"))?;
    all.retain(|device| device.status != LocalStatus::Denied && device.account_device.is_none());
    all.sort_by_key(|device| device.status != LocalStatus::Pending);
    Ok(all)
}

enum DeviceView<'a, T> { Loading, Failed(&'a str), Empty, Loaded(&'a [T]) }

/// O que a lista mostra. Numa releitura o valor anterior fica na tela: `Loading` é só a primeira.
fn device_view<T>(list: &Remote<Vec<T>>) -> DeviceView<'_, T> {
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
        Some("connect_account_taken") => tr_shared("connect_error_name_taken", &[]),
        Some("connect_account_created_link_failed") => tr("connect_error_account_created"),
        Some("connect_preview_port_refused") => tr("connect_preview_port_refused"),
        Some("connect_not_running") => tr("ports_link_connect_off"),
        Some("port_not_http") => tr("ports_not_page"),
        Some("connect_account_machine_pending") => tr_shared("connect_device_waiting_release", &[]),
        Some("connect_account_machine_unknown" | "connect_account_machine_revoked") => tr("connect_account_machine_unknown"),
        Some("connect_account_device_unknown" | "connect_account_device_revoked") => tr("connect_account_device_gone"),
        _ => Hangar::fetch_failure(error),
    }
}

/// O serviço dá a cada máquina `<máquina>.<conta>.<domínio>`.
const ACCOUNT_DOMAIN: &str = "hangar.dev.br";
/// Primeira ligação: os arcos acendem um depois do outro.
const MARK_IN: Duration = Duration::from_millis(900);
const MARK_PULSE: Duration = Duration::from_millis(2400);

/// O endereço que a conta vai dar, mostrado enquanto a pessoa digita; campo vazio mostra o exemplo.
fn address_preview(machine: &str, account: &str) -> String {
    let part = |value: &str, fallback: &str| { let value = service_name(value); if value.is_empty() { fallback.to_owned() } else { value } };
    format!("{}.{}.{ACCOUNT_DOMAIN}", part(machine, "notebook"), part(account, "sua-conta"))
}

/// Nome de conta ou de máquina como vai ao serviço: a prévia do endereço e o envio usam esta mesma forma.
fn service_name(value: &str) -> String { value.trim().to_lowercase() }

/// `(máquina, conta)` do host que o serviço deu a esta máquina.
fn identity_of(host: &str) -> Option<(&str, &str)> {
    let mut labels = host.split('.');
    Some((labels.next().filter(|l| !l.is_empty())?, labels.next().filter(|l| !l.is_empty())?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry { Create, Login }

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct AccountMachine { machine_id: String, name: String, host: String, online: bool, added: bool }

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
struct ThisMachine { machine_id: String }

/// `GET /api/connect/machines`: as outras máquinas da conta; `added` diz se já estão no `peers.json` deste servidor.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct AccountList { this: ThisMachine, machines: Vec<AccountMachine> }

fn parse_account(value: Value) -> Result<AccountList, String> {
    serde_json::from_value(value).map_err(|_| tr("invalid_response"))
}

/// Uma linha de `GET /api/connect/peers`: o pedido de ligar esta máquina a outra.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct PeerLink { machine_id: String, stage: String, #[serde(default)] message: Option<String> }

fn parse_links(value: Value) -> Result<Vec<PeerLink>, String> {
    serde_json::from_value(value).map_err(|_| tr("invalid_response"))
}

/// A marca do Hangar acendendo arco por arco; `pulse` repete um respiro enquanto o Connect liga.
pub(super) fn connect_mark(id: &str, size: f32, pulse: bool, reduce: bool) -> AnyElement {
    let arc = |path: &'static str| svg().path(path).absolute().top_0().left_0().size(px(size)).text_color(theme::accent());
    let mut mark = div().relative().size(px(size)).flex_shrink_0();
    if reduce { return mark.children(crate::CONNECT_MARK_ARCS.map(arc)).into_any_element(); }
    // De fora para dentro: uma animação por arco, cada um com a própria janela.
    let lit = |t: f32, from: f32| ((t - from) / 0.6).clamp(0., 1.);
    for (at, path) in crate::CONNECT_MARK_ARCS.into_iter().enumerate() {
        let from = at as f32 * 0.2;
        mark = mark.child(arc(path).with_animation(SharedString::from(format!("{id}-arc-{at}")), Animation::new(MARK_IN).with_easing(crate::motion::ease_out),
            move |el, t| el.opacity(lit(t, from)).top(px(3. * (1. - lit(t, from))))));
    }
    if !pulse { return mark.into_any_element(); }
    mark.with_animation(SharedString::from(format!("{id}-pulse")), Animation::new(MARK_PULSE).repeat(),
        |el, t| el.opacity(0.55 + 0.45 * (0.5 + 0.5 * (t * std::f32::consts::TAU).cos()))).into_any_element()
}

pub(super) fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

fn parse_preview(value: Value) -> Result<String, String> {
    value.get("url").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response"))
}

fn open_preview_port(value: &Value) -> Option<u64> {
    value.get("preview")?.get("port")?.as_u64()
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

/// O endereço de exemplo acompanha o que se digita na conta e no nome.
fn redraw_on_change(input: &Entity<InputState>, cx: &mut Context<Hangar>) -> Subscription {
    cx.subscribe(input, |_: &mut Hangar, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify(); })
}

struct LinkForm { account: Entity<InputState>, password: Entity<InputState>, machine: Entity<InputState>, _subscriptions: [Subscription; 4] }

/// Servidor guardado que é esta máquina do Connect: salvo pelo endereço dela ou com ela como terceiro caminho.
fn saved_with_connect(servers: &[super::servers::ServerEntry], host: &str) -> bool {
    let address = super::servers::norm(&format!("https://{host}"));
    servers.iter().any(|entry| super::servers::norm(&entry.address) == address
        || entry.connect.as_ref().is_some_and(|c| super::servers::norm(&c.url) == address))
}

/// O nome que o app já mostra para esta máquina, no formato que o serviço aceita: minúsculas, números e hífen, sem `--`.
fn machine_label(shown: &str) -> String {
    let mut label = String::new();
    for c in shown.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && (label.is_empty() || label.ends_with('-'))) { label.push(c); }
    }
    label.truncate(32);
    label.trim_end_matches('-').to_owned()
}

impl LinkForm {
    fn new(machine_name: String, window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let account = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let machine = cx.new(|cx| InputState::new(window, cx).default_value(machine_name));
        let _subscriptions = [submit_on_enter(&password, window, cx), submit_on_enter(&machine, window, cx),
            redraw_on_change(&account, cx), redraw_on_change(&machine, cx)];
        Self { account, password, machine, _subscriptions }
    }
}

#[derive(Default)]
pub(in crate::app) struct Connect {
    status: Remote<Status>,
    devices: Remote<Vec<Device>>,
    saving: bool,
    error: Option<String>,
    /// O aparelho da conta com Liberar, Recusar ou Tirar acesso em voo: um por vez.
    answering: Option<String>,
    device_error: Option<String>,
    /// Aparelhos desta máquina que não vieram pela conta: aprovados aqui e pedidos de app antigo.
    local: Remote<Vec<LocalDevice>>,
    /// O aparelho desta máquina com Aprovar, Recusar ou Tirar acesso em voo: um por vez.
    local_answering: Option<String>,
    local_error: Option<String>,
    form: Option<LinkForm>,
    /// A porta da prévia aberta no servidor e até quando (segundos Unix): o link pode ter saído da tela, a prévia não.
    preview_open: Option<(u64, Option<u64>)>,
    preview_busy: bool,
    preview_error: Option<String>,
    /// Sem ligação: `None` mostra a abertura; escolhido, o formulário de criar conta ou de entrar.
    entry: Option<Entry>,
    machines: Remote<AccountList>,
    /// Acompanhamento dos pedidos de ligar a outra máquina: falha de leitura só deixa a linha como estava.
    links: Vec<PeerLink>,
    links_failing: bool,
    /// Última releitura da lista da conta que falhou, mostrada junto da lista boa anterior.
    machines_error: Option<String>,
    linking: Option<String>,
    link_error: Option<String>,
}

impl Connect {
    /// Religar ao mesmo servidor guarda o que está na tela, inclusive o que foi digitado. As respostas da conexão
    /// anterior não chegam mais (o filtro descarta), então nada fica esperando por elas.
    pub(super) fn carry_over(mut self) -> Self {
        (self.saving, self.preview_busy, self.linking, self.answering, self.local_answering) = (false, false, None, None, None);
        self
    }

    /// Resposta de ligar ou desligar: a fase nova, ou `None` com o erro já na tela.
    fn saved(&mut self, result: Result<Value, Failure>) -> Option<Phase> {
        self.saving = false;
        // A conta nasceu e só a ligação falhou: criar de novo daria nome já usado, então o formulário vira o de entrar.
        if result.as_ref().is_err_and(|error| error.code.as_deref() == Some("connect_account_created_link_failed")) {
            self.entry = Some(Entry::Login);
        }
        match result.map_err(|error| failure_text(&error)).and_then(parse_status) {
            Ok(status) => {
                let now = phase(&status);
                // Desligado: as máquinas e os pedidos eram da ligação que acabou.
                if now == Phase::Unlinked {
                    (self.machines, self.machines_error, self.links, self.links_failing) = (Remote::default(), None, Vec::new(), false);
                }
                self.entry = None;
                self.status.set(Ok(status));
                Some(now)
            }
            Err(error) => { self.error = Some(error); None }
        }
    }

    /// Releitura que falha não apaga a lista boa: o erro fica ao lado dela. Sem lista, o erro ocupa o lugar.
    fn machines_read(&mut self, seq: u64, result: Result<AccountList, String>) -> bool {
        let failed = result.as_ref().err().cloned();
        let previous = if matches!(self.machines.value, Some(Ok(_))) { self.machines.value.take() } else { None };
        if !self.machines.finish(seq, result) {
            if previous.is_some() { self.machines.value = previous; }
            return false;
        }
        self.machines_error = match (failed, previous) {
            (Some(error), Some(previous)) => { self.machines.value = Some(previous); Some(error) }
            _ => None,
        };
        true
    }

    /// A lista da conta só é relida em rodada enquanto um pedido de ligar ainda espera resposta.
    fn linking_in_progress(&self) -> bool {
        self.links.iter().any(|link| matches!(link.stage.as_str(), "requesting" | "waiting_approval"))
    }

    /// O formulário que está na tela: com a ligação parada é sempre o de entrar ("Ligar de novo").
    fn shown_entry(&self) -> Entry {
        match self.status.ok().map(phase) {
            Some(Phase::Stopped) => Entry::Login,
            _ => self.entry.unwrap_or(Entry::Login),
        }
    }

    /// Acompanhamento dos pedidos de ligar: a tela fica com a última leitura boa, e o log ganha uma linha só quando
    /// a leitura passa a falhar ou volta.
    fn links_read(&mut self, result: Result<Value, Failure>) -> Option<String> {
        match result.map_err(|error| failure_text(&error)).and_then(parse_links) {
            Ok(links) => {
                let line = self.links_failing.then(|| "connect: connect_links_recovered".to_owned());
                (self.links, self.links_failing) = (links, false);
                line
            }
            Err(error) => {
                let line = (!self.links_failing).then(|| format!("connect: connect_links_unreadable: {error}"));
                self.links_failing = true;
                line
            }
        }
    }
}

pub(super) enum ConnectReply {
    Loaded(u64, Result<Value, Failure>),
    Machines(u64, Result<Value, Failure>),
    Links(Result<Value, Failure>),
    LinkStarted(Result<Value, Failure>),
    Saved(Result<Value, Failure>),
    Devices(u64, Result<Value, Failure>),
    Answered(Result<Value, Failure>),
    LocalDevices(u64, Result<Value, Failure>),
    LocalAnswered(Result<Value, Failure>),
    /// "Link de fora" de uma porta da aba Contexto: (servidor, porta).
    PortLink(String, u16, Result<Value, Failure>),
    PreviewClosed(Result<Value, Failure>),
    PreviewState(Result<Value, Failure>),
}

/// A prévia do Connect para `port` da máquina de `api`; abrir outra fecha a anterior.
pub(super) async fn request_preview(api: &Api, port: u16) -> Result<Value, Failure> {
    // A primeira prévia reinicia o `frpc` e o Caddy e espera o site abrir de fora: prazo longo.
    let body = json!({"port": port});
    let result = api.server_send(reqwest::Method::POST, &["connect", "preview"], Some(body.clone()), 90).await;
    // Quem pede pelo próprio Connect perde a conexão na troca dos filhos.
    if result.as_ref().is_err_and(|error| error.status.is_none()) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        return api.server_send(reqwest::Method::POST, &["connect", "preview"], Some(body), 90).await;
    }
    result
}

impl Hangar {
    pub(super) fn connect_send_later(&self) -> impl Fn(ConnectReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Connect(reply) }).await; })
        }
    }

    // Senha digitada e não enviada não fica no campo fora da página.
    pub(super) fn connect_page_left(&mut self) { self.connect.form = None; }

    /// Conexão nova: um "Ligar" em voo da anterior deixaria a página travada, e o formulário dela montado.
    pub(super) fn connect_reset(&mut self) { self.connect = Connect::default(); }

    pub(super) fn connect_opened(&mut self, cx: &mut Context<Self>) {
        (self.connect.error, self.connect.device_error, self.connect.local_error) = (None, None, None);
        self.load_connect(cx);
        self.load_connect_devices(cx);
        self.load_local_devices(cx);
        self.load_connect_preview();
        self.load_account_machines(cx);
        self.load_peer_links();
    }

    fn load_account_machines(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.connect.machines.start();
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::Machines(seq, api.server_read(&["connect", "machines"], &[], 15).await)).await });
        cx.notify();
    }

    fn load_peer_links(&self) {
        let Some(api) = self.api.clone() else { return };
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::Links(api.server_read(&["connect", "peers"], &[], 10).await)).await });
    }

    /// Pede a entrada nesta outra máquina pela conta: o cartão de aprovação aparece lá.
    fn add_account_machine(&mut self, id: String, cx: &mut Context<Self>) {
        if self.connect.linking.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        (self.connect.linking, self.connect.link_error) = (Some(id.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            done(ConnectReply::LinkStarted(api.server_send(reqwest::Method::POST, &["connect", "peers", &id, "link"], None, 15).await)).await
        });
        cx.notify();
    }

    fn load_connect_preview(&self) {
        let Some(api) = self.api.clone() else { return };
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::PreviewState(api.server_read(&["connect", "preview"], &[], 10).await)).await });
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
        self.runtime.spawn(async move { done(ConnectReply::Devices(seq, api.server_read(&["connect", "account-devices"], &[], 10).await)).await });
        cx.notify();
    }

    fn load_local_devices(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let seq = self.connect.local.start();
        let done = self.connect_send_later();
        self.runtime.spawn(async move { done(ConnectReply::LocalDevices(seq, api.server_read(&["connect", "devices"], &[], 10).await)).await });
        cx.notify();
    }

    /// Aprovar aqui dá acesso de dono a esta máquina: pede confirmação, como Tirar acesso.
    fn confirm_local_approve(&mut self, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let machine = self.vault_machine_label(&self.active_key(), cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("connect_local_approve_title").replace("{name}", &name),
            tr("connect_local_approve_body").replace("{machine}", &machine), tr_shared("connect_device_approve", &[]), ButtonVariant::Primary,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.answer_local_device(device.clone(), true, cx));
                true
            });
    }

    fn confirm_local_revoke(&mut self, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let machine = self.vault_machine_label(&self.active_key(), cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_device_revoke_title", &[("name", &name)]),
            tr("connect_local_revoke_body").replace("{machine}", &machine), tr_shared("connect_device_revoke", &[]), ButtonVariant::Danger,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.answer_local_device(device.clone(), false, cx));
                true
            });
    }

    /// `approve` aprova o pendente; sem ele, a máquina o apaga: é o Recusar do pendente e o Tirar acesso do aprovado.
    fn answer_local_device(&mut self, device: String, approve: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.connect.local_answering.is_some() { return; }
        (self.connect.local_answering, self.connect.local_error) = (Some(device.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let result = if approve { api.server_send(reqwest::Method::POST, &["connect", "devices", &device, "approve"], None, 15).await }
                else { api.server_send(reqwest::Method::DELETE, &["connect", "devices", &device], None, 15).await };
            done(ConnectReply::LocalAnswered(result)).await
        });
        cx.notify();
    }

    fn submit_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connect.saving { return; }
        let Some(form) = &self.connect.form else { return };
        let password = take_password(&form.password, window, cx);
        let account = service_name(&form.account.read(cx).value());
        let machine_name = service_name(&form.machine.read(cx).value());
        if account.is_empty() || password.is_empty() || machine_name.is_empty() {
            self.connect.error = Some(tr("connect_fields_required"));
            cx.notify();
            return;
        }
        let Some(api) = self.api.clone() else { return };
        (self.connect.saving, self.connect.error) = (true, None);
        let create_account = self.connect.shown_entry() == Entry::Create;
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let body = json!({"service_url": SERVICE_URL, "account": account, "password": password, "machine_name": machine_name,
                "create_account": create_account});
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

    /// Liberar vale para todas as máquinas da conta: pede confirmação, como Tirar acesso.
    fn confirm_release(&mut self, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_account_device_release_title", &[("name", &name)]),
            tr_shared("connect_account_device_release_body", &[]), tr_shared("connect_account_device_release", &[]), ButtonVariant::Primary,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.answer_device(device.clone(), true, cx));
                true
            });
    }

    fn confirm_revoke(&mut self, device: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_device_revoke_title", &[("name", &name)]), tr_shared("connect_account_device_revoke_body", &[]),
            tr_shared("connect_device_revoke", &[]), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.answer_device(device.clone(), false, cx));
                true
            });
    }

    /// `release` libera o pendente; sem ele, a conta revoga: é o Recusar do pendente e o Tirar acesso do liberado.
    fn answer_device(&mut self, device: String, release: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.connect.answering.is_some() { return; }
        (self.connect.answering, self.connect.device_error) = (Some(device.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let result = if release { api.server_send(reqwest::Method::POST, &["connect", "account-devices", &device, "approve"], None, 15).await }
                else { api.server_send(reqwest::Method::DELETE, &["connect", "account-devices", &device], None, 15).await };
            done(ConnectReply::Answered(result)).await
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
                match self.connect.saved(result) {
                    Some(Phase::Unlinked) => {}
                    Some(_) => self.load_account_machines(cx),
                    // O envio pode ter estourado o prazo com o servidor já ligado: relê em vez de supor.
                    None => { self.load_connect(cx); self.load_account_machines(cx); }
                }
                self.load_connect_devices(cx);
                self.load_local_devices(cx);
            }
            ConnectReply::Machines(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_account);
                if !self.connect.machines_read(seq, result) { return; }
            }
            ConnectReply::Links(result) => if let Some(line) = self.connect.links_read(result) { crate::log_line(&line); },
            ConnectReply::LinkStarted(result) => {
                self.connect.linking = None;
                self.connect.link_error = result.err().map(|error| failure_text(&error));
                self.load_peer_links();
            }
            ConnectReply::Devices(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_devices);
                if !self.connect.devices.finish(seq, result) { return; }
            }
            ConnectReply::Answered(result) => {
                self.connect.answering = None;
                self.connect.device_error = result.err().map(|error| failure_text(&error));
                self.load_connect_devices(cx);
            }
            ConnectReply::LocalDevices(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_local_devices);
                if !self.connect.local.finish(seq, result) { return; }
            }
            ConnectReply::LocalAnswered(result) => {
                self.connect.local_answering = None;
                self.connect.local_error = result.err().map(|error| failure_text(&error));
                self.load_local_devices(cx);
            }
            ConnectReply::PortLink(server, port, result) => {
                self.port_link_done(server, port, result.map_err(|error| failure_text(&error)).and_then(parse_preview));
                self.load_connect_preview();
            }
            ConnectReply::PreviewClosed(result) => {
                self.connect.preview_busy = false;
                match result {
                    Ok(_) => if let Some(server) = self.api.as_ref().map(Api::identity) { self.port_link_closed(&server); },
                    Err(error) => self.connect.preview_error = Some(failure_text(&error)),
                }
                self.load_connect_preview();
            }
            // Acompanhamento: falha de leitura só deixa de mostrar a linha.
            ConnectReply::PreviewState(result) => self.connect.preview_open = result.ok().as_ref()
                .and_then(|value| Some((open_preview_port(value)?, value["preview"]["expires_at"].as_u64()))),
        }
        if self.connect.form.is_none() && self.settings == Some(Page::Connect) {
            let name = machine_label(&self.vault_machine_label(&self.active_key(), cx));
            self.connect.form = Some(LinkForm::new(name, window, cx));
        }
        cx.notify();
    }

    /// Só com a página aberta: ligado e subindo vira pronto, e o aparelho que pede liberação aparece, sem ninguém clicar.
    pub(super) fn connect_tick(&mut self, cx: &mut Context<Self>) {
        if self.settings == Some(Page::Connect) && !self.connect.saving {
            self.load_connect(cx);
            self.load_connect_devices(cx);
            self.load_local_devices(cx);
            self.load_connect_preview();
            // A lista da conta pergunta ao serviço: a cada rodada só quando um pedido de ligar está andando.
            if self.connect.linking_in_progress() { self.load_account_machines(cx); }
            self.load_peer_links();
        }
    }

    /// Pendente: Liberar e Recusar; liberado: Tirar acesso; esta máquina, nenhum botão. O ícone e a legenda dizem se é PC.
    fn render_device_row(&self, device: &Device, this_machine_id: &str, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let machine = device.kind == DeviceKind::Machine;
        let kind = tr(if machine { "connect_kind_machine" } else { "connect_kind_device" });
        let caption = match device.status {
            DeviceStatus::Pending => format!("{kind} · {}",
                tr_shared("connect_account_device_asked", &[("when", &super::side::ago(super::side::now_seconds() - device.created_at))])),
            _ => kind,
        };
        let busy = self.connect.answering.as_deref() == Some(device.device_id.as_str());
        let blocked = self.connect.answering.is_some();
        let button = |action: &str| Button::new(SharedString::from(format!("connect-{action}-{}", device.device_id))).small().flex_shrink_0().disabled(blocked);
        let (id, name) = (device.device_id.clone(), device.name.clone());
        let right = match row_actions(device, this_machine_id) {
            RowActions::This => chip(tr("connect_machine_this"), theme::accent_text(), theme::accent_dim()).into_any_element(),
            RowActions::Revoke => button("revoke").outline().label(tr_shared("connect_device_revoke", &[])).loading(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_revoke(id.clone(), name.clone(), window, cx))).into_any_element(),
            RowActions::Answer => {
                let deny = id.clone();
                div().flex().items_center().gap_2()
                    .child(button("deny").outline().label(tr_shared("connect_device_deny", &[]))
                        .on_click(cx.listener(move |this, _, _, cx| this.answer_device(deny.clone(), false, cx))))
                    .child(button("release").primary().label(tr_shared("connect_account_device_release", &[])).loading(busy)
                        .on_click(cx.listener(move |this, _, window, cx| this.confirm_release(id.clone(), name.clone(), window, cx))))
                    .into_any_element()
            }
        };
        div().flex().items_center().gap_3().px_3().py_2().when(!first, |el| el.border_t_1().border_color(theme::border()))
            .child(chrome::small_icon(if machine { IconName::Monitor } else { IconName::Smartphone }, 16., theme::muted()))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_sm().whitespace_normal().child(device.name.clone()))
                .child(div().text_xs().text_color(if device.status == DeviceStatus::Pending { theme::warning() } else { theme::muted() }).child(caption)))
            .child(right)
            .into_any_element()
    }

    fn render_connect_devices(&self, this_machine_id: &str, cx: &mut Context<Self>) -> Div {
        let section = div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_devices_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_devices_add")))
            .children(self.connect.device_error.clone().map(|error| div().id("connect-device-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)));
        match device_view(&self.connect.devices) {
            DeviceView::Loading => section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))),
            DeviceView::Failed(error) => section
                .child(div().id("connect-devices-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))
                .child(div().child(Button::new("connect-devices-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_connect_devices(cx))))),
            DeviceView::Empty => section.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr_shared("connect_account_devices_empty", &[]))),
            DeviceView::Loaded(devices) => {
                let rows: Vec<AnyElement> = devices.iter().enumerate().map(|(at, device)| self.render_device_row(device, this_machine_id, at == 0, cx)).collect();
                section.child(settings_box().children(rows))
                    .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr_shared("connect_account_devices_help", &[])))
            }
        }
    }

    /// Pendente: Aprovar e Recusar, com o código que o celular mostra; aprovado: Tirar acesso, com quando foi visto.
    fn render_local_row(&self, device: &LocalDevice, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let pending = device.status == LocalStatus::Pending;
        let caption = if pending {
            device.install_code.as_ref().map_or_else(|| tr("connect_local_pending"), |code| tr("connect_local_pending_code").replace("{code}", code))
        } else {
            device.last_seen.map_or_else(|| tr("connect_local_never_seen"),
                |at| tr("connect_local_last_seen").replace("{when}", &super::side::ago(super::side::now_seconds() - at as f64)))
        };
        let busy = self.connect.local_answering.as_deref() == Some(device.device_id.as_str());
        let blocked = self.connect.local_answering.is_some();
        let button = |action: &str| Button::new(SharedString::from(format!("connect-local-{action}-{}", device.device_id))).small().flex_shrink_0().disabled(blocked);
        let (id, name) = (device.device_id.clone(), device.name.clone());
        let right = if pending {
            let deny = id.clone();
            div().flex().items_center().gap_2()
                .child(button("deny").outline().label(tr_shared("connect_device_deny", &[]))
                    .on_click(cx.listener(move |this, _, _, cx| this.answer_local_device(deny.clone(), false, cx))))
                .child(button("approve").primary().label(tr_shared("connect_device_approve", &[])).loading(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm_local_approve(id.clone(), name.clone(), window, cx))))
                .into_any_element()
        } else {
            button("revoke").outline().label(tr_shared("connect_device_revoke", &[])).loading(busy)
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_local_revoke(id.clone(), name.clone(), window, cx))).into_any_element()
        };
        div().flex().items_center().gap_3().px_3().py_2().when(!first, |el| el.border_t_1().border_color(theme::border()))
            .child(chrome::small_icon(IconName::Smartphone, 16., theme::muted()))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_sm().whitespace_normal().child(device.name.clone()))
                .child(div().text_xs().text_color(if pending { theme::warning() } else { theme::muted() }).child(caption)))
            .child(right)
            .into_any_element()
    }

    /// Os aparelhos que entraram só nesta máquina, sem a conta: aprovados aqui e pedidos de app antigo.
    fn render_local_devices(&self, cx: &mut Context<Self>) -> Div {
        let section = div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_local_devices_title")))
            .children(self.connect.local_error.clone().map(|error| div().id("connect-local-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)));
        match device_view(&self.connect.local) {
            DeviceView::Loading => section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))),
            DeviceView::Failed(error) => section
                .child(div().id("connect-local-devices-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))
                .child(div().child(Button::new("connect-local-devices-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_local_devices(cx))))),
            DeviceView::Empty => section.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_local_devices_empty"))),
            DeviceView::Loaded(devices) => {
                let rows: Vec<AnyElement> = devices.iter().enumerate().map(|(at, device)| self.render_local_row(device, at == 0, cx)).collect();
                section.child(settings_box().children(rows))
                    .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr("connect_local_devices_help")))
            }
        }
    }

    /// A abertura de quem nunca ligou: o que o Connect dá e os dois caminhos.
    fn render_connect_hero(&self, cx: &mut Context<Self>) -> Div {
        let perk = |key: &str| div().flex().items_start().gap_2()
            .child(div().pt(px(2.)).child(chrome::small_icon(IconName::Check, 14., theme::success())))
            .child(div().flex_1().min_w_0().text_sm().whitespace_normal().child(tr_shared(key, &[])));
        let width = px(400.);
        settings_box().p_6().items_center().gap_3()
            .child(connect_mark("connect-hero-mark", 72., false, cx.reduce_motion()))
            .child(div().text_2xl().font_weight(FontWeight::BOLD).child(tr_shared("connect_title", &[])))
            .child(div().max_w(width).text_sm().text_color(theme::muted()).text_center().whitespace_normal().child(tr_shared("connect_hero_subtitle", &[])))
            .child(chip(tr_shared("connect_free", &[]), theme::accent_text(), theme::accent_dim()))
            .child(div().w_full().max_w(width).flex().flex_col().gap_2()
                .child(perk("connect_perk_networks")).child(perk("connect_perk_approve")).child(perk("connect_perk_one_account")))
            .child(div().w_full().max_w(px(320.)).pt_2().flex().flex_col().gap_2()
                .child(Button::new("connect-create").primary().w_full().label(tr_shared("connect_create_free", &[]))
                    .on_click(cx.listener(|this, _, _, cx| { (this.connect.entry, this.connect.error) = (Some(Entry::Create), None); cx.notify(); })))
                .child(Button::new("connect-have-account").outline().w_full().label(tr_shared("connect_have_account", &[]))
                    .on_click(cx.listener(|this, _, _, cx| { (this.connect.entry, this.connect.error) = (Some(Entry::Login), None); cx.notify(); }))))
    }

    /// Criar conta ou entrar nela, e já ligar este computador. `relink` é a ligação gravada que parou.
    fn render_connect_form(&self, entry: Entry, relink: bool, cx: &mut Context<Self>) -> Option<Div> {
        let form = self.connect.form.as_ref()?;
        let saving = self.connect.saving;
        let create = entry == Entry::Create;
        let (title, help) = if relink { (tr("connect_stopped"), tr("connect_stopped_help")) }
            else if create { (tr_shared("connect_sign_up", &[]), tr("connect_create_help")) }
            else { (tr("connect_login_title"), tr("connect_link_help")) };
        let hint = |text: String| div().text_xs().text_color(theme::muted()).whitespace_normal().child(text);
        let preview = address_preview(&form.machine.read(cx).value().to_string(), &form.account.read(cx).value().to_string());
        let label = if saving { tr("connect_turning_on") } else if relink { tr("connect_turn_on_again") }
            else if create { tr("connect_create_and_link") } else { tr("connect_login_and_link") };
        Some(settings_box().p_5().gap_3()
            .child(div().flex().items_center().gap_2()
                .child(connect_mark("connect-form-mark", 24., false, true))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title)))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(help))
            .child(div().flex().flex_col().gap_1().child(super::sync::sync_field(tr_shared("connect_account_name", &[]), &form.account, saving))
                .child(hint(tr("connect_address_preview").replace("{address}", &preview))))
            .child(div().flex().flex_col().gap_1().child(super::sync::sync_field(tr("connect_password"), &form.password, saving))
                .when(create, |el| el.child(hint(tr("connect_password_hint")))))
            .child(div().flex().flex_col().gap_1().child(super::sync::sync_field(tr("connect_machine_name"), &form.machine, saving))
                .child(hint(tr("connect_machine_hint"))))
            .child(div().flex().items_center().gap_2().pt_1()
                .child(Button::new("connect-on").primary().label(label).loading(saving).disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_link(window, cx))))
                .when(!relink, |el| el.child(Button::new("connect-back").ghost().label(tr_shared("comum_voltar", &[])).disabled(saving)
                    .on_click(cx.listener(|this, _, _, cx| { (this.connect.entry, this.connect.error) = (None, None); cx.notify(); }))))))
    }

    /// O mesmo nome de máquina num servidor desta lista: pelo endereço do Connect.
    fn in_app_list(&self, host: &str) -> bool {
        saved_with_connect(&self.servers, host) || self.remote.contains_key(&super::servers::norm(&format!("https://{host}")))
    }

    fn render_account_machines(&self, this_host: &str, cx: &mut Context<Self>) -> Div {
        let section = div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_machines_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_machines_help")))
            .children(self.connect.link_error.clone().map(|error| div().id("connect-link-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .children(self.connect.machines_error.clone().map(|error| div().id("connect-machines-stale")
                .text_xs().text_color(theme::muted()).whitespace_normal().child(error)));
        let list = match &self.connect.machines.value {
            None => return section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))),
            Some(Err(error)) => return section
                .child(div().id("connect-machines-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(div().child(Button::new("connect-machines-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_account_machines(cx))))),
            Some(Ok(list)) => list,
        };
        let row = |name: String, host: String, online: bool, right: AnyElement, first: bool| div().flex().items_center().gap_3().px_3().py_2()
            .when(!first, |el| el.border_t_1().border_color(theme::border()))
            .child(div().size(px(8.)).rounded_full().flex_shrink_0().bg(if online { theme::success() } else { theme::faint() }))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).truncate().child(name))
                .child(div().text_xs().text_color(theme::muted()).font_family(theme::MONO).truncate().child(host)))
            .child(div().flex_shrink_0().child(right));
        let tag = |text: String| div().text_xs().text_color(theme::muted()).child(text).into_any_element();
        let this_name = identity_of(this_host).map_or_else(|| list.this.machine_id.clone(), |(machine, _)| machine.to_owned());
        let mut rows = vec![row(this_name, this_host.to_owned(), true, chip(tr("connect_machine_this"), theme::accent_text(), theme::accent_dim()).into_any_element(), true).into_any_element()];
        for machine in &list.machines {
            let link = self.connect.links.iter().find(|link| link.machine_id == machine.machine_id);
            // Ligar entre PCs da conta não pede aprovação: enquanto o pedido anda, o próprio botão mostra a espera.
            let pending = link.is_some_and(|link| matches!(link.stage.as_str(), "requesting" | "waiting_approval"));
            let right = if machine.added || self.in_app_list(&machine.host) || link.is_some_and(|link| link.stage == "linked") {
                tag(tr("connect_machine_added"))
            } else if !machine.online && !pending {
                tag(tr("connect_machine_offline"))
            } else {
                let id = machine.machine_id.clone();
                let failed = link.filter(|link| link.stage == "failed").and_then(|link| link.message.clone());
                div().flex().flex_col().items_end().gap_1()
                    .children(failed.map(|message| div().max_w(px(260.)).text_xs().text_color(theme::danger()).whitespace_normal().child(message)))
                    .child(Button::new(SharedString::from(format!("connect-add-{id}"))).outline().small().label(tr("connect_machine_add"))
                        .loading(pending || self.connect.linking.as_deref() == Some(id.as_str())).disabled(pending || self.connect.linking.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| this.add_account_machine(id.clone(), cx))))
                    .into_any_element()
            };
            rows.push(row(machine.name.clone(), machine.host.clone(), machine.online, right, false).into_any_element());
        }
        let alone = list.machines.is_empty();
        section.child(settings_box().children(rows))
            .when(alone, |el| el.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_machines_alone"))))
    }

    /// O link nasce no "Link de fora" de cada porta, na aba Contexto; aqui ficam a prévia aberta e o fechar.
    /// Parar fica sempre à mão: se a leitura do estado falhar, a prévia aberta continua alcançável.
    fn render_connect_preview(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.connect.form.as_ref()?;
        let busy = self.connect.preview_busy;
        let open = self.connect.preview_open.map(|(port, expires)| {
            let until = expires.and_then(|at| chrono::DateTime::from_timestamp(at as i64, 0))
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string());
            let text = match until {
                Some(time) => tr("connect_preview_open_until").replace("{port}", &port.to_string()).replace("{time}", &time),
                None => tr("connect_preview_open").replace("{port}", &port.to_string()),
            };
            div().id("connect-preview-open").text_sm().font_weight(FontWeight::MEDIUM).whitespace_normal().child(text)
        });
        // O link pedido pela aba Contexto, para esta máquina: o servidor só guarda o hash dele.
        let server = self.api.as_ref().map(Api::identity);
        let link = self.ports.link.as_ref().filter(|link| Some(&link.server) == server.as_ref() && open.is_some()).map(|link| {
            let copied = link.url.clone();
            div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().font_family(theme::MONO).text_sm().truncate().child(link.url.clone()))
                .child(Button::new("connect-preview-copy").outline().small().flex_shrink_0().label(tr("connect_preview_copy"))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))))
        });
        Some(settings_box().p_4().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_preview_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_preview_help")))
            .children(open)
            .children(link)
            .children(self.connect.preview_error.clone().map(|error| div().id("connect-preview-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("connect-preview-stop").outline().small().label(tr("connect_preview_stop")).loading(busy).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.stop_preview(cx))))))
    }

    pub(super) fn render_connect(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Connect.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        let mut page = div().flex().flex_col().gap_4().child(title);
        if self.api.is_none() {
            return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element();
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
        // O erro do último envio aparece em toda fase, inclusive sem formulário aberto.
        if let Some(error) = self.connect.error.clone() {
            page = page.child(div().id("connect-write-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error));
        }
        let (state, saving) = (phase(&status), self.connect.saving);
        let host = status.host.clone().unwrap_or_default();
        // O id do vínculo; antes da lista da conta chegar, o primeiro rótulo do endereço, que o serviço monta com ele.
        let this_machine_id = self.connect.machines.ok().map(|list| list.this.machine_id.clone())
            .or_else(|| identity_of(&host).map(|(machine, _)| machine.to_owned())).unwrap_or_default();
        // Ligado em qualquer fase (parado, subindo, pronto) dá para desligar.
        let off = |cx: &mut Context<Self>| Button::new("connect-off").outline().small().label(tr("connect_off")).disabled(saving)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_unlink(window, cx)));
        match state {
            Phase::Unlinked => page = match self.connect.entry {
                None => page.child(self.render_connect_hero(cx)),
                Some(entry) => page.children(self.render_connect_form(entry, false, cx)),
            },
            Phase::Stopped => page = page.children(self.render_connect_form(Entry::Login, true, cx)).child(div().child(off(cx))),
            Phase::Starting => page = page.child(settings_box().p_6().items_center().gap_2()
                .child(connect_mark("connect-starting-mark", 56., true, cx.reduce_motion()))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("connect_linking_title")))
                .child(div().max_w(px(400.)).text_sm().text_color(theme::muted()).text_center().whitespace_normal().child(tr("connect_linking_help")))
                .child(div().pt_1().child(off(cx)))),
            Phase::Ready => {
                let (machine, account) = identity_of(&host).unwrap_or(("", ""));
                let address = format!("https://{host}");
                page = page.child(settings_box().p_4().gap_2()
                    .child(div().flex().items_center().gap_2()
                        .child(div().size(px(10.)).rounded_full().flex_shrink_0().bg(theme::success()))
                        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("connect_on"))))
                    .child(div().text_sm().text_color(theme::muted()).whitespace_normal()
                        .child(tr("connect_identity").replace("{account}", account).replace("{machine}", machine)))
                    .child(div().font_family(theme::MONO).text_sm().truncate().child(address.clone()))
                    .child(div().flex().items_center().gap_2().pt_1()
                        .child(Button::new("connect-copy").outline().small().label(tr("connect_copy_address"))
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(address.clone()))))
                        .child(off(cx))));
            }
        }
        // A lista é da conta, lida com a chave desta máquina: sem ligação não há conta para perguntar.
        if state != Phase::Unlinked {
            page = page.child(self.render_connect_devices(&this_machine_id, cx)).child(self.render_local_devices(cx));
        }
        if state == Phase::Ready {
            page = page.child(self.render_account_machines(&host, cx)).children(self.render_connect_preview(cx));
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{address_preview, device_view, identity_of, machine_label, open_preview_port, parse_account, parse_devices, parse_links, parse_local_devices, parse_port, parse_preview, parse_status, phase, row_actions, take_password, Device, DeviceKind, DeviceStatus, DeviceView, InputState, LocalStatus, Phase, Remote, RowActions};
    use crate::i18n::tr_shared;
    // O nome explícito vence qualquer `test` trazido por glob.
    use core::prelude::v1::test;
    use gpui::{Context, Entity, IntoElement, Render, TestAppContext, Window, div, prelude::*};
    use serde_json::json;

    /// O que entrou pela conta fica em "Aparelhos da conta"; aqui, só o desta máquina, com o pedido esperando primeiro.
    #[test]
    fn local_devices_leave_out_the_account_ones_and_the_refused() {
        let devices = parse_local_devices(json!([
            {"device_id": "a1", "name": "Pixel", "status": "approved", "last_seen": 10, "created_at": 1},
            {"device_id": "b2", "name": "iPhone da conta", "status": "approved", "last_seen": null, "created_at": 1, "account_device": "D"},
            {"device_id": "c3", "name": "Recusado", "status": "denied", "last_seen": null, "created_at": 1},
            {"device_id": "d4", "name": "App antigo", "status": "pending", "last_seen": null, "created_at": 2, "install_code": "123 456"}])).unwrap();
        let rows: Vec<(&str, LocalStatus, Option<&str>)> = devices.iter()
            .map(|device| (device.device_id.as_str(), device.status, device.install_code.as_deref())).collect();
        assert_eq!(rows, [("d4", LocalStatus::Pending, Some("123 456")), ("a1", LocalStatus::Approved, None)]);
        assert!(parse_local_devices(json!({"devices": []})).is_err());
        let mut list = Remote::default();
        list.set(parse_local_devices(json!([{"device_id": "b2", "name": "x", "status": "approved", "created_at": 1, "account_device": "D"}])));
        assert!(matches!(device_view(&list), DeviceView::Empty), "só aparelhos da conta: a lista desta máquina fica vazia");
    }

    #[test]
    fn machine_saved_with_its_connect_route_is_in_the_list() {
        use crate::api::route::ConnectRoute;
        use crate::app::servers::ServerEntry;
        let saved = |address: &str, connect: Option<&str>| ServerEntry { address: address.into(),
            connect: connect.map(|url| ConnectRoute { url: url.into(), ..ConnectRoute::default() }), ..ServerEntry::default() };
        let host = "casa.jeff.hangar.dev.br";
        // Guardada pelo Tailscale, com o Connect como terceiro caminho: é ela.
        assert!(super::saved_with_connect(&[saved("https://casa.tail1.ts.net", Some("https://Casa.jeff.hangar.dev.br/"))], host));
        assert!(super::saved_with_connect(&[saved("https://casa.jeff.hangar.dev.br", None)], host));
        assert!(!super::saved_with_connect(&[saved("https://casa.tail1.ts.net", Some("https://outra.jeff.hangar.dev.br"))], host));
        assert!(!super::saved_with_connect(&[saved("https://casa.tail1.ts.net", None)], host));
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
        list.finish(seq, parse_devices(json!([{"device_id": "a", "name": "Velho", "status": "revoked", "created_at": 1.0, "revoked_at": 2.0}])));
        assert!(matches!(device_view(&list), DeviceView::Empty), "revogado é lápide: não aparece");
        let seq = list.start();
        list.finish(seq, parse_devices(json!([{"device_id": "b", "name": "Pixel", "status": "approved", "created_at": 1.0}])));
        assert!(matches!(device_view(&list), DeviceView::Loaded(rows) if rows.len() == 1 && rows[0].name == "Pixel"));
        // Releitura em curso não apaga a lista da tela.
        list.start();
        assert!(matches!(device_view(&list), DeviceView::Loaded(_)));
    }

    /// O pedido esperando liberação é o que pede ação: vem antes dos liberados. Estado que o app não conhece fica fora.
    #[test]
    fn account_devices_show_pending_first_and_tell_pcs_from_phones() {
        let devices = parse_devices(json!([
            {"device_id": "a1", "name": "iPhone", "status": "approved", "created_at": 1.0, "approved_at": 2.0},
            {"device_id": "b2", "name": "casa", "kind": "machine", "status": "approved", "created_at": 1.0},
            {"device_id": "c3", "name": "Pixel", "kind": "device", "status": "pending", "created_at": 5.0},
            {"device_id": "d4", "name": "Velho", "status": "revoked", "created_at": 1.0},
            {"device_id": "e5", "name": "Futuro", "status": "frozen", "created_at": 1.0},
            {"device_id": "f6", "name": "Tablet", "kind": "watch", "status": "approved", "created_at": 1.0}])).unwrap();
        let shape: Vec<(&str, DeviceKind, DeviceStatus)> = devices.iter().map(|d| (d.device_id.as_str(), d.kind, d.status)).collect();
        assert_eq!(shape, [("c3", DeviceKind::Device, DeviceStatus::Pending), ("a1", DeviceKind::Device, DeviceStatus::Approved),
            ("b2", DeviceKind::Machine, DeviceStatus::Approved), ("f6", DeviceKind::Device, DeviceStatus::Approved)],
            "sem `kind` (serviço antigo) ou com um desconhecido é aparelho");
        assert!(parse_devices(json!({"devices": []})).is_err());
    }

    /// Esta máquina não se tira da conta pela própria lista: a linha dela não tem botão.
    #[test]
    fn this_machine_row_has_no_actions() {
        let devices = parse_devices(json!([
            {"device_id": "a1", "name": "pc1", "kind": "machine", "status": "approved", "created_at": 1.0},
            {"device_id": "b2", "name": "pc1", "status": "approved", "created_at": 1.0},
            {"device_id": "c3", "name": "casa", "kind": "machine", "status": "pending", "created_at": 1.0},
            {"device_id": "d4", "name": "notebook", "kind": "machine", "status": "approved", "created_at": 1.0}])).unwrap();
        let actions: Vec<(&str, RowActions)> = devices.iter().map(|device| (device.device_id.as_str(), row_actions(device, "pc1"))).collect();
        assert_eq!(actions, [("c3", RowActions::Answer), ("a1", RowActions::This), ("b2", RowActions::Revoke), ("d4", RowActions::Revoke)],
            "esta máquina é a do id do vínculo (pc1), não a do rótulo (notebook); celular com o mesmo nome não é ela");
    }

    #[test]
    fn account_device_refusals_have_their_own_text() {
        let text = |code: &str| super::failure_text(&crate::api::Failure { status: Some(403), detail: "do servidor".into(), retry_after: None,
            uncertain: false, code: Some(code.into()) });
        assert_eq!(text("connect_account_machine_pending"), tr_shared("connect_device_waiting_release", &[]));
        assert_eq!(text("connect_account_device_unknown"), text("connect_account_device_revoked"));
        assert_eq!(text("connect_account_machine_revoked"), text("connect_account_machine_unknown"));
        for code in ["connect_account_device_unknown", "connect_account_machine_unknown"] {
            assert!(!text(code).contains("do servidor"), "{code}: frase própria, nos dois idiomas");
        }
    }

    #[test]
    fn preview_form_reads_the_port_and_the_link() {
        assert_eq!(parse_port(" 3000 "), Some(3000));
        for bad in ["", "0", "65536", "porta"] {
            assert_eq!(parse_port(bad), None, "{bad}");
        }
        assert_eq!(parse_preview(json!({"url": "https://a/?hangar_preview=b"})).as_deref(), Ok("https://a/?hangar_preview=b"));
        assert!(parse_preview(json!({})).is_err());
        assert_eq!(open_preview_port(&json!({"preview": {"port": 3000}})), Some(3000));
        assert_eq!(open_preview_port(&json!({"preview": null})), None);
    }

    #[test]
    fn address_preview_follows_the_typed_names_and_identity_reads_the_host() {
        assert_eq!(address_preview(" Casa ", "jefferson"), "casa.jefferson.hangar.dev.br");
        assert_eq!(address_preview("", ""), "notebook.sua-conta.hangar.dev.br");
        assert_eq!(identity_of("notebook.jeffersonfelizardo.hangar.dev.br"), Some(("notebook", "jeffersonfelizardo")));
        assert_eq!(identity_of(""), None);
    }

    #[test]
    fn machine_name_comes_from_the_shown_label_in_the_service_format() {
        assert_eq!(machine_label("Notebook do Jefferson"), "notebook-do-jefferson");
        assert_eq!(machine_label("  casa--pc  "), "casa-pc", "sem `--`, que é do endereço do convidado");
        assert_eq!(machine_label("192.168.0.5"), "192-168-0-5");
        assert_eq!(machine_label(&"a".repeat(40)).len(), 32);
        assert_eq!(machine_label("é"), "", "nada aproveitável fica vazio para a pessoa digitar");
    }

    #[test]
    fn reconnecting_to_the_same_server_keeps_the_form_and_the_phase() {
        let mut kept = super::Connect { entry: Some(super::Entry::Create), saving: true, preview_busy: true,
            linking: Some("casa".to_owned()), answering: Some("x".to_owned()), ..Default::default() };
        kept.status.set(Ok(parse_status(json!({"enabled": false, "running": false, "host": null, "error": null})).unwrap()));
        let kept = kept.carry_over();
        assert_eq!(kept.entry, Some(super::Entry::Create), "o formulário escolhido continua");
        assert!(kept.status.ok().is_some(), "a fase continua até a nova leitura");
        assert!(!kept.saving && !kept.preview_busy && kept.linking.is_none() && kept.answering.is_none(),
            "resposta da conexão anterior não chega: nada fica travado esperando por ela");
    }

    fn refused(code: &str) -> Result<serde_json::Value, crate::api::Failure> {
        Err(crate::api::Failure { status: Some(502), detail: "x".into(), retry_after: None, uncertain: false, code: Some(code.into()) })
    }

    fn status(enabled: bool, running: bool, host: Option<&str>) -> super::Status {
        parse_status(json!({"enabled": enabled, "running": running, "host": host, "error": null})).unwrap()
    }

    #[test]
    fn turning_on_again_always_signs_in_even_after_choosing_create() {
        let mut connect = super::Connect { entry: Some(super::Entry::Create), ..Default::default() };
        connect.status.set(Ok(status(false, false, Some("mesa.conta.hangar.dev.br"))));
        assert_eq!(connect.shown_entry(), super::Entry::Login, "\"Ligar de novo\" é o formulário de entrar");
        connect.status.set(Ok(status(false, false, None)));
        assert_eq!(connect.shown_entry(), super::Entry::Create);
    }

    #[test]
    fn what_is_sent_is_what_the_preview_shows() {
        assert_eq!(super::service_name("  Casa-01 "), "casa-01");
        assert_eq!(address_preview("Casa-01", "Jefferson"), "casa-01.jefferson.hangar.dev.br");
    }

    #[test]
    fn account_machines_are_polled_only_while_a_link_waits() {
        let mut connect = super::Connect::default();
        let links = |stage: &str| Ok(json!([{"machine_id": "casa", "stage": stage, "peer": null, "code": null, "message": null}]));
        for (stage, polled) in [("requesting", true), ("waiting_approval", true), ("linked", false), ("failed", false)] {
            connect.links_read(links(stage));
            assert_eq!(connect.linking_in_progress(), polled, "{stage}");
        }
    }

    #[test]
    fn a_failed_machines_read_keeps_the_last_good_list() {
        let mut connect = super::Connect::default();
        let good = parse_account(json!({"this": {"machine_id": "a", "host": "a.j.hangar.dev.br"},
            "machines": [{"machine_id": "casa", "name": "casa", "host": "casa.j.hangar.dev.br", "online": true, "added": false}]}));
        let first = connect.machines.start();
        assert!(connect.machines_read(first, good));
        let second = connect.machines.start();
        assert!(connect.machines_read(second, Err("sem rede".into())));
        assert_eq!(connect.machines.ok().map(|list| list.machines.len()), Some(1), "a lista boa fica");
        assert_eq!(connect.machines_error.as_deref(), Some("sem rede"));
        assert!(!connect.machines.loading);

        let mut empty = super::Connect::default();
        let seq = empty.machines.start();
        empty.machines_read(seq, Err("sem rede".into()));
        assert!(matches!(empty.machines.value, Some(Err(_))) && empty.machines_error.is_none(), "sem lista, o erro ocupa o lugar");
    }

    #[test]
    fn account_created_but_not_linked_turns_the_form_into_sign_in() {
        let mut connect = super::Connect { entry: Some(super::Entry::Create), saving: true, ..Default::default() };
        assert_eq!(connect.saved(refused("connect_account_created_link_failed")), None);
        assert_eq!(connect.entry, Some(super::Entry::Login), "criar de novo daria nome já usado");
        assert!(!connect.saving && connect.error.is_some());

        let mut other = super::Connect { entry: Some(super::Entry::Create), ..Default::default() };
        other.saved(refused("connect_account_taken"));
        assert_eq!(other.entry, Some(super::Entry::Create), "outra recusa não troca o formulário");
    }

    #[test]
    fn unlinking_forgets_the_account_machines_and_the_link_requests() {
        let mut connect = super::Connect::default();
        connect.machines.set(parse_account(json!({"this": {"machine_id": "a", "host": "a.j.hangar.dev.br"}, "machines": []})));
        connect.links_read(Ok(json!([{"machine_id": "casa", "stage": "linked", "peer": null, "code": null, "message": null}])));
        let unlinked = json!({"enabled": false, "running": false, "host": null, "error": null});
        assert_eq!(connect.saved(Ok(unlinked)), Some(Phase::Unlinked));
        assert!(connect.machines.ok().is_none() && connect.links.is_empty());
    }

    #[test]
    fn link_requests_log_only_when_reading_starts_failing_or_comes_back() {
        let mut connect = super::Connect::default();
        let ok = || Ok(json!([{"machine_id": "casa", "stage": "waiting_approval", "peer": null, "code": null, "message": null}]));
        assert_eq!(connect.links_read(ok()), None);
        let first = connect.links_read(refused("connect_service_unreachable")).expect("a primeira falha vai ao log");
        assert!(first.contains("connect_links_unreadable"));
        assert_eq!(connect.links_read(refused("connect_service_unreachable")), None, "falha repetida não repete a linha");
        assert_eq!(connect.links.len(), 1, "a tela fica com a última leitura boa");
        assert!(connect.links_read(ok()).is_some_and(|line| line.contains("connect_links_recovered")));
    }

    #[test]
    fn account_list_reads_this_machine_and_the_others() {
        let list = parse_account(json!({"this": {"machine_id": "notebook", "host": "notebook.j.hangar.dev.br"},
            "machines": [{"machine_id": "casa", "name": "casa", "host": "casa.j.hangar.dev.br", "online": false, "added": false}]})).unwrap();
        assert_eq!(list.this.machine_id, "notebook");
        assert_eq!(list.machines[0].name, "casa");
        assert!(!list.machines[0].online && !list.machines[0].added);
        assert!(parse_account(json!({"machines": []})).is_err(), "sem a máquina atual não passa por lista");
        assert_eq!(parse_links(json!([{"machine_id": "casa", "stage": "waiting_approval", "peer": null, "code": null, "message": null}])).unwrap()[0].stage, "waiting_approval");
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
