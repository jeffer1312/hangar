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
/// Sem o login do dono, repetir a cada rodada só enche as falhas recentes; a recusa pode ser de um instante, e
/// quem pede entrada não pode esperar a próxima conexão.
const REFUSED_PAUSE: Duration = Duration::from_secs(5 * 60);

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
pub(super) struct Device {
    device_id: String, name: String, status: DeviceStatus, last_seen: Option<u64>,
    /// Mesmo valor nos pedidos de um celular a várias máquinas; servidor antigo não manda.
    #[serde(default)] install_id: Option<String>,
    #[serde(default)] created_at: Option<u64>,
}

/// O que o celular mostra e o cartão repete, para o dono conferir que é o mesmo aparelho antes de aprovar. Cópia de
/// `installCode` em `packages/core` (o nativo não o importa), conferida pelos mesmos casos. Sai do SHA-256 do id, não
/// de um pedaço dele: quem pede escolhe o id e copiaria um prefixo.
/// shortcut: são 30 bits; quem tem a senha da conta e viu o código ainda acha outro id com o mesmo código por força
/// bruta (~2^30 hashes). Por isso o cartão mostra também o nome e há quanto tempo o pedido chegou.
fn install_code(install_id: &str) -> String {
    // Sem 0/O e 1/I: a pessoa compara de olho, numa tela e na outra.
    const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let digest = ring::digest::digest(&ring::digest::SHA256, install_id.as_bytes());
    let head = u32::from_be_bytes(digest.as_ref()[..4].try_into().expect("o SHA-256 tem 32 bytes")) >> 2;
    let code: String = (0..6).map(|i| ALPHABET[((head >> (25 - 5 * i)) & 31) as usize] as char).collect();
    format!("{}-{}", &code[..3], &code[3..])
}

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
        Some("connect_account_taken") => tr_shared("connect_error_name_taken", &[]),
        Some("connect_account_created_link_failed") => tr("connect_error_account_created"),
        Some("connect_preview_port_refused") => tr("connect_preview_port_refused"),
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
fn connect_mark(id: &str, size: f32, pulse: bool, reduce: bool) -> AnyElement {
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

fn parse_port(text: &str) -> Option<u16> {
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

struct LinkForm { account: Entity<InputState>, password: Entity<InputState>, machine: Entity<InputState>, preview_port: Entity<InputState>, _subscriptions: [Subscription; 4] }

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
        let preview_port = cx.new(|cx| InputState::new(window, cx));
        let _subscriptions = [submit_on_enter(&password, window, cx), submit_on_enter(&machine, window, cx),
            redraw_on_change(&account, cx), redraw_on_change(&machine, cx)];
        Self { account, password, machine, preview_port, _subscriptions }
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
    /// A porta da prévia aberta no servidor: o link pode ter saído da tela, a prévia não.
    preview_open: Option<u64>,
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
        (self.saving, self.preview_busy, self.linking, self.revoking) = (false, false, None, None);
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
    Revoked(Result<Value, Failure>),
    PreviewOpened(Result<Value, Failure>),
    PreviewClosed(Result<Value, Failure>),
    PreviewState(Result<Value, Failure>),
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

/// Um cartão por celular: pedidos com o mesmo `install_id` e o mesmo nome em várias máquinas se juntam; sem ele, cada pedido
/// fica no seu. O nome é de quem pede: nome diferente nunca junta, senão o dono aprovaria um aparelho que não viu.
/// Uma caixa por máquina, a do pedido mais novo; os anteriores saem pela `Notices::older_twins`.
fn request_groups<'a>(machines: &[(&'a str, &'a [Request])]) -> Vec<Vec<(&'a str, &'a Request)>> {
    let mut groups: Vec<Vec<(&str, &Request)>> = Vec::new();
    for &(machine, cards) in machines {
        for card in cards {
            let same = card.device.install_id.as_deref().and_then(|id| groups.iter()
                .position(|group| group[0].1.device.install_id.as_deref() == Some(id) && group[0].1.device.name == card.device.name));
            let Some(at) = same else {
                groups.push(vec![(machine, card)]);
                continue;
            };
            match groups[at].iter_mut().find(|(at_machine, _)| *at_machine == machine) {
                Some(slot) if slot.1.device.created_at <= card.device.created_at => slot.1 = card,
                Some(_) => {}
                None => groups[at].push((machine, card)),
            }
        }
    }
    groups
}

/// Por máquina, chave `servers::norm`: sobrevive à troca do ativo, ao contrário de `Connect`.
#[derive(Default)]
pub(in crate::app) struct Notices {
    pending: HashMap<String, Vec<Request>>, refused: HashMap<String, Instant>,
    /// Máquinas desmarcadas no cartão agrupado, por (`group_key`, máquina): nova tentativa do celular troca o pedido, não
    /// a escolha. O padrão é todas marcadas.
    unchecked: HashSet<(String, String)>,
    /// Por cartão (`group_key`): as máquinas escolhidas que não foram aprovadas porque o pedido sumiu da lista. Fica fora
    /// do cartão até a pessoa dispensar: o cartão pode sumir junto com o pedido.
    skipped: HashMap<String, String>,
    /// Por (máquina, pedido) em resposta: as tentativas anteriores a recusar quando a resposta der certo.
    then_deny: HashMap<(String, String), Vec<String>>,
}

fn group_key(device: &Device) -> Option<String> {
    Some(format!("{}\n{}", device.install_id.as_deref()?, device.name))
}

impl Notices {
    /// Depois de ler a lista de `machine`: as marcas de pedido que ela não tem mais saem. As das outras máquinas ficam,
    /// mesmo fora do ar ou recusando, para a escolha valer quando a lista voltar.
    fn forget_answered(&mut self, machine: &str) {
        let cards = self.pending.get(machine).map(Vec::as_slice).unwrap_or_default();
        self.unchecked.retain(|(key, at)| at != machine || cards.iter().any(|card| group_key(&card.device).as_deref() == Some(key.as_str())));
    }

    /// As tentativas anteriores a recusar agora: só se a resposta do pedido deu certo.
    fn after_answer(&mut self, machine: &str, device: &str, ok: bool) -> Vec<String> {
        let twins = self.then_deny.remove(&(machine.to_owned(), device.to_owned())).unwrap_or_default();
        if ok { twins } else { Vec::new() }
    }

    /// As máquinas escolhidas cujo pedido saiu da lista desde que o cartão foi desenhado (recusou a lista, saiu do ar
    /// ou alguém respondeu em outro lugar): o diálogo de confirmar não vê essa mudança.
    fn gone<'a>(&self, chosen: &'a [(String, String, String)]) -> Vec<&'a (String, String, String)> {
        chosen.iter().filter(|(machine, device, _)| !self.pending.get(machine).is_some_and(|cards| cards.iter().any(|card| card.device.device_id == *device)))
            .collect()
    }

    /// Os pedidos anteriores do mesmo celular (mesmo `install_id` e nome) nesta máquina, fora do cartão: saem junto com
    /// a resposta, senão voltariam como cartão novo logo depois. Um mais novo que chegou com o diálogo aberto fica:
    /// ninguém o viu.
    fn older_twins(&self, machine: &str, device: &str) -> Vec<String> {
        let cards = self.pending.get(machine).map(Vec::as_slice).unwrap_or_default();
        let Some(this) = cards.iter().find(|card| card.device.device_id == device) else { return Vec::new() };
        let Some(key) = group_key(&this.device) else { return Vec::new() };
        cards.iter().filter(|card| card.device.device_id != device && group_key(&card.device).as_deref() == Some(key.as_str())
            && card.device.created_at <= this.device.created_at).map(|card| card.device.device_id.clone()).collect()
    }

    fn card(&mut self, machine: &str, device: &str) -> Option<&mut Request> {
        self.pending.get_mut(machine)?.iter_mut().find(|card| card.device.device_id == device)
    }

    fn paused(&self, machine: &str, now: Instant) -> bool {
        self.refused.get(machine).is_some_and(|since| now.saturating_duration_since(*since) < REFUSED_PAUSE)
    }
}

/// Fora de `ConnectReply` porque cada máquina responde por si: a troca do ativo não descarta a resposta.
pub(super) enum NoticeReply {
    Listed(String, Result<Value, Failure>),
    Answered { machine: String, device: String, result: Result<Value, Failure> },
}

/// Dados simples porque a animação da novidade (`news::connect_approval`) desenha o mesmo cartão; lá, `focusable`
/// é falso: a miniatura não entra no Tab.
pub(crate) fn device_card(id: &str, name: &str, machine: &str, sending: bool, error: Option<String>, focusable: bool,
    approve: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, deny: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    let button = |action: &str, key: &str| Button::new(SharedString::from(format!("connect-{action}-{id}"))).xsmall().w_full()
        .label(tr_shared(key, &[])).disabled(sending).tab_stop(focusable);
    div().id(SharedString::from(format!("connect-request-{id}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.)).bg(theme::inset())
        .flex().flex_col().gap_1().role(Role::Group).aria_label(name.to_owned())
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning()).whitespace_normal()
            .child(tr_shared("connect_device_pending", &[("machine", machine)])))
        .child(div().text_sm().text_color(theme::text()).whitespace_normal().child(name.to_owned()))
        .child(request_pair(button("approve", "connect_device_approve").primary().on_click(approve),
            button("deny", "connect_device_deny").outline().on_click(deny)))
        .children(error.map(|error| div().id(SharedString::from(format!("connect-request-error-{id}"))).role(Role::Alert)
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
        self.runtime.spawn(async move { done(ConnectReply::Devices(seq, api.server_read(&["connect", "devices"], &[], 10).await)).await });
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
            // A primeira prévia reinicia o `frpc` e o Caddy e espera o site abrir de fora: prazo longo.
            let body = json!({"port": port});
            let mut result = api.server_send(reqwest::Method::POST, &["connect", "preview"], Some(body.clone()), 90).await;
            // Quem pede pelo próprio Connect perde a conexão na troca dos filhos.
            if result.as_ref().is_err_and(|error| error.status.is_none()) {
                tokio::time::sleep(Duration::from_secs(10)).await;
                result = api.server_send(reqwest::Method::POST, &["connect", "preview"], Some(body), 90).await;
            }
            done(ConnectReply::PreviewOpened(result)).await
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
                match self.connect.saved(result) {
                    Some(Phase::Unlinked) => {}
                    Some(_) => self.load_account_machines(cx),
                    // O envio pode ter estourado o prazo com o servidor já ligado: relê em vez de supor.
                    None => { self.load_connect(cx); self.load_account_machines(cx); }
                }
                self.load_connect_devices(cx);
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
                self.load_connect_preview();
            }
            ConnectReply::PreviewClosed(result) => {
                self.connect.preview_busy = false;
                match result {
                    Ok(_) => self.connect.preview_url = None,
                    Err(error) => self.connect.preview_error = Some(failure_text(&error)),
                }
                self.load_connect_preview();
            }
            // Acompanhamento: falha de leitura só deixa de mostrar a linha.
            ConnectReply::PreviewState(result) => self.connect.preview_open = result.ok().as_ref().and_then(open_preview_port),
        }
        if self.connect.form.is_none() && self.settings == Some(Page::Connect) {
            let name = machine_label(&self.vault_machine_label(&self.active_key(), cx));
            self.connect.form = Some(LinkForm::new(name, window, cx));
        }
        cx.notify();
    }

    /// Devolve se os cartões da máquina mudaram.
    fn set_pending(&mut self, machine: &str, devices: &[Device]) -> bool {
        let old = self.connect_notices.pending.remove(machine).unwrap_or_default();
        let cards = merge(&old, devices);
        let changed = cards != old;
        if !cards.is_empty() { self.connect_notices.pending.insert(machine.to_owned(), cards); }
        self.connect_notices.forget_answered(machine);
        changed
    }

    /// Máquinas que o cartão acompanha. Convite não é do dono, e a rota de aparelhos recusaria a cada rodada.
    fn connect_machines(&self) -> Vec<(String, Api)> {
        let mut machines: Vec<(String, Api)> = self.remote.iter().filter(|(_, list)| list.online)
            .filter_map(|(key, list)| Some((key.clone(), list.api.clone()?))).collect();
        machines.extend(self.api.clone().map(|api| (self.active_key(), api)));
        let now = Instant::now();
        machines.retain(|(key, _)| !self.connect_notices.paused(key, now) && !self.server_entry(key).is_some_and(|entry| entry.invite));
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
            self.load_connect_preview();
            // A lista da conta pergunta ao serviço: a cada rodada só quando um pedido de ligar está andando.
            if self.connect.linking_in_progress() { self.load_account_machines(cx); }
            self.load_peer_links();
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
                Err(error) if matches!(error.status, Some(401 | 403)) => {
                    crate::log_line(&format!("connect: máquina recusou a lista de aparelhos ({:?}); nova tentativa em {} s", error.status, REFUSED_PAUSE.as_secs()));
                    self.connect_notices.refused.insert(machine.clone(), Instant::now());
                    if self.connect_notices.pending.remove(&machine).is_some() { cx.notify(); }
                }
                // Acompanhamento: a falha fica nas recentes do servidor, sem aviso na tela.
                Err(_) => {}
            },
            NoticeReply::Answered { machine, device, result } => {
                let twins = self.connect_notices.after_answer(&machine, &device, result.is_ok());
                match result {
                    Ok(_) => if let Some(cards) = self.connect_notices.pending.get_mut(&machine) { cards.retain(|card| card.device.device_id != device); },
                    Err(error) => if let Some(card) = self.connect_notices.card(&machine, &device) { (card.sending, card.error) = (false, Some(failure_text(&error))); },
                }
                for twin in twins { self.answer_device(machine.clone(), twin, false, cx); }
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
        let lists: Vec<(&str, &[Request])> = keys.iter().map(|key| (key.as_str(), self.connect_notices.pending[*key].as_slice())).collect();
        let mut cards = Vec::new();
        for group in request_groups(&lists) {
            // Pedido com `install_id` vem de celular que mostra o código: o cartão repete o código mesmo numa máquina só.
            if group[0].1.device.install_id.is_some() {
                cards.push(self.connect_group_card(&group, cx));
                continue;
            }
            let [(key, card)] = group[..] else { unreachable!("pedido sem install_id não se junta a outro") };
            let label = self.vault_machine_label(key, cx);
            let (machine, device, name) = (key.to_owned(), card.device.device_id.clone(), card.device.name.clone());
            let approve = cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_approve(machine.clone(), device.clone(), name.clone(), window, cx));
            let (machine, device) = (key.to_owned(), card.device.device_id.clone());
            let deny = cx.listener(move |this, _: &ClickEvent, _, cx| this.answer_device(machine.clone(), device.clone(), false, cx));
            cards.push(device_card(&card.device.device_id, &card.device.name, &label, card.sending, card.error.clone(), true, approve, deny).into_any_element());
        }
        // Só na lista geral: o aviso vale para o cartão inteiro, não para uma máquina.
        let mut skipped: Vec<(&String, &String)> = self.connect_notices.skipped.iter().filter(|_| only.is_none()).collect();
        skipped.sort();
        for (i, (key, text)) in skipped.into_iter().enumerate() {
            let key = key.clone();
            cards.push(div().id(SharedString::from(format!("connect-skipped-{i}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.))
                .bg(theme::inset()).flex().flex_col().gap_1().role(Role::Alert)
                .child(div().text_xs().text_color(theme::danger()).whitespace_normal().child(text.clone()))
                .child(Button::new(SharedString::from(format!("connect-skipped-dismiss-{i}"))).xsmall().outline()
                    .label(tr_shared("connect_notice_dismiss", &[]))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.connect_notices.skipped.remove(&key);
                        cx.notify();
                    })))
                .into_any_element());
        }
        cards
    }

    /// O mesmo celular pedindo entrada em várias máquinas: uma marca por máquina, cada aprovação vai à rota da máquina
    /// com a credencial que este app já tem nela. Máquina que este app não alcança não entra aqui: o pedido aparece nela.
    fn connect_group_card(&self, group: &[(&str, &Request)], cx: &mut Context<Self>) -> AnyElement {
        let first = &group[0].1.device;
        let name = first.name.clone();
        // Dois cartões podem ter o mesmo `install_id` com nomes diferentes: a identidade do elemento é o primeiro pedido.
        let id = first.device_id.clone();
        let sending = group.iter().any(|(_, card)| card.sending);
        let targets: Vec<(String, String, String)> = group.iter()
            .map(|(machine, card)| ((*machine).to_owned(), card.device.device_id.clone(), self.vault_machine_label(machine, cx))).collect();
        let key = group_key(first).unwrap_or_default();
        let marked = |machine: &str| !self.connect_notices.unchecked.contains(&(key.clone(), machine.to_owned()));
        let chosen: Vec<(String, String, String)> = targets.iter().filter(|(machine, _, _)| marked(machine)).cloned().collect();
        let machines: HashSet<&str> = group.iter().map(|(machine, _)| *machine).collect();
        let title = if machines.len() == 1 { tr_shared("connect_group_title_one", &[("name", &name), ("machine", &targets[0].2)]) }
            else { tr_shared("connect_group_title", &[("name", &name), ("count", &machines.len().to_string())]) };
        let code = first.install_id.as_deref().map(|install| tr_shared("connect_group_code", &[("code", &install_code(install))]));
        let asked = group.iter().filter_map(|(_, card)| card.device.created_at).min()
            .map(|at| tr_shared("connect_group_asked", &[("when", &super::side::ago(super::side::now_seconds() - at as f64))]));
        let mut body = div().id(SharedString::from(format!("connect-group-{id}"))).flex_shrink_0().mx(px(8.)).my(px(4.)).p_2().rounded(px(8.))
            .bg(theme::inset()).flex().flex_col().gap_1().role(Role::Group).aria_label(name.clone())
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).whitespace_normal().child(title))
            .child(div().text_xs().text_color(theme::text()).whitespace_normal()
                .child([code, asked].into_iter().flatten().collect::<Vec<_>>().join(" · ")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("connect_group_help", &[])));
        for (i, (machine, _, label)) in targets.iter().enumerate().filter(|_| targets.len() > 1) {
            let mark = (key.clone(), machine.clone());
            body = body.child(Checkbox::new(SharedString::from(format!("connect-group-{id}-{i}"))).label(label.clone())
                .checked(marked(machine)).disabled(sending)
                .on_click(cx.listener(move |this, _: &bool, _, cx| {
                    if !this.connect_notices.unchecked.remove(&mark) { this.connect_notices.unchecked.insert(mark.clone()); }
                    cx.notify();
                })));
        }
        let button = |action: &str, key: &str| Button::new(SharedString::from(format!("connect-group-{action}-{id}"))).xsmall().w_full()
            .label(tr_shared(key, &[])).disabled(sending);
        let approve = {
            let (chosen, name, key) = (chosen.clone(), name.clone(), key.clone());
            cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_group_approve(key.clone(), chosen.clone(), name.clone(), window, cx))
        };
        let deny = cx.listener(move |this, _: &ClickEvent, _, cx| {
            for (machine, device, _) in &targets { this.answer_with_twins(machine, device, false, cx); }
        });
        let failed = group.iter()
            .filter_map(|(machine, card)| Some((card.device.device_id.clone(), self.vault_machine_label(machine, cx), card.error.clone()?))).collect::<Vec<_>>();
        body.child(request_pair(button("approve", "connect_group_approve").primary().disabled(sending || chosen.is_empty()).on_click(approve),
                button("deny", "connect_group_deny").outline().on_click(deny)))
            .children(failed.into_iter().map(|(device, label, error)| div().id(SharedString::from(format!("connect-request-error-{device}")))
                .role(Role::Alert).text_xs().text_color(theme::danger()).whitespace_normal()
                .child(tr_shared("connect_group_failed", &[("machines", &label), ("error", &error)]))))
            .into_any_element()
    }

    fn confirm_group_approve(&mut self, key: String, chosen: Vec<(String, String, String)>, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let labels = chosen.iter().map(|(_, _, label)| label.as_str()).collect::<Vec<_>>().join(", ");
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("connect_device_approve_title", &[("name", &name)]), tr_shared("connect_device_approve_body", &[("machine", &labels)]),
            tr_shared("connect_group_approve", &[]), ButtonVariant::Primary, move |_, cx| {
                let _ = this.update(cx, |this, cx| {
                    let gone: Vec<(String, String, String)> = this.connect_notices.gone(&chosen).into_iter().cloned().collect();
                    if !gone.is_empty() {
                        let labels = gone.iter().map(|(_, _, label)| label.as_str()).collect::<Vec<_>>().join(", ");
                        let text = tr_shared("connect_group_failed", &[("machines", &labels), ("error", &tr_shared("connect_group_unavailable", &[]))]);
                        this.connect_notices.skipped.insert(key.clone(), text);
                    }
                    for target in chosen.iter().filter(|target| !gone.contains(target)) {
                        this.answer_with_twins(&target.0, &target.1, true, cx);
                    }
                    cx.notify();
                });
                true
            });
    }

    /// Responde o pedido; as tentativas anteriores do mesmo celular na mesma máquina são recusadas quando a resposta dele
    /// der certo (`receive_connect_notice`).
    fn answer_with_twins(&mut self, machine: &str, device: &str, approve: bool, cx: &mut Context<Self>) {
        let twins = self.connect_notices.older_twins(machine, device);
        if !twins.is_empty() { self.connect_notices.then_deny.insert((machine.to_owned(), device.to_owned()), twins); }
        self.answer_device(machine.to_owned(), device.to_owned(), approve, cx);
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

    fn render_connect_devices(&self, this_machine: &str, cx: &mut Context<Self>) -> Div {
        let key = self.active_key();
        let section = div().flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_devices_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_devices_add").replace("{machine}", this_machine)))
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
                    .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr_shared("connect_devices_help", &[])))
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
        let address = super::servers::norm(&format!("https://{host}"));
        self.servers.iter().any(|entry| super::servers::norm(&entry.address) == address) || self.remote.contains_key(&address)
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
            let right = if machine.added || self.in_app_list(&machine.host) || link.is_some_and(|link| link.stage == "linked") {
                tag(tr("connect_machine_added"))
            } else if link.is_some_and(|link| matches!(link.stage.as_str(), "requesting" | "waiting_approval")) {
                tag(tr("connect_machine_asking").replace("{name}", &machine.name))
            } else if !machine.online {
                tag(tr("connect_machine_offline"))
            } else {
                let id = machine.machine_id.clone();
                let failed = link.filter(|link| link.stage == "failed").and_then(|link| link.message.clone());
                div().flex().flex_col().items_end().gap_1()
                    .children(failed.map(|message| div().max_w(px(260.)).text_xs().text_color(theme::danger()).whitespace_normal().child(message)))
                    .child(Button::new(SharedString::from(format!("connect-add-{id}"))).outline().small().label(tr("connect_machine_add"))
                        .loading(self.connect.linking.as_deref() == Some(id.as_str())).disabled(self.connect.linking.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| this.add_account_machine(id.clone(), cx))))
                    .into_any_element()
            };
            rows.push(row(machine.name.clone(), machine.host.clone(), machine.online, right, false).into_any_element());
        }
        let alone = list.machines.is_empty();
        section.child(settings_box().children(rows))
            .when(alone, |el| el.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_machines_alone"))))
    }

    /// Parar fica sempre à mão: se a leitura do estado falhar, a prévia aberta continua alcançável.
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
        Some(settings_box().p_4().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_preview_title")))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_preview_help")))
            .children(self.connect.preview_open.map(|port| div().id("connect-preview-open").text_sm().font_weight(FontWeight::MEDIUM)
                .whitespace_normal().child(tr("connect_preview_open").replace("{port}", &port.to_string()))))
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
        let this_machine = identity_of(&host).map_or_else(|| self.vault_machine_label(&self.active_key(), cx), |(machine, _)| machine.to_owned());
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
        // Desligar guarda as credenciais dos aparelhos: quem ficou com uma ainda pode ser revogado.
        if state != Phase::Unlinked || self.connect.devices.ok().is_some_and(|devices| !devices.is_empty()) {
            page = page.child(self.render_connect_devices(&this_machine, cx));
        }
        if state == Phase::Ready {
            page = page.child(self.render_account_machines(&host, cx)).children(self.render_connect_preview(cx));
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{address_preview, device_card, device_view, group_key, identity_of, machine_label, merge, open_preview_port, parse_account, parse_devices, parse_links, parse_port, parse_preview, parse_status, phase, install_code, request_groups, take_password, Device, DeviceStatus, DeviceView, InputState, Notices, Phase, Remote, Request, REFUSED_PAUSE};
    use crate::i18n::tr_shared;
    // O nome explícito vence qualquer `test` trazido por glob.
    use core::prelude::v1::test;
    use gpui::{Context, Entity, IntoElement, Render, TestAppContext, Window, div, prelude::*};
    use serde_json::json;

    fn device(id: &str, status: DeviceStatus) -> Device {
        Device { device_id: id.to_owned(), name: format!("Pixel {id}"), status, last_seen: None, install_id: None, created_at: None }
    }

    #[test]
    fn refused_machine_is_read_again_after_the_pause() {
        let start = std::time::Instant::now();
        let mut notices = Notices::default();
        notices.refused.insert("m".to_owned(), start);
        assert!(notices.paused("m", start + REFUSED_PAUSE / 2));
        assert!(!notices.paused("m", start + REFUSED_PAUSE));
        assert!(!notices.paused("outra", start));
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
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let card = &self.card;
            device_card(&card.device.device_id, &card.device.name, "mesa", card.sending, card.error.clone(), true, |_, _, _| {}, |_, _, _| {})
        }
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
    fn one_phone_on_several_machines_becomes_one_card_and_old_phones_keep_theirs() {
        let phone = |id: &str, install: Option<&str>| Request {
            device: Device { install_id: install.map(str::to_owned), name: "iPhone".to_owned(), ..device(id, DeviceStatus::Pending) }, sending: false, error: None };
        let casa = [phone("a", Some("x1")), phone("b", None)];
        let mesa = [phone("c", Some("x1")), phone("d", Some("y2"))];
        let groups = request_groups(&[("casa", &casa[..]), ("mesa", &mesa[..])]);
        let shape: Vec<Vec<(&str, &str)>> = groups.iter()
            .map(|group| group.iter().map(|(machine, card)| (*machine, card.device.device_id.as_str())).collect()).collect();
        assert_eq!(shape, vec![vec![("casa", "a"), ("mesa", "c")], vec![("casa", "b")], vec![("mesa", "d")]]);
        let parsed = parse_devices(json!([{"device_id": "e", "name": "iPhone", "status": "pending", "last_seen": null, "install_id": "x1", "created_at": 42}]))
            .unwrap();
        assert_eq!((parsed[0].install_id.as_deref(), parsed[0].created_at), (Some("x1"), Some(42)));
    }

    /// Os mesmos casos do teste do `packages/core`: o celular e o Expo mostram estes códigos.
    #[test]
    fn check_code_matches_the_phone() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../../packages/core/src/installCode.fixtures.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(install_code(case["install_id"].as_str().unwrap()), case["code"].as_str().unwrap(), "{case}");
        }
    }

    /// Nova tentativa do mesmo celular antes de expirar: uma caixa por máquina, a do pedido mais novo; a anterior é
    /// recusada junto com a resposta. Sem `install_id` não há como saber que é o mesmo celular: cada pedido fica no seu.
    #[test]
    fn a_retry_on_the_same_machine_keeps_one_box_with_the_newest_request() {
        let phone = |id: &str, install: Option<&str>, at: u64| Request { device: Device { install_id: install.map(str::to_owned),
            name: "iPhone".to_owned(), created_at: Some(at), ..device(id, DeviceStatus::Pending) }, sending: false, error: None };
        let casa = [phone("old", Some("x1"), 100), phone("new", Some("x1"), 200), phone("l1", None, 100), phone("l2", None, 100)];
        let groups = request_groups(&[("casa", &casa[..])]);
        let shape: Vec<Vec<&str>> = groups.iter().map(|group| group.iter().map(|(_, card)| card.device.device_id.as_str()).collect()).collect();
        assert_eq!(shape, vec![vec!["new"], vec!["l1"], vec!["l2"]]);
        let mut notices = Notices::default();
        // `newer` chegou com o diálogo aberto: quem aprovou não o viu, ele não sai junto.
        notices.pending.insert("casa".to_owned(), [&casa[..], &[phone("newer", Some("x1"), 300)]].concat());
        assert_eq!(notices.older_twins("casa", "new"), ["old"]);
        assert!(notices.older_twins("casa", "l1").is_empty());
    }

    /// As tentativas anteriores só são recusadas depois que a resposta do pedido atual deu certo.
    #[test]
    fn older_requests_are_denied_only_after_the_answer_succeeds() {
        let mut notices = Notices::default();
        let pair = ("casa".to_owned(), "new".to_owned());
        notices.then_deny.insert(pair.clone(), vec!["old".to_owned()]);
        assert!(notices.after_answer("casa", "new", false).is_empty());
        assert!(notices.then_deny.is_empty(), "falhou: as anteriores ficam e não são lembradas");
        notices.then_deny.insert(pair, vec!["old".to_owned()]);
        assert_eq!(notices.after_answer("casa", "new", true), ["old"]);
    }

    /// O nome é de quem pede: com o mesmo id e nome diferente, o dono veria um nome só aprovando dois aparelhos.
    #[test]
    fn requests_with_the_same_install_id_but_another_name_stay_apart() {
        let phone = |id: &str, name: &str| Request {
            device: Device { install_id: Some("x1".to_owned()), name: name.to_owned(), ..device(id, DeviceStatus::Pending) }, sending: false, error: None };
        let (casa, mesa) = ([phone("a", "iPhone da Ana")], [phone("b", "iPhone do Bruno")]);
        assert_eq!(request_groups(&[("casa", &casa[..]), ("mesa", &mesa[..])]).len(), 2);
    }

    #[test]
    fn a_machine_that_dropped_the_request_while_confirming_is_reported_not_skipped_silently() {
        let mut notices = Notices::default();
        notices.pending.insert("casa".to_owned(), merge(&[], &[device("a", DeviceStatus::Pending)]));
        let chosen = [("casa", "a", "Casa"), ("mesa", "b", "Mesa")].map(|(m, d, l)| (m.to_owned(), d.to_owned(), l.to_owned()));
        assert_eq!(notices.gone(&chosen).iter().map(|(_, _, label)| label.as_str()).collect::<Vec<_>>(), ["Mesa"],
            "a mesa recusou a lista (401/403) com o diálogo aberto");
    }

    /// A marca é da máquina dentro do cartão do celular: nova tentativa troca o pedido, não a escolha. Só a lista lida
    /// apaga marca; máquina fora do ar ou recusando (401/403) guarda a escolha para quando voltar.
    #[test]
    fn an_unchecked_machine_stays_unchecked_when_the_phone_retries_there() {
        let phone = |id: &str| Request { device: Device { install_id: Some("x1".to_owned()), name: "iPhone".to_owned(),
            ..device(id, DeviceStatus::Pending) }, sending: false, error: None };
        let key = group_key(&phone("a").device).unwrap();
        let mut notices = Notices::default();
        notices.unchecked.extend([(key.clone(), "casa".to_owned()), (key.clone(), "mesa".to_owned()), ("outro\nPixel".to_owned(), "casa".to_owned())]);
        notices.pending.insert("casa".to_owned(), vec![phone("retry")]);
        notices.forget_answered("casa");
        let mut kept: Vec<_> = notices.unchecked.into_iter().collect();
        kept.sort();
        assert_eq!(kept, [(key.clone(), "casa".to_owned()), (key, "mesa".to_owned())]);
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
            linking: Some("casa".to_owned()), revoking: Some("x".to_owned()), ..Default::default() };
        kept.status.set(Ok(parse_status(json!({"enabled": false, "running": false, "host": null, "error": null})).unwrap()));
        let kept = kept.carry_over();
        assert_eq!(kept.entry, Some(super::Entry::Create), "o formulário escolhido continua");
        assert!(kept.status.ok().is_some(), "a fase continua até a nova leitura");
        assert!(!kept.saving && !kept.preview_busy && kept.linking.is_none() && kept.revoking.is_none(),
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
