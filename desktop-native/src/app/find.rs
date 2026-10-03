//! Ctrl+F na conversa aberta: busca no transcript inteiro (o que ainda não foi carregado vem na abertura), mostra
//! "n de N", Enter e Shift+Enter andam entre os achados e a linha do achado fica realçada. Esc fecha e devolve o foco.
use super::*;
use super::panes::Area;
use gpui_kit::base::text::RangeHighlight;

pub(super) struct Find {
    pub(super) open: bool,
    input: Entity<InputState>,
    query: String,
    /// Ids dos eventos que casam, na ordem da conversa.
    hits: Vec<String>,
    active: usize,
    /// Linha realçada: a do achado ativo.
    pub(super) row: Option<String>,
    /// Quantos eventos havia na última busca: chegou mais (histórico inteiro, mensagem nova), ela roda de novo.
    searched: usize,
    back: Option<FocusHandle>,
    /// O que cada texto da conversa já tem pintado (busca, se é o achado ativo, tamanho do texto): pintar de novo
    /// a cada quadro redesenharia o texto em laço.
    painted: HashMap<String, (String, bool, usize)>,
    /// Quadros que ainda esperam o texto do achado ativo aparecer para trazê-lo à vista; a linha recém-rolada só
    /// ganha texto no quadro seguinte. Achado em ferramenta, que não tem texto pintável, esgota e para.
    reveal: u8,
    /// O texto pesquisável de cada evento, em minúsculas: montado uma vez, não a cada tecla.
    texts: HashMap<String, String>,
}

impl Find {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("find_placeholder")));
        cx.subscribe(&input, |this: &mut Hangar, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) { this.find_changed(cx); }
        }).detach();
        Self { open: false, input, query: String::new(), hits: Vec::new(), active: 0, row: None, searched: 0, back: None,
            painted: HashMap::new(), reveal: 0, texts: HashMap::new() }
    }

    /// Outra conversa: fecha e esquece o que era da anterior.
    pub(super) fn reset(&mut self) {
        (self.open, self.row, self.hits, self.searched) = (false, None, Vec::new(), 0);
        self.texts.clear();
    }
}

/// O que a busca lê de um evento: o texto, os valores da entrada da ferramenta (sem as chaves do JSON) e o resultado.
fn haystack(event: &ChatEvent) -> String {
    fn values(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(text) => out.push(text.clone()),
            Value::Array(items) => for item in items { values(item, out); },
            Value::Object(items) => for item in items.values() { values(item, out); },
            Value::Null => {},
            other => out.push(other.to_string()),
        }
    }
    let mut parts = vec![event.text.clone().unwrap_or_default()];
    if let Some(input) = &event.tool_input { for value in input.values() { values(value, &mut parts); } }
    parts.push(event.result.clone().unwrap_or_default());
    parts.join("\n").to_lowercase()
}

/// Onde `needle` (já em minúsculas) aparece em `text`, sem diferenciar maiúsculas, em bytes de `text`.
pub(super) fn occurrences(text: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() { return out; }
    let mut from = 0;
    for (start, _) in text.char_indices() {
        if start < from { continue; }
        let mut want = needle.chars();
        let mut end = start;
        let mut matched = true;
        for (ix, ch) in text[start..].char_indices() {
            let Some(next) = want.next() else { break };
            if !ch.to_lowercase().eq(std::iter::once(next)) { matched = false; break; }
            end = start + ix + ch.len_utf8();
        }
        if matched && want.next().is_none() { out.push(start..end); from = end; }
    }
    out
}

/// A linha que mostra o evento `ix`: a mensagem, a ferramenta, o grupo ou o raciocínio que o contém.
fn covers(item: &Item, ix: usize) -> bool {
    match item {
        Item::Event(i) | Item::Orphan(i) => *i == ix,
        Item::Tool(tool) => tool.call == ix || tool.result == Some(ix),
        Item::Group { tools, .. } => tools.iter().any(|t| t.call == ix || t.result == Some(ix)),
        Item::Thinking { parts, .. } => parts.contains(&ix),
        Item::Tasks { .. } => false,
    }
}

impl Hangar {
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.as_ref().is_some_and(|s| s.readable()) { return; }
        if !self.find.open { (self.find.open, self.find.back) = (true, window.focused(cx)); }
        // Não depende do "Carregar anteriores" aparecer: na primeira página ele ainda está escondido.
        if self.history_limit != 0 { self.load_all(cx); }
        self.find.input.update(cx, |input, cx| input.focus(window, cx));
        self.redraw(Area::Conversation, cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        (self.find.open, self.find.row) = (false, None);
        self.find.texts.clear();
        if let Some(back) = self.find.back.take() { back.focus(window, cx); }
        self.redraw(Area::Conversation, cx);
    }

    fn find_run(&mut self) {
        let query = self.find.query.to_lowercase();
        let current = self.find.hits.get(self.find.active).cloned();
        self.find.searched = self.chat.events.len();
        let (events, texts) = (&self.chat.events, &mut self.find.texts);
        self.find.hits = if query.is_empty() { Vec::new() } else {
            events.iter().filter(|e| texts.entry(e.id.clone()).or_insert_with(|| haystack(e)).contains(&query)).map(|e| e.id.clone()).collect()
        };
        // O histórico antigo que chega entra na frente: o achado ativo continua o mesmo, só muda de número.
        let kept = current.and_then(|id| self.find.hits.iter().position(|h| *h == id));
        self.find.active = kept.unwrap_or(self.find.active).min(self.find.hits.len().saturating_sub(1));
    }

    fn find_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.find.input.read(cx).value().trim().to_owned();
        if query == self.find.query { return; }
        self.find.query = query;
        self.find_run();
        // Começa pelo mais recente, que é onde a conversa está.
        self.find.active = self.find.hits.len().saturating_sub(1);
        self.find.row = None;
        if !self.find.hits.is_empty() { self.find_go(cx); } else { self.redraw(Area::Conversation, cx); }
    }

    fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.find.hits.len() as isize;
        if count == 0 { return; }
        self.find.active = (self.find.active as isize + delta).rem_euclid(count) as usize;
        self.find_go(cx);
    }

    fn find_go(&mut self, cx: &mut Context<Self>) {
        // Achado sem linha visível não deixa o realce no achado anterior.
        self.find.row = None;
        let Some(id) = self.find.hits.get(self.find.active) else { return };
        let Some(ix) = self.chat.events.iter().position(|e| &e.id == id) else { return };
        let Some(row) = self.items.iter().position(|item| covers(item, ix)) else { return };
        self.find.row = self.row_ids.get(row).cloned();
        self.find.reveal = 8;
        self.jump_to_row(row, cx);
    }

    /// Pinta a palavra buscada em cada texto da conversa já desenhado; no achado ativo, a primeira ocorrência mais
    /// forte e trazida à vista. Fechada a busca, apaga o que pintou.
    fn find_paint(&mut self, cx: &mut Context<Self>) {
        let query = if self.find.open { self.find.query.to_lowercase() } else { String::new() };
        let (rich, find) = (&self.rich, &mut self.find);
        find.painted.retain(|key, _| rich.contains_key(key));
        for (key, text) in rich {
            let active = find.open && find.row.as_deref() == Some(text.row.as_str());
            let rendered = text.view.read(cx).rendered_text();
            let stamp = (query.clone(), active, rendered.as_str().len());
            let reveal = active && find.reveal > 0;
            if find.painted.get(key) == Some(&stamp) && !reveal { continue; }
            if query.is_empty() && !find.painted.contains_key(key) { continue; }
            let ranges = occurrences(rendered.as_str(), &query);
            text.view.update(cx, |state, cx| {
                if ranges.is_empty() { state.clear_range_highlights(cx); return; }
                let strong = theme::warning().opacity(0.55);
                let soft = theme::warning().opacity(0.22);
                let _ = state.set_range_highlights(ranges.iter().enumerate()
                    .map(|(ix, range)| RangeHighlight::new(range.clone(), if active && ix == 0 { strong } else { soft })), cx);
                if reveal { let _ = state.reveal_range(ranges[0].clone(), cx); }
            });
            if reveal { find.reveal = 0; }
            if query.is_empty() { find.painted.remove(key); } else { find.painted.insert(key.clone(), stamp); }
        }
        if self.find.reveal > 0 {
            self.find.reveal -= 1;
            // Daqui o desenho da conversa ainda está em curso: o aviso só agenda o próximo quadro depois dele.
            let pane = self.panes.conversation.entity_id();
            cx.defer(move |cx| cx.notify(pane));
        }
    }

    pub(super) fn render_find(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.find.painted.is_empty() || self.find.open { self.find_paint(cx); }
        if !self.find.open { return None; }
        if self.find.searched != self.chat.events.len() && !self.find.query.is_empty() { self.find_run(); }
        let count = self.find.hits.len();
        let status = if self.find.query.is_empty() { String::new() }
            else if count == 0 && self.loading { tr("find_loading") }
            else if count == 0 { tr("find_none") }
            else { tr("find_count").replace("{n}", &(self.find.active + 1).to_string()).replace("{total}", &count.to_string()) };
        Some(div().id("find-bar").absolute().top(px(8.)).right(px(24.)).w(px(360.)).p(px(4.)).flex().items_center().gap_1()
            .rounded(px(10.)).border_1().border_color(theme::glass_border()).bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow())
            .occlude()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.close_find(window, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.find_step(-1, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.find_step(1, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" { this.find_step(if event.keystroke.modifiers.shift { -1 } else { 1 }, cx); }
            }))
            .child(div().flex_1().min_w_0().child(Input::new(&self.find.input).small().aria_label(tr("find_placeholder"))
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))))
            .child(div().id("find-status").role(Role::Status).flex_none().px_1().text_xs().text_color(theme::muted()).child(status))
            .child(chrome::icon_button("find-prev", IconName::ChevronUp, tr("find_prev"), cx).disabled(count == 0)
                .on_click(cx.listener(|this, _, _, cx| this.find_step(-1, cx))))
            .child(chrome::icon_button("find-next", IconName::ChevronDown, tr("find_next"), cx).disabled(count == 0)
                .on_click(cx.listener(|this, _, _, cx| this.find_step(1, cx))))
            .child(chrome::icon_button("find-close", IconName::Close, tr("find_close"), cx)
                .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn occurrences_ignore_case_and_keep_byte_offsets() {
        let text = "Área MEWS e mews; ÁREA";
        assert_eq!(super::occurrences(text, "mews").iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["MEWS", "mews"]);
        assert_eq!(super::occurrences(text, "área").iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(), ["Área", "ÁREA"]);
        assert!(super::occurrences(text, "").is_empty());
    }
}
