//! Convite de sessão compartilhada, do lado de quem recebe: o link vira um servidor marcado `invite`, com a sessão do dono
//! na lista junto das próprias.
use super::*;
use super::machines::enter_to_focused;

/// Endereço e código do link `https://host:porta/convite/CÓDIGO` ou `hangar://convite/host:porta/CÓDIGO`.
pub(super) fn parse_invite_link(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim().trim_end_matches('/');
    // Convite pela rede local vem em http://IP-privado:8766; http para qualquer outro host não passa.
    let private = |host: &str| host.split_once(':').map_or(host, |(h, _)| h).parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_private());
    let (address, code) = if let Some(rest) = raw.strip_prefix("hangar://convite/") {
        let (host, code) = rest.rsplit_once('/')?;
        (format!("{}://{host}", if private(host) { "http" } else { "https" }), code.to_owned())
    } else {
        let url = url::Url::parse(raw).ok()?;
        let local_http = url.scheme() == "http" && url.host_str().is_some_and(private);
        if (url.scheme() != "https" && !local_http) || !url.username().is_empty() || url.password().is_some() { return None; }
        let mut parts = url.path_segments()?;
        let (Some("convite"), Some(code), None) = (parts.next(), parts.next(), parts.next()) else { return None };
        (url.origin().ascii_serialization(), code.to_owned())
    };
    // O endereço vira o servidor gravado: credencial embutida e host torto não passam, como no web.
    let host_ok = url::Url::parse(&address).ok().is_some_and(|u| u.host_str().is_some_and(|h| !h.is_empty()) && u.path() == "/"
        && u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none());
    let code_ok = !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (host_ok && code_ok).then_some((address, code))
}

/// Num servidor de convite só 401 e 410 encerram. 502 (Funnel com o servidor Rust reiniciando), 503 e conexão recusada
/// são tentar de novo.
pub(super) fn ends_invite(status: Option<u16>) -> bool { matches!(status, Some(401 | 410)) }

/// O nome desta máquina, que o dono vê na lista de quem entrou.
pub(super) fn device_label() -> String {
    let name = if cfg!(windows) { std::env::var("COMPUTERNAME").ok() }
        else { std::fs::read_to_string("/etc/hostname").ok().or_else(|| std::env::var("HOSTNAME").ok()) };
    name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Hangar".into())
}

/// Este endereço já é um servidor do próprio usuário (não convite): o resgate nem sai, para não gastar o código, e a entrada
/// dele nunca é tocada.
fn own_server_at(list: &[servers::ServerEntry], address: &str) -> bool {
    list.iter().any(|s| !s.invite && servers::norm(&s.address) == servers::norm(address))
}

/// Token de convite que este app já tem para a máquina: o resgate novo entra nele em vez de trocar de sessão. O token só do
/// par não serve: o convite resgatado nele sumiria junto com o par.
pub(super) fn existing_invite_token(list: &[servers::ServerEntry], address: &str) -> Option<String> {
    let key = servers::norm(address);
    list.iter().find(|s| s.invite && !s.ephemeral && servers::norm(&s.address) == key).map(|s| s.token.clone()).filter(|t| !t.is_empty())
}

/// Coloca o convite na lista. Já existindo um convite no endereço (mesmo dono), a entrada é a mesma: o token devolvido é o
/// que o app mandou, e o rótulo se atualiza.
/// Falso quando o endereço é de um servidor próprio, que fica como está.
fn place_invite(list: &mut Vec<servers::ServerEntry>, entry: servers::ServerEntry) -> bool {
    if own_server_at(list, &entry.address) { return false; }
    servers::upsert(list, entry);
    true
}

/// 404, 410 e 503 têm frase própria: código errado, convite gasto e túnel do dono fora do ar pedem coisas diferentes de quem
/// recebeu (só o último vale tentar de novo, com o mesmo código).
fn redeem_failure(error: &Failure) -> String {
    match error.status {
        // O servidor manda o código (usado, vencido, revogado, inexistente) e o texto sai no idioma da tela, como no web.
        Some(404 | 410) if error.detail.starts_with("erro_convite_") => tr_shared(&error.detail, &[]),
        Some(404) => tr_shared("erro_convite_inexistente", &[]),
        Some(410) => tr_shared("erro_convite_encerrado", &[]),
        Some(503) => tr_shared("erro_sessao_indisponivel", &[]),
        None if matches!(error.detail.as_str(), "network_error" | "delivery_uncertain") => tr_shared("convite_erro_rede", &[]),
        _ => Hangar::failure(error),
    }
}

pub(super) struct InviteDialog { hangar: WeakEntity<Hangar>, input: Entity<InputState>, busy: bool, error: Option<String> }

impl InviteDialog {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy { return; }
        // Link de par colado onde se colam convites: este diálogo cede lugar ao de par.
        if let Some(link) = super::pair_accept::parse_pair_link(&self.input.read(cx).value()) {
            window.close_all_dialogs(cx);
            let _ = self.hangar.update(cx, |this, cx| this.open_pair_accept_dialog(None, Some(link), window, cx));
            return;
        }
        let Some((address, code)) = parse_invite_link(&self.input.read(cx).value()) else {
            self.error = Some(tr_shared("convite_link_invalido", &[]));
            cx.notify();
            return;
        };
        if self.hangar.read_with(cx, |hangar, _| own_server_at(&hangar.servers, &address)).unwrap_or(false) {
            self.error = Some(tr("invite_own_server"));
            cx.notify();
            return;
        }
        (self.busy, self.error) = (true, None);
        let me = cx.entity().downgrade();
        let _ = self.hangar.update(cx, |this, cx| this.redeem_invite(me, address, code, window, cx));
        cx.notify();
    }
}

impl Render for InviteDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_3()
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr_shared("convite_colar_ajuda", &[])))
            .child(Input::new(&self.input).aria_label(tr_shared("convite_campo_aria", &[])))
            .when(self.busy, |el| el.child(div().id("invite-busy").role(Role::Status).text_sm().text_color(theme::muted()).child(tr_shared("convite_entrando", &[]))))
            .when_some(self.error.clone(), |el, error| el.child(div().id("invite-error").role(Role::Alert).text_sm()
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().justify_end().child(Button::new("invite-enter").primary().label(tr_shared("convite_entrar", &[])).disabled(self.busy)
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))))
    }
}

impl Hangar {
    /// Aberto pelo botão da página Máquinas e pelo link `hangar://`: o link chega preenchido e só entra com o clique.
    pub(super) fn open_invite_dialog(&mut self, link: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let hangar = cx.entity().downgrade();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr_shared("convite_placeholder", &[])).default_value(link.unwrap_or_default()));
        let dialog = cx.new(|_| InviteDialog { hangar, input: input.clone(), busy: false, error: None });
        window.open_dialog(cx, move |d, _, cx| {
            let busy = dialog.read(cx).busy;
            let confirm = dialog.clone();
            popup::dialog(d).w(px(520.)).title(tr_shared("convite_colar_titulo", &[])).child(dialog.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(move |event, window, cx| {
                    // Enter no campo entra; em outro controle segue para ele, como no resto do app.
                    if confirm.read(cx).input.read(cx).focus_handle(cx).is_focused(window) { confirm.update(cx, |d, cx| d.submit(window, cx)); false }
                    else { enter_to_focused(event, window, cx) }
                })
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn redeem_invite(&mut self, dialog: WeakEntity<InviteDialog>, address: String, code: String, window: &mut Window, cx: &mut Context<Self>) {
        let device = device_label();
        let existing = existing_invite_token(&self.servers, &address);
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            // O endereço devolvido vira o servidor que recebe o token: só vale se for o mesmo host do convite.
            let host = |a: &str| url::Url::parse(a).ok().map(|u| (u.host_str().map(str::to_owned), u.port_or_known_default()));
            let result = api::redeem_invite(&address, &code, &device, existing.as_deref()).await.map(|mut r| {
                if host(&r.address).is_none() || host(&r.address) != host(&address) { r.address = address.clone(); }
                r
            });
            let _ = done.send(result);
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(redeemed) => { this.forget_question(); window.close_all_dialogs(cx); this.add_invite_server(redeemed, window, cx); }
                Err(error) => { let _ = dialog.update(cx, |d, cx| { (d.busy, d.error) = (false, Some(redeem_failure(&error))); cx.notify(); }); }
            });
        }).detach();
    }

    /// O convite vira um servidor a mais e a sessão compartilhada abre na hora, como a de qualquer outra máquina.
    fn add_invite_server(&mut self, redeemed: api::Redeemed, window: &mut Window, cx: &mut Context<Self>) {
        let key = servers::norm(&redeemed.address);
        self.invite_ended.remove(&key);
        let entry = servers::ServerEntry { id: servers::new_id(), label: tr_shared("convite_rotulo", &[("dono", &redeemed.owner)]),
            address: redeemed.address, token: redeemed.token, disabled: false, invite: true, lan: None, ephemeral: false, ..Default::default() };
        // Um servidor próprio pode ter entrado na lista enquanto o resgate corria: ele fica intacto.
        if !place_invite(&mut self.servers, entry) {
            window.push_notification(Notification::warning(tr("invite_own_server")), cx);
            return;
        }
        // A entrada só do par virou a do convite (o upsert a torna gravada); o par entra nela pelo attach da reconciliação.
        self.apply_external_pairs(cx);
        self.servers_rev += 1;
        self.persist_servers();
        self.start_remote_lists();
        self.open_remote(&key, redeemed.session, window, cx);
    }

    pub(super) fn active_invite(&self) -> bool {
        self.server.as_deref().and_then(|a| self.server_entry(&servers::norm(a))).is_some_and(|s| s.invite)
    }

    /// Num servidor de convite, 401 e 410 são o compartilhamento que acabou e 403 é rota fora do convite: nenhum abre a tela
    /// de conexão. Nos outros, 401/403 continuam sendo login perdido.
    pub(super) fn auth_lost(&mut self, error: &Failure) -> bool {
        if !self.active_invite() { return matches!(error.status, Some(401 | 403)); }
        if ends_invite(error.status) && let Some(address) = self.server.as_deref() {
            self.invite_ended.insert(servers::norm(address));
            self.sessions.clear();
        }
        false
    }

    /// Texto da falha na conexão ativa: num convite, 401/410 é o compartilhamento que acabou, não login perdido.
    pub(super) fn active_failure(&self, error: &Failure) -> String {
        if self.active_invite() && ends_invite(error.status) { tr_shared("convite_encerrado", &[]) } else { Self::failure(error) }
    }

    fn open_invite(&self) -> bool {
        self.open_api.as_ref().and_then(|api| self.server_entry(&servers::norm(&api.identity()))).is_some_and(|s| s.invite)
    }

    /// Falha na sessão aberta. Numa sessão de outra máquina o login do servidor ativo não tem culpa: nunca abre a conexão.
    pub(super) fn chat_auth_lost(&mut self, error: &Failure) -> bool {
        let Some(key) = self.open_api.as_ref().map(|api| servers::norm(&api.identity())) else { return self.auth_lost(error) };
        if self.open_invite() && ends_invite(error.status) { self.invite_ended.insert(key); }
        false
    }

    pub(super) fn chat_failure(&self, error: &Failure) -> String {
        if self.open_api.is_none() { return self.active_failure(error); }
        if self.open_invite() && ends_invite(error.status) { tr_shared("convite_encerrado", &[]) } else { Self::failure(error) }
    }

    /// Parar de acompanhar um convite: sai só deste aparelho; a sessão do dono continua viva.
    pub(super) fn stop_following(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.server_entry(key).is_some_and(|s| s.invite) || self.server.as_deref().map(servers::norm).as_deref() == Some(key) { return; }
        if self.open_key().as_deref() == Some(key) { self.close_open_session(window, cx); }
        if self.pending_remote.as_ref().is_some_and(|(want, _)| want == key) { self.pending_remote = None; }
        self.servers.retain(|s| servers::norm(&s.address) != key);
        self.invite_ended.remove(key);
        self.servers_rev += 1;
        self.persist_servers();
        self.start_remote_lists();
        // O par que estava nesta entrada volta na entrada só dele.
        self.apply_external_pairs(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn entry(id: &str, label: &str, address: &str, token: &str, invite: bool) -> servers::ServerEntry {
        servers::ServerEntry { id: id.into(), label: label.into(), address: address.into(), token: token.into(), disabled: false, invite, lan: None, ephemeral: false, ..Default::default() }
    }

    #[test]
    fn only_401_and_410_end_an_invite() {
        assert!(ends_invite(Some(401)) && ends_invite(Some(410)));
        for status in [Some(502), Some(503), Some(500), None] { assert!(!ends_invite(status), "{status:?}"); }
    }

    #[test]
    fn an_own_server_at_the_address_blocks_the_invite_and_keeps_its_token() {
        let mut list = vec![entry("a", "PC", "https://h:8443", "own-token", false)];
        assert!(own_server_at(&list, "https://H:8443/"));
        assert!(!own_server_at(&list, "https://outro:8443"));
        assert!(!place_invite(&mut list, entry("i", "Convite · Ana", "https://h:8443/", "guest", true)));
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].token.as_str(), list[0].invite), ("own-token", false));
    }

    #[test]
    fn a_newer_invite_from_the_same_owner_replaces_token_and_label() {
        let mut list = vec![entry("i", "Convite · Ana", "https://h:8443", "old", true), entry("a", "PC", "http://127.0.0.1:8765", "own", false)];
        assert!(!own_server_at(&list, "https://h:8443"));
        assert!(place_invite(&mut list, entry("j", "Convite · Ana B", "https://h:8443/", "new", true)));
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].id.as_str(), list[0].token.as_str(), list[0].label.as_str(), list[0].invite), ("i", "new", "Convite · Ana B", true));
    }

    #[test]
    fn existing_invite_token_only_from_invite_entries_at_that_address() {
        let list = vec![entry("i", "Convite", "https://a.ts.net:8443", "t1", true), entry("o", "PC", "https://b.ts.net", "own", false)];
        assert_eq!(existing_invite_token(&list, "https://A.ts.net:8443/").as_deref(), Some("t1"));
        assert_eq!(existing_invite_token(&list, "https://b.ts.net"), None);
        assert_eq!(existing_invite_token(&list, "https://c.ts.net:8443"), None);
        let pair_only = vec![servers::ServerEntry { ephemeral: true, ..entry("p", "Par", "https://a.ts.net:8443", "tp", true) }];
        assert_eq!(existing_invite_token(&pair_only, "https://a.ts.net:8443"), None, "o token só do par não recebe convite");
    }

    #[test]
    fn redeem_codes_use_the_web_texts_by_key() {
        let failure = |status, detail: &str| Failure { status: Some(status), detail: detail.into(), retry_after: None, uncertain: false, code: None };
        assert_eq!(redeem_failure(&failure(410, "erro_convite_usado")), tr_shared("erro_convite_usado", &[]));
        assert_eq!(redeem_failure(&failure(404, "erro_convite_inexistente")), tr_shared("erro_convite_inexistente", &[]));
        assert_eq!(redeem_failure(&failure(410, "HTTP 410")), tr_shared("erro_convite_encerrado", &[]));
        assert_eq!(redeem_failure(&failure(503, "erro_sessao_indisponivel")), tr_shared("erro_sessao_indisponivel", &[]));
    }

    #[test]
    fn https_and_deep_link_give_the_same_address_and_code() {
        let expected = Some(("https://notebook.tailcac351.ts.net:8443".to_owned(), "K7P29QX4ABCD".to_owned()));
        assert_eq!(parse_invite_link("https://notebook.tailcac351.ts.net:8443/convite/K7P29QX4ABCD"), expected);
        assert_eq!(parse_invite_link("  hangar://convite/notebook.tailcac351.ts.net:8443/K7P29QX4ABCD/ \n"), expected);
    }

    #[test]
    fn anything_else_is_not_an_invite() {
        for raw in ["", "K7P29QX4ABCD", "http://host:8443/convite/ABC", "https://host:8443/api/sessions",
            "https://host:8443/convite/", "https://host:8443/convite/AB C", "hangar://convite/host:8443",
            "hangar://outra/host:8443/ABC", "https://user:pw@host:8443/convite/ABC", "hangar://convite/user@host:8443/ABC"] {
            assert_eq!(parse_invite_link(raw), None, "{raw}");
        }
    }
}
