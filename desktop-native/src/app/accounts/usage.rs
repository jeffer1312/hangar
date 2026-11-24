//! Cartões dos anéis do compositor. O de contexto mostra a ocupação e o uso da sessão. O de uso (o `account_usage` do
//! Zeron) mostra as contas Claude e Codex, a que a sessão usa
//! primeiro, com plano e as janelas de sessão e semana. Lê a mesma lista da página Contas e só quando abre: o servidor
//! devolve a cota guardada, sem releitura periódica daqui.
use super::*;

fn account_in_use<'a>(list: &'a [Credential], session: Option<&SessionInfo>) -> Option<&'a Credential> {
    let provider = session.map(|s| s.provider.as_str()).filter(|p| !p.is_empty()).unwrap_or("claude");
    let account = session.and_then(|s| s.conta.as_deref());
    if let Some(binding) = session.and_then(|s| s.engine_binding.as_deref()) {
        let id = format!("cliproxy-binding:{binding}");
        return list.iter().find(|c| c.id == id);
    }
    if session.is_some_and(SessionInfo::uses_engine_account) {
        return list.iter().find(|c| matches!(c.kind.as_str(), "codex" | "cliproxy") && account == Some(c.id.as_str()));
    }
    if provider == "claude" && session.is_some_and(|s| s.engine.as_deref().is_some_and(|e| !e.is_empty())) { return None; }
    list.iter().find(|c| c.kind == provider && account.map_or(c.active, |id| id == c.id))
}

fn proxy_windows(credential: &Credential) -> Option<&[QuotaWindow]> {
    let quota = if credential.kind == "cliproxy" { credential.quota.as_ref() } else { credential.cliproxy_quota.as_ref() }?;
    (quota.state == "lida" && !quota.windows.is_empty()).then_some(quota.windows.as_slice())
}

impl Hangar {
    /// Abre ou fecha o cartão, fechando o painel que estiver aberto sobre o compositor.
    pub(in crate::app) fn toggle_usage_card(&mut self, cx: &mut Context<Self>) {
        let open = !self.accounts.card || self.accounts.card_top;
        self.close_popups();
        (self.accounts.card, self.accounts.card_top) = (open, false);
        if open { self.accounts.card_since = Some(Instant::now()); }
        if open { self.load_session_accounts(cx); }
        cx.notify();
    }

    /// O anel de contexto abre o cartão dele pela mesma regra do de contas.
    pub(in crate::app) fn toggle_context_card(&mut self, cx: &mut Context<Self>) {
        let open = !self.context_card;
        self.close_popups();
        self.context_card = open;
        cx.notify();
    }

    /// Cartão do anel de contexto, no padrão do de contas: o número grande com a barra, como o bloco do painel, e o uso da
    /// sessão numa grade rotulada de duas colunas.
    pub(in crate::app) fn render_context_card(&self) -> AnyElement {
        let status = self.status();
        let pct = status.as_ref().and_then(|s| s.ctx_pct);
        let window = match (status.as_ref().and_then(|s| s.ctx_used), status.as_ref().and_then(|s| s.ctx_total)) {
            (Some(used), Some(total)) => Some(tr("side_ctx_of").replace("{used}", &side::tokens(used)).replace("{total}", &side::tokens(total))),
            (None, Some(total)) => Some(side::tokens(total)),
            _ => None,
        };
        let number = div().px(px(8.)).flex().items_baseline().gap(px(8.))
            .child(div().flex_shrink_0().text_size(px(28.)).line_height(px(32.)).font_weight(FontWeight::SEMIBOLD)
                .text_color(match pct { Some(p) if p >= 70. => chrome::ring_text(pct), Some(_) => theme::text(), None => theme::faint() })
                .child(pct.map(|p| format!("{}%", p.round())).unwrap_or_else(|| "—".into())))
            .child(div().min_w_0().truncate().text_xs().text_color(theme::faint())
                .child(window.map_or_else(|| tr("side_ctx_label"), |w| format!("{} · {w}", tr("side_ctx_label")))));
        let gauge = match pct {
            Some(p) => div().px(px(8.)).pt(px(8.)).pb(px(6.)).child(chrome::meter(p)),
            None => div().px(px(8.)).pt(px(4.)).pb(px(6.)).text_xs().text_color(theme::muted()).whitespace_normal().child(tr("side_ctx_unknown")),
        };
        let cells = self.stats.as_ref().map(side::stats_cells).unwrap_or_default();
        let grid = (!cells.is_empty()).then(|| div().px(px(8.)).pb(px(4.)).flex().flex_wrap().gap_y(px(6.))
            .children(cells.into_iter().map(|(label, value)| div().w(relative(0.5)).pr(px(12.)).flex().items_baseline().justify_between().gap(px(8.))
                .text_size(px(12.))
                .child(div().min_w_0().truncate().text_color(theme::muted()).child(label))
                .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(value)))));
        div().p(px(popup::INSET)).rounded_md().bg(theme::popup_content_fill()).flex().flex_col().gap(px(2.))
            .child(popup::title(tr("ring_context"), None))
            .child(number)
            .child(gauge)
            .when_some(grid, |el, grid| el.child(popup::separator()).child(popup::title(tr("ctx_card_usage"), None)).child(grid))
            .into_any_element()
    }

    /// A pílula da barra do topo abre o mesmo cartão, só que preso a ela.
    pub(in crate::app) fn toggle_top_usage_card(&mut self, cx: &mut Context<Self>) {
        let open = !(self.accounts.card && self.accounts.card_top);
        self.close_popups();
        (self.accounts.card, self.accounts.card_top) = (open, open);
        if open { self.accounts.card_since = Some(Instant::now()); }
        if open { self.load_session_accounts(cx); }
        cx.notify();
    }

    /// Relê a lista de contas (com a cota guardada do servidor), para a pílula da barra do topo.
    pub(in crate::app) fn refresh_default_account(&mut self, cx: &mut Context<Self>) {
        self.load_session_accounts(cx);
    }

    /// A conta da sessão em foco para a pílula da barra do topo: provider, nome e a janela mais cheia, com o rótulo dela
    /// (o `piorJanela` do web). Sem sessão, o Claude; conta ausente (servidor sem o campo) é a padrão do provider.
    /// `None` antes da lista chegar ou sem conta daquele provider.
    pub(in crate::app) fn focused_account(&self) -> Option<(String, String, Option<(String, f64)>)> {
        let c = account_in_use(self.session_accounts().ok()?, self.selected.as_ref())?;
        let login = c.login.as_ref().filter(|l| l.logged_in == Some(true));
        let title = c.alias.clone().filter(|a| !a.is_empty()).or_else(|| login.and_then(|l| l.email.clone())).unwrap_or_else(|| c.name.clone());
        let model = self.status().and_then(|s| s.model).map(|m| m.to_lowercase());
        let window = if self.has_proxy_session() {
            proxy_windows(c).and_then(|windows| windows.iter().filter(|w| !w.por_modelo).max_by(|a, b| a.pct.total_cmp(&b.pct)))
                .map(|w| (w.label.clone(), w.pct))
        } else {
            match build_row(c, &HashMap::new(), false, now()).quota {
                QuotaView::Bars { bars, .. } if c.kind == "antigravity" => {
                    let bars = agy_group(bars, model.as_deref());
                    fullest(&bars, None).map(|b| (b.label.clone(), b.pct))
                }
                QuotaView::Bars { bars, .. } => fullest(&bars, model.as_deref()).map(|b| (b.label.clone(), b.pct)),
                _ => None,
            }
        };
        Some((c.kind.clone(), title, window))
    }

    pub(in crate::app) fn has_proxy_session(&self) -> bool {
        self.selected.as_ref().is_some_and(SessionInfo::uses_engine_account)
            || self.session_engine().is_some_and(|e| e.cliproxy_accounts.is_some())
    }

    pub(in crate::app) fn focused_proxy_pct(&self) -> Option<f64> {
        let session = self.selected.as_ref().filter(|s| s.uses_engine_account())?;
        let c = account_in_use(self.session_accounts().ok()?, Some(session))?;
        let windows = proxy_windows(c)?;
        windows.iter().find(|w| w.label == "5h").or_else(|| windows.iter().find(|w| w.label == "7d")).map(|w| w.pct)
    }

    pub(in crate::app) fn focused_proxy_quota(&self) -> String {
        let Some(c) = self.selected.as_ref().filter(|s| s.uses_engine_account())
            .and_then(|s| account_in_use(self.session_accounts().ok()?, Some(s))) else { return String::new() };
        proxy_windows(c).into_iter().flatten().filter(|w| matches!(w.label.as_str(), "5h" | "7d"))
            .map(|w| format!("{} {}%", w.label, w.pct.round())).collect::<Vec<_>>().join(" · ")
    }

    pub(in crate::app) fn render_usage_card(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let session = self.selected.as_ref();
        let current = self.session_accounts().ok().and_then(|list| account_in_use(list, session)).map(|c| c.id.as_str());
        let in_use = |c: &Credential| current == Some(c.id.as_str());
        let proxy = self.has_proxy_session();
        let engine_session = session.is_some_and(|s| s.engine.as_deref().is_some_and(|e| !e.is_empty()) || s.uses_engine_account());
        let note = |text: String, color: Hsla| div().px(px(8.)).py(px(4.)).text_sm().text_color(color).whitespace_normal().child(text).into_any_element();
        let list = self.session_accounts();
        let fill = if cx.reduce_motion() { 1. } else { self.accounts.card_since.map_or(1., |at| motion::BAR_FILL.ease(motion::BAR_FILL.raw(at))) };
        // Aberto pelo anel de uma sessão Claude na conta Anthropic: as outras contas Claude levam esta conversa para elas.
        let movable = (!self.accounts.card_top || proxy) && session.is_some_and(|s| s.provider == "claude"
            && (proxy || (s.engine.as_deref().is_none_or(|e| e.is_empty()) && s.conta.as_deref().is_some_and(|c| c.starts_with("claude:"))))
            && !s.read_only() && !self.selected_target().is_some_and(|t| self.sidebar.moving.contains_key(&t)));
        let idle = session.is_some_and(|s| s.state == "idle");
        let transfer = session.and_then(super::sidebar::transfer_source).zip(self.selected_target())
            .filter(|(_, target)| !self.accounts.card_top && !self.sidebar.moving.contains_key(target))
            .map(|((life, jsonl), target)| (target, life.to_owned(), jsonl.to_owned()));
        let body = match (&list.value, list.ok()) {
            (_, Some(list)) => {
                let mut mine: Vec<&Credential> = list.iter().filter(|c| c.kind == "claude" || (c.kind == "codex" && !proxy)
                    || (proxy && matches!(c.kind.as_str(), "codex" | "cliproxy") && (in_use(c)
                        || self.session_engine().and_then(|e| e.cliproxy_accounts.as_ref()).is_some_and(|accounts| accounts.iter().any(|a| a.credential_id == c.id))))).collect();
                if session.is_some_and(|s| s.engine_binding.is_some()) {
                    mine.retain(|c| session.and_then(|s| s.conta.as_deref()) != Some(c.id.as_str()));
                }
                mine.sort_by_key(|c| !in_use(c));
                if mine.is_empty() { note(tr("usage_card_empty"), theme::muted()) } else {
                    let (engines, now) = (HashMap::new(), now());
                    let rows = mine.into_iter().map(|c| {
                        let proxy_credential;
                        let c = if proxy && c.kind == "codex" {
                            proxy_credential = Credential { quota: c.cliproxy_quota.clone(), ..c.clone() };
                            &proxy_credential
                        } else { c };
                        let quota = build_row(c, &engines, false, now).quota;
                        // A janela mais cheia, como a lista do backend: a partir de 95% pede confirmação, a partir de 99% não aceita.
                        let pct = match &quota { QuotaView::Bars { bars, .. } => bars.iter().map(|b| b.pct).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))), _ => None };
                        let target = c.id.strip_prefix("claude:").map(str::to_owned)
                            .filter(|_| movable && idle && c.kind == "claude" && !in_use(c) && pct.is_none_or(|p| p < 99.));
                        let chatgpt = proxy && matches!(c.kind.as_str(), "codex" | "cliproxy");
                        let row = account_row(c, in_use(c), chatgpt, quota, fill);
                        if chatgpt && movable && !in_use(c) {
                            let account = self.session_engine().filter(|e| e.cliproxy_error.is_none()).and_then(|e|
                                e.cliproxy_accounts.as_ref()?.iter().find(|a| a.credential_id == c.id
                                    && (e.cliproxy_binding.is_none() || e.cliproxy_bound_account.as_ref().is_some_and(|bound| bound.account == a.account))))
                                .map(|a| a.account.clone());
                            let available = idle && account.is_some() && pct.is_none_or(|p| p < 99.);
                            let label = c.alias.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| c.name.clone());
                            let aria = tr("sidebar_proxy_title").replace("{n}", &label);
                            let warn = pct.filter(|p| *p >= 95.);
                            return Button::new(SharedString::from(format!("usage-proxy-{}", c.id))).ghost().w_full().h_auto()
                                .disabled(!available).accessibility_label(aria).child(row)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let (Some(owner), Some(account)) = (this.selected_target(), account.as_ref()) {
                                        this.close_popups();
                                        this.confirm_engine_account(owner, account.clone(), label.clone(), warn, window, cx);
                                    }
                                })).into_any_element();
                        }
                        if c.kind == "codex" && movable && !proxy {
                            let account = c.codex_account.clone();
                            let connected = c.login.as_ref().is_some_and(|l| l.logged_in == Some(true));
                            let source = transfer.clone();
                            let available = source.is_some() && account.is_some() && connected && c.id.starts_with("codex:");
                            let title = tr("session_transfer_account_aria").replace("{name}", &session.map(|s| s.name.clone()).unwrap_or_default())
                                .replace("{account}", c.alias.as_deref().filter(|a| !a.is_empty()).unwrap_or(&c.name));
                            return Button::new(SharedString::from(format!("usage-transfer-{}", c.id))).ghost().w_full().h_auto()
                                .disabled(!available).accessibility_label(title).child(row)
                                .when(!connected, |b| b.tooltip(tr("create_codex_disconnected")))
                                .when(!available && connected, |b| b.tooltip(tr("session_transfer_unavailable")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let (Some((target, life, jsonl)), Some(account)) = (&source, &account) else { return };
                                    this.close_popups();
                                    this.open_transfer(target.clone(), life.clone(), jsonl.clone(), account.clone(), window, cx);
                                })).into_any_element();
                        }
                        match target {
                            Some(path) => {
                                let label = c.alias.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| c.name.clone());
                                let warn = pct.filter(|p| *p >= 95.);
                                let aria = tr(if proxy { "sidebar_return_claude_account" } else { "sidebar_same_title" }).replace("{n}", &label);
                                Button::new(SharedString::from(format!("usage-move-{path}"))).outline().w_full().h_auto()
                                    .accessibility_label(aria).child(row)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_popups();
                                        if let Some(owner) = this.selected_target() {
                                            this.confirm_account(owner, path.clone(), label.clone(), warn, window, cx);
                                        }
                                    })).into_any_element()
                            }
                            None => row.when(movable && c.kind == "claude" && !in_use(c) && pct.is_some_and(|p| p >= 99.), |el| el.opacity(0.55))
                                .into_any_element(),
                        }
                    });
                    div().flex().flex_col().gap(px(2.))
                        .when(proxy, |el| el.child(note(tr("usage_card_proxy_hint"), theme::faint())))
                        .when_some(self.session_engine().and_then(|e| e.cliproxy_error.as_ref()), |el, error|
                            el.child(note(tr("create_proxy_error").replace("{reason}", error), theme::warning())))
                        .when(engine_session && self.accounts.session_engines.loading, |el| el.child(note(tr("loading"), theme::muted())))
                        .when_some(self.accounts.session_engines.value.as_ref().and_then(|e| e.as_ref().err()).filter(|_| engine_session), |el, error|
                            el.child(note(tr("create_proxy_error").replace("{reason}", error), theme::warning())))
                        .when(movable, |el| el.child(note(tr(if transfer.is_some() { "session_transfer_accounts_hint" }
                            else if !idle { "usage_card_move_busy" } else if proxy { "usage_card_proxy_move_hint" } else { "usage_card_move_hint" }), theme::faint())))
                        .children(rows)
                        .into_any_element()
                }
            }
            (Some(Err(error)), _) => note(tr("accounts_failed").replace("{reason}", error), theme::warning()),
            _ => popup::skeleton("usage-card-loading", 1).into_any_element(),
        };
        div().p(px(popup::INSET)).rounded_md().bg(theme::popup_content_fill()).flex().flex_col().gap(px(2.))
            .child(popup::title(tr(if proxy { "usage_card_proxy_title" } else { "usage_card_title" }), None))
            .child(div().id("usage-card-scroll").max_h((window.viewport_size().height - px(180.)).max(px(120.))).overflow_y_scroll().child(body))
            // Rodapé do web: atalho para a tela de contas. As configurações são do servidor ativo, então com sessão de
            // outra máquina o atalho levaria às contas erradas e some.
            .when(self.open_api.is_none(), |el| el.child(div().px(px(4.)).pt(px(4.)).border_t_1().border_color(theme::border()).flex()
                .child(Button::new("usage-card-accounts").ghost().small().label(crate::app::activity::web("contas_titulo"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_popups();
                        this.open_settings(super::settings::Page::Accounts, window, cx);
                    })))))
            .into_any_element()
    }
}

fn account_row(c: &Credential, in_use: bool, chatgpt: bool, quota: QuotaView, fill: f32) -> Div {
    let login = c.login.as_ref().filter(|l| l.logged_in == Some(true));
    let email = login.and_then(|l| l.email.clone()).filter(|e| !e.is_empty());
    let title = c.alias.clone().filter(|a| !a.is_empty()).or_else(|| login.and_then(|l| l.email.clone())).unwrap_or_else(|| c.name.clone());
    // O servidor manda o plano cru ("max", "pro").
    let plan = login.and_then(|l| l.plan.as_deref()).and_then(|p| {
        let mut chars = p.chars();
        chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
    });
    let meta = div().flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(theme::muted())
        .child(if chatgpt { "ChatGPT".to_owned() } else { side::agent_label(&c.kind) })
        .when(plan.is_some(), |el| el.child(div().text_color(theme::faint()).child("·")))
        .when_some(plan.clone(), |el, plan| el.child(plan))
        .when(in_use, |el| el.child(div().text_color(theme::faint()).child("·")))
        .when(in_use, |el| el.child(div().text_color(theme::accent()).child(tr("usage_card_in_use"))));
    let meters = match quota {
        QuotaView::Bars { bars, stale, cause } => div().flex().flex_col().gap(px(4.)).when(stale.is_some(), |el| el.opacity(0.6))
            .children(bars.iter().take(2).map(|bar| meter(bar, fill)))
            .children(stale.map(|text| div().text_size(px(11.)).text_color(theme::faint()).child(text)))
            .children(cause.map(|text| div().text_size(px(11.)).text_color(theme::warning()).whitespace_normal().child(text))),
        QuotaView::Note(text) => div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(text),
        QuotaView::Nothing => div(),
    };
    div().flex().items_center().gap(px(12.)).px(px(8.)).py(px(7.)).rounded(px(7.)).when(in_use, |el| el.bg(theme::accent_dim()))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(title.clone()))
            // Apelido não diz qual login é: o email vem embaixo, a menos que já seja o próprio título.
            .when_some(email.filter(|e| *e != title), |el, email| el.child(div().truncate().text_size(px(11.5)).text_color(theme::faint()).child(email)))
            .child(meta))
        .child(div().w(px(236.)).flex_shrink_0().child(meters))
}

#[cfg(test)]
mod tests {
    use super::super::Credential;
    use crate::api::dto::SessionInfo;
    use serde_json::json;

    #[test]
    fn claude_proxy_usage_resolves_only_the_pinned_codex_credential() {
        let list: Vec<Credential> = serde_json::from_value(json!([
            {"id":"claude:/storage", "tipo":"claude", "nome":"Storage", "ativa":true},
            {"id":"codex:/first", "tipo":"codex", "nome":"First", "ativa":true},
            {"id":"codex:/second", "tipo":"codex", "nome":"Second", "ativa":false}
        ])).unwrap();
        let mut session = SessionInfo { provider:"claude".into(), engine:Some("proxy".into()),
            engine_account:Some("second".into()), conta:Some("codex:/second".into()), ..Default::default() };
        assert_eq!(super::account_in_use(&list, Some(&session)).unwrap().id, "codex:/second");
        session.conta = Some("codex:/missing".into());
        assert!(super::account_in_use(&list, Some(&session)).is_none());
        session.conta = Some("claude:/storage".into());
        assert!(super::account_in_use(&list, Some(&session)).is_none());
        session.conta = None;
        assert!(super::account_in_use(&list, Some(&session)).is_none());
        session.engine_account = None;
        session.conta = Some("claude:/storage".into());
        assert!(super::account_in_use(&list, Some(&session)).is_none());
        session.conta = None;
        session.engine = None;
        assert_eq!(super::account_in_use(&list, Some(&session)).unwrap().id, "claude:/storage");
    }

    #[test]
    fn agy_sessions_see_only_the_quota_group_of_their_model() {
        let bar = |label: &str, pct: f64, per_model: bool| super::super::Bar { label: label.into(), reset: String::new(), reset_days: String::new(),
            pct, reset_at: None, per_model };
        let bars = || vec![bar("7d", 3., false), bar("5h", 4., false), bar("Claude/GPT 7d", 50., true), bar("Claude/GPT 5h", 60., true)];
        let gemini = super::agy_group(bars(), Some("gemini-4-neon-high"));
        assert_eq!(super::fullest(&gemini, None).map(|b| (b.label.as_str(), b.pct)), Some(("5h", 4.)));
        let claude = super::agy_group(bars(), Some("claude-opus-5-5"));
        assert_eq!(super::fullest(&claude, None).map(|b| (b.label.as_str(), b.pct)), Some(("5h", 60.)));
    }
}

/// No `agy` a cota é por grupo de modelos: o do Gemini nas janelas gerais e o "Claude/GPT" nas por modelo. A sessão
/// vê só o grupo do modelo dela, com o rótulo de sempre ("5h", "7d").
fn agy_group(bars: Vec<Bar>, model: Option<&str>) -> Vec<Bar> {
    let third = model.is_some_and(|m| m.contains("claude") || m.contains("gpt"));
    bars.into_iter().filter(|b| b.per_model == third).map(|mut b| {
        if let Some((_, window)) = b.label.rsplit_once(' ') { b.label = window.to_owned(); }
        b.per_model = false;
        b
    }).collect()
}

/// A janela mais cheia; janela de um modelo só conta quando é o modelo da sessão, e no empate vence a que renova antes.
fn fullest<'a>(bars: &'a [Bar], model: Option<&str>) -> Option<&'a Bar> {
    bars.iter()
        .filter(|b| !(b.per_model && model.is_some_and(|m| !m.contains(&b.label.to_lowercase()))))
        .fold(None, |best: Option<&Bar>, b| match best {
            None => Some(b),
            Some(best) if b.pct > best.pct => Some(b),
            Some(best) if b.pct == best.pct && b.reset_at.is_some_and(|at| best.reset_at.is_none_or(|was| at < was)) => Some(b),
            best => best,
        })
}

/// Uma janela numa linha: rótulo, barra, % e, embaixo, quando ela renova ("↺ 1h20" na curta, "↺ sáb 27/09 15h · 3 dias"
/// na semanal, com os dias da coluna "Reset semanal" das Contas). `fill` (0 a 1) é quanto da barra já subiu ao abrir.
fn meter(bar: &Bar, fill: f32) -> Div {
    let label = if bar.label == "5h" { tr("usage_card_session") } else { window_label(&bar.label) };
    div().flex().flex_col().gap(px(1.))
        .child(div().flex().items_center().gap(px(8.)).text_size(px(12.))
            .child(div().w(px(52.)).flex_shrink_0().truncate().text_color(theme::muted()).child(label))
            .child(div().flex_1().h(px(4.)).rounded_full().bg(theme::border_strong())
                .child(div().h_full().rounded_full().bg(level(bar.pct)).w(relative((bar.pct.clamp(0., 100.) / 100.) as f32 * fill))))
            .child(div().w(px(34.)).flex_shrink_0().flex().justify_end().text_color(if bar.pct > 80. { level(bar.pct) } else { theme::muted() })
                .child(format!("{}%", bar.pct.round()))))
        .when(!bar.reset.is_empty(), |el| el.child(div().pl(px(60.)).truncate().text_size(px(11.)).text_color(theme::faint())
            .child(match bar.reset_days.as_str() {
                days if bar.label == "7d" && days != "—" => format!("↺ {} · {days}", bar.reset),
                _ => format!("↺ {}", bar.reset),
            })))
}
