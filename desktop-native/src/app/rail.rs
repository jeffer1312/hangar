//! Marcador de mensagens (`MessageRail` do Zeron): uma risca por pergunta do usuário na borda esquerda da conversa.
//! A da pergunta que está no topo da tela acende, o mouse em cima abre um cartão com o começo dela e da resposta, e o
//! clique leva até ela, mesmo fora do trecho desenhado da lista.
use super::*;
use super::panes::Area;

/// Abaixo desta largura da conversa o marcador some: encostaria no texto.
const MIN_WIDTH: f32 = 768.;
/// Altura de cada risca clicável e o vão entre elas.
const SLOT: f32 = 10.;
const GAP: f32 = 3.;
/// Folga acima e abaixo da pilha de riscas.
const MARGIN: f32 = 24.;
/// Pilha compacta em qualquer altura de janela: passando disso, cada risca vale um trecho da conversa.
const MAX_TICKS: usize = 12;
const PREVIEW_PROMPT_CHARS: usize = 160;
const PREVIEW_REPLY_CHARS: usize = 200;

/// Trechos `[início, fim)` das perguntas que cada risca representa: uma por risca enquanto couberem.
fn buckets(count: usize, capacity: usize) -> Vec<(usize, usize)> {
    if count == 0 { return Vec::new(); }
    let slots = capacity.clamp(1, count);
    (0..slots).map(|k| (k * count / slots, (k + 1) * count / slots)).collect()
}

fn preview(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { return flat; }
    format!("{}…", flat.chars().take(max - 1).collect::<String>().trim_end())
}

/// Primeira resposta com texto depois da pergunta `event`, antes da pergunta seguinte.
fn reply_after(events: &[ChatEvent], event: usize) -> Option<String> {
    events[event + 1..].iter().take_while(|e| e.kind != "user_msg" || peer_of(e).is_some())
        .filter(|e| e.kind == "assistant_msg").map(display_body).find(|text| !text.trim().is_empty())
}

impl Hangar {
    pub(super) fn render_rail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let viewport = self.list_state.viewport_bounds().size;
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        // Antes do primeiro layout a lista mede zero: aí desenha, e a medida certa decide no quadro seguinte.
        if width > 0. && width < MIN_WIDTH { return None; }
        let events = &self.chat.events;
        let queue = self.chat.queue_panel();
        // Recado de outra sessão não é pergunta sua, nem a mensagem que espera no painel "Na fila". Trocando de sessão,
        // o chat zera antes das linhas serem refeitas: índice que não existe mais é linha da sessão anterior.
        let rows: Vec<(usize, usize)> = self.conversation.items.iter().enumerate().filter_map(|(row, item)| match item {
            Item::Event(i) => events.get(*i).filter(|e| e.kind == "user_msg" && peer_of(e).is_none()
                && !crate::queue_panel::hidden(&e.id, &queue)).map(|_| (row, *i)),
            _ => None,
        }).collect();
        // Uma pergunta só não é navegação.
        if rows.len() < 2 { return None; }
        // Colada no fim, o topo lógico passa do último item e acende a última pergunta.
        let top = self.list_state.logical_scroll_top().item_ix;
        let active = rows.iter().rposition(|&(row, _)| row <= top).unwrap_or(0);
        let usable = (if height > 0. { height } else { 600. } - 2. * MARGIN).max(SLOT);
        let capacity = (((usable + GAP) / (SLOT + GAP)).floor() as usize).min(MAX_TICKS);
        let hover = self.rail_hover;
        // Com três perguntas o marcador vira atalho de verdade: a primeira risca leva a âncora da dica.
        let anchored_tip = rows.len() >= 3 && self.tour.tip_pending("rail");
        let ticks = buckets(rows.len(), capacity).into_iter().enumerate().map(|(ix, (start, end))| {
            let lit = (start..end).contains(&active);
            let hovered = hover == Some(ix);
            let (row, event) = rows[if lit { active } else { start }];
            let text = super::quotes::summary(&display_body(&events[event]));
            let text = if text.trim().is_empty() { activity::web("anexos_imagem_enviada") } else { text };
            let prompt = preview(&text, PREVIEW_PROMPT_CHARS);
            // Cartão do Zeron: título de uma linha, o começo da resposta em até três e, num trecho, quantas perguntas junta.
            let card = hovered.then(|| {
                let reply = reply_after(events, event).map(|reply| preview(&reply, PREVIEW_REPLY_CHARS));
                let body = div().id("hover-card-rail").w(px(280.)).p(px(12.)).flex().flex_col().gap(px(6.))
                    .child(div().text_size(px(12.)).line_height(px(18.)).text_color(theme::text()).truncate().child(prompt.clone()))
                    .when_some(reply, |el, reply| el.child(div().text_size(px(11.)).line_height(px(17.)).text_color(theme::muted())
                        .line_clamp(3).text_ellipsis().child(reply)))
                    .when(end - start > 1, |el| el.child(div().text_size(px(10.)).text_color(theme::faint())
                        .child(tr("rail_prompts").replace("{n}", &(end - start).to_string()))));
                deferred(anchored().anchor(gpui_kit::Anchor::LeftCenter).snap_to_window_with_margin(px(8.))
                    .child(div().pl(px(26.)).child(chrome::popover(body.into_any_element(), false))))
            });
            div().id(SharedString::from(format!("rail-{}", events[rows[start].1].id)))
                .relative().h(px(SLOT)).w_full().flex().items_center().cursor_pointer()
                .role(Role::Button).aria_label(prompt)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    // Sair de uma risca e entrar na vizinha chegam em qualquer ordem: a saída só apaga a própria.
                    let next = if *hovered { Some(ix) } else { this.rail_hover.filter(|&h| h != ix) };
                    if next != this.rail_hover { this.rail_hover = next; this.redraw(Area::Conversation, cx); }
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.jump_to_row(row, cx)))
                .child(div().h(px(2.)).w(px(if hovered { 20. } else { 12. })).rounded(px(1.))
                    .bg(if lit || hovered { theme::text().opacity(0.8) } else { theme::faint().opacity(0.5) }))
                .children(card)
                .when(ix == 0 && anchored_tip, |el| super::popup::anchor(el, crate::tour::anchors::RAIL))
        });
        Some(div().absolute().left(px(16.)).top_0().bottom_0().w(px(26.)).flex().flex_col().items_start().justify_center()
            .gap(px(GAP)).children(ticks).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn buckets_cover_every_question_once() {
        assert_eq!(super::buckets(3, 12), vec![(0, 1), (1, 2), (2, 3)]);
        let many = super::buckets(100, 12);
        assert_eq!((many.len(), many[0].0, many[11].1), (12, 0, 100));
        assert!(many.windows(2).all(|pair| pair[0].1 == pair[1].0));
        assert!(super::buckets(0, 12).is_empty());
    }
}
