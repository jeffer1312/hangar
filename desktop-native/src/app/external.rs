//! Par externo do lado de quem o tem: a sessão da outra pessoa aparece abaixo do grupo e abre pelo servidor de convite da
//! máquina dela, só leitura.
use super::*;

/// Monta ou remove as entradas só do par e devolve os attach pendentes (endereço, token da entrada, token do par).
pub(super) fn reconcile_pair_entries(list: &mut Vec<servers::ServerEntry>, pairs: &[api::ExternalPairDto]) -> Vec<(String, String, String)> {
    let at = |address: &str| { let key = servers::norm(address); pairs.iter().filter(move |p| servers::norm(&p.address) == key) };
    list.retain(|s| !s.ephemeral || at(&s.address).next().is_some());
    let mut pending = Vec::new();
    for p in pairs {
        let key = servers::norm(&p.address);
        match list.iter_mut().find(|s| servers::norm(&s.address) == key) {
            // Servidor próprio no mesmo endereço: o token do par não entra nele.
            Some(s) if !s.invite => {}
            Some(s) => {
                // Par que acabou levaria a entrada junto: ela passa ao token de um par vivo.
                if s.ephemeral && !at(&s.address).any(|q| q.token == s.token) { s.token = p.token.clone(); }
                if s.token != p.token { pending.push((s.address.clone(), s.token.clone(), p.token.clone())); }
            }
            None => list.push(servers::ServerEntry { id: servers::new_id(), label: tr_shared("native_par_rotulo", &[("dono", &p.owner)]),
                address: p.address.clone(), token: p.token.clone(), disabled: false, invite: true, lan: None, connect: None, connect_refused: false, ephemeral: true }),
        }
    }
    pending
}

/// Servidor próprio ligado: só ele tem pares lidos e entradas de par.
fn is_own(s: &servers::ServerEntry) -> bool { !s.invite && !s.disabled }

/// Donos dos pares que existiam e sumiram entre duas leituras: a outra pessoa saiu (ou o par acabou do lado dela).
pub(super) fn left_owners(before: &[(String, api::ExternalPairDto)], after: &[(String, api::ExternalPairDto)]) -> Vec<String> {
    let same = |a: &(String, api::ExternalPairDto), b: &(String, api::ExternalPairDto)| a.0 == b.0 && a.1.alias == b.1.alias && a.1.session == b.1.session;
    let mut owners: Vec<String> = before.iter().filter(|b| !after.iter().any(|a| same(a, b))).map(|(_, p)| p.owner.clone()).collect();
    owners.dedup();
    owners
}

impl Hangar {
    /// A sessão aberta é a da outra pessoa num par: as rotas que não são de leitura da conversa levariam 403.
    pub(super) fn open_read_only(&self) -> bool { self.selected.as_ref().is_some_and(SessionInfo::read_only) }

    /// A linha `target` é a sessão de outra pessoa num par meu: de qual servidor próprio, com que apelido e dono.
    pub(super) fn outside_of(&self, target: &super::sidebar::Target) -> Option<api::dto::OutsideMark> {
        self.external_pairs.iter().find(|(_, p)| servers::norm(&p.address) == target.server && p.session == target.name)
            .map(|(own, p)| api::dto::OutsideMark { own: own.clone(), alias: p.alias.clone(), owner: p.owner.clone() })
    }

    /// A linha de verdade da sessão de fora, da lista da máquina dela, posta no grupo `gid` da lista própria `remote`.
    /// `None` enquanto a lista dela não chegou (ou servidor antigo, sem endereço no par).
    pub(super) fn outside_row(&self, gid: &str, pair: &api::dto::PairExternalEntry, remote: Option<&str>) -> Option<(SessionInfo, Option<String>)> {
        if pair.address.is_empty() { return None; }
        let key = servers::norm(&pair.address);
        let mut session = self.sessions_of(&key).iter().find(|s| s.name == pair.session)?.clone();
        session.pair_gid = Some(gid.to_owned());
        let own = remote.map(str::to_owned).unwrap_or_else(|| self.active_key());
        session.outside = Some(api::dto::OutsideMark { own, alias: pair.alias.clone(), owner: pair.owner.clone() });
        Some((session, (!self.is_active_key(&key)).then_some(key)))
    }

    /// Sessões de fora na máquina `key`: aparecem só dentro do grupo, então a seção da máquina as esconde.
    pub(super) fn outside_names_on(&self, key: &str) -> Option<&HashSet<String>> { self.outside_names.get(key) }

    /// Donos dos pares que a sessão `origin` segura: ela sair do grupo ou fechar desfaz o par para o grupo inteiro.
    pub(super) fn anchored_owners(&self, origin: &super::sidebar::Target) -> Vec<String> {
        self.external_pairs.iter().filter(|(own, p)| *own == origin.server && p.local_session == origin.name).map(|(_, p)| p.owner.clone()).collect()
    }

    /// Desfaz o par dos dois lados pelo servidor próprio que o tem. Sai da lista na hora: senão a releitura avisaria
    /// "saiu do pareamento" de um par que eu mesmo desfiz.
    pub(super) fn leave_outside(&mut self, mark: api::dto::OutsideMark, session: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&mark.own) else {
            window.push_notification(Notification::error(self.machine_error(&mark.own)), cx);
            return;
        };
        let (done, result) = tokio::sync::oneshot::channel();
        let alias = mark.alias.clone();
        let peer = session.clone();
        self.runtime.spawn(async move { let _ = done.send(api.leave_external_pair(&alias, &peer).await); });
        cx.spawn_in(window, async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.external_pairs.retain(|(own, p)| !(*own == mark.own && p.alias == mark.alias && p.session == session));
                    this.apply_external_pairs(cx);
                    this.refresh_external_pairs(window, cx);
                }
                Err(error) => window.push_notification(Notification::error(Self::fetch_failure(&error)), cx),
            });
        }).detach();
    }

    /// Confirma e desfaz: `outside` é o "Sair do pareamento" da linha de fora; senão, o "Desfazer par" da sessão própria.
    pub(super) fn confirm_leave_pair(&mut self, mark: api::dto::OutsideMark, session: String, outside: bool, window: &mut Window, cx: &mut Context<Self>) {
        let dono = [("dono", mark.owner.as_str())];
        let (title, body) = if outside { (tr_shared("pair_leave_outside", &dono), tr_shared("pair_leave_outside_body", &dono)) }
            else { (tr_shared("pair_undo_own", &dono), tr_shared("pair_undo_anchor_body", &dono)) };
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, title.clone(), body, title, ButtonVariant::Danger, move |window, cx| {
            let _ = this.update(cx, |this, cx| this.leave_outside(mark.clone(), session.clone(), window, cx));
            true
        });
    }

    /// Libera o acesso completo pendente do par `alias` na sessão `target`.
    pub(super) fn release_pair(&mut self, target: super::sidebar::Target, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&target.server) else {
            window.push_notification(Notification::error(self.machine_error(&target.server)), cx);
            return;
        };
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move { let _ = done.send(api.pair_access(&target.name, &alias, api::PairLevel::Full).await); });
        cx.spawn_in(window, async move |_, cx| {
            let result = result.await.unwrap_or_else(|_| Err(Failure::local("network_error")));
            // Liberado, a próxima lista já chega sem o pendente.
            if let Err(error) = result { let _ = cx.update(|window, cx| window.push_notification(Notification::error(Self::fetch_failure(&error)), cx)); }
        }).detach();
    }

    /// Lista própria mudou: relê os pares só quando muda quem tem par externo (e uma vez, na primeira lista).
    pub(super) fn external_pairs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut seen = Vec::new();
        for s in self.servers.iter().filter(|s| is_own(s)) {
            let key = servers::norm(&s.address);
            let list = if self.is_active_key(&key) { Some(&self.sessions) } else { self.remote.get(&key).map(|l| &l.sessions) };
            seen.extend(list.into_iter().flatten().map(|x| (key.clone(), x.name.clone(), x.outside_pairs().into_owned())).filter(|(_, _, p)| !p.is_empty()));
        }
        if self.external_seen.as_ref() == Some(&seen) { return; }
        self.external_seen = Some(seen);
        self.refresh_external_pairs(window, cx);
    }

    /// Lê os pares de cada servidor próprio. Servidor que não respondeu fica com os pares que já tinha.
    pub(super) fn refresh_external_pairs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let own: Vec<(String, Api)> = self.servers.iter().filter(|s| is_own(s))
            .filter_map(|s| { let key = servers::norm(&s.address); self.machine_api(&key).map(|api| (key, api)) }).collect();
        self.external_seq += 1;
        let seq = self.external_seq;
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let reads = own.into_iter().map(|(key, api)| async move { (key, api.external_pairs().await) });
            let _ = done.send(futures::future::join_all(reads).await);
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(results) = result.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if seq != this.external_seq { return; }
                let own: HashSet<String> = this.servers.iter().filter(|s| is_own(s)).map(|s| servers::norm(&s.address)).collect();
                let read: HashSet<String> = results.iter().filter(|(_, r)| r.is_ok()).map(|(key, _)| key.clone()).collect();
                let before = std::mem::take(&mut this.external_pairs);
                let mut pairs: Vec<(String, api::ExternalPairDto)> = before.iter().filter(|(key, _)| own.contains(key) && !read.contains(key))
                    .cloned().collect();
                for (key, result) in results {
                    match result {
                        Ok(list) => pairs.extend(list.into_iter().map(|p| (key.clone(), p))),
                        Err(error) => {
                            // Sem a marca, a próxima lista igual à de antes não releria: falha precisa poder tentar de novo.
                            this.external_seen = None;
                            eprintln!("pares externos de {key} não lidos: {}", error.detail);
                        }
                    }
                }
                // Servidor próprio removido leva os pares dele junto, e isso não é a outra pessoa saindo.
                let before: Vec<_> = before.into_iter().filter(|(key, _)| own.contains(key)).collect();
                for owner in left_owners(&before, &pairs) {
                    window.push_notification(Notification::info(tr_shared("pair_left_notice", &[("dono", &owner)])), cx);
                }
                this.external_pairs = pairs;
                this.apply_external_pairs(cx);
            });
        }).detach();
    }

    /// Acerta as entradas do par na lista de máquinas; as listas recomeçam só quando a entrada muda ou o attach entra.
    pub(super) fn apply_external_pairs(&mut self, cx: &mut Context<Self>) {
        let pairs: Vec<api::ExternalPairDto> = self.external_pairs.iter().map(|(_, p)| p.clone()).collect();
        self.outside_names.clear();
        for p in &pairs { self.outside_names.entry(servers::norm(&p.address)).or_default().insert(p.session.clone()); }
        let before = self.servers.clone();
        let pending = reconcile_pair_entries(&mut self.servers, &pairs);
        // Par que acabou tira a marca do attach dele: um par novo no mesmo token precisa ligar de novo.
        self.attached.retain(|(address, _, other)| pairs.iter().any(|p| servers::norm(&p.address) == servers::norm(address) && &p.token == other));
        if self.servers != before { self.restart_lists_for_pairs(cx); }
        let fresh: Vec<_> = pending.into_iter().filter(|attach| self.attached.insert(attach.clone())).collect();
        if fresh.is_empty() { return; }
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let calls = fresh.into_iter().map(|attach| async move {
                let result = api::attach_guest(&attach.0, &attach.1, &attach.2).await;
                (attach, result)
            });
            let _ = done.send(futures::future::join_all(calls).await);
        });
        cx.spawn(async move |this, cx| {
            let Ok(results) = result.await else { return };
            let _ = this.update(cx, |this, cx| {
                let mut any_ok = false;
                for (attach, result) in results {
                    match result {
                        Ok(()) => any_ok = true,
                        Err(error) => {
                            // Fora do conjunto e sem a marca de lista vista: o próximo aviso de lista relê os pares e tenta de novo.
                            this.attached.remove(&attach);
                            this.external_seen = None;
                            eprintln!("attach do par em {} falhou: {}", attach.0, error.detail);
                        }
                    }
                }
                if any_ok { this.restart_lists_for_pairs(cx); }
            });
        }).detach();
    }

    fn restart_lists_for_pairs(&mut self, cx: &mut Context<Self>) {
        self.servers_rev += 1;
        self.start_remote_lists();
        self.redraw(panes::Area::Nav, cx);
        cx.notify();
    }

    /// Clique na linha da outra pessoa: abre a sessão dela pela entrada da máquina dela.
    fn open_external_pair(&mut self, own: &str, alias: &str, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.machines_tray.open { self.close_popups(); cx.notify(); }
        let address = self.external_pairs.iter().find(|(key, p)| key == own && p.alias == alias && p.session == session)
            .map(|(_, p)| p.address.clone());
        let Some(key) = address.map(|a| servers::norm(&a)) else {
            window.push_notification(Notification::warning(tr("search_session_gone").replace("{name}", session)), cx);
            return;
        };
        // Entrada da máquina dela tirada da lista: volta a do par antes de abrir, senão o clique esperaria uma lista que não vem.
        if !self.remote.contains_key(&key) && !self.is_active_key(&key) { self.apply_external_pairs(cx); }
        if self.is_active_key(&key) { self.open_target(&super::sidebar::Target::new(&key, session), window, cx); }
        else if self.remote.contains_key(&key) { self.open_remote(&key, session.to_owned(), window, cx); }
        else {
            let text = tr("remote_open_failed").replace("{name}", session).replace("{erro}", &self.machine_error(&key));
            window.push_notification(Notification::warning(text), cx);
        }
    }

    /// Linha recuada sob o grupo, com a faixa dele: ícone de link, "dono · sessão" e a marca de externo.
    pub(super) fn render_external_pair_row(&self, gid: &str, owner: &str, session: &str, alias: &str, remote: Option<&str>, window: &mut Window,
        cx: &mut Context<Self>) -> AnyElement {
        let own = remote.map(str::to_owned).unwrap_or_else(|| self.active_key());
        let id = SharedString::from(format!("pair-ext-{own}-{gid}-{alias}-{session}"));
        let focus = window.use_keyed_state(SharedString::from(format!("{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        let label = format!("{owner} · {session}");
        let mark = tr("par_externo");
        let (click, key) = ((own.clone(), alias.to_owned(), session.to_owned()), (own, alias.to_owned(), session.to_owned()));
        let row_focus = focus.clone();
        div().id(id).relative().flex_shrink_0().ml(px(8.)).px(px(8.)).py(px(7.)).rounded(px(10.))
            .track_focus(&focus)
            .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .hover(|el| el.bg(theme::hover())).cursor_pointer()
            .flex().items_center().gap(px(8.))
            .child(div().absolute().left_0().top(px(6.)).bottom(px(6.)).w(px(2.)).rounded_full().bg(theme::accent()))
            .child(div().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(chrome::small_icon(IconName::Link, 14., theme::accent())))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(label.clone()))
            .child(badge(mark.clone(), theme::muted()))
            .role(Role::Button).aria_label(format!("{label} · {mark}"))
            .on_click(cx.listener(move |this, _, window, cx| this.open_external_pair(&click.0, &click.1, &click.2, window, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !matches!(event.keystroke.key.as_str(), "enter" | "space") || !row_focus.is_focused(window) { return; }
                this.open_external_pair(&key.0, &key.1, &key.2, window, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    const ADDR: &str = "https://a.ts.net:8443";

    fn pair(addr: &str, token: &str) -> api::ExternalPairDto {
        api::ExternalPairDto { local_session: "X".into(), alias: "pc-ana".into(), owner: "pc-ana".into(), session: "Y".into(),
            address: addr.into(), token: token.into() }
    }
    fn entry(addr: &str, token: &str, ephemeral: bool) -> servers::ServerEntry {
        servers::ServerEntry { address: addr.into(), token: token.into(), invite: true, ephemeral, ..Default::default() }
    }

    #[test]
    fn a_pair_gone_between_reads_names_its_owner() {
        let own = |p: api::ExternalPairDto| ("own".to_owned(), p);
        let ana = pair(ADDR, "t1");
        let bia = api::ExternalPairDto { alias: "pc-bia".into(), owner: "Bia".into(), session: "Z".into(), ..pair(ADDR, "t2") };
        assert_eq!(left_owners(&[own(ana.clone()), own(bia.clone())], &[own(bia.clone())]), ["pc-ana"]);
        assert!(left_owners(&[], &[own(ana.clone())]).is_empty(), "primeira leitura não avisa nada");
        assert!(left_owners(&[own(ana.clone())], &[own(ana)]).is_empty());
    }

    #[test]
    fn pair_without_entry_gets_an_ephemeral_one() {
        let mut list = vec![];
        assert!(reconcile_pair_entries(&mut list, &[pair(ADDR, "tp")]).is_empty());
        assert!(list[0].ephemeral && list[0].invite && list[0].token == "tp");
    }

    #[test]
    fn pair_on_an_existing_invite_attaches_to_its_token() {
        let mut list = vec![entry(ADDR, "ti", false)];
        let pend = reconcile_pair_entries(&mut list, &[pair(ADDR, "tp")]);
        assert_eq!(pend, vec![(ADDR.to_owned(), "ti".to_owned(), "tp".to_owned())]);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn ephemeral_entry_without_pair_is_dropped() {
        let mut list = vec![entry("https://b.ts.net:8443", "tp", true)];
        reconcile_pair_entries(&mut list, &[]);
        assert!(list.is_empty());
    }

    #[test]
    fn two_pairs_on_the_same_machine_share_one_entry() {
        let mut list = vec![];
        let pend = reconcile_pair_entries(&mut list, &[pair(ADDR, "t1"), pair(ADDR, "t2")]);
        assert_eq!((list.len(), list[0].token.as_str()), (1, "t1"));
        assert_eq!(pend, vec![(ADDR.to_owned(), "t1".to_owned(), "t2".to_owned())]);
    }

    #[test]
    fn ephemeral_entry_moves_to_a_live_pair_token_when_its_own_ends() {
        let mut list = vec![entry(ADDR, "t1", true)];
        let pend = reconcile_pair_entries(&mut list, &[pair(ADDR, "t2")]);
        assert!(pend.is_empty());
        assert_eq!(list[0].token, "t2");
    }

    #[test]
    fn own_server_at_the_pair_address_is_never_touched() {
        let mut list = vec![servers::ServerEntry { address: ADDR.into(), token: "own".into(), ..Default::default() }];
        assert!(reconcile_pair_entries(&mut list, &[pair("https://a.ts.net:8443/", "tp")]).is_empty());
        assert_eq!((list.len(), list[0].token.as_str()), (1, "own"));
    }

    #[test]
    fn ephemeral_entry_is_never_persisted() {
        let list = vec![entry(ADDR, "tp", true), entry("https://b.ts.net:8443", "ti", false)];
        let saved = servers::persistable(&list);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].token, "ti");
    }

    #[test]
    fn upsert_over_an_ephemeral_entry_makes_it_persistent_with_the_new_token() {
        let mut list = vec![entry(ADDR, "tp", true)];
        servers::upsert(&mut list, servers::ServerEntry { address: ADDR.into(), token: "own".into(), ..Default::default() });
        assert_eq!((list.len(), list[0].token.as_str(), list[0].ephemeral, list[0].invite), (1, "own", false, false));
        assert_eq!(servers::persistable(&list).len(), 1);
    }
}
