//! Abas horizontais: o grupo ocupa uma pilha sem desnível vertical.
use super::*;
use super::grouping::{Member, Row, global_groups};
use super::sidebar::Target;

pub(super) struct TabGroup {
    pub(super) label: Option<String>,
    pub(super) members: Vec<Target>,
    /// O primeiro membro abriu os outros (filhas ligadas) ou é o líder `orq`: o grupo vira uma aba só, a dele.
    pub(super) mother: bool,
    /// Chave do grupo no modelo global (`GlobalGroup::key`); aba solta não tem.
    pub(super) key: Option<String>,
}

/// O que cada grupo diz de quem deve ficar na frente, juntado das linhas dele.
#[derive(Default)]
struct Hints {
    /// (filha, quem a abriu); o vínculo é só local, então quem abriu está na máquina da filha.
    openers: Vec<(Target, Target)>,
    /// Quem formou o grupo, pelo `pair_lead` da primeira linha que o traz.
    formed_by: Option<Target>,
    orq: bool,
}

pub(super) fn reveal_offset(current: f32, viewport: (f32, f32), tab: (f32, f32)) -> f32 {
    if tab.0 + current < viewport.0 { viewport.0 - tab.0 }
    else if tab.1 + current > viewport.1 { viewport.1 - tab.1 }
    else { current }
}

/// Espera a pessoa: pergunta, permissão ou menu aberto. Na ordem da barra vem antes das que trabalham, como na lista.
fn waits(s: &SessionInfo) -> bool { s.state == "awaiting_input" || s.pending_questions > 0 }

pub(super) fn bar_height() -> f32 { if appearance::get().tabs_full { 64. } else { 56. } }
/// 8 px de folga em cima e embaixo: a máquina fica montada na borda de cima e as filhas do grupo na de baixo, metade para fora.
pub(super) fn tab_height() -> f32 { bar_height() - 16. }
const KID_CHIP: f32 = 14.;
/// Etiquetas de filhas à vista na borda; o resto vira "+N".
const KID_CHIPS: usize = 3;

/// A linha sob o nome, na Completa: a pasta (se difere do nome) e a branch.
pub(super) fn meta_line(session: &SessionInfo, full: bool) -> Option<Div> {
    let folder = full.then(|| folder_name(session)).flatten().filter(|f| *f != session.name);
    let branch = full.then(|| shown_branch(session)).flatten();
    // Commits para enviar e para puxar, os mesmos ↑↓ da barra lateral; ficam inteiros no fim da linha.
    let ahead = session.git_ahead.filter(|n| full && *n > 0);
    let behind = session.git_behind.filter(|n| full && *n > 0);
    if folder.is_none() && branch.is_none() && ahead.is_none() && behind.is_none() { return None; }
    // `overflow_hidden` deixa cada pedaço encolher até zero: só com `min_w_0` o texto cortado segurava a aba larga.
    let item = |icon: IconName, text: String| div().min_w_0().overflow_hidden().flex().items_center().gap(px(3.))
        .child(chrome::small_icon(icon, 10., theme::faint())).child(div().min_w_0().truncate().child(text));
    Some(div().min_w_0().overflow_hidden().flex().items_center().gap(px(8.)).text_size(px(10.)).line_height(px(12.)).text_color(theme::faint())
        .children(folder.map(|f| item(IconName::Folder, f)))
        .children(branch.map(|b| item(IconName::GitBranch, b)))
        .children(ahead.map(|n| div().flex_shrink_0().text_color(theme::accent()).child(format!("↑{n}"))))
        .children(behind.map(|n| div().flex_shrink_0().text_color(theme::warning()).child(format!("↓{n}")))))
}

/// A máquina como legenda no canto direito da borda de cima. O fundo cheio corta a linha da borda atrás do texto.
fn machine_legend(machine: String, on: bool) -> Div {
    div().absolute().top(px(-6.)).right(px(8.)).max_w(px(110.)).px(px(4.)).h(px(12.)).flex().items_center().gap(px(3.))
        .rounded(px(3.)).bg(theme::background().alpha(1.)).text_size(px(9.)).line_height(px(12.))
        .text_color(if on { theme::accent_text() } else { theme::muted() })
        .child(chrome::small_icon(IconName::Monitor, 9., if on { theme::accent_text() } else { theme::faint() }))
        .child(div().min_w_0().truncate().child(machine))
}

/// Na Completa, a marca de sessão sem terminal da linha da barra lateral, ao lado do nome.
fn terminal_mark(session: &SessionInfo) -> Option<Div> {
    (appearance::get().tabs_full && session.headless)
        .then(|| div().flex_shrink_0().flex().opacity(0.72).child(chrome::no_terminal_mark(11., theme::muted())))
}
/// O pedaço do nome de cada filha que a distingue: tira o começo (até um `-`/`_`) que ela divide com outro nome do
/// grupo, sem diferenciar maiúsculas. `ABC-42-backend` ao lado de `ABC-42-Lead` vira `backend`.
pub(super) fn short_names(names: &[&str]) -> Vec<String> {
    names.iter().enumerate().map(|(i, name)| {
        let cut = names.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, other)| {
            let common = name.chars().zip(other.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            name.char_indices().take(common).filter(|(_, c)| matches!(c, '-' | '_')).map(|(at, _)| at + 1).last().unwrap_or(0)
        }).max().unwrap_or(0);
        let rest = &name[cut..];
        let rest = if rest.is_empty() { *name } else { rest };
        if rest.chars().count() > 12 { format!("{}…", rest.chars().take(11).collect::<String>()) } else { rest.to_owned() }
    }).collect()
}

/// Toda aba tem a mesma largura; o nome e a linha de baixo cortam com reticências.
const TAB_WIDTH: f32 = 240.;
const REAR_WIDTH: f32 = 88.;
const OVERLAP: f32 = 12.;

impl TabGroup {
    fn slot(&self, index: usize) -> Option<(f32, f32)> {
        let label = self.label.as_deref()?;
        let front = (label.chars().count() as f32 * 4.5 + 32.).clamp(132., 184.);
        Some(if index == 0 { (0., front) } else { (front - OVERLAP + (index - 1) as f32 * (REAR_WIDTH - OVERLAP), REAR_WIDTH) })
    }
}

/// Uma faixa por grupo do modelo global: membros de máquinas diferentes do mesmo grupo dividem a faixa. `chosen` é a
/// principal escolhida à mão por chave de grupo.
pub(super) fn groups(entries: Vec<(Target, &SessionInfo)>, ids: &HashMap<String, String>, chosen: &HashMap<String, Target>) -> Vec<TabGroup> {
    let rows: Vec<Row> = entries.iter().map(|(t, s)| Row { server: &t.server, session: s, invite: false }).collect();
    let mut keys: HashMap<Target, String> = HashMap::new();
    for g in global_groups(&rows, ids) {
        for m in g.members {
            if let Member::Here { server, name } = m { keys.insert(Target { server, name }, g.key.clone()); }
        }
    }
    let mut out: Vec<TabGroup> = Vec::new();
    let mut indexes = HashMap::new();
    let mut hints: Vec<Hints> = Vec::new();
    for (target, session) in entries {
        let Some(key) = keys.get(&target).cloned() else {
            out.push(TabGroup { label: None, members: vec![target], mother: false, key: None });
            hints.push(Hints::default());
            continue;
        };
        let index = *indexes.entry(key.clone()).or_insert_with(|| {
            out.push(TabGroup { label: Some(String::new()), members: Vec::new(), mother: false, key: Some(key) });
            hints.push(Hints::default());
            out.len() - 1
        });
        let group = &mut out[index];
        if group.label.as_ref().is_some_and(|s| s.is_empty()) {
            group.label = Some(session.pair_task.as_deref().unwrap_or("").trim().to_owned());
        }
        let hint = &mut hints[index];
        if let Some(parent) = session.linked_by.as_deref() { hint.openers.push((target.clone(), Target::new(&target.server, parent))); }
        if let Some(lead) = session.pair_lead.as_deref() { hint.formed_by.get_or_insert_with(|| Target::new(&target.server, lead)); }
        if session.orq() {
            hint.orq = true;
            group.mother = true;
            group.members.insert(0, target);
        } else { group.members.push(target); }
    }
    for (group, hint) in out.iter_mut().zip(hints) {
        if group.label.as_ref().is_some_and(|s| s.is_empty()) {
            group.label = Some(group.members.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
        }
        if group.members.len() < 2 { group.mother = false; continue; }
        let openers = &hint.openers;
        let at = |t: &Target| group.members.iter().position(|m| m == t);
        // Numa cadeia (A abriu B, B abriu C) a mãe é a do topo: quem abriu e não foi aberta por ninguém do grupo.
        let opened = |t: &Target| openers.iter().any(|(child, parent)| child == t && group.members.contains(parent));
        let opener = |t: &Target| openers.iter().any(|(_, parent)| parent == t);
        // A escolhida à mão vence; depois o líder `orq` (já na frente), quem formou o grupo e o topo da cadeia.
        let front = group.key.as_ref().and_then(|k| chosen.get(k)).and_then(at)
            .or(hint.orq.then_some(0))
            .or_else(|| hint.formed_by.as_ref().and_then(at))
            .or_else(|| group.members.iter().position(|t| opener(t) && !opened(t)))
            .or_else(|| group.members.iter().position(|t| opener(t)));
        if let Some(front) = front {
            let mother = group.members.remove(front);
            group.members.insert(0, mother);
        }
        // Todo grupo é uma aba só. Sem nenhuma dessas, a principal é a primeira da lista: a sessão não diz quando
        // nasceu.
        group.mother = true;
    }
    // Grupos primeiro, cada um na ordem da lista; as abas soltas depois.
    out.sort_by_key(|group| !group.mother);
    out
}

impl Hangar {
    /// Recalculado só quando muda o que `groups` lê: as abas, os campos de grupo delas e os identificadores.
    pub(super) fn tab_groups(&self) -> std::rc::Rc<Vec<TabGroup>> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let entries: Vec<(Target, &SessionInfo)> = self.ungrouped_tab_targets().into_iter()
            .filter_map(|target| self.target_session(&target).map(|session| (target, session))).collect();
        let order = appearance::get();
        let recent_first = order.tabs_recent_first;
        // A ordem por data também separa as que trabalham: assim a aba só muda de lugar quando o turno acaba.
        let split = order.tabs_working_first || recent_first;
        let mut hasher = DefaultHasher::new();
        self.hash_identities(&mut hasher);
        (split, recent_first).hash(&mut hasher);
        let mut chosen: Vec<_> = self.sidebar.group_leads.iter().collect();
        chosen.sort_by(|a, b| a.0.cmp(b.0));
        chosen.hash(&mut hasher);
        for (t, s) in &entries {
            (t, &s.pair_gid, &s.pair_owner, s.peers(), &s.pair_task, s.orq(), &s.linked_by, &s.pair_lead).hash(&mut hasher);
            let busy = super::sidebar::working(s);
            if split { (busy, waits(s)).hash(&mut hasher); }
            // A data de quem trabalha não entra: resposta nova no meio do turno não mexe na barra.
            if recent_first && !busy { s.last_reply_at.map(f64::to_bits).hash(&mut hasher); }
        }
        super::grouping::memo(&self.sidebar.grouping.tabs, hasher.finish(), || {
            let busy: std::collections::HashSet<Target> = entries.iter().filter(|(_, s)| super::sidebar::working(s)).map(|(t, _)| t.clone()).collect();
            let waiting: std::collections::HashSet<Target> = entries.iter().filter(|(_, s)| waits(s)).map(|(t, _)| t.clone()).collect();
            let at: HashMap<Target, f64> = entries.iter().map(|(t, s)| (t.clone(), s.last_reply_at.unwrap_or(0.))).collect();
            let mut out = groups(entries, &self.identities(), &self.sidebar.group_leads);
            if split {
                // Como a lista lateral: grupos, as que esperam a pessoa, as que trabalham, depois as paradas; sem a ordem
                // por data, cada parte fica como estava.
                let section = |g: &TabGroup| if g.mother { 0 }
                    else if g.members.iter().any(|t| waiting.contains(t)) { 1 }
                    else if g.members.iter().any(|t| busy.contains(t)) { 2 } else { 3 };
                let recency = |g: &TabGroup| if recent_first && section(g) == 3 {
                    g.members.iter().filter_map(|t| at.get(t)).copied().fold(0., f64::max)
                } else { 0. };
                out.sort_by(|a, b| section(a).cmp(&section(b)).then_with(|| recency(b).total_cmp(&recency(a))));
            }
            out
        })
    }

    /// Onde a aba de `target` começa na janela e a faixa de cima e de baixo dela, já com a rolagem; pela mesma conta do
    /// `reveal_target_tab`. Filha escondida na mãe vale a aba da mãe.
    pub(super) fn tab_anchor(&self, target: &Target) -> Option<(Pixels, Pixels, Pixels)> {
        let viewport = self.tabs_scroll.bounds();
        let offset = self.tabs_scroll.offset().x;
        for (index, group) in self.tab_groups().iter().enumerate() {
            let Some(member) = group.members.iter().position(|t| t == target) else { continue; };
            let bounds = self.tabs_scroll.bounds_for_item(index)?;
            let left = if group.mother { 0. } else { group.slot(member).map_or(0., |(left, _)| left) };
            return Some((bounds.left() + offset + px(left), viewport.top(), viewport.bottom()));
        }
        None
    }

    /// Quantas abas a rolagem deixa fora da faixa, à esquerda e à direita (cortadas contam).
    pub(super) fn tabs_hidden(&self) -> (usize, usize) {
        let viewport = self.tabs_scroll.bounds();
        let offset = self.tabs_scroll.offset().x;
        (0..self.tab_groups().len()).filter_map(|i| self.tabs_scroll.bounds_for_item(i)).fold((0, 0), |(left, right), bounds| {
            let start = bounds.left() + offset;
            let end = start + bounds.size.width;
            if start < viewport.left() - px(4.) { (left + 1, right) }
            else if end > viewport.right() + px(4.) { (left, right + 1) }
            else { (left, right) }
        })
    }

    /// Rola a faixa quase uma tela para o lado (`-1` esquerda, `1` direita).
    pub(super) fn scroll_tabs(&mut self, direction: f32, cx: &mut Context<Self>) {
        let viewport = self.tabs_scroll.bounds();
        let offset = self.tabs_scroll.offset();
        let max = self.tabs_scroll.max_offset().x;
        let x = (offset.x - viewport.size.width * 0.8 * direction).min(px(0.)).max(-max);
        self.tabs_scroll.set_offset(point(x, offset.y));
        cx.notify();
    }

    /// A aba sob `x` (posição na janela), pela mesma conta do `tab_anchor`; na faixa de um grupo, a da frente ganha.
    pub(super) fn tab_at(&self, x: Pixels) -> Option<Target> {
        let offset = self.tabs_scroll.offset().x;
        for (index, group) in self.tab_groups().iter().enumerate() {
            let Some(bounds) = self.tabs_scroll.bounds_for_item(index) else { continue; };
            let left = bounds.left() + offset;
            if x < left || x >= left + bounds.size.width { continue; }
            if group.mother { return group.members.first().cloned(); }
            let local = f32::from(x - left);
            // O índice 0 é a aba da frente (desenhada por último).
            let member = (0..group.members.len()).find(|&i| group.slot(i).is_some_and(|(l, w)| local >= l && local < l + w)).unwrap_or(0);
            return group.members.get(member).cloned();
        }
        None
    }

    pub(super) fn tab_targets(&self) -> Vec<Target> {
        self.tab_groups().iter().flat_map(|group| group.members.iter().cloned()).collect()
    }

    /// As abas que estão na faixa e recebem foco: do grupo com mãe só ela, as filhas ficam na janela dela.
    pub(super) fn tab_stops(&self) -> Vec<Target> {
        self.tab_groups().iter().flat_map(|group| group.members.iter().take(if group.mother { 1 } else { usize::MAX }).cloned()).collect()
    }

    /// A aba da faixa que mostra `target`: a própria, ou a da mãe quando ela é filha escondida.
    pub(super) fn tab_stop_of(&self, target: &Target) -> Target {
        self.tab_groups().iter().find(|g| g.mother && g.members.contains(target)).map_or_else(|| target.clone(), |g| g.members[0].clone())
    }

    pub(super) fn scroll_target_tab(&mut self, target: &Target) {
        self.pending_tab_reveal = Some(target.clone());
        self.reveal_target_tab(target);
    }

    pub(super) fn schedule_tab_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_tab_reveal.is_none() || self.tab_reveal_scheduled { return; }
        self.tab_reveal_scheduled = true;
        // Agendado no render: o quadro atual atualiza os bounds antes desta única repetição.
        cx.on_next_frame(window, |this, _, cx| {
            this.tab_reveal_scheduled = false;
            if let Some(target) = this.pending_tab_reveal.take() {
                this.reveal_target_tab(&target);
                cx.notify();
            }
        });
    }

    fn reveal_target_tab(&self, target: &Target) {
        for (index, group) in self.tab_groups().iter().enumerate() {
            let Some(member) = group.members.iter().position(|t| t == target) else { continue; };
            let viewport = self.tabs_scroll.bounds();
            if let Some(bounds) = self.tabs_scroll.bounds_for_item(index) {
                // Filha de um grupo com mãe não tem aba própria na faixa: vale a aba inteira da mãe.
                let whole = (0., f32::from(bounds.size.width));
                let (left, width) = if group.mother { whole } else { group.slot(member).unwrap_or(whole) };
                let member = if group.mother { 0 } else { member };
                let start = bounds.left() + px(left + if member > 0 { OVERLAP } else { 0. });
                let end = bounds.left() + px(left + width);
                let offset = self.tabs_scroll.offset();
                let inset = px((f32::from(viewport.size.width) / 2.).min(3.));
                let x = reveal_offset(f32::from(offset.x), (f32::from(viewport.left() + inset), f32::from(viewport.right() - inset)),
                    (f32::from(start), f32::from(end)));
                self.tabs_scroll.set_offset(point(px(x), offset.y));
            } else { self.tabs_scroll.scroll_to_item(index); }
            break;
        }
    }

    /// Rola a faixa o quanto a aba `index` andou desde `from` (onde ela estava antes da ordem nova), na curva do deslize
    /// dela: as duas andam juntas e ela não sai do lugar na tela. `span` = rolagem de partida e de chegada, e a última
    /// gravada por este laço. Para quando outra ordem começa outro (`seq`) ou quando alguém rolou a faixa no meio.
    #[allow(clippy::too_many_arguments)]
    fn follow_open_tab(&mut self, seq: u64, index: usize, from: Pixels, start: std::time::Instant, span: Option<(f32, f32, f32)>,
        window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.tab_follow.get() != seq { return; }
        let offset = self.tabs_scroll.offset();
        if span.is_some_and(|(_, _, wrote)| (f32::from(offset.x) - wrote).abs() > 0.5) { return; }
        let Some((begin, end, _)) = span.or_else(|| {
            let now = self.tabs_scroll.bounds_for_item(index)?;
            let max = f32::from(self.tabs_scroll.max_offset().x);
            let begin = f32::from(offset.x);
            Some((begin, (begin + f32::from(from - now.left())).clamp(-max, 0.), begin))
        }) else { return };
        // Sem animação (pedido do sistema, janela fora de foco) a aba pula, e a faixa pula junto.
        let still = cx.reduce_motion() || !window.is_window_active();
        let raw = if still { 1. } else { motion::REORDER.raw_at(start, std::time::Instant::now()) };
        let wrote = begin + (end - begin) * motion::REORDER.ease(raw);
        self.tabs_scroll.set_offset(point(px(wrote), offset.y));
        cx.notify();
        if raw < 1. {
            cx.on_next_frame(window, move |this, window, cx| this.follow_open_tab(seq, index, from, start, Some((begin, end, wrote)), window, cx));
        }
    }

    pub(super) fn render_tab_groups(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut order = 0;
        let bands = self.tab_groups();
        // Ordem nova (trabalhando primeiro, mais recente primeiro) desliza cada aba até o lugar novo, como na lista.
        let keys: Vec<String> = bands.iter().map(|g| g.members[0].id().to_string()).collect();
        let open = self.selected_target().map(|t| self.tab_stop_of(&t)).and_then(|t| bands.iter().position(|g| g.members.contains(&t)));
        let moved = {
            let mut book = self.sidebar.tab_flip.borrow_mut();
            let moved = open.and_then(|new| book.index_of(&keys[new]).filter(|old| *old != new).map(|old| (old, new)));
            book.sections(&keys.iter().map(|k| (k.clone(), String::new())).collect::<Vec<_>>(), std::time::Instant::now(), cx.reduce_motion());
            book.order(&keys);
            moved
        };
        // A aba aberta mudou de lugar: a faixa rola junto com ela, que fica parada onde a pessoa estava olhando.
        if let Some((old, new)) = moved && let Some(before) = self.tabs_scroll.bounds_for_item(old) {
            let (from, start) = (before.left(), std::time::Instant::now());
            let seq = self.sidebar.tab_follow.get() + 1;
            self.sidebar.tab_follow.set(seq);
            cx.on_next_frame(window, move |this, window, cx| this.follow_open_tab(seq, new, from, start, None, window, cx));
        }
        bands.iter().zip(keys).map(|(group, key)| {
            let first = order;
            order += group.members.len();
            let tab = match group.label.as_deref() {
                None => div().flex_shrink_0().children(self.render_session_tab(&group.members[0], None, None, first, window, cx)).into_any_element(),
                Some(label) => match group.mother.then(|| self.render_mother_tab(group, label, first, window, cx)).flatten() {
                    Some(tab) => tab,
                    None => {
                        let (last, size) = group.slot(group.members.len() - 1).expect("grupo com membro");
                        let width = last + size;
                        // Os de trás pintam primeiro; todos mantêm a mesma altura e deixam uma faixa clicável exposta.
                        let tabs = group.members.iter().enumerate().rev().filter_map(|(index, target)| {
                            self.render_session_tab(target, (index == 0).then_some(label), group.slot(index), first + index, window, cx)
                        }).collect::<Vec<_>>();
                        div().id(SharedString::from(format!("tab-group-{}", group.members[0].id())))
                            .relative().flex_shrink_0().w(px(width)).h(px(tab_height())).children(tabs).into_any_element()
                    }
                },
            };
            motion::Flip::new(key, tab, self.sidebar.tab_flip.clone()).horizontal().into_any_element()
        }).collect()
    }

    /// Grupo com mãe: uma aba só, a dela. As filhas viram etiquetas pequenas sobre o nome (o clique abre a filha); o clique no
    /// resto abre a mãe e a janela com as abas inteiras das filhas, para cima quando a faixa fica embaixo.
    fn render_mother_tab(&self, group: &TabGroup, label: &str, tab_index: usize, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mother = &group.members[0];
        let session = self.target_session(mother)?;
        let focus = self.row_focus(mother)?.clone().tab_index(tab_index as isize);
        let selected = self.selected_target();
        let on = selected.as_ref().is_some_and(|t| group.members.contains(t));
        let was_open = self.tab_group_open.as_ref() == Some(mother);
        let bottom = appearance::get().navigation == appearance::Navigation::BottomTabs;
        let remote = mother.server != self.active_key();
        let names: Vec<&str> = group.members.iter().map(|t| t.name.as_str()).collect();
        let short = short_names(&names);
        let waits = |s: &SessionInfo| s.pending_questions > 0 || s.state == "awaiting_input" || s.limited == Some(true);
        let mut kids: Vec<(Target, &SessionInfo, String)> = group.members.iter().zip(short).skip(1)
            .filter_map(|(t, text)| self.target_session(t).map(|s| (t.clone(), s, text))).collect();
        // Quem espera a pessoa vem primeiro: com muitas filhas as últimas etiquetas ficam fora da aba.
        kids.sort_by_key(|(_, s, _)| !waits(s));
        let waiting = kids.iter().filter(|(_, s, _)| waits(s)).count();
        let state = |s: &SessionInfo| tr(&format!("chip_{}", if s.limited == Some(true) { "limited" } else { s.state.as_str() }));
        let host = self.server_label(cx);
        let machine = |t: &Target| self.server_entry(&t.server).map(|s| s.label.clone()).unwrap_or_else(|| host.clone());
        let multi = self.multi_server();
        // Fundo cheio: a etiqueta corta a borda de cima da aba, em vez de deixar a linha passar por trás.
        let chip = || div().h(px(KID_CHIP)).px(px(4.)).flex().items_center().gap(px(3.)).rounded(px(KID_CHIP / 2.))
            .bg(theme::background().alpha(1.)).border_1().text_size(px(9.)).line_height(px(12.));
        let mut chips = kids.iter().take(KID_CHIPS).map(|(target, kid, text)| {
            let pick = target.clone();
            let kid_on = selected.as_ref() == Some(target);
            // O avatar animado é o mesmo da aba: pintado fora da faixa guardada, o giro não a redesenha.
            let mark = self.avatar_slot(panes::Area::Nav, format!("tab-kid-mark-{}", target.id()), sidebar::session_look(kid), 10.);
            chip().id(SharedString::from(format!("tab-kid-{}", target.id()))).min_w_0().max_w(px(76.)).cursor_pointer()
                .border_color(if kid_on { theme::accent() } else { theme::border_strong() }).hover(|el| el.border_color(theme::accent()))
                .role(Role::Button).aria_label(format!("{} · {} · {}", kid.name, machine(target), state(kid)))
                // A etiqueta corta o nome; parado sobre ela, o nome inteiro com a máquina e o estado.
                .tooltip({
                    let tip = format!("{} · {} · {}", kid.name, machine(target), state(kid));
                    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(mark)
                .child(div().min_w_0().truncate().text_color(if kid_on { theme::text() } else { theme::muted() }).child(text.clone()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.tab_group_open = None;
                    this.open_target(&pick, window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        }).collect::<Vec<_>>();
        if kids.len() > KID_CHIPS {
            chips.push(chip().flex_shrink_0().border_color(theme::border_strong()).text_color(theme::muted())
                .child(format!("+{}", kids.len() - KID_CHIPS)).into_any_element());
        }
        // Quantas filhas de cada agente, na ordem em que aparecem.
        let mut agents: Vec<(chrome::AvatarLook, usize)> = Vec::new();
        for (_, kid, _) in &kids {
            let look = sidebar::session_look(kid);
            match agents.iter_mut().find(|(seen, _)| seen.glyph() == look.glyph()) {
                Some((_, n)) => *n += 1,
                None => agents.push((chrome::AvatarLook::new(look.glyph(), None, "", false), 1)),
            }
        }
        let count = tr_shared("tab_group_children", &[("n", &kids.len().to_string())]);
        let aria = self.session_number_label(mother, format!("{} · {} · {} · {label} · {count}", session.name, machine(mother), state(session)));
        let mark = self.avatar_slot(panes::Area::Nav, format!("tab-mark-{}", mother.id()), sidebar::session_look(session), 20.);
        let window_tabs = was_open.then(|| {
            let tabs = kids.iter().enumerate().filter_map(|(i, (t, _, _))| self.render_session_tab(t, None, None, tab_index + 1 + i, window, cx))
                .collect::<Vec<_>>();
            // O nome do grupo numa linha própria, inteiro, e as filhas embaixo: ao lado delas ele saía cortado.
            let body = div().id("tab-group-window").p(px(6.)).flex().flex_col().gap(px(4.))
                .child(div().px(px(4.)).max_w(px(560.)).truncate().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::accent_text()).child(label.to_owned()))
                .child(div().flex().items_center().gap(px(2.)).children(tabs))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| { this.tab_group_open = None; cx.notify(); }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { this.tab_group_open = None; cx.notify(); }));
            // Âncora num canto fixo da aba: dentro da linha `items_center` a origem do `anchored` seria o meio dela.
            div().absolute().top_0().left_0().child(deferred(anchored().anchor(if bottom { gpui_kit::Anchor::BottomLeft } else { gpui_kit::Anchor::TopLeft })
                .snap_to_window_with_margin(px(8.))
                .child(div().map(|el| if bottom { el.pb(px(6.)) } else { el.pt(px(tab_height() + 6.)) }).child(chrome::popover(body.into_any_element(), false)))))
        });
        let (pick, open, menu_target, key) = (mother.clone(), mother.clone(), mother.clone(), mother.clone());
        let weak = cx.entity().downgrade();
        let tooltip = aria.clone();
        let full = appearance::get().tabs_full;
        let tab = div().id(SharedString::from(format!("tab-{}", mother.id()))).relative().track_focus(&focus)
            .flex_shrink_0().w(px(TAB_WIDTH)).h(px(tab_height())).px(px(8.)).flex().items_center().gap(px(6.)).rounded(px(6.)).border_1().cursor_pointer()
            .map(|el| if on { el.bg(theme::accent_dim()).border_color(theme::accent()).text_color(theme::text()).font_weight(FontWeight::SEMIBOLD) }
                // Fundo e borda sempre à vista: a aba de grupo se distingue das soltas sem precisar do mouse.
                else { el.bg(theme::text().alpha(0.07)).border_color(if waiting > 0 { theme::warning() } else if was_open { theme::accent() } else { theme::border_strong() })
                    .text_color(theme::muted()).hover(|el| el.border_color(theme::accent())) })
            .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .role(Role::Tab).aria_selected(on).aria_label(aria)
            .when(!appearance::get().tab_preview, |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)))
            .child(mark).children(self.session_number_badge(mother))
            .child(div().min_w_0().overflow_hidden().flex_1().flex().flex_col().when(full, |el| el.gap(px(1.)))
                .child(div().min_w_0().flex().items_center().gap(px(4.))
                    .child(div().min_w_0().truncate().text_size(px(12.)).line_height(px(14.)).child(session.name.clone()))
                    .children(terminal_mark(session)))
                .children(meta_line(session, full)))
            .when(session.pending_questions > 0 || waiting > 0, |el| el.child(div().flex_shrink_0().text_size(px(12.)).text_color(theme::warning())
                .child(format!("? {}", session.pending_questions as usize + waiting))))
            // A janela das filhas abre só pelo contador; o resto da aba abre a sessão-mãe. Alterna descendo, no mesmo
            // despacho do "clique fora" da janela (que já fechou): `was_open` ainda é o do quadro com ela aberta.
            .child(div().id(SharedString::from(format!("tab-kids-{}", mother.id()))).flex_shrink_0().h(px(22.)).px(px(5.))
                // Borda e fundo no hover: o contador é um botão dentro da aba, e precisa parecer um.
                .flex().items_center().gap(px(3.)).rounded(px(6.)).cursor_pointer().border_1().border_color(transparent_black())
                .when(was_open, |el| el.bg(theme::accent_dim()).border_color(theme::accent()))
                .hover(|el| el.bg(theme::accent_dim()).border_color(theme::accent()))
                .role(Role::Button).aria_label(count.clone())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(count.clone()).build(window, cx))
                .child(div().px(px(5.)).rounded(px(8.)).flex().items_center().gap(px(3.)).text_size(px(10.)).line_height(px(14.))
                    .bg(theme::accent_dim()).text_color(theme::accent_text())
                    .children(agents.iter().map(|(look, n)| div().flex().items_center().gap(px(2.))
                        .child(chrome::session_avatar(look, 11., None)).child(n.to_string()))))
                .child(chrome::small_icon(if bottom { IconName::ChevronUp } else { IconName::ChevronDown }, 10., theme::faint()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.tab_group_open = (!was_open).then(|| key.clone());
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_click(|_, _, cx| cx.stop_propagation()))
            .on_click(cx.listener(move |this, _, window, cx| { this.tab_group_open = None; this.open_target(&pick, window, cx) }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.select_target(&open, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_target.clone(), cx)))
            .when(!self.invite_target(mother) && !session.read_only(), |el| {
                self.group_row(el, session, remote.then_some(mother.server.as_str()), 6., cx).ml(px(0.))
            })
            .children(window_tabs);
        // As filhas montadas na borda de baixo, a partir do canto esquerdo, e a máquina na de cima. Fora da aba e depois
        // dela: o GPUI pinta a borda por cima do conteúdo do próprio elemento, e aqui é a legenda que tem de cobrir a linha.
        let legend = div().absolute().bottom(px(-KID_CHIP / 2.)).left(px(8.)).right(px(8.)).flex().gap(px(4.)).children(chips);
        Some(div().relative().flex_shrink_0()
            .child(tab.context_menu(sidebar::session_menu(weak, mother.server.clone(), session.clone())))
            .child(legend)
            .children(multi.then(|| machine_legend(machine(mother), on))).into_any_element())
    }

    fn render_session_tab(&self, target: &Target, group_label: Option<&str>, slot: Option<(f32, f32)>, tab_index: usize,
        window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.target_session(target)?;
        let focus = self.row_focus(target)?.clone().tab_index(tab_index as isize);
        let on = self.selected_target().as_ref() == Some(target);
        let remote = target.server != self.active_key();
        let host = self.server_label(cx);
        let machine = self.server_entry(&target.server).map(|s| s.label.as_str()).unwrap_or(&host);
        let compact = slot.is_some() && group_label.is_none();
        let grouped = slot.is_some();
        let state = if session.limited == Some(true) { "limited" } else { session.state.as_str() };
        let mut label = format!("{} · {} · {}", session.name, machine, tr(&format!("chip_{state}")));
        if let Some(group) = session.pair_task.as_deref().filter(|s| !s.trim().is_empty()) {
            label.push_str(&format!(" · {}", tr_shared("sessao_grupo_pareado", &[("label", group)])));
        }
        if session.pending_questions > 0 { label.push_str(&format!(" · ? {}", session.pending_questions)); }
        let mark = self.avatar_slot(panes::Area::Nav, format!("tab-mark-{}", target.id()), sidebar::session_look(session), if compact { 16. } else { 20. });
        let pick = target.clone();
        let open = target.clone();
        let menu_target = target.clone();
        let label = self.session_number_label(target, label);
        let weak = cx.entity().downgrade();
        // A aba estreita de trás da faixa fica só com a máquina: pasta e branch não cabem nela.
        let full = appearance::get().tabs_full && !compact;
        let title = div().min_w_0().overflow_hidden().flex_1().flex().flex_col().when(full, |el| el.gap(px(1.)))
            .when_some(group_label, |el, text| el.child(div().truncate().text_size(px(9.)).line_height(px(11.))
                .text_color(theme::accent_text()).child(text.to_owned())))
            .child(div().min_w_0().flex().items_center().gap(px(4.))
                .child(div().min_w_0().truncate().text_size(px(if grouped { 11. } else { 13. })).line_height(px(14.)).child(session.name.clone()))
                .children(terminal_mark(session).filter(|_| !compact)))
            .children(meta_line(session, full));
        let tab = div().id(SharedString::from(format!("tab-{}", target.id()))).relative().track_focus(&focus)
            .flex_shrink_0().w(px(TAB_WIDTH)).h(px(tab_height())).px(px(8.))
            .when_some(slot, |el, (left, width)| el.absolute().top_0().left(px(left)).w(px(width)).block_mouse_except_scroll())
            .when(compact, |el| el.pl(px(20.)))
            .flex().items_center().gap(px(if compact { 4. } else { 6. })).rounded(px(6.)).border_1().cursor_pointer()
            .map(|el| if on { el.bg(theme::accent_dim()).border_color(theme::accent()).text_color(theme::text()).font_weight(FontWeight::SEMIBOLD) }
                else if grouped { el.bg(theme::raised()).border_color(theme::border()).text_color(theme::muted()).hover(|el| el.border_color(theme::accent())) }
                // Toda aba tem a borda à vista; a solta fica sem fundo, o que a separa da de grupo.
                else { el.border_color(theme::border_strong()).text_color(theme::muted()).hover(|el| el.border_color(theme::accent())) })
            .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .role(Role::Tab).aria_selected(on).aria_label(label.clone())
            .when(!appearance::get().tab_preview, |el| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)))
            .child(mark).children(self.session_number_badge(target))
            .child(title)
            .when(session.pending_questions > 0, |el| el.child(div().flex_shrink_0().text_size(px(if compact { 9. } else { 12. })).text_color(theme::warning())
                .child(if compact { format!("?{}", session.pending_questions) } else { format!("? {}", session.pending_questions) })))
            .on_click(cx.listener(move |this, _, window, cx| this.open_target(&pick, window, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.select_target(&open, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_target.clone(), cx)))
            .when(!self.invite_target(target) && !session.read_only(), |el| {
                self.group_row(el, session, remote.then_some(target.server.as_str()), 6., cx).ml(px(0.))
            });
        let tab = tab.context_menu(sidebar::session_menu(weak, target.server.clone(), session.clone()));
        // A aba de trás de um grupo é posicionada pela faixa dele: sem legenda de máquina.
        if slot.is_some() || !self.multi_server() { return Some(tab.into_any_element()); }
        Some(div().relative().flex_shrink_0().child(tab).child(machine_legend(machine.to_owned(), on)).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{groups, reveal_offset, Target, SessionInfo};
    use std::collections::HashMap;

    fn session(name: &str, group: Option<&str>, label: Option<&str>) -> SessionInfo {
        SessionInfo { name: name.into(), pair_gid: group.map(str::to_owned), pair_task: label.map(str::to_owned), ..Default::default() }
    }

    #[test]
    fn children_show_the_part_of_the_name_that_differs() {
        let short = super::short_names(&["ABC-42-Lead", "ABC-42-backend", "abc41-mobile", "abc41-web", "solo"]);
        assert_eq!(short, ["Lead", "backend", "mobile", "web", "solo"]);
        assert_eq!(super::short_names(&["x-a", "x-uma-filha-de-nome-comprido"])[1], "uma-filha-d…");
    }

    #[test]
    fn groups_come_before_loose_tabs() {
        let (solo, a, b) = (session("solo", None, None), session("a", Some("g"), Some("Par")), session("b", Some("g"), None));
        let grouped = groups(vec![(Target::new("s", "solo"), &solo), (Target::new("s", "a"), &a), (Target::new("s", "b"), &b)], &HashMap::new(), &HashMap::new());
        assert_eq!(grouped[0].members, vec![Target::new("s", "a"), Target::new("s", "b")]);
        assert_eq!(grouped[1].members, vec![Target::new("s", "solo")]);
    }

    #[test]
    fn tab_groups_keep_members_together_and_preserve_the_front() {
        let lead = session("coordenadora", Some("g"), Some("DEMO-42 · Painéis"));
        let other = session("outra", None, None);
        let worker = session("serviço", Some("g"), None);
        let grouped = groups(vec![(Target::new("a", &lead.name), &lead), (Target::new("a", &other.name), &other),
            (Target::new("a", &worker.name), &worker)], &HashMap::new(), &HashMap::new());
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].label.as_deref(), Some("DEMO-42 · Painéis"));
        assert_eq!(grouped[0].members, vec![Target::new("a", "coordenadora"), Target::new("a", "serviço")]);
        assert_eq!(grouped[1].members, vec![Target::new("a", "outra")]);
    }

    #[test]
    fn tab_groups_use_the_orchestrator_without_crossing_servers() {
        let worker = session("executor", Some("g"), Some(" "));
        let mut lead = session("principal", Some("g"), Some("Plano"));
        lead.provider = "orq".into();
        let remote = session("executor", Some("g"), Some("Outro trabalho"));
        let grouped = groups(vec![(Target::new("a", "executor"), &worker), (Target::new("b", "executor"), &remote),
            (Target::new("a", "principal"), &lead)], &HashMap::new(), &HashMap::new());
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].members, vec![Target::new("a", "principal"), Target::new("a", "executor")]);
        assert_eq!(grouped[0].label.as_deref(), Some("Plano"));
        assert_eq!(grouped[1].members, vec![Target::new("b", "executor")]);
    }

    #[test]
    fn members_of_one_group_on_two_machines_share_the_band() {
        let owned = |name: &str| SessionInfo { pair_owner: Some("casa".into()), ..session(name, Some("g"), Some("ABC-1")) };
        let (x, y, solo) = (owned("x"), owned("y"), session("z", Some("g"), None));
        let ids = HashMap::from([("a".to_owned(), "casa".to_owned()), ("b".to_owned(), "lab".to_owned())]);
        let grouped = groups(vec![(Target::new("a", "x"), &x), (Target::new("b", "z"), &solo), (Target::new("b", "y"), &y)], &ids, &HashMap::new());
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].members, vec![Target::new("a", "x"), Target::new("b", "y")], "mesma dona e gid: uma faixa");
        assert_eq!(grouped[1].members, vec![Target::new("b", "z")], "gid igual sem dona é outro grupo, da máquina dele");
    }

    #[test]
    fn the_session_that_opened_the_others_becomes_the_single_tab() {
        let lead = session("ABC-42-Lead", Some("g"), Some("ABC-42"));
        let kid = |name: &str| SessionInfo { linked_by: Some("ABC-42-Lead".into()), ..session(name, Some("g"), None) };
        let (web, api, solo) = (kid("ABC-42-web"), kid("ABC-42-api"), session("outra", None, None));
        let grouped = groups(vec![(Target::new("a", &web.name), &web), (Target::new("a", "outra"), &solo),
            (Target::new("a", &lead.name), &lead), (Target::new("a", &api.name), &api)], &HashMap::new(), &HashMap::new());
        assert!(grouped[0].mother, "quem abriu as filhas é a aba do grupo");
        assert_eq!(grouped[0].members[0], Target::new("a", "ABC-42-Lead"));
        assert_eq!(grouped[0].members.len(), 3);
        assert!(!grouped[1].mother);
        // Cadeia: a abriu b, b abriu c; a mãe é a do topo, mesmo listada depois.
        let link = |name: &str, by: &str| SessionInfo { linked_by: Some(by.into()), ..session(name, Some("k"), None) };
        let (b, c, a) = (link("b", "a"), link("c", "b"), session("a", Some("k"), Some("Cadeia")));
        let chain = groups(vec![(Target::new("a", "b"), &b), (Target::new("a", "c"), &c), (Target::new("a", "a"), &a)], &HashMap::new(), &HashMap::new());
        assert_eq!(chain[0].members[0], Target::new("a", "a"));
        // Pareamento de iguais, sem quem abriu: também uma aba só, a da primeira da lista.
        let (x, y) = (session("x", Some("h"), Some("Par")), session("y", Some("h"), None));
        let pair = groups(vec![(Target::new("a", "x"), &x), (Target::new("a", "y"), &y)], &HashMap::new(), &HashMap::new());
        assert!(pair[0].mother);
        assert_eq!(pair[0].members[0], Target::new("a", "x"));
    }

    #[test]
    fn who_formed_the_group_leads_and_a_manual_choice_wins() {
        let formed = |name: &str| SessionInfo { pair_lead: Some("Coord".into()), ..session(name, Some("g"), Some("PM-1")) };
        let (kid, coord) = (formed("Filha"), formed("Coord"));
        let entries = || vec![(Target::new("a", "Filha"), &kid), (Target::new("a", "Coord"), &coord)];
        let auto = groups(entries(), &HashMap::new(), &HashMap::new());
        assert_eq!(auto[0].members[0], Target::new("a", "Coord"), "quem formou o grupo pelo --pair fica na frente");
        let key = auto[0].key.clone().unwrap();
        let chosen = HashMap::from([(key.clone(), Target::new("a", "Filha"))]);
        assert_eq!(groups(entries(), &HashMap::new(), &chosen)[0].members[0], Target::new("a", "Filha"));
        // Escolhida que saiu do grupo não vale: volta a automática.
        let gone = HashMap::from([(key, Target::new("a", "Outra"))]);
        assert_eq!(groups(entries(), &HashMap::new(), &gone)[0].members[0], Target::new("a", "Coord"));
    }

    #[test]
    fn tab_reveal_scrolls_to_the_member_inside_a_wide_group() {
        assert_eq!(reveal_offset(0., (100., 400.), (700., 778.)), -378.);
        assert_eq!(reveal_offset(-400., (100., 400.), (700., 778.)), -400.);
        assert_eq!(reveal_offset(-400., (100., 400.), (100., 284.)), 0.);
    }
}
