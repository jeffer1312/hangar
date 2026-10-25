//! Página Máquinas (web: "Servidores"): o servidor conectado vira um cartão, e o detalhe dele traz identificador, endereços,
//! reinício do serviço e o Avançado. Porta de `MaquinasSettings.svelte` e da parte "detalhe" de `AcessoSettings.svelte`.
//! A lista casa as máquinas guardadas neste aparelho com o registro do servidor pelo identificador, uma linha por máquina
//! (`unirMaquinas` de `lib/maquinas.ts`).
mod add;
mod pair;

use super::*;
use std::{collections::HashMap, rc::Rc};
use super::device::Remote;
use add::{AddMachine, Found};
use pair::Pair;
use super::server_config::chip;
use super::settings::{section_head, settings_box, Page};
use gpui_kit::component::{WindowExt, switch::Switch, tooltip::Tooltip};

/// O web espera o serviço voltar por até 2 minutos, perguntando a cada 2 segundos.
const RESTART_WAIT: Duration = Duration::from_secs(120);
const RESTART_POLL: Duration = Duration::from_secs(2);
/// Etapas da atualização passam de 5 minutos; a página Sobre espera 10.
const UPGRADE_WAIT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind { Here, Lan, Tailscale, Public }

impl Kind {
    fn parse(raw: &str) -> Option<Self> {
        match raw { "nesta_maquina" => Some(Kind::Here), "rede_local" => Some(Kind::Lan), "tailscale" => Some(Kind::Tailscale),
            "publico" => Some(Kind::Public), _ => None }
    }
    fn name(self) -> String {
        tr(match self { Kind::Here => "machines_kind_here", Kind::Lan => "machines_kind_lan", Kind::Tailscale => "machines_kind_tailscale",
            Kind::Public => "machines_kind_public" })
    }
    /// O nome do tipo nas rotas do servidor (`/api/alcance/pareamento?endereco=`).
    pub(super) fn raw(self) -> &'static str {
        match self { Kind::Here => "nesta_maquina", Kind::Lan => "rede_local", Kind::Tailscale => "tailscale", Kind::Public => "publico" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status { Ok, Failed, Testing, Unset }

#[derive(Clone, Debug)]
pub(super) struct Address { pub(super) kind: Kind, pub(super) url: String, pub(super) status: Status, pub(super) ms: Option<i64> }

/// `/api/alcance`: por onde o servidor responde, medido por ele mesmo.
#[derive(Clone, Debug, Default)]
pub(super) struct Reach { pub(super) loopback: bool, pub(super) bind: String, pub(super) addresses: Vec<Address> }

/// Farol de uma linha ou do cartão: a frase ao lado diz o mesmo, a cor nunca vai sozinha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light { Ok, No, Test, Neutral }

impl Light {
    fn glyph(self) -> &'static str { match self { Light::Ok | Light::No => "●", Light::Test => "◌", Light::Neutral => "○" } }
    fn color(self) -> Hsla { match self { Light::Ok => theme::success(), Light::No => theme::danger(), _ => theme::muted() } }
    /// Bolinha no canto do quadradinho: cheia quando há resultado, vazada enquanto testa ou quando não há o que medir.
    fn dot(self) -> (Hsla, bool) { (self.color(), matches!(self, Light::Ok | Light::No)) }
}

/// O quadradinho de cada máquina (ícone ou inicial), com o farol no canto.
fn tile(content: impl IntoElement, size: f32, fill: Hsla, dot: Option<(Hsla, bool)>) -> Div {
    div().relative().size(px(size)).flex_shrink_0().rounded(px((size * 0.27).round())).bg(fill).border_1().border_color(theme::border())
        .flex().items_center().justify_center()
        .child(content)
        .when_some(dot, |el, (color, filled)| el.child(div().absolute().right(px(-2.)).bottom(px(-2.)).size(px(10.)).rounded_full()
            .map(|d| if filled { d.bg(color) } else { d.border_2().border_color(color) })))
}

fn initial(name: &str) -> String { name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() }

/// Ida e volta medidas: as duas máquinas nas pontas e o tempo de cada sentido na seta dele.
fn two_way(here: &str, name: &str, going: i64, back: i64) -> Stateful<Div> {
    let node = |text: &str, color: Hsla| div().flex_shrink_0().px(px(10.)).py(px(8.)).rounded(px(9.)).bg(theme::inset()).border_1()
        .border_color(theme::border()).flex().items_center().gap(px(7.))
        .child(div().size(px(7.)).rounded_full().bg(color))
        .child(div().font_family(theme::MONO).text_size(px(12.)).child(text.to_owned()));
    let way = |ms: i64, forward: bool| div().flex().items_center().gap(px(6.))
        .when(!forward, |el| el.child(chrome::small_icon(IconName::ArrowLeft, 13., theme::success())))
        .child(div().flex_1().h(px(1.5)).bg(theme::success()))
        .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).child(format!("{ms} ms")))
        .child(div().flex_1().h(px(1.5)).bg(theme::success()))
        .when(forward, |el| el.child(chrome::small_icon(IconName::ArrowRight, 13., theme::success())));
    let spoken = [(here, name, going), (name, here, back)].map(|(de, para, ms)| tr("machines_peer_measure").replace("{de}", de)
        .replace("{para}", para).replace("{ms}", &ms.to_string())).join(". ");
    div().id("machines-peer-ways").role(Role::Group).aria_label(spoken).flex().items_center().gap(px(10.))
        .child(node(here, theme::accent()))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(6.)).child(way(going, true)).child(way(back, false)))
        .child(node(name, theme::muted()))
}

/// A resposta de `/api/peers/anunciar` (ou os `avisos` de gravar o nome): uma linha por outra máquina.
pub(super) fn parse_announced(value: Option<&Value>) -> Option<Vec<(String, bool, String)>> {
    value?.as_array()?.iter().map(|a| Some((a.get("id")?.as_str()?.to_owned(), a.get("estado")?.as_str()? == "ok",
        a.get("motivo").and_then(Value::as_str).unwrap_or_default().to_owned()))).collect()
}

pub(super) fn parse_reach(value: &Value) -> Option<Reach> {
    // Uma linha fora do formato derruba a leitura inteira: sumir com ela mudaria o veredito sem aviso.
    let addresses = value.get("enderecos")?.as_array()?.iter().map(|e| Some(Address {
        kind: Kind::parse(e.get("tipo")?.as_str()?)?,
        url: e.get("url").and_then(Value::as_str).unwrap_or_default().to_owned(),
        status: match e.get("estado").and_then(Value::as_str)? { "ok" => Status::Ok, "falhou" => Status::Failed, "nao_configurado" => Status::Unset,
            _ => Status::Testing },
        ms: e.get("tempo_ms").and_then(Value::as_f64).map(|ms| ms.round() as i64),
    })).collect::<Option<Vec<_>>>()?;
    Some(Reach { loopback: value.get("loopback").and_then(Value::as_bool).unwrap_or(false),
        bind: value.get("bind").and_then(Value::as_str).unwrap_or_default().to_owned(), addresses })
}

impl Reach {
    /// Com CP_PUBLIC_URL apontando para o nome do Tailscale, as duas linhas são o mesmo endereço.
    fn public_same(&self) -> bool {
        let tailscale = self.addresses.iter().find(|a| a.kind == Kind::Tailscale).map(|a| a.url.as_str()).unwrap_or_default();
        !tailscale.is_empty() && self.addresses.iter().any(|a| a.kind == Kind::Public && a.url == tailscale)
    }
    fn main(&self, a: &Address) -> bool {
        matches!(a.kind, Kind::Lan | Kind::Tailscale) || (a.kind == Kind::Public && !self.public_same() && a.status != Status::Unset)
    }
    fn extras(&self) -> Vec<&Address> {
        self.addresses.iter().filter(|a| !self.main(a) && !(a.kind == Kind::Public && self.public_same())).collect()
    }
    fn fastest<'a>(mut list: impl Iterator<Item = &'a Address>) -> Option<&'a Address> {
        let first = list.next()?;
        Some(list.fold(first, |best, a| if a.ms.unwrap_or(0) < best.ms.unwrap_or(0) { a } else { best }))
    }
    /// O caminho de fora de casa mais rápido que respondeu.
    fn outside(&self) -> Option<&Address> {
        let same = self.public_same();
        Self::fastest(self.addresses.iter().filter(|a| a.status == Status::Ok && (a.kind == Kind::Tailscale || (a.kind == Kind::Public && !same))))
    }
    fn lan(&self) -> Option<&Address> { self.addresses.iter().find(|a| a.kind == Kind::Lan && a.status == Status::Ok) }
    /// Resumo do cartão: o endereço que responde mais rápido, fora "nesta máquina".
    fn summary(&self) -> (String, Light) {
        match Self::fastest(self.addresses.iter().filter(|a| a.status == Status::Ok && a.kind != Kind::Here)) {
            Some(best) => (format!("{} · {} ms", best.kind.name(), best.ms.unwrap_or(0)), Light::Ok),
            None => (tr("machines_loopback_short"), Light::No),
        }
    }
    /// A rede local fechada com o serviço escutando só em loopback é escolha da máquina, não defeito.
    fn by_choice(&self, a: &Address) -> bool { a.kind == Kind::Lan && a.status == Status::Failed && self.loopback }
    fn light(&self, a: &Address) -> Light {
        if self.by_choice(a) { return Light::Neutral; }
        match a.status { Status::Ok => Light::Ok, Status::Failed => Light::No, Status::Testing => Light::Test, Status::Unset => Light::Neutral }
    }
    fn phrase(&self, a: &Address) -> String {
        let time = format!("{} ms", a.ms.unwrap_or(0));
        match (a.status, a.kind) {
            (Status::Unset, _) => tr("machines_public_empty"),
            (Status::Testing, _) => tr("machines_testing"),
            (Status::Failed, Kind::Lan) if self.loopback && !self.bind.is_empty() => tr("machines_closed_loopback").replace("{endereco}", &self.bind),
            (Status::Failed, _) => tr("machines_failed"),
            (Status::Ok, Kind::Lan) => tr("machines_ok_wifi").replace("{tempo}", &time),
            (Status::Ok, Kind::Here) => tr("machines_ok_local"),
            (Status::Ok, _) => tr("machines_ok_4g").replace("{tempo}", &time),
        }
    }
}

/// Mesma regra do backend (`peers.validar_id`): minúsculas, números, hífen e sublinhado, até 32.
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.len() <= 32 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn id_hint() -> String { tr("machines_id_hint").replace("{exemplos}", "casa, notebook") }

/// Uma outra máquina do `peers.json` deste servidor, como `/api/peers` a devolve. O token vem mascarado e não é lido: o que
/// este aparelho usa para falar com ela é o da entrada guardada aqui.
#[derive(Clone, Debug)]
struct Peer { id: String, url: String, enabled: bool }

/// Por que uma entrada deste aparelho não disse o identificador (`MotivoSemId` do web): fora do ar e token recusado se
/// consertam de jeitos diferentes. `Empty` é "respondeu", com ou sem nome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason { Empty, NoAnswer, Token }

/// Uma linha por máquina (`LinhaMaquina` do web): as entradas deste aparelho que são ela e o registro dela no servidor.
#[derive(Clone, Debug)]
struct Line {
    key: String,
    name: String,
    ident: Option<String>,
    /// Da entrada principal; `None` é "ainda perguntando".
    reason: Option<Reason>,
    /// A entrada principal (a que responde, de preferência) e todas as que são a mesma máquina.
    entry: Option<ServerEntry>,
    entries: Vec<ServerEntry>,
    peer: Option<Peer>,
    /// É o servidor conectado: vira o cartão de cima, não uma linha.
    this: bool,
}

struct MachineRename {
    ids: Vec<String>,
    input: Entity<InputState>,
    _events: Subscription,
}

impl Line {
    /// Guardar o token troca a chave da mesma máquina; o detalhe aberto segue pelo identificador.
    fn open_key(&self) -> &str { self.ident.as_deref().unwrap_or(&self.key) }
}

/// Host e caminho, sem esquema (`hostDe` do web): atrás de um proxy por caminho, máquinas diferentes dividem o host.
fn host_key(url: &str) -> String {
    match url::Url::parse(url.trim()) {
        Ok(u) => format!("{}{}{}", u.host_str().unwrap_or_default(), u.port().map(|p| format!(":{p}")).unwrap_or_default(),
            u.path().trim_end_matches('/')).to_lowercase(),
        Err(_) => url.trim().trim_end_matches('/').to_lowercase(),
    }
}

/// `unirMaquinas` do web: a chave é o identificador que a entrada respondeu (ou o último lembrado); sem ele, o endereço igual
/// ao de um registro do servidor. Pela URL inteira a mesma máquina virava duas linhas (IP da rede aqui, Tailscale lá).
fn join_lines(servers: &[ServerEntry], ids: &HashMap<String, Option<String>>, reasons: &HashMap<String, Reason>, peers: &[Peer],
    active: &str) -> Vec<Line> {
    let mut groups: Vec<(String, Vec<&ServerEntry>)> = Vec::new();
    for s in servers {
        let key = ids.get(&s.id).cloned().flatten()
            .or_else(|| peers.iter().find(|p| host_key(&p.url) == host_key(&s.address)).map(|p| p.id.clone()))
            .unwrap_or_else(|| format!("srv:{}", s.id));
        match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, group)) => group.push(s), None => groups.push((key, vec![s])) }
    }
    let is_active = |s: &ServerEntry| servers::norm(&s.address) == active;
    let mut lines: Vec<Line> = groups.into_iter().map(|(key, group)| {
        let ident = (!key.starts_with("srv:")).then_some(key);
        let peer = ident.as_ref().and_then(|id| peers.iter().find(|p| &p.id == id)).cloned();
        let main = *group.iter().find(|s| is_active(s))
            .or_else(|| group.iter().find(|s| !s.disabled && reasons.get(&s.id) == Some(&Reason::Empty)))
            .or_else(|| group.iter().find(|s| !s.disabled)).unwrap_or(&group[0]);
        let name = if main.label.is_empty() { servers::default_label(&main.address) } else { main.label.clone() };
        Line { key: format!("srv:{}", main.id), name, ident, reason: reasons.get(&main.id).copied(), entry: Some(main.clone()),
            this: group.iter().any(|s| is_active(s)), entries: group.into_iter().cloned().collect(), peer }
    }).collect();
    for p in peers {
        if lines.iter().any(|l| l.peer.as_ref().is_some_and(|lp| lp.id == p.id)) { continue; }
        lines.push(Line { key: format!("peer:{}", p.id), name: p.id.clone(), ident: Some(p.id.clone()), reason: None, entry: None,
            entries: Vec::new(), peer: Some(p.clone()), this: false });
    }
    lines.sort_by(|a, b| b.this.cmp(&a.this).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    lines
}

fn parse_peers(value: &Value) -> Option<Vec<Peer>> {
    value.as_array()?.iter().map(|p| Some(Peer { id: p.get("id")?.as_str()?.to_owned(), url: p.get("base_url")?.as_str()?.to_owned(),
        enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(true) })).collect()
}

/// A ida (este servidor → ela), medida pelo servidor em `/api/peers/check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Way { Ok, Other, Failed, Unset }

#[derive(Clone, Debug)]
pub(super) struct Going {
    way: Way,
    answered_as: String,
    ms: Option<i64>,
    /// O pedido do teste falhou neste servidor (não é a outra máquina que não respondeu): a frase dele vai no cartão.
    error: Option<String>,
}

fn parse_going(value: &Value) -> Going {
    let way = match value.get("estado").and_then(Value::as_str) {
        Some("ok") => Way::Ok, Some("estranho") => Way::Other, Some("falhou" | "recusou") => Way::Failed, _ => Way::Unset,
    };
    Going { way, answered_as: value.get("identificador").and_then(Value::as_str).unwrap_or_default().to_owned(),
        ms: value.get("tempo_ms").and_then(Value::as_f64).map(|ms| ms.round() as i64), error: None }
}

impl Going {
    fn failed(error: String) -> Self { Going { way: Way::Failed, answered_as: String::new(), ms: None, error: Some(error) } }
}

/// A volta (ela → este servidor), medida com o token da entrada guardada aqui pelo endereço que o lado de lá guardou.
#[derive(Clone, Debug)]
pub(super) enum Back { NoToken, NoRegistration, Refused, Measured { going: Going, url: String } }

/// Medição de uma máquina, só em memória: cada abertura da página mede de novo.
#[derive(Default)]
struct Check { seq: u64, testing: bool, going: Option<Going>, back: Option<Back>, at: Option<chrono::DateTime<chrono::Local>> }

impl Check {
    fn done(&self) -> Option<&Going> { self.going.as_ref().filter(|_| !self.testing) }
    fn back(&self) -> Option<&Back> { self.back.as_ref().filter(|_| !self.testing) }
}

/// O que a linha da lista diz (`sessionsState` do web): se as sessões dela aparecem neste aparelho.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row { Shown, Testing, TokenRefused, NoToken, Off, OffHere, Silent }

fn row_state(line: &Line, check: Option<&Check>) -> Row {
    if line.peer.as_ref().is_some_and(|p| !p.enabled) { return Row::Off; }
    if !line.entries.is_empty() && line.entries.iter().all(|s| s.disabled) { return Row::OffHere; }
    if line.entry.is_some() {
        return match line.reason {
            None => Row::Testing, Some(Reason::Token) => Row::TokenRefused, Some(Reason::NoAnswer) => Row::Silent, Some(Reason::Empty) => Row::Shown,
        };
    }
    match check.and_then(Check::done) { None => Row::Testing, Some(g) if g.way == Way::Ok => Row::NoToken, Some(_) => Row::Silent }
}

/// Desligada no servidor ou sem resposta: vai para "Não respondem", como no web (`recolhida`).
fn collapsed(row: Row) -> bool { matches!(row, Row::Off | Row::Silent) }

/// O cartão dos recados (`recadosCard` do web).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Card {
    Testing, Ok, MissingToken, NoRegistration, TokenRefused, OneWay, BackOther, GoingFailed, GoingOther, Paused,
    // Sem registro no servidor: só a entrada deste aparelho.
    NoOwnId, NoTheirId, NoAnswer, Off,
}

fn card_state(line: &Line, check: Option<&Check>, own_id: &str) -> Card {
    let Some(peer) = &line.peer else {
        if own_id.is_empty() { return Card::NoOwnId; }
        if line.ident.is_some() { return Card::Off; }
        return match line.reason {
            Some(Reason::Token) => Card::TokenRefused, Some(Reason::Empty) => Card::NoTheirId, Some(Reason::NoAnswer) => Card::NoAnswer,
            None => Card::Testing,
        };
    };
    if !peer.enabled { return Card::Paused; }
    let Some(going) = check.and_then(Check::done) else { return Card::Testing };
    let back = check.and_then(Check::back);
    let back_way = match back { Some(Back::Measured { going, .. }) => Some(going.way), Some(Back::Refused) => Some(Way::Failed), _ => None };
    let broken = |way: Option<Way>| matches!(way, Some(Way::Failed | Way::Other));
    // A ordem do `estadoDaLinha`: o endereço torto vem antes do "só de ida", que engoliria ele.
    match back {
        Some(Back::Refused) => Card::TokenRefused,
        Some(Back::Measured { going: b, .. }) if b.way == Way::Other => Card::BackOther,
        _ if going.way == Way::Other => Card::GoingOther,
        _ if broken(Some(going.way)) || broken(back_way) => if going.way == Way::Ok { Card::OneWay } else { Card::GoingFailed },
        Some(Back::NoToken) => Card::MissingToken,
        Some(Back::NoRegistration) => Card::NoRegistration,
        Some(Back::Measured { going: b, .. }) if b.way == Way::Ok && going.way == Way::Ok => Card::Ok,
        _ => Card::Testing,
    }
}

/// Mede a volta como o `checarUm` do web: sem entrada aqui (ou sem o nome deste servidor) não há como.
async fn measure_back(saved: Option<ServerEntry>, own_id: String) -> Back {
    let Some(entry) = saved.filter(|_| !own_id.is_empty()) else { return Back::NoToken };
    let failed = |error: String| Back::Measured { going: Going::failed(error), url: String::new() };
    let api = match Api::new(&entry.address, &entry.token) { Ok(api) => api, Err(error) => return failed(Hangar::fetch_failure(&error)) };
    let list = match api.server_read(&["peers"], &[], 15).await {
        Ok(value) => match parse_peers(&value) { Some(list) => list, None => return failed(tr("invalid_response")) },
        // 401 é o token deste aparelho para ela recusado, não ela fora do ar.
        Err(error) if error.status == Some(401) => return Back::Refused,
        Err(error) => return failed(Hangar::fetch_failure(&error)),
    };
    match list.into_iter().find(|p| p.id == own_id) {
        None => Back::NoRegistration,
        Some(me) => Back::Measured { going: add::check(&api, &me.url, &own_id).await, url: me.url },
    }
}

/// Desfaz o registro deste servidor lá, com o token guardado aqui (`removerPeerDoisLados`). 404 é "já não estava".
async fn undo_there(entry: ServerEntry, own_id: String) -> bool {
    let Ok(api) = Api::new(&entry.address, &entry.token) else { return false };
    match api.server_send(reqwest::Method::DELETE, &["peers", &own_id], None, 15).await { Ok(_) => true, Err(error) => error.status == Some(404) }
}

/// De onde saiu uma gravação de outra máquina: a falha aparece só junto desse controle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot { Row, Messages, TurnOn, Scan, Footer }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeerWrite { Enabled(bool), Removed }

/// Como o reinício terminou, lido do estado que o motor grava (`fase: pronto`, casado pelo pid do pedido).
pub(super) enum RestartEnd { Done(Option<String>), Failed(Option<String>), Unconfirmed }
pub(super) enum UpgradeEnd { Done { ts: Option<String>, manual: bool, warnings: Vec<String> }, Failed(Option<String>), Unconfirmed }

/// O que a atualização terminou deixando pendente (`avisos` do estado), como o web lista em "algo ficou pendente".
fn update_warnings(state: &Value) -> Vec<String> {
    state["avisos"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim)
        .filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// `GET`/`PUT /api/web-ui`. Servidor anterior à rota (404/405) não oferece a escolha: `Ok(None)`, e a linha some.
fn web_ui_value(result: Result<Value, Failure>) -> Result<Option<bool>, String> {
    match result {
        Err(error) if matches!(error.status, Some(404 | 405)) => Ok(None),
        Err(error) => Err(Hangar::setting_failure(&error)),
        Ok(value) => value.get("web_ui").and_then(Value::as_bool).map(Some).ok_or_else(|| tr("invalid_response")),
    }
}

/// O modo de uma máquina migrada, do `GET /api/atualizacao`; `checkout` é a pasta que a troca oferece.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DevMode { developer: bool, channel: String, checkout: String }

/// `Ok(None)`: servidor sem a troca pela tela (sem `canal`, 404/405) ou máquina sem migrar (sem `modo` do Rust); a linha some.
fn dev_mode_value(result: Result<Value, Failure>) -> Result<Option<DevMode>, String> {
    let value = match result {
        Err(error) if matches!(error.status, Some(404 | 405)) => return Ok(None),
        Err(error) => return Err(Hangar::setting_failure(&error)),
        Ok(value) => value,
    };
    let developer = match value["modo"].as_str() { Some("pacote") => false, Some("desenvolvedor") => true, _ => return Ok(None) };
    let Some(channel) = value["canal"].as_str() else { return Ok(None) };
    Ok(Some(DevMode { developer, channel: channel.to_owned(), checkout: value["checkout"].as_str().unwrap_or_default().to_owned() }))
}

fn dev_mode_line(mode: &DevMode) -> String {
    if mode.developer { tr("dev_mode_developer").replace("{folder}", &mode.checkout) } else { tr("dev_mode_package").replace("{channel}", &mode.channel) }
}

/// Qual acompanhamento do executor destacado a resposta alimenta: o Atualizar ou a troca de modo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Track { Update, DevMode }

pub(super) enum MachinesReply {
    Reach(u64, Result<Value, Failure>),
    Id(u64, Result<Value, Failure>),
    IdSaved(u64, Result<Value, Failure>),
    Announced(u64, Result<Value, Failure>),
    Restart(u64, Result<Value, Failure>),
    RestartEnd(u64, RestartEnd),
    Upgrade(u64, Result<Value, Failure>),
    UpgradeStep(Track, u64, u64, u64, String),
    UpgradeEnd(Track, u64, UpgradeEnd),
    DevMode(u64, Result<Value, Failure>),
    DevSwitch(u64, Result<Value, Failure>),
    Peers(u64, Result<Value, Failure>),
    /// O identificador que uma entrada deste aparelho respondeu, pela geração da leitura.
    SavedId(u64, String, Result<Value, Failure>),
    PeerCheck(String, u64, Going, Back),
    /// Com as entradas deste aparelho que saem junto e se o lado de lá ficou com o registro.
    PeerSaved(u64, PeerWrite, Result<Value, Failure>, Vec<String>, bool),
    /// O token que o servidor guarda para os recados, testado e pronto para entrar neste aparelho.
    TokenAdopted(u64, String, String, Result<String, String>),
    /// Do diálogo Adicionar, pela entidade dele: a resposta de um diálogo já fechado não acha dono.
    Discovered(EntityId, u64, Result<Value, Failure>),
    Probed(EntityId, u64, Result<Found, String>),
    Registered(EntityId, u64, Result<(Going, Going), String>),
    Paired(u64, Result<Value, Failure>),
    WebUi(u64, Result<Value, Failure>),
    WebUiSaved(u64, Result<Value, Failure>),
}

#[derive(Default)]
struct Restart {
    seq: u64,
    asking: bool,
    waiting: bool,
    /// Hora em que o serviço novo respondeu.
    at: Option<String>,
    error: Option<String>,
    /// 409: o servidor respondeu recusando, com o motivo dele; não é travamento.
    refused: bool,
    task: Option<JoinHandle<()>>,
}

/// A atualização do servidor ativo pedida no cartão: o mesmo motor do "Atualizar" da página Sobre.
#[derive(Default)]
struct Upgrade {
    seq: u64,
    asking: bool,
    waiting: bool,
    step: Option<(u64, u64, String)>,
    at: Option<String>,
    manual: bool,
    warnings: Vec<String>,
    error: Option<String>,
    task: Option<JoinHandle<()>>,
}

impl Upgrade { fn busy(&self) -> bool { self.asking || self.waiting } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leave { SignOut, Remove }

#[derive(Default)]
pub(in crate::app) struct Machines {
    reach: Remote<Reach>,
    /// Identificador lido do servidor; sem leitura boa o campo fica desligado e nada é gravado.
    id: Remote<String>,
    id_input: Option<(Entity<InputState>, Subscription)>,
    id_saving: bool,
    id_seq: u64,
    id_error: Option<String>,
    id_saved: bool,
    announcing: bool,
    announce_seq: u64,
    /// Cada outra máquina avisada do nome desta: (nome, recebeu, motivo).
    announced: Option<Result<Vec<(String, bool, String)>, String>>,
    restart: Restart,
    upgrade: Upgrade,
    advanced: bool,
    /// O arquivo da conexão não saiu do disco: a ação não aconteceu. Cada ação mostra a sua falha junto do próprio botão.
    leave_error: Option<(Leave, String)>,
    /// As outras máquinas que este servidor conhece, e a medição de cada uma pelo id.
    peers: Remote<Vec<Peer>>,
    checks: HashMap<String, Check>,
    check_seq: u64,
    /// Uma gravação por vez: a máquina e o controle que a pediu. O `seq` descarta resposta de pedido velho.
    peer_busy: Option<(String, Spot)>,
    peer_seq: u64,
    peer_error: Option<(String, Spot, String)>,
    silent_open: bool,
    /// Identificador que cada entrada deste aparelho respondeu, e o último bom enquanto ela não responde; por `ServerEntry.id`.
    ids: HashMap<String, Option<String>>,
    reasons: HashMap<String, Reason>,
    ids_gen: u64,
    /// Leituras de identificador em voo: a medição da volta espera todas, senão nenhuma linha casaria com a entrada daqui.
    ids_pending: usize,
    /// Guardando aqui o token que o servidor usa para os recados de uma máquina (o id dela), e o erro de cada uma.
    adopting: Option<String>,
    adopt_seq: u64,
    adopt_error: Option<(String, String)>,
    /// A última remoção saiu daqui, mas o lado de lá não desfez o registro.
    far_failed: bool,
    /// A máquina do detalhe aberto (`Line::open_key`) e o Avançado dele.
    peer_open: Option<String>,
    peer_advanced: bool,
    rename: Option<MachineRename>,
    /// O diálogo Adicionar aberto.
    add: Option<Entity<AddMachine>>,
    pair: Pair,
    /// Janela estreita: o detalhe fica embaixo da lista, e escolher uma máquina rola até ele.
    stacked: bool,
    /// Tela web deste servidor; `Ok(None)` = servidor sem a rota. A gravação não mexe no valor até o servidor confirmar.
    web_ui: Remote<Option<bool>>,
    web_ui_saving: bool,
    web_ui_seq: u64,
    web_ui_error: Option<String>,
    /// Pacote ou modo desenvolvedor; `Ok(None)` = servidor sem a troca pela tela (só antes de [`Self::dev_mode_seen`]).
    dev_mode: Remote<Option<DevMode>>,
    /// Já houve uma leitura boa do modo neste servidor: a linha não some mais.
    dev_mode_seen: bool,
    dev_folder: Option<(Entity<InputState>, Subscription)>,
    /// A troca pedida, acompanhada como o Atualizar.
    dev_switch: Upgrade,
}

impl Drop for Machines {
    fn drop(&mut self) {
        if let Some(task) = self.restart.task.take() { task.abort(); }
        if let Some(task) = self.upgrade.task.take() { task.abort(); }
        if let Some(task) = self.dev_switch.task.take() { task.abort(); }
    }
}

impl Machines {
    fn track(&mut self, track: Track) -> &mut Upgrade { match track { Track::Update => &mut self.upgrade, Track::DevMode => &mut self.dev_switch } }
    /// Um executor por vez no servidor: reiniciar, atualizar e trocar de modo se excluem.
    fn executor_busy(&self) -> bool { self.restart.asking || self.restart.waiting || self.upgrade.busy() || self.dev_switch.busy() }

    /// Guarda a leitura do modo, se for do último pedido. Depois de uma leitura boa, "sem a troca"
    /// vira erro: a linha fica, para desligar um checkout de uma versão sem a troca pela tela.
    fn dev_mode_read(&mut self, seq: u64, result: Result<Value, Failure>) -> bool {
        let parsed = match dev_mode_value(result) {
            Ok(None) if self.dev_mode_seen => Err(tr("dev_mode_unknown_reason")),
            other => other,
        };
        let good = matches!(parsed, Ok(Some(_)));
        if !self.dev_mode.finish(seq, parsed) { return false; }
        self.dev_mode_seen |= good;
        true
    }

    /// Modo desconhecido depois de uma leitura boa: a linha mostra o motivo e oferece desligar.
    fn dev_mode_lost(&self) -> bool {
        self.dev_mode_seen && !self.dev_mode.loading && matches!(self.dev_mode.value, Some(Err(_)))
    }

    fn id_value(&self, cx: &App) -> String { self.id_input.as_ref().map(|(i, _)| i.read(cx).value().trim().to_owned()).unwrap_or_default() }
    fn id_changed(&self, cx: &App) -> bool { self.id.ok().is_some_and(|loaded| *loaded != self.id_value(cx)) }
    /// O identificador salvo no servidor (vazio enquanto não leu).
    fn id_loaded(&self) -> &str { self.id.ok().map(String::as_str).unwrap_or_default() }

    /// Falha na gravação deixa o valor de antes e mostra o motivo.
    fn web_ui_saved(&mut self, seq: u64, result: Result<Value, Failure>) {
        if seq != self.web_ui_seq { return; }
        self.web_ui_saving = false;
        match web_ui_value(result) {
            Ok(saved) => { self.web_ui.set(Ok(saved)); self.web_ui_error = None; }
            Err(error) => self.web_ui_error = Some(error),
        }
    }
}

impl Hangar {
    fn machines_send_later(&self) -> impl Fn(MachinesReply) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
        let (tx, connection) = (self.tx.clone(), self.connection);
        move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Machines(reply) }).await; })
        }
    }

    /// Página aberta: relê identificador e alcance. Como o web remonta a tela, o resultado de um reinício anterior sai.
    pub(super) fn machines_opened(&mut self, cx: &mut Context<Self>) {
        self.sync_updater(cx);
        let m = &mut self.machines;
        if let Some(task) = m.restart.task.take() { task.abort(); }
        m.restart = Restart { seq: m.restart.seq + 1, ..Restart::default() };
        // Atualização em curso segue na tela; o resultado de uma anterior sai, como o do reinício.
        if !m.upgrade.busy() { m.upgrade = Upgrade { seq: m.upgrade.seq + 1, ..Upgrade::default() }; }
        if !m.dev_switch.busy() { m.dev_switch = Upgrade { seq: m.dev_switch.seq + 1, ..Upgrade::default() }; }
        (m.id_saved, m.leave_error, m.peer_error, m.adopt_error, m.far_failed) = (false, None, None, None, false);
        m.announced = None;
        // "Não respondem" nasce fechado, como o `<details>` do web remontado; o painel volta a este servidor.
        (m.silent_open, m.peer_open, m.peer_advanced, m.rename) = (false, None, false, None);
        // Medições só em memória: cada abertura mede de novo, e a resposta de um teste de antes cai pelo `seq`.
        m.checks.clear();
        if !m.id_saving { self.load_machine_id(cx); }
        if !self.machines.web_ui_saving { self.load_web_ui(cx); }
        if !self.machines.dev_switch.busy() { self.load_dev_mode(cx); }
        self.load_reach(cx);
        self.load_saved_ids(cx);
        self.load_peers(cx);
        if let Some(updater) = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone()) {
            updater.update(cx, |updater, cx| updater.refresh_machine_updates(cx));
        }
    }

    /// Pergunta o identificador a cada entrada deste aparelho, com o token dela. Os lembrados ficam até a resposta: a lista abre
    /// já agrupada.
    fn load_saved_ids(&mut self, cx: &mut Context<Self>) {
        self.machines.ids_gen += 1;
        // Servidor de convite não tem `/api/peers`: perguntar levaria 403.
        self.machines.ids_pending = self.servers.iter().filter(|s| !s.invite).count();
        let generation = self.machines.ids_gen;
        for entry in self.servers.iter().filter(|s| !s.invite) {
            let (id, address, token, done) = (entry.id.clone(), entry.address.clone(), entry.token.clone(), self.machines_send_later());
            self.runtime.spawn(async move {
                let result = match Api::new(&address, &token) { Ok(api) => api.server_read(&["peers", "identificador"], &[], 15).await, Err(e) => Err(e) };
                done(MachinesReply::SavedId(generation, id, result)).await
            });
        }
        cx.notify();
    }

    /// As linhas da lista e do detalhe, refeitas de quem é dono de cada parte: as entradas deste aparelho e o registro do servidor.
    fn machine_lines(&self) -> Vec<Line> {
        let m = &self.machines;
        let active = self.server.as_deref().map(servers::norm).unwrap_or_default();
        join_lines(&self.servers, &m.ids, &m.reasons, m.peers.ok().map(Vec::as_slice).unwrap_or_default(), &active)
    }

    fn line_of_peer(&self, id: &str) -> Option<Line> {
        self.machine_lines().into_iter().find(|l| l.peer.as_ref().is_some_and(|p| p.id == id))
    }

    /// "Mostrar as sessões dele": desligar só esconde, a entrada e o token ficam aqui.
    fn follow_line(&mut self, entry_ids: &[String], on: bool) {
        for s in self.servers.iter_mut().filter(|s| entry_ids.contains(&s.id)) { s.disabled = !on; }
        self.servers_rev += 1;
        self.persist_servers();
        self.start_remote_lists();
    }

    /// Tira de vez as entradas deste aparelho de uma máquina.
    fn forget_entries(&mut self, entry_ids: &[String], cx: &mut Context<Self>) {
        self.servers.retain(|s| !entry_ids.contains(&s.id));
        for id in entry_ids { self.machines.ids.remove(id); self.machines.reasons.remove(id); }
        self.servers_rev += 1;
        self.persist_servers();
        self.start_remote_lists();
        // A entrada que levava um par externo some: o par volta na entrada só dele.
        self.apply_external_pairs(cx);
        self.sync_updater(cx);
    }

    /// Máquina que só o servidor conhecia: usa o token que ele guarda para os recados, testa, e respondendo ela entra neste
    /// aparelho com o nome que o servidor já usa (`informarToken` do web, sem token digitado).
    fn adopt_token(&mut self, peer_id: String, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let Some(url) = self.machines.peers.ok().and_then(|list| list.iter().find(|p| p.id == peer_id)).map(|p| p.url.clone()) else { return };
        let m = &mut self.machines;
        if m.adopting.is_some() { return; }
        m.adopt_seq += 1;
        (m.adopting, m.adopt_error) = (Some(peer_id.clone()), None);
        let (seq, done) = (m.adopt_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            let reason = |error: Failure| if error.status == Some(401) { tr("machines_short_token_refused") } else { Hangar::fetch_failure(&error) };
            let result = async {
                let value = api.server_read(&["peers", &peer_id, "token"], &[], 15).await.map_err(reason)?;
                let token = value.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| tr("invalid_response"))?.to_owned();
                let there = Api::new(&url, &token).map_err(reason)?;
                there.server_read(&["peers", "identificador"], &[], 15).await.map_err(reason)?;
                Ok(token)
            }.await;
            done(MachinesReply::TokenAdopted(seq, peer_id, url, result)).await
        });
        cx.notify();
    }

    fn load_peers(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.machines.peers.start();
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Peers(seq, api.server_read(&["peers"], &[], 15).await)).await });
        cx.notify();
    }

    /// Mede a ida de uma máquina de novo. Uma medição em voo não é refeita (clique duplo, abrir o detalhe enquanto a lista mede).
    fn check_peer(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let (saved, own_id) = (self.line_of_peer(id).and_then(|l| l.entry), self.machines.id_loaded().to_owned());
        let m = &mut self.machines;
        let Some(url) = m.peers.ok().and_then(|list| list.iter().find(|p| p.id == id && p.enabled)).map(|p| p.url.clone()) else { return };
        if m.checks.get(id).is_some_and(|c| c.testing) { return; }
        m.check_seq += 1;
        let seq = m.check_seq;
        let check = m.checks.entry(id.to_owned()).or_default();
        (check.seq, check.testing) = (seq, true);
        let (id, done) = (id.to_owned(), self.machines_send_later());
        self.runtime.spawn(async move {
            let (going, back) = tokio::join!(add::check(&api, &url, &id), measure_back(saved, own_id));
            done(MachinesReply::PeerCheck(id, seq, going, back)).await
        });
        cx.notify();
    }

    /// PUT `/api/peers/{id}/enabled` ou DELETE `/api/peers/{id}`; as duas respondem a lista nova. Remover leva junto as entradas
    /// deste aparelho e, com o token delas, o registro deste servidor lá.
    fn write_peer(&mut self, id: String, spot: Spot, write: PeerWrite, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let (entries, far) = match (write, self.line_of_peer(&id)) {
            (PeerWrite::Removed, Some(line)) => (line.entries.iter().map(|s| s.id.clone()).collect(), line.entry),
            _ => (Vec::new(), None),
        };
        let own_id = self.machines.id_loaded().to_owned();
        let m = &mut self.machines;
        if m.peer_busy.is_some() { return; }
        m.peer_seq += 1;
        (m.peer_busy, m.peer_error) = (Some((id.clone(), spot)), None);
        let (seq, done) = (m.peer_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            let result = match write {
                PeerWrite::Enabled(on) => api.server_send(reqwest::Method::PUT, &["peers", &id, "enabled"], Some(json!({"enabled": on})), 15).await,
                PeerWrite::Removed => api.server_send(reqwest::Method::DELETE, &["peers", &id], None, 15).await,
            };
            // Sem o nome deste servidor não há o que desfazer lá: o aviso diz que o registro de lá ficou.
            let far_failed = match far.filter(|_| result.is_ok()) {
                Some(entry) => own_id.is_empty() || !undo_there(entry, own_id).await,
                None => false,
            };
            done(MachinesReply::PeerSaved(seq, write, result, entries, far_failed)).await
        });
        cx.notify();
    }

    /// Remover pergunta antes, como o web, e a pergunta diz o que sai: só daqui, deste servidor, ou dos dois lados.
    fn confirm_line_removal(&mut self, line: &Line, spot: Spot, window: &mut Window, cx: &mut Context<Self>) {
        let title = tr("machines_peer_remove_title").replace("{nome}", &line.name);
        let text = tr(if line.peer.is_none() { "machines_remove_line_local" } else if line.entry.is_some() { "machines_remove_line" } else {
            "machines_peer_here_only" });
        let (peer, entries) = (line.peer.as_ref().map(|p| p.id.clone()), line.entries.iter().map(|s| s.id.clone()).collect::<Vec<_>>());
        let open_key = line.open_key().to_owned();
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, title, text, tr("machines_peer_remove"), ButtonVariant::Danger, move |window, cx| {
            match &peer {
                Some(id) => { let _ = this.update(cx, |this, cx| this.write_peer(id.clone(), spot, PeerWrite::Removed, cx)); true }
                // Só deste aparelho: sai na hora. O painel só deixa a máquina se era ela que estava aberta.
                None => {
                    let _ = this.update(cx, |this, cx| {
                        this.forget_entries(&entries, cx);
                        if this.machines.peer_open.as_deref() == Some(open_key.as_str()) { this.machines.peer_open = None; }
                        cx.notify();
                    });
                    if spot == Spot::Row { return true; }
                    // O botão focado saiu com o detalhe: fecha só a pergunta e devolve o foco à página, senão o Esc não chega.
                    window.close_dialog(cx);
                    let _ = this.update(cx, |this, cx| this.root_focus.focus(window, cx));
                    false
                }
            }
        });
    }

    /// O detalhe mora no painel ao lado da lista, como a Aparência: nada abre em diálogo.
    fn open_line_detail(&mut self, key: String, _: &mut Window, cx: &mut Context<Self>) {
        // Com uma gravação do detalhe no ar, trocar de máquina esconderia o erro dela; a da linha mostra o erro na própria linha.
        if self.detail_busy() { return; }
        // Abrir o detalhe mede de novo: o resultado de antes era de outra hora.
        let peer = self.machine_lines().into_iter().find(|l| l.open_key() == key).and_then(|l| l.peer);
        if let Some(peer) = peer { self.check_peer(&peer.id, cx); }
        let (m, id) = (&mut self.machines, key);
        (m.peer_open, m.peer_advanced, m.rename) = (Some(id.clone()), false, None);
        if m.peer_error.as_ref().is_some_and(|(_, spot, _)| *spot != Spot::Row) { m.peer_error = None; }
        if m.stacked { self.jump_to("machines_detail"); }
        cx.notify();
    }

    fn start_machine_rename(&mut self, ids: Vec<String>, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(label));
        let events = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) { this.save_machine_rename(cx); }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.machines.rename = Some(MachineRename { ids, input, _events: events });
        cx.notify();
    }

    fn save_machine_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.machines.rename.take() else { return };
        let typed = rename.input.read(cx).value().trim().to_owned();
        let Some(first) = self.servers.iter().find(|server| rename.ids.contains(&server.id)) else { cx.notify(); return };
        let label = if typed.is_empty() { servers::default_label(&first.address) } else { typed };
        for server in &mut self.servers {
            if rename.ids.contains(&server.id) { server.label = label.clone(); }
        }
        self.servers_rev += 1;
        self.persist_servers();
        cx.notify();
    }

    pub(super) fn cancel_machine_rename(&mut self) -> bool { self.machines.rename.take().is_some() }

    fn machine_rename_editor(&self, ids: &[String], cx: &mut Context<Self>) -> Option<AnyElement> {
        let rename = self.machines.rename.as_ref().filter(|rename| rename.ids == ids)?;
        Some(div().flex().items_center().gap(px(8.))
            .child(div().w(px(250.)).max_w_full().child(Input::new(&rename.input).small().aria_label(tr_shared("comum_nome", &[]))))
            .child(Button::new("machines-rename-save").primary().small().label(tr("server_save"))
                .on_click(cx.listener(|this, _, _, cx| this.save_machine_rename(cx))))
            .child(Button::new("machines-rename-cancel").ghost().small().label(tr("cancel"))
                .on_click(cx.listener(|this, _, _, cx| { this.machines.rename = None; cx.notify(); })))
            .into_any_element())
    }

    fn peer_error_at(&self, id: &str, spot: Spot) -> Option<String> {
        self.machines.peer_error.as_ref().filter(|(i, s, _)| i == id && *s == spot).map(|(.., error)| error.clone())
    }

    fn detail_busy(&self) -> bool { self.machines.peer_busy.as_ref().is_some_and(|(_, spot)| *spot != Spot::Row) }

    fn peer_busy_at(&self, id: &str, spot: Spot) -> bool {
        self.machines.peer_busy.as_ref().is_some_and(|(i, s)| i == id && *s == spot)
    }

    fn load_reach(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.machines.reach.start();
        let done = self.machines_send_later();
        // O servidor testa cada endereço antes de responder.
        self.runtime.spawn(async move { done(MachinesReply::Reach(seq, api.server_read(&["alcance"], &[], 30).await)).await });
        cx.notify();
    }

    fn load_machine_id(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.machines.id.start();
        self.machines.id_error = None;
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Id(seq, api.server_read(&["peers", "identificador"], &[], 15).await)).await });
        cx.notify();
    }

    fn load_web_ui(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.machines.web_ui.start();
        self.machines.web_ui_error = None;
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::WebUi(seq, api.server_read(&["web-ui"], &[], 15).await)).await });
        cx.notify();
    }

    fn save_web_ui(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let m = &mut self.machines;
        if m.web_ui_saving || m.web_ui.loading || m.web_ui.ok().copied().flatten().is_none_or(|now| now == on) { return; }
        m.web_ui_seq += 1;
        (m.web_ui_saving, m.web_ui_error) = (true, None);
        let (seq, done) = (m.web_ui_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            done(MachinesReply::WebUiSaved(seq, api.server_send(reqwest::Method::PUT, &["web-ui"], Some(json!({"web_ui": on})), 15).await)).await
        });
        cx.notify();
    }

    fn load_dev_mode(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        let seq = self.machines.dev_mode.start();
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::DevMode(seq, api.server_read(&["atualizacao"], &[], 20).await)).await });
        cx.notify();
    }

    /// A pasta sugerida entra no campo só enquanto ele está vazio: o que a pessoa digitou fica.
    fn show_dev_folder(&mut self, suggested: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.machines.dev_folder.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("dev_mode_folder_placeholder")));
            let sub = cx.subscribe_in(&input, window, |_: &mut Hangar, _, event: &InputEvent, _, cx| if let InputEvent::Change = event { cx.notify() });
            self.machines.dev_folder = Some((input, sub));
        }
        if let Some((input, _)) = &self.machines.dev_folder && input.read(cx).value().trim().is_empty() {
            let suggested = suggested.to_owned();
            input.update(cx, |state, cx| state.set_value(suggested, window, cx));
        }
    }

    fn dev_folder_value(&self, cx: &App) -> String {
        self.machines.dev_folder.as_ref().map(|(input, _)| input.read(cx).value().trim().to_owned()).unwrap_or_default()
    }

    fn confirm_dev_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let title = tr(if on { "dev_mode_confirm_on" } else { "dev_mode_confirm_off" }).replace("{server}", &self.server_label(cx));
        chrome::confirm_alert(window, cx, title, tr("dev_mode_confirm_desc"), tr(if on { "dev_mode_turn_on" } else { "dev_mode_turn_off" }),
            ButtonVariant::Primary, move |_, cx| { let _ = this.update(cx, |this, cx| this.start_dev_mode(on, cx)); true });
        // O interruptor volta ao valor lido até o servidor confirmar a troca.
        cx.notify();
    }

    fn start_dev_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| !self.active_invite()) else { return };
        if self.machines.executor_busy() { return; }
        let body = if on { json!({"on": true, "checkout": self.dev_folder_value(cx)}) } else { json!({"on": false}) };
        let d = &mut self.machines.dev_switch;
        d.seq += 1;
        (d.asking, d.step, d.at, d.manual, d.error) = (true, None, None, false, None);
        d.warnings.clear();
        let (seq, done) = (d.seq, self.machines_send_later());
        self.runtime.spawn(async move {
            done(MachinesReply::DevSwitch(seq, api.server_send(reqwest::Method::POST, &["atualizacao", "dev-mode"], Some(body), 30).await)).await
        });
        cx.notify();
    }

    /// Salvar é um botão, não sair do campo: o nome é como as outras máquinas chegam aqui, e vai para o .env.
    fn save_machine_id(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let value = self.machines.id_value(cx);
        let m = &mut self.machines;
        if m.id_saving || !m.id_changed(cx) { return; }
        if !value.is_empty() && !valid_id(&value) { m.id_error = Some(id_hint()); cx.notify(); return; }
        m.id_seq += 1;
        (m.id_saving, m.id_error, m.id_saved) = (true, None, false);
        let (seq, done) = (m.id_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            let body = json!({"identificador": value});
            done(MachinesReply::IdSaved(seq, api.server_send(reqwest::Method::PUT, &["peers", "identificador"], Some(body), 15).await)).await
        });
        cx.notify();
    }

    /// Regrava esta máquina em cada outra: nome, endereço e token de agora.
    fn announce_machine(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let m = &mut self.machines;
        if m.announcing { return; }
        m.announce_seq += 1;
        (m.announcing, m.announced) = (true, None);
        let (seq, done) = (m.announce_seq, self.machines_send_later());
        self.runtime.spawn(async move {
            done(MachinesReply::Announced(seq, api.server_send(reqwest::Method::POST, &["peers", "anunciar"], None, 60).await)).await
        });
        cx.notify();
    }

    fn undo_machine_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loaded = self.machines.id_loaded().to_owned();
        if let Some((input, _)) = &self.machines.id_input { input.update(cx, |state, cx| state.set_value(loaded, window, cx)); }
        self.machines.id_error = None;
        cx.notify();
    }

    fn restart_service(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let r = &mut self.machines.restart;
        if r.asking || r.waiting { return; }
        r.seq += 1;
        (r.asking, r.at, r.error, r.refused) = (true, None, None, false);
        let (seq, done) = (r.seq, self.machines_send_later());
        self.runtime.spawn(async move { done(MachinesReply::Restart(seq, api.server_post(&["atualizacao", "reiniciar"], 30).await)).await });
        cx.notify();
    }

    fn confirm_upgrade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let server = self.server_label(cx);
        chrome::confirm_alert(window, cx, tr("update_confirm_title").replace("{server}", &server), tr("update_confirm_desc"),
            tr("update_confirm_ok"), ButtonVariant::Primary,
            move |_, cx| { let _ = this.update(cx, |this, cx| this.start_upgrade(cx)); true });
    }

    fn start_upgrade(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.machines.upgrade.busy() || self.machines.restart.asking || self.machines.restart.waiting { return; }
        let u = &mut self.machines.upgrade;
        u.seq += 1;
        (u.asking, u.step, u.at, u.manual, u.error) = (true, None, None, false, None);
        u.warnings.clear();
        let (seq, done) = (u.seq, self.machines_send_later());
        self.runtime.spawn(async move { done(MachinesReply::Upgrade(seq, api.server_post(&["atualizacao", "iniciar"], 30).await)).await });
        cx.notify();
    }

    /// Como o reinício: o desfecho é o que o motor lançado por este pedido (o `pid` da resposta) gravou.
    fn wait_upgrade(&mut self, track: Track, pid: Option<i64>) {
        let Some(api) = self.api.clone() else {
            let u = self.machines.track(track);
            (u.waiting, u.error) = (false, Some(tr("update_silent")));
            return;
        };
        let (seq, done) = (self.machines.track(track).seq, self.machines_send_later());
        self.machines.track(track).task = Some(self.runtime.spawn(async move {
            let deadline = Instant::now() + UPGRADE_WAIT;
            let end = loop {
                if Instant::now() >= deadline { break UpgradeEnd::Unconfirmed; }
                tokio::time::sleep(RESTART_POLL).await;
                // Servidor caído no reinício do fim da atualização é o esperado: pergunta de novo.
                let Ok(value) = api.server_read(&["atualizacao"], &[], 10).await else { continue };
                let state = &value["estado"];
                let text = |k: &str| state[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                if state["fase"].as_str() == Some("rodando") {
                    let number = |k: &str| state[k].as_u64().unwrap_or(0);
                    done(MachinesReply::UpgradeStep(track, seq, number("passo"), number("total"), text("texto").unwrap_or_default())).await;
                    continue;
                }
                if state["pid"].as_i64() != pid || state["fase"].as_str() != Some("pronto") { continue; }
                break if state["ok"].as_bool() == Some(true) {
                    UpgradeEnd::Done { ts: text("ts"), manual: state["reiniciar_manual"].as_bool() == Some(true),
                        warnings: update_warnings(state) }
                } else { UpgradeEnd::Failed(text("erro")) };
            };
            done(MachinesReply::UpgradeEnd(track, seq, end)).await
        }));
    }

    /// Pedir não é reiniciar: quem diz que o serviço voltou é o estado gravado pelo motor que este pedido lançou.
    fn wait_restart(&mut self, pid: i64, request_id: Option<String>) {
        let Some(api) = self.api.clone() else {
            let r = &mut self.machines.restart;
            (r.waiting, r.error) = (false, Some(tr("machines_restart_unconfirmed")));
            return;
        };
        let (seq, done) = (self.machines.restart.seq, self.machines_send_later());
        self.machines.restart.task = Some(self.runtime.spawn(async move {
            let deadline = Instant::now() + RESTART_WAIT;
            let end = loop {
                if Instant::now() >= deadline { break RestartEnd::Unconfirmed; }
                tokio::time::sleep(RESTART_POLL).await;
                // Servidor caído no meio do reinício é o esperado: pergunta de novo.
                let Ok(value) = api.server_read(&["atualizacao"], &[], 10).await else { continue };
                let state = &value["estado"];
                if !crate::update::restart_result_matches(state, pid, request_id.as_deref()) || state["fase"].as_str() != Some("pronto") { continue; }
                let text = |k: &str| state[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                break if state["ok"].as_bool() == Some(true) { RestartEnd::Done(text("ts")) }
                    else { RestartEnd::Failed(text("reinicio_erro").or_else(|| text("erro"))) };
            };
            done(MachinesReply::RestartEnd(seq, end)).await
        }));
    }

    pub(super) fn receive_machines(&mut self, reply: MachinesReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            MachinesReply::Reach(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|v| parse_reach(&v).ok_or_else(|| tr("invalid_response")));
                self.machines.reach.finish(seq, parsed);
            }
            MachinesReply::Id(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e))
                    .and_then(|v| v.get("identificador").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response")));
                let loaded = parsed.as_ref().ok().cloned();
                if !self.machines.id.finish(seq, parsed) { return; }
                if let Some(loaded) = loaded { self.show_machine_id(loaded, window, cx); }
            }
            MachinesReply::IdSaved(seq, result) => {
                let m = &mut self.machines;
                if seq != m.id_seq { return; }
                m.id_saving = false;
                // O motivo do servidor, como o web; o texto de "entrega incerta" é do envio de mensagens.
                let result_avisos = result.as_ref().ok().and_then(|v| v.get("avisos")).cloned();
                match result.map_err(|e| Self::fetch_failure(&e))
                    .and_then(|v| v.get("identificador").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| tr("invalid_response"))) {
                    Ok(saved) => {
                        // Nome igual ao de antes não avisa ninguém: a lista vazia não diz nada e fica de fora.
                        m.announced = parse_announced(result_avisos.as_ref()).filter(|l| !l.is_empty()).map(Ok);
                        // Mais novo que qualquer leitura em voo: ela passa a ser descartada.
                        let newest = m.id.start();
                        m.id.finish(newest, Ok(saved.clone()));
                        m.id_saved = true;
                        self.show_machine_id(saved, window, cx);
                    }
                    // A recusa do backend ("identificador invalido…") chega como veio; o digitado fica no campo.
                    Err(error) => m.id_error = Some(error),
                }
            }
            MachinesReply::Announced(seq, result) => {
                let m = &mut self.machines;
                if seq != m.announce_seq { return; }
                m.announcing = false;
                m.announced = Some(result.map_err(|e| Self::fetch_failure(&e))
                    .and_then(|v| parse_announced(Some(&v)).ok_or_else(|| tr("invalid_response"))));
            }
            MachinesReply::Restart(seq, result) => {
                let r = &mut self.machines.restart;
                if seq != r.seq { return; }
                r.asking = false;
                match result {
                    Ok(value) => {
                        if let Some(pid) = crate::update::update_process_id(&value) {
                            r.waiting = true;
                            let request_id = value["request_id"].as_str().filter(|id| !id.is_empty()).map(str::to_owned);
                            self.wait_restart(pid, request_id);
                        } else { r.error = Some(tr("invalid_response")); }
                    }
                    Err(error) => {
                        r.refused = error.status == Some(409);
                        r.error = Some(Self::fetch_failure(&error));
                    }
                }
            }
            MachinesReply::RestartEnd(seq, end) => {
                let r = &mut self.machines.restart;
                if seq != r.seq { return; }
                (r.waiting, r.task) = (false, None);
                match end {
                    RestartEnd::Done(ts) => {
                        let local = ts.and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok()).map(|t| t.with_timezone(&chrono::Local))
                            .unwrap_or_else(chrono::Local::now);
                        r.at = Some(local.format("%H:%M:%S").to_string());
                    }
                    RestartEnd::Failed(error) => r.error = Some(error.unwrap_or_else(|| tr("machines_restart_failed"))),
                    RestartEnd::Unconfirmed => r.error = Some(tr("machines_restart_unconfirmed")),
                }
            }
            MachinesReply::Upgrade(seq, result) => {
                let u = &mut self.machines.upgrade;
                if seq != u.seq { return; }
                u.asking = false;
                match result {
                    Ok(value) => {
                        if let Some(pid) = crate::update::update_process_id(&value) {
                            u.waiting = true;
                            self.wait_upgrade(Track::Update, Some(pid));
                        } else { u.error = Some(tr("invalid_response")); }
                    }
                    Err(error) => u.error = Some(tr("update_refused").replace("{reason}", &Self::fetch_failure(&error))),
                }
            }
            MachinesReply::DevMode(seq, result) => {
                if !self.machines.dev_mode_read(seq, result) { return; }
                let suggested = self.machines.dev_mode.ok().cloned().flatten().map(|mode| mode.checkout);
                if let Some(suggested) = suggested { self.show_dev_folder(&suggested, window, cx); }
            }
            MachinesReply::DevSwitch(seq, result) => {
                if seq != self.machines.dev_switch.seq { return; }
                self.machines.dev_switch.asking = false;
                match result {
                    // Já estava no modo pedido: nada foi lançado, a linha relida mostra o modo.
                    Ok(value) if value["nada_a_fazer"].as_bool() == Some(true) => self.load_dev_mode(cx),
                    Ok(value) => match crate::update::update_process_id(&value) {
                        Some(pid) => { self.machines.dev_switch.waiting = true; self.wait_upgrade(Track::DevMode, Some(pid)); }
                        None => self.machines.dev_switch.error = Some(tr("invalid_response")),
                    },
                    Err(error) => self.machines.dev_switch.error = Some(Self::fetch_failure(&error)),
                }
            }
            MachinesReply::UpgradeStep(track, seq, step, total, text) => {
                let u = self.machines.track(track);
                if seq == u.seq && u.waiting { u.step = Some((step, total, text)); }
            }
            MachinesReply::UpgradeEnd(track, seq, end) => {
                let u = self.machines.track(track);
                if seq != u.seq { return; }
                (u.waiting, u.task, u.step) = (false, None, None);
                if let UpgradeEnd::Done { warnings, .. } = &end { u.warnings.clone_from(warnings); }
                let failed = if track == Track::DevMode { "dev_mode_failed" } else { "update_failed" };
                match end {
                    UpgradeEnd::Done { manual: true, .. } => u.manual = true,
                    UpgradeEnd::Done { ts, .. } => {
                        let local = ts.and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok()).map(|t| t.with_timezone(&chrono::Local))
                            .unwrap_or_else(chrono::Local::now);
                        u.at = Some(local.format("%H:%M:%S").to_string());
                    }
                    UpgradeEnd::Failed(error) => u.error = Some(tr(failed).replace("{reason}",
                        error.as_deref().map(|e| e.trim_end_matches('.')).unwrap_or("?"))),
                    UpgradeEnd::Unconfirmed => u.error = Some(tr("update_silent")),
                }
                if track == Track::DevMode { self.load_dev_mode(cx); }
                self.sync_updater(cx);
            }
            MachinesReply::Peers(seq, result) => {
                let parsed = result.map_err(|e| Self::failure(&e)).and_then(|v| parse_peers(&v).ok_or_else(|| tr("invalid_response")));
                if !self.machines.peers.finish(seq, parsed) { return; }
                self.peers_arrived(cx);
            }
            MachinesReply::SavedId(generation, entry, result) => {
                let m = &mut self.machines;
                if generation != m.ids_gen { return; }
                let (id, reason) = match result {
                    Ok(value) => (value.get("identificador").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned), Reason::Empty),
                    // Fora do ar ela não diz o nome: vale o último que respondeu, e a mesma máquina continua uma linha só.
                    Err(error) => (m.ids.get(&entry).cloned().flatten(), if error.status == Some(401) { Reason::Token } else { Reason::NoAnswer }),
                };
                m.ids.insert(entry.clone(), id);
                m.reasons.insert(entry, reason);
                m.ids_pending = m.ids_pending.saturating_sub(1);
                if m.ids_pending == 0 { self.peers_arrived(cx); }
            }
            MachinesReply::PeerCheck(id, seq, going, back) => {
                let Some(check) = self.machines.checks.get_mut(&id).filter(|c| c.seq == seq) else { return };
                (check.testing, check.going, check.back, check.at) = (false, Some(going), Some(back), Some(chrono::Local::now()));
            }
            MachinesReply::TokenAdopted(seq, peer_id, url, result) => {
                if seq != self.machines.adopt_seq { return; }
                self.machines.adopting = None;
                match result {
                    Ok(token) => {
                        self.merge_servers(vec![ServerEntry { id: servers::new_id(), label: peer_id.clone(), address: url.clone(), token, disabled: false, invite: false, lan: None, ephemeral: false }], cx);
                        // O nome já foi conferido no teste: a linha casa com o registro sem esperar outra leitura.
                        if let Some(entry) = self.servers.iter().find(|s| servers::norm(&s.address) == servers::norm(&url)) {
                            self.machines.ids.insert(entry.id.clone(), Some(peer_id.clone()));
                            self.machines.reasons.insert(entry.id.clone(), Reason::Empty);
                        }
                        // Com o token aqui a volta passa a ser medida.
                        self.check_peer(&peer_id, cx);
                    }
                    Err(error) => self.machines.adopt_error = Some((peer_id, error)),
                }
            }
            MachinesReply::PeerSaved(seq, write, result, entries, far_failed) => {
                let m = &mut self.machines;
                if seq != m.peer_seq { return; }
                let Some((id, spot)) = m.peer_busy.take() else { return };
                let parsed = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| parse_peers(&v).ok_or_else(|| tr("invalid_response")));
                match parsed {
                    Ok(list) => {
                        // A resposta é a lista nova do servidor: mais nova que qualquer leitura em voo.
                        let newest = m.peers.start();
                        m.peers.finish(newest, Ok(list));
                        // Religada: a medição de antes de desligar não vale para agora (sem isto o web fica em "Testando…").
                        if write == PeerWrite::Enabled(true) { m.checks.remove(&id); }
                        if write == PeerWrite::Removed {
                            m.checks.remove(&id);
                            m.far_failed = far_failed;
                            if !entries.is_empty() { self.forget_entries(&entries, cx); }
                            let m = &mut self.machines;
                            // A máquina aberta saiu (por onde for): o painel volta a este servidor, e a chave velha não reabre
                            // sozinha se ela voltar à lista.
                            if m.peer_open.as_deref() == Some(id.as_str()) {
                                m.peer_open = None;
                                // Pedida pelo detalhe, o botão focado saiu junto: sem isto o foco fica fora da árvore e o Esc não chega.
                                if matches!(spot, Spot::Footer | Spot::Messages) && !window.has_active_dialog(cx) { self.root_focus.focus(window, cx); }
                            }
                        }
                        self.peers_arrived(cx);
                    }
                    Err(error) => m.peer_error = Some((id, spot, error)),
                }
            }
            MachinesReply::Discovered(dialog, seq, result) => {
                let parsed = result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| add::parse_discovered(&v).ok_or_else(|| tr("invalid_response")));
                if let Some(add) = self.add_dialog(dialog) { add.update(cx, |add, cx| add.discovered(seq, parsed, cx)); }
            }
            MachinesReply::Probed(dialog, seq, result) => {
                // O diálogo não pode ler o Hangar daqui de dentro (ele está em atualização): o nome conhecido vai pronto.
                let known = result.as_ref().ok().and_then(|f| self.known_machine(f.id())).map(|k| k.label).filter(|l| !l.is_empty());
                if let Some(add) = self.add_dialog(dialog) { add.update(cx, |add, cx| add.probed(seq, result, known, window, cx)); }
            }
            MachinesReply::Registered(dialog, seq, result) => {
                // Gravou aqui: a lista já tem a máquina, mesmo que o outro lado tenha falhado.
                if result.is_ok() { self.load_peers(cx); }
                let Some(add) = self.add_dialog(dialog).filter(|add| add.read(cx).waiting(seq)) else { return };
                if result.as_ref().is_ok_and(|(going, back)| going.way == Way::Ok && back.way == Way::Ok) {
                    // Em voo o diálogo não fecha nem tem outro por cima: ele é o do topo.
                    self.machines.add = None;
                    window.close_dialog(cx);
                } else {
                    add.update(cx, |add, cx| add.registered(seq, result, cx));
                }
            }
            MachinesReply::Paired(seq, result) => self.paired(seq, result, window),
            MachinesReply::WebUi(seq, result) => { self.machines.web_ui.finish(seq, web_ui_value(result)); }
            MachinesReply::WebUiSaved(seq, result) => self.machines.web_ui_saved(seq, result),
        }
        cx.notify();
    }

    fn add_dialog(&self, dialog: EntityId) -> Option<Entity<AddMachine>> {
        self.machines.add.clone().filter(|add| add::owns(Some(add.entity_id()), dialog))
    }

    /// Foco que ficou sem dono na página (a linha saiu da lista, o botão sumiu): vai ao ancestral focável mais próximo que
    /// sobrou ou à raiz, onde o Esc fecha a página — o `fallbackFocus` do web.
    pub(super) fn machines_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings != Some(Page::Servers) { return; }
        window.focus_lost_restore_target(cx).unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    /// Lista nova: some a medição de quem saiu, e toda máquina ligada sem medição (ou religada agora) é medida.
    fn peers_arrived(&mut self, cx: &mut Context<Self>) {
        let list = self.machines.peers.ok().cloned().unwrap_or_default();
        self.machines.checks.retain(|id, _| list.iter().any(|p| &p.id == id));
        // A volta usa a entrada daqui que casa com o registro: medir antes dos identificadores chegarem dava "falta o token" sempre.
        if self.machines.ids_pending > 0 { return; }
        for peer in list.iter().filter(|p| p.enabled) {
            if self.machines.checks.get(&peer.id).is_none_or(|c| c.going.is_none() && !c.testing) { self.check_peer(&peer.id, cx); }
        }
    }

    /// O campo do identificador com o valor salvo no servidor.
    fn show_machine_id(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.machines.id_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("machines_id_placeholder")));
            let sub = cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
                // Só o que a pessoa digita apaga o "salvo" e o erro; o valor que chegou do servidor não.
                InputEvent::Change if this.machines.id_changed(cx) => { (this.machines.id_error, this.machines.id_saved) = (None, false); cx.notify(); }
                InputEvent::PressEnter { .. } => this.save_machine_id(cx),
                _ => {}
            });
            self.machines.id_input = Some((input, sub));
        }
        if let Some((input, _)) = &self.machines.id_input { input.update(cx, |state, cx| state.set_value(value, window, cx)); }
        cx.notify();
    }

    /// Sair e remover a última máquina: o endereço e o token guardados saem do disco e a conexão acaba. Falso quando o arquivo
    /// ficou; aí nada muda na tela além do aviso.
    fn forget_connection(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.machines.leave_error = None;
        let removed = saved_connection_path().map_or(Ok(()), |path| match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        });
        if removed.is_err() {
            let key = match leave { Leave::SignOut => "machines_sign_out_error", Leave::Remove => "machines_remove_error" };
            self.machines.leave_error = Some((leave, tr(key)));
            cx.notify();
            return false;
        }
        self.drop_connection(window, cx);
        (self.api, self.server, self.unsaved_connection) = (None, None, None);
        self.reset_device(window, cx);
        self.server_config.reconnected(String::new());
        // Voltar pede o token de novo, como no web.
        self.token.update(cx, |input, cx| input.set_value("", window, cx));
        self.close_settings(window, cx);
        self.open_connection(window, cx);
        true
    }

    fn confirm_leave(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) {
        let (title, description, ok) = if leave == Leave::Remove {
            (tr("machines_remove_title").replace("{nome}", &self.server_label(cx)),
                format!("{} {}", tr("machines_remove_token"), tr("machines_back_needs")), tr("machines_remove_ok"))
        } else {
            (tr("machines_sign_out_title"), tr("machines_back_needs"), tr("machines_sign_out"))
        };
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, title, description, ok, ButtonVariant::Danger, move |window, cx| {
            // Saiu: o detalhe e esta pergunta fecham juntos. Não saiu: o aviso aparece onde se clicou.
            let left = this.update(cx, |this, cx| this.forget_connection(leave, window, cx)).unwrap_or(false);
            if left { let _ = this.update(cx, |this, _| this.forget_question()); window.close_all_dialogs(cx); }
            !left
        });
    }

    fn open_machine_detail(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.detail_busy() { return; }
        self.machines.peer_open = None;
        if self.machines.stacked { self.jump_to("machines_detail"); }
        cx.notify();
    }

    /// O resumo do cartão deste servidor: o endereço que responde mais rápido, e o farol dele.
    fn reach_summary(&self) -> (String, Light) {
        match &self.machines.reach.value {
            _ if self.machines.reach.loading => (tr("machines_testing"), Light::Test),
            Some(Ok(reach)) => reach.summary(),
            Some(Err(error)) => (error.clone(), Light::No),
            None => (tr("machines_testing"), Light::Test),
        }
    }

    /// O detalhe da máquina escolhida fica num painel ao lado da lista; com a janela pequena demais para as duas colunas, ele
    /// desce para baixo da lista. `width` é a largura da janela.
    pub(super) fn render_machines(&mut self, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let width = f32::from(width);
        let wide = width >= 1320.;
        let list_w = (width >= 1100.).then_some(if wide { 400. } else { 320. });
        self.machines.stacked = list_w.is_none();
        let offline = self.api.is_none();
        // Os dois abrem diálogo: o foco volta a eles no Esc.
        let this = cx.entity().downgrade();
        let add = FocusOnClick { id: "machines-add".into(), button: Button::new("machines-add").outline().small().icon(IconName::Plus)
            .label(tr("machines_add_device")).disabled(offline), open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_add_machine(window, cx));
            }) };
        let this = cx.entity().downgrade();
        let pair = FocusOnClick { id: "machines-pair".into(), button: Button::new("machines-pair").primary().small().icon(IconName::Smartphone)
            .label(tr("machines_pair")).disabled(offline), open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_pair(window, cx));
            }) };
        let this = cx.entity().downgrade();
        // Não depende da conexão ativa: quem recebe pode ainda não ter servidor próprio ligado.
        let invite = FocusOnClick { id: "machines-invite".into(), button: Button::new("machines-invite").ghost().small().icon(IconName::Link)
            .label(crate::i18n::tr_shared("convite_colar_titulo", &[])), open: Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.open_invite_dialog(None, window, cx));
            }) };
        // Título em cima e botões embaixo, o topo de toda página das Configurações.
        let top = div().flex().flex_col().gap(px(14.))
            .child(div().flex().flex_col().gap(px(4.))
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Servers.title()))
                .child(div().text_size(px(13.5)).text_color(theme::muted()).whitespace_normal().child(tr("machines_subtitle"))))
            .child(div().flex().flex_wrap().items_center().gap(px(8.))
                // Consertar ou acrescentar agentes e Tailscale: o assistente roda na pasta já instalada.
                .when(super::setup::supported(), |el| el.child(Button::new("machines-setup").ghost().small().icon(IconName::Wrench)
                    .label(tr("setup_open_menu")).on_click(cx.listener(|this, _, window, cx| this.open_setup_from_menu(window, cx)))))
                .child(invite)
                .child(self.mark(div().rounded(px(8.)).child(add), "machines_search_tailscale"))
                .child(pair));
        if self.api.is_none() {
            return div().flex().flex_col().child(top).child(div().mt_4().text_sm().text_color(theme::muted()).child(tr("settings_offline")))
                .into_any_element();
        }
        // O painel mostra a máquina aberta; sem uma (ou se ela saiu da lista), este servidor.
        let open_line = self.machines.peer_open.clone()
            .and_then(|key| self.machine_lines().into_iter().find(|l| !l.this && l.open_key() == key));
        let detail = if open_line.is_some() { self.render_peer_detail(cx) } else { self.render_machine_detail(cx) };
        let this_selected = open_line.is_none();
        let open_key = open_line.as_ref().map(|l| l.open_key().to_owned());

        let m = &self.machines;
        let (summary, light) = self.reach_summary();
        let id = m.id_loaded().to_owned();
        let name = self.server_label(cx);
        let no_id = m.id.ok().is_some_and(String::is_empty);
        let update = cx.try_global::<crate::update::Handle>().and_then(|handle|
            self.api.as_ref().filter(|_| !self.active_invite()).and_then(|api| handle.0.read(cx).machine_update_text(api)));
        // O nome do botão substitui o conteúdo para o leitor de tela: o estado que o farol pinta vai junto.
        let spoken = [Some(tr("machines_open").replace("{nome}", &name)), (!id.is_empty()).then(|| id.clone()),
            Some(format!("{} · {summary}", tr("machines_this_server"))), no_id.then(|| tr("machines_no_id_short")), update.clone()]
            .into_iter().flatten().collect::<Vec<_>>().join(". ");
        // Buscar um ajuste: identificador e origens moram no detalhe, e a busca (como no web) só abre a página e aponta o cartão.
        let card_hit = self.search_hit("machines_id") || self.search_hit("server_term_origins");
        let fill = if card_hit || this_selected { theme::accent_dim() } else { transparent_black() };
        let card = Button::new("machines-this")
            .custom(ButtonCustomVariant::new(cx).color(fill).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h_auto().min_h(px(64.)).px(px(12.)).py(px(10.)).rounded(px(10.))
            .accessibility_label(spoken)
            .child(div().w_full().flex().items_center().gap(px(12.))
                .child(tile(chrome::small_icon(IconName::Server, 19., theme::accent_text()), 38., theme::accent_dim(), Some(light.dot())))
                .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(3.))
                    .child(div().flex().flex_wrap().items_center().gap(px(8.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                        .when(!id.is_empty(), |el| el.child(div().px(px(8.)).rounded_full().border_1().border_color(theme::border())
                            .font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted()).child(id.clone()))))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(summary))
                    .when_some(update, |el, text| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text)))
                    .when(no_id, |el| el.child(div().text_size(px(12.5)).text_color(theme::warning())
                        .child(tr("machines_no_id_short")))))
                .child(chrome::small_icon(IconName::ChevronRight, 16., if this_selected { theme::accent_text() } else { theme::muted() })));
        let this = cx.entity().downgrade();
        let card = FocusOnClick { id: "machines-this".into(), button: card, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.open_machine_detail(window, cx));
        }) };
        let card = self.mark(self.mark(div().rounded(px(10.)).child(card), "machines_id"), "server_term_origins");
        let this_group = settings_box()
            .child(section_head(IconName::Plug, tr("machines_this_server"), Some(tr("machines_this_server_lead")), None, px(16.)))
            .child(div().px(px(6.)).pb(px(6.)).child(card))
            .when_some(m.id.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.id.loading).cloned(), |el, error| el.child(div()
                .id("machines-id-read-error").role(Role::Alert).px_4().pb(px(12.)).text_size(px(12.5)).text_color(theme::danger()).child(error)));
        let sign_out_error = m.leave_error.clone().filter(|(leave, _)| *leave == Leave::SignOut).map(|(_, error)| error);
        let actions = div().flex().flex_col().gap(px(8.))
            .when_some(sign_out_error, |el, error| el.child(div().id("machines-sign-out-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).child(error)))
            .child(div().flex().items_center().justify_between()
                .child(self.mark(div().rounded(px(8.)).child(Button::new("machines-reconnect").ghost().small().icon(IconName::RefreshCw)
                    .label(tr("machines_reconnect")).on_click(cx.listener(|this, _, window, cx| this.connect(window, cx)))), "machines_reconnect"))
                .child(self.mark(div().rounded(px(8.)).child(Button::new("machines-sign-out").ghost().small().icon(IconName::LogOut)
                    .label(tr("machines_sign_out")).text_color(theme::danger())
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_leave(Leave::SignOut, window, cx)))), "machines_sign_out_title")));
        let list = div().flex().flex_col().gap(px(20.))
            .child(this_group)
            .child(self.render_peers(open_key.as_deref(), cx))
            .child(actions);
        // Fundo mais fundo que as caixas do detalhe: no mesmo tom elas sumiriam dentro do painel.
        let pane = self.mark(div().flex_1().min_w_0().p(px(22.)).rounded(px(16.)).border_1().border_color(theme::border())
            .bg(theme::inset()).child(detail), "machines_detail").id("machines-detail-pane");
        let body = match list_w {
            Some(w) => div().mt(px(24.)).flex().items_start().gap(px(24.)).child(div().w(px(w)).flex_shrink_0().child(list)).child(pane),
            None => div().mt(px(24.)).flex().flex_col().gap(px(24.)).child(list).child(pane),
        };
        div().flex().flex_col().child(top).child(body).into_any_element()
    }

    /// "Máquinas neste aparelho": as outras máquinas deste servidor, e as que não respondem recolhidas embaixo (ListaMaquinas.svelte).
    /// `open`: a máquina mostrada no painel ao lado, em destaque na lista.
    fn render_peers(&mut self, open: Option<&str>, cx: &mut Context<Self>) -> Div {
        let rows = self.machine_lines().into_iter().filter(|l| !l.this)
            .map(|l| { let row = row_state(&l, l.peer.as_ref().and_then(|p| self.machines.checks.get(&p.id))); (l, row) }).collect::<Vec<_>>();
        let m = &self.machines;
        let total = rows.len();
        let showing = rows.iter().filter(|(_, row)| *row == Row::Shown).count();
        let (silent, shown): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, row)| collapsed(*row));
        // A falha do registro do servidor não esconde as máquinas guardadas aqui: ela aparece em cima delas.
        let error = m.peers.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.peers.loading).cloned();
        let note = |text: String| div().px(px(10.)).py(px(12.)).text_size(px(12.5)).text_color(theme::muted()).child(text);
        let count = (total > 0).then(|| tr("machines_count").replace("{n}", &total.to_string()).replace("{m}", &showing.to_string()));
        let update = cx.try_global::<crate::update::Handle>().map(|handle| handle.0.clone())
            .and_then(|updater| updater.update(cx, |updater, cx| updater.render_machine_update_button(cx)));
        let head = self.mark(section_head(IconName::Server, tr("machines_others"), count, update, px(16.)), "machines_others");
        let rows = div().px(px(6.)).pb(px(6.)).flex().flex_col().gap(px(2.))
            .when_some(error, |el, error| el.child(div().id("machines-peers-error").role(Role::Alert).flex().items_center().gap(px(10.)).px(px(10.)).py(px(10.))
                .child(div().flex_1().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))
                .child(Button::new("machines-peers-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_peers(cx))))))
            .map(|el| if shown.is_empty() {
                el.child(note(if m.peers.loading { tr("server_loading") } else if !silent.is_empty() { tr("machines_peers_none_answering") }
                    else { tr("machines_peers_empty") }))
            } else {
                el.children(shown.iter().map(|(line, row)| self.peer_row(line, *row, false, open == Some(line.open_key()), cx)).collect::<Vec<_>>())
            });
        let list = div().flex().flex_col().gap(px(8.)).child(settings_box().child(head).child(rows))
            .when(m.far_failed, |el| el.child(div().id("machines-far-failed").role(Role::Status).px(px(6.)).text_size(px(12.5)).text_color(theme::warning())
                .whitespace_normal().child(tr("machines_remove_far_failed"))));
        let open_silent = m.silent_open;
        let this = cx.entity().downgrade();
        let silent_block = (!silent.is_empty()).then(|| div().mt(px(12.)).flex().flex_col().gap(px(8.))
            .child(div().flex().child(super::settings::Disclosure::new("machines-silent", open_silent,
                tr("machines_peers_silent").replace("{n}", &silent.len().to_string()), false)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.silent_open = open; cx.notify(); }); })))
            .when(open_silent, |el| el.child(settings_box().p(px(6.)).gap(px(2.)).children(silent.iter()
                .map(|(line, row)| self.peer_row(line, *row, true, open == Some(line.open_key()), cx)).collect::<Vec<_>>()))));
        div().flex().flex_col().child(list).children(silent_block)
    }

    /// Uma linha da lista: abre o detalhe. Recolhida ("não respondem"), ganha o Remover ao lado, como no web.
    fn peer_row(&self, line: &Line, row: Row, silent: bool, selected: bool, cx: &mut Context<Self>) -> Div {
        let (light, color, phrase) = match row {
            Row::Shown => (Light::Ok, theme::success(), tr("machines_row_shown")),
            Row::Testing => (Light::Test, theme::muted(), tr("machines_testing")),
            Row::TokenRefused => (Light::No, theme::danger(), tr("machines_row_token_refused")),
            Row::NoToken => (Light::Ok, theme::warning(), tr("machines_peer_no_token")),
            Row::Off => (Light::Neutral, theme::muted(), tr("machines_peer_off")),
            Row::OffHere => (Light::Neutral, theme::muted(), tr("machines_row_off_here")),
            Row::Silent => (Light::Neutral, theme::muted(),
                tr(if line.entry.is_some() { "machines_row_silent" } else { "machines_peer_silent" })),
        };
        // Sem token aqui a máquina responde, mas o farol é o aviso: a bolinha leva a cor da frase.
        let dot = if row == Row::NoToken { (theme::warning(), true) } else { light.dot() };
        let id = line.open_key().to_owned();
        let (name, label) = (line.name.clone(), line.name.clone());
        let chip = line.ident.clone().filter(|ident| *ident != name);
        let repeated = (line.entries.len() > 1).then(|| tr("machines_row_saved_times").replace("{n}", &line.entries.len().to_string()));
        let invite = line.entry.as_ref().is_some_and(|e| e.invite);
        let update = cx.try_global::<crate::update::Handle>().and_then(|handle|
            line.entries.iter().find_map(|entry| handle.0.read(cx).machine_entry_update_text(&entry.id)));
        let face = if invite { chrome::small_icon(IconName::Link, 17., theme::muted()).into_any_element() }
            else { div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(initial(&name)).into_any_element() };
        let key = SharedString::from(format!("machines-peer-{id}"));
        let button = Button::new(key.clone())
            .custom(ButtonCustomVariant::new(cx).color(if selected { theme::accent_dim() } else { transparent_black() })
                .foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h_auto().min_h(px(60.)).px(px(12.)).py(px(10.)).rounded(px(10.))
            .accessibility_label([Some(tr("machines_open").replace("{nome}", &name)), chip.clone(), Some(phrase.clone()), repeated.clone(), update.clone()]
                .into_iter().flatten().collect::<Vec<_>>().join(". "))
            .child(div().w_full().flex().items_center().gap(px(12.))
                .child(tile(face, 38., theme::raised(), Some(dot)))
                .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(3.))
                    .child(div().flex().flex_wrap().items_center().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).child(name))
                        .when_some(chip, |el, chip| el.child(div().px(px(8.)).rounded_full().border_1().border_color(theme::border())
                            .font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted()).child(chip))))
                    .child(div().text_size(px(12.5)).text_color(color).whitespace_normal().child(phrase))
                    .when_some(update, |el, text| el.child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text)))
                    .when_some(repeated, |el, text| el.child(div().text_size(px(12.5)).text_color(theme::warning()).child(text))))
                .when(!silent, |el| el.child(chrome::small_icon(IconName::ChevronRight, 16., if selected { theme::accent_text() } else { theme::muted() }))));
        let this = cx.entity().downgrade();
        let open_id = id.clone();
        let row_line = line.clone();
        let line = FocusOnClick { id: key.into(), button, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.open_line_detail(open_id.clone(), window, cx));
        }) };
        if !silent { return div().child(line); }
        let key = SharedString::from(format!("machines-peer-{id}-remove"));
        let removing = self.peer_busy_at(&id, Spot::Row);
        let remove = Button::new(key.clone()).ghost().small().label(tr(if removing { "machines_peer_removing" } else { "machines_peer_remove" }))
            .text_color(theme::danger())
            .accessibility_label(if removing { tr("machines_peer_removing") } else { tr("machines_peer_remove_aria").replace("{nome}", &label) })
            // Gravando a própria remoção: carregando, não desligado. O botão desligado larga o foco, e o Esc não sobe mais.
            .loading(removing).disabled(self.machines.peer_busy.is_some() && !removing);
        let this = cx.entity().downgrade();
        let remove = FocusOnClick { id: key.into(), button: remove, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.confirm_line_removal(&row_line, Spot::Row, window, cx));
        }) };
        div().flex().flex_col()
            .child(div().flex().items_center().child(div().flex_1().min_w_0().child(line)).child(div().pr(px(8.)).flex_shrink_0().child(remove)))
            .when_some(self.peer_error_at(&id, Spot::Row), |el, error| el.child(div().id(SharedString::from(format!("machines-peer-{id}-error")))
                .role(Role::Alert).px(px(12.)).pb(px(10.)).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
    }

    /// Detalhe de outra máquina (DetalheServidor.svelte): o que o aparelho guarda dela, os recados e o avançado.
    fn render_peer_detail(&mut self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let Some(line) = m.peer_open.as_ref().and_then(|key| self.machine_lines().into_iter().find(|l| !l.this && l.open_key() == key)) else {
            return div();
        };
        let check = line.peer.as_ref().and_then(|p| m.checks.get(&p.id));
        let own_id = m.id_loaded().to_owned();
        let (card, row) = (card_state(&line, check, &own_id), row_state(&line, check));
        let (here, name) = (self.server_label(cx), line.name.clone());
        let rename_ids = line.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>();
        let rename_editor = self.machine_rename_editor(&rename_ids, cx);
        let fill = |key: &str| tr(key).replace("{este}", &here).replace("{nome}", &name);
        let tested = if check.is_some_and(|c| c.testing) || card == Card::Testing || row == Row::Testing { tr("machines_testing") } else {
            match check.and_then(|c| c.at) {
                Some(at) => tr("machines_peer_tested_at").replace("{hora}", &at.format("%H:%M").to_string()),
                None => tr("machines_peer_tested_now"),
            }
        };
        let busy = m.peer_busy.is_some();
        let next = tr("settings_next_version");
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);
        let setting = |title: String, help: String| div().border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(12.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::MEDIUM).child(title)).child(muted(help)));
        let error = |id: &str, error: Option<String>| error.map(|error| div().id(SharedString::from(id.to_owned())).role(Role::Alert).px_4().pb(px(12.))
            .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error));
        let scope = || chip(tr("server_scope"), theme::muted(), theme::raised());
        let peer_id = line.peer.as_ref().map(|p| p.id.clone());
        let adopting = peer_id.is_some() && m.adopting == peer_id;
        let adopt_error = m.adopt_error.as_ref().filter(|(id, _)| Some(id) == peer_id.as_ref()).map(|(_, e)| e.clone()).filter(|_| !adopting);

        // Neste aparelho: desligar só esconde (a entrada e o token ficam); ligar sem entrada usa o token que o servidor guarda.
        let saved = line.entry.is_some();
        let legend = if adopting { tr("machines_testing") } else if !saved { tr("machines_peer_no_token_here") } else {
            tr(match row {
                Row::Shown => "machines_row_shown", Row::Testing => "machines_testing", Row::Silent => "machines_row_silent",
                Row::TokenRefused => "machines_row_token_refused", Row::NoToken => "machines_peer_no_token_here",
                Row::Off => "machines_row_shown", Row::OffHere => "machines_row_off_here",
            })
        };
        let entry_ids = line.entries.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        let adopt_id = peer_id.clone();
        let follow = Switch::new("machines-peer-follow-switch").checked(line.entries.iter().any(|s| !s.disabled) || adopting)
            .disabled(adopting || (!saved && adopt_id.is_none()))
            .accessibility_label(format!("{}. {legend}", tr("machines_peer_show_sessions")))
            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                if !entry_ids.is_empty() {
                    this.follow_line(&entry_ids, *on);
                    this.sync_updater(cx);
                    cx.notify();
                }
                else if let Some(id) = adopt_id.clone().filter(|_| *on) { this.adopt_token(id, cx); }
            }));
        let entries = line.entries.iter().map(|s| {
            let reason = match m.reasons.get(&s.id) {
                Some(Reason::NoAnswer) => format!(" · {}", tr("machines_row_silent")),
                Some(Reason::Token) => format!(" · {}", tr("machines_short_token_refused")),
                _ => String::new(),
            };
            div().border_t_1().border_color(theme::border()).px_4().py(px(10.)).flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(if s.label.is_empty() { servers::default_label(&s.address) } else { s.label.clone() }))
                .child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                    .child(format!("{}{reason}", s.address)))
        }).collect::<Vec<_>>();
        let device = settings_box().child(section_head(IconName::Monitor, tr("machines_peer_on_device"), None, None, px(16.))).child(setting(tr("machines_peer_show_sessions"), legend).child(div().flex_shrink_0().child(follow)))
            .children(error("machines-peer-adopt-error", adopt_error.clone()))
            .children(entries)
            .when(line.entries.len() > 1, |el| el.child(div().px_4().pb(px(12.)).text_size(px(12.5)).text_color(theme::warning()).whitespace_normal()
                .child(tr("machines_saved_explain").replace("{n}", &line.entries.len().to_string()))));

        // Recados: desligar é remover o registro, com a mesma pergunta do Remover. Ligar a partir daqui ainda não existe no app.
        let has_id = !own_id.is_empty();
        let registered = line.peer.is_some();
        let remove_line = line.clone();
        let messages_switch = Switch::new("machines-peer-messages").checked(registered).disabled(!registered || !has_id || busy)
            // Desligado sem identificador: o motivo vai no nome e embaixo da legenda.
            .accessibility_label(if !registered { format!("{}. {next}", fill("machines_peer_messages_title")) } else if has_id {
                fill("machines_peer_messages_title") } else { format!("{}. {}", fill("machines_peer_messages_title"), tr("machines_no_id_short")) })
            .on_click(cx.listener(move |this, on: &bool, window, cx| if !*on { this.confirm_line_removal(&remove_line, Spot::Messages, window, cx) }));
        let messages_switch = div().id("machines-peer-messages-slot").child(messages_switch)
            .when(!registered, |el| { let next = next.clone(); el.tooltip(move |window, cx| Tooltip::new(next.clone()).build(window, cx)) });
        let tone = match card {
            Card::Testing | Card::Off => theme::border(),
            Card::Ok => theme::success(),
            Card::OneWay | Card::MissingToken | Card::NoTheirId | Card::NoOwnId | Card::NoRegistration | Card::Paused => theme::warning(),
            _ => theme::danger(),
        };
        let phrase = |text: String| div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(text);
        let going = check.and_then(Check::done).cloned();
        let back = check.and_then(Check::back).cloned();
        let back_going = match &back { Some(Back::Measured { going, url }) => Some((going.clone(), url.clone())), _ => None };
        let both_ms = going.as_ref().is_some_and(|g| g.ms.is_some()) && back_going.as_ref().is_some_and(|(g, _)| g.ms.is_some());
        let adopt_peer = peer_id.clone();
        let turn_on = peer_id.clone();
        let result = div().id("machines-peer-result").role(Role::Status).p(px(12.)).rounded(px(10.)).border_1().border_color(tone).bg(theme::inset())
            .flex().flex_col().gap(px(8.))
            .map(|el| match card {
                Card::Testing => el.child(phrase(format!("◌ {}", tr("machines_peer_testing_both")))),
                Card::Ok => el.child(phrase(format!("✓ {}", tr("machines_peer_ok")))),
                Card::MissingToken => el.child(phrase(fill("machines_peer_missing_token"))).child(muted(fill("machines_peer_missing_token_p")))
                    .when(!saved, |el| el.child(div().flex().child(Button::new("machines-peer-use-token").primary().small()
                        .label(if adopting { tr("machines_testing") } else { fill("machines_peer_use_token") }).loading(adopting).disabled(adopting)
                        .on_click(cx.listener(move |this, _, _, cx| if let Some(id) = adopt_peer.clone() { this.adopt_token(id, cx) }))))),
                Card::NoRegistration => el.child(phrase(fill("machines_peer_no_registration"))).child(muted(fill("machines_peer_no_registration_p"))),
                Card::TokenRefused => el.child(phrase(fill("machines_peer_token_refused"))).child(muted(tr("machines_peer_token_refused_p"))),
                Card::OneWay => el.child(phrase(fill("machines_peer_one_way")))
                    .map(|el| match back_going.clone() {
                        Some((_, url)) if !url.is_empty() => el.child(muted(tr("machines_peer_back_tried").replace("{endereco}", &url))),
                        _ => el,
                    })
                    .when_some(back_going.as_ref().and_then(|(g, _)| g.error.clone()), |el, error| el.child(div().id("machines-peer-back-error")
                        .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))),
                Card::BackOther => el.child(phrase(fill("machines_peer_back_other"))).child(muted(tr("machines_peer_back_other_p")
                    .replace("{endereco}", back_going.as_ref().map(|(_, url)| url.as_str()).unwrap_or_default())
                    .replace("{outro}", back_going.as_ref().map(|(g, _)| g.answered_as.as_str()).unwrap_or_default()))),
                Card::GoingFailed => el.child(phrase(fill("machines_peer_going_failed"))).child(muted(tr("machines_peer_going_failed_p")))
                    .when_some(going.as_ref().and_then(|g| g.error.clone()), |el, error| el.child(div().id("machines-peer-check-error")
                        .text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))),
                Card::GoingOther => el.child(phrase(fill("machines_peer_going_other"))).child(muted(fill("machines_peer_going_other_p")
                    .replace("{endereco}", line.peer.as_ref().map(|p| p.url.as_str()).unwrap_or_default())
                    .replace("{outro}", going.as_ref().map(|g| g.answered_as.as_str()).unwrap_or_default()))),
                Card::Paused => el.child(phrase(tr("machines_peer_off"))).child(muted(tr("machines_peer_scan_legend")))
                    .child(div().flex().child(Button::new("machines-peer-turn-on").primary().small().label(tr("machines_peer_turn_on"))
                        .loading(peer_id.as_deref().is_some_and(|id| self.peer_busy_at(id, Spot::TurnOn))).disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| if let Some(id) = turn_on.clone() {
                            this.write_peer(id, Spot::TurnOn, PeerWrite::Enabled(true), cx) }))))
                    .when_some(peer_id.as_deref().and_then(|id| self.peer_error_at(id, Spot::TurnOn)), |el, error| el.child(div()
                        .id("machines-peer-turn-on-error").role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error))),
                Card::NoOwnId => el.child(phrase(fill("machines_peer_no_own_id"))).child(muted(fill("machines_peer_no_own_id_p"))),
                Card::NoTheirId => el.child(phrase(fill("machines_peer_no_their_id"))).child(muted(tr("machines_peer_no_their_id_p"))),
                Card::NoAnswer => el.child(phrase(fill("machines_peer_no_answer"))).child(muted(tr("machines_peer_no_answer_p"))),
                Card::Off => el,
            })
            .when_some(adopt_error.filter(|_| card == Card::MissingToken), |el, error| el.child(div().id("machines-peer-use-token-error")
                .role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
            // Os dois sentidos medidos viram o desenho de ida e volta; faltando um, cada medida é uma linha.
            .when_some(going.as_ref().zip(back_going.as_ref()).filter(|_| card == Card::Ok).and_then(|(g, (b, _))| g.ms.zip(b.ms)),
                |el, (go, back)| el.child(two_way(&here, &name, go, back)))
            // Desligada não é testada: a medida de antes de desligar não é o estado de agora.
            .when_some(going.as_ref().filter(|_| !(card == Card::Ok && both_ms)).filter(|g| g.way == Way::Ok && card != Card::Paused).and_then(|g| g.ms), |el, ms| el.child(muted(tr("machines_peer_measure")
                .replace("{de}", &here).replace("{para}", &name).replace("{ms}", &ms.to_string()))))
            .when_some(back_going.as_ref().filter(|_| !(card == Card::Ok && both_ms)).filter(|(g, _)| g.way == Way::Ok && card != Card::Paused).and_then(|(g, _)| g.ms), |el, ms| el.child(muted(
                tr("machines_peer_measure").replace("{de}", &name).replace("{para}", &here).replace("{ms}", &ms.to_string()))))
            .when_some(peer_id.clone().filter(|_| matches!(card, Card::Testing | Card::GoingFailed | Card::GoingOther | Card::Ok)), |el, id| {
                el.child(div().flex().child(Button::new("machines-peer-test").outline().small().label(tr("machines_peer_test_again"))
                    .disabled(card == Card::Testing).on_click(cx.listener(move |this, _, _, cx| this.check_peer(&id, cx)))))
            });
        let messages = settings_box()
            .child(section_head(IconName::MessageSquare, tr("machines_peer_messages"), None, Some(scope().flex_shrink_0().into_any_element()), px(16.)))
            .child(setting(fill("machines_peer_messages_title"), fill(if card == Card::Off { "machines_peer_messages_off" } else { "machines_peer_messages_legend" }))
                .child(div().flex_shrink_0().child(messages_switch)))
            .when(!has_id && registered, |el| el.child(div().px_4().pb(px(12.)).text_size(px(12.5)).text_color(theme::warning()).whitespace_normal()
                .child(tr("machines_no_id_short"))))
            .children(error("machines-peer-messages-error", peer_id.as_deref().and_then(|id| self.peer_error_at(id, Spot::Messages))))
            .when(card != Card::Off, |el| el.child(div().border_t_1().border_color(theme::border()).p(px(12.)).child(result)));

        // Avançado: tirar da varredura grava no servidor; os endereços de ida e de volta são os que cada lado guardou.
        let this = cx.entity().downgrade();
        let advanced_open = m.peer_advanced;
        let advanced = line.peer.clone().map(|peer| {
            let peer_id = peer.id.clone();
            let back_url = back_going.as_ref().map(|(_, url)| url.clone()).filter(|url| !url.is_empty());
            let address = |title: String, url: String| div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(url));
            div().flex().flex_col().gap(px(10.))
                .child(div().flex().child(super::settings::Disclosure::new("machines-peer-advanced", advanced_open, tr("machines_advanced"), false)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.peer_advanced = open; cx.notify(); }); })))
                .when(advanced_open, |el| el.child(settings_box()
                    .child(div().flex().items_center().gap(px(14.)).px_4().py(px(12.))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                            .child(div().flex().flex_wrap().items_center().gap(px(8.)).child(div().font_weight(FontWeight::MEDIUM).child(tr("machines_peer_scan")))
                                .child(scope()))
                            .child(muted(tr("machines_peer_scan_legend"))))
                        .child(div().flex_shrink_0().child(Switch::new("machines-peer-scan").checked(peer.enabled).disabled(busy)
                            .accessibility_label(tr("machines_peer_scan"))
                            .on_click(cx.listener(move |this, on: &bool, _, cx| this.write_peer(peer_id.clone(), Spot::Scan, PeerWrite::Enabled(*on), cx))))))
                    .children(error("machines-peer-scan-error", self.peer_error_at(&peer.id, Spot::Scan)))
                    .child(address(fill("machines_peer_going_url"), peer.url.clone()))
                    .children(back_url.map(|url| address(fill("machines_peer_back_url"), url)))))
        });

        let key = SharedString::from("machines-peer-remove");
        let removing = peer_id.as_deref().is_some_and(|id| self.peer_busy_at(id, Spot::Footer));
        let remove = Button::new(key.clone()).ghost().small().text_color(theme::danger())
            .child(div().flex().items_center().gap(px(8.))
                .child(tr(if removing { "machines_peer_removing" } else { "machines_peer_remove_machine" })).when(registered, |el| el.child(scope())))
            .accessibility_label(if removing { tr("machines_peer_removing") } else if registered { fill("machines_peer_remove_machine_aria") } else {
                tr("machines_peer_remove_aria").replace("{nome}", &name) })
            // Carregando, não desligado: com o foco nele o Esc tem de continuar chegando ao diálogo.
            .loading(removing).disabled(busy && !removing);
        let this = cx.entity().downgrade();
        let remove_line = line.clone();
        let remove = FocusOnClick { id: key.into(), button: remove, open: Rc::new(move |window, cx| {
            let _ = this.update(cx, |this, cx| this.confirm_line_removal(&remove_line, Spot::Footer, window, cx));
        }) };
        let footer = div().flex().flex_col().items_end().gap(px(6.))
            .children(peer_id.as_deref().and_then(|id| self.peer_error_at(id, Spot::Footer)).map(|error| div().id("machines-peer-remove-error")
                .role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error)))
            .child(remove);

        let chip_id = line.ident.clone().filter(|ident| *ident != name);
        let head_dot = match row {
            Row::Shown => (theme::success(), true), Row::TokenRefused => (theme::danger(), true), Row::NoToken => (theme::warning(), true),
            _ => (theme::muted(), false),
        };
        let face = if line.entry.as_ref().is_some_and(|e| e.invite) { chrome::small_icon(IconName::Link, 21., theme::muted()).into_any_element() }
            else { div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(initial(&name)).into_any_element() };
        div().flex().flex_col().gap(px(16.)).pb(px(8.))
            .child(div().mb(px(4.)).flex().items_center().gap(px(14.))
                .child(tile(face, 46., theme::raised(), Some(head_dot)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                    .child(div().flex().items_center().gap(px(8.)).child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                        .when_some(chip_id, |el, id| el.child(div().px(px(8.)).rounded_full().border_1().border_color(theme::border())
                            .font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted()).child(id))))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tested))))
                .when(!rename_ids.is_empty(), |el| el.child(Button::new("machines-peer-rename").ghost().small().label(tr("machines_rename"))
                    .accessibility_label(tr("machines_rename_aria").replace("{nome}", &name))
                    .on_click(cx.listener(move |this, _, window, cx| this.start_machine_rename(rename_ids.clone(), name.clone(), window, cx)))))
            .children(rename_editor)
            .child(device)
            .child(messages)
            .children(advanced)
            .child(footer)
    }

    fn render_machine_detail(&mut self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let id = m.id_loaded().to_owned();
        let rename_ids = self.servers.iter().filter(|entry| self.server.as_ref().is_some_and(|address| servers::norm(&entry.address) == servers::norm(address)))
            .map(|entry| entry.id.clone()).collect::<Vec<_>>();
        let rename_editor = self.machine_rename_editor(&rename_ids, cx);
        let muted = |text: String| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text);

        // Identificador: o CP_SERVER_ID do .env. Vazio, os outros servidores não conseguem registrar este.
        let changed = m.id_changed(cx);
        let id_ready = m.id.ok().is_some() && !m.id.loading;
        // O campo não tem descrição no kit: por que está desligado, ou o erro dele, vai no nome.
        let id_label = match (id_ready, m.id_saving, &m.id_error) {
            (false, ..) => format!("{}. {}", tr("machines_id"), tr("server_loading")),
            (_, true, _) => format!("{}. {}", tr("machines_id"), tr("server_saving")),
            (_, _, Some(error)) => format!("{}. {error}", tr("machines_id")),
            _ => tr("machines_id"),
        };
        // O título e o escopo moram no cabeçalho do cartão; a linha é a explicação e o campo.
        let id_row = div().flex().items_center().gap(px(12.))
            .child(div().flex_1().min_w_0().child(muted(if id.is_empty() { id_hint() } else { tr("machines_id_set").replace("{nome}", &id) })))
            .children(m.id_input.as_ref().map(|(input, _)| div().w(px(200.)).flex_shrink_0()
                .child(Input::new(input).small().font_family(theme::MONO).disabled(!id_ready || m.id_saving).aria_label(id_label))));
        let id_state = if changed || m.id_saving {
            Some(div().flex().items_center().gap(px(8.))
                .child(Button::new("machines-id-save").primary().small().label(tr(if m.id_saving { "server_saving" } else { "server_save" }))
                    .loading(m.id_saving).disabled(m.id_saving).on_click(cx.listener(|this, _, _, cx| this.save_machine_id(cx))))
                .child(Button::new("machines-id-undo").ghost().small().label(tr("server_undo")).disabled(m.id_saving)
                    .on_click(cx.listener(|this, _, window, cx| this.undo_machine_id(window, cx))))
                .into_any_element())
        } else if m.id_saved {
            Some(div().id("machines-id-saved").role(Role::Status).text_size(px(12.5)).text_color(theme::success()).child(tr("machines_id_saved"))
                .into_any_element())
        } else { None };
        let id_read_error = m.id.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.id.loading).cloned();
        // As outras máquinas guardam este nome, endereço e token; quando um deles muda, elas precisam ouvir de novo.
        let announce = (id_ready && !id.is_empty() && !changed && !m.id_saving).then(|| {
            let line = |key: &str, text: String, color: Hsla| div().id(SharedString::from(format!("machines-announce-{key}")))
                .text_size(px(12.5)).text_color(color).whitespace_normal().child(text);
            let lines: Vec<Stateful<Div>> = match &m.announced {
                None => Vec::new(),
                Some(Err(error)) => vec![line("error", error.clone(), theme::danger())],
                Some(Ok(list)) if list.is_empty() => vec![line("none", tr("machines_announce_none"), theme::muted())],
                Some(Ok(list)) => list.iter().map(|(peer, ok, why)| if *ok {
                    line(peer, tr("machines_announce_ok").replace("{maquina}", peer), theme::success())
                } else {
                    line(peer, tr("machines_announce_failed").replace("{maquina}", peer).replace("{erro}", why), theme::danger())
                }).collect(),
            };
            div().flex().flex_col().gap(px(4.))
                .child(div().flex().child(Button::new("machines-announce").outline().small().icon(IconName::RotateCw)
                    .label(tr(if m.announcing { "machines_announcing" } else { "machines_announce" })).loading(m.announcing).disabled(m.announcing)
                    .on_click(cx.listener(|this, _, _, cx| this.announce_machine(cx)))))
                .child(div().id("machines-announce-result").role(Role::Status).flex().flex_col().gap(px(2.)).children(lines))
        });
        let identifier = div().flex().flex_col().gap(px(8.))
            .when(id_ready && id.is_empty(), |el| el.child(muted(tr("machines_id_legend")))
                .child(div().text_size(px(12.5)).text_color(theme::warning()).whitespace_normal().child(tr("machines_id_unset"))))
            .child(id_row)
            .children(id_state)
            .children(announce)
            .when_some(m.id_error.clone(), |el, error| el.child(div().id("machines-id-error").role(Role::Alert).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(error)))
            // Sem leitura boa o campo não grava: o vazio de uma falha apagaria o nome que o servidor tem.
            .when_some(id_read_error, |el, error| el.child(div().flex().items_center().gap(px(10.))
                .child(div().id("machines-id-load-error").role(Role::Alert).flex_1().text_size(px(12.5)).text_color(theme::danger())
                    .whitespace_normal().child(error))
                .child(Button::new("machines-id-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_machine_id(cx))))));

        // Endereços medidos pelo servidor; enquanto a medida não chega, as duas linhas que todo servidor tem aparecem testando.
        let line = |reach: &Reach, a: &Address, n: usize| {
            let light = reach.light(a);
            let text_color = match light { Light::Ok => theme::success(), Light::No => theme::danger(), _ => theme::muted() };
            let copy = (a.status == Status::Ok && a.kind != Kind::Here).then(|| {
                let url = a.url.clone();
                Button::new(SharedString::from(format!("machines-copy-{n}"))).ghost().small().icon(IconName::Copy).label(tr("machines_copy"))
                    .accessibility_label(format!("{} {}", tr("machines_copy"), a.kind.name()))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(url.clone())))
            });
            let icon = match a.kind { Kind::Here => IconName::Monitor, Kind::Lan => IconName::Wifi, Kind::Tailscale | Kind::Public => IconName::Globe };
            let (fill, fg) = match light {
                Light::Ok => (theme::success().alpha(0.14), theme::success()),
                Light::No => (theme::danger().alpha(0.14), theme::danger()),
                _ => (theme::inset(), theme::muted()),
            };
            div().mt(px(-1.)).border_t_1().border_color(theme::border()).flex().items_center().gap(px(14.)).px_4().py(px(12.))
                .child(tile(chrome::small_icon(icon, 16., fg), 32., fill, None))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(a.kind.name()))
                    .child(div().font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).truncate()
                        .child(if a.status == Status::Unset { tr("machines_unset") } else { a.url.clone() }))
                    .child(div().text_size(px(12.5)).text_color(text_color).whitespace_normal().child(reach.phrase(a)))
                    .when(a.kind == Kind::Tailscale && reach.public_same(), |el| el.child(muted(tr("machines_public_same")))))
                .children(copy.map(|c| div().flex_shrink_0().child(c)))
        };
        let testing = Reach { addresses: [Kind::Lan, Kind::Public].map(|kind| Address { kind, url: String::new(), status: Status::Testing, ms: None })
            .to_vec(), ..Reach::default() };
        let reach = m.reach.ok().filter(|_| !m.reach.loading).cloned();
        let reach_error = m.reach.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.reach.loading).cloned();
        let addresses = settings_box().child(section_head(IconName::Wifi, tr("machines_addresses"), None, None, px(16.))).map(|el| match (&reach, &reach_error) {
            (_, Some(error)) => el.child(div().id("machines-reach-error").role(Role::Alert).flex().items_center().gap(px(10.)).px_4().py(px(12.))
                .child(div().flex_1().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(error.clone()))
                .child(Button::new("machines-reach-retry").outline().small().label(tr("server_retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_reach(cx))))),
            (Some(reach), None) => el.children(reach.addresses.iter().filter(|a| reach.main(a)).enumerate().map(|(n, a)| line(reach, a, n))
                .collect::<Vec<_>>()),
            (None, None) => el.children(testing.addresses.iter().enumerate().map(|(n, a)| line(&testing, a, n)).collect::<Vec<_>>()),
        });
        let verdict = reach.as_ref().map(|reach| {
            let outside = reach.outside();
            let lan = reach.lan();
            let point = |light: Light, bold: String, rest: String| div().flex().items_start().gap(px(8.)).text_size(px(13.))
                .child(div().w(px(14.)).flex_shrink_0().text_color(light.color()).child(light.glyph()))
                .child(div().flex_1().min_w_0().whitespace_normal().child(div().font_weight(FontWeight::SEMIBOLD).child(bold))
                    .when(!rest.is_empty(), |el| el.child(div().text_color(theme::muted()).child(rest))));
            let isolated = outside.is_none() && lan.is_none();
            let body = if isolated {
                div().flex().flex_col().gap(px(6.))
                    .child(point(Light::No, tr("machines_verdict_none"), tr("machines_verdict_none_why").replace("{endereco}", &reach.bind)))
                    .child(muted(tr("machines_verdict_way_out").replace("{variavel}", "CP_LAN_BIND_IP").replace("{valor}", "auto")))
            } else {
                let out_rest = match outside {
                    Some(a) => tr("machines_verdict_out_how").replace("{rede}", &a.kind.name()).replace("{tempo}", &format!("{} ms", a.ms.unwrap_or(0))),
                    None => tr("machines_verdict_out_no_why"),
                };
                let lan_rest = match (lan, outside) {
                    (None, Some(a)) => tr("machines_verdict_lan_no_ok").replace("{rede}", &a.kind.name()),
                    _ => String::new(),
                };
                // Um cartão por caminho: quem alcança de fora e quem alcança no Wi-Fi, cada um com o sim ou não bem à vista.
                let card = |icon: IconName, title: &str, ok: bool, rest: String| div().flex_1().min_w_0().p(px(14.)).rounded(px(12.)).border_1()
                    .border_color(if ok { theme::success().alpha(0.35) } else { theme::border() })
                    .bg(if ok { theme::success().alpha(0.08) } else { theme::inset() })
                    .flex().flex_col().gap(px(6.))
                    .child(div().flex().items_center().gap(px(8.))
                        .child(chrome::small_icon(icon, 15., theme::muted()))
                        .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(theme::muted()).child(tr(title)))
                        .child(if ok { chrome::small_icon(IconName::Check, 16., theme::success()).into_any_element() }
                            else { div().size(px(10.)).rounded_full().border_2().border_color(theme::muted()).into_any_element() }))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD)
                        .child(tr(if ok { "machines_verdict_works" } else { "machines_verdict_no_reach" })))
                    .when(!rest.is_empty(), |el| el.child(muted(rest)));
                div().flex().flex_col().gap(px(10.))
                    .child(div().flex().gap(px(12.))
                        .child(card(IconName::Globe, "machines_verdict_out_title", outside.is_some(), out_rest))
                        .child(card(IconName::Wifi, "machines_verdict_lan_title", lan.is_some(), lan_rest)))
                    .when(lan.is_none() && reach.loopback, |el| el.child(muted(tr("machines_verdict_want_lan")
                        .replace("{variavel}", "CP_LAN_BIND_IP").replace("{valor}", "auto"))))
            };
            settings_box().when(isolated, |el| el.border_color(theme::danger()))
                .child(section_head(IconName::Globe, tr("machines_verdict"), None, None, px(16.)))
                .child(div().px_4().pb_4().child(body))
        });

        // Reiniciar não é avançado: é o gesto que faz valer o identificador e as outras chaves do .env.
        let r = &m.restart;
        let u = &m.upgrade;
        let busy = m.executor_busy();
        let outdated = cx.try_global::<crate::update::Handle>().map(|h| h.0.read(cx)).filter(|u| u.server_outdated())
            .map(|u| { let (running, app) = u.outdated_versions(); tr("machines_server_outdated").replace("{running}", &running).replace("{app}", &app) });
        let upgrade_status = if u.waiting {
            Some((u.step.as_ref().filter(|(_, total, _)| *total > 0).map(|(step, total, text)| tr("update_step")
                .replace("{step}", &step.to_string()).replace("{total}", &total.to_string()).replace("{text}", text))
                .unwrap_or_else(|| tr("update_running")), theme::muted()))
        } else if let Some(at) = &u.at { Some((tr("machines_updated").replace("{hora}", at), theme::success())) }
        else if u.manual { Some((tr("update_done_manual"), theme::warning())) } else { None };
        let upgrade_error = u.error.clone();
        let upgrade_warnings = (!u.busy() && !u.warnings.is_empty()).then(|| u.warnings.clone());
        // A ponte do app do computador só existe no Electron; o web a mostra para o servidor desta máquina, depois de um erro
        // que não é recusa.
        let local = self.address.read(cx).value().trim().parse::<url::Url>().ok()
            .and_then(|u| u.host_str().map(|h| matches!(h, "localhost" | "[::1]" | "::1") || h.starts_with("127.")))
            .unwrap_or(false);
        let desktop_note = tr("settings_next_version");
        let service = div().flex().flex_col().gap(px(8.))
            .when_some(outdated, |el, text| el.child(div().id("machines-server-outdated").role(Role::Status).flex().items_start().gap(px(6.))
                .text_size(px(12.5)).text_color(theme::warning())
                .child(div().pt(px(2.)).flex_shrink_0().child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme::warning())))
                .child(div().flex_1().min_w_0().whitespace_normal().child(text))))
            .child(muted(tr("machines_service_help")))
            .child(div().flex().flex_wrap().items_center().gap(px(10.))
                .child(Button::new("machines-restart").primary().small().icon(IconName::RotateCw)
                    .label(tr(if r.asking || r.waiting { "machines_restarting" } else { "machines_restart" })).loading(r.asking || r.waiting)
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_service(cx))))
                .child(Button::new("machines-update").outline().small().icon(IconName::Download)
                    .label(tr(if u.busy() { "update_running" } else { "update_confirm_ok" })).loading(u.busy()).disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_upgrade(window, cx))))
                .when(r.waiting, |el| el.child(div().id("machines-restart-waiting").role(Role::Status).text_size(px(12.5))
                    .text_color(theme::muted()).child(tr("machines_restart_waiting"))))
                .when_some(r.at.clone(), |el, at| el.child(div().id("machines-restarted").role(Role::Status).text_size(px(12.5))
                    .text_color(theme::success()).child(tr("machines_restarted").replace("{hora}", &at)))))
            .when_some(upgrade_status, |el, (text, color)| el.child(div().id("machines-update-status").role(Role::Status)
                .text_size(px(12.5)).text_color(color).whitespace_normal().child(text)))
            .when_some(upgrade_warnings, |el, warnings| el.child(div().id("machines-update-warnings").role(Role::Status)
                .flex().flex_col().gap(px(4.)).text_size(px(12.5)).text_color(theme::warning())
                .child(div().flex().items_start().gap(px(6.))
                    .child(div().pt(px(2.)).flex_shrink_0().child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme::warning())))
                    .child(div().flex_1().min_w_0().whitespace_normal().child(tr_shared("atualizar_com_avisos", &[]))))
                .children(warnings.into_iter().map(|w| div().pl(px(20.)).whitespace_normal().text_color(theme::text())
                    .child(format!("• {w}"))))))
            .when_some(upgrade_error, |el, error| el.child(div().id("machines-update-error").role(Role::Alert).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .when_some(r.error.clone(), |el, error| el.child(div().id("machines-restart-error").role(Role::Alert).text_size(px(12.5))
                    .text_color(theme::danger()).whitespace_normal().child(error))
                .when(!r.refused, |el| el.child(muted(tr("machines_restart_stuck"))))
                .when(!r.refused && local, |el| el.child(div().flex().items_center().gap(px(8.))
                    .child(Button::new("machines-restart-desktop").outline().small().label(tr("machines_restart_desktop")).disabled(true)
                        .accessibility_label(format!("{}. {desktop_note}", tr("machines_restart_desktop"))))
                    .child(muted(desktop_note.clone())))));

        // Convidado não escolhe; servidor sem a rota (`Ok(None)`) não oferece a escolha.
        let web_ui = (!self.active_invite() && !matches!(m.web_ui.value, Some(Ok(None)))).then(|| {
            let on = m.web_ui.ok().copied().flatten();
            let read_error = m.web_ui.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.web_ui.loading).cloned();
            let status = match on {
                _ if m.web_ui_saving => tr("server_saving"),
                Some(on) => format!("{} · {}", tr(if on { "web_ui_on" } else { "web_ui_off" }), tr("web_ui_help")),
                None if read_error.is_none() => tr("server_loading"),
                None => tr("web_ui_help"),
            };
            let toggle = Switch::new("machines-web-ui").checked(on.unwrap_or(false)).accessibility_label(tr("web_ui_title"))
                .disabled(on.is_none() || m.web_ui.loading || m.web_ui_saving)
                .on_click(cx.listener(|this, on: &bool, _, cx| this.save_web_ui(*on, cx)));
            let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).flex_1().text_size(px(12.5)).text_color(theme::danger())
                .whitespace_normal().child(text);
            settings_box().child(self.row_with(IconName::Globe, "web_ui_title", div().whitespace_normal().child(status), on.is_some(), toggle.into_any_element()))
                .when_some(read_error, |el, error| el.child(div().flex().items_center().gap(px(10.)).px_4().pb_4()
                    .child(alert("machines-web-ui-load-error", error))
                    .child(Button::new("machines-web-ui-retry").outline().small().label(tr("server_retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_web_ui(cx))))))
                .when_some(m.web_ui_error.clone(), |el, error| el.child(div().px_4().pb_4().child(alert("machines-web-ui-error", error))))
        });

        // Convidado não troca; servidor sem a troca ou máquina sem migrar (`Ok(None)`) não oferece.
        let dev_mode = (!self.active_invite() && !matches!(m.dev_mode.value, Some(Ok(None)))).then(|| {
            let mode = m.dev_mode.ok().cloned().flatten();
            let read_error = m.dev_mode.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !m.dev_mode.loading).cloned();
            // Sem o modo depois de uma leitura boa: o interruptor aparece ligado e só desliga.
            let lost = m.dev_mode_lost();
            let d = &m.dev_switch;
            let status = match &mode {
                _ if d.busy() => d.step.as_ref().filter(|(_, total, _)| *total > 0).map(|(step, total, text)| tr("update_step")
                    .replace("{step}", &step.to_string()).replace("{total}", &total.to_string()).replace("{text}", text))
                    .unwrap_or_else(|| tr("dev_mode_switching")),
                _ if lost => tr("dev_mode_unknown"),
                Some(mode) => dev_mode_line(mode),
                None if read_error.is_none() => tr("server_loading"),
                None => tr("dev_mode_help"),
            };
            let developer = mode.as_ref().is_some_and(|mode| mode.developer);
            // Para ligar é preciso a pasta; desligar volta para o pacote instalado.
            let folder = (mode.is_some() && !developer).then(|| m.dev_folder.as_ref().map(|(input, _)| input.clone())).flatten();
            let no_folder = folder.is_some() && self.dev_folder_value(cx).is_empty();
            let toggle = Switch::new("machines-dev-mode").checked(developer || lost).accessibility_label(tr("dev_mode_title"))
                .disabled((mode.is_none() && !lost) || m.dev_mode.loading || busy || no_folder)
                .on_click(cx.listener(|this, on: &bool, window, cx| this.confirm_dev_mode(*on, window, cx)));
            let done = if d.manual { Some((tr("dev_mode_done_manual"), theme::warning())) }
                else { d.at.as_ref().map(|_| (tr("dev_mode_done"), theme::success())) };
            let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).flex_1().text_size(px(12.5)).text_color(theme::danger())
                .whitespace_normal().child(text);
            settings_box().child(self.row_with(IconName::SquareTerminal, "dev_mode_title", div().whitespace_normal().child(status), mode.is_some() || lost,
                    toggle.into_any_element()))
                .when_some(folder, |el, input| el.child(div().px_4().pb_4().flex().flex_col().gap(px(6.))
                    .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("dev_mode_folder")))
                    .child(Input::new(&input).small().disabled(busy).aria_label(tr("dev_mode_folder")))))
                .when(!d.busy() && mode.is_some(), |el| el.child(div().px_4().pb_4().child(muted(tr("dev_mode_help")))))
                .when_some(done.filter(|_| !d.busy()), |el, (text, color)| el.child(div().id("machines-dev-mode-done").role(Role::Status)
                    .px_4().pb_4().text_size(px(12.5)).text_color(color).whitespace_normal().child(text)))
                .when_some(read_error, |el, error| el.child(div().flex().items_center().gap(px(10.)).px_4().pb_4()
                    .child(alert("machines-dev-mode-load-error", error))
                    .child(Button::new("machines-dev-mode-retry").outline().small().label(tr("server_retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_dev_mode(cx))))))
                .when_some(d.error.clone(), |el, error| el.child(div().px_4().pb_4().child(alert("machines-dev-mode-error", error))))
        });

        let this = cx.entity().downgrade();
        let advanced_open = m.advanced;
        let advanced = div().flex().flex_col().gap(px(10.))
            .child(div().flex().child(super::settings::Disclosure::new("machines-advanced", advanced_open, tr("machines_advanced"), false)
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.machines.advanced = open; cx.notify(); }); })))
            .when(advanced_open, |el| {
                let extras = reach.as_ref().map(|reach| reach.extras().into_iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
                let bind = reach.as_ref().map(|r| r.bind.clone()).filter(|b| !b.is_empty());
                el.when(!extras.is_empty(), |el| {
                    let reach = reach.clone().unwrap_or_default();
                    el.child(settings_box().children(extras.iter().enumerate().map(|(n, a)| line(&reach, a, 100 + n)).collect::<Vec<_>>()))
                })
                .when_some(bind, |el, bind| el.child(muted(tr("machines_listening").replace("{ip}", &bind))))
                .child(self.term_origins_block(cx))
            });

        let remove_error = m.leave_error.clone().filter(|(leave, _)| *leave == Leave::Remove).map(|(_, error)| error);
        let remove = div().flex().flex_col().items_end().gap(px(6.))
            .when_some(remove_error, |el, error| el.child(div().id("machines-remove-error").role(Role::Alert)
                .text_size(px(12.5)).text_color(theme::danger()).child(error)))
            .child(Button::new("machines-remove").ghost().small().label(tr("machines_remove_here")).text_color(theme::danger())
                .on_click(cx.listener(|this, _, window, cx| this.confirm_leave(Leave::Remove, window, cx))));

        let (summary, light) = self.reach_summary();
        let server_name = self.server_label(cx);
        let head = div().mb(px(2.)).flex().items_center().gap(px(14.))
            .child(tile(chrome::small_icon(IconName::Server, 22., theme::accent_text()), 46., theme::accent_dim(), Some(light.dot())))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(server_name.clone()))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal()
                    .child(format!("{} · {summary}", tr("machines_this_server")))))
            .when(!rename_ids.is_empty(), |el| el.child(Button::new("machines-this-rename").ghost().small().label(tr("machines_rename"))
                .accessibility_label(tr("machines_rename_aria").replace("{nome}", &server_name))
                .on_click(cx.listener(move |this, _, window, cx| this.start_machine_rename(rename_ids.clone(), server_name.clone(), window, cx)))))
            .when(!id.is_empty(), |el| el.child(div().flex_shrink_0().px(px(10.)).py(px(2.)).rounded_full().border_1().border_color(theme::border())
                .font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).child(id.clone())));
        let scope_env = chip(tr("machines_scope_env"), theme::muted(), theme::raised()).flex_shrink_0().into_any_element();
        div().flex().flex_col().gap(px(16.)).pb(px(8.))
            .child(head)
            .children(rename_editor)
            .children(verdict)
            .child(settings_box().child(section_head(IconName::Hash, tr("machines_id"), None, Some(scope_env), px(16.)))
                .child(div().px_4().pb_4().child(identifier)))
            .child(addresses)
            .child(settings_box().child(section_head(IconName::RotateCw, tr("machines_service"), None, None, px(16.)))
                .child(div().px_4().pb_4().child(service)))
            .children(web_ui)
            .children(dev_mode)
            .child(advanced)
            .child(remove)
    }
}

/// Diálogo sem ação principal: o Enter vira o "confirmar" do kit, que sem `propagate` para a tecla ali e o botão focado nunca
/// recebe o clique de teclado. Seguindo, o botão focado clica ao soltar a tecla, e o diálogo não fecha.
pub(super) fn enter_to_focused(_: &ClickEvent, _: &mut Window, cx: &mut App) -> bool {
    cx.propagate();
    false
}

/// O `Button` do kit não toma foco no clique, e o diálogo devolve ao fechar o foco de quem o abriu: sem focar o cartão antes,
/// o Esc deixaria o foco na página. O foco é o que o próprio botão guarda, achado pelo mesmo caminho na árvore (o gpui põe
/// o nome do tipo do componente no caminho antes de desenhá-lo).
#[derive(IntoElement)]
pub(super) struct FocusOnClick {
    /// O mesmo id dado ao `Button`.
    pub(super) id: ElementId,
    pub(super) button: Button,
    pub(super) open: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl RenderOnce for FocusOnClick {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.with_id(std::any::type_name::<Button>(), |window| {
            window.use_keyed_state(self.id, cx, |_, cx| cx.focus_handle()).read(cx).clone()
        });
        let open = self.open;
        self.button.on_click(move |_, window, cx| {
            focus.focus(window, cx);
            open(window, cx);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Back, Card, Check, Kind, Light, Line, Peer, Reach, Reason, Row, ServerEntry, card_state, collapsed, host_key, join_lines, parse_going,
        parse_peers, parse_reach, row_state, update_warnings, valid_id, web_ui_value, Machines};
    use std::collections::HashMap;
    use crate::{api::Failure, i18n::tr};

    fn refused(status: u16, detail: &str) -> Failure {
        Failure { status: Some(status), detail: detail.into(), retry_after: None, uncertain: false, code: Some("web_ui_unavailable".into()) }
    }

    #[test]
    fn dev_mode_reads_the_mode_and_hides_where_the_switch_does_not_exist() {
        use super::{dev_mode_line, dev_mode_value, DevMode};
        let package = dev_mode_value(Ok(serde_json::json!({"modo": "pacote", "canal": "beta", "checkout": "/x/hangar"})));
        assert_eq!(package, Ok(Some(DevMode { developer: false, channel: "beta".into(), checkout: "/x/hangar".into() })));
        assert_eq!(dev_mode_line(&package.unwrap().unwrap()), tr("dev_mode_package").replace("{channel}", "beta"));
        let dev = dev_mode_value(Ok(serde_json::json!({"modo": "desenvolvedor", "canal": "main", "checkout": "/x/hangar"}))).unwrap().unwrap();
        assert!(dev.developer);
        assert_eq!(dev_mode_line(&dev), tr("dev_mode_developer").replace("{folder}", "/x/hangar"));
        assert_eq!(dev_mode_value(Ok(serde_json::json!({"modo": "pacote", "canal": "latest", "checkout": null}))).unwrap().unwrap().checkout, "");
        // Servidor anterior à troca (sem `canal`) e máquina sem migrar (motor `git`, sem `modo`): o interruptor some.
        assert_eq!(dev_mode_value(Ok(serde_json::json!({"modo": "pacote"}))), Ok(None));
        assert_eq!(dev_mode_value(Ok(serde_json::json!({"versoes": {}}))), Ok(None));
        assert_eq!(dev_mode_value(Err(refused(404, "Not Found"))), Ok(None));
        assert_eq!(dev_mode_value(Err(refused(503, "fixture-unavailable"))), Err("fixture-unavailable".into()), "o motivo do servidor");
    }

    #[test]
    fn dev_mode_after_a_good_read_keeps_the_row_with_unknown_mode_and_the_off_switch() {
        let mut m = Machines::default();
        let first = m.dev_mode.start();
        assert!(m.dev_mode_read(first, Ok(serde_json::json!({"modo": "pacote"}))));
        assert_eq!(m.dev_mode.value, Some(Ok(None)), "primeira leitura sem a troca: a linha some");
        assert!(!m.dev_mode_lost());

        let good = m.dev_mode.start();
        m.dev_mode_read(good, Ok(serde_json::json!({"modo": "pacote", "canal": "beta", "checkout": "/x/hangar"})));
        assert!(!m.dev_mode_lost());
        // Trocou para um checkout sem a troca (sem `canal`, 404) ou a leitura falhou: a linha fica, com o motivo.
        for (reply, reason) in [(Ok(serde_json::json!({"modo": "desenvolvedor"})), tr("dev_mode_unknown_reason")),
                                (Err(refused(404, "Not Found")), tr("dev_mode_unknown_reason")),
                                (Err(refused(503, "fixture-unavailable")), "fixture-unavailable".to_owned())] {
            let seq = m.dev_mode.start();
            assert!(!m.dev_mode_lost(), "lendo de novo");
            m.dev_mode_read(seq, reply);
            assert!(m.dev_mode_lost());
            assert_eq!(m.dev_mode.value, Some(Err(reason)));
        }
        let back = m.dev_mode.start();
        m.dev_mode_read(back, Ok(serde_json::json!({"modo": "desenvolvedor", "canal": "main", "checkout": "/x/hangar"})));
        assert!(!m.dev_mode_lost());
    }

    #[test]
    fn web_ui_reads_the_choice_and_old_servers_hide_the_row() {
        assert_eq!(web_ui_value(Ok(serde_json::json!({"web_ui": false}))), Ok(Some(false)));
        assert_eq!(web_ui_value(Ok(serde_json::json!({"web_ui": true}))), Ok(Some(true)));
        assert_eq!(web_ui_value(Err(refused(404, "Not Found"))), Ok(None), "servidor sem a rota");
        assert_eq!(web_ui_value(Err(refused(405, "Method Not Allowed"))), Ok(None));
        assert_eq!(web_ui_value(Err(refused(503, "fixture-unavailable"))), Err("fixture-unavailable".into()), "o motivo do servidor");
        assert_eq!(web_ui_value(Ok(serde_json::json!({"web_ui": "sim"}))), Err(tr("invalid_response")));
    }

    #[test]
    fn web_ui_failed_save_keeps_the_previous_value_and_shows_why() {
        let mut m = Machines::default();
        let read = m.web_ui.start();
        m.web_ui.finish(read, Ok(Some(true)));
        (m.web_ui_saving, m.web_ui_seq) = (true, 1);
        m.web_ui_saved(1, Err(refused(503, "fixture-unavailable")));
        assert_eq!((m.web_ui.ok(), m.web_ui_saving, m.web_ui_error.as_deref()), (Some(&Some(true)), false, Some("fixture-unavailable")));
        (m.web_ui_saving, m.web_ui_seq) = (true, 2);
        m.web_ui_saved(1, Ok(serde_json::json!({"web_ui": false})));
        assert!(m.web_ui_saving && m.web_ui.ok() == Some(&Some(true)), "resposta de pedido velho cai");
        m.web_ui_saved(2, Ok(serde_json::json!({"web_ui": false})));
        assert_eq!((m.web_ui.ok(), m.web_ui_saving, m.web_ui_error.as_deref()), (Some(&Some(false)), false, None));
    }

    #[test]
    fn update_warnings_are_the_state_avisos_like_the_web() {
        let state = serde_json::json!({"avisos": ["versão mais nova ainda sem binário", " ", 3, ""]});
        assert_eq!(update_warnings(&state), ["versão mais nova ainda sem binário"]);
        assert!(update_warnings(&serde_json::json!({"avisos": null})).is_empty());
        assert!(update_warnings(&serde_json::json!({})).is_empty());
    }

    fn reach(text: &str) -> Reach { parse_reach(&serde_json::from_str(text).expect("JSON")).expect("formato do /api/alcance") }

    #[test]
    fn summary_and_verdict_follow_the_measure_like_the_web() {
        let r = reach(r#"{"loopback": false, "bind": "0.0.0.0", "enderecos": [
            {"tipo": "rede_local", "url": "http://192.168.0.2:8765", "estado": "ok", "tempo_ms": 9},
            {"tipo": "tailscale", "url": "https://casa.ts.net", "estado": "ok", "tempo_ms": 3.4},
            {"tipo": "publico", "url": "https://casa.ts.net", "estado": "ok", "tempo_ms": 2}]}"#);
        // Público igual ao Tailscale não conta duas vezes nem vira caminho próprio.
        assert!(r.public_same());
        assert_eq!(r.outside().map(|a| a.kind), Some(Kind::Tailscale));
        assert_eq!(r.addresses.iter().filter(|a| r.main(a)).map(|a| a.kind).collect::<Vec<_>>(), [Kind::Lan, Kind::Tailscale]);
        assert!(r.extras().is_empty());
        // O cartão resume pelo mais rápido que respondeu, e o público repetido entra na conta como no web.
        assert_eq!(r.summary().1, Light::Ok);

        let isolated = reach(r#"{"loopback": true, "bind": "127.0.0.1", "enderecos": [
            {"tipo": "nesta_maquina", "url": "http://127.0.0.1:8765", "estado": "ok", "tempo_ms": 1},
            {"tipo": "rede_local", "url": "http://192.168.0.2:8765", "estado": "falhou", "tempo_ms": 3},
            {"tipo": "publico", "url": "", "estado": "nao_configurado", "tempo_ms": null}]}"#);
        assert!(isolated.outside().is_none() && isolated.lan().is_none());
        assert_eq!(isolated.summary(), (tr("machines_loopback_short"), Light::No));
        // Rede local fechada por escolha (loopback) é neutra; "nesta máquina" e o público vazio vão para o Avançado.
        let lan = &isolated.addresses[1];
        assert_eq!(isolated.light(lan), Light::Neutral);
        assert_eq!(isolated.extras().iter().map(|a| a.kind).collect::<Vec<_>>(), [Kind::Here, Kind::Public]);
    }

    fn entry(id: &str, address: &str, disabled: bool) -> ServerEntry {
        ServerEntry { id: id.into(), label: id.into(), address: address.into(), token: "t".into(), disabled, invite: false, lan: None, ephemeral: false }
    }
    fn peer(id: &str, url: &str, enabled: bool) -> Peer { Peer { id: id.into(), url: url.into(), enabled } }
    fn check(going: &str, back: Option<Back>) -> Check {
        Check { going: Some(parse_going(&serde_json::json!({"estado": going}))), back, ..Check::default() }
    }
    fn measured(estado: &str) -> Back { Back::Measured { going: parse_going(&serde_json::json!({"estado": estado})), url: "http://aqui".into() } }

    #[test]
    fn saved_machines_join_the_server_registry_like_the_web() {
        // O caso da tela: delphi-02 guardado aqui pelo Tailscale e registrado no servidor com outro endereço.
        let servers = [entry("pc", "http://127.0.0.1:8765", false), entry("delphi", "https://delphi-02.tailcac351.ts.net", false),
            entry("delphi-lan", "http://192.168.77.142:8765", false), entry("velho", "http://10.0.0.9:8765/", false),
            entry("jf", "https://jefferson-felizardo.ts.net/", false)];
        let peers = [peer("delphi-02", "http://192.168.77.142:8765", true), peer("srv1633222", "https://srv.example", true),
            peer("jefferson-felizardo", "https://Jefferson-Felizardo.ts.net", true)];
        let ids = HashMap::from([("pc".to_owned(), Some("notebook".to_owned())), ("delphi".to_owned(), Some("delphi-02".to_owned())),
            ("delphi-lan".to_owned(), None), ("velho".to_owned(), None)]);
        let reasons = HashMap::from([("pc".to_owned(), Reason::Empty), ("delphi".to_owned(), Reason::Empty),
            ("delphi-lan".to_owned(), Reason::NoAnswer), ("velho".to_owned(), Reason::NoAnswer)]);
        let lines = join_lines(&servers, &ids, &reasons, &peers, "http://127.0.0.1:8765");
        let find = |name: &str| lines.iter().find(|l| l.open_key() == name).expect(name);
        // O conectado é o cartão de cima, primeiro na ordem.
        assert!(lines[0].this && lines[0].open_key() == "notebook");
        // Pelo identificador e, sem ele, pelo host do registro: as duas entradas do delphi-02 viram uma linha só, com a que responde na frente.
        let delphi = find("delphi-02");
        assert_eq!(delphi.entries.len(), 2);
        assert_eq!(delphi.entry.as_ref().map(|e| e.id.as_str()), Some("delphi"));
        assert!(delphi.peer.is_some() && delphi.reason == Some(Reason::Empty));
        // Host sem esquema, maiúsculas e barra final: ainda é o registro dele, mesmo antes de responder o nome.
        assert!(find("jefferson-felizardo").entry.is_some() && find("jefferson-felizardo").peer.is_some());
        // Só no servidor e só neste aparelho continuam na lista.
        assert!(find("srv1633222").entry.is_none());
        assert!(find("srv:velho").peer.is_none());
        assert_eq!(lines.len(), 5);
        assert_eq!(host_key("https://casa.ts.net/hangar/"), host_key("http://CASA.ts.net/hangar"));
        assert_ne!(host_key("https://pocket.test/casa"), host_key("https://pocket.test/notebook"));
    }

    #[test]
    fn rows_say_whether_sessions_show_here() {
        let line = |entry: Option<ServerEntry>, reason: Option<Reason>, peer: Option<Peer>| Line { key: "k".into(), name: "casa".into(),
            ident: Some("casa".into()), reason, entries: entry.iter().cloned().collect(), entry, peer, this: false };
        let saved = || Some(entry("a", "https://casa.test", false));
        let registered = || Some(peer("casa", "https://casa.test", true));
        // Guardada aqui: o que manda é a resposta dela a este aparelho, não a medição do servidor.
        assert_eq!(row_state(&line(saved(), Some(Reason::Empty), registered()), Some(&check("falhou", None))), Row::Shown);
        assert_eq!(row_state(&line(saved(), None, None), None), Row::Testing);
        assert_eq!(row_state(&line(saved(), Some(Reason::Token), None), None), Row::TokenRefused);
        assert!(collapsed(row_state(&line(saved(), Some(Reason::NoAnswer), None), None)));
        assert_eq!(row_state(&line(Some(entry("a", "https://casa.test", true)), Some(Reason::Empty), None), None), Row::OffHere);
        // Só no servidor: a ida decide entre pedir o token e "não respondem".
        assert_eq!(row_state(&line(None, None, registered()), None), Row::Testing);
        assert_eq!(row_state(&line(None, None, registered()), Some(&check("ok", None))), Row::NoToken);
        assert!(collapsed(row_state(&line(None, None, registered()), Some(&check("falhou", None)))));
        // Desligada no servidor: recolhida, qualquer que seja o resto.
        assert_eq!(row_state(&line(saved(), Some(Reason::Empty), Some(peer("casa", "x", false))), None), Row::Off);
        assert!(!collapsed(Row::OffHere) && !collapsed(Row::NoToken));
        // Lista do servidor: `enabled` ausente é ligada, como no backend.
        let list = parse_peers(&serde_json::json!([{"id": "a", "base_url": "http://a"}, {"id": "b", "base_url": "http://b", "enabled": false}]))
            .expect("formato do /api/peers");
        assert_eq!(list.iter().map(|p| p.enabled).collect::<Vec<_>>(), [true, false]);
    }

    #[test]
    fn messages_card_follows_both_ways_like_the_web() {
        let line = |entry: Option<ServerEntry>, peer: Option<Peer>, ident: Option<&str>, reason| Line { key: "k".into(), name: "casa".into(),
            ident: ident.map(str::to_owned), reason, entries: entry.iter().cloned().collect(), entry, peer, this: false };
        let both = line(Some(entry("a", "https://casa.test", false)), Some(peer("casa", "https://casa.test", true)), Some("casa"), Some(Reason::Empty));
        let card = |c: &Check| card_state(&both, Some(c), "notebook");
        assert_eq!(card_state(&both, None, "notebook"), Card::Testing);
        assert_eq!(card_state(&both, Some(&Check { testing: true, ..check("ok", Some(measured("ok"))) }), "notebook"), Card::Testing);
        assert_eq!(card(&check("ok", Some(measured("ok")))), Card::Ok);
        assert_eq!(card(&check("ok", Some(measured("falhou")))), Card::OneWay);
        assert_eq!(card(&check("falhou", Some(measured("ok")))), Card::GoingFailed);
        // Endereço torto vem antes do "só de ida", e a volta antes da ida.
        assert_eq!(card(&check("estranho", Some(measured("estranho")))), Card::BackOther);
        assert_eq!(card(&check("estranho", Some(measured("ok")))), Card::GoingOther);
        assert_eq!(card(&check("ok", Some(Back::Refused))), Card::TokenRefused);
        assert_eq!(card(&check("ok", Some(Back::NoRegistration))), Card::NoRegistration);
        assert_eq!(card(&check("ok", Some(Back::NoToken))), Card::MissingToken);
        let paused = line(None, Some(peer("casa", "x", false)), Some("casa"), None);
        assert_eq!(card_state(&paused, Some(&check("ok", None)), "notebook"), Card::Paused);
        // Só neste aparelho: o que falta para ligar os recados.
        let local = |ident: Option<&str>, reason| line(Some(entry("a", "https://casa.test", false)), None, ident, reason);
        assert_eq!(card_state(&local(Some("casa"), Some(Reason::Empty)), None, ""), Card::NoOwnId);
        assert_eq!(card_state(&local(Some("casa"), Some(Reason::Empty)), None, "notebook"), Card::Off);
        assert_eq!(card_state(&local(None, Some(Reason::Empty)), None, "notebook"), Card::NoTheirId);
        assert_eq!(card_state(&local(None, Some(Reason::NoAnswer)), None, "notebook"), Card::NoAnswer);
        assert_eq!(card_state(&local(None, Some(Reason::Token)), None, "notebook"), Card::TokenRefused);
    }

    #[test]
    fn identifier_follows_the_backend_rule() {
        for ok in ["casa", "notebook-2", "a", "x_y", &"a".repeat(32)] { assert!(valid_id(ok), "{ok}"); }
        for bad in ["", "Casa", "-casa", "_x", "casa nova", "ção", &"a".repeat(33)] { assert!(!valid_id(bad), "{bad}"); }
    }
}
