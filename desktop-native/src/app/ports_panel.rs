//! Seção Portas da aba Contexto: os servidores que a sessão abriu, para abrir no navegador embutido, encaminhar a
//! este computador ou mandar para fora pela prévia do Connect.
use super::*;
use crate::port_forward::{ForwardError, OpenError, Opening, PortForwards};

/// A prévia do Connect que esta tela pediu: o link só existe aqui, o servidor guarda o hash.
pub(super) struct PortLink { pub server: String, pub port: u16, pub url: String }

#[derive(Default)]
pub(super) struct Ports {
    /// Um registro para o app inteiro, pela identidade de cada servidor.
    pub forwards: PortForwards,
    /// O SOCKS de cada servidor remoto, para o navegador das sessões dele.
    #[cfg(target_os = "linux")]
    pub socks: crate::socks::Socks,
    /// A última ação que a pessoa fez nessa porta falhou: (servidor, porta) → motivo.
    failed: HashMap<(String, u16), String>,
    /// "Link de fora" em voo, um por vez.
    link_busy: Option<(String, u16)>,
    pub link: Option<PortLink>,
    other: Option<(Entity<InputState>, Subscription)>,
    other_error: Option<String>,
    /// A última porta digitada em "Outra porta…" (servidor, porta): vira linha mesmo sem a sessão listar (container,
    /// servidor macOS), para abrir e pedir o link de fora.
    /// Chave `servidor::sessão`: a porta digitada numa sessão não aparece nas outras da mesma máquina.
    typed: Option<(String, u16)>,
    /// (servidor, porta) → sessão que pediu o encaminhamento: o túnel é da máquina, a linha fica na sessão que o abriu.
    origin: HashMap<(String, u16), String>,
    /// "Parar servidor" em voo, um por vez: (servidor, porta).
    stopping: Option<(String, u16)>,
}

/// Texto pequeno da seção; as peças abaixo também desenham a animação da novidade.
pub(crate) fn port_note(text: String, tone: Hsla) -> Div { div().text_xs().text_color(tone).whitespace_normal().child(text) }

/// Nome da porta e, quando a porta local difere, "encaminhada como N". As ações entram depois.
pub(crate) fn port_row_view(label: String, forwarded_as: Option<u16>) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().min_w_0().truncate().text_xs().font_family(crate::theme::MONO).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(label))
        .when_some(forwarded_as, |el, local| el.child(port_note(tr("ports_forwarded_as").replace("{port}", &local.to_string()), theme::faint())))
}

pub(crate) fn port_actions() -> Div { div().flex().flex_wrap().gap_1() }

/// Os botões de uma porta, sem ação: quem desenha liga os cliques.
/// `stop` para o encaminhamento; `stop_server`, o processo da porta. `open_system` abre no navegador do computador.
pub(crate) struct PortButtons { pub open: Button, pub open_system: Button, pub forward: Button, pub stop: Button, pub copy: Button, pub link: Button, pub stop_server: Button }

pub(crate) fn port_buttons(port: u16) -> PortButtons {
    let id = |what: &str| SharedString::from(format!("ports-{what}-{port}"));
    PortButtons {
        open: Button::new(id("open")).xsmall().outline().label(tr("ports_open")),
        open_system: Button::new(id("open-system")).xsmall().ghost().label(tr("ports_open_system")),
        forward: Button::new(id("forward")).xsmall().ghost().label(tr("ports_forward")),
        stop: Button::new(id("stop")).xsmall().ghost().label(tr("ports_stop")),
        copy: Button::new(id("copy")).xsmall().ghost().label(tr("ports_copy")),
        link: Button::new(id("link")).xsmall().ghost().label(tr("ports_link")).tooltip(tr("ports_link_tip")),
        stop_server: Button::new(id("stop-server")).xsmall().ghost().label(tr_shared("ports_stop_server", &[])),
    }
}

/// Copiar e abrir do link de fora.
pub(crate) fn port_link_buttons(port: u16, cx: &App) -> (Button, Button) {
    let id = |what: &str| SharedString::from(format!("ports-{what}-{port}"));
    (Button::new(id("link-copy")).xsmall().outline().flex_shrink_0().label(tr("connect_preview_copy")),
     chrome::icon_button(id("link-open"), IconName::ExternalLink, tr("ports_link_open"), cx).flex_shrink_0())
}

/// O link de fora aberto, com o aviso de que só uma prévia fica aberta por vez.
pub(crate) fn port_link_view(url: String, copy: Button, open: Button) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().flex().items_center().gap_1()
            .child(div().flex_1().min_w_0().truncate().text_xs().font_family(crate::theme::MONO).text_color(theme::muted()).child(url))
            .child(copy)
            .child(open))
        .child(port_note(tr("ports_link_one"), theme::faint()))
}

fn forward_text(error: ForwardError) -> String {
    match error {
        ForwardError::Blocked => tr("ports_err_blocked"),
        ForwardError::NothingListening => tr("ports_err_nothing"),
        ForwardError::PortRefused => tr("ports_err_port_refused"),
        ForwardError::TokenRequired => tr("ports_err_token"),
        ForwardError::Unreachable => tr("ports_err_server_down"),
        ForwardError::Refused(status) => tr("ports_err_refused").replace("{status}", &status.to_string()),
        ForwardError::Network => tr("ports_err_network"),
        ForwardError::Dropped => tr("ports_err_dropped"),
        ForwardError::TooMany => tr("ports_err_too_many"),
    }
}

fn open_text(error: OpenError) -> String {
    match error {
        OpenError::SameMachine => tr("ports_err_same_machine"),
        OpenError::Unavailable(Some(reason)) => forward_text(reason),
        OpenError::Unavailable(None) => tr("ports_err_probe"),
        OpenError::Listen(error) => tr("ports_err_listen").replace("{error}", &error),
    }
}

/// Túnel e canal anfitrião são só do dono: a entrada existe e não é de convite nem de par externo.
pub(super) fn owner_server(servers: &[ServerEntry], key: &str) -> bool {
    servers.iter().any(|s| !s.invite && !s.ephemeral && servers::norm(&s.address) == key)
}

impl Hangar {
    /// A senha do dono da máquina `server`, como nos outros sockets dela; nunca a do servidor ativo.
    pub(super) fn server_token(&self, server: &str) -> String {
        if self.is_active_key(server) { self.active_token.clone() } else { self.server_entry(server).map(|s| s.token.clone()).unwrap_or_default() }
    }

    /// A sonda e a busca de porta livre rodam no runtime; enquanto isso a linha diz "abrindo…".
    fn forward_port(&mut self, port: u16, cx: &mut Context<Self>) -> Option<Opening> {
        let Some(api) = self.session_api() else {
            self.no_connection(port);
            return None;
        };
        let key = (api.identity(), port);
        // O token vai na URL do túnel: é o da máquina da sessão.
        let token = self.server_token(&self.open_server());
        self.ports.failed.remove(&key);
        self.ports.failed.remove(&(String::new(), port));
        if let Some(session) = self.side_key() { self.ports.origin.insert(key.clone(), session); }
        let opening = self.ports.forwards.open(self.runtime.handle(), &api, &token, port);
        let result = opening.clone();
        cx.spawn(async move |this, cx| {
            let result = result.wait().await;
            let _ = this.update(cx, |this, cx| {
                // Parado antes de abrir (`None`) não é falha.
                if let Some(Err(error)) = result { this.ports.failed.insert(key, open_text(error)); }
                cx.notify();
            });
        }).detach();
        Some(opening)
    }

    /// Servidor desta máquina: a página direto; de outra, pelo encaminhamento até este computador.
    fn open_port(&mut self, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        // No Linux o navegador da sessão remota tem o SOCKS do servidor (`session_proxy`): o `localhost` dele já é o de
        // lá, com a mesma porta e a mesma origem, sem encaminhar.
        if !self.session_is_remote() || cfg!(target_os = "linux") {
            self.open_port_page(format!("http://localhost:{port}"), window, cx);
        } else if let Some(opening) = self.forward_port(port, cx) {
            cx.spawn_in(window, async move |this, cx| {
                let Some(Ok(local)) = opening.wait().await else { return };
                let _ = this.update_in(cx, |this, window, cx| this.open_port_page(format!("http://localhost:{local}"), window, cx));
            }).detach();
        }
        cx.notify();
    }

    /// No navegador do sistema, que não tem o SOCKS da sessão: de outra máquina, pelo encaminhamento até este computador.
    fn open_port_outside(&mut self, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session_is_remote() {
            super::links::open_link(&format!("http://localhost:{port}"), true, window, cx);
        } else if let Some(opening) = self.forward_port(port, cx) {
            cx.spawn_in(window, async move |_, cx| {
                let Some(Ok(local)) = opening.wait().await else { return };
                let _ = cx.update(|window, cx| super::links::open_link(&format!("http://localhost:{local}"), true, window, cx));
            }).detach();
        }
        cx.notify();
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn open_port_page(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        // A aba Navegador de sessão remota mostra a tela do app de lá; o localhost encaminhado só existe aqui.
        if self.session_is_remote() && let Some(remote) = self.side_key().and_then(|key| self.side.remote_browsers.remove(&key)) {
            remote.update(cx, |view, cx| view.set_shown(false, cx));
        }
        self.open_in_browser_panel(url, window, cx);
    }

    /// O proxy do navegador da sessão `key` (`servidor::sessão`): a porta do SOCKS do servidor quando ele é de outra
    /// máquina e do dono; `None` para os desta máquina. Convite e par externo não têm túnel: ficam sem proxy, e o
    /// "Abrir" deles nem aparece.
    #[cfg(target_os = "linux")]
    pub(super) fn session_proxy(&mut self, key: &str) -> Option<Result<u16, String>> {
        let server = key.rsplit_once("::").map(|(server, _)| server)?;
        if !owner_server(&self.servers, server) { return None; }
        // Sem saber a conexão não dá para dizer se é desta máquina: abrir sem proxy mostraria o `localhost` daqui.
        let Some(api) = self.machine_api(server) else { return Some(Err("sem conexão com a máquina da sessão".into())) };
        if api.is_loopback() { return None; }
        let token = self.server_token(server);
        Some(self.ports.socks.ensure(self.runtime.handle(), &api, &token))
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn open_port_page(&mut self, url: String, _window: &mut Window, cx: &mut Context<Self>) { cx.open_url(&url); }

    fn forward_other(&mut self, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.ports.other else { return };
        let port = super::connect::parse_port(&input.read(cx).value());
        self.ports.other_error = None;
        match (port, self.session_api()) {
            (None, _) => self.ports.other_error = Some(tr("connect_preview_port_invalid")),
            (Some(_), None) => self.ports.other_error = Some(tr("ports_err_network")),
            // Daqui a porta já é deste computador: só vira linha. De outra máquina, também encaminha.
            (Some(port), Some(api)) => {
                let Some(key) = self.side_key() else { return };
                self.ports.typed = Some((key.clone(), port));
                if !api.is_loopback() { self.forward_port(port, cx); }
                // A linha nasce na seção Portas do Contexto: menu e aba de subagente saem da frente para ela aparecer.
                self.show_side_tab_here(key, crate::appearance::SideTab::Context, cx);
                // Campo novo, vazio: o número não fica esperando um Enter que o mande de novo, noutra sessão.
                self.ports.other = None;
            }
        }
        cx.notify();
    }

    fn request_port_link(&mut self, port: u16, cx: &mut Context<Self>) {
        if self.ports.link_busy.is_some() { return; }
        let Some(api) = self.session_api() else { return };
        let server = api.identity();
        self.ports.failed.remove(&(server.clone(), port));
        self.ports.link_busy = Some((server.clone(), port));
        let done = self.connect_send_later();
        self.runtime.spawn(async move {
            let result = super::connect::request_preview(&api, port).await;
            done(super::connect::ConnectReply::PortLink(server, port, result)).await
        });
        cx.notify();
    }

    pub(super) fn port_link_done(&mut self, server: String, port: u16, result: Result<String, String>) {
        self.ports.link_busy = None;
        match result {
            Ok(url) => self.ports.link = Some(PortLink { server, port, url }),
            Err(error) => { self.ports.failed.insert((server, port), error); }
        }
    }

    /// Fechada a prévia em Ajustes, o link desta tela parou de valer.
    pub(super) fn port_link_closed(&mut self, server: &str) {
        if self.ports.link.as_ref().is_some_and(|link| link.server == server) { self.ports.link = None; }
    }

    pub(super) fn render_ports(&mut self, beta: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (api, name) = (self.session_api()?, self.selected.as_ref()?.name.clone());
        let server = api.identity();
        // Convidado não tem portas nem túnel: a seção mentiria "nenhuma porta" e o Encaminhar daria erro.
        if !owner_server(&self.servers, &servers::norm(&server)) { return None; }
        let remote = !api.is_loopback();
        let list = self.sessions_of(&server);
        let row = list.iter().find(|s| s.name == name);
        let list_error = if self.is_active_key(&servers::norm(&server)) { self.list_error.clone() }
            else { self.remote.get(&servers::norm(&server)).and_then(|l| l.error.clone()) };
        let mut ports: Vec<(u16, String, bool)> = row.map(|r| r.ports.iter().map(|p| (p.port, p.process.clone(), p.shared)).collect()).unwrap_or_default();
        let forwards = self.ports.forwards.list(&server);
        let opening = self.ports.forwards.opening(&server);
        // Porta que a sessão não lista: a última digitada nela e as que ela encaminhou (ou está abrindo).
        let session_key = self.side_key();
        let mine = |port: &u16| self.ports.origin.get(&(server.clone(), *port)) == session_key.as_ref();
        let typed = self.ports.typed.as_ref().filter(|(key, _)| Some(key) == session_key.as_ref()).map(|(_, port)| *port);
        let extra: Vec<u16> = forwards.iter().map(|f| f.port).chain(opening.iter().copied()).filter(mine).chain(typed).collect();
        for port in extra {
            if !ports.iter().any(|(known, ..)| *known == port) { ports.push((port, String::new(), false)); }
        }
        ports.sort_by_key(|(port, ..)| *port);
        // Sem porta não há seção: a lista chegando ou vazia só ocupava espaço. "Outra porta…" mora no "+" do painel.
        if ports.is_empty() && (row.is_some() || list_error.is_none()) { return None; }
        let title = if beta { super::side::beta_title(tr("ports_title")).mb_0() } else { chrome::section_label(tr("ports_title")) };
        // A lista com erro não esconde a porta digitada: o aviso vai acima das linhas.
        let error = list_error.filter(|_| row.is_none()).map(|error| port_note(tr("ports_error").replace("{error}", &error), theme::warning()));
        let body = div().flex().flex_col().gap(px(10.)).children(error).children(ports.into_iter().map(|(port, process, shared)| {
                let forward = forwards.iter().find(|f| f.port == port);
                let is_opening = opening.contains(&port);
                let local =forward.map(|f| f.local).or((!remote).then_some(port));
                let failure = self.ports.failed.get(&(server.clone(), port))
                    .or_else(|| self.ports.failed.get(&(String::new(), port))).cloned()
                    .or_else(|| forward.and_then(|f| f.error).map(forward_text));
                let link = self.ports.link.as_ref().filter(|l| l.server == server && l.port == port).map(|l| l.url.clone());
                let busy = self.ports.link_busy.as_ref().is_some_and(|(s, p)| *s == server && *p == port);
                let label = if process.is_empty() { port.to_string() } else { format!("{port} · {process}") };
                let server_close = server.clone();
                let buttons = port_buttons(port);
                port_row_view(label, forward.filter(|f| f.local != port).map(|f| f.local))
                    .child(port_actions()
                        .child(buttons.open.on_click(cx.listener(move |this, _, window, cx| this.open_port(port, window, cx))))
                        .child(buttons.open_system.loading(is_opening).disabled(is_opening)
                            .on_click(cx.listener(move |this, _, window, cx| this.open_port_outside(port, window, cx))))
                        .when(remote, |el| el.child(match forward {
                            Some(_) => buttons.stop
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ports.forwards.close(&server_close, port);
                                    this.ports.failed.remove(&(server_close.clone(), port));
                                    cx.notify();
                                })),
                            None => buttons.forward.loading(is_opening).disabled(is_opening)
                                .on_click(cx.listener(move |this, _, _, cx| { this.forward_port(port, cx); cx.notify(); })),
                        }))
                        .when_some(local, |el, local| el.child(buttons.copy
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(format!("localhost:{local}"))))))
                        .child(buttons.link.loading(busy)
                            .disabled(self.ports.link_busy.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.request_port_link(port, cx))))
                        // Só a porta da sessão: a digitada em "Outra porta…" não é dela.
                        .when(row.is_some_and(|r| r.ports.iter().any(|p| p.port == port)), |el| el.child(buttons.stop_server
                            .loading(self.ports.stopping.as_ref().is_some_and(|(s, p)| *s == server && *p == port))
                            .disabled(self.ports.stopping.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| this.confirm_stop_server(port, window, cx))))))
                    .when(shared, |el| el.child(port_note(tr("ports_shared"), theme::muted())))
                    .when(is_opening, |el| el.child(port_note(tr("ports_opening"), theme::muted())))
                    .when_some(failure, |el, text| el.child(port_note(text, theme::warning())))
                    .when_some(link, |el, url| {
                        let (copied, opened) = (url.clone(), url.clone());
                        let (copy, open) = port_link_buttons(port, cx);
                        el.child(port_link_view(url,
                            copy.on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))),
                            open.on_click(move |_, _, cx| cx.open_url(&opened))))
                    })
                    .into_any_element()
            }));
        Some(div().flex().flex_col().gap(px(10.)).child(title).child(body).into_any_element())
    }

    pub(super) fn clear_other_port_error(&mut self) { self.ports.other_error = None; }

    /// Linha do "+" do painel: só na sessão do dono que ele pode mexer, como a seção Portas (a orq não tem a seção).
    pub(super) fn side_menu_ports(&self) -> bool {
        self.selected.as_ref().is_some_and(|s| s.readable() && !s.read_only() && !s.orq())
            && self.session_api().is_some_and(|api| owner_server(&self.servers, &servers::norm(&api.identity())))
    }

    /// Porta que a sessão não listou (container, servidor macOS): a digitada vira linha, com Abrir e Link de fora.
    pub(super) fn render_other_port(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let remote = self.session_api().is_some_and(|api| !api.is_loopback());
        let input = match &self.ports.other {
            Some((input, _)) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("ports_number")));
                let sub = cx.subscribe(&input, |this: &mut Hangar, _, event: &InputEvent, cx| {
                    if let InputEvent::PressEnter { .. } = event { this.forward_other(cx); }
                });
                self.ports.other = Some((input.clone(), sub));
                input
            }
        };
        div().flex().flex_col().gap(px(6.))
            .child(div().px(px(4.)).text_size(px(11.)).text_color(theme::muted()).child(tr("ports_other")))
            .child(div().flex().items_center().gap_1()
                .child(div().flex_1().min_w_0().child(Input::new(&input).small().aria_label(tr("ports_other"))))
                .child(Button::new("ports-other-forward").small().outline().flex_shrink_0()
                    .label(if remote { tr("ports_forward") } else { tr("ports_other_add") })
                    .on_click(cx.listener(|this, _, _, cx| this.forward_other(cx)))))
            .children(self.ports.other_error.clone().map(|text| port_note(text, theme::warning())))
    }
}

fn stop_server_text(failure: &Failure) -> String {
    match failure.code.as_deref() {
        Some("port_stop_not_found") => tr_shared("ports_stop_err_not_found", &[]),
        Some("port_stop_shared_group") => tr_shared("ports_stop_err_shared", &[]),
        Some("port_stop_unsupported") => tr_shared("ports_stop_err_unsupported", &[]),
        // Sem resposta do servidor, o `detail` é a chave da frase; com resposta, já é a frase traduzida.
        _ => {
            let error = if failure.status.is_none() { tr(&failure.detail) } else { failure.detail.clone() };
            tr_shared("ports_stop_err_failed", &[("error", &error)])
        }
    }
}

impl Hangar {
    fn confirm_stop_server(&mut self, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        if self.ports.stopping.is_some() { return; }
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr_shared("ports_stop_server_title", &[("port", &port.to_string())]),
            tr_shared("ports_stop_server_desc", &[]), tr_shared("ports_stop_server", &[]), ButtonVariant::Danger,
            move |_, cx| { let _ = this.update(cx, |this, cx| this.stop_server(port, cx)); true });
    }

    /// Sem conexão não há identidade do servidor: a nota fica na porta com servidor vazio e sai na próxima ação nela.
    fn no_connection(&mut self, port: u16) { self.ports.failed.insert((String::new(), port), tr("ports_err_network")); }

    /// O servidor relê quem segura a porta; a linha some na próxima lista.
    fn stop_server(&mut self, port: u16, cx: &mut Context<Self>) {
        if self.ports.stopping.is_some() { return; }
        let (Some(api), Some(name)) = (self.session_api(), self.selected.as_ref().map(|s| s.name.clone())) else {
            self.no_connection(port);
            cx.notify();
            return;
        };
        let server = api.identity();
        self.ports.failed.remove(&(server.clone(), port));
        self.ports.failed.remove(&(String::new(), port));
        self.ports.stopping = Some((server.clone(), port));
        let task = self.runtime.spawn(async move { api.act(&name, &["ports", &port.to_string(), "stop"], None, false, 20).await });
        cx.spawn(async move |this, cx| {
            let failure = match task.await {
                Ok(Ok(_)) => None,
                Ok(Err(failure)) => Some(stop_server_text(&failure)),
                Err(_) => Some(tr_shared("ports_stop_err_failed", &[("error", "")])),
            };
            let _ = this.update(cx, |this, cx| {
                this.ports.stopping = None;
                if let Some(text) = failure { this.ports.failed.insert((server, port), text); }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}
