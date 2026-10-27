//! Grupo de trabalho a partir do compositor, como o web (chip do par e "mandar pro grupo" do `Composer`, `PairSheet`): o chip
//! abre o painel com os membros e o estado de cada um, abrir a conversa de um membro, adicionar sessões, o contrato
//! compartilhado em markdown, a conversa do grupo e sair. Sem grupo, o painel escolhe sessões e a tarefa para formar um.
use super::*;
use super::activity::web;
use super::device::Remote;
use super::grouping::{Grant, IdSlot, Refusal, failed, full_grants, glyph, identity_of, identity_settled, pair_and_grant, qualified, web_with};
use super::sidebar::{SidebarReply, Target};
use crate::api::dto::{PairExternalEntry, PairResult};
use gpui_kit::component::{WindowExt, checkbox::Checkbox};

/// Cauda do histórico de cada membro lida para a conversa do grupo.
// ponytail: o web lê o histórico inteiro; recado mais velho que esta cauda não aparece.
const FEED_TAIL: usize = 1000;
/// A conversa mostra só os últimos recados, como o web; o resto vive no chat de cada um.
const FEED_SHOWN: usize = 40;
/// Largura do painel com grupo (membros e conversa lado a lado); sem grupo ele fica estreito.
const SHEET_WIDE: f32 = 1040.;

#[derive(Debug, PartialEq)]
struct FeedItem { from: String, to: String, text: String, ts: f64 }

/// `missing`: membros cujo histórico falhou, que não é o mesmo que conversa vazia.
struct Feed { items: Vec<FeedItem>, missing: Vec<String> }

pub(super) struct Sheet {
    session: String,
    /// Chave da máquina da sessão (`servers::norm`): pode não ser a ativa nem a aberta.
    server: String,
    id: u64,
    /// Membros de quando carregou: grupo que muda com o painel aberto recomeça tudo, como o web.
    peers_key: String,
    /// Marcadas para entrar, de qualquer máquina.
    picked: Vec<Target>,
    task: Entity<InputState>,
    adding: bool,
    /// Membro (endereço da lista) com o "Retirar" aberto na linha, à espera da confirmação.
    removing: Option<String>,
    /// Recados longos abertos por inteiro, por remetente e hora.
    expanded: HashSet<String>,
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
    Removed(u64, String, Result<PairResult, Failure>),
}

/// O corpo do painel, redesenhado quando o `Hangar` muda (lista de sessões, respostas).
struct SheetBody { hangar: WeakEntity<Hangar>, _observe: Subscription }

impl Render for SheetBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = window.viewport_size();
        // O diálogo começa a um décimo da altura; título, rodapé e margens levam ~190.
        let fit = (view.width >= px(SHEET_WIDE + 64.)).then(|| (view.height * 0.9 - px(190.)).clamp(px(320.), px(680.)));
        self.hangar.update(cx, |hangar, cx| hangar.render_group_sheet(fit, cx)).unwrap_or_else(|_| div())
    }
}

/// `rotuloEstado` do web.
fn state_label(state: &str) -> Option<String> {
    Some(web(match state {
        "working" => "estado_em_execucao", "idle" => "estado_pronto", "awaiting_input" => "estado_aguardando", "dead" => "estado_encerrado",
        _ => return None,
    }))
}

/// Quem o painel lista: esta sessão primeiro, marcada, e os pares; o título conta todos.
fn sheet_members(me: &str, peers: &[String]) -> Vec<(String, bool)> {
    std::iter::once((me.to_owned(), true)).chain(peers.iter().map(|p| (p.clone(), false))).collect()
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

/// Recado mais fácil de ler: a palavra de protocolo do começo (`ANDAMENTO`, `RESPOSTA`) vira etiqueta, a pasta do usuário
/// vira `~` e cada item numerado `(n)` começa numa linha.
fn tidy_message(text: &str, home: Option<&str>) -> (Option<String>, String) {
    let text = text.trim();
    let (tag, rest) = match text.split_once(' ') {
        Some((word, rest)) if word.chars().count() >= 4 && word.chars().all(|c| c.is_uppercase() || c == '_') => (Some(word.to_owned()), rest.trim_start()),
        _ => (None, text),
    };
    // Só com o separador junto: `/home/eu` não pode comer o começo de `/home/euler`.
    let rest = match home.filter(|h| h.len() > 1) {
        Some(home) => rest.replace(&format!("{home}/"), "~/").replace(&format!("{home}\\"), "~\\"),
        None => rest.to_owned(),
    };
    let mut body = String::with_capacity(rest.len());
    let mut items = 0;
    for (ix, ch) in rest.char_indices() {
        // Só a sequência (1), (2)… quebra linha: "(ver item 2)" fica onde está.
        let next = (items + 1).to_string();
        if ch == '(' && rest[ix + 1..].starts_with(&format!("{next})")) {
            items += 1;
            while body.ends_with(' ') { body.pop(); }
            if !body.is_empty() { body.push('\n'); }
        }
        body.push(ch);
    }
    (tag, body)
}

/// Onde um membro do painel está: numa máquina conectada aqui, numa sem conexão aqui (o identificador dela), ou sem como
/// saber ainda (par de fora, identificadores em leitura).
enum MemberAt { Here(Target), Missing(String), Unknown }

/// Envio ao grupo: o `/broadcast` responde 200 com o resultado por sessão, e membro que não recebeu vira recusa dizendo quem
/// recebeu e quem não, como o web; sem isso a falha de um membro passaria calada.
pub(super) fn group_delivery(me: &str, results: Vec<(String, Delivery, Option<String>)>) -> Result<Delivery, Failure> {
    let names = |list: &[&(String, Delivery, Option<String>)]| list.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>().join(", ");
    let (reached, failed): (Vec<_>, Vec<_>) = results.iter().partition(|(_, delivery, _)| delivery.ok);
    let Some((_, _, reason)) = failed.first() else {
        let delivered = results.iter().any(|(name, delivery, _)| name == me && delivery.delivered);
        return Ok(Delivery { ok: true, delivered, ..Default::default() });
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

    /// O chip do compositor: o painel da sessão aberta.
    pub(super) fn open_group_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.selected_target() { self.open_group_sheet_on(target, window, cx); }
    }

    /// O painel do grupo de qualquer sessão da barra (o selo da linha), sem abrir a conversa dela.
    pub(super) fn open_group_sheet_on(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Target { server, name: session } = target;
        let task = cx.new(|cx| InputState::new(window, cx).placeholder(web("par_tarefa_placeholder")));
        let g = &mut self.sidebar.grouping;
        g.seq += 1;
        let id = g.seq;
        g.sheet = Some(Sheet { session, server, id, peers_key: String::new(), picked: Vec::new(), task, adding: false, removing: None, expanded: HashSet::new(), busy: false, error: None,
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
            popup::dialog(dialog).w(px(if paired { SHEET_WIDE } else { 560. }))
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
    fn sheet_api(&self) -> Option<Api> { self.sidebar.grouping.sheet.as_ref().and_then(|s| self.machine_api(&s.server)) }

    /// A sessão do painel, na lista da máquina dela.
    fn sheet_live(&self) -> Option<&SessionInfo> {
        let sheet = self.sidebar.grouping.sheet.as_ref()?;
        self.target_session(&Target::new(&sheet.server, &sheet.session))
    }

    fn sheet_peers(&self) -> Vec<String> { self.sheet_live().map(|s| s.peers().to_vec()).unwrap_or_default() }

    /// Um membro pelo endereço que a lista dá: nome sozinho é da máquina do painel; `identificador::nome`, da máquina com
    /// esse identificador.
    fn member_at(&self, address: &str, ids: &HashMap<String, String>) -> MemberAt {
        let Some(sheet) = self.sidebar.grouping.sheet.as_ref() else { return MemberAt::Unknown };
        let Some((id, name)) = address.split_once("::") else { return MemberAt::Here(Target::new(&sheet.server, address)) };
        let machines = self.group_machines();
        if let Some(key) = machines.iter().find(|k| identity_of(k, ids) == Some(id)) { return MemberAt::Here(Target::new(key, name)); }
        let outside = self.sheet_live().is_some_and(|s| s.outside_pairs().iter().any(|p| p.alias == id && p.session == name));
        // Máquina conectada ainda sem identificador (em leitura ou falhou) pode ser a deste membro: não se diz que ela está fora.
        let unsettled = machines.iter().any(|k| !identity_settled(self.identity_slot(k)));
        if outside || unsettled { MemberAt::Unknown } else { MemberAt::Missing(id.to_owned()) }
    }

    /// Com grupo, quantos são; em voo; erro; as marcadas (pelo nome que o pedido leva).
    fn sheet_frame(&self) -> Option<(bool, usize, bool, Option<String>, Vec<String>)> {
        let sheet = self.sidebar.grouping.sheet.as_ref()?;
        let peers = self.sheet_peers();
        let ids = self.identities();
        let picked = sheet.picked.iter().map(|t| qualified(&t.server, &sheet.server, &t.name, &ids).unwrap_or_else(|| t.name.clone())).collect();
        Some((!peers.is_empty(), peers.len() + 1, sheet.busy, sheet.error.clone(), picked))
    }

    /// (Re)começa o painel pelos membros de agora: marcadas, tarefa e erro zeram; com grupo, contrato e conversa são relidos.
    fn load_group_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api = self.sheet_api();
        let tell = self.sidebar_tell();
        let peers = self.sheet_peers();
        // Histórico de cada membro na máquina dele; o de máquina sem conexão aqui entra como "sem histórico".
        let ids = self.identities();
        let sources: Vec<(String, Option<(Api, String)>)> = peers.iter().map(|p| (p.clone(), match self.member_at(p, &ids) {
            MemberAt::Here(t) => self.machine_api(&t.server).map(|api| (api, t.name)),
            _ => None,
        })).collect();
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        let task = sheet.task.clone();
        (sheet.peers_key, sheet.picked, sheet.adding, sheet.removing, sheet.busy, sheet.error) = (peers.join(","), Vec::new(), false, None, false, None);
        (sheet.contract, sheet.feed) = (Remote::default(), Remote::default());
        task.update(cx, |input, cx| input.set_value("", window, cx));
        if peers.is_empty() { return; }
        let Some(api) = api else {
            sheet.contract.value = Some(Err(tr("connection_failed")));
            sheet.feed.value = Some(Err(tr("connection_failed")));
            return;
        };
        let (id, contract_seq, feed_seq) = (sheet.id, sheet.contract.start(), sheet.feed.start());
        let me = sheet.session.clone();
        let members: Vec<String> = std::iter::once(me.clone()).chain(peers).collect();
        let sources: Vec<(String, Option<(Api, String)>)> = std::iter::once((me.clone(), Some((api.clone(), me)))).chain(sources).collect();
        self.runtime.spawn(async move {
            let contract = api.read(&members[0], &["pair", "contract"], &[], 15).await;
            tell.send(SidebarReply::Sheet(SheetReply::Contract(id, contract_seq, contract))).await;
            let histories = futures::future::join_all(sources.into_iter().map(|(label, source)| async move {
                let result = match source {
                    Some((api, name)) => api.history(&name, FEED_TAIL, None).await.map(|h| h.events.unwrap_or_default()),
                    None => Err(Failure::local(tr("connection_failed"))),
                };
                (label, result)
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
        if sheet.busy || self.sheet_peers().join(",") == sheet.peers_key { return; }
        self.load_group_sheet(window, cx);
    }

    fn toggle_pick(&mut self, target: Target, cx: &mut Context<Self>) {
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if let Some(ix) = sheet.picked.iter().position(|t| *t == target) { sheet.picked.remove(ix); } else { sheet.picked.push(target); }
        cx.notify();
    }

    /// As marcadas que entram num grupo com par de fora de acesso completo: ficam abertas para ele ao confirmar. Só as da
    /// máquina do painel: o par de fora só ganha acesso na máquina onde foi feito.
    fn sheet_grants(&self) -> Vec<Grant> {
        let Some(sheet) = self.sidebar.grouping.sheet.as_ref() else { return Vec::new() };
        let Some(group) = self.sheet_live() else { return Vec::new() };
        sheet.picked.iter().filter(|t| t.server == sheet.server).filter_map(|t| self.target_session(t)).flat_map(|side| full_grants(group, side)).collect()
    }

    /// Criar grupo e adicionar membro são o mesmo pedido: o servidor une os grupos.
    fn pair_from_sheet(&mut self, cx: &mut Context<Self>) {
        let api = self.sheet_api();
        let tell = self.sidebar_tell();
        let grants = self.sheet_grants();
        let ids = self.identities();
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if sheet.busy || sheet.picked.is_empty() { return; }
        let Some(api) = api else { sheet.error = Some(tr("connection_failed")); cx.notify(); return };
        // O pedido vai ao servidor desta sessão; a marcada de outra máquina vai pelo identificador dela.
        let Some(picked) = sheet.picked.iter().map(|t| qualified(&t.server, &sheet.server, &t.name, &ids)).collect::<Option<Vec<String>>>() else {
            sheet.error = Some(Refusal::NoIdentity.text());
            cx.notify();
            return;
        };
        let (id, name, task) = (sheet.id, sheet.session.clone(), sheet.task.read(cx).value().trim().to_owned());
        (sheet.busy, sheet.error) = (true, None);
        self.runtime.spawn(async move {
            let result = pair_and_grant(api, name, picked.clone(), task, false, grants).await;
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

    /// Retira outro membro: é a saída dele, pedida à máquina onde ele está; os demais seguem juntos.
    fn remove_member(&mut self, address: String, cx: &mut Context<Self>) {
        let at = self.member_at(&address, &self.identities());
        let tell = self.sidebar_tell();
        let api = match &at { MemberAt::Here(target) => self.machine_api(&target.server), _ => None };
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if sheet.busy { return; }
        // A máquina do membro saiu do ar entre abrir o "Retirar" e confirmar.
        let MemberAt::Here(target) = at else { sheet.error = Some(Refusal::NoIdentity.text()); cx.notify(); return };
        let Some(api) = api else { sheet.error = Some(tr("connection_failed")); cx.notify(); return };
        (sheet.busy, sheet.error) = (true, None);
        let id = sheet.id;
        self.runtime.spawn(async move {
            let result = api.unpair(&target.name).await;
            tell.send(SidebarReply::Sheet(SheetReply::Removed(id, address, result))).await;
        });
        cx.notify();
    }

    fn ask_remove(&mut self, address: Option<String>, cx: &mut Context<Self>) {
        let Some(sheet) = self.sidebar.grouping.sheet.as_mut() else { return };
        if sheet.busy { return; }
        (sheet.removing, sheet.error) = (address, None);
        cx.notify();
    }

    /// A aba de grupo desta sessão: a chave e a principal, a mesma que a barra de abas põe na frente.
    fn sheet_lead(&self) -> Option<(String, Target)> {
        let sheet = self.sidebar.grouping.sheet.as_ref()?;
        let me = Target::new(&sheet.server, &sheet.session);
        let groups = self.tab_groups();
        let group = groups.iter().find(|g| g.members.len() > 1 && g.members.contains(&me))?;
        Some((group.key.clone()?, group.members[0].clone()))
    }

    /// Membro escolhido: o painel fecha e a conversa dele abre no lugar desta, na máquina dele.
    fn open_member(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.target_session(target).cloned() else { return };
        self.sidebar.grouping.sheet = None;
        window.close_dialog(cx);
        self.select_on(&target.server, session, window, cx);
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
            SheetReply::Removed(id, address, result) if id == sheet.id => {
                sheet.busy = false;
                // Na falha a confirmação fica aberta, na linha do membro, para tentar de novo.
                if result.is_ok() { sheet.removing = None; }
                match result {
                    Ok(PairResult { warning: Some(warning) }) => sheet.error = Some(warning),
                    Ok(_) => {}
                    Err(error) => {
                        let base = tr("group_sheet_remove_failed").replace("{name}", &address);
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
    fn pick_row(&self, target: &Target, session: &SessionInfo, picked: bool, aria: &str, cx: &mut Context<Self>) -> Div {
        let (name, key) = (session.name.clone(), target.id());
        let state = state_label(&session.state).unwrap_or_default();
        let label = web_with(aria, "nome", &name).replace("{estado}", &state);
        let (check, click) = (target.clone(), target.clone());
        let peers = session.peers();
        div().flex().items_center().gap_3().px_2().rounded(px(8.)).border_1()
            .border_color(if picked { theme::accent() } else { transparent_black() }).when(picked, |el| el.bg(theme::accent_dim()))
            .child(Checkbox::new(SharedString::from(format!("group-pick-{key}"))).checked(picked).accessibility_label(label)
                .on_change(cx.listener(move |this, _: &bool, _, cx| this.toggle_pick(check.clone(), cx))))
            .child(div().id(SharedString::from(format!("group-row-{key}"))).flex_1().min_w_0().py(px(8.)).flex().items_center().gap_3()
                .cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_pick(click.clone(), cx)))
                .child(dot(&session.state))
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(name.clone()))
                    .when_some(session.cwd.clone(), |el, cwd| el.child(div().truncate().text_xs().text_color(theme::faint()).child(cwd))))
                .when(!peers.is_empty(), |el| el.child(div().id(SharedString::from(format!("group-already-{key}"))).flex_shrink_0()
                    .flex().items_center().gap_1().text_xs().text_color(theme::muted()).child(glyph(12., theme::muted())).child(peers.len().to_string())
                    .tooltip({ let tip = web_with("par_ja_agrupada", "nomes", &peers.join(", "));
                        move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx) }))))
    }

    /// `fit`: altura do corpo quando o painel cabe em duas colunas; `None` empilha tudo numa coluna.
    fn render_group_sheet(&mut self, fit: Option<Pixels>, cx: &mut Context<Self>) -> Div {
        let Some(sheet) = self.sidebar.grouping.sheet.as_ref() else { return div() };
        let me = sheet.session.clone();
        let here = sheet.server.clone();
        let peers = self.sheet_peers();
        let ids = self.identities();
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let heading = |text: String| div().mt(px(8.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(text);
        let machines = self.group_machines();
        let multi = machines.len() > 1;
        // As mesmas candidatas do arrastar e do "Agrupar com…".
        let candidates: Vec<(Target, SessionInfo)> = self.group_candidates(&Target::new(&here, &me), cx).into_iter()
            .filter_map(|(t, _)| self.target_session(&t).cloned().map(|s| (t, s))).collect();
        // Máquinas cujo identificador ainda está em leitura ou faltou: as sessões delas não aparecem, e o painel diz por quê.
        let notes: Vec<String> = machines.iter().filter(|_| multi).filter_map(|k| {
            let label = self.machine_label(k, cx);
            match self.identity_slot(k) {
                Some(IdSlot::Known(_)) => None,
                None | Some(IdSlot::Loading) => Some(format!("{label}: {}", web("comum_carregando"))),
                _ => Some(format!("⚠ {label}: {}", Refusal::NoIdentity.text())),
            }
        }).collect();
        let (picked, adding, busy, task) = (sheet.picked.clone(), sheet.adding, sheet.busy, sheet.task.clone());
        let picked_names = picked.iter().map(|t| qualified(&t.server, &here, &t.name, &ids).unwrap_or_else(|| t.name.clone())).collect::<Vec<_>>().join(", ");
        let pick_list = |this: &Self, aria: &str, empty: &str, cx: &mut Context<Self>| {
            let list = div().flex().flex_col().gap(px(2.)).children(notes.iter().map(|n| muted(n.clone()).text_xs()));
            if candidates.is_empty() { return list.child(muted(web(empty)).py_2()); }
            let mut shown: Option<&str> = None;
            candidates.iter().fold(list, |list, (t, s)| {
                // Com mais de uma máquina, as candidatas vêm separadas pelo nome da máquina.
                let title = (multi && shown != Some(t.server.as_str())).then(|| div().pt(px(6.)).px_2().text_xs().text_color(theme::faint())
                    .child(this.machine_label(&t.server, cx)));
                shown = Some(t.server.as_str());
                list.children(title).child(this.pick_row(t, s, picked.contains(t), aria, cx))
            })
        };
        if peers.is_empty() {
            return div().flex().flex_col().gap_3()
                .child(muted(web("par_passam_hint")))
                .child(pick_list(self, "par_parear_aria", "forward_nenhuma_viva", cx))
                .child(Input::new(&task).disabled(busy).aria_label(web("par_tarefa_placeholder")));
        }
        let lead = self.sheet_lead();
        let removing = sheet.removing.clone();
        let tag = |text: String, color: Hsla| div().flex_shrink_0().px(px(6.)).py(px(1.)).rounded(px(4.)).text_xs().text_color(color).bg(color.opacity(0.12)).child(text);
        let mut rows: Vec<(String, bool, MemberAt)> = sheet_members(&me, &peers).into_iter().map(|(peer, this)| {
            let at = if this { MemberAt::Here(Target::new(&here, &me)) } else { self.member_at(&peer, &ids) };
            (peer, this, at)
        }).collect();
        let is_lead = |at: &MemberAt| matches!((at, &lead), (MemberAt::Here(t), Some((_, l))) if t == l);
        // A principal vem primeiro, como na aba do grupo.
        rows.sort_by_key(|(_, _, at)| !is_lead(at));
        let count = rows.len();
        let members = rows.into_iter().enumerate().fold(div().flex().flex_col().rounded(px(10.)).border_1().border_color(theme::border()), |list, (i, (peer, this, at))| {
            let principal = is_lead(&at);
            let live = match &at { MemberAt::Here(t) => self.target_session(t).map(|s| (t.clone(), s.state.clone(), s.cwd.clone())), _ => None };
            let (short, machine_id) = match peer.split_once("::") { Some((id, name)) => (name.to_owned(), Some(id.to_owned())), None => (peer.clone(), None) };
            let machine = match &at {
                MemberAt::Here(t) if t.server != here => Some(self.machine_label(&t.server, cx)),
                MemberAt::Here(_) => None,
                _ => machine_id,
            };
            let detail = match (&at, &live) {
                (MemberAt::Missing(id), _) => Some(tr_shared("grupo_membro_maquina_desconectada", &[("maquina", id)])),
                (_, Some((_, state, cwd))) => Some([state_label(state), cwd.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ")),
                _ => None,
            };
            let key = SharedString::from(format!("{i}-{peer}"));
            let confirming = removing.as_deref() == Some(peer.as_str());
            let mut actions = div().flex_shrink_0().flex().items_center().gap(px(2.));
            if let (Some((t, ..)), Some((lead_key, _)), false) = (&live, &lead, principal) {
                let (t, lead_key) = (t.clone(), lead_key.clone());
                actions = actions.child(chrome::icon_button(SharedString::from(format!("group-lead-{key}")), IconName::Star, tr("group_make_lead"), cx)
                    .disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.set_group_lead(lead_key.clone(), Some(t.clone()), cx))));
            }
            // Só o membro numa máquina conectada aqui tem conversa para abrir ou saída para pedir; esta sessão sai pelo rodapé.
            if let Some((open, ..)) = live.as_ref().filter(|_| !this) {
                let (open, address) = (open.clone(), peer.clone());
                actions = actions
                    .child(chrome::icon_button(SharedString::from(format!("group-open-{key}")), IconName::MessageCircle, web_with("par_abrir_conversa_de", "nome", &peer), cx)
                        .on_click(cx.listener(move |this, _, window, cx| this.open_member(&open, window, cx))))
                    .child(chrome::icon_button(SharedString::from(format!("group-remove-{key}")), IconName::Close, tr("group_sheet_remove"), cx)
                        .disabled(busy || confirming).on_click(cx.listener(move |this, _, _, cx| this.ask_remove(Some(address.clone()), cx))));
            }
            let confirm = confirming.then(|| {
                let target = live.as_ref().map(|(t, ..)| t.clone());
                let anchors = target.as_ref().map(|t| self.anchored_owners(t)).unwrap_or_default();
                let address = peer.clone();
                div().mt(px(8.)).p_2().rounded(px(8.)).bg(theme::danger().opacity(0.08)).flex().flex_col().gap_2()
                    .child(div().text_sm().whitespace_normal().child(tr("group_sheet_remove_confirm").replace("{name}", &short)))
                    .children(anchors.iter().map(|dono| div().text_xs().whitespace_normal().text_color(theme::warning_text())
                        .child(format!("⚠ {}", tr_shared("pair_undo_anchor_body", &[("dono", dono)])))))
                    .child(div().flex().justify_end().gap_2()
                        .child(Button::new(SharedString::from(format!("group-remove-cancel-{key}"))).small().label(tr("cancel")).disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.ask_remove(None, cx))))
                        .child(Button::new(SharedString::from(format!("group-remove-ok-{key}"))).small().danger()
                            .label(tr(if busy { "group_sheet_removing" } else { "group_sheet_remove_ok" })).loading(busy).disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_member(address.clone(), cx)))))
            });
            let lead_tag = principal.then(|| div().id(SharedString::from(format!("group-lead-tag-{key}"))).flex_shrink_0().flex().items_center().gap(px(3.))
                .px(px(6.)).py(px(1.)).rounded(px(4.)).text_xs().text_color(theme::accent_text()).bg(theme::accent_dim())
                .child(chrome::small_icon(IconName::Star, 10., theme::accent_text())).child(tr("group_sheet_lead"))
                .tooltip({ let tip = tr("group_sheet_lead_tip"); move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx) }));
            list.child(div().px_3().py(px(10.)).when(i + 1 < count, |el| el.border_b_1().border_color(theme::border()))
                .when(principal, |el| el.bg(theme::accent_dim().opacity(0.35)))
                .child(div().flex().items_center().gap_3()
                    .child(live.as_ref().map_or_else(|| dot("dead").opacity(0.4), |(_, state, _)| dot(state)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().flex().items_center().gap_2().min_w_0()
                            .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(short.clone()))
                            .children(lead_tag)
                            .when(this, |el| el.child(tag(tr_shared("worktree_esta_sessao", &[]), theme::muted())))
                            .children(machine.map(|m| tag(m, theme::faint()))))
                        .children(detail.filter(|d| !d.is_empty()).map(|text| div().truncate().text_xs().text_color(theme::faint()).child(text))))
                    .child(actions))
                .children(confirm))
        });
        // Acesso completo pendente de cada membro daqui, com o "Liberar" ao lado.
        let server = here.clone();
        let mut pending: Vec<(String, PairExternalEntry)> = Vec::new();
        for s in std::iter::once(&me).chain(peers.iter().filter(|p| !p.contains("::"))).filter_map(|name| self.target_session(&Target::new(&server, name))) {
            pending.extend(s.outside_pairs().iter().filter(|p| p.pending).map(|p| (s.name.clone(), p.clone())));
        }
        let pending = pending.into_iter().enumerate().fold(div().flex().flex_col().gap(px(2.)), |list, (i, (name, p))| {
            let (target, alias) = (Target::new(&server, &name), p.alias.clone());
            list.child(div().flex().items_center().gap_3().px_2().py(px(6.)).rounded(px(8.)).bg(theme::warning().opacity(0.08))
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(name))
                    .child(div().text_xs().text_color(theme::warning()).child(tr_shared("pair_full_pending", &[]))))
                .child(Button::new(("group-release", i)).small().label(tr_shared("pair_release", &[("dono", &p.owner)]))
                    .on_click(cx.listener(move |this, _, window, cx| this.release_pair(target.clone(), alias.clone(), window, cx)))))
        });
        let outside = self.sheet_live().is_some_and(|s| !s.outside_pairs().is_empty());
        let notices: Vec<String> = self.sheet_grants().iter().map(|g| tr_shared("pair_full_confirm", &[("sessao", &g.session), ("dono", &g.owner)])).collect();
        let (add, add_panel) = if adding {
            let label = if busy { web("par_adicionando") } else if picked.is_empty() { web("par_escolha_sessoes") }
                else { web_with("par_adicionar_nomes", "nomes", &picked_names) };
            (None, Some(div().flex().flex_col().gap_2()
                .child(pick_list(self, "par_adicionar_aria", "par_vazio_fora_grupo", cx))
                // Confirmar o "Adicionar" com esta frase à vista é o que libera o acesso completo.
                .when(!notices.is_empty(), |el| el.child(div().p_2().rounded_md().bg(theme::warning().opacity(0.12)).flex().flex_col().gap(px(4.))
                    .text_sm().whitespace_normal().children(notices)))
                .child(div().child(Button::new("group-sheet-add").primary().small().label(label).loading(busy).disabled(busy || picked.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.pair_from_sheet(cx)))))))
        } else {
            (Some(Button::new("group-sheet-adding").ghost().small().label(web("par_adicionar_sessao")).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(sheet) = this.sidebar.grouping.sheet.as_mut() { sheet.adding = true; }
                    cx.notify();
                }))), None)
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
        let home = super::tree::home_dir().map(|h| h.to_string_lossy().into_owned());
        let feed = match (&sheet.feed.value, sheet.feed.loading) {
            (_, true) | (None, _) => muted(web("comum_carregando")).into_any_element(),
            (Some(Err(error)), _) => warn(error.clone()).into_any_element(),
            (Some(Ok(feed)), _) => div().flex().flex_col().gap_2().when(fit.is_some(), |el| el.flex_1().min_h_0())
                .when(!feed.missing.is_empty(), |el| el.child(warn(web_with("par_sem_historico", "nomes", &feed.missing.join(", ")))))
                .when(feed.items.is_empty(), |el| el.child(muted(web("par_vazio_trocas"))))
                // A mais nova no topo, numa área com rolagem própria: membros e ações ficam à vista.
                .when(!feed.items.is_empty(), |el| el.child(div().id("group-feed")
                    .map(|el| if fit.is_some() { el.flex_1().min_h_0() } else { el.max_h(px(420.)) })
                    .overflow_y_scroll().flex().flex_col().gap(px(10.))
                    .children(feed.items.iter().rev().enumerate().map(|(ix, item)| {
                        let out = item.from == me;
                        let short = |a: &str| a.split_once("::").map_or(a, |(_, n)| n).to_owned();
                        let when = (item.ts > 0.).then(|| super::side::ago(now - item.ts));
                        let (tag, body) = tidy_message(&item.text, home.as_deref());
                        // Muito recado vem sem hora: o texto entra na chave para dois recados não dividirem o mesmo id.
                        let key = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new();
                            (&item.from, &item.to, &item.text, item.ts.to_bits()).hash(&mut h); format!("{:x}-{ix}", h.finish()) };
                        let open = sheet.expanded.contains(&key);
                        // ponytail: "longo" por tamanho e linhas, sem medir o texto desenhado.
                        let long = body.chars().count() > 360 || body.lines().count() > 6;
                        let accent = if out { theme::accent() } else { theme::border_strong() };
                        let id = SharedString::from(format!("group-feed-more-{key}"));
                        div().flex().flex_col().gap(px(6.)).pl(px(12.)).pr_3().py(px(10.)).rounded(px(8.)).bg(theme::inset())
                            .border_l_2().border_color(accent)
                            .child(div().flex().items_center().gap_2().text_xs().min_w_0()
                                .child(div().flex_shrink_0().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(short(&item.from)))
                                .child(div().flex_shrink_0().text_color(theme::faint()).child("→"))
                                .child(div().min_w_0().truncate().text_color(theme::muted()).child(short(&item.to)))
                                .children(when.map(|w| div().ml_auto().flex_shrink_0().text_color(theme::faint()).child(w))))
                            .child(div().flex().flex_col().gap(px(4.))
                                .children(tag.map(|t| div().flex().child(div().px(px(6.)).py(px(1.)).rounded(px(4.)).bg(theme::accent_dim())
                                    .text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::accent_text()).child(t))))
                                .child(div().text_sm().line_height(px(21.)).text_color(theme::body_text()).whitespace_normal()
                                    .when(long && !open, |el| el.line_clamp(5).text_ellipsis())
                                    .children(body.lines().map(|line| div().child(line.to_owned())))))
                            .when(long, |el| el.child(div().id(id).flex().cursor_pointer().text_xs().font_weight(FontWeight::MEDIUM)
                                .text_color(theme::accent_text()).hover(|s| s.underline())
                                .child(tr(if open { "group_sheet_less" } else { "group_sheet_more" }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(sheet) = this.sidebar.grouping.sheet.as_mut() && !sheet.expanded.remove(&key) { sheet.expanded.insert(key.clone()); }
                                    cx.notify();
                                }))))
                    }))))
                .into_any_element(),
        };
        let task_text = self.sheet_live().and_then(|s| s.pair_task.clone()).filter(|t| !t.trim().is_empty());
        let members_head = div().flex().items_center().justify_between().gap_2()
            .child(heading(format!("{} · {count}", tr("group_sheet_members"))).mt_0())
            .children(add);
        let side = div().flex().flex_col().gap_3()
            .children(task_text.map(|t| div().flex().items_start().gap_2().p_3().rounded(px(8.)).bg(theme::inset())
                .child(chrome::small_icon(IconName::ListChecks, 14., theme::muted()).mt(px(2.)))
                .child(div().flex_1().min_w_0().text_sm().whitespace_normal().child(t))))
            .when(outside, |el| el.child(muted(tr_shared("pair_not_transitive", &[]))))
            // Esta sessão é a ponta do par: o "Sair do grupo" do rodapé desfaz o par para todos.
            .children(self.anchored_owners(&Target::new(&server, &me)).iter()
                .map(|dono| warn(tr_shared("pair_undo_anchor_body", &[("dono", dono)]))))
            .child(div().flex().flex_col().gap_2().child(members_head).child(members)
                .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(web("par_membros_hint"))))
            .children(add_panel)
            .child(pending)
            .children(contract);
        let talk = div().flex().flex_col().gap_2().child(heading(web("par_conversa_titulo")).mt_0());
        match fit {
            // Duas colunas de altura fixa, cada uma com rolagem: o painel não cresce nem pula quando a conversa chega.
            Some(height) => div().flex().gap(px(20.)).h(height)
                .child(div().id("group-side").w(px(420.)).flex_shrink_0().h_full().overflow_y_scroll().pr_1().child(side))
                .child(div().w(px(1.)).h_full().bg(theme::border()))
                .child(talk.flex_1().min_w_0().h_full().child(div().flex_1().min_h_0().flex().flex_col().child(feed))),
            None => side.child(talk.mt(px(8.)).child(feed)),
        }
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{ChatEvent, Delivery, Failure, FeedItem, build_feed, group_delivery, sheet_members, tidy_message};

    #[test]
    fn panel_lists_every_member_with_this_session_marked() {
        let peers = ["ABC-1-proxy".to_owned(), "vm::ABC-1-app".into(), "vm::abc2-app".into()];
        let got = sheet_members("ABC-1-coord", &peers);
        assert_eq!(got.len(), 4, "o título diz quatro: a lista também");
        assert_eq!(got[0], ("ABC-1-coord".to_owned(), true), "esta sessão primeiro, marcada");
        assert!(got[1..].iter().all(|(_, me)| !me));
    }

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
    fn message_gets_its_kind_as_tag_short_home_and_one_line_per_item() {
        let (tag, body) = tidy_message("ANDAMENTO W5 seta: (1) sim, em /home/eu/x.png e /home/euler/y; (2) a seta é dela", Some("/home/eu"));
        assert_eq!(tag.as_deref(), Some("ANDAMENTO"));
        assert_eq!(body, "W5 seta:\n(1) sim, em ~/x.png e /home/euler/y;\n(2) a seta é dela");
        let (tag, body) = tidy_message("OK, pode seguir (ver item 2)", None);
        assert_eq!(tag, None, "palavra comum não vira etiqueta");
        assert_eq!(body, "OK, pode seguir (ver item 2)");
    }

    #[test]
    fn group_send_fails_when_any_member_did_not_get_it() {
        let ok = |delivered| Delivery { ok: true, delivered, ..Default::default() };
        let sent = group_delivery("a", vec![("a".into(), ok(true), None), ("b".into(), ok(false), None)]).expect("todos receberam");
        assert!(sent.delivered, "a entrega é a desta sessão");
        let error = group_delivery("a", vec![("a".into(), ok(true), None), ("b".into(), Delivery { ok: false, delivered: false, ..Default::default() }, Some("morta".into()))])
            .expect_err("b não recebeu");
        assert!(error.detail.contains('a') && error.detail.ends_with("b (morta)"), "{}", error.detail);
    }
}
