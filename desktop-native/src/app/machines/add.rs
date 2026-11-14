//! Adicionar servidor (AdicionarMaquina.svelte): o endereço é testado antes de gravar, e o registro dos recados vai às duas pontas
//! (`registrarPeerDoisLados`). "Mostrar as sessões dele" grava a entrada neste aparelho (`addServer` do web).
use super::*;
use url::{Host, Url};
use crate::api::route::tailscale_host;

/// Porta em que o backend nasce: quem digita só o IP quase sempre quer ela.
const PORT: u16 = 8765;

/// O endereço como a pessoa digitou, pronto para testar (`normalizarEndereco` do web).
#[derive(Debug, PartialEq)]
pub(super) struct Target { base: String, token: Option<String>, alternative: Option<String> }

fn has_scheme(s: &str) -> bool {
    s.split_once("://").is_some_and(|(scheme, _)| scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
}

/// IP, nome, com ou sem porta, ou o link de pareamento inteiro. Nome com ponto é domínio atrás de https, com http na porta padrão de
/// reserva; IP, `localhost`, nome sem ponto e `.local` são rede local, na porta padrão. Esquema ou porta digitados vencem a dedução.
pub(super) fn normalize(raw: &str) -> Option<Target> {
    let s = raw.trim();
    if s.is_empty() { return None; }
    let bare = !has_scheme(s);
    let mut url = Url::parse(&if bare { format!("http://{s}") } else { s.to_owned() }).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) { return None; }
    let tokens = url.query_pairs().filter(|(k, _)| k == "token").map(|(_, v)| v.into_owned()).collect::<Vec<_>>();
    if tokens.len() > 1 { return None; }
    let token = tokens.into_iter().next();
    if token.as_ref().is_some_and(|t| t.is_empty() || t.chars().any(char::is_whitespace)) { return None; }
    // A porta padrão do esquema some da URL: se foi digitada decide-se pelo texto.
    let rest = if bare { s } else { s.split_once("://").map_or(s, |(_, rest)| rest) };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let typed_port = authority.rsplit_once(':').is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()));
    let mut alternative = None;
    if bare && !typed_port {
        let host = url.host_str().unwrap_or_default().to_owned();
        let local = matches!(url.host(), Some(Host::Ipv4(_))) || host == "localhost" || !host.contains('.') || host.ends_with(".local");
        if local {
            let _ = url.set_port(Some(PORT));
        } else {
            alternative = Some(format!("http://{host}:{PORT}"));
            let _ = url.set_scheme("https");
        }
    }
    // O caminho fica: servidor atrás de proxy com prefixo só responde nele.
    let path = url.path().trim_end_matches('/').to_owned();
    Some(Target { base: format!("{}{path}", url.origin().ascii_serialization()), token, alternative: alternative.map(|a| a + &path) })
}

/// IP fica inteiro: o primeiro pedaço de 192.168.0.10 seria "192".
fn short_host(base: &str) -> String {
    let Some(host) = Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_owned)) else { return base.to_owned() };
    if host.contains(':') || host.chars().all(|c| c.is_ascii_digit() || c == '.') { host } else { host.split('.').next().unwrap_or_default().to_owned() }
}

/// Entra na lista deste aparelho ou atualiza a de mesmo endereço. Falso quando o endereço é de um convite: ele fica intacto.
fn place_machine(list: &mut Vec<ServerEntry>, entry: ServerEntry) -> bool {
    if list.iter().any(|s| s.invite && servers::norm(&s.address) == servers::norm(&entry.address)) { return false; }
    servers::upsert(list, entry);
    true
}

fn host_of(url: &str) -> String { Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_else(|| url.trim().to_lowercase()) }

/// A resposta vai só ao diálogo que a pediu, se ele ainda for o aberto: fechado ou trocado, ela não tem dono.
pub(super) fn owns(open: Option<EntityId>, dialog: EntityId) -> bool { open == Some(dialog) }

/// Uma máquina que a busca no Tailscale achou respondendo como Hangar.
#[derive(Clone, Debug)]
pub(in crate::app) struct Discovered { name: String, url: String, hosts: Vec<String>, connect_error: Option<String> }

pub(super) fn parse_discovered(value: &Value) -> Option<Vec<Discovered>> {
    value.as_array()?.iter().map(|d| Some(Discovered {
        name: d.get("nome")?.as_str()?.to_owned(), url: d.get("base_url")?.as_str()?.to_owned(),
        hosts: d.get("hosts").and_then(Value::as_array).map(|h| h.iter().filter_map(Value::as_str).map(str::to_lowercase).collect()).unwrap_or_default(),
        connect_error: d.get("connect_error").and_then(|error| error.get("msg")).and_then(Value::as_str).map(str::to_owned),
    })).collect()
}

/// A conta do Connect falhou e a lista ficou só com o Tailscale: as linhas trazem a causa.
fn discovery_gap(found: &[Discovered]) -> Option<String> {
    found.iter().find_map(|machine| machine.connect_error.clone())
}

/// Quem respondeu no endereço testado. Sem o identificador não dá pra saber se é máquina repetida; a falha ao lê-lo aparece.
#[derive(Clone)]
pub(in crate::app) struct Found { base: String, token: String, id: String, id_error: Option<String> }

impl Found {
    pub(super) fn id(&self) -> &str { &self.id }
    /// Máquina já conhecida por este aparelho: endereço, token e identificador já lidos, sem novo teste.
    pub(super) fn known(base: String, token: String, id: String) -> Self { Self { base, token, id, id_error: None } }
}

/// O que o outro servidor disse, como o web mostra ("401: …"); queda de rede não tem texto próprio.
fn reason(error: &Failure) -> Option<String> {
    match error.status {
        Some(_) if error.detail.starts_with("HTTP ") => Some(error.detail.clone()),
        Some(status) => Some(format!("{status}: {}", tr(&error.detail))),
        None if matches!(error.detail.as_str(), "invalid_url" | "invalid_token") => Some(tr(&error.detail)),
        None => None,
    }
}

fn failed(errors: &[&Failure]) -> String {
    let parts = errors.iter().filter_map(|e| reason(e)).collect::<Vec<_>>();
    if parts.is_empty() { tr("connection_failed") } else { format!("{}: {}", tr("connection_failed"), parts.join(" · ")) }
}

fn failed_at(base: &str, errors: &[&Failure]) -> String {
    match tailscale_host(base) {
        Some(host) if errors.iter().all(|e| e.detail == "network_error") => tr_shared("login_tailscale_sem_acesso", &[("host", &host)]),
        _ => failed(errors),
    }
}

async fn ask_config(base: &str, token: &str) -> Result<Api, Failure> {
    let api = Api::new(base, token)?;
    // 20 s: pela Tailscale em relay a primeira conexão passa dos prazos curtos.
    api.server_read(&["config"], &[], 20).await.map(|_| api)
}

/// Responde? A reserva http só é tentada quando a primeira falha foi de rede: uma resposta HTTP já veio de alguém.
async fn probe(target: Target, token: String) -> Result<Found, String> {
    let (base, api) = match ask_config(&target.base, &token).await {
        Ok(api) => (target.base, api),
        Err(first) => match target.alternative.filter(|_| first.status.is_none()) {
            None => return Err(failed_at(&target.base, &[&first])),
            Some(alternative) => match ask_config(&alternative, &token).await {
                Ok(api) => (alternative, api),
                Err(second) => return Err(failed_at(&target.base, &[&first, &second])),
            },
        },
    };
    let (id, id_error) = match api.server_read(&["peers", "identificador"], &[], 20).await {
        Ok(value) => (value.get("identificador").and_then(Value::as_str).unwrap_or_default().to_owned(), None),
        Err(error) => (String::new(), Some(reason(&error).unwrap_or_else(|| tr("connection_failed")))),
    };
    Ok(Found { base, token, id, id_error })
}

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
        None => false,
    }
}

/// O endereço deste servidor para o outro lado guardar (`enderecoDoDono`): o da conexão, salvo loopback, que lá apontaria para ele
/// mesmo; aí quem responde é o servidor, pelos endereços que ele mediu e que servem a API (mesma porta, ou proxy sem porta).
async fn own_address(here: &Api) -> String {
    let base = here.identity().trim_end_matches('/').to_owned();
    let Some(url) = Url::parse(&base).ok().filter(loopback) else { return base };
    let Some(reach) = here.server_read(&["alcance"], &[], 30).await.ok().and_then(|v| parse_reach(&v)) else { return base };
    reach.addresses.into_iter()
        .filter(|a| a.status == Status::Ok && a.kind != Kind::Here
            && Url::parse(&a.url).is_ok_and(|u| !loopback(&u) && (u.port().is_none() || u.port() == url.port())))
        .min_by_key(|a| a.ms.unwrap_or(0)).map_or(base, |a| a.url)
}

pub(super) async fn check(api: &Api, url: &str, id: &str) -> Going {
    match api.server_read(&["peers", "check"], &[("url", url), ("id", id)], 30).await {
        Ok(value) => parse_going(&value),
        Err(error) => Going { way: Way::Failed, answered_as: String::new(), ms: None, error: Some(Hangar::fetch_failure(&error)) },
    }
}

/// Máquina do Connect: o serviço mora no mesmo domínio e não é máquina. Comparado inteiro, porque `connect` é nome válido de máquina.
fn connect_host(base: &str) -> bool {
    Url::parse(base).ok().and_then(|url| url.host_str().map(|host| host.trim_end_matches('.').to_owned()))
        .is_some_and(|host| host.ends_with(".hangar.dev.br") && host != "connect.hangar.dev.br")
}

/// A volta que não se tenta: o `peers.json` da outra máquina guardaria a senha do dono desta.
fn back_refused(base: &str) -> Option<Going> { connect_host(base).then(|| Going::failed(tr("machines_add_connect_back"))) }

/// Grava aqui (falha aqui é o resultado: nada mudou), grava este servidor lá e testa as duas pontas.
pub(super) async fn register(here: Api, found: Found, own_id: String, own_token: String) -> Result<(Going, Going), String> {
    let body = json!({"id": found.id, "base_url": found.base, "token": found.token});
    here.server_send(reqwest::Method::POST, &["peers"], Some(body), 15).await.map_err(|e| Hangar::fetch_failure(&e))?;
    if let Some(back) = back_refused(&found.base) { return Ok((check(&here, &found.base, &found.id).await, back)); }
    let own_base = own_address(&here).await;
    let there = Api::new(&found.base, &found.token);
    let saved = match &there {
        Ok(api) => api.server_send(reqwest::Method::POST, &["peers"], Some(json!({"id": own_id, "base_url": own_base, "token": own_token})), 15)
            .await.map(|_| ()).map_err(|e| failed(&[&e])),
        Err(error) => Err(failed(&[error])),
    };
    let going = check(&here, &found.base, &found.id).await;
    let mut back = match &there {
        Ok(api) => check(api, &own_base, &own_id).await,
        Err(error) => Going { way: Way::Failed, answered_as: String::new(), ms: None, error: Some(failed(&[error])) },
    };
    // A volta diz o estado real; a gravação que falhou lá é o motivo quando ela não fechou.
    if back.way != Way::Ok && let Err(error) = saved { back.error = Some(error); }
    Ok((going, back))
}

/// O diálogo "Adicionar neste aparelho". É uma entidade própria porque o diálogo é desenhado durante o desenho da janela, quando o
/// estado do `Hangar` não pode ser escrito.
pub(in crate::app) struct AddMachine {
    hangar: WeakEntity<Hangar>,
    seq: u64,
    address: Entity<InputState>,
    token: Entity<InputState>,
    name: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    found: Option<Found>,
    /// "Recados entre sessões": é uma pergunta, quem quer marca.
    messages: bool,
    /// "Mostrar as sessões dele": ligado por padrão, como no web; desligado com recados, a máquina fica só para recado.
    follow: bool,
    /// Registrado aqui, mas uma das pontas não fechou: o resultado de cada uma fica à vista.
    sides: Option<(Going, Going)>,
    /// A máquina já foi salva nesta abertura e os recados falharam: tentar de novo só registra, não trata como conhecida.
    unregistered: Option<String>,
    /// Busca no Tailscale, sob demanda e deste diálogo: `None` é "ainda não buscou".
    discover: Remote<Vec<Discovered>>,
    _subscriptions: Vec<Subscription>,
}

impl AddMachine {
    fn new(hangar: &Entity<Hangar>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let name = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.observe(hangar, |_, _, cx| cx.notify()),
            cx.subscribe_in(&address, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.edited(cx),
                // Link de pareamento colado inteiro: o token vai para o campo dele ao sair do campo, não a cada tecla.
                InputEvent::Blur => this.split_token(window, cx),
                InputEvent::PressEnter { .. } => this.primary(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&token, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.edited(cx),
                InputEvent::PressEnter { .. } => this.primary(window, cx),
                _ => {}
            }),
        ];
        Self { hangar: hangar.downgrade(), seq: 0, address, token, name, busy: false, error: None, found: None, messages: false, follow: true, sides: None,
            unregistered: None, discover: Remote::default(), _subscriptions: subscriptions }
    }

    pub(super) fn waiting(&self, seq: u64) -> bool { self.busy && self.seq == seq }

    /// O que a pessoa digita desfaz o teste: o resultado era de outro endereço.
    fn edited(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.sides.is_some() { return; }
        (self.error, self.found) = (None, None);
        cx.notify();
    }

    fn split_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Target { base, token: Some(token), .. }) = normalize(&self.address.read(cx).value()) else { return };
        self.address.update(cx, |input, cx| input.set_value(base, window, cx));
        self.token.update(cx, |input, cx| input.set_value(token, window, cx));
    }

    fn use_found(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.address.update(cx, |input, cx| input.set_value(url, window, cx));
        // O token digitado era de outro servidor.
        self.token.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
        (self.error, self.found) = (None, None);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy { return; }
        // Enter no campo e o confirmar do diálogo podem chegar os dois: só o primeiro fecha, e nunca o diálogo de baixo.
        let me = cx.entity_id();
        let mine = self.hangar.update(cx, |hangar, _| {
            let mine = hangar.machines.add.as_ref().is_some_and(|a| a.entity_id() == me);
            if mine { hangar.machines.add = None; }
            mine
        }).unwrap_or(false);
        if mine { window.close_dialog(cx); }
    }

    /// Enter e o botão principal: testar, depois adicionar; com o resultado das pontas à vista, fechar.
    pub(super) fn primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sides.is_some() { self.close(window, cx); } else if self.found.is_some() { self.add(window, cx); } else { self.test(cx); }
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        if self.busy { return; }
        let raw = self.address.read(cx).value().to_string();
        let Some(target) = normalize(&raw) else {
            let key = if raw.contains("?token=") || raw.contains("&token=") { "machines_add_error_token" } else { "machines_add_error_address" };
            self.error = Some(tr(key));
            cx.notify();
            return;
        };
        let token = target.token.clone().unwrap_or_else(|| self.token.read(cx).value().to_string()).trim().to_owned();
        if token.is_empty() || token.chars().any(char::is_whitespace) { self.error = Some(tr("machines_add_error_token")); cx.notify(); return; }
        self.seq += 1;
        (self.busy, self.error) = (true, None);
        let (me, seq) = (cx.entity_id(), self.seq);
        if !self.hangar.update(cx, |hangar, cx| hangar.probe_machine(me, seq, target, token, cx)).unwrap_or(false) {
            (self.busy, self.error) = (false, Some(tr("settings_offline")));
        }
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.found.clone().filter(|_| !self.busy) else { return };
        let Some(hangar) = self.hangar.upgrade() else { return };
        let (own_id, known, stuck) = {
            let h = hangar.read(cx);
            let known = h.known_machine(&found.id);
            (h.machines.id_loaded().to_owned(), known.clone(), known.is_some_and(|k| h.moves_active(&k, &found.base)))
        };
        let retry = self.unregistered.as_ref() == Some(&found.id);
        let typed = self.name.read(cx).value().trim().to_owned();
        let label = if !typed.is_empty() { typed } else if !found.id.is_empty() { found.id.clone() } else { short_host(&found.base) };
        // Mesma máquina por outro endereço: a entrada que já existe troca de endereço, continua uma linha só.
        if let Some(entry) = known.filter(|_| !retry) {
            if stuck { return; }
            if !hangar.update(cx, |hangar, cx| hangar.move_machine(&entry.id, &found, label, window, cx)) {
                self.error = Some(tr("machines_add_error_invite"));
                cx.notify();
                return;
            }
            return self.close(window, cx);
        }
        // Sem identificador aqui nenhum outro registra este: só a entrada neste aparelho.
        let messages = !own_id.is_empty() && self.messages;
        if !messages && !self.follow { self.error = Some(tr("machines_add_error_none")); cx.notify(); return; }
        if messages && found.id.is_empty() { self.error = Some(tr("machines_add_error_no_id")); cx.notify(); return; }
        let entry = ServerEntry { id: servers::new_id(), label, address: found.base.clone(), token: found.token.clone(), disabled: false, invite: false, lan: None, ephemeral: false, ..Default::default() };
        if self.follow && !retry &&!hangar.update(cx, |hangar, cx| hangar.save_machine(entry, window, cx)) {
            self.error = Some(tr("machines_add_error_invite"));
            cx.notify();
            return;
        }
        if !messages { return self.close(window, cx); }
        self.unregistered = Some(found.id.clone());
        self.seq += 1;
        (self.busy, self.error) = (true, None);
        let (me, seq) = (cx.entity_id(), self.seq);
        if !hangar.update(cx, |hangar, cx| hangar.register_machine(me, seq, found, cx)) {
            (self.busy, self.error) = (false, Some(tr("settings_offline")));
        }
        cx.notify();
    }

    pub(super) fn probed(&mut self, seq: u64, result: Result<Found, String>, known: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.waiting(seq) { return; }
        self.busy = false;
        match result {
            Ok(found) => {
                let name = known.unwrap_or_else(|| if found.id.is_empty() { short_host(&found.base) } else { found.id.clone() });
                self.name.update(cx, |input, cx| input.set_value(name, window, cx));
                self.found = Some(found);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn discover(&mut self, cx: &mut Context<Self>) {
        if self.discover.loading { return; }
        let (me, seq) = (cx.entity_id(), self.discover.start());
        if !self.hangar.update(cx, |hangar, _| hangar.discover_machines(me, seq)).unwrap_or(false) {
            self.discover.finish(seq, Err(tr("settings_offline")));
        }
        cx.notify();
    }

    pub(super) fn discovered(&mut self, seq: u64, result: Result<Vec<Discovered>, String>, cx: &mut Context<Self>) {
        if self.discover.finish(seq, result) { cx.notify(); }
    }

    pub(super) fn registered(&mut self, seq: u64, result: Result<(Going, Going), String>, cx: &mut Context<Self>) {
        if !self.waiting(seq) { return; }
        self.busy = false;
        match result { Ok(sides) => { self.sides = Some(sides); self.unregistered = None; } Err(error) => self.error = Some(error) }
        cx.notify();
    }
}

fn muted(text: String) -> Div { div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text) }

fn field(label: String, help: String, input: Input) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().text_size(px(13.)).text_color(theme::muted()).child(label.clone()))
        .child(input.aria_label(label))
        .child(muted(help))
}

/// Uma linha com interruptor, no desenho das linhas das páginas do servidor.
fn toggle(title: String, help: String, switch: impl IntoElement) -> Div {
    div().flex().items_center().gap(px(14.))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::MEDIUM).child(title)).child(muted(help)))
        .child(div().flex_shrink_0().child(switch))
}

fn next_tip(id: &'static str, child: impl IntoElement) -> Stateful<Div> {
    let next = tr("settings_next_version");
    div().id(id).child(child).tooltip(move |window, cx| Tooltip::new(next.clone()).build(window, cx))
}

impl Render for AddMachine {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(hangar) = self.hangar.upgrade() else { return div() };
        let (here, own_id, online, known, same, stuck) = {
            let h = hangar.read(cx);
            let m = &h.machines;
            let own_url = h.api.as_ref().map(|api| api.identity().trim_end_matches('/').to_owned()).unwrap_or_default();
            let mut known = m.peers.ok().map(|list| list.iter().map(|p| host_of(&p.url)).collect::<HashSet<_>>()).unwrap_or_default();
            known.insert(host_of(&own_url));
            // A que esta abertura salvou e ainda falta registrar não conta como repetida: os recados continuam à vista.
            let same = self.found.as_ref().filter(|f| self.unregistered.as_ref() != Some(&f.id)).and_then(|f| h.known_machine(&f.id));
            let stuck = same.as_ref().zip(self.found.as_ref()).is_some_and(|(s, f)| h.moves_active(s, &f.base));
            (h.server_label(cx), m.id_loaded().to_owned(), h.api.is_some(), known, same, stuck)
        };
        let d = &self.discover;
        let searching = d.loading;
        let search_error = d.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !searching).cloned();
        let results = d.ok().filter(|_| !searching).cloned();
        let search_gap = results.as_deref().and_then(discovery_gap);
        let next = tr("settings_next_version");
        let (busy, done) = (self.busy, self.sides.is_some());
        let locked = busy || done;
        let can_talk = !own_id.is_empty();
        let repeated = same.is_some();
        let peer_name = self.found.as_ref().map(|f| f.id.clone()).filter(|id| !id.is_empty()).unwrap_or_else(|| tr("machines_this_machine"));
        let fill = |key: &str| tr(key).replace("{este}", &here).replace("{nome}", &peer_name);

        // Grava no aparelho, não no servidor: sem a faixa parecia que a máquina passava a aparecer para todo mundo.
        let band = div().p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
            .child(muted(tr("machines_add_band").replace("{este}", if here.is_empty() { "—" } else { &here })));

        // Sob demanda: cada busca bate em todos os servidores da rede.
        let found_list = results.map(|list| list.into_iter()
            .filter(|d| !std::iter::once(host_of(&d.url)).chain(d.hosts.iter().cloned()).any(|h| known.contains(&h))).collect::<Vec<_>>());
        let search = div().flex().flex_col().gap(px(8.)).pb(px(12.)).border_b_1().border_color(theme::border())
            .child(div().flex().items_center().gap(px(12.))
                .child(div().flex_1().min_w_0().child(muted(tr(if online { "machines_search_help" } else { "machines_search_no_server" }))))
                .child(Button::new("machines-add-search").flex_shrink_0().outline().small().label(tr(if searching { "machines_searching" } else { "machines_search_tailscale" }))
                    .loading(searching).disabled(!online || busy)
                    .when(!online, |b| b.accessibility_label(format!("{}. {}", tr("machines_search_tailscale"), tr("machines_search_no_server"))))
                    .on_click(cx.listener(|this, _, _, cx| this.discover(cx)))))
            .when_some(search_error, |el, error| el.child(div().id("machines-add-search-error").role(Role::Status).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .when_some(search_gap, |el, error| el.child(div().id("machines-add-search-gap").role(Role::Status).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(tr_shared("mobile_machines_search_partial", &[("reason", &error)]))))
            .when_some(found_list, |el, list| if list.is_empty() {
                el.child(div().id("machines-add-search-none").role(Role::Status).child(muted(tr("machines_search_none"))))
            } else {
                el.child(muted(tr("machines_search_found"))).child(settings_box().children(list.into_iter().enumerate().map(|(n, d)| {
                    let url = d.url.clone();
                    div().flex().items_center().gap(px(12.)).px_4().py(px(8.)).when(n > 0, |el| el.border_t_1().border_color(theme::border()))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().child(d.name.clone()))
                            .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted()).whitespace_normal().child(d.url.clone())))
                        .child(div().flex_shrink_0().child(Button::new(SharedString::from(format!("machines-add-found-{n}"))).outline().small()
                            .label(tr("machines_add_server")).accessibility_label(format!("{} {}", tr("machines_add_server"), d.name)).disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| this.use_found(url.clone(), window, cx)))))
                }).collect::<Vec<_>>()))
            });

        let answered = self.found.as_ref().map(|f| div().id("machines-add-answered").role(Role::Status).p(px(12.)).rounded(px(10.)).border_1()
            .border_color(theme::success()).bg(theme::inset()).flex().flex_col().gap(px(4.))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::success())
                .child(format!("✓ {}", if f.id.is_empty() { tr("machines_add_answered_no_id") } else { tr("machines_add_answered").replace("{id}", &f.id) })))
            .when_some(f.id_error.clone(), |el, error| el.child(div().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal()
                .child(tr("machines_add_id_failed").replace("{erro}", &error))))
            .when_some(same.as_ref(), |el, s| el.child(muted(tr("machines_add_repeated")
                .replace("{nome}", &if s.label.is_empty() { servers::default_label(&s.address) } else { s.label.clone() })
                .replace("{endereco}", &s.address)))));
        let name = self.found.as_ref().map(|_| field(tr("machines_add_name"), tr("machines_add_name_help"), Input::new(&self.name).disabled(locked)));

        // Sem identificador aqui o web nem pergunta: adicionar é só a entrada no aparelho.
        let switches = (can_talk && !repeated).then(|| div().flex().flex_col().gap(px(12.))
            .child(toggle(tr("machines_peer_show_sessions"), tr("machines_add_follow_help"),
                Switch::new("machines-add-follow-switch").checked(self.follow).disabled(locked).accessibility_label(tr("machines_peer_show_sessions"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| { (this.follow, this.error) = (*on, None); cx.notify(); }))))
            .child(toggle(fill("machines_peer_messages_title"), fill("machines_add_messages_help"),
                Switch::new("machines-add-messages").checked(self.messages).disabled(locked).accessibility_label(fill("machines_peer_messages_title"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| { (this.messages, this.error) = (*on, None); cx.notify(); })))));

        let sides = self.sides.as_ref().map(|(going, back)| {
            let (headline, detail, why) = if going.way != Way::Ok {
                (fill(if going.way == Way::Other { "machines_peer_going_other" } else { "machines_peer_going_failed" }),
                    tr("machines_add_going_failed_p"), going.error.clone())
            } else {
                (fill(if back.way == Way::Other { "machines_add_back_other" } else { "machines_add_one_way" }), fill("machines_add_one_way_p"),
                    back.error.clone())
            };
            div().id("machines-add-sides").role(Role::Alert).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::danger()).bg(theme::inset())
                .flex().flex_col().gap(px(6.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(headline))
                .child(muted(detail))
                .when_some(why, |el, why| el.child(div().text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(why)))
        });

        let primary = if done {
            Button::new("machines-add-close").primary().small().label(tr("close")).on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                .into_any_element()
        } else if self.found.is_some() {
            let label = tr(if repeated { "machines_add_use_address" } else { "machines_add_add" });
            let blocked = stuck;
            let button = Button::new("machines-add-add").primary().small().label(label.clone()).loading(busy).disabled(blocked)
                .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx)));
            // Trocar o endereço do servidor conectado pede reconectar, que este diálogo ainda não faz.
            if blocked { next_tip("machines-add-add-tip", button.accessibility_label(format!("{label}. {next}"))).into_any_element() } else { button.into_any_element() }
        } else {
            let can_test = !self.address.read(cx).value().trim().is_empty();
            Button::new("machines-add-test").primary().small().label(tr("machines_add_test")).loading(busy).disabled(!can_test && !busy)
                .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx))).into_any_element()
        };
        let footer = div().mt(px(4.)).flex().items_center().justify_end().gap(px(8.))
            .when(!done, |el| el.child(Button::new("machines-add-cancel").ghost().small().label(tr("cancel")).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
                .child(next_tip("machines-add-scan", Button::new("machines-add-scan-button").outline().small().label(tr("machines_scan_qr")).disabled(true)
                    .accessibility_label(format!("{}. {next}", tr("machines_scan_qr"))))))
            .child(primary);

        div().flex().flex_col().gap(px(14.)).pb(px(4.))
            .child(band)
            .child(search)
            .child(field(tr("machines_add_address"), tr("machines_add_address_help"), Input::new(&self.address).disabled(locked)))
            .child(field(tr("machines_add_token"), tr("machines_add_token_help").replace("{variavel}", "CP_AUTH_TOKEN"),
                Input::new(&self.token).disabled(locked)))
            .children(answered)
            .children(name)
            .when(self.found.is_some() && !can_talk && !repeated, |el| el.child(div().text_size(px(12.5)).text_color(theme::warning())
                .whitespace_normal().child(tr("machines_no_id_short"))))
            .children(switches)
            .when(busy, |el| el.child(div().id("machines-add-busy").role(Role::Status).child(muted(tr("machines_testing")))))
            .when_some(self.error.clone(), |el, error| el.child(div().id("machines-add-error").role(Role::Alert).text_size(px(12.5))
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .children(sides)
            .child(footer)
    }
}

impl Hangar {
    pub(super) fn open_add_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.api.is_none() { return; }
        let hangar = cx.entity();
        let add = cx.new(|cx| AddMachine::new(&hangar, window, cx));
        self.machines.add = Some(add.clone());
        let (weak, address) = (hangar.downgrade(), add.read(cx).address.clone());
        window.open_dialog(cx, move |dialog, _, cx| {
            // Em voo o diálogo não fecha: ele é o único lugar onde a resposta aparece, como o `fechar()` do web.
            let busy = add.read(cx).busy;
            let (weak, confirm, me) = (weak.clone(), add.clone(), add.entity_id());
            popup::dialog(dialog).w(px(600.)).title(tr("machines_add_device")).child(add.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(move |_, window, cx| { confirm.update(cx, |add, cx| add.primary(window, cx)); false })
                .on_close(move |_, _, cx| { let _ = weak.update(cx, |this, _| {
                    if this.machines.add.as_ref().is_some_and(|a| a.entity_id() == me) { this.machines.add = None; }
                }); })
        });
        address.update(cx, |input, cx| input.focus(window, cx));
    }

    fn discover_machines(&mut self, dialog: EntityId, seq: u64) -> bool {
        let Some(api) = self.api.clone() else { return false };
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Discovered(dialog, seq, api.server_read(&["peers", "descobrir"], &[], 30).await)).await });
        true
    }

    fn probe_machine(&mut self, dialog: EntityId, seq: u64, target: Target, token: String, _: &mut Context<Self>) -> bool {
        if self.api.is_none() { return false; }
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Probed(dialog, seq, probe(target, token).await)).await });
        true
    }

    /// A entrada deste aparelho que já é a máquina deste identificador (`conhecidas` do web).
    pub(super) fn known_machine(&self, id: &str) -> Option<ServerEntry> {
        if id.is_empty() { return None; }
        let active = self.server.as_deref().map(servers::norm).unwrap_or_default();
        self.machine_lines().into_iter().find(|l| l.ident.as_deref() == Some(id)).and_then(|l| l.entry).filter(|e| !e.invite)
            // O identificador das entradas chega depois de abrir a página; o do servidor conectado já está lido.
            .or_else(|| (id == self.machines.id_loaded()).then(|| self.server_entry(&active).cloned()).flatten())
    }

    /// A entrada é a do servidor conectado e o endereço novo é outro.
    fn moves_active(&self, entry: &ServerEntry, address: &str) -> bool {
        let active = self.server.as_deref().map(servers::norm).unwrap_or_default();
        servers::norm(&entry.address) == active && servers::norm(address) != active
    }

    fn servers_changed(&mut self, cx: &mut Context<Self>) {
        self.servers_rev += 1;
        self.persist_servers();
        self.start_remote_lists();
        self.sync_updater(cx);
        self.load_saved_ids(cx);
    }

    fn save_machine(&mut self, entry: ServerEntry, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = servers::norm(&entry.address);
        if !place_machine(&mut self.servers, entry) { return false; }
        self.servers_changed(cx);
        self.reopen_edited(&[key.clone()], &key, window, cx);
        true
    }

    /// Troca endereço, token e nome da entrada e a religa: adicionar de novo é pedir para ver. Outra entrada no endereço novo
    /// sai, senão a mesma máquina teria duas listas.
    fn move_machine(&mut self, entry_id: &str, found: &Found, label: String, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = servers::norm(&found.base);
        if self.servers.iter().any(|s| s.invite && servers::norm(&s.address) == key) { return false; }
        self.servers.retain(|s| s.id == entry_id || servers::norm(&s.address) != key);
        let Some(entry) = self.servers.iter_mut().find(|s| s.id == entry_id) else { return false };
        let old = servers::norm(&entry.address);
        (entry.address, entry.token, entry.label, entry.disabled) = (found.base.clone(), found.token.clone(), label, false);
        servers::clear_connect_refusal(entry);
        self.servers_changed(cx);
        self.reopen_edited(&[old, key.clone()], &key, window, cx);
        true
    }

    /// A sessão aberta era de uma entrada que mudou: reabre pela entrada nova; sem conexão possível, fica fechada.
    fn reopen_edited(&mut self, old: &[String], key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(open), Some(session)) = (self.open_key(), self.selected.clone()) else { return };
        if !old.contains(&open) { return; }
        let draft = self.composer.read(cx).value().to_string();
        self.close_open_session(window, cx);
        self.select_on(key, session, window, cx);
        // O rascunho foi guardado no endereço antigo.
        if self.selected.is_some() && !draft.is_empty() { self.composer.update(cx, |input, cx| input.set_value(draft, window, cx)); }
    }

    fn register_machine(&mut self, dialog: EntityId, seq: u64, found: Found, cx: &mut Context<Self>) -> bool {
        let Some(api) = self.api.clone() else { return false };
        let own_id = self.machines.id_loaded().to_owned();
        if own_id.is_empty() { return false; }
        // O token com que este app fala com este servidor: é o que o outro lado usa para responder.
        let own_token = self.token.read(cx).value().trim().to_owned();
        let done = self.machines_send_later();
        self.runtime.spawn(async move { done(MachinesReply::Registered(dialog, seq, register(api, found, own_id, own_token).await)).await });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Discovered, EntityId, Remote, ServerEntry, Target, Way, back_refused, discovery_gap, normalize, owns, parse_discovered, place_machine, short_host, tailscale_host};

    #[test]
    fn discovery_keeps_the_cause_of_a_failed_account() {
        let rows = serde_json::json!([{"nome": "pc", "base_url": "http://x", "connect_error": {"code": "c", "msg": "o serviço caiu"}}]);
        assert_eq!(discovery_gap(&parse_discovered(&rows).unwrap()).as_deref(), Some("o serviço caiu"));
        let clean = serde_json::json!([{"nome": "pc", "base_url": "http://x"}]);
        assert_eq!(discovery_gap(&parse_discovered(&clean).unwrap()), None);
    }

    #[test]
    fn owner_password_never_goes_to_a_connect_host() {
        for base in ["https://pc-b.acme.hangar.dev.br", "https://PC-B.acme.hangar.dev.br/", "https://pc-b.acme.hangar.dev.br.", "https://connect.acme.hangar.dev.br"] {
            let back = back_refused(base).unwrap_or_else(|| panic!("{base}"));
            assert_eq!(back.way, Way::Failed, "{base}");
            assert_eq!(back.error, Some(crate::i18n::tr("machines_add_connect_back")), "{base}");
        }
        for base in ["https://casa.tail0000.ts.net", "http://100.64.0.2:8765", "http://192.168.0.10:8765", "http://casa:8765",
            "https://connect.hangar.dev.br", "https://hangar.dev.br", "https://nothangar.dev.br", "https://hangar.dev.br.exemplo.com"] {
            assert!(back_refused(base).is_none(), "{base}");
        }
    }

    #[test]
    fn tailscale_addresses_are_the_tailnet_name_and_the_cgnat_range() {
        assert_eq!(tailscale_host("https://hangar.tail0000.ts.net").as_deref(), Some("hangar.tail0000.ts.net"));
        assert_eq!(tailscale_host("http://100.64.0.2:8765").as_deref(), Some("100.64.0.2"));
        for other in ["http://100.128.0.1:8765", "http://100.63.0.1:8765", "http://192.168.0.5:8765", "https://exemplo.com", "https://ts.net.exemplo.com"] {
            assert_eq!(tailscale_host(other), None, "{other}");
        }
    }

    #[test]
    fn saving_updates_the_same_address_and_never_an_invite() {
        let entry = |address: &str, token: &str, invite: bool| ServerEntry { id: token.into(), label: token.into(), address: address.into(),
            token: token.into(), disabled: true, invite, lan: None, ephemeral: false, ..Default::default() };
        let mut list = vec![entry("http://casa:8765", "old", false), entry("https://h:8443", "guest", true)];
        assert!(place_machine(&mut list, ServerEntry { disabled: false, ..entry("http://CASA:8765/", "new", false) }));
        assert_eq!((list.len(), list[0].token.as_str(), list[0].disabled), (2, "new", false));
        assert!(!place_machine(&mut list, entry("https://h:8443/", "own", false)));
        assert_eq!((list.len(), list[1].token.as_str()), (2, "guest"));
    }

    #[test]
    fn search_answer_reaches_only_its_dialog_and_last_request() {
        let (a, b) = (EntityId::from(1u64), EntityId::from(2u64));
        assert!(owns(Some(a), a));
        assert!(!owns(Some(b), a), "resposta de A com B aberto");
        assert!(!owns(None, a), "resposta de A com o diálogo fechado");
        let mut search = Remote::<Vec<Discovered>>::default();
        let (old, new) = (search.start(), search.start());
        assert!(!search.finish(old, Ok(Vec::new())) && search.loading && search.value.is_none());
        assert!(search.finish(new, Err("x".into())) && !search.loading);
    }

    fn target(base: &str, token: Option<&str>, alternative: Option<&str>) -> Option<Target> {
        Some(Target { base: base.into(), token: token.map(Into::into), alternative: alternative.map(Into::into) })
    }

    #[test]
    fn address_is_normalized_like_the_web() {
        assert_eq!(normalize("192.168.0.10"), target("http://192.168.0.10:8765", None, None));
        assert_eq!(normalize("casa"), target("http://casa:8765", None, None));
        assert_eq!(normalize("casa.ts.net"), target("https://casa.ts.net", None, Some("http://casa.ts.net:8765")));
        assert_eq!(normalize("casa.ts.net:9000/"), target("http://casa.ts.net:9000", None, None));
        assert_eq!(normalize("https://vps.test/delphi/?token=abc"), target("https://vps.test/delphi", Some("abc"), None));
        for bad in ["", "ftp://x", "http://x?token=a&token=b", "http://x?token=", "x?token=a b"] { assert_eq!(normalize(bad), None, "{bad}"); }
        assert_eq!((short_host("http://192.168.0.10:8765"), short_host("https://casa.ts.net")), ("192.168.0.10".into(), "casa".into()));
    }
}
