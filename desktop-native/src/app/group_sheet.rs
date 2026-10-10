//! Grupo de trabalho a partir do compositor, como o web (chip do par e "mandar pro grupo" do `Composer`, `PairSheet`): o chip
//! abre o painel com os membros e o estado de cada um, abrir a conversa de um membro, adicionar sessões, o contrato
//! compartilhado em markdown, a conversa do grupo e sair. Sem grupo, o painel escolhe sessões e a tarefa para formar um.
use super::*;
use super::activity::web;
use super::device::Remote;
use super::grouping::{failed, glyph, web_with};
use super::sidebar::SidebarReply;
use crate::api::dto::PairResult;
use gpui_kit::component::{WindowExt, checkbox::Checkbox};

/// Cauda do histórico de cada membro lida para a conversa do grupo.
// ponytail: o web lê o histórico inteiro; recado mais velho que esta cauda não aparece.
const FEED_TAIL: usize = 1000;
/// A conversa mostra só os últimos recados, como o web; o resto vive no chat de cada um.
const FEED_SHOWN: usize = 40;

#[derive(Debug, PartialEq)]
struct FeedItem { from: String, to: String, text: String, ts: f64 }

/// `missing`: membros cujo histórico falhou, que não é o mesmo que conversa vazia.
struct Feed { items: Vec<FeedItem>, missing: Vec<String> }

pub(super) struct Sheet {
    session: String,
    /// Máquina da sessão: pode não ser a ativa.
    server: String,
    id: u64,
    /// Membros de quando carregou: grupo que muda com o painel aberto recomeça tudo, como o web.
    peers_key: String,
    picked: Vec<String>,
    task: Entity<InputState>,
    adding: bool,
    busy: bool,
    error: Option<String>,
    /// Caminho e texto renderizado; `None` é grupo que ainda não escreveu contrato.
    contract: Remote<Option<(String, Entity<TextViewState>)>>,
    feed: Remote<Feed>,
}

/// Respostas amarradas ao painel que as pediu (`id`) e, nas leituras, ao pedido mais novo.
pub(super) enum SheetReply {
    Contract(u64, u64, Result<Value, Failure>),
    Feed(u64, u64, Vec<String>, Vec<(String, Result<Vec<ChatEvent>, Failure>)>),
    Paired(u64, Vec<String>, Result<PairResult, Failure>),
    Left(u64, Result<PairResult, Failure>),
}

/// O corpo do painel, redesenhado quando o `Hangar` muda (lista de sessões, respostas).
struct SheetBody { hangar: WeakEntity<Hangar>, _observe: Subscription }

impl Render for SheetBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.hangar.update(cx, |hangar, cx| hangar.render_group_sheet(cx)).unwrap_or_else(|_| div())
    }
}

/// `rotuloEstado` do web.
fn state_label(state: &str) -> Option<String> {
    Some(web(match state {
        "working" => "estado_em_execucao", "idle" => "estado_pronto", "awaiting_input" => "estado_aguardando", "dead" => "estado_encerrado",
        _ => return None,
    }))
}

fn dot(state: &str) -> Div { div().size(px(8.)).flex_shrink_0().rounded_full().bg(theme::status(state)) }

/// Recados `[de: X]` que um membro recebeu de outro membro, de todos os históricos, em ordem de tempo; só a cauda.
fn build_feed(members: &[String], histories: Vec<(String, Result<Vec<ChatEvent>, Failure>)>) -> Feed {
    let (mut items, mut missing) = (Vec::new(), Vec::new());
    for (owner, result) in histories {
        let Ok(events) = result else { missing.push(owner); continue };
        for event in events.iter().filter(|e| e.kind == "user_msg") {
            let Some(peer) = event.text.as_deref().and_then(crate::cards::peer_message) else { continue };
            if peer.from == owner || !members.contains(&peer.from) { continue; }
            let text = match peer.canal { Some(canal) => format!("[{canal}] {}", peer.body), None => peer.body };
            items.push(FeedItem { from: peer.from, to: owner.clone(), text, ts: event.ts.unwrap_or(0.) });
        }
    }
    items.sort_by(|a, b| a.ts.total_cmp(&b.ts));
    items.drain(..items.len().saturating_sub(FEED_SHOWN));
    Feed { items, missing }
}

/// Envio ao grupo: o `/broadcast` responde 200 com o resultado por sessão, e membro que não recebeu vira recusa dizendo quem
/// recebeu e quem não, como o web; sem isso a falha de um membro passaria calada.
pub(super) fn group_delivery(me: &str, results: Vec<(String, Delivery, Option<String>)>) -> Result<Delivery, Failure> {
    let names = |list: &[&(String, Delivery, Option<String>)]| list.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>().join(", ");
    let (reached, failed): (Vec<_>, Vec<_>) = results.iter().partition(|(_, delivery, _)| delivery.ok);
    let Some((_, _, reason)) = failed.first() else {
        let delivered = results.iter().any(|(name, delivery, _)| name == me && delivery.delivered);
        return Ok(Delivery { ok: true, delivered });
    };
    let reached = if reached.is_empty() { String::new() } else { web_with("chat_chegou_mas", "n", &names(&reached)) };
    let reason = reason.clone().unwrap_or_else(|| web("board_falha_envio"));
    Err(Failure::local(format!("{reached}{}{} ({reason})", web("chat_nao_chegou_em"), names(&failed))))
}

impl Hangar {
    /// Grupo é por máquina: os membros estão na lista da máquina da sessão aberta.
    fn members_list(&self) -> &[SessionInfo] { self.session_server().map_or(&[], |server| self.sessions_of(&server)) }

    fn live(&self, name: &str) -> Option<&SessionInfo> { self.members_list().iter().find(|s| s.name == name) }

    pub(super) fn live_peers(&self, name: &str) -> Vec<String> { self.live(name).map(|s| s.peers().to_vec()).unwrap_or_default() }

    /// Com o "mandar pro grupo" ligado nesta sessão e o mesmo grupo de quando ligou: ela e os membros. Comando `/` vai só para
    /// ela (o servidor recusa comando em grupo).
    pub(super) fn group_targets(&self, key: &SessionKey, text: &str) -> Option<Vec<String>> {
        let (on, peers_key) = self.sidebar.grouping.send_to_group.as_ref()?;
        let peers = self.live_peers(&key.name);
        if on != key || peers.is_empty() || peers.join(",") != *peers_key || text.trim_start().starts_with('/') { return None; }
        Some(std::iter::once(key.name.clone()).chain(peers).collect())
    }

    fn toggle_send_to_group(&mut self, cx: &mut Context<Self>) {
        self.dictation.cancel_pending_send();
        let Some(key) = self.selected_key() else { return };
        let on = self.group_targets(&key, "").is_some();
        let peers = self.live_peers(&key.name).join(",");
        self.sidebar.grouping.send_to_group = (!on && !peers.is_empty()).then_some((key, peers));
        cx.notify();
    }

    /// Chips do compositor: o do grupo (só o glifo sem grupo; com um par, o nome e o estado dele; com mais, "grupo (n)") e,
    /// agrupada, o "mandar pro grupo".
    pub(super) fn render_group_chips(&self, session: &SessionInfo, cx: &mut Context<Self>) -> Div {
        let peers = self.live_peers(&session.name);
        let label = match peers.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            many => Some(web_with("composer_grupo_n", "n", &(many.len() + 1).to_string())),
        };
        let paired_state = match peers.as_slice() { [one] => self.live(one).map(|s| s.state.clone()), _ => None };
        let tip = if peers.is_empty() { web("composer_parear_outra") } else { web_with("composer_grupo_voce", "n", &peers.join(", ")) };
        let color = if peers.is_empty() { theme::faint() } else { theme::accent() };
        let quiet = |id: &'static str| Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::faint())
            .hover(theme::hover()).active(theme::hover())).flex_shrink_0().h(px(22.)).px(px(6.)).rounded(px(6.));
        let chip = quiet("composer-group").tooltip(tip).accessibility_label(web("composer_pareamento_sessoes"))
            .child(div().flex().items_center().gap(px(5.)).text_xs().text_color(color).child(glyph(13., color))
                .when_some(label, |el, label| el.child(div().max_w(px(160.)).truncate().child(label)))
                .when_some(paired_state, |el, state| el.child(dot(&state).size(px(7.)))))
            .on_click(cx.listener(|this, _, window, cx| this.open_group_sheet(window, cx)));
        let on = self.selected_key().is_some_and(|key| self.group_targets(&key, "").is_some());
        let both = (!peers.is_empty()).then(|| {
            let tip = if on { web("composer_mandando_grupo") } else { web_with("composer_mandar_tambem", "n", &peers.join(", ")) };
            let label = web(if peers.len() == 1 { "composer_pros_dois" } else { "composer_pro_grupo" });
            quiet("composer-send-group").when(on, |el| el.bg(theme::accent_dim())).selected(on).tooltip(tip)
                .accessibility_label(web("composer_mandar_grupo"))
                .child(div().flex().items_center().gap(px(5.)).text_xs().text_color(if on { theme::accent() } else { theme::faint() })
                    .child("⇄").when(on, |el| el.child(label)))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_send_to_group(cx)))
        });
        let orq = quiet("composer-orq").tooltip(web("orqcfg_titulo")).accessibility_label(web("orqcfg_titulo"))
            .child(chrome::small_icon(IconName::Workflow, 13., theme::faint()))
            .on_click(cx.listener(|this, _, window, cx| this.open_orq_roles(window, cx)));
        div().flex_shrink_0().flex().items_center().gap(px(2.)).child(chip).children(both).child(orq)
    }

    // ── Painel ──

    pub(super) fn open_group_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(server)) = (self.selected.as_ref().map(|s| s.name.clone()), self.session_server()) else { return };
        let task = cx.new(|cx| InputState::new(window, cx).placeholder(web("par_tarefa_placeholder")));
        let g = &mut self.sidebar.grouping;
        g.seq += 1;
        let id = g.seq;
        g.sheet = Some(Sheet { session, server, id, peers_key: String::new(), picked: Vec::new(), task, adding: false, busy: false, error: None,
            contract: Remote::default(), feed: Remote::default() });
        self.load_group_sheet(window, cx);
        let hangar = cx.entity();
        let body = cx.new(|cx| SheetBody { _observe: cx.observe(&hangar, |_, _, cx| cx.notify()), hangar: hangar.downgrade() });
        let weak = hangar.downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let frame = weak.upgrade().and_then(|h| h.read(cx).sheet_frame());
            let Some((paired, count, busy, error, picked)) = frame else { return popup::dialog(dialog).w(px(560.)) };
            let title = if paired { web_with("par_grupo_titulo", "n", &count.to_string()) } else { web("par_parear_titulo") };
            let act = weak.clone();
            let footer = div().w_full().flex().items_center().justify_end().gap_2()
                .when_some(error, |el, error| el.child(div().id("group-sheet-error").role(Role::Alert).flex_1().min_w_0().text_sm()
                    .whitespace_normal().text_color(theme::danger()).child(error)))
                .child(if paired {
                    Button::new("group-sheet-leave").danger().label(web(if busy { "par_saindo" } else { "par_sair_grupo" })).loading(busy).disabled(busy)
                        .on_click(move |_, _, cx| { let _ = act.update(cx, |this, cx| this.leave_from_sheet(cx)); })
                } else {
                    let label = if busy { web("par_pareando") }
                        else if picked.is_empty() { web("par_escolha_varias") }
                        else { web_with("par_parear_nomes", "nomes", &picked.join(", ")) };
                    Button::new("group-sheet-pair").primary().label(label).loading(busy).disabled(busy || picked.is_empty())
                        .on_click(move |_, _, cx| { let _ = act.update(cx, |this, cx| this.pair_from_sheet(cx)); })
                });
            let close = weak.clone();
            // Com a chamada em voo o painel não fecha: a resposta (aviso, erro) cairia no vazio.
            popup::dialog(dialog).w(px(560.))
                .title(div().flex().items_center().gap_2().when(paired, |el| el.child(glyph(18., theme::accent()))).child(title))
                .keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .child(body.clone())
                .footer(footer)
                .on_ok(super::machines::enter_to_focused)
                .on_close(move |_, _, cx| { let _ = close.update(cx, |this, _| {
                    if this.sidebar.grouping.sheet.as_ref().is_some_and(|s| s.id == id) { this.sidebar.grouping.sheet = None; }
                }); })
        });
        cx.notify();
    }

    /// Conexão da máquina do painel; `None` se ela saiu do ar no meio (o painel mostra falha de conexão).
    fn sheet_api(&self) -> Option<Api> { self.sidebar.grouping.sheet.as_ref().and_then(|s| self.api_for(&s.server)) }

    /// Com grupo, quantos são; em voo; erro; as marcadas.
    fn sheet_frame(&self) -> Option<(bool, usize, bool, Option<String>, Vec<String>)> {
        let sheet = self.sidebar.grouping.sheet.as_ref()?;
        let peers = self.live_peers(&sheet.session);
        Some((!peers.is_empty(), peers.len() + 1, sheet.busy, sheet.error.clone(), sheet.picked.clone()))
    }

    /// (Re)começa o painel pelos membros de agora: marcadas, tarefa e erro zeram; com grupo, contrato e conversa são relidos.
    fn load_group_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api = self.sheet_api();
        let tell = self.sidebar_tell();
        let Some(name) = self.sidebar.grouping.sheet.as_ref().map(|s| s.session.clone()) else { return };
        let peers = self.live_peers(&name);
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        let task = sheet.task.clone();
        (sheet.peers_key, sheet.picked, sheet.adding, sheet.busy, sheet.error) = (peers.join(","), Vec::new(), false, false, None);
        (sheet.contract, sheet.feed) = (Remote::default(), Remote::default());
        task.update(cx, |input, cx| input.set_value("", window, cx));
        if peers.is_empty() { return; }
        let Some(api) = api else {
            sheet.contract.value = Some(Err(tr("connection_failed")));
            sheet.feed.value = Some(Err(tr("connection_failed")));
            return;
        };
        let (id, contract_seq, feed_seq) = (sheet.id, sheet.contract.start(), sheet.feed.start());
        let members: Vec<String> = std::iter::once(sheet.session.clone()).chain(peers).collect();
        self.runtime.spawn(async move {
            let contract = api.read(&members[0], &["pair", "contract"], &[], 15).await;
            tell.send(SidebarReply::Sheet(SheetReply::Contract(id, contract_seq, contract))).await;
            let histories = futures::future::join_all(members.iter().map(|name| {
                let api = api.clone();
                async move { (name.clone(), api.history(name, FEED_TAIL, None).await.map(|h| h.events.unwrap_or_default())) }
            })).await;
            tell.send(SidebarReply::Sheet(SheetReply::Feed(id, feed_seq, members, histories))).await;
        });
    }

    /// A lista nova chegou: grupo que mudou desliga o "mandar pro grupo" de vez (voltar ao mesmo grupo não o reacende) e
    /// recomeça o painel aberto.
    pub(super) fn refresh_group_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((key, peers_key)) = self.sidebar.grouping.send_to_group.as_ref()
            && self.live_peers(&key.name).join(",") != *peers_key { self.sidebar.grouping.send_to_group = None; }
        let Some(sheet) = self.sidebar.grouping.sheet.as_ref() else { return };
        if sheet.busy || self.live_peers(&sheet.session).join(",") == sheet.peers_key { return; }
        self.load_group_sheet(window, cx);
    }

    fn toggle_pick(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if let Some(ix) = sheet.picked.iter().position(|n| *n == name) { sheet.picked.remove(ix); } else { sheet.picked.push(name); }
        cx.notify();
    }

    /// Criar grupo e adicionar membro são o mesmo pedido: o servidor une os grupos.
    fn pair_from_sheet(&mut self, cx: &mut Context<Self>) {
        let api = self.sheet_api();
        let tell = self.sidebar_tell();
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if sheet.busy || sheet.picked.is_empty() { return; }
        let Some(api) = api else { sheet.error = Some(tr("connection_failed")); cx.notify(); return };
        let (id, name, picked, task) = (sheet.id, sheet.session.clone(), sheet.picked.clone(), sheet.task.read(cx).value().trim().to_owned());
        (sheet.busy, sheet.error) = (true, None);
        self.runtime.spawn(async move {
            let result = api.pair(&name, &picked, &task, false).await;
            tell.send(SidebarReply::Sheet(SheetReply::Paired(id, picked, result))).await;
        });
        cx.notify();
    }

    fn leave_from_sheet(&mut self, cx: &mut Context<Self>) {
        let api = self.sheet_api();
        let tell = self.sidebar_tell();
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if sheet.busy { return; }
        let Some(api) = api else { sheet.error = Some(tr("connection_failed")); cx.notify(); return };
        let (id, name) = (sheet.id, sheet.session.clone());
        (sheet.busy, sheet.error) = (true, None);
        self.runtime.spawn(async move { tell.send(SidebarReply::Sheet(SheetReply::Left(id, api.unpair(&name).await))).await; });
        cx.notify();
    }

    /// Membro escolhido: o painel fecha e a conversa dele abre no lugar desta.
    fn open_member(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.live(name).cloned() else { return };
        let Some(server) = self.sidebar.grouping.sheet.take().map(|s| s.server) else { return };
        window.close_dialog(cx);
        self.select_on(&servers::norm(&server), session, window, cx);
    }

    pub(super) fn receive_sheet(&mut self, reply: SheetReply, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        let mut close = false;
        match reply {
            SheetReply::Contract(id, seq, result) if id == sheet.id => {
                let value = result.map_err(|e| failed(&e, "par_contrato_falhou")).map(|value| {
                    let text = value.get("content").and_then(Value::as_str).unwrap_or_default();
                    let path = value.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
                    (!text.trim().is_empty()).then(|| (path, cx.new(|cx| TextViewState::markdown(&safe_markdown(text), cx))))
                });
                sheet.contract.finish(seq, value);
            }
            SheetReply::Feed(id, seq, members, histories) if id == sheet.id => { sheet.feed.finish(seq, Ok(build_feed(&members, histories))); }
            SheetReply::Paired(id, picked, result) if id == sheet.id => {
                sheet.busy = false;
                match result {
                    Ok(PairResult { warning: Some(warning) }) => sheet.error = Some(warning),
                    Ok(_) => close = true,
                    Err(error) => {
                        let base = web_with("par_falhou_pareamento", "nomes", &picked.join(", "));
                        sheet.error = Some(format!("{base} {}", Self::fetch_failure(&error)).trim().to_owned());
                    }
                }
            }
            SheetReply::Left(id, result) if id == sheet.id => {
                sheet.busy = false;
                match result {
                    Ok(PairResult { warning: Some(warning) }) => sheet.error = Some(warning),
                    Ok(_) => close = true,
                    Err(error) => sheet.error = Some(format!("{} {}", web("par_falhou_saida"), Self::fetch_failure(&error)).trim().to_owned()),
                }
            }
            _ => return,
        }
        if close {
            self.sidebar.grouping.sheet = None;
            window.close_dialog(cx);
        }
        cx.notify();
    }

    /// Linha de sessão marcável: caixa, estado, nome, pasta e, se já está num grupo, com quantos.
    fn pick_row(&self, session: &SessionInfo, picked: bool, aria: &str, cx: &mut Context<Self>) -> Div {
        let name = session.name.clone();
        let state = state_label(&session.state).unwrap_or_default();
        let label = web_with(aria, "nome", &name).replace("{estado}", &state);
        let (check, click) = (name.clone(), name.clone());
        let peers = session.peers();
        div().flex().items_center().gap_3().px_2().rounded(px(8.)).border_1()
            .border_color(if picked { theme::accent() } else { transparent_black() }).when(picked, |el| el.bg(theme::accent_dim()))
            .child(Checkbox::new(SharedString::from(format!("group-pick-{name}"))).checked(picked).accessibility_label(label)
                .on_change(cx.listener(move |this, _: &bool, _, cx| this.toggle_pick(check.clone(), cx))))
            .child(div().id(SharedString::from(format!("group-row-{name}"))).flex_1().min_w_0().py(px(8.)).flex().items_center().gap_3()
                .cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_pick(click.clone(), cx)))
                .child(dot(&session.state))
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(name.clone()))
                    .when_some(session.cwd.clone(), |el, cwd| el.child(div().truncate().text_xs().text_color(theme::faint()).child(cwd))))
                .when(!peers.is_empty(), |el| el.child(div().id(SharedString::from(format!("group-already-{name}"))).flex_shrink_0()
                    .flex().items_center().gap_1().text_xs().text_color(theme::muted()).child(glyph(12., theme::muted())).child(peers.len().to_string())
                    .tooltip({ let tip = web_with("par_ja_agrupada", "nomes", &peers.join(", "));
                        move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx) }))))
    }

    fn render_group_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(sheet) = self.sidebar.grouping.sheet.as_ref() else { return div() };
        let me = sheet.session.clone();
        let peers = self.live_peers(&me);
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let heading = |text: String| div().mt(px(8.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(text);
        // Candidatas a entrar: vivas, fora desta e do grupo dela.
        let candidates: Vec<SessionInfo> = self.sessions_of(&sheet.server).iter().filter(|s| s.name != me && s.state != "dead" && !peers.contains(&s.name)).cloned().collect();
        let (picked, adding, busy, task) = (sheet.picked.clone(), sheet.adding, sheet.busy, sheet.task.clone());
        let pick_list = |this: &Self, aria: &str, empty: &str, cx: &mut Context<Self>| {
            if candidates.is_empty() { return muted(web(empty)).py_2(); }
            candidates.iter().fold(div().flex().flex_col().gap(px(2.)), |list, s| list.child(this.pick_row(s, picked.contains(&s.name), aria, cx)))
        };
        if peers.is_empty() {
            return div().flex().flex_col().gap_3()
                .child(muted(web("par_passam_hint")))
                .child(pick_list(self, "par_parear_aria", "forward_nenhuma_viva", cx))
                .child(Input::new(&task).disabled(busy).aria_label(web("par_tarefa_placeholder")));
        }
        let members = peers.iter().fold(div().flex().flex_col().gap(px(2.)), |list, peer| {
            let state = self.live(peer).map(|s| s.state.clone());
            let open = peer.clone();
            let tip = web_with("par_abrir_conversa_de", "nome", peer);
            list.child(div().flex().items_center().gap_3().px_2().py(px(6.)).rounded(px(8.))
                .children(state.as_deref().map(dot))
                .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(peer.clone()))
                .children(state.as_deref().and_then(state_label).map(|label| div().flex_shrink_0().text_xs().text_color(theme::muted()).child(label)))
                // Membro de outra máquina não está na lista deste servidor: não há conversa aqui para abrir.
                .when(state.is_some(), |el| el.child(chrome::icon_button(SharedString::from(format!("group-open-{peer}")), IconName::MessageCircle, tip, cx)
                    .on_click(cx.listener(move |this, _, window, cx| this.open_member(&open, window, cx))))))
        });
        let add = if adding {
            let label = if busy { web("par_adicionando") } else if picked.is_empty() { web("par_escolha_sessoes") }
                else { web_with("par_adicionar_nomes", "nomes", &picked.join(", ")) };
            div().flex().flex_col().gap_2()
                .child(pick_list(self, "par_adicionar_aria", "par_vazio_fora_grupo", cx))
                .child(div().child(Button::new("group-sheet-add").primary().small().label(label).loading(busy).disabled(busy || picked.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.pair_from_sheet(cx)))))
        } else {
            div().child(Button::new("group-sheet-adding").ghost().small().label(web("par_adicionar_sessao")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(sheet) = this.sidebar.grouping.sheet.as_mut() { sheet.adding = true; }
                    cx.notify();
                })))
        };
        let warn = |text: String| muted(format!("⚠ {text}"));
        // Contrato: só aparece com texto; grupo sem contrato é normal e não diz nada, mas a busca que falhou diz.
        let contract = match sheet.contract.value.as_ref() {
            Some(Ok(Some((path, view)))) => Some(div().flex().flex_col().gap_2()
                .child(heading(web("par_contrato_titulo")))
                .child(div().p_3().rounded(px(8.)).bg(theme::inset()).text_sm().child(TextView::new(view).selectable(true).scrollable(false)))
                .child(div().truncate().font_family(theme::MONO).text_xs().text_color(theme::faint()).child(path.clone()))),
            Some(Err(error)) => Some(warn(error.clone())),
            _ => None,
        };
        let now = chrono::Local::now().timestamp() as f64;
        let feed = match (&sheet.feed.value, sheet.feed.loading) {
            (_, true) | (None, _) => muted(web("comum_carregando")).into_any_element(),
            (Some(Err(error)), _) => warn(error.clone()).into_any_element(),
            (Some(Ok(feed)), _) => div().flex().flex_col().gap_2()
                .when(!feed.missing.is_empty(), |el| el.child(warn(web_with("par_sem_historico", "nomes", &feed.missing.join(", ")))))
                .when(feed.items.is_empty(), |el| el.child(muted(web("par_vazio_trocas"))))
                .children(feed.items.iter().map(|item| {
                    let out = item.from == me;
                    let when = (item.ts > 0.).then(|| format!(" · {}", super::side::ago(now - item.ts))).unwrap_or_default();
                    div().flex().flex_col().gap(px(2.)).px_3().py_2().rounded(px(8.)).bg(if out { theme::accent_dim() } else { theme::inset() })
                        .child(div().text_xs().text_color(theme::faint()).child(format!("{} → {}{when}", item.from, item.to)))
                        .child(div().text_sm().whitespace_normal().child(item.text.clone()))
                }))
                .into_any_element(),
        };
        div().flex().flex_col().gap_3()
            .child(muted(web("par_membros_hint")))
            .child(members)
            .child(add)
            .children(contract)
            .child(div().flex().flex_col().gap_2().child(heading(web("par_conversa_titulo"))).child(feed))
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{ChatEvent, Delivery, Failure, FeedItem, build_feed, group_delivery};

    fn msg(text: &str, ts: f64) -> ChatEvent { ChatEvent { kind: "user_msg".into(), text: Some(text.into()), ts: Some(ts), ..Default::default() } }

    #[test]
    fn feed_keeps_only_messages_between_members_in_time_order() {
        let members = ["a".to_owned(), "b".to_owned()];
        let feed = build_feed(&members, vec![
            ("a".into(), Ok(vec![msg("[de: b] segundo", 2.), msg("[de: estranha] fora", 3.), msg("[de: a] eco", 4.), msg("sem prefixo", 5.)])),
            ("b".into(), Ok(vec![msg("[de: a] [vigia] primeiro", 1.)])),
            ("c".into(), Err(Failure::local("x"))),
        ]);
        assert_eq!(feed.items, [
            FeedItem { from: "a".into(), to: "b".into(), text: "[vigia] primeiro".into(), ts: 1. },
            FeedItem { from: "b".into(), to: "a".into(), text: "segundo".into(), ts: 2. },
        ]);
        assert_eq!(feed.missing, ["c"], "histórico que falhou não é conversa vazia");
    }

    #[test]
    fn group_send_fails_when_any_member_did_not_get_it() {
        let ok = |delivered| Delivery { ok: true, delivered };
        let sent = group_delivery("a", vec![("a".into(), ok(true), None), ("b".into(), ok(false), None)]).expect("todos receberam");
        assert!(sent.delivered, "a entrega é a desta sessão");
        let error = group_delivery("a", vec![("a".into(), ok(true), None), ("b".into(), Delivery { ok: false, delivered: false }, Some("morta".into()))])
            .expect_err("b não recebeu");
        assert!(error.detail.contains('a') && error.detail.ends_with("b (morta)"), "{}", error.detail);
    }
}
