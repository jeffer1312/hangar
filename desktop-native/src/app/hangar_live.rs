//! Terminais de atalho vivos (evento `shortcut_terminals` do stream da lista): o estado que o chip "N no Hangar", os
//! blocos de atalho, o painel de terminal e o cartão da pergunta leem. Porta de `HangarRunning.svelte`,
//! `ShortcutQuestion.svelte`, `ShortcutTiles.svelte` e `lib/hangarTerminals.svelte.ts`.
use super::*;
use gpui_kit::component::dialog::Dialog;
use super::{popup, side::{Shortcut, now_seconds}, terminal::{LiveTerm, ShortcutTerm, TermQuestion, hangar_failure}};

/// O backend grava a chave com o espaço colapsado; comparar do mesmo jeito.
pub(super) fn norm_key(key: &str) -> String { key.split_whitespace().collect::<Vec<_>>().join(" ") }

/// Identidade do atalho: global = `global:<id>`; do projeto = `project:<repositório>:<id>` (o mesmo id em repositórios
/// diferentes não pode dividir uma cópia).
pub(super) fn shortcut_key(project: Option<&str>, id: &str) -> String {
    match project { Some(project) => format!("project:{project}:{id}"), None => format!("global:{id}") }
}

/// "rodando há": `created` em segundos, nunca "0 min".
pub(super) fn elapsed(created: i64, now: i64) -> String {
    let minutes = ((now - created) / 60).max(1);
    if minutes < 60 { tr_shared("hangar_min", &[("n", &minutes.to_string())]) }
    else { tr_shared("hangar_h", &[("n", &(minutes / 60).to_string())]) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum TileState { #[default] Idle, Running, Asking, Exited }

/// O que o bloco de um atalho `shell` mostra do terminal dele.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Tile { pub state: TileState, pub line: String, pub tip: Option<String>, pub mark: bool, pub opened_in: Option<String> }

/// Terminal que o bloco de `shortcut` representa: No Hangar, a cópia do servidor (dono vazio); na sessão `session`, o
/// mais novo dela.
fn tile_term<'a>(live: &'a [LiveTerm], session: &str, key: &str, hangar: bool) -> Option<&'a LiveTerm> {
    let want = norm_key(key);
    let mut found = live.iter().filter(|t| norm_key(&t.key) == want && if hangar { t.owner.is_empty() } else { t.owner == session });
    if hangar { found.next() } else { found.last() }
}

/// Estado do bloco de `shortcut` na sessão `session`. O da sessão que saiu não acende nem marca "caiu": a saída continua
/// na aba do painel.
pub(super) fn tile_of(live: &[LiveTerm], session: &str, shortcut: &Shortcut, now: i64) -> Tile {
    let Shortcut::Shell { key, hangar, label, .. } = shortcut else { return Tile::default() };
    let term = tile_term(live, session, key, *hangar);
    let state = match term {
        None => TileState::Idle,
        Some(t) if !t.term.alive => if *hangar { TileState::Exited } else { TileState::Idle },
        Some(t) if t.question.is_some() => TileState::Asking,
        Some(_) => TileState::Running,
    };
    let line = match (state, term) {
        (TileState::Running, Some(t)) if t.created > 0 => tr_shared("hangar_rodando_ha", &[("tempo", &elapsed(t.created, now))]),
        (TileState::Asking, _) => tr_shared("atalho_tile_pergunta", &[]),
        (TileState::Exited, Some(t)) => tr_shared("atalho_tile_caiu", &[("codigo", &t.term.exit_code.map_or_else(|| "?".to_owned(), |c| c.to_string()))]),
        _ => String::new(),
    };
    let elsewhere = term.filter(|t| *hangar && t.term.alive && !t.origin.is_empty() && t.origin != session);
    let tip = match elsewhere {
        Some(t) => Some(tr_shared("atalho_tile_dica_hangar", &[("rotulo", label), ("sessao", &t.origin)])),
        None => (state == TileState::Running).then(|| tr_shared("atalho_tile_parar", &[("rotulo", label)])),
    };
    Tile { state, line, tip, mark: *hangar, opened_in: elsewhere.map(|t| t.origin.clone()) }
}

/// Terminal vivo e sem pergunta que o bloco acende; o clique nele encerra o atalho.
pub(super) fn running_term(live: &[LiveTerm], session: &str, shortcut: &Shortcut) -> Option<LiveTerm> {
    let Shortcut::Shell { key, hangar, .. } = shortcut else { return None };
    tile_term(live, session, key, *hangar).filter(|t| t.term.alive && t.question.is_none()).cloned()
}

/// Ações das linhas do popover do chip.
#[derive(Clone, Copy)]
enum Act { Go, Terminal, Stop, Answer, Output, Again, Dismiss }

/// Rota de `hangar-terminals/{id}/…` que uma ação chamou.
#[derive(Clone, Copy, Debug)]
pub(super) enum HangarCall { Focus, Restart, Close }

/// Onde o chip mora: ao lado da marca (barra aberta e abas) ou um ponto no canto da marca (trilho).
#[derive(Clone, Copy, PartialEq)]
/// `Count` é a forma curta da faixa de abas: o ícone e o número, com a frase no tooltip.
pub(super) enum Chip { Label, Dot, Count }

impl Hangar {
    /// Cópia da lista viva da máquina `server` (a ativa vem do stream próprio; as outras, do `RemoteList`).
    pub(super) fn live_for(&self, server: &str) -> Vec<LiveTerm> {
        let key = servers::norm(server);
        if self.is_active_key(&key) { self.live_terms.clone() } else { self.remote.get(&key).map(|l| l.live_terms.clone()).unwrap_or_default() }
    }

    /// Terminais No Hangar (sem dono) da máquina `server`, do mais antigo ao mais novo.
    pub(super) fn hangar_terms_of(&self, server: &str) -> Vec<ShortcutTerm> {
        self.live_for(server).into_iter().filter(|t| t.owner.is_empty()).map(|t| t.term).collect()
    }

    /// Todos os No Hangar de todas as máquinas, a ativa primeiro.
    fn all_hangar(&self) -> Vec<(String, LiveTerm)> {
        let mut keys = vec![self.active_key()];
        for entry in &self.servers {
            let key = servers::norm(&entry.address);
            if !keys.contains(&key) { keys.push(key); }
        }
        keys.into_iter().flat_map(|key| self.live_for(&key).into_iter().filter(|t| t.owner.is_empty()).map(move |t| (key.clone(), t))).collect()
    }

    /// Terminal do atalho com pergunta na tela (a cópia No Hangar, ou o mais novo da sessão `session`).
    pub(super) fn asking_term(&self, server: &str, key: &str, hangar: bool, session: &str) -> Option<LiveTerm> {
        tile_term(&self.live_for(server), session, key, hangar).filter(|t| t.term.alive && t.question.is_some()).cloned()
    }

    pub(super) fn tile_for(&self, server: &str, session: &str, shortcut: &Shortcut) -> Tile {
        if !matches!(shortcut, Shortcut::Shell { .. }) { return Tile::default(); }
        tile_of(&self.live_for(server), session, shortcut, now_seconds() as i64)
    }

    /// Uma linha por atalho No Hangar vivo que outra sessão abriu (rótulo e sessão), para a nota única sob a grade.
    pub(super) fn hangar_notes(&self, server: &str, session: &str, tiles: &[(String, Shortcut, bool)]) -> Vec<(String, String)> {
        let live = self.live_for(server);
        tiles.iter().filter_map(|(_, shortcut, _)| tile_of(&live, session, shortcut, 0).opened_in.map(|origin| (shortcut.label(), origin))).collect()
    }

    /// A lista viva de `server` mudou (ou o stream caiu): o que depende dela se acerta.
    pub(super) fn live_changed(&mut self, server: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.hangar_open && self.all_hangar().is_empty() { self.hangar_open = false; }
        if self.terminal.as_ref().is_some_and(|panel| panel.server_is(server)) { self.sync_hangar_tabs(); }
        self.sync_question(window, cx);
        self.keep_live_clock(cx);
        cx.notify();
    }

    /// O tempo de "rodando · 3 min" anda enquanto houver um No Hangar vivo com hora de início.
    fn live_running(&self) -> bool { self.all_hangar().iter().any(|(_, t)| t.term.alive && t.created > 0) }

    fn keep_live_clock(&mut self, cx: &mut Context<Self>) {
        if self.live_clock.is_some() || !self.live_running() { return; }
        self.live_clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let keep = this.update(cx, |this, cx| { cx.notify(); this.live_running() });
            if !matches!(keep, Ok(true)) {
                let _ = this.update(cx, |this, _| this.live_clock = None);
                break;
            }
        }));
    }

    /// Aviso do bloco No Hangar depois da resposta de `shortcut-shell`: a cópia já rodando não vira segunda cópia.
    pub(super) fn receive_hangar_shell(&mut self, key: SessionKey, label: String, terminal: Option<String>, result: Result<Value, Failure>,
        window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_shortcut_terms(&key.name);
        let started = tr("shortcut_started").replace("{label}", &label);
        let note = match &result {
            // Servidor antigo ignora `runs_in` e abriria uma cópia da sessão: sem `reused` na resposta, avisa.
            Ok(value) => match value.get("reused").and_then(Value::as_bool) {
                None => Some((tr_shared("hangar_servidor_desatualizado", &[]), true)),
                Some(reused) => {
                    let focused = value.get("focused").and_then(Value::as_bool).unwrap_or(false);
                    if let (true, false, Some(id)) = (reused, focused, terminal) { self.open_hangar_terminal(&key.server, &id, window, cx); }
                    None
                }
            },
            Err(error) if matches!(error.status, Some(404 | 405)) => Some((tr("shortcut_shell_unsupported"), true)),
            Err(error) => Some((format!("{label}: {}", hangar_failure(error)), true)),
        };
        match note {
            Some(note) => { self.action_feedback.insert(key, note); }
            // O bloco já mostra o estado: o "iniciando" sai, se ainda é ele.
            None => if self.action_feedback.get(&key).is_some_and(|(text, _)| *text == started) { self.action_feedback.remove(&key); },
        }
        cx.notify();
    }

    // ---- chip e popover ----

    /// O chip "N no Hangar"; some sem terminal No Hangar (e enquanto o stream da lista não trouxe nada).
    pub(super) fn render_hangar_chip(&self, kind: Chip, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = self.all_hangar();
        if items.is_empty() { return None; }
        let asking = items.iter().any(|(_, t)| t.term.alive && t.question.is_some());
        let color = if asking { theme::warning() } else { theme::success() };
        let label = tr_shared("hangar_chip", &[("n", &items.len().to_string())]);
        // Aberto, o foco vai para a primeira ação da lista (o botão dela segue o `hangar_focus`), como o web.
        let toggle = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.hangar_open = !this.hangar_open;
            this.hangar_error = None;
            if this.hangar_open {
                let focus = this.hangar_focus.clone();
                window.on_next_frame(move |window, cx| focus.focus(window, cx));
            }
            cx.notify();
        };
        let element = match kind {
            Chip::Dot => {
                let tip = label.clone();
                div().id("hangar-chip").flex_shrink_0().size(px(12.)).rounded_full().bg(color).border_2().border_color(theme::background())
                    .cursor_pointer().role(Role::Button).aria_label(label)
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| toggle(this, window, cx))).into_any_element()
            }
            Chip::Label => Button::new("hangar-chip")
                .custom(ButtonCustomVariant::new(cx).color(theme::accent().opacity(0.14)).foreground(theme::accent_text())
                    .hover(theme::accent().opacity(0.2)).active(theme::accent().opacity(0.24)))
                .h(px(30.)).px(px(12.)).rounded_full().border_1().border_color(theme::accent().opacity(0.5)).flex_shrink_0()
                .selected(self.hangar_open).accessibility_label(label.clone()).tooltip(label.clone())
                .child(div().flex().items_center().gap(px(8.)).child(div().size(px(7.)).rounded_full().bg(color))
                    .child(div().text_xs().whitespace_nowrap().child(label)))
                .on_click(cx.listener(move |this, _, window, cx| toggle(this, window, cx))).into_any_element(),
            Chip::Count => Button::new("hangar-chip").ghost()
                .h(px(26.)).px(px(7.)).rounded_full().flex_shrink_0()
                .selected(self.hangar_open).accessibility_label(label.clone()).tooltip(label)
                .child(div().flex().items_center().gap(px(5.))
                    .child(chrome::small_icon(IconName::SquareTerminal, 13., theme::muted()))
                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(color).child(items.len().to_string())))
                .on_click(cx.listener(move |this, _, window, cx| toggle(this, window, cx))).into_any_element(),
        };
        Some(popup::anchor(div().relative().flex_shrink_0(), "hangar-chip").child(element).into_any_element())
    }

    /// A lista do chip: uma linha por terminal, com as ações do `HangarRunning.svelte`. Fundo sólido: fica sobre a
    /// conversa e a lista, e o vidro deixaria o texto de baixo atravessar.
    pub(super) fn render_hangar_popover(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let items = self.all_hangar();
        let multi = items.iter().map(|(server, _)| server.as_str()).collect::<HashSet<_>>().len() > 1;
        let now = now_seconds() as i64;
        // Muitos terminais rolam dentro da janela, em vez de passar dela (`max-height` do web).
        let max_height = (f32::from(window.viewport_size().height) - 120.).max(160.);
        let mut rows = Vec::new();
        for (row, (server, t)) in items.into_iter().enumerate() {
            let asking = t.term.alive && t.question.is_some();
            let origin = if t.origin.is_empty() { String::new() } else if multi {
                let label = self.server_entry(&server).map_or_else(|| server.clone(), |s| s.label.clone());
                format!("{label}::{}", t.origin)
            } else { t.origin.clone() };
            let meta = if !t.term.alive {
                tr_shared("hangar_caiu", &[("codigo", &t.term.exit_code.map_or_else(|| "?".to_owned(), |c| c.to_string()))])
            } else if asking { tr_shared("hangar_esperando", &[]) } else {
                let time = elapsed(t.created, now);
                if origin.is_empty() { time } else { format!("{time} · {}", tr_shared("hangar_aberto_em", &[("sessao", &origin)])) }
            };
            let dot = div().size(px(8.)).flex_shrink_0().rounded_full()
                .when(!t.term.alive, |el| el.border_1().border_color(theme::faint()))
                .when(t.term.alive, |el| el.bg(if asking { theme::warning() } else { theme::success() }));
            let acts: &[Act] = if !t.term.alive { &[Act::Output, Act::Again, Act::Dismiss] }
                else if asking { &[Act::Answer, Act::Terminal, Act::Stop] } else { &[Act::Go, Act::Terminal, Act::Stop] };
            rows.push(div().flex().flex_col().gap(px(10.)).px(px(16.)).py(px(12.)).border_t_1().border_color(theme::border())
                .when(asking, |el| el.bg(theme::warning().opacity(0.06)))
                .child(div().flex().items_center().gap(px(10.)).min_w_0().child(dot)
                    .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM)
                        .text_color(if t.term.alive { theme::text() } else { theme::muted() }).child(t.term.label.clone()))
                    .child(div().min_w_0().truncate().text_xs()
                        .text_color(if !t.term.alive { theme::danger() } else if asking { theme::warning_text() } else { theme::faint() }).child(meta)))
                .when_some(t.question.as_ref().filter(|_| asking), |el, q| el.child(div().pl(px(18.)).text_xs().text_color(theme::muted()).whitespace_normal().child(q.text.clone())))
                .child(div().pl(px(18.)).flex().flex_wrap().gap(px(6.))
                    .children(acts.iter().enumerate().map(|(n, act)| self.hangar_button(&server, &t.term.id, *act, row == 0 && n == 0, cx)))));
        }
        div().flex().flex_col().rounded(px(12.)).bg(theme::elevated()).overflow_hidden()
            .child(div().px(px(16.)).pt(px(12.)).pb(px(8.)).text_xs().text_color(theme::faint()).child(tr_shared("hangar_lista_titulo", &[])))
            .child(div().id("hangar-list").max_h(px(max_height)).overflow_y_scroll().flex().flex_col().children(rows))
            .when_some(self.hangar_error.clone(), |el, error| el.child(div().id("hangar-error").role(Role::Alert).px(px(14.)).py(px(8.)).text_sm()
            .text_color(theme::danger()).whitespace_normal().child(error))).into_any_element()
    }

    fn hangar_button(&self, server: &str, id: &str, act: Act, first: bool, cx: &mut Context<Self>) -> Button {
        let label = tr_shared(match act {
            Act::Go => "hangar_ir_janela", Act::Terminal => "hangar_terminal", Act::Stop => "hangar_parar", Act::Answer => "hangar_responder",
            Act::Output => "hangar_ver_saida", Act::Again => "hangar_rodar_de_novo", Act::Dismiss => "hangar_dispensar",
        }, &[]);
        let base = Button::new(SharedString::from(format!("hangar-{}-{id}", act as u8)));
        let button = match act {
            Act::Go => base.custom(ButtonCustomVariant::new(cx).color(theme::accent_press()).foreground(theme::on_press())
                .hover(theme::accent()).active(theme::accent_press())),
            Act::Answer => base.custom(ButtonCustomVariant::new(cx).color(theme::warning_press()).foreground(theme::on_press())
                .hover(theme::warning_press().opacity(0.88)).active(theme::warning_press().opacity(0.78))),
            Act::Stop => base.custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::removed())
                .hover(theme::danger().opacity(0.12)).active(theme::danger().opacity(0.2))).border_1().border_color(theme::border_strong()),
            Act::Dismiss => base.ghost(),
            _ => base.outline(),
        };
        let (server, id) = (server.to_owned(), id.to_owned());
        button.small().h(px(30.)).px(px(12.)).rounded(px(7.)).label(label).when(first, |button| button.track_focus(&self.hangar_focus)).on_click(cx.listener(move |this, _, window, cx| this.hangar_act(act, &server, &id, window, cx)))
    }

    fn hangar_act(&mut self, act: Act, server: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Terminal | Act::Output => { self.hangar_open = false; self.open_hangar_terminal(server, id, window, cx); }
            Act::Answer => { self.hangar_open = false; self.open_question(server, "", id, window, cx); }
            Act::Go => self.hangar_call(HangarCall::Focus, server, id),
            Act::Again => self.hangar_call(HangarCall::Restart, server, id),
            Act::Stop | Act::Dismiss => self.hangar_call(HangarCall::Close, server, id),
        }
        cx.notify();
    }

    fn hangar_call(&mut self, call: HangarCall, server: &str, id: &str) {
        self.hangar_error = None;
        let Some(api) = self.machine_api(server) else {
            self.hangar_error = Some(tr_shared("hangar_erro", &[("msg", &self.machine_error(server))]));
            return;
        };
        let path = match call { HangarCall::Focus => "focus", HangarCall::Restart => "restart", HangarCall::Close => "close" };
        let (connection, tx, server, id) = (self.connection, self.tx.clone(), server.to_owned(), id.to_owned());
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["hangar-terminals", &id, path], None, 20).await;
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::Terminal(terminal::Reply::Hangar(call, server, id, result)) }).await;
        });
    }

    /// Resposta de uma ação do popover. Sem janela para trazer, o menu fica aberto com o aviso e o terminal abre por baixo.
    pub(super) fn receive_hangar_call(&mut self, call: HangarCall, server: String, id: String, result: Result<Value, Failure>,
        window: &mut Window, cx: &mut Context<Self>) {
        match (call, result) {
            (HangarCall::Focus, Ok(value)) => if value.get("focused").and_then(Value::as_bool) == Some(true) { self.hangar_open = false; } else {
                self.hangar_error = Some(tr_shared("hangar_sem_janela", &[]));
                self.open_hangar_terminal(&server, &id, window, cx);
            },
            (_, Ok(_)) => {}
            (_, Err(error)) => self.hangar_error = Some(tr_shared("hangar_erro", &[("msg", &hangar_failure(&error))])),
        }
        cx.notify();
    }

    // ---- cartão da pergunta ----

    /// Abre o cartão da pergunta do terminal `id` (`owner` vazio = No Hangar). Já aberto para o mesmo terminal, só o acompanha.
    pub(super) fn open_question(&mut self, server: &str, owner: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let server = servers::norm(server);
        let Some(api) = self.machine_api(&server) else {
            // Sem a conexão da máquina o clique não pode morrer calado; o popover pode estar fechado, então também avisa na janela.
            let text = tr_shared("hangar_erro", &[("msg", &self.machine_error(&server))]);
            self.hangar_error = Some(text.clone());
            window.push_notification(Notification::warning(text), cx);
            cx.notify();
            return;
        };
        let target = (server.clone(), owner.to_owned(), id.to_owned());
        if self.question_open.as_ref() == Some(&target) && self.question_card.is_some() { self.sync_question(window, cx); return; }
        self.close_question(window, cx);
        let term = self.live_for(&server).into_iter().find(|t| t.term.id == id);
        let label = term.as_ref().map(|t| t.term.label.clone()).unwrap_or_default();
        self.question_open = Some(target);
        let hangar = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        let (server_key, owner_name, term_id) = (server.clone(), owner.to_owned(), id.to_owned());
        let card = cx.new(|cx| QuestionCard::new(api, runtime, hangar.clone(), server_key, owner_name, term_id, term, tr_shared("pergunta_titulo", &[("rotulo", &label)]), window, cx));
        self.question_card = Some(card.clone());
        let mine = card.entity_id();
        window.open_dialog(cx, move |dialog, _, _| question_dialog(dialog).child(card.clone())
            .on_close({
                let hangar = hangar.clone();
                move |_, _, cx| { let _ = hangar.update(cx, |this, _| {
                    // Um cartão novo já pode ter tomado o lugar enquanto este fechava.
                    if this.question_card.as_ref().is_some_and(|card| card.entity_id() == mine) { (this.question_open, this.question_card) = (None, None); }
                }); }
            }));
        cx.notify();
    }

    /// Fecha o cartão pelo código: o estado sai antes, porque `close_dialog` só tira o diálogo do topo e não chama o
    /// `on_close` (que só corre no Esc e no fechar do kit). Sem cartão registrado, nada é fechado.
    pub(super) fn close_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.question_open = None;
        if self.question_card.take().is_none() { return; }
        window.close_dialog(cx);
        cx.notify();
    }

    /// Zera o estado do cartão sem tocar no diálogo: quem chama `close_all_dialogs` não passa pelo `on_close`,
    /// e o cartão registrado sem diálogo travaria a próxima pergunta em "já aberto".
    pub(super) fn forget_question(&mut self) { (self.question_open, self.question_card) = (None, None); }

    /// `close_question` só se o cartão registrado ainda é `card`: um temporizador velho não pode fechar o diálogo de outro.
    fn close_question_of(&mut self, card: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if self.question_card.as_ref().is_some_and(|open| open.entity_id() == card) { self.close_question(window, cx); }
    }

    fn sync_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((server, _, id)), Some(card)) = (self.question_open.clone(), self.question_card.clone()) else { return; };
        let term = self.live_for(&server).into_iter().find(|t| t.term.id == id);
        card.update(cx, |card, cx| card.sync(term, window, cx));
    }
}

/// Assinatura de uma pergunta: a lista só atualiza depois do envio, e até lá a pergunta velha não pode voltar como nova.
/// A tela entra: a mesma pergunta repetida (senha errada) traz uma linha nova acima dela.
fn signature(question: &TermQuestion) -> String { format!("{}|{}|{}", question.text, question.default, question.screen.join("\n")) }

struct Answered { text: String, value: String, hidden: bool }

/// Diálogo da pergunta de um terminal de atalho (na sessão ou No Hangar): guarda as respostas desta rodada e fica aberto
/// entre uma pergunta e a seguinte do mesmo terminal.
pub(super) struct QuestionCard {
    api: Api,
    runtime: Arc<tokio::runtime::Runtime>,
    hangar: WeakEntity<Hangar>,
    server: String,
    owner: String,
    id: String,
    title: String,
    term: Option<LiveTerm>,
    input: Entity<InputState>,
    hide: bool,
    sending: bool,
    waiting: bool,
    error: Option<String>,
    answered: Vec<Answered>,
    last_sig: String,
    sent_sig: String,
    close_timer: Option<Task<()>>,
    _events: Subscription,
}

impl QuestionCard {
    #[allow(clippy::too_many_arguments)]
    fn new(api: Api, runtime: Arc<tokio::runtime::Runtime>, hangar: WeakEntity<Hangar>, server: String, owner: String, id: String,
        term: Option<LiveTerm>, title: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.send(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let field = input.clone();
        cx.defer_in(window, move |_, window, cx| field.update(cx, |state, cx| state.focus(window, cx)));
        let mut card = Self { api, runtime, hangar, server, owner, id, title, term: None, input, hide: false, sending: false, waiting: false, error: None,
            answered: Vec::new(), last_sig: String::new(), sent_sig: String::new(), close_timer: None, _events: events };
        card.sync(term, window, cx);
        card
    }

    fn pending(&self) -> bool {
        self.term.as_ref().and_then(|t| t.question.as_ref()).is_some_and(|q| signature(q) != self.sent_sig)
    }

    /// A lista trouxe o estado novo do terminal (`None` = sumiu). Pergunta nova entra com o padrão preenchido; a que já
    /// foi respondida não volta como nova.
    fn sync(&mut self, term: Option<LiveTerm>, window: &mut Window, cx: &mut Context<Self>) {
        self.term = term;
        match self.term.as_ref().and_then(|t| t.question.as_ref()) {
            None => self.sent_sig.clear(),
            Some(question) => {
                let sig = signature(question);
                if sig != self.sent_sig && sig != self.last_sig {
                    let default = question.default.clone();
                    self.input.update(cx, |state, cx| state.set_value(default, window, cx));
                    self.last_sig = sig;
                    self.waiting = false;
                }
            }
        }
        self.arm_close(window, cx);
        cx.notify();
    }

    /// Depois de responder, espera a próxima pergunta por 5 s; sem ela, fecha.
    fn arm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_timer = None;
        if !self.waiting || self.term.as_ref().is_some_and(|t| t.question.is_some()) { return; }
        let (hangar, mine) = (self.hangar.clone(), cx.entity_id());
        self.close_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.term.as_ref().is_none_or(|t| t.question.is_none()) {
                    let _ = hangar.update(cx, |hangar, cx| hangar.close_question_of(mine, window, cx));
                }
            });
        }));
    }

    /// A resposta vai como foi digitada (um `-` ou `;` na frente vale); vazia aceita o padrão.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(question) = self.term.as_ref().and_then(|t| t.question.clone()) else { return; };
        if self.sending || !self.pending() { return; }
        let text = self.input.read(cx).value().to_string();
        (self.sending, self.error) = (true, None);
        let (api, owner, id, body) = (self.api.clone(), self.owner.clone(), self.id.clone(), json!({"text": text}));
        let job = self.runtime.spawn(async move {
            if owner.is_empty() { api.server_send(reqwest::Method::POST, &["hangar-terminals", &id, "answer"], Some(body), 15).await }
            else { api.server_send(reqwest::Method::POST, &["sessions", &owner, "shortcut-terminals", &id, "answer"], Some(body), 15).await }
        });
        let hidden = self.hide;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = job.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.sending = false;
                match result {
                    Ok(_) => {
                        this.sent_sig = signature(&question);
                        this.answered.push(Answered { text: question.text, value: text, hidden });
                        this.waiting = true;
                        this.last_sig.clear();
                        this.arm_close(window, cx);
                    }
                    Err(error) => this.error = Some(tr_shared("hangar_erro", &[("msg", &hangar_failure(&error))])),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (hangar, server, owner, id, mine) = (self.hangar.clone(), self.server.clone(), self.owner.clone(), self.id.clone(), cx.entity_id());
        let _ = hangar.update(cx, |this, cx| {
            this.close_question_of(mine, window, cx);
            if owner.is_empty() { this.open_hangar_terminal(&server, &id, window, cx); }
            else { this.open_session_terminal(&server, &owner, &id, window, cx); }
        });
    }
}

/// O cartão desenha a própria superfície (cartão âmbar e caixa da tela soltos, como no desenho): o diálogo só segura o lugar.
fn question_dialog(dialog: Dialog) -> Dialog {
    dialog.w(px(512.)).p(px(0.)).bg(transparent_black()).border_color(transparent_black()).close_button(false)
}

/// O prompt do script traz o exemplo entre parênteses; a linha da resposta dada fica só com o nome.
fn short_text(text: &str) -> &str {
    let t = text.trim_end();
    let t = t.strip_suffix(':').map_or(t, str::trim_end);
    let stripped = t.strip_suffix(')').and_then(|head| head.rfind('(').map(|n| head[..n].trim_end())).unwrap_or(t);
    let stripped = stripped.trim_end_matches(':').trim_end();
    // Um prompt que é só o parêntese não pode virar linha vazia.
    if stripped.is_empty() { t.trim_end_matches(':').trim_end() } else { stripped }
}

impl Render for QuestionCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.pending();
        let mut card = div().id("question-card").role(Role::Dialog).aria_label(self.title.clone())
            .w_full().flex().flex_col().gap(px(16.)).p(px(20.)).rounded(px(14.)).bg(theme::surface())
            .border_1().border_color(theme::warning().opacity(0.4))
            .child(div().flex().items_center().gap(px(10.))
                .child(chrome::small_icon(IconName::SquareTerminal, 18., theme::warning()))
                .child(div().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                .child(div().flex_1())
                .when(self.owner.is_empty(), |el| el.child(div().flex_shrink_0().text_size(px(11.)).text_color(theme::faint()).child(tr_shared("pergunta_marca_hangar", &[])))));
        for done in &self.answered {
            let value = if done.hidden { "••••".to_owned() } else if done.value.is_empty() { "⏎".to_owned() } else { done.value.clone() };
            card = card.child(div().flex().items_center().gap(px(10.)).px(px(12.)).py(px(10.)).rounded(px(8.)).bg(theme::success().opacity(0.07))
                .child(chrome::small_icon(IconName::Check, 14., theme::success()))
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(theme::muted()).child(short_text(&done.text).to_owned()))
                .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(12.)).child(value)));
        }
        let question = self.term.as_ref().and_then(|t| t.question.as_ref()).filter(|_| pending);
        match question {
            Some(question) => {
                card = card.child(div().flex().flex_col().gap(px(8.))
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).whitespace_normal().child(question.text.clone()))
                    .child(Input::new(&self.input))
                    .when(!question.default.is_empty(), |el| el.child(div().text_size(px(12.)).text_color(theme::faint()).child(tr_shared("pergunta_padrao", &[])))));
            }
            None => {
                card = card.child(div().id("question-status").role(Role::Status).text_sm().text_color(theme::muted())
                    .child(if self.waiting { tr_shared("pergunta_aguardando", &[]) } else { tr("loading") }));
            }
        }
        if let Some(error) = self.error.clone() {
            card = card.child(div().id("question-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(error));
        }
        // "Esconder" à esquerda; Abrir terminal e Enviar à direita, na mesma linha.
        let hide = pending.then(|| Checkbox::new("question-hide").label(tr_shared("pergunta_esconder", &[])).checked(self.hide)
            .on_click(cx.listener(|this, on: &bool, window, cx| {
                this.hide = *on;
                this.input.update(cx, |state, cx| state.set_masked(*on, window, cx));
                cx.notify();
            })));
        card = card.child(div().flex().items_center().gap(px(8.)).children(hide).child(div().flex_1())
            .child(Button::new("question-open-terminal").outline().h(px(36.)).px(px(14.)).rounded(px(8.)).label(tr_shared("pergunta_abrir_terminal", &[]))
                .on_click(cx.listener(|this, _, window, cx| this.open_terminal(window, cx))))
            .child(Button::new("question-send")
                .custom(ButtonCustomVariant::new(cx).color(theme::warning_press()).foreground(theme::on_press())
                    .hover(theme::warning_press().opacity(0.88)).active(theme::warning_press().opacity(0.78)))
                .h(px(36.)).px(px(16.)).rounded(px(8.)).label(tr_shared("pergunta_enviar", &[])).loading(self.sending)
                .disabled(self.sending || !pending).on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))));
        // A tela do terminal é uma caixa à parte, abaixo do cartão; a última linha (a pergunta) leva o cursor âmbar.
        let screen = question.map(|question| div().w_full().flex().flex_col().gap(px(8.)).px(px(16.)).py(px(14.)).rounded(px(10.))
            .bg(theme::inset()).border_1().border_color(theme::border())
            .child(div().text_size(px(11.)).text_color(theme::faint()).child(tr_shared("pergunta_tela", &[]).to_uppercase()))
            .children(question.screen.iter().enumerate().map(|(n, line)| {
                let last = n + 1 == question.screen.len();
                div().flex().items_end().gap(px(4.)).font_family(theme::MONO).text_size(px(12.)).line_height(px(20.)).text_color(if last { theme::text() } else { theme::muted() })
                    .child(div().min_w_0().whitespace_normal().child(line.clone()))
                    .when(last, |el| el.child(div().flex_shrink_0().mb(px(3.)).w(px(7.)).h(px(14.)).bg(theme::warning())))
            })));
        div().w_full().flex().flex_col().gap(px(14.)).child(card).children(screen)
    }
}

#[cfg(test)]
mod live_tests {
    use super::{Shortcut, TileState, elapsed, norm_key, running_term, shortcut_key, short_text, signature, tile_of};
    use crate::app::terminal::{LiveTerm, ShortcutTerm, TermQuestion};

    fn shell(key: &str, hangar: bool) -> Shortcut {
        Shortcut::Shell { label: "VM".into(), command: "rdp".into(), confirm: false, icon: None, pasta: None, key: key.into(), hangar, home: true, ask: true }
    }

    fn term(key: &str, owner: &str, alive: bool, question: Option<&str>, created: i64) -> LiveTerm {
        LiveTerm { term: ShortcutTerm { id: format!("id-{owner}-{key}"), label: "VM".into(), alive, exit_code: (!alive).then_some(3) },
            owner: owner.into(), key: key.into(), origin: "pm-1".into(), created,
            question: question.map(|text| TermQuestion { text: text.into(), default: String::new(), screen: Vec::new() }) }
    }

    #[test]
    fn keys_compare_with_collapsed_whitespace_and_project_ids_do_not_collide() {
        assert_eq!(norm_key("global:a  b\t c"), "global:a b c");
        assert_eq!(shortcut_key(None, "vm"), "global:vm");
        assert_eq!(shortcut_key(Some("/repo"), "vm"), "project:/repo:vm");
        assert_ne!(shortcut_key(None, "vm"), shortcut_key(Some("/repo"), "vm"));
    }

    #[test]
    fn running_time_never_says_zero_minutes_and_rolls_into_hours() {
        assert_eq!(elapsed(1000, 1010), "1 min");
        assert_eq!(elapsed(1000, 1000 + 59 * 60), "59 min");
        assert_eq!(elapsed(1000, 1000 + 125 * 60), "2 h");
    }

    #[test]
    fn tile_follows_the_server_copy_for_hangar_and_the_newest_own_terminal_for_session() {
        let live = [term("global:vm", "", true, None, 500), term("global:vm", "pm-2", true, Some("Porta"), 500)];
        let hangar = tile_of(&live, "pm-2", &shell("global:vm", true), 500 + 300);
        assert_eq!((hangar.state, hangar.mark), (TileState::Running, true));
        assert_eq!(hangar.line, "rodando · 5 min");
        assert!(hangar.tip.is_some_and(|tip| tip.contains("pm-1")));
        // Na sessão: vale o terminal dela, e a cópia No Hangar não interfere.
        let session = tile_of(&live, "pm-2", &shell("global:vm", false), 0);
        assert_eq!((session.state, session.mark, session.line.is_empty()), (TileState::Asking, false, false));
        assert_eq!(tile_of(&live, "outra", &shell("global:vm", false), 0).state, TileState::Idle);
        let own = [term("global:vm", "pm-2", false, None, 100), term("global:vm", "pm-2", true, None, 500)];
        let running = tile_of(&own, "pm-2", &shell("global:vm", false), 500 + 300);
        assert_eq!((running.state, running.line.as_str()), (TileState::Running, "rodando · 5 min"));
        // O da sessão que saiu não fica aceso nem marca "caiu": a saída continua na aba do painel.
        assert_eq!(tile_of(&own[..1], "pm-2", &shell("global:vm", false), 900).state, TileState::Idle);
        // Sem hora de início a linha some; parado mostra o código.
        assert!(tile_of(&[term("k", "", true, None, 0)], "s", &shell("k", true), 100).line.is_empty());
        let dead = tile_of(&[term("k", "", false, None, 9)], "s", &shell("k", true), 100);
        assert_eq!(dead.state, TileState::Exited);
        assert!(dead.line.contains('3'));
        // A chave com espaço colapsado casa com a que o backend guardou.
        assert_eq!(tile_of(&[term("global:a b", "", true, None, 1)], "s", &shell("global:a  b", true), 100).state, TileState::Running);
    }

    #[test]
    fn a_click_stops_only_the_running_terminal_of_the_tile() {
        let hangar = [term("k", "", true, None, 1)];
        assert_eq!(running_term(&hangar, "s", &shell("k", true)).map(|t| t.term.id), Some("id--k".into()));
        let session = [term("k", "s", true, None, 1)];
        assert_eq!(running_term(&session, "s", &shell("k", false)).map(|t| t.term.id), Some("id-s-k".into()));
        // Outra sessão, terminal que saiu e terminal perguntando não param com o clique.
        assert!(running_term(&session, "outra", &shell("k", false)).is_none());
        assert!(running_term(&[term("k", "", false, None, 1)], "s", &shell("k", true)).is_none());
        assert!(running_term(&[term("k", "s", true, Some("Porta"), 1)], "s", &shell("k", false)).is_none());
    }

    #[test]
    fn answered_line_drops_the_example_in_parentheses_and_the_colon() {
        assert_eq!(short_text("Destino SSH da VM (usuario@host ou alias do ~/.ssh/config)"), "Destino SSH da VM");
        assert_eq!(short_text("Pasta do PSS na VM:"), "Pasta do PSS na VM");
        assert_eq!(short_text("Porta (3000):  "), "Porta");
        assert_eq!(short_text("Senha"), "Senha");
        assert_eq!(short_text("(x)"), "(x)");
    }

    #[test]
    fn a_question_is_new_only_when_its_text_default_or_screen_changes() {
        let q = |text: &str, default: &str| TermQuestion { text: text.into(), default: default.into(), screen: Vec::new() };
        assert_eq!(signature(&q("Porta", "3000")), signature(&q("Porta", "3000")));
        assert_ne!(signature(&q("Porta", "3000")), signature(&q("Porta", "3001")));
        let again = TermQuestion { screen: vec!["Sorry, try again.".into()], ..q("Porta", "3000") };
        assert_ne!(signature(&q("Porta", "3000")), signature(&again));
    }
}
