//! Aceite de convite de par: a sessão que vai parear vem do menu dela ou é escolhida aqui.
use super::*;
use super::machines::enter_to_focused;
use crate::api::{PairAccepted, PairLevel, ShareFailure, SharePrereqs};
use super::share::prerequisite_notice;

/// Só `<máquina>--guest.<conta>.hangar.dev.br`, a regra do `is_guest_host` do `hangar-connect`: `<máquina>.<conta>` é a porta do dono.
fn connect_guest_host(host: &str) -> bool {
    let label = |l: &str| (1..=63).contains(&l.len()) && !l.starts_with('-') && !l.ends_with('-')
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let Some((machine, account)) = host.strip_suffix(".hangar.dev.br").and_then(|rest| rest.split_once('.')) else { return false };
    label(machine) && label(account) && machine.strip_suffix("--guest").is_some_and(label)
}

/// Só `https://host.ts.net:8443/par/CÓDIGO` do Funnel ou `https://<máquina>--guest.<conta>.hangar.dev.br/par/CÓDIGO` do Connect,
/// sem porta: outro host ou porta nunca recebe o resgate.
pub(super) fn parse_pair_link(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let funnel = host.ends_with(".ts.net") && host.len() > ".ts.net".len() && url.port() == Some(8443);
    // `Url` apaga a `:443` e aceita `\` no lugar de `/`: só o texto mostra que o link veio com porta.
    let authority = raw.trim().get("https:".len()..)?.trim_start_matches(['/', '\\']).split(['/', '\\']).next()?;
    let connect = connect_guest_host(host) && url.port().is_none() && !authority.contains(':');
    if !funnel && !connect { return None; }
    let mut parts: Vec<&str> = url.path_segments()?.collect();
    if parts.last() == Some(&"") { parts.pop(); }
    let &["par", code] = parts.as_slice() else { return None };
    let code_ok = (1..=64).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric());
    code_ok.then(|| if funnel { format!("https://{host}:8443/par/{code}") } else { format!("https://{host}/par/{code}") })
}

/// Código do convite (usado, vencido, revogado…) tem frase própria; `Hangar::failure` leria todo 410 como "encerrado".
pub(super) fn unreachable_text(stage: &str) -> String {
    tr_shared(if stage == "dns" { "share_reach_failed_dns" } else { "share_reach_failed_funnel" }, &[])
}

/// Antes do aceite só se conhece a máquina do link: ela faz as vezes do dono na pergunta.
pub(super) fn accept_level_title(raw: &str) -> String {
    match parse_pair_link(raw).and_then(|link| url::Url::parse(&link).ok()?.host_str()?.split('.').next().map(|machine| machine.trim_end_matches("--guest").to_owned())) {
        Some(machine) => tr_shared("pair_accept_level_title", &[("dono", &machine)]),
        None => tr_shared("pair_level_title", &[]),
    }
}

/// Aviso do aceite: o nível que veio do outro lado.
pub(super) fn received_notice(level: PairLevel, owner: &str) -> String {
    tr_shared(if level == PairLevel::Full { "pair_level_received_full" } else { "pair_level_received_view" }, &[("dono", owner)])
}

fn accept_failure(error: &Failure) -> String {
    if error.detail.starts_with("erro_convite_") { tr_shared(&error.detail, &[]) } else { Hangar::failure(error) }
}

pub(super) struct PairAcceptDialog {
    hangar: WeakEntity<Hangar>,
    // Fixada na abertura: a máquina da sessão do menu, ou a ativa quando o link veio colado.
    api: Api,
    input: Entity<InputState>,
    fixed: Option<String>,
    choice: Option<String>,
    sessions: Vec<String>,
    busy: bool,
    error: Option<String>,
    // O túnel que o aceite sobe ainda não está pronto: o aviso é o mesmo do compartilhar.
    blocked: Option<SharePrereqs>,
    /// O que quem convidou terá aqui; começa no mais fechado.
    level: PairLevel,
    /// Cada sessão possível com o grupo local dela: o acesso completo abre todas.
    groups: Vec<(String, Vec<String>)>,
}

impl PairAcceptDialog {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy { return; }
        self.blocked = None;
        let Some(link) = parse_pair_link(&self.input.read(cx).value()) else {
            self.error = Some(tr_shared("erro_par_link_invalido", &[]));
            cx.notify();
            return;
        };
        let Some(session) = self.fixed.clone().or_else(|| self.choice.clone()) else {
            self.error = Some(tr("par_escolha_sessao"));
            cx.notify();
            return;
        };
        (self.busy, self.error, self.blocked) = (true, None, None);
        let (me, api, level) = (cx.entity().downgrade(), self.api.clone(), self.level);
        let _ = self.hangar.update(cx, |this, cx| this.accept_pair(me, api, session, link, level, window, cx));
        cx.notify();
    }
}

impl Render for PairAcceptDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col().gap_3()
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("par_aceitar_ajuda")))
            .child(Input::new(&self.input).aria_label(tr("par_campo_aria")));
        if self.fixed.is_none() {
            col = col.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("par_qual_sessao")));
            if self.sessions.is_empty() {
                col = col.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("par_sem_sessao")));
            }
            for (i, name) in self.sessions.clone().into_iter().enumerate() {
                let picked = self.choice.as_deref() == Some(name.as_str());
                let label = name.clone();
                col = col.child(Button::new(("par-sessao", i)).small().when(picked, |b| b.primary()).label(label)
                    .on_click(cx.listener(move |this, _, _, cx| { this.choice = Some(name.clone()); cx.notify(); })));
            }
        }
        let session = self.fixed.clone().or_else(|| self.choice.clone());
        let group = self.groups.iter().find(|(name, _)| Some(name) == session.as_ref()).map(|(_, g)| g.clone()).unwrap_or_default();
        let title = accept_level_title(&self.input.read(cx).value());
        col = col.child(super::share::level_choice("par-nivel", title, &group, Some(self.level), self.busy,
            |d: &mut PairAcceptDialog, level, cx| { d.level = level; cx.notify(); }, cx));
        col.when(self.busy, |el| el.child(div().id("par-busy").role(Role::Status).text_sm().text_color(theme::muted()).child(tr("par_aceitando"))))
            .when_some(self.blocked.as_ref(), |el, prereqs| el.child(prerequisite_notice("par-blocked", prereqs, None, None)))
            .when_some(self.error.clone(), |el, error| el.child(div().id("par-error").role(Role::Alert).text_sm()
                .text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().justify_end().child(Button::new("par-aceitar").primary().label(tr("par_aceitar")).disabled(self.busy)
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))))
    }
}

impl Hangar {
    /// `target` fixa a sessão e a máquina dela (menu da linha); sem ele, o link veio colado e a sessão é escolhida na máquina ativa.
    pub(super) fn open_pair_accept_dialog(&mut self, target: Option<super::sidebar::Target>, link: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let api = match &target {
            Some(t) => self.machine_api(&t.server),
            None => self.api.clone(),
        };
        let Some(api) = api else {
            let error = target.as_ref().map_or_else(|| tr("connection_failed"), |t| self.machine_error(&t.server));
            window.push_notification(Notification::error(error), cx);
            return;
        };
        let hangar = cx.entity().downgrade();
        let sessions: Vec<String> = self.sessions.iter().filter(|s| s.state != "dead" && !s.orq()).map(|s| s.name.clone()).collect();
        let groups = match &target {
            Some(t) => vec![(t.name.clone(), self.local_group(t))],
            None => { let active = self.active_key(); sessions.iter().map(|n| (n.clone(), self.local_group(&super::sidebar::Target::new(&active, n)))).collect() }
        };
        let fixed = target.map(|t| t.name);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("par_placeholder")).default_value(link.unwrap_or_default()));
        // O título do nível segue o link digitado.
        let dialog = cx.new(|cx| {
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            PairAcceptDialog { hangar, api, input: input.clone(), fixed: fixed.clone(), choice: None, sessions, busy: false, error: None,
                blocked: None, level: PairLevel::View, groups }
        });
        let title = match &fixed { Some(s) => tr_shared("native_par_titulo_sessao", &[("sessao", s)]), None => tr("par_titulo") };
        window.open_dialog(cx, move |d, _, cx| {
            let busy = dialog.read(cx).busy;
            let confirm = dialog.clone();
            popup::dialog(d).w(px(520.)).title(title.clone()).child(dialog.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(move |event, window, cx| {
                    // Enter no campo aceita; em outro controle segue para ele, como no resto do app.
                    if confirm.read(cx).input.read(cx).focus_handle(cx).is_focused(window) { confirm.update(cx, |d, cx| d.submit(window, cx)); false }
                    else { enter_to_focused(event, window, cx) }
                })
        });
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    #[allow(clippy::too_many_arguments)]
    fn accept_pair(&mut self, dialog: WeakEntity<PairAcceptDialog>, api: Api, session: String, link: String, level: PairLevel, window: &mut Window,
        cx: &mut Context<Self>) {
        let (done, result) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move { let _ = done.send(api.accept_pair(&session, &link, level).await); });
        cx.spawn_in(window, async move |this, cx| {
            let result: Result<PairAccepted, ShareFailure> = result.await.unwrap_or_else(|_| Err(ShareFailure::Other(Failure::local("network_error"))));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(ok) => {
                    window.close_all_dialogs(cx);
                    window.push_notification(Notification::success(tr_shared("native_par_aceito", &[("dono", &ok.owner), ("sessao", &ok.session)])), cx);
                    window.push_notification(Notification::info(received_notice(ok.level, &ok.owner)), cx);
                    this.refresh_external_pairs(window, cx);
                }
                Err(failure) => { let _ = dialog.update(cx, |d, cx| {
                    d.busy = false;
                    match &failure {
                        ShareFailure::Blocked(prereqs) => (d.error, d.blocked) = (None, Some(prereqs.clone())),
                        ShareFailure::Other(e) => (d.error, d.blocked) = (Some(accept_failure(e)), None),
                        ShareFailure::NeedsRust => (d.error, d.blocked) = (Some(tr_shared("share_create_needs_rust", &[])), None),
                        ShareFailure::Unreachable { stage, .. } => (d.error, d.blocked) = (Some(unreachable_text(stage)), None),
                    }
                    cx.notify();
                }); }
            });
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn level_question_names_the_machine_and_the_notice_the_level() {
        let named = accept_level_title("https://pc-ana.tail.ts.net:8443/par/ABC12");
        assert!(named.contains("pc-ana") && !named.contains("tail"), "{named}");
        assert_eq!(accept_level_title("lixo"), tr_shared("pair_level_title", &[]), "sem link, a pergunta genérica");
        let (full, view) = (received_notice(PairLevel::Full, "Ana"), received_notice(PairLevel::View, "Ana"));
        assert!(full.contains("Ana") && view.contains("Ana") && full != view, "{full} / {view}");
    }

    #[test]
    fn pair_link_accepts_funnel_par_links_only() {
        assert_eq!(parse_pair_link(" https://a.tail.ts.net:8443/par/ABC12 ").as_deref(), Some("https://a.tail.ts.net:8443/par/ABC12"));
        assert_eq!(parse_pair_link("https://a.tail.ts.net:8443/par/ABC12/").as_deref(), Some("https://a.tail.ts.net:8443/par/ABC12"));
        for raw in ["https://a.tail.ts.net:8443/convite/ABC12", "http://a.tail.ts.net:8443/par/ABC12", "https://evil.com:8443/par/ABC12",
            "https://a.tail.ts.net/par/ABC12", "https://a.tail.ts.net:9000/par/ABC12", "https://a.tail.ts.net:8443/par/",
            "https://a.tail.ts.net:8443/par/AB-C", "https://a.tail.ts.net:8443/par/ABC/x", "https://u:p@a.tail.ts.net:8443/par/ABC",
            "https://a.tail.ts.net:8443/par/ABC?x=1", "https://.ts.net:8443/par/ABC", "https://a.ts.net.evil.com:8443/par/ABC", "hangar://par/a.tail.ts.net:8443/ABC12"] {
            assert_eq!(parse_pair_link(raw), None, "{raw}");
        }
        assert_eq!(parse_pair_link(&format!("https://a.tail.ts.net:8443/par/{}", "A".repeat(65))), None);
    }

    #[test]
    fn pair_link_accepts_the_connect_guest_host_only() {
        assert_eq!(parse_pair_link(" https://pc1--guest.acme.hangar.dev.br/par/ABC12/ ").as_deref(), Some("https://pc1--guest.acme.hangar.dev.br/par/ABC12"));
        for raw in ["https://pc1.acme.hangar.dev.br/par/ABC12", "https://pc1--preview.acme.hangar.dev.br/par/ABC12",
            "https://--guest.acme.hangar.dev.br/par/ABC12", "https://pc1--guest.hangar.dev.br/par/ABC12",
            "https://pc1--guest.acme.hangar.dev.br:443/par/ABC12", "https://pc1--guest.acme.hangar.dev.br:8443/par/ABC12",
            "https://pc1--guest.acme.hangar.dev.br.evil.com/par/ABC12", "http://pc1--guest.acme.hangar.dev.br/par/ABC12",
            r"https:\\pc1--guest.acme.hangar.dev.br:9000/par/ABC12", r"https:\\pc1--guest.acme.hangar.dev.br:443/par/ABC12"] {
            assert_eq!(parse_pair_link(raw), None, "{raw}");
        }
    }
}
