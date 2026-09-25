//! Adicionar servidor (AdicionarMaquina.svelte): o endereço é testado antes de gravar, e o registro dos recados vai às duas pontas
//! (`registrarPeerDoisLados`). "Mostrar as sessões dele" pede uma entrada neste aparelho, que o nativo não tem: chega depois.
use super::*;
use url::{Host, Url};

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

fn host_of(url: &str) -> String { Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_else(|| url.trim().to_lowercase()) }

/// A resposta vai só ao diálogo que a pediu, se ele ainda for o aberto: fechado ou trocado, ela não tem dono.
pub(super) fn owns(open: Option<EntityId>, dialog: EntityId) -> bool { open == Some(dialog) }

/// Uma máquina que a busca no Tailscale achou respondendo como Hangar.
#[derive(Clone, Debug)]
pub(in crate::app) struct Discovered { name: String, url: String, hosts: Vec<String> }

pub(super) fn parse_discovered(value: &Value) -> Option<Vec<Discovered>> {
    value.as_array()?.iter().map(|d| Some(Discovered {
        name: d.get("nome")?.as_str()?.to_owned(), url: d.get("base_url")?.as_str()?.to_owned(),
        hosts: d.get("hosts").and_then(Value::as_array).map(|h| h.iter().filter_map(Value::as_str).map(str::to_lowercase).collect()).unwrap_or_default(),
    })).collect()
}

/// Quem respondeu no endereço testado. Sem o identificador não dá pra saber se é máquina repetida; a falha ao lê-lo aparece.
#[derive(Clone)]
pub(in crate::app) struct Found { base: String, token: String, id: String, id_error: Option<String> }

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
            None => return Err(failed(&[&first])),
            Some(alternative) => match ask_config(&alternative, &token).await {
                Ok(api) => (alternative, api),
                Err(second) => return Err(failed(&[&first, &second])),
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

async fn check(api: &Api, url: &str, id: &str) -> Going {
    match api.server_read(&["peers", "check"], &[("url", url), ("id", id)], 30).await {
        Ok(value) => parse_going(&value),
        Err(error) => Going { way: Way::Failed, answered_as: String::new(), ms: None, error: Some(Hangar::fetch_failure(&error)) },
    }
}

/// Grava aqui (falha aqui é o resultado: nada mudou), grava este servidor lá e testa as duas pontas.
async fn register(here: Api, found: Found, own_id: String, own_token: String) -> Result<(Going, Going), String> {
    let body = json!({"id": found.id, "base_url": found.base, "token": found.token});
    here.server_send(reqwest::Method::POST, &["peers"], Some(body), 15).await.map_err(|e| Hangar::fetch_failure(&e))?;
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
    /// Registrado aqui, mas uma das pontas não fechou: o resultado de cada uma fica à vista.
    sides: Option<(Going, Going)>,
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
        Self { hangar: hangar.downgrade(), seq: 0, address, token, name, busy: false, error: None, found: None, messages: false, sides: None,
            discover: Remote::default(), _subscriptions: subscriptions }
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
        if self.sides.is_some() { self.close(window, cx); } else if self.found.is_some() { self.add(cx); } else { self.test(cx); }
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

    fn add(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.found.clone().filter(|_| !self.busy) else { return };
        let Some(hangar) = self.hangar.upgrade() else { return };
        let own_id = hangar.read(cx).machines.id_loaded().to_owned();
        // Sem identificador aqui nenhum outro registra este, e a mesma máquina por outro endereço não vira linha nova.
        if own_id.is_empty() || found.id == own_id { return; }
        if !self.messages { self.error = Some(tr("machines_add_error_none")); cx.notify(); return; }
        if found.id.is_empty() { self.error = Some(tr("machines_add_error_no_id")); cx.notify(); return; }
        self.seq += 1;
        (self.busy, self.error) = (true, None);
        let (me, seq) = (cx.entity_id(), self.seq);
        if !hangar.update(cx, |hangar, cx| hangar.register_machine(me, seq, found, cx)) {
            (self.busy, self.error) = (false, Some(tr("settings_offline")));
        }
        cx.notify();
    }

    pub(super) fn probed(&mut self, seq: u64, result: Result<Found, String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.waiting(seq) { return; }
        self.busy = false;
        match result {
            Ok(found) => {
                let name = if found.id.is_empty() { short_host(&found.base) } else { found.id.clone() };
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
        match result { Ok(sides) => self.sides = Some(sides), Err(error) => self.error = Some(error) }
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
        let (here, own_id, own_url, online, known) = {
            let h = hangar.read(cx);
            let m = &h.machines;
            let own_url = h.api.as_ref().map(|api| api.identity().trim_end_matches('/').to_owned()).unwrap_or_default();
            let mut known = m.peers.ok().map(|list| list.iter().map(|p| host_of(&p.url)).collect::<HashSet<_>>()).unwrap_or_default();
            known.insert(host_of(&own_url));
            (h.server_label(cx), m.id_loaded().to_owned(), own_url, h.api.is_some(), known)
        };
        let d = &self.discover;
        let searching = d.loading;
        let search_error = d.value.as_ref().and_then(|v| v.as_ref().err()).filter(|_| !searching).cloned();
        let results = d.ok().filter(|_| !searching).cloned();
        let next = tr("settings_next_version");
        let (busy, done) = (self.busy, self.sides.is_some());
        let locked = busy || done;
        let can_talk = !own_id.is_empty();
        let repeated = self.found.as_ref().is_some_and(|f| !f.id.is_empty() && f.id == own_id);
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
            .when(repeated, |el| el.child(muted(tr("machines_add_repeated").replace("{nome}", &here).replace("{endereco}", &own_url)))));
        // O nome é o da entrada neste aparelho, que chega na próxima versão.
        let name = self.found.as_ref().map(|_| div().flex().flex_col().gap(px(4.))
            .child(div().text_size(px(13.)).text_color(theme::muted()).child(tr("machines_add_name")))
            .child(next_tip("machines-add-name", Input::new(&self.name).disabled(true).aria_label(format!("{}. {next}", tr("machines_add_name")))))
            .child(muted(format!("{} {next}", tr("machines_add_name_help")))));

        // Sem identificador aqui o web nem pergunta: adicionar seria só a entrada no aparelho.
        let switches = (can_talk && !repeated).then(|| div().flex().flex_col().gap(px(12.))
            .child(toggle(tr("machines_peer_show_sessions"), format!("{} {next}", tr("machines_add_follow_help")),
                next_tip("machines-add-follow", Switch::new("machines-add-follow-switch").checked(false).disabled(true)
                    .accessibility_label(format!("{}. {next}", tr("machines_peer_show_sessions"))))))
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
            let (label, blocked) = if repeated { (tr("machines_add_use_address"), true) } else { (tr("machines_add_add"), !can_talk) };
            let button = Button::new("machines-add-add").primary().small().label(label.clone()).loading(busy).disabled(blocked)
                .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx)));
            // Os dois caminhos desligados são os da entrada neste aparelho.
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
            dialog.w(px(600.)).title(tr("machines_add_device")).child(add.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
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
    use super::{Discovered, EntityId, Remote, Target, normalize, owns, short_host};

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
