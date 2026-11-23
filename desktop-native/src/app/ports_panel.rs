//! Seção Portas da aba Contexto: os servidores que a sessão abriu, para abrir no navegador embutido, encaminhar a
//! este computador ou mandar para fora pela prévia do Connect.
use super::*;
use crate::port_forward::{ForwardError, PortForwards};

/// A prévia do Connect que esta tela pediu: o link só existe aqui, o servidor guarda o hash.
pub(super) struct PortLink { pub server: String, pub port: u16, pub url: String }

#[derive(Default)]
pub(super) struct Ports {
    /// Um registro para o app inteiro, pela identidade de cada servidor.
    pub forwards: PortForwards,
    /// A última ação que a pessoa fez nessa porta falhou: (servidor, porta) → motivo.
    failed: HashMap<(String, u16), String>,
    /// "Link de fora" em voo, um por vez.
    link_busy: Option<(String, u16)>,
    pub link: Option<PortLink>,
    other: Option<(Entity<InputState>, Subscription)>,
    other_error: Option<String>,
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
pub(crate) struct PortButtons { pub open: Button, pub forward: Button, pub stop: Button, pub copy: Button, pub link: Button }

pub(crate) fn port_buttons(port: u16) -> PortButtons {
    let id = |what: &str| SharedString::from(format!("ports-{what}-{port}"));
    PortButtons {
        open: Button::new(id("open")).xsmall().outline().label(tr("ports_open")),
        forward: Button::new(id("forward")).xsmall().ghost().label(tr("ports_forward")),
        stop: Button::new(id("stop")).xsmall().ghost().label(tr("ports_stop")),
        copy: Button::new(id("copy")).xsmall().ghost().label(tr("ports_copy")),
        link: Button::new(id("link")).xsmall().ghost().label(tr("ports_link")).tooltip(tr("ports_link_tip")),
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
        ForwardError::Refused(502) => tr("ports_err_nothing"),
        ForwardError::Refused(404) => tr("ports_err_old_server"),
        ForwardError::Refused(422) => tr("ports_err_port_refused"),
        ForwardError::Refused(status) => tr("ports_err_refused").replace("{status}", &status.to_string()),
        ForwardError::Network => tr("ports_err_network"),
        ForwardError::Dropped => tr("ports_err_dropped"),
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

    fn forward_port(&mut self, port: u16) -> Option<u16> {
        let api = self.session_api()?;
        let key = (api.identity(), port);
        // O token vai na URL do túnel: é o da máquina da sessão.
        let token = self.server_token(&self.open_server());
        match self.ports.forwards.open(self.runtime.handle(), &api, &token, port) {
            Ok(local) => { self.ports.failed.remove(&key); Some(local) }
            Err(error) => { self.ports.failed.insert(key, tr("ports_err_listen").replace("{error}", &error.to_string())); None }
        }
    }

    /// Servidor desta máquina: a página direto; de outra, pelo encaminhamento até este computador.
    fn open_port(&mut self, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        let local = if self.session_is_remote() { self.forward_port(port) } else { Some(port) };
        if let Some(local) = local { self.open_port_page(format!("http://localhost:{local}"), window, cx); }
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

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn open_port_page(&mut self, url: String, _window: &mut Window, cx: &mut Context<Self>) { cx.open_url(&url); }

    fn forward_other(&mut self, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.ports.other else { return };
        let port = super::connect::parse_port(&input.read(cx).value());
        self.ports.other_error = None;
        match port {
            None => self.ports.other_error = Some(tr("connect_preview_port_invalid")),
            Some(port) => if let Some(api) = self.session_api() && self.forward_port(port).is_none() {
                // O motivo fica embaixo do campo: a porta digitada não vira linha sem encaminhamento.
                self.ports.other_error = self.ports.failed.remove(&(api.identity(), port));
            },
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

    pub(super) fn render_ports(&mut self, beta: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (api, name) = (self.session_api()?, self.selected.as_ref()?.name.clone());
        let server = api.identity();
        // Convidado não tem portas nem túnel: a seção mentiria "nenhuma porta" e o Encaminhar daria erro.
        if !owner_server(&self.servers, &servers::norm(&server)) { return None; }
        let remote = !api.is_loopback();
        let list = self.sessions_of(&server);
        let row = list.iter().find(|s| s.name == name);
        let list_error = if self.is_active_key(&servers::norm(&server)) { self.list_error.clone() }
            else { self.remote.get(&servers::norm(&server)).and_then(|l| l.error.clone()) };
        let mut ports: Vec<(u16, String)> = row.map(|r| r.ports.iter().map(|p| (p.port, p.process.clone())).collect()).unwrap_or_default();
        let forwards = self.ports.forwards.list(&server);
        // Porta digitada em "Outra porta…" não é da sessão: aparece enquanto estiver encaminhada.
        for forward in &forwards { if !ports.iter().any(|(port, _)| *port == forward.port) { ports.push((forward.port, String::new())); } }
        ports.sort_by_key(|(port, _)| *port);
        let title = if beta { super::side::beta_title(tr("ports_title")).mb_0() } else { chrome::section_label(tr("ports_title")) };
        let body = match row {
            None => match list_error {
                Some(error) => port_note(tr("ports_error").replace("{error}", &error), theme::warning()).into_any_element(),
                None => port_note(tr("ports_loading"), theme::muted()).into_any_element(),
            },
            Some(_) if ports.is_empty() => port_note(tr("ports_empty"), theme::muted()).into_any_element(),
            Some(_) => div().flex().flex_col().gap(px(10.)).children(ports.into_iter().map(|(port, process)| {
                let forward = forwards.iter().find(|f| f.port == port);
                let local = forward.map(|f| f.local).or((!remote).then_some(port));
                let failure = self.ports.failed.get(&(server.clone(), port)).cloned()
                    .or_else(|| forward.and_then(|f| f.error).map(forward_text));
                let link = self.ports.link.as_ref().filter(|l| l.server == server && l.port == port).map(|l| l.url.clone());
                let busy = self.ports.link_busy.as_ref().is_some_and(|(s, p)| *s == server && *p == port);
                let label = if process.is_empty() { port.to_string() } else { format!("{port} · {process}") };
                let server_close = server.clone();
                let buttons = port_buttons(port);
                port_row_view(label, forward.filter(|f| f.local != port).map(|f| f.local))
                    .child(port_actions()
                        .child(buttons.open.on_click(cx.listener(move |this, _, window, cx| this.open_port(port, window, cx))))
                        .when(remote, |el| el.child(match forward {
                            Some(_) => buttons.stop
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ports.forwards.close(&server_close, port);
                                    this.ports.failed.remove(&(server_close.clone(), port));
                                    cx.notify();
                                })),
                            None => buttons.forward
                                .on_click(cx.listener(move |this, _, _, cx| { this.forward_port(port); cx.notify(); })),
                        }))
                        .when_some(local, |el, local| el.child(buttons.copy
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(format!("localhost:{local}"))))))
                        .child(buttons.link.loading(busy)
                            .disabled(self.ports.link_busy.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.request_port_link(port, cx)))))
                    .when_some(failure, |el, text| el.child(port_note(text, theme::warning())))
                    .when_some(link, |el, url| {
                        let (copied, opened) = (url.clone(), url.clone());
                        let (copy, open) = port_link_buttons(port, cx);
                        el.child(port_link_view(url,
                            copy.on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))),
                            open.on_click(move |_, _, cx| cx.open_url(&opened))))
                    })
                    .into_any_element()
            })).into_any_element(),
        };
        // Encaminhar um número digitado só faz sentido de outra máquina: aqui a porta já é deste computador.
        let other = remote.then(|| {
            let input = match &self.ports.other {
                Some((input, _)) => input.clone(),
                None => {
                    let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("ports_other")));
                    let sub = cx.subscribe(&input, |this: &mut Hangar, _, event: &InputEvent, cx| {
                        if let InputEvent::PressEnter { .. } = event { this.forward_other(cx); }
                    });
                    self.ports.other = Some((input.clone(), sub));
                    input
                }
            };
            div().flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap_1()
                    .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall().aria_label(tr("ports_other"))))
                    .child(Button::new("ports-other-forward").xsmall().outline().flex_shrink_0().label(tr("ports_forward"))
                        .on_click(cx.listener(|this, _, _, cx| this.forward_other(cx)))))
                .children(self.ports.other_error.clone().map(|text| port_note(text, theme::warning())))
        });
        Some(div().flex().flex_col().gap(px(10.)).child(title).child(body).children(other).into_any_element())
    }
}
