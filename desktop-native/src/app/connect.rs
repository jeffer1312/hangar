//! Hangar Connect deste servidor: a ligação com a conta, o endereço público, os computadores e os celulares da conta.
//! Celular novo é liberado uma vez na conta, pela faixa do pedido no topo, e passa a entrar em todos os computadores dela.
//! O que vale só nesta máquina fica recolhido em "Acessos só deste computador".
use super::*;
use super::device::Remote;
use super::server_config::chip;
use super::settings::{settings_box, section_head, Disclosure, Page};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};

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
pub(super) struct Device {
    device_id: String, name: String, status: DeviceStatus, #[serde(default)] kind: DeviceKind, created_at: f64,
    #[serde(default)] approved_at: Option<f64>,
    /// O código que o aparelho mostra no pedido; serviço antigo não manda.
    #[serde(default)] install_code: Option<String>,
}

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
    /// Entrou pela conta: aparece em "Celulares com acesso", não aqui.
    #[serde(default)] account_device: Option<String>,
    #[serde(default)] created_at: Option<u64>,
    /// O computador a quem esta máquina deu a chave ao adicioná-lo; sem ele, é um app.
    #[serde(default)] peer: Option<String>,
    /// O servidor leu o peers.json: só aí a falta de `peer` prova que é app. Servidor antigo não manda.
    #[serde(default)] peers_checked: bool,
    /// Desde quando o servidor grava o uso; servidor antigo não manda.
    #[serde(default)] tracked_since: Option<u64>,
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

/// Um pedido esperando liberação: da conta (vale em todas as máquinas) ou só desta máquina (app antigo).
#[derive(Clone, Copy)]
enum Request<'a> { Account(&'a Device), Local(&'a LocalDevice) }

/// O pendente sai da lista da conta: ele vai para a faixa do topo.
fn account_split(devices: &[Device]) -> (Vec<&Device>, Vec<&Device>) {
    devices.iter().partition(|device| device.status == DeviceStatus::Pending)
}

/// As linhas de `GET /api/connect/devices` que não vieram pela conta, por destino.
struct LocalGroups<'a> { pending: Vec<&'a LocalDevice>, apps: Vec<&'a LocalDevice>, peers: Vec<&'a LocalDevice> }

/// `peer` é a chave que este computador deu a outro ao adicioná-lo; o resto é app que entrou sem a conta.
fn local_groups(devices: &[LocalDevice]) -> LocalGroups<'_> {
    let mut groups = LocalGroups { pending: Vec::new(), apps: Vec::new(), peers: Vec::new() };
    for device in devices {
        match (device.status, &device.peer) {
            (LocalStatus::Pending, _) => groups.pending.push(device),
            (_, Some(_)) => groups.peers.push(device),
            (_, None) => groups.apps.push(device),
        }
    }
    groups
}

const UNUSED_FOR: u64 = 30 * 24 * 60 * 60;

/// Os apps sem uso há 30 dias: do último uso ou, sem ele, da criação, nunca antes de o servidor gravar o uso
/// (`tracked_since`): uso de antes disso não ficou no disco. Fica quem não tem a data de criação, quem veio de servidor
/// antigo e quem não tem `peers_checked`, porque a chave pode ser de outro computador.
fn unused_for_30_days(apps: &[&LocalDevice], now: u64) -> Vec<String> {
    apps.iter().filter(|device| device.peers_checked && device.tracked_since.is_some_and(|tracked| {
        let since = device.last_seen.or_else(|| device.created_at.map(|created| created.max(tracked)));
        since.is_some_and(|since| now.saturating_sub(since) >= UNUSED_FOR)
    })).map(|device| device.device_id.clone()).collect()
}

/// Sem prévia aberta o Parar não faria nada; com a leitura falhando, ele fica para a prévia que talvez esteja aberta.
fn shows_stop(open: bool, read_failed: bool) -> bool { open || read_failed }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stats { computers: Option<(usize, usize)>, phones: Option<usize>, site: usize }

/// Os números do topo: (computadores, no ar), celulares liberados e site aberto. Lista não lida fica sem número, não com zero.
fn stats(machines: Option<&AccountList>, devices: Option<&[Device]>, site_open: bool) -> Stats {
    Stats {
        computers: machines.map(|list| (1 + list.machines.len(), 1 + list.machines.iter().filter(|machine| machine.online).count())),
        phones: devices.map(|devices| devices.iter().filter(|d| d.status == DeviceStatus::Approved && d.kind == DeviceKind::Device).count()),
        site: usize::from(site_open),
    }
}

/// O que a navegação das Configurações e as margens da página tiram da janela.
const SETTINGS_CHROME: f32 = 350.;
const TWO_COLUMNS_FROM: f32 = 1000.;
const COLUMN_GAP: f32 = 16.;
const COMPUTER_MIN: f32 = 210.;
const GRID_GAP: f32 = 12.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout { two_columns: bool, computer_columns: u16 }

/// Janela larga: computadores à esquerda (60%), celulares e site à direita. A grade cabe quantos blocos de 210 px houver.
fn page_layout(viewport: f32) -> Layout {
    let content = (viewport - SETTINGS_CHROME).max(0.);
    let two_columns = content >= TWO_COLUMNS_FROM;
    let left = if two_columns { (content - COLUMN_GAP) * 0.6 } else { content };
    Layout { two_columns, computer_columns: computer_columns(left) }
}

/// Quantos blocos de computador cabem em `width`: a página e a miniatura da novidade usam a mesma conta.
pub(crate) fn computer_columns(width: f32) -> u16 {
    ((width + GRID_GAP) / (COMPUTER_MIN + GRID_GAP)).floor().max(1.) as u16
}

/// O que um bloco da grade "Seus computadores" mostra, em dados simples: a página e a miniatura da novidade desenham
/// pelo mesmo `computer_block`.
pub(crate) struct ComputerLook {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) online: bool,
    /// Esta máquina: realçada, com o selo no canto.
    pub(crate) this: bool,
    pub(crate) removing: bool,
    pub(crate) error: Option<String>,
}

/// Um bloco da grade: ícone, nome e estado, a peça do canto (selo ou "⋯") e o rodapé.
pub(crate) fn computer_block(look: ComputerLook, corner: Option<AnyElement>, footer: Option<AnyElement>) -> Stateful<Div> {
    let ComputerLook { id, name, online, this, removing, error } = look;
    // Este computador está sempre no ar: no lugar do estado vai o selo, e o nome fica com a largura toda.
    let status = if this { div().flex().child(this_computer_chip()) } else {
        div().flex().items_center().gap(px(6.)).text_xs().text_color(theme::muted())
            .child(div().size(px(7.)).rounded_full().flex_shrink_0().bg(if online { theme::success() } else { theme::faint() }))
            .child(tr(if online { "connect_computer_online" } else { "connect_machine_offline" }))
    };
    div().id(SharedString::from(format!("connect-computer-{id}"))).min_w_0().flex().flex_col().gap_3().p_4().rounded(px(14.)).border_1()
        .border_color(if this { theme::accent().alpha(0.5) } else { theme::border() }).bg(theme::boxed())
        .child(div().flex().items_start().gap_3()
            .child(div().size(px(32.)).flex_shrink_0().rounded(px(9.)).bg(if this { theme::accent_dim() } else { theme::inset() })
                .flex().items_center().justify_center()
                .child(chrome::small_icon(IconName::Monitor, 17., if this { theme::accent_text() } else { theme::muted() })))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                .child(status))
            .children(corner))
        .when(removing, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("connect_computer_removing"))))
        .children(error.map(|error| div().id(SharedString::from(format!("connect-computer-error-{id}"))).role(Role::Alert)
            .text_xs().text_color(theme::danger()).whitespace_normal().child(error)))
        .children(footer)
}

/// O selo "Este computador", embaixo do nome do bloco desta máquina.
pub(crate) fn this_computer_chip() -> Div { chip(tr("connect_computer_this"), theme::accent_text(), theme::accent_dim()).flex_shrink_0() }

/// O Desligar no rodapé do bloco desta máquina; quem chama liga o clique.
pub(crate) fn this_computer_off_button(saving: bool) -> Button {
    Button::new("connect-off").outline().small().label(tr("connect_off_this")).disabled(saving)
}

/// "Neste app" no rodapé de um computador que já está na lista.
pub(crate) fn computer_in_app() -> Div {
    div().flex().items_center().gap_1().text_xs().text_color(theme::muted())
        .child(chrome::small_icon(IconName::Check, 13., theme::success_text())).child(tr("connect_computer_in_app"))
}

/// O Adicionar do rodapé; quem chama liga o clique.
pub(crate) fn computer_add_button(id: &str, busy: bool, disabled: bool) -> Button {
    Button::new(SharedString::from(format!("connect-add-{id}"))).outline().small().label(tr("connect_computer_add"))
        .loading(busy).disabled(disabled)
}

/// O "⋯" do bloco sem o menu; quem chama pendura o menu.
pub(crate) fn computer_menu_button(id: &str, name: &str, disabled: bool) -> Button {
    Button::new(SharedString::from(format!("connect-computer-menu-{id}"))).ghost().small().icon(IconName::Ellipsis).flex_shrink_0()
        .accessibility_label(tr("connect_computer_more").replace("{name}", name)).disabled(disabled)
}

/// Largura mínima do menu do "⋯".
pub(crate) const COMPUTER_MENU_WIDTH: f32 = 200.;

/// O item "Remover da conta" do menu do "⋯", em vermelho.
pub(crate) fn computer_remove_item() -> Div { div().text_color(theme::danger()).child(tr("connect_computer_remove")) }

/// O cabeçalho de "Seus computadores", com a conta na explicação.
pub(crate) fn computers_head(account: &str) -> Div {
    section_head(IconName::Monitor, tr("connect_computers_title"), Some(tr("connect_computers_help").replace("{account}", account)), None, px(0.))
}

/// A grade dos blocos.
pub(crate) fn computers_grid(columns: u16) -> Div { div().grid().grid_cols(columns).gap_3() }

/// Os blocos da grade, `None` sendo este computador: ele vem sempre, primeiro, porque é onde mora o Desligar, e a
/// lista da conta pode estar carregando ou ter falhado.
fn computer_slots(list: Option<&AccountList>) -> Vec<Option<&AccountMachine>> {
    std::iter::once(None).chain(list.into_iter().flat_map(|list| list.machines.iter().map(Some))).collect()
}

/// O erro de uma resposta fica junto de quem foi respondido: na faixa do pedido ou na linha da lista.
fn error_of(error: &Option<(String, String)>, id: &str) -> Option<String> {
    error.as_ref().filter(|(who, _)| who == id).map(|(_, text)| text.clone())
}

/// O erro da remoção cujo computador não está na grade: o bloco sumiu com ele (saiu da conta e ficou neste app),
/// então ele vai para o aviso da seção. Lista sem leitura boa também não tem o bloco.
fn loose_remove_error<'a>(error: &'a Option<(String, String)>, machines: Option<&AccountList>) -> Option<&'a str> {
    let (id, text) = error.as_ref()?;
    let in_grid = machines.is_some_and(|list| list.machines.iter().any(|machine| &machine.machine_id == id));
    (!in_grid).then_some(text.as_str())
}

/// A frase da remoção que falhou; a que tirou da conta e não daqui diz o nome, porque aparece fora do bloco.
fn remove_failure_text(error: &Failure, name: &str) -> String {
    match error.code.as_deref() {
        Some("connect_machine_peer_left") => tr("connect_machine_peer_left").replace("{name}", name),
        _ => failure_text(error),
    }
}

fn list_row(icon: IconName, name: String, caption: String, first: bool, right: AnyElement) -> AnyElement {
    div().flex().items_center().gap_3().px_3().py_2().when(!first, |el| el.border_t_1().border_color(theme::border()))
        .child(chrome::small_icon(icon, 16., theme::muted()))
        .child(div().flex_1().min_w_0().flex().flex_col()
            .child(div().text_sm().whitespace_normal().child(name))
            .child(div().text_xs().text_color(theme::muted()).child(caption)))
        .child(right)
        .into_any_element()
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
        Some("connect_machine_remove_unavailable") => tr("connect_machine_remove_unavailable"),
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
    /// (aparelho, frase) da última resposta que falhou.
    device_error: Option<(String, String)>,
    /// Aparelhos desta máquina que não vieram pela conta: aprovados aqui e pedidos de app antigo.
    local: Remote<Vec<LocalDevice>>,
    /// O aparelho desta máquina com Aprovar, Recusar ou Tirar acesso em voo: um por vez.
    local_answering: Option<String>,
    local_error: Option<(String, String)>,
    /// "Acessos só deste computador" aberto; fechado de início, é a parte que pouca gente precisa.
    local_open: bool,
    /// "Tirar os sem uso há 30 dias" em voo, e da última vez quantos saíram e o primeiro erro.
    pruning: bool,
    prune_note: Option<(String, Option<String>)>,
    form: Option<LinkForm>,
    /// A porta da prévia aberta no servidor e até quando (segundos Unix): o link pode ter saído da tela, a prévia não.
    preview_open: Option<(u64, Option<u64>)>,
    preview_busy: bool,
    preview_error: Option<String>,
    /// A última leitura do estado da prévia falhou: o Parar fica à vista.
    preview_read_error: Option<String>,
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
    /// O computador com "Remover da conta" em voo, e (computador, frase) da última remoção que falhou.
    removing: Option<String>,
    remove_error: Option<(String, String)>,
}

impl Connect {
    /// Religar ao mesmo servidor guarda o que está na tela, inclusive o que foi digitado. As respostas da conexão
    /// anterior não chegam mais (o filtro descarta), então nada fica esperando por elas.
    pub(super) fn carry_over(mut self) -> Self {
        (self.saving, self.preview_busy, self.linking, self.answering, self.local_answering) = (false, false, None, None, None);
        (self.pruning, self.removing) = (false, None);
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
    /// "Tirar os sem uso há 30 dias": quantos saíram, de quantos, e o primeiro erro.
    Pruned { removed: usize, total: usize, failure: Option<Failure> },
    MachineRemoved(String, Result<Value, Failure>),
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
        (self.connect.error, self.connect.device_error, self.connect.local_error, self.connect.remove_error) = (None, None, None, None);
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
        (self.connect.linking, self.connect.link_error, self.connect.remove_error) = (Some(id.clone()), None, None);
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
        if self.connect.local_answering.is_some() || self.connect.pruning { return; }
        (self.connect.local_answering, self.connect.local_error) = (Some(device.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let result = if approve { api.server_send(reqwest::Method::POST, &["connect", "devices", &device, "approve"], None, 15).await }
                else { api.server_send(reqwest::Method::DELETE, &["connect", "devices", &device], None, 15).await };
            done(ConnectReply::LocalAnswered(result)).await
        });
        cx.notify();
    }

    fn confirm_prune(&mut self, devices: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let count = devices.len().to_string();
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("connect_local_prune_title").replace("{n}", &count), tr("connect_local_prune_body"),
            tr("connect_local_prune").replace("{n}", &count), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.prune_local(devices.clone(), cx));
                true
            });
    }

    /// Um DELETE por vez, até o fim: a que falha não impede as outras, e a tela diz quantas saíram.
    fn prune_local(&mut self, devices: Vec<String>, cx: &mut Context<Self>) {
        if devices.is_empty() || self.connect.pruning || self.connect.local_answering.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        (self.connect.pruning, self.connect.prune_note, self.connect.local_error) = (true, None, None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let (total, mut removed, mut failure) = (devices.len(), 0, None);
            for device in &devices {
                match api.server_send(reqwest::Method::DELETE, &["connect", "devices", device], None, 15).await {
                    Ok(_) => removed += 1,
                    Err(error) => { failure.get_or_insert(error); }
                }
            }
            done(ConnectReply::Pruned { removed, total, failure }).await
        });
        cx.notify();
    }

    fn confirm_remove_machine(&mut self, machine: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("connect_computer_remove_title").replace("{name}", &name), tr("connect_computer_remove_body"),
            tr("connect_computer_remove"), ButtonVariant::Danger, move |_, cx| {
                let _ = this.update(cx, |this, cx| this.remove_machine(machine.clone(), cx));
                true
            });
    }

    /// Tira o computador da conta; o servidor também o apaga dos peers deste.
    fn remove_machine(&mut self, machine: String, cx: &mut Context<Self>) {
        if self.connect.removing.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        (self.connect.removing, self.connect.remove_error) = (Some(machine.clone()), None);
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::DELETE, &["connect", "machines", &machine], None, 15).await;
            done(ConnectReply::MachineRemoved(machine, result)).await
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
                let device = self.connect.answering.take().unwrap_or_default();
                self.connect.device_error = result.err().map(|error| (device, failure_text(&error)));
                self.load_connect_devices(cx);
            }
            ConnectReply::LocalDevices(seq, result) => {
                let result = result.map_err(|error| failure_text(&error)).and_then(parse_local_devices);
                if !self.connect.local.finish(seq, result) { return; }
            }
            ConnectReply::LocalAnswered(result) => {
                let device = self.connect.local_answering.take().unwrap_or_default();
                self.connect.local_error = result.err().map(|error| (device, failure_text(&error)));
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
            // Leitura que falha guarda a última prévia vista: o Parar continua alcançável.
            ConnectReply::PreviewState(result) => match result {
                Ok(value) => {
                    self.connect.preview_open = open_preview_port(&value).map(|port| (port, value["preview"]["expires_at"].as_u64()));
                    self.connect.preview_read_error = None;
                }
                Err(error) => {
                    let text = failure_text(&error);
                    crate::log_line(&format!("connect: leitura da prévia falhou: {text}"));
                    self.connect.preview_read_error = Some(text);
                }
            },
            ConnectReply::Pruned { removed, total, failure } => {
                self.connect.pruning = false;
                let summary = tr("connect_local_pruned").replace("{n}", &removed.to_string()).replace("{total}", &total.to_string());
                self.connect.prune_note = Some((summary, failure.map(|error| failure_text(&error))));
                self.load_local_devices(cx);
            }
            ConnectReply::MachineRemoved(machine, result) => {
                self.connect.removing = None;
                // O nome sai da lista de antes da releitura: depois dela o computador pode já não estar lá.
                let name = self.connect.machines.ok().and_then(|list| list.machines.iter().find(|m| m.machine_id == machine))
                    .map_or_else(|| machine.clone(), |m| m.name.clone());
                self.connect.remove_error = result.err().map(|error| { let text = remove_failure_text(&error, &name); (machine, text) });
                self.load_account_machines(cx);
                self.load_peer_links();
            }
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

    /// Faixa do pedido esperando: Recusar e Liberar, com o código que o aparelho mostra. Pedido da conta ou só desta máquina.
    fn render_request(&self, request: Request<'_>, cx: &mut Context<Self>) -> AnyElement {
        let (id, name, detail, machine, local) = match request {
            Request::Account(device) => {
                let when = super::side::ago(super::side::now_seconds() - device.created_at);
                let detail = match &device.install_code {
                    Some(code) => tr_shared("connect_account_device_asked_code", &[("code", code), ("when", &when)]),
                    None => tr_shared("connect_account_device_asked", &[("when", &when)]),
                };
                (device.device_id.clone(), device.name.clone(), Some(detail), device.kind == DeviceKind::Machine, false)
            }
            Request::Local(device) => (device.device_id.clone(), device.name.clone(),
                device.install_code.as_ref().map(|code| tr("connect_request_code").replace("{code}", code)), false, true),
        };
        let (busy, blocked, error) = if local {
            (self.connect.local_answering.as_deref() == Some(id.as_str()), self.connect.local_answering.is_some() || self.connect.pruning,
                error_of(&self.connect.local_error, &id))
        } else {
            (self.connect.answering.as_deref() == Some(id.as_str()), self.connect.answering.is_some(), error_of(&self.connect.device_error, &id))
        };
        let button = |action: &str| Button::new(SharedString::from(format!("connect-request-{action}-{id}"))).small().flex_shrink_0().disabled(blocked);
        let (deny, release, release_name) = (id.clone(), id.clone(), name.clone());
        let actions = div().flex().items_center().gap_2()
            .child(button("deny").outline().label(tr_shared("connect_device_deny", &[]))
                .on_click(cx.listener(move |this, _, _, cx| if local { this.answer_local_device(deny.clone(), false, cx) } else { this.answer_device(deny.clone(), false, cx) })))
            .child(button("release").primary().label(tr_shared("connect_account_device_release", &[])).loading(busy)
                .on_click(cx.listener(move |this, _, window, cx| if local { this.confirm_local_approve(release.clone(), release_name.clone(), window, cx) }
                    else { this.confirm_release(release.clone(), release_name.clone(), window, cx) })));
        div().id(SharedString::from(format!("connect-request-{id}"))).flex().flex_wrap().items_center().gap_3().px_4().py_3()
            .rounded(px(14.)).border_1().border_color(theme::warning().alpha(0.45)).bg(theme::warning().alpha(0.1))
            .child(chrome::small_icon(if machine { IconName::Monitor } else { IconName::Smartphone }, 18., theme::warning()))
            .child(div().flex_1().min_w(px(240.)).flex().flex_col().gap_1()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(tr("connect_request_title").replace("{name}", &name)))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_request_help")))
                .children(detail.map(|detail| div().text_sm().font_weight(FontWeight::MEDIUM).whitespace_normal().child(detail)))
                .children(error.map(|error| div().id(SharedString::from(format!("connect-request-error-{id}"))).role(Role::Alert)
                    .text_sm().text_color(theme::danger()).whitespace_normal().child(error))))
            .child(actions)
            .into_any_element()
    }

    /// Os pedidos esperando, da conta e só desta máquina: saem das listas de baixo e ficam aqui, no topo.
    fn render_requests(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let account = self.connect.devices.ok().into_iter().flatten().filter(|device| device.status == DeviceStatus::Pending).map(Request::Account);
        let local = self.connect.local.ok().into_iter().flatten().filter(|device| device.status == LocalStatus::Pending).map(Request::Local);
        let requests: Vec<Request<'_>> = account.chain(local).collect();
        requests.into_iter().map(|request| self.render_request(request, cx)).collect()
    }

    /// Linha liberada na conta: Tirar acesso; esta máquina, nenhum botão. O ícone diz se é PC.
    fn render_phone_row(&self, device: &Device, this_machine_id: &str, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let machine = device.kind == DeviceKind::Machine;
        let caption = device.approved_at.map_or_else(|| tr("connect_phone_released"),
            |at| tr("connect_phone_released_ago").replace("{when}", &super::side::ago(super::side::now_seconds() - at)));
        let busy = self.connect.answering.as_deref() == Some(device.device_id.as_str());
        let (id, name) = (device.device_id.clone(), device.name.clone());
        let right = match row_actions(device, this_machine_id) {
            RowActions::This => chip(tr("connect_computer_this"), theme::accent_text(), theme::accent_dim()).into_any_element(),
            _ => Button::new(SharedString::from(format!("connect-revoke-{}", device.device_id))).small().flex_shrink_0().outline()
                .label(tr_shared("connect_device_revoke", &[])).loading(busy).disabled(self.connect.answering.is_some())
                .on_click(cx.listener(move |this, _, window, cx| this.confirm_revoke(id.clone(), name.clone(), window, cx))).into_any_element(),
        };
        list_row(if machine { IconName::Monitor } else { IconName::Smartphone }, device.name.clone(), caption, first, right)
    }

    /// Liberados na conta e, recolhidos embaixo, os acessos que valem só aqui.
    fn render_phones(&self, this_machine_id: &str, cx: &mut Context<Self>) -> Div {
        let pending = |id: &str| self.connect.devices.ok().is_some_and(|devices| devices.iter().any(|d| d.device_id == id && d.status == DeviceStatus::Pending));
        let section = div().flex().flex_col().gap_3()
            .child(section_head(IconName::Smartphone, tr("connect_phones_title"), Some(tr("connect_phones_help")), None, px(0.)))
            .children(self.connect.device_error.as_ref().filter(|(id, _)| !pending(id.as_str())).map(|(_, error)| div().id("connect-device-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone())));
        let list = match device_view(&self.connect.devices) {
            DeviceView::Loading => div().text_sm().text_color(theme::muted()).child(tr("loading")),
            DeviceView::Failed(error) => div().flex().flex_col().gap_2()
                .child(div().id("connect-devices-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))
                .child(div().child(Button::new("connect-devices-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_connect_devices(cx))))),
            DeviceView::Empty => div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_phones_empty")),
            DeviceView::Loaded(devices) => {
                let (_, released) = account_split(devices);
                if released.is_empty() {
                    div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_phones_empty"))
                } else {
                    let rows: Vec<AnyElement> = released.iter().enumerate().map(|(at, device)| self.render_phone_row(device, this_machine_id, at == 0, cx)).collect();
                    settings_box().children(rows)
                }
            }
        };
        section.child(list).child(self.render_local_access(cx))
    }

    /// Credencial só desta máquina: Tirar, com quando foi usada. A entregue a outro computador diz isso na legenda.
    fn render_local_row(&self, device: &LocalDevice, first: bool, cx: &mut Context<Self>) -> AnyElement {
        let peer = device.peer.is_some();
        let used = device.last_seen.map_or_else(|| tr("connect_local_never_used"),
            |at| tr("connect_local_used").replace("{when}", &super::side::ago(super::side::now_seconds() - at as f64)));
        let caption = if peer { format!("{} · {used}", tr("connect_local_peer_caption")) } else { used };
        let busy = self.connect.local_answering.as_deref() == Some(device.device_id.as_str());
        let blocked = self.connect.local_answering.is_some() || self.connect.pruning;
        let (id, name) = (device.device_id.clone(), device.name.clone());
        let right = Button::new(SharedString::from(format!("connect-local-revoke-{}", device.device_id))).small().flex_shrink_0().outline()
            .label(tr("connect_local_remove")).loading(busy).disabled(blocked)
            .on_click(cx.listener(move |this, _, window, cx| this.confirm_local_revoke(id.clone(), name.clone(), window, cx))).into_any_element();
        list_row(if peer { IconName::Monitor } else { IconName::Smartphone }, device.name.clone(), caption, first, right)
    }

    /// "Acessos só deste computador": fechado por padrão, com os apps antigos e as chaves dadas aos outros computadores.
    fn render_local_access(&self, cx: &mut Context<Self>) -> Div {
        let open = self.connect.local_open;
        let groups = self.connect.local.ok().map(|devices| local_groups(devices));
        let label = groups.as_ref().map_or_else(|| tr("connect_local_title_plain"),
            |groups| tr("connect_local_title").replace("{n}", &(groups.apps.len() + groups.peers.len()).to_string()));
        let pending = |id: &str| groups.as_ref().is_some_and(|groups| groups.pending.iter().any(|d| d.device_id == id));
        let weak = cx.entity().downgrade();
        let mut section = div().flex().flex_col().gap_2()
            .child(div().flex().child(Disclosure::new("connect-local-open", open, label, false)
                .on_change(move |open, cx| { let _ = weak.update(cx, |this, cx| { this.connect.local_open = open; cx.notify(); }); })))
            .children(self.connect.local_error.as_ref().filter(|(id, _)| !pending(id.as_str())).map(|(_, error)| div().id("connect-local-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone())));
        // A leitura que falhou aparece mesmo fechado: senão o "(N)" sumido não diria por quê.
        if let DeviceView::Failed(error) = device_view(&self.connect.local) {
            return section
                .child(div().id("connect-local-devices-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned()))
                .child(div().child(Button::new("connect-local-devices-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_local_devices(cx)))));
        }
        if !open { return section; }
        section = section.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_local_help")));
        let Some(groups) = groups else { return section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))) };
        if groups.apps.is_empty() && groups.peers.is_empty() {
            return section.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_local_empty")));
        }
        let group_title = |key: &str| div().flex_1().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr(key));
        if !groups.apps.is_empty() {
            let unused = unused_for_30_days(&groups.apps, super::side::now_seconds() as u64);
            let prune = (!unused.is_empty()).then(|| {
                let count = unused.len();
                Button::new("connect-local-prune").outline().xsmall().flex_shrink_0().label(tr("connect_local_prune").replace("{n}", &count.to_string()))
                    .loading(self.connect.pruning).disabled(self.connect.pruning || self.connect.local_answering.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm_prune(unused.clone(), window, cx)))
            });
            let rows: Vec<AnyElement> = groups.apps.iter().enumerate().map(|(at, device)| self.render_local_row(device, at == 0, cx)).collect();
            section = section.child(div().flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_2().min_h(px(24.)).child(group_title("connect_local_apps")).children(prune))
                .child(settings_box().children(rows)));
        }
        if let Some((summary, error)) = self.connect.prune_note.clone() {
            section = section.child(div().id("connect-local-pruned").text_xs().text_color(theme::muted()).whitespace_normal().child(summary))
                .children(error.map(|error| div().id("connect-local-prune-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error)));
        }
        if !groups.peers.is_empty() {
            let rows: Vec<AnyElement> = groups.peers.iter().enumerate().map(|(at, device)| self.render_local_row(device, at == 0, cx)).collect();
            section = section.child(div().flex().flex_col().gap_2()
                .child(div().flex().items_center().min_h(px(24.)).child(group_title("connect_local_peers")))
                .child(settings_box().children(rows)));
        }
        section
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

    /// Um bloco da grade de computadores: o desta máquina vem realçado, com o Desligar; os outros, com Adicionar e o "⋯".
    fn render_account_computer(&self, machine: Option<&AccountMachine>, this_name: &str, cx: &mut Context<Self>) -> AnyElement {
        let (name, online) = machine.map_or((this_name.to_owned(), true), |m| (m.name.clone(), m.online));
        let corner = machine.map(|machine| self.computer_menu(machine, cx).into_any_element());
        let footer: Option<AnyElement> = match machine {
            None => Some(this_computer_off_button(self.connect.saving)
                .on_click(cx.listener(|this, _, window, cx| this.confirm_unlink(window, cx))).into_any_element()),
            Some(machine) => self.computer_footer(machine, cx),
        };
        let look = ComputerLook {
            id: machine.map_or_else(|| "this".to_owned(), |m| m.machine_id.clone()),
            name,
            online,
            this: machine.is_none(),
            removing: machine.is_some_and(|m| self.connect.removing.as_deref() == Some(m.machine_id.as_str())),
            error: machine.and_then(|m| error_of(&self.connect.remove_error, &m.machine_id)),
        };
        computer_block(look, corner, footer).into_any_element()
    }

    /// "Neste app", ou Adicionar enquanto ela está no ar; fora do ar e fora do app o bloco já diz tudo.
    fn computer_footer(&self, machine: &AccountMachine, cx: &mut Context<Self>) -> Option<AnyElement> {
        let link = self.connect.links.iter().find(|link| link.machine_id == machine.machine_id);
        // Ligar entre PCs da conta não pede aprovação: enquanto o pedido anda, o próprio botão mostra a espera.
        let pending = link.is_some_and(|link| matches!(link.stage.as_str(), "requesting" | "waiting_approval"));
        if machine.added || self.in_app_list(&machine.host) || link.is_some_and(|link| link.stage == "linked") {
            return Some(computer_in_app().into_any_element());
        }
        if !machine.online && !pending { return None; }
        let id = machine.machine_id.clone();
        let failed = link.filter(|link| link.stage == "failed").and_then(|link| link.message.clone());
        Some(div().flex().flex_col().items_start().gap_1()
            .children(failed.map(|message| div().text_xs().text_color(theme::danger()).whitespace_normal().child(message)))
            .child(computer_add_button(&id, pending || self.connect.linking.as_deref() == Some(id.as_str()), pending || self.connect.linking.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.add_account_machine(id.clone(), cx))))
            .into_any_element())
    }

    /// O "⋯" do bloco: o que é raro e perigoso fica no menu, longe do Adicionar.
    fn computer_menu(&self, machine: &AccountMachine, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, name) = (machine.machine_id.clone(), machine.name.clone());
        let weak = cx.entity().downgrade();
        computer_menu_button(&id, &name, self.connect.removing.is_some())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let (weak, id, name) = (weak.clone(), id.clone(), name.clone());
                sidebar::menu_style(menu).min_w(px(COMPUTER_MENU_WIDTH))
                    .item(PopupMenuItem::element(|_, _| computer_remove_item()).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.root_focus.focus(window, cx);
                            this.confirm_remove_machine(id.clone(), name.clone(), window, cx);
                        });
                    }))
            })
    }

    /// "Seus computadores": a grade com esta máquina primeiro e os outros PCs da conta.
    fn render_computers(&self, host: &str, columns: u16, cx: &mut Context<Self>) -> Div {
        let account = identity_of(host).map_or("", |(_, account)| account);
        let section = div().flex().flex_col().gap_3()
            .child(computers_head(account))
            .children(self.connect.link_error.clone().map(|error| div().id("connect-link-error").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .children(self.connect.machines_error.clone().map(|error| div().id("connect-machines-stale")
                .text_xs().text_color(theme::muted()).whitespace_normal().child(error)))
            .children(loose_remove_error(&self.connect.remove_error, self.connect.machines.ok()).map(|error| div().id("connect-remove-error")
                .role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.to_owned())));
        let list = self.connect.machines.ok();
        let this_name = identity_of(host).map(|(machine, _)| machine.to_owned())
            .or_else(|| list.map(|list| list.this.machine_id.clone())).unwrap_or_default();
        let mut blocks = Vec::new();
        for machine in computer_slots(list) { blocks.push(self.render_account_computer(machine, &this_name, cx)); }
        let section = section.child(computers_grid(columns).children(blocks));
        match &self.connect.machines.value {
            None => section.child(div().text_sm().text_color(theme::muted()).child(tr("loading"))),
            Some(Err(error)) => section
                .child(div().id("connect-machines-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(div().child(Button::new("connect-machines-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_account_machines(cx))))),
            Some(Ok(_)) => section.child(div().flex().items_start().gap_2()
                .child(div().pt(px(2.)).child(chrome::small_icon(IconName::Info, 14., theme::faint())))
                .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_computers_tip")))),
        }
    }

    /// O site aberto por "Link de fora" na aba Contexto. Sem prévia aberta não há o que parar; com a leitura falhando,
    /// o Parar fica à mão para a prévia que talvez esteja aberta.
    fn render_site(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.connect.form.as_ref()?;
        let busy = self.connect.preview_busy;
        let read_error = self.connect.preview_read_error.clone();
        let body = match self.connect.preview_open {
            Some((port, expires)) => {
                let until = expires.and_then(|at| chrono::DateTime::from_timestamp(at as i64, 0))
                    .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string());
                let detail = until.map_or_else(|| tr("connect_site_open"), |time| tr("connect_site_until").replace("{time}", &time));
                // O link pedido pela aba Contexto, para esta máquina: o servidor só guarda o hash dele.
                let server = self.api.as_ref().map(Api::identity);
                let link = self.ports.link.as_ref().filter(|link| Some(&link.server) == server.as_ref()).map(|link| {
                    let copied = link.url.clone();
                    div().flex().items_center().gap_2()
                        .child(div().flex_1().min_w_0().font_family(theme::MONO).text_sm().truncate().child(link.url.clone()))
                        .child(Button::new("connect-preview-copy").outline().small().flex_shrink_0().label(tr("connect_preview_copy"))
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))))
                });
                div().id("connect-preview-open").flex().flex_col().gap_1()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("connect_site_port").replace("{port}", &port.to_string())))
                    .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(detail))
                    .children(link.map(|link| div().pt_1().child(link)))
                    .into_any_element()
            }
            None if read_error.is_none() => div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_site_none")).into_any_element(),
            None => div().into_any_element(),
        };
        let stop = shows_stop(self.connect.preview_open.is_some(), read_error.is_some()).then(|| div().child(Button::new("connect-preview-stop")
            .outline().small().label(tr("connect_preview_stop")).loading(busy).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.stop_preview(cx)))));
        Some(div().flex().flex_col().gap_3()
            .child(section_head(IconName::Globe, tr("connect_site_title"), None, None, px(0.)))
            .child(settings_box().p_4().gap_3()
                .child(body)
                .children(self.connect.preview_error.clone().map(|error| div().id("connect-preview-error").role(Role::Alert)
                    .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
                .children(stop))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("connect_site_tip"))))
    }

    /// O topo ligado: estado, endereço e os três números da página.
    fn render_ready_top(&self, host: &str) -> Div {
        let address = format!("https://{host}");
        let numbers = stats(self.connect.machines.ok(), self.connect.devices.ok().map(Vec::as_slice), self.connect.preview_open.is_some());
        let count = |value: Option<usize>| value.map_or_else(|| "–".to_owned(), |value| value.to_string());
        let stat = |value: String, label: String, note: Option<String>| div().flex().flex_col().items_end().gap(px(2.))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(value))
            .child(div().text_xs().text_color(theme::muted()).child(label))
            .children(note.map(|note| div().text_xs().text_color(theme::success_text()).child(note)));
        let online = numbers.computers.map(|(_, online)| tr("connect_stat_online").replace("{n}", &online.to_string()));
        settings_box().p_5().child(div().flex().flex_wrap().items_center().gap_x(px(24.)).gap_y(px(16.))
            .child(div().flex_1().min_w(px(280.)).flex().flex_col().gap_2()
                .child(div().flex().items_center().gap_2()
                    .child(div().size(px(10.)).rounded_full().flex_shrink_0().bg(theme::success()))
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("connect_on"))))
                .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("connect_on_help")))
                .child(div().flex().items_center().gap_2().pt_1()
                    .child(div().min_w_0().font_family(theme::MONO).text_sm().truncate().child(address.clone()))
                    .child(Button::new("connect-copy").outline().small().flex_shrink_0().label(tr("connect_copy"))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(address.clone()))))))
            .child(div().flex().items_start().gap_6()
                .child(stat(count(numbers.computers.map(|(all, _)| all)), tr("connect_stat_computers"), online))
                .child(stat(count(numbers.phones), tr("connect_stat_phones"), None))
                .child(stat(numbers.site.to_string(), tr("connect_stat_site"), None))))
    }

    pub(super) fn render_connect(&mut self, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let layout = page_layout(f32::from(width));
        let title = div().flex().items_center().gap_2()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Connect.title()))
            .child(chip(tr("server_scope"), theme::muted(), theme::raised()));
        // Fora do "ligado" a página é um formulário ou um aviso: fica na coluna de leitura de antes.
        let narrow = |page: Div| div().w_full().flex().justify_center().child(page.w_full().max_w(px(720.))).into_any_element();
        let mut page = div().flex().flex_col().gap_4().child(title);
        if self.api.is_none() {
            return narrow(page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))));
        }
        let status = match &self.connect.status.value {
            Some(Ok(status)) => status.clone(),
            Some(Err(error)) => return narrow(page.child(div().id("connect-load-error").role(Role::Alert).text_sm().text_color(theme::danger()).child(error.clone()))
                .child(div().child(Button::new("connect-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.connect.error = None; this.load_connect(cx); }))))),
            None => return narrow(page.child(div().text_sm().text_color(theme::muted()).child(tr("loading")))),
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
        // Parado ou subindo dá para desligar; ligado, o Desligar mora no bloco desta máquina.
        let off = |cx: &mut Context<Self>| Button::new("connect-off").outline().small().label(tr("connect_off")).disabled(saving)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_unlink(window, cx)));
        match state {
            Phase::Unlinked => {
                page = match self.connect.entry {
                    None => page.child(self.render_connect_hero(cx)),
                    Some(entry) => page.children(self.render_connect_form(entry, false, cx)),
                };
                return narrow(page);
            }
            Phase::Stopped => page = page.children(self.render_connect_form(Entry::Login, true, cx)).child(div().child(off(cx))),
            Phase::Starting => page = page.child(settings_box().p_6().items_center().gap_2()
                .child(connect_mark("connect-starting-mark", 56., true, cx.reduce_motion()))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("connect_linking_title")))
                .child(div().max_w(px(400.)).text_sm().text_color(theme::muted()).text_center().whitespace_normal().child(tr("connect_linking_help")))
                .child(div().pt_1().child(off(cx)))),
            Phase::Ready => page = page.child(self.render_ready_top(&host)),
        }
        // A lista é da conta, lida com a chave desta máquina: parado ou subindo, os pedidos e os acessos continuam à mão.
        page = page.children(self.render_requests(cx));
        if state != Phase::Ready {
            return narrow(page.child(self.render_phones(&this_machine_id, cx)));
        }
        let computers = self.render_computers(&host, layout.computer_columns, cx);
        let side = div().flex().flex_col().gap_6().child(self.render_phones(&this_machine_id, cx)).children(self.render_site(cx));
        if layout.two_columns {
            page.child(div().flex().items_start().gap_4()
                .child(div().flex_basis(relative(0.6)).flex_shrink(1.).min_w_0().child(computers))
                .child(div().flex_basis(relative(0.4)).flex_shrink(1.).min_w_0().child(side)))
                .into_any_element()
        } else {
            page.child(computers).child(side).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{account_split, address_preview, computer_slots, device_view, identity_of, local_groups, machine_label, open_preview_port, page_layout, parse_account, parse_devices, parse_links, parse_local_devices, parse_port, parse_preview, parse_status, phase, row_actions, shows_stop, stats, take_password, unused_for_30_days, Device, DeviceKind, DeviceStatus, DeviceView, InputState, Layout, LocalStatus, Phase, Remote, RowActions, Stats};
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

    /// A chave que este computador deu a outro ao adicioná-lo é de "Outros computadores"; o resto é app. Pendente vai ao topo.
    #[test]
    fn local_access_splits_apps_from_keys_given_to_other_computers() {
        let devices = parse_local_devices(json!([
            {"device_id": "a1", "name": "Pixel", "status": "approved", "last_seen": 10, "created_at": 1},
            {"device_id": "p1", "name": "casa", "status": "approved", "last_seen": null, "created_at": 1, "peer": "casa"},
            {"device_id": "d4", "name": "App antigo", "status": "pending", "last_seen": null, "created_at": 2, "install_code": "123 456"},
            {"device_id": "b2", "name": "iPhone da conta", "status": "approved", "last_seen": null, "created_at": 1, "account_device": "D"}])).unwrap();
        let groups = local_groups(&devices);
        let ids = |rows: &[&super::LocalDevice]| rows.iter().map(|d| d.device_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&groups.pending), ["d4"]);
        assert_eq!(ids(&groups.apps), ["a1"]);
        assert_eq!(ids(&groups.peers), ["p1"], "servidor sem o campo peer deixa tudo em Apps");
    }

    /// Sem uso há 30 dias conta do último uso; sem ele, da criação, mas nunca antes de o servidor gravar o uso.
    #[test]
    fn unused_counts_thirty_days_from_the_last_use_or_from_when_use_was_tracked() {
        let day: u64 = 24 * 60 * 60;
        let now = 100 * day;
        let row = |id: &str, last_seen: Option<u64>, created_at: Option<u64>, tracked_since: Option<u64>| json!({"device_id": id, "name": id,
            "status": "approved", "last_seen": last_seen, "created_at": created_at, "tracked_since": tracked_since, "peers_checked": true});
        let devices = parse_local_devices(json!([
            row("seen-old", Some(now - 30 * day), Some(1), Some(1)),
            row("seen-recent", Some(now - 29 * day), Some(1), Some(1)),
            row("never-old", None, Some(now - 40 * day), Some(now - 40 * day)),
            // Credencial de antes do uso ir ao disco: nulo não prova que nunca entrou.
            row("tracked-late", None, Some(now - 60 * day), Some(now - 10 * day)),
            row("never-new", None, Some(now - 10 * day), Some(now - 60 * day)),
            row("old-server", None, Some(1), None),
            row("undated", None, None, Some(1)),
            json!({"device_id": "peer", "name": "f", "status": "approved", "last_seen": null, "created_at": 1, "tracked_since": 1,
                "peer": "casa", "peers_checked": true})])).unwrap();
        let groups = local_groups(&devices);
        assert_eq!(unused_for_30_days(&groups.apps, now), ["seen-old", "never-old"], "exatamente 30 dias já conta");
        assert_eq!(unused_for_30_days(&groups.apps, now + 20 * day), ["seen-old", "seen-recent", "never-old", "tracked-late", "never-new"],
            "servidor antigo e linha sem criação nunca contam");
        assert!(unused_for_30_days(&groups.apps, 0).is_empty(), "relógio atrás não tira ninguém");
    }

    /// Sem peers.json lido (ou servidor antigo), a chave sem `peer` pode ser de outro computador: o botão não a conta.
    #[test]
    fn unused_skips_rows_whose_peers_were_not_checked() {
        let devices = parse_local_devices(json!([
            {"device_id": "unchecked", "name": "a", "status": "approved", "last_seen": null, "created_at": 1, "tracked_since": 1, "peers_checked": false},
            {"device_id": "old-server", "name": "b", "status": "approved", "last_seen": null, "created_at": 1, "tracked_since": 1},
            {"device_id": "checked", "name": "c", "status": "approved", "last_seen": null, "created_at": 1, "tracked_since": 1, "peers_checked": true}])).unwrap();
        let groups = local_groups(&devices);
        assert_eq!(groups.apps.len(), 3, "continuam na lista de apps");
        assert_eq!(unused_for_30_days(&groups.apps, 40 * 24 * 60 * 60), ["checked"]);
    }

    /// Este computador, com o Desligar, está na grade mesmo antes da lista da conta chegar ou com ela falhando.
    #[test]
    fn this_computer_is_always_the_first_block() {
        assert_eq!(computer_slots(None), [None], "lista carregando ou com erro: só este computador");
        let list = parse_account(json!({"this": {"machine_id": "a"}, "machines": [
            {"machine_id": "b", "name": "Casa", "host": "b.j.hangar.dev.br", "online": true, "added": false}]})).unwrap();
        let ids: Vec<Option<&str>> = computer_slots(Some(&list)).into_iter().map(|slot| slot.map(|m| m.machine_id.as_str())).collect();
        assert_eq!(ids, [None, Some("b")]);
    }

    /// O erro fica no bloco enquanto o computador está na grade; saído dela, vai para o aviso da seção.
    #[test]
    fn a_remove_error_without_its_block_goes_to_the_section() {
        let list = parse_account(json!({"this": {"machine_id": "a", "host": "a.j.hangar.dev.br"},
            "machines": [{"machine_id": "b", "name": "Casa", "host": "b.j.hangar.dev.br", "online": true, "added": true}]})).unwrap();
        let error = |id: &str| Some((id.to_owned(), "falhou".to_owned()));
        assert_eq!(super::loose_remove_error(&error("b"), Some(&list)), None, "no bloco de b");
        assert_eq!(super::loose_remove_error(&error("c"), Some(&list)), Some("falhou"), "c saiu da lista na releitura");
        assert_eq!(super::loose_remove_error(&error("b"), None), Some("falhou"), "sem lista boa não há bloco");
        assert_eq!(super::loose_remove_error(&None, None), None);
        let failure = |code: &str| crate::api::Failure { status: Some(500), detail: "do servidor".into(), retry_after: None, uncertain: false,
            code: Some(code.into()) };
        let left = super::remove_failure_text(&failure("connect_machine_peer_left"), "Casa");
        assert!(left.contains("Casa") && left != "connect_machine_peer_left", "{left}");
        assert_ne!(super::remove_failure_text(&failure("connect_machine_remove_unavailable"), "Casa"), "do servidor");
    }

    /// O Parar só aparece com prévia aberta ou com a leitura do estado falhando.
    #[test]
    fn stop_shows_only_with_an_open_preview_or_a_failed_read() {
        assert!(!shows_stop(false, false), "sem prévia, Parar não faria nada");
        assert!(shows_stop(true, false));
        assert!(shows_stop(false, true), "leitura falhou: a prévia talvez esteja aberta");
        assert!(shows_stop(true, true));
    }

    /// Os pendentes da conta saem da lista: a faixa do topo os mostra, com o código quando o serviço manda.
    #[test]
    fn pending_account_devices_leave_the_list_for_the_top() {
        let devices = parse_devices(json!([
            {"device_id": "a1", "name": "iPhone", "status": "approved", "created_at": 1.0, "approved_at": 2.0},
            {"device_id": "c3", "name": "Pixel", "status": "pending", "created_at": 5.0, "install_code": "482 913"}])).unwrap();
        let (pending, released) = account_split(&devices);
        assert_eq!(pending.iter().map(|d| (d.device_id.as_str(), d.install_code.as_deref())).collect::<Vec<_>>(), [("c3", Some("482 913"))]);
        assert_eq!(released.iter().map(|d| (d.device_id.as_str(), d.approved_at)).collect::<Vec<_>>(), [("a1", Some(2.0))]);
    }

    #[test]
    fn top_numbers_count_this_machine_and_only_released_phones() {
        let list = parse_account(json!({"this": {"machine_id": "a"}, "machines": [
            {"machine_id": "casa", "name": "casa", "host": "casa.j.hangar.dev.br", "online": true, "added": false},
            {"machine_id": "vps", "name": "vps", "host": "vps.j.hangar.dev.br", "online": false, "added": true}]})).unwrap();
        let devices = parse_devices(json!([
            {"device_id": "a1", "name": "iPhone", "status": "approved", "created_at": 1.0},
            {"device_id": "b2", "name": "casa", "kind": "machine", "status": "approved", "created_at": 1.0},
            {"device_id": "c3", "name": "Pixel", "status": "pending", "created_at": 1.0}])).unwrap();
        assert_eq!(stats(Some(&list), Some(&devices), true), Stats { computers: Some((3, 2)), phones: Some(1), site: 1 });
        assert_eq!(stats(None, None, false), Stats { computers: None, phones: None, site: 0 }, "lista não lida não vira zero");
    }

    #[test]
    fn wide_window_splits_the_page_and_fits_the_computer_grid() {
        assert_eq!(page_layout(1400.), Layout { two_columns: true, computer_columns: 2 });
        assert_eq!(page_layout(1900.), Layout { two_columns: true, computer_columns: 4 });
        assert_eq!(page_layout(1200.), Layout { two_columns: false, computer_columns: 3 });
        assert_eq!(page_layout(400.), Layout { two_columns: false, computer_columns: 1 }, "estreita demais ainda tem uma coluna");
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
            linking: Some("casa".to_owned()), answering: Some("x".to_owned()), pruning: true, removing: Some("pc".to_owned()), ..Default::default() };
        kept.status.set(Ok(parse_status(json!({"enabled": false, "running": false, "host": null, "error": null})).unwrap()));
        let kept = kept.carry_over();
        assert_eq!(kept.entry, Some(super::Entry::Create), "o formulário escolhido continua");
        assert!(kept.status.ok().is_some(), "a fase continua até a nova leitura");
        assert!(!kept.saving && !kept.preview_busy && kept.linking.is_none() && kept.answering.is_none() && !kept.pruning && kept.removing.is_none(),
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
