//! Faixa do grupo acima do campo: todos os membros do grupo da conversa aberta, de qualquer máquina conectada, com o
//! vínculo de cada um (quem abriu quem). Cartão do próprio Hangar (borda e fonte da interface), como o do app do
//! celular, não a caixa mono dos mods. Começa fechado, como os mods; aberto ou fechado é escolha de cada conversa.
use gpui_kit::base::AccordionTrigger;

use super::*;
use super::grouping::{Member, Row, global_groups};
use super::sidebar::Target;

/// Cerca de quatro membros à vista; o resto rola, para o cartão não empurrar a conversa.
const LIST_ROWS: usize = 4;
const LIST_MAX: f32 = LIST_ROWS as f32 * 46. + 8.;

/// O que não é pergunta, pronto nem encerrado conta como trabalhando (carregando, sem estado).
fn tone(state: &str) -> &'static str {
    match state { "awaiting_input" => "awaiting_input", "idle" => "idle", "dead" => "dead", _ => "working" }
}

fn rank(tone: &str) -> u8 {
    match tone { "awaiting_input" => 0, "working" => 1, "idle" => 2, _ => 3 }
}

/// As palavras do `rotuloEstado` do web, pelo tom.
fn state_label(tone: &str) -> String {
    tr_shared(match tone { "awaiting_input" => "estado_aguardando", "idle" => "estado_pronto", "dead" => "estado_encerrado", _ => "estado_em_execucao" }, &[])
}

impl Hangar {
    /// O avatar do agente com o estado (o giro de trabalhando é pintado fora da faixa guardada, como nas abas).
    /// `place` separa o chip do cabeçalho da linha da lista: a mesma chave faria um lugar apagar o outro.
    fn linked_avatar(&self, place: &str, target: &Target, session: &SessionInfo, size: f32) -> AnyElement {
        self.avatar_slot(panes::Area::Bottom, format!("linked-{place}-{}", target.id()), sidebar::session_look(session), size)
    }

    /// Os membros do grupo da conversa aberta que abrem daqui, por nome, e os que não abrem (máquina sem conexão aqui,
    /// par de fora) como texto; nada sem grupo de dois.
    fn open_group(&self) -> Option<(Target, Vec<(Target, &SessionInfo)>, Vec<String>)> {
        let open = self.selected.as_ref()?;
        // Sem grupo nada a montar: a conta abaixo roda a cada desenho da área do campo.
        if open.pair_gid.as_deref().is_none_or(str::is_empty) && open.peers().is_empty() { return None; }
        // shortcut: refeito a cada desenho com grupo; lista curta. Memoizar como `tab_groups` se pesar.
        let current = Target::new(&self.open_server(), &open.name);
        let mut machines = self.group_machines();
        if !machines.contains(&current.server) { machines.push(current.server.clone()); }
        let rows: Vec<Row> = machines.iter().flat_map(|key| self.sessions_of(key).iter().map(move |s| Row { server: key, session: s, invite: false })).collect();
        let here = |m: &Member| matches!(m, Member::Here { server, name } if *server == current.server && *name == current.name);
        let group = global_groups(&rows, &self.identities()).into_iter().find(|g| g.members.iter().any(here))?;
        let mut members: Vec<(Target, &SessionInfo)> = group.members.iter().filter_map(|m| match m {
            Member::Here { server, name } => self.sessions_of(server).iter().find(|s| s.name == *name).map(|s| (Target::new(server, name), s)),
            _ => None,
        }).collect();
        let away: Vec<String> = group.members.iter().filter_map(|m| match m {
            Member::Missing { server_id, name } => Some(format!("{server_id}::{name}")),
            Member::Outside { alias, session } => Some(format!("{alias}::{session}")),
            Member::Here { .. } => None,
        }).collect();
        if members.len() + away.len() < 2 { return None; }
        members.sort_by(|a, b| a.1.name.cmp(&b.1.name).then_with(|| a.0.server.cmp(&b.0.server)));
        Some((current, members, away))
    }

    /// O grupo da conversa aberta; sem grupo, nada.
    pub(super) fn render_linked_children(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (current, members, away) = self.open_group()?;
        // A máquina de cada um, com o ícone, como a legenda das abas.
        let machines: Vec<String> = members.iter().map(|(t, _)| self.machine_label(&t.server, cx)).collect();
        let label = members.iter().find_map(|(_, s)| s.pair_task.as_deref().map(str::trim).filter(|t| !t.is_empty())).unwrap_or_default().to_owned();
        // O vínculo só existe na mesma máquina: quem abriu está na máquina da filha. Com a conversa aberta, "desta conversa".
        let relation = |t: &Target, s: &SessionInfo| -> String {
            if let Some(parent) = s.linked_by.as_deref().filter(|p| members.iter().any(|(m, _)| m.server == t.server && m.name == *p)) {
                if current.server == t.server && current.name == parent { return tr("group_panel_child_of_here"); }
                return tr_shared("native_group_panel_child_of", &[("name", parent)]);
            }
            let kids: Vec<&str> = members.iter().filter(|(m, k)| m.server == t.server && k.linked_by.as_deref() == Some(t.name.as_str())).map(|(m, _)| m.name.as_str()).collect();
            if kids.is_empty() { String::new() }
            else if kids.len() == 1 && current.server == t.server && current.name == kids[0] { tr("group_panel_parent_of_here") }
            else { tr_shared("native_group_panel_parent_of", &[("names", &kids.join(", "))]) }
        };
        let relations: Vec<String> = members.iter().map(|(t, s)| relation(t, s)).collect();
        // A conversa aberta já está na tela: o painel mostra só os outros membros.
        let mut others: Vec<usize> = (0..members.len()).filter(|&i| members[i].0 != current).collect();
        // Com rolagem, quem pede resposta e quem trabalha vem primeiro; o nome desempata.
        if others.len() > LIST_ROWS { others.sort_by_key(|&i| rank(tone(&members[i].1.state))); }
        let expanded = self.linked_children_open.contains(&current);
        let total = (members.len() + away.len()).to_string();
        let title = tr_shared("native_group_panel_title", &[("count", &total)]);
        let working = others.iter().filter(|&&i| sidebar::working(members[i].1)).count();
        let busy = (working > 0).then(|| tr_shared("native_group_panel_working", &[("count", &working.to_string())]));

        let focus = window.use_keyed_state("linked-children-header-focus", cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        let weak = cx.entity().downgrade();
        let key = current.clone();
        let header = AccordionTrigger::new("linked-children-header").open(expanded).track_focus(&focus)
            .flex().items_center().gap(px(10.)).min_w_0().px(px(10.)).py(px(7.)).cursor_pointer().hover(|el| el.bg(theme::hover()))
            .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .aria_label(match &busy { Some(b) => format!("{title}, {b}"), None => title.clone() })
            .on_change(move |open, _, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if open { this.linked_children_open.insert(key.clone()); } else { this.linked_children_open.remove(&key); }
                    cx.notify();
                });
            })
            .child(chrome::small_icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::muted()))
            .child(super::grouping::glyph(16., theme::text()))
            .child(div().flex_shrink_0().whitespace_nowrap().font_weight(FontWeight::SEMIBOLD).child(title))
            .when(!label.is_empty(), |el| el.child(div().flex_shrink_0().max_w(px(280.)).truncate().text_color(theme::muted()).child(label.clone())))
            .child(div().flex().flex_1().gap(px(6.)).min_w_0().overflow_hidden()
                .children(others.iter().map(|&i| {
                    let (t, s) = &members[i];
                    let pick = t.clone();
                    // Como os chips da barra de abas; a máquina só quando é outra.
                    div().id(SharedString::from(format!("group-chip-{i}"))).flex_shrink_0().flex().items_center().gap(px(6.)).pl(px(3.)).pr(px(10.)).py(px(2.))
                        .rounded_full().bg(theme::hover()).border_1().border_color(theme::border()).text_size(px(12.5))
                        .cursor_pointer().hover(|el| el.bg(theme::accent_dim()))
                        .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.open_target(&pick, window, cx); }))
                        .child(self.linked_avatar("chip", t, s, 18.)).child(t.name.clone())
                        .when(t.server != current.server, |el| el.child(div().text_size(px(11.)).text_color(theme::faint()).child(machines[i].clone())))
                }))
                // Membro que não abre daqui: só o nome, apagado e sem clique.
                .children(away.iter().map(|name| div().flex_shrink_0().px(px(10.)).py(px(2.)).rounded_full().border_1().border_color(theme::border())
                    .text_size(px(12.5)).text_color(theme::faint()).child(name.clone()))))
            .children(busy.map(|b| div().flex_shrink_0().flex().items_center().gap(px(6.)).whitespace_nowrap().text_color(theme::muted())
                .child(div().size(px(8.)).rounded_full().bg(theme::success())).child(b)));

        let list = expanded.then(|| {
            let open_label = tr_shared("linked_children_open", &[]);
            div().id("linked-children-list").max_h(px(LIST_MAX)).overflow_y_scroll().flex().flex_col().py(px(4.))
                .border_t_1().border_color(theme::border())
                .children(others.iter().map(|&i| {
                    let (t, s) = &members[i];
                    let provider = super::orchestration::provider_name(&s.provider);
                    let kind = match s.engine.as_deref().or(s.model.as_deref()).filter(|m| !m.is_empty()) {
                        Some(model) => format!("{provider} · {}", crate::status::model_label(model)),
                        None => provider.to_owned(),
                    };
                    let state = state_label(tone(&s.state));
                    let last = s.last_reply.as_deref().map(|text| conversation::one_line(text, 200)).unwrap_or_default();
                    let (row, target, name) = (t.clone(), t.clone(), t.name.clone());
                    let dot = || div().flex_shrink_0().child("·");
                    // O avatar animado diz o estado (gira trabalhando); o nome falado o leva em palavras.
                    div().id(SharedString::from(format!("group-row-{i}"))).flex().items_center().gap(px(12.)).mx(px(4.)).px(px(8.)).py(px(7.))
                        .rounded(px(9.)).cursor_pointer().hover(|el| el.bg(theme::hover()))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_target(&row, window, cx)))
                        .child(self.linked_avatar("row", t, s, 22.))
                        .child(div().w(px(360.)).flex_shrink_0().min_w_0().flex().flex_col()
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(name.clone()))
                            .child(div().flex().items_center().gap(px(5.)).min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.5)).line_height(px(15.)).text_color(theme::faint())
                                .child(chrome::small_icon(IconName::Monitor, 11., theme::faint()))
                                .child(div().flex_shrink_0().child(machines[i].clone()))
                                .child(dot()).child(div().flex_shrink_0().child(kind))
                                .when(!relations[i].is_empty(), |el| el.child(dot()).child(div().min_w_0().truncate().text_color(theme::accent_text()).child(relations[i].clone())))))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(last))
                        .child(Button::new(SharedString::from(format!("linked-child-open-{i}"))).ghost().xsmall()
                            .child(chrome::small_icon(IconName::ChevronRight, 14., theme::faint()))
                            .accessibility_label(format!("{open_label}: {name} · {state}"))
                            .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.open_target(&target, window, cx); })))
                }))
        });

        Some(linked_frame().child(header).children(list).into_any_element())
    }
}

/// Cartão do Hangar no material do compositor: borda, cantos de 12 e a fonte da interface.
fn linked_frame() -> Div {
    div().w_full().mb(px(4.)).rounded(px(12.)).border_1().border_color(theme::border_strong()).bg(theme::hover()).overflow_hidden()
        .font_family(theme::SANS).text_size(px(13.)).line_height(px(18.)).text_color(theme::text())
}
