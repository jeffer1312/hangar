//! Citar trecho de uma resposta e a bandeja de referências acima do campo de mensagem (citação e elemento do navegador).
//! O texto enviado leva os blocos de `context_refs` antes do que foi digitado, e a mensagem do usuário que volta da
//! conversa mostra cada bloco formatado; clicar numa citação volta ao trecho de origem.
use std::ops::Range;

use gpui_kit::base::{TextSelection, text::RangeHighlight};

use super::*;
use crate::context_refs::{self, ContextRef, Quote, Segment};

/// Quanto o trecho de origem fica realçado depois de voltar a ele.
const FLASH: Duration = Duration::from_millis(2400);
/// Quadros esperando a linha de origem ganhar texto (ela pode estar fora da tela até a rolagem chegar).
const FLASH_TRIES: u8 = 30;

/// O botão "Citar" sobre a seleção solta numa resposta; some quando a seleção some ou a conversa rola.
struct Offer { key: SessionKey, quote: Quote, at: Point<Pixels>, scroll: (usize, Pixels), focus: FocusHandle, _keys: Subscription }

/// Volta à origem de uma citação: a sessão, a linha, o trecho e quantos quadros ainda dá para esperar o texto dela.
struct Flash { key: SessionKey, row: String, quote: Quote, tries: u8, jumped: bool }

/// Uma referência da bandeja e o anexo do recorte dela (só o elemento escolhido tem): tirar a referência tira a foto.
#[derive(Clone)]
struct Held { r: ContextRef, shot: Option<u64> }

#[derive(Default)]
pub(super) struct Quotes {
    offer: Option<Offer>,
    /// Referências esperando o envio, por sessão, na ordem em que entraram.
    context_refs: HashMap<SessionKey, Vec<Held>>,
    /// Comentário em edição na bandeja: a sessão, a posição e o campo de uma linha.
    editing: Option<(SessionKey, usize, Entity<InputState>)>,
    flash: Option<Flash>,
    flash_seq: u64,
    /// A bandeja de antes de editar uma mensagem da fila; volta quando a edição acaba.
    saved: Option<(SessionKey, Vec<Held>)>,
}

/// O elemento sem o comentário: o mesmo elemento escolhido de novo é igual a ele mesmo comentado.
fn bare(element: &context_refs::Element) -> context_refs::Element { context_refs::Element { comment: String::new(), ..element.clone() } }

fn too_long(quote: &Quote) -> bool { quote.text.chars().count() > context_refs::TEXT_MAX }

/// Caracteres visíveis de um texto: compara a seleção da janela com a de uma mensagem sem depender de espaços e quebras.
fn visible_chars(text: &str) -> usize { text.chars().filter(|c| !c.is_whitespace()).count() }

fn window_selection_chars(window: &mut Window, cx: &mut App) -> usize {
    if !TextSelection::has_selection(window, cx) { return 0; }
    visible_chars(&TextSelection::selected_text(window, cx))
}

/// Só marcadores de anexo, sem legenda: o que `compose_prompt` cola depois dos blocos com " — ".
fn attachments_only(typed: &str) -> bool { !typed.trim().is_empty() && composer::caption(typed).is_empty() }

/// O texto que sai: os blocos antes do digitado. Sem referência, o digitado como está. Com só anexos depois dos blocos,
/// a mesma junção do `compose_prompt`, para a mensagem da fila editada sem mudança voltar igual.
pub(super) fn compose_refs(refs: &[ContextRef], typed: &str) -> String {
    if refs.is_empty() { return typed.to_owned(); }
    if attachments_only(typed) { return format!("{} — {}", context_refs::compose(refs, ""), typed.trim()); }
    context_refs::compose(refs, typed)
}

/// O texto digitado e as referências de uma mensagem já montada (a da fila que volta ao campo para editar); o inverso
/// de `compose_refs`.
pub(super) fn split_refs(text: &str) -> (Vec<ContextRef>, String) {
    let mut refs = Vec::new();
    let mut typed = String::new();
    for segment in context_refs::parse(text) {
        match segment { Segment::Ref(r) => refs.push(r), Segment::Text(t) => typed.push_str(&t) }
    }
    if refs.is_empty() { return (refs, text.to_owned()); }
    let typed = typed.trim();
    let typed = typed.strip_prefix('—').map(str::trim_start).filter(|rest| attachments_only(rest)).unwrap_or(typed);
    (refs, typed.to_owned())
}

/// Pedaço da mensagem do usuário com blocos, com o Markdown já pronto.
pub(super) enum Piece { Text(String), Quote(Quote, String), Element(context_refs::Element) }

/// A mensagem do usuário com citação ou elemento, lida uma vez no preparo, para o desenho não reler o texto a cada
/// quadro: os pedaços, cada um com a posição dele (a chave da vista), e se ela é longa.
pub(super) struct RefMessage { pieces: Vec<(usize, Piece)>, pub(super) long: bool }

/// Os pedaços da mensagem do usuário (`body`, o `display_body` dela) quando ela traz blocos; `None` na mensagem comum.
pub(super) fn ref_message(body: &str) -> Option<RefMessage> {
    if !body.contains("<hangar-ref") { return None; }
    let segments = context_refs::parse(body);
    if !segments.iter().any(|s| matches!(s, Segment::Ref(_))) { return None; }
    // Longa: o digitado ou alguma citação passa do limite de linhas.
    let typed: String = segments.iter().filter_map(|s| match s { Segment::Text(t) => Some(t.as_str()), _ => None }).collect();
    let long = long_message(typed.trim()) || segments.iter().any(|s| matches!(s, Segment::Ref(ContextRef::Quote(q)) if long_message(&q.text)));
    let pieces = segments.into_iter().enumerate().filter_map(|(n, segment)| Some((n, match segment {
        Segment::Text(text) if text.trim().is_empty() => return None,
        Segment::Text(text) => Piece::Text(safe_markdown(text.trim())),
        Segment::Ref(ContextRef::Quote(quote)) => { let markdown = safe_markdown(&quote.text); Piece::Quote(quote, markdown) }
        Segment::Ref(ContextRef::Element(element)) => Piece::Element(element),
    }))).collect();
    Some(RefMessage { pieces, long })
}

/// Texto de uma linha só (fila, trilho, prévias): cada bloco vira o rótulo curto dele, nunca o bloco cru.
pub(super) fn summary(text: &str) -> String {
    if !context_refs::has_refs(text) { return text.to_owned(); }
    context_refs::parse(text).into_iter().filter_map(|segment| match segment {
        Segment::Text(t) => Some(t.trim().to_owned()).filter(|t| !t.is_empty()),
        Segment::Ref(r @ ContextRef::Quote(_)) => Some(tr("ref_quote_short").replace("{label}", &r.label())),
        Segment::Ref(r) => Some(tr("ref_element_short").replace("{label}", &r.label())),
    }).collect::<Vec<_>>().join("\n")
}

fn decode_entities(text: &str) -> String { text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&") }

/// Cada link como a tela o mostra: a citação de arquivo vira `caminho:linha` (o texto que o plugin dela desenha) e os
/// outros links, o rótulo. Sem isso as palavras do endereço não casam com nada do texto desenhado.
fn as_shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(mid) = rest.find("](") {
        let close = rest[mid + 2..].find(')').map(|i| mid + 2 + i);
        // O `[` de abertura mais perto, fora de `\[` (o rótulo da citação de arquivo escapa colchetes).
        let open = rest[..mid].char_indices().rev().find(|&(i, c)| c == '[' && !rest[..i].ends_with('\\')).map(|(i, _)| i);
        let (Some(open), Some(close)) = (open, close) else { out.push_str(&rest[..mid + 2]); rest = &rest[mid + 2..]; continue };
        let (label, target) = (&rest[open + 1..mid], &rest[mid + 2..close]);
        out.push_str(&rest[..open]);
        match composer::citation_target(target) {
            Some((path, line)) => out.push_str(&composer::citation_text(&path, line)),
            None => out.push_str(&label.replace("\\[", "[").replace("\\]", "]")),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Palavras (letras e números seguidos) com o começo e o fim de cada uma no texto.
fn words(text: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => { out.push((from..at, text[from..at].to_owned())); start = None; }
            _ => {}
        }
    }
    out
}

/// Onde a sequência `needle` aparece: as palavras do meio iguais, e as das pontas podem ser o fim e o começo de uma
/// palavra maior, porque a seleção pode começar ou acabar no meio de uma.
fn matches_at(hay: &[(Range<usize>, String)], needle: &[String]) -> Vec<usize> {
    let k = needle.len();
    if k == 0 || hay.len() < k { return Vec::new(); }
    (0..=hay.len() - k).filter(|&i| needle.iter().enumerate().all(|(j, want)| {
        let word = &hay[i + j].1;
        match (j == 0, j == k - 1) {
            (true, true) => word.contains(want.as_str()),
            (true, false) => word.ends_with(want.as_str()),
            (false, true) => word.starts_with(want.as_str()),
            (false, false) => word == want,
        }
    })).collect()
}

/// Faixa no texto desenhado que corresponde a `range` da fonte Markdown: as mesmas palavras na mesma ordem, ignorando a
/// marcação entre elas. Quando elas se repetem, vale a mesma repetição da fonte.
fn rendered_range(rendered: &str, source: &str, range: Range<usize>) -> Option<Range<usize>> {
    let needle: Vec<String> = words(&decode_entities(&as_shown(source.get(range.clone())?))).into_iter().map(|(_, w)| w).collect();
    let before = matches_at(&words(&decode_entities(&as_shown(&source[..range.end]))), &needle).len().saturating_sub(1);
    let shown = words(rendered);
    let hits = matches_at(&shown, &needle);
    let at = *hits.get(before).or(hits.last())?;
    let (first, last) = (&shown[at], &shown[at + needle.len() - 1]);
    // Uma palavra só: o pedaço dela; senão, do fim da primeira ao começo da última.
    if needle.len() == 1 {
        let inner = first.1.find(needle[0].as_str())?;
        return Some(first.0.start + inner..first.0.start + inner + needle[0].len());
    }
    Some(first.0.end - needle[0].len()..last.0.start + needle[needle.len() - 1].len())
}

impl Hangar {
    /// Põe uma referência na bandeja da sessão; a mesma referência duas vezes entra uma só.
    pub fn add_context_ref(&mut self, key: &SessionKey, r: ContextRef, cx: &mut Context<Self>) {
        self.hold_context_ref(key, Held { r, shot: None }, cx);
    }

    fn hold_context_ref(&mut self, key: &SessionKey, held: Held, cx: &mut Context<Self>) {
        let list = self.quotes.context_refs.entry(key.clone()).or_default();
        if !list.iter().any(|h| h.r == held.r) { list.push(held); }
        cx.notify();
    }

    /// O texto que sai: os blocos da bandeja antes do digitado. Sem bandeja, o digitado como está.
    pub(super) fn compose_with_refs(&self, key: &SessionKey, typed: &str) -> String {
        compose_refs(&self.context_refs_snapshot(key), typed)
    }

    /// A bandeja como está agora: o que vai na mensagem do Enter, mesmo que ela só saia depois dos anexos subirem.
    pub(super) fn context_refs_snapshot(&self, key: &SessionKey) -> Vec<ContextRef> {
        self.quotes.context_refs.get(key).map(|list| list.iter().map(|h| h.r.clone()).collect()).unwrap_or_default()
    }

    /// O elemento já está na bandeja (com ou sem comentário).
    pub(super) fn has_element_ref(&self, key: &SessionKey, element: &context_refs::Element) -> bool {
        let element = bare(element);
        self.quotes.context_refs.get(key).is_some_and(|list| list.iter().any(|h| matches!(&h.r, ContextRef::Element(e) if bare(e) == element)))
    }

    /// Põe o elemento na bandeja com o anexo do recorte dele.
    pub(super) fn add_element_ref(&mut self, key: &SessionKey, element: context_refs::Element, shot: Option<u64>, cx: &mut Context<Self>) {
        self.hold_context_ref(key, Held { r: ContextRef::Element(element), shot }, cx);
    }

    pub(super) fn has_context_refs(&self, key: &SessionKey) -> bool { self.quotes.context_refs.get(key).is_some_and(|l| !l.is_empty()) }

    /// Envio confirmado: saem da bandeja só as referências que foram nele, pelo conteúdo; uma posta depois continua.
    pub(super) fn sent_context_refs(&mut self, key: &SessionKey, text: &str) {
        let unsent = |h: &Held| !text.contains(&h.r.serialize());
        // Editando uma mensagem da fila, a bandeja à vista é a dela: o envio confirmado levou a guardada.
        if let Some((_, saved)) = self.quotes.saved.as_mut().filter(|(k, _)| k == key) {
            saved.retain(unsent);
            return;
        }
        let Some(list) = self.quotes.context_refs.get_mut(key) else { return };
        list.retain(unsent);
        if list.is_empty() { self.quotes.context_refs.remove(key); }
        if self.quotes.editing.as_ref().is_some_and(|(k, i, _)| k == key && *i >= self.quotes.context_refs.get(key).map_or(0, Vec::len)) {
            self.quotes.editing = None;
        }
    }

    /// `/clear` trocou o transcript: a bandeja segue para a chave nova, como os anexos.
    pub(super) fn rekey_context_refs(&mut self, from: &SessionKey, to: &SessionKey) {
        if let Some(list) = self.quotes.context_refs.remove(from) { self.quotes.context_refs.insert(to.clone(), list); }
    }

    // ── Editar mensagem da fila: as referências dela vão para a bandeja, e a bandeja de antes volta no fim ──

    pub(super) fn begin_edit_refs(&mut self, key: &SessionKey, refs: Vec<ContextRef>) {
        let current = self.quotes.context_refs.remove(key).unwrap_or_default();
        if self.quotes.saved.as_ref().is_none_or(|(k, _)| k != key) { self.quotes.saved = Some((key.clone(), current)); }
        if !refs.is_empty() { self.quotes.context_refs.insert(key.clone(), refs.into_iter().map(|r| Held { r, shot: None }).collect()); }
        self.quotes.editing = None;
    }

    /// `keep`: a mensagem sumiu da fila e virou rascunho; as referências dela ficam na bandeja junto das de antes.
    pub(super) fn end_edit_refs(&mut self, key: &SessionKey, keep: bool) {
        let Some((saved_key, saved)) = self.quotes.saved.take() else { return };
        if &saved_key != key { self.quotes.saved = Some((saved_key, saved)); return; }
        let edited = self.quotes.context_refs.remove(key).unwrap_or_default();
        let mut list = if keep { edited } else { Vec::new() };
        for held in saved { if !list.iter().any(|h| h.r == held.r) { list.push(held); } }
        if !list.is_empty() { self.quotes.context_refs.insert(key.clone(), list); }
        self.quotes.editing = None;
    }

    // ── Botão Citar ──

    /// Linha que pode ser citada: resposta do agente já gravada, sem erro e fora da linha do tempo do orquestrador.
    fn citable(&self, row: &str) -> bool {
        row != PREVIEW && self.chat.events.iter().find(|e| e.id == row)
            .is_some_and(|e| e.kind == "assistant_msg" && e.is_error != Some(true) && e.orq.is_none())
    }

    /// A fonte desenhada da linha inteira (a mesma que vai ao `TextView` ou aos pedaços do `render_charted`).
    fn drawn_source(&self, row: &str) -> Option<String> {
        match self.prepared.get(row) {
            Some(Prepared::Message { markdown, .. }) => Some(markdown.clone()),
            _ => self.rich.get(row).map(|rich| rich.source.clone()),
        }
    }

    /// A fonte da resposta (o que o agente escreveu, `display_body`) e onde cada trecho do desenhado está nela. Sem o
    /// preparo, ou com ele de outra versão do texto, o próprio desenhado.
    fn cite_source(&self, row: &str, drawn: &str) -> (String, Option<std::rc::Rc<source_map::SourceMap>>) {
        let found = match self.prepared.get(row) {
            Some(Prepared::Message { markdown, map, .. }) if markdown == drawn => self.chat.events.iter().find(|e| e.id == row)
                .map(display_body).filter(|body| body.len() == map.source_len).map(|body| (body, map.clone())),
            _ => None,
        };
        match found { Some((body, map)) => (body, Some(map)), None => (drawn.to_owned(), None) }
    }

    /// Onde a fonte de um pedaço começa na fonte inteira; confere pelo texto, porque o pedaço foi remontado por linhas.
    fn piece_offset(&self, key: &str, piece: &str, full: &str) -> Option<usize> {
        if let Some(at) = self.rich.get(key).and_then(|rich| rich.offset) && full.get(at..at + piece.len()) == Some(piece) { return Some(at); }
        if full.starts_with(piece) && self.rich.get(key).is_some_and(|rich| rich.row == key) { return Some(0); }
        full.find(piece)
    }

    /// A citação da seleção atual, se ela está toda dentro de uma resposta que pode ser citada.
    fn cite_candidate(&self, window_chars: usize, cx: &App) -> Option<(SessionKey, Quote)> {
        if window_chars == 0 { return None; }
        let (mut row, mut range, mut chars): (Option<&str>, Option<Range<usize>>, usize) = (None, None, 0);
        let mut full = None;
        for (key, rich) in &self.rich {
            let view = rich.view.read(cx);
            let Some(selected) = view.selected_source_range().filter(|r| !r.is_empty()) else { continue };
            if row.is_some_and(|row| row != rich.row) { return None; }
            if row.is_none() { full = Some(self.drawn_source(&rich.row)?); }
            row = Some(&rich.row);
            let offset = self.piece_offset(key, &rich.source, full.as_deref()?)?;
            let global = selected.start + offset..selected.end + offset;
            range = Some(match range { Some(old) => old.start.min(global.start)..old.end.max(global.end), None => global });
            chars += visible_chars(&view.selected_text());
        }
        let row = row?;
        // A janela tem mais selecionado do que esta resposta: a seleção passa para outra linha.
        if window_chars > chars || !self.citable(row) { return None; }
        // A seleção é sobre o desenhado; a citação leva a fonte, sem os chips e escapes que o agente não escreveu.
        let (source, map) = self.cite_source(row, full.as_deref()?);
        let range = range?;
        let range = match &map { Some(map) => map.to_source(range), None => range };
        let quote = context_refs::quote_from(row, &source, range)?;
        Some((self.selected_key()?, quote))
    }

    /// Soltou o botão do mouse na conversa: a seleção só fica pronta depois deste evento.
    pub(super) fn selection_released(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left { return; }
        let at = event.position;
        cx.defer_in(window, move |this, window, cx| this.offer_cite(at, window, cx));
    }

    fn offer_cite(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let chars = window_selection_chars(window, cx);
        let Some((key, quote)) = self.cite_candidate(chars, cx) else {
            if self.quotes.offer.take().is_some() { self.redraw(panes::Area::Conversation, cx); }
            return;
        };
        let me = cx.entity().downgrade();
        // Tab leva ao botão e Esc fecha, como no T3, venha a tecla de onde vier; com o botão em foco, o Tab segue.
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if event.keystroke.modifiers.modified() { return; }
            let key = event.keystroke.key.clone();
            let _ = me.update(cx, |this, cx| {
                if this.offer_gone(window, cx) {
                    // O botão sumiu sem ninguém redesenhar: a tecla segue ao seu destino, e ele sai de vez.
                    if this.quotes.offer.is_some() { cx.defer_in(window, |this, _, cx| this.close_cite_offer(cx)); }
                    return;
                }
                let Some(offer) = this.quotes.offer.as_ref() else { return };
                match key.as_str() {
                    "tab" if !offer.focus.is_focused(window) && !too_long(&offer.quote) => {
                        offer.focus.clone().focus(window, cx);
                        cx.stop_propagation();
                    }
                    "escape" => {
                        cx.stop_propagation();
                        cx.defer_in(window, |this, _, cx| this.close_cite_offer(cx));
                    }
                    _ => {}
                }
            });
        });
        let top = self.list_state.logical_scroll_top();
        self.quotes.offer = Some(Offer { key, quote, at, scroll: (top.item_ix, top.offset_in_item), focus: cx.focus_handle(), _keys: keys });
        self.redraw(panes::Area::Conversation, cx);
    }

    fn close_cite_offer(&mut self, cx: &mut Context<Self>) {
        if self.quotes.offer.take().is_some() { self.redraw(panes::Area::Conversation, cx); }
    }

    /// "Citar" abre o modal da resposta ao trecho; a citação só entra na bandeja ao adicionar.
    fn cite(&mut self, key: SessionKey, quote: Quote, window: &mut Window, cx: &mut Context<Self>) {
        if too_long(&quote) { return; }
        self.quotes.offer = None;
        self.redraw(panes::Area::Conversation, cx);
        self.open_ref_editor(key, ContextRef::Quote(quote), window, cx);
    }

    /// O botão não vale mais: outra sessão, a conversa rolou ou a seleção sumiu.
    fn offer_gone(&self, window: &mut Window, cx: &mut App) -> bool {
        let Some(offer) = self.quotes.offer.as_ref() else { return true };
        let top = self.list_state.logical_scroll_top();
        Some(&offer.key) != self.selected_key().as_ref() || (top.item_ix, top.offset_in_item) != offer.scroll
            || !TextSelection::has_selection(window, cx)
    }

    /// O botão flutuante, preso onde o mouse soltou a seleção.
    pub(super) fn render_cite_offer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.quotes.offer.as_ref()?;
        if self.offer_gone(window, cx) { self.quotes.offer = None; return None; }
        let offer = self.quotes.offer.as_ref()?;
        let long = too_long(&offer.quote);
        let (label, aria) = if long { ("cite_shorten", "cite_too_long_aria") } else { ("cite_action", "cite_selection_aria") };
        let (label, aria) = (tr_shared(label, &[]), tr_shared(aria, &[]));
        let (key, quote) = (offer.key.clone(), offer.quote.clone());
        let (down_key, down_quote) = (key.clone(), quote.clone());
        let button = Button::new("cite-selection").small().outline().icon(chrome::small_icon(IconName::TextQuote, 14., theme::muted()))
            .label(label).accessibility_label(aria).disabled(long)
            .on_click(cx.listener(move |this, _, window, cx| this.cite(key.clone(), quote.clone(), window, cx)));
        let button = chrome::OwnFocus { id: "cite-selection".into(), focus: Some(offer.focus.clone()), button };
        Some(deferred(anchored().position(offer.at + point(px(6.), px(10.))).snap_to_window_with_margin(px(8.))
            .child(div().id("cite-offer").occlude().rounded(px(8.)).bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow())
                // Qualquer clique esquerdo limpa a seleção da janela antes de chegar aqui, e o botão sumiria antes de
                // soltar: o mouse cita ao descer, com a citação guardada ao soltar a seleção. O teclado segue no clique.
                .when(!long, |el| el.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.cite(down_key.clone(), down_quote.clone(), window, cx);
                })))
                .child(button))).with_priority(1).into_any_element())
    }

    /// Item "Citar seleção" do menu de contexto da mensagem `row`, com a seleção dentro dela.
    fn context_menu_cite(&self, row: &str, window_chars: usize, cx: &App) -> Option<(SessionKey, Quote)> {
        self.cite_candidate(window_chars, cx).filter(|(_, quote)| quote.source == row)
    }

    // ── Modal da resposta ao trecho ──

    /// Modal com o trecho e o campo da resposta. "Comentar e continuar" (Enter) põe na bandeja e devolve tudo como
    /// estava: a conversa na mesma posição e o foco onde estava (o kit o devolve ao fechar), para citar o próximo
    /// trecho. "Finalizar" (Ctrl+Enter) põe na bandeja, leva o foco ao campo de mensagem e a conversa ao fim, sem enviar.
    /// Shift+Enter quebra a linha; Esc e Cancelar fecham sem pôr nada.
    fn open_ref_editor(&mut self, key: SessionKey, item: ContextRef, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).submit_on_enter(true)
            .placeholder(tr_shared("cite_reply_placeholder", &[])));
        let shown = match &item {
            ContextRef::Quote(quote) => safe_markdown(&quote.text),
            ContextRef::Element(element) => format!("**{}**  \n`{}`", element.label(), element.url),
        };
        let excerpt = cx.new(|cx| TextViewState::markdown(&shown, cx));
        // Enter, "Comentar e continuar" e "Finalizar" gravam do mesmo jeito; `finish` diz para onde o foco vai depois.
        let owner = cx.entity().downgrade();
        let field = input.clone();
        let save = std::rc::Rc::new(move |finish: bool, window: &mut Window, cx: &mut App| {
            let comment = field.read(cx).value().to_string();
            let _ = owner.update(cx, |this, cx| this.save_ref(&key, item.clone(), &comment, finish, window, cx));
        });
        let on_enter = save.clone();
        window.subscribe(&input, cx, move |_, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary, shift: false } = event { on_enter(*secondary, window, cx); }
        }).detach();
        let focus = input.read(cx).focus_handle(cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let (finish, next) = (save.clone(), save.clone());
            super::popup::dialog(dialog).w(px(560.)).centered()
                .title(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(tr_shared("cite_reply_title", &[])))
                .child(div().id("ref-editor-excerpt").max_h(px(200.)).overflow_y_scroll().pl(px(10.)).py(px(2.)).border_l_2()
                    .border_color(theme::accent()).child(chat_text(&excerpt, cx).text_color(theme::muted())))
                .child(div().pt(px(12.)).child(Textarea::new(&input).accessibility_id("ref-editor-input")))
                .footer(div().w_full().flex().justify_end().gap_2()
                    .child(Button::new("ref-editor-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("ref-editor-finish").label(tr_shared("cite_finish", &[])).on_click(move |_, window, cx| finish(true, window, cx)))
                    .child(Button::new("ref-editor-continue").primary().label(tr_shared("cite_comment_continue", &[]))
                        .on_click(move |_, window, cx| next(false, window, cx))))
        });
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
    }

    /// Põe na bandeja com o comentário, limpa a seleção e fecha o modal. `finish`: depois, campo de mensagem e fim da conversa.
    fn save_ref(&mut self, key: &SessionKey, mut item: ContextRef, comment: &str, finish: bool, window: &mut Window, cx: &mut Context<Self>) {
        item.set_comment(comment);
        self.add_context_ref(key, item, cx);
        TextSelection::clear(window, cx);
        window.close_dialog(cx);
        if finish {
            // Depois do fechamento, que devolveria o foco a quem o tinha antes do modal.
            cx.defer_in(window, |this, window, cx| {
                this.composer.update(cx, |input, cx| input.focus(window, cx));
                this.follow_engage(cx);
            });
        }
        cx.notify();
    }

    // ── Bandeja ──

    /// Tira a referência da bandeja; a de um elemento leva junto o anexo do recorte dele.
    fn remove_context_ref(&mut self, key: &SessionKey, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut shot = None;
        if let Some(list) = self.quotes.context_refs.get_mut(key) && at < list.len() {
            shot = list.remove(at).shot;
            if list.is_empty() { self.quotes.context_refs.remove(key); }
        }
        self.quotes.editing = None;
        if let Some(id) = shot { self.remove_attachment(id, window, cx); }
        cx.notify();
    }

    fn edit_context_ref(&mut self, key: &SessionKey, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.quotes.context_refs.get(key).and_then(|list| list.get(at)).map(|h| h.r.comment().to_owned()) else { return };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("ref_comment_placeholder")));
        input.update(cx, |input, cx| { input.set_value(current, window, cx); input.focus(window, cx); });
        let (save_key, blur_key) = (key.clone(), key.clone());
        cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.save_ref_comment(&save_key, at, true, window, cx),
            InputEvent::Blur => this.save_ref_comment(&blur_key, at, false, window, cx),
            _ => {}
        }).detach();
        self.quotes.editing = Some((key.clone(), at, input));
        cx.notify();
    }

    /// Enter grava e devolve o foco ao campo de mensagem; sair do campo também grava.
    fn save_ref_comment(&mut self, key: &SessionKey, at: usize, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editing, index, input)) = self.quotes.editing.take() else { return };
        if &editing != key || index != at { self.quotes.editing = Some((editing, index, input)); return; }
        let comment = input.read(cx).value().to_string();
        if let Some(h) = self.quotes.context_refs.get_mut(key).and_then(|list| list.get_mut(at)) { h.r.set_comment(&comment); }
        if back { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
        cx.notify();
    }

    fn cancel_ref_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quotes.editing.take().is_some() {
            self.composer.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        }
    }

    /// Referências acima do campo, no padrão da bandeja de anexos: rótulo, lápis para o comentário e o X.
    pub(super) fn render_context_tray(&self, key: &SessionKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.quotes.context_refs.get(key).filter(|list| !list.is_empty())?;
        // Com o envio delas em voo, a bandeja fica como está até a resposta.
        let busy = self.uploading.contains_key(key) || self.delivery.pending(key);
        let editing = self.quotes.editing.as_ref().filter(|(k, _, _)| k == key).map(|(_, at, input)| (*at, input.clone()));
        let chips = list.iter().map(|h| &h.r).enumerate().map(|(at, r)| {
            let label = r.label();
            let (icon, kind) = match r { ContextRef::Quote(_) => (IconName::TextQuote, tr("ref_quote_title")), ContextRef::Element(_) => (IconName::Globe, tr_shared("cite_element", &[])) };
            let on = editing.as_ref().is_some_and(|(i, _)| *i == at);
            let (edit_key, remove_key) = (key.clone(), key.clone());
            div().id(SharedString::from(format!("context-ref-{at}"))).flex().items_center().gap(px(6.)).max_w(px(340.)).h(px(30.))
                .pl(px(9.)).pr(px(3.)).rounded(px(10.)).bg(theme::inset()).border_1()
                .border_color(if on { theme::accent() } else { theme::border() })
                .child(chrome::small_icon(icon, 14., theme::muted()))
                .child(div().min_w_0().flex_1().truncate().text_xs().text_color(theme::text()).child(label.clone()))
                .child(Button::new(SharedString::from(format!("context-ref-edit-{at}"))).ghost().xsmall()
                    .icon(chrome::small_icon(IconName::Pencil, 12., theme::muted())).disabled(busy)
                    .tooltip(tr("ref_comment")).accessibility_label(format!("{}: {kind} {label}", tr("ref_comment")))
                    .on_click(cx.listener(move |this, _, window, cx| this.edit_context_ref(&edit_key, at, window, cx))))
                .child(Button::new(SharedString::from(format!("context-ref-remove-{at}"))).ghost().xsmall()
                    .icon(chrome::small_icon(IconName::Close, 12., theme::muted())).disabled(busy)
                    .tooltip(tr("ref_remove").replace("{label}", &label)).accessibility_label(tr("ref_remove").replace("{label}", &label))
                    .on_click(cx.listener(move |this, _, window, cx| this.remove_context_ref(&remove_key, at, window, cx))))
        }).collect::<Vec<_>>();
        let editor = editing.map(|(_, input)| div().id("context-ref-comment").flex().items_center().gap_2()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { this.cancel_ref_comment(window, cx); cx.stop_propagation(); }))
            .child(div().flex_1().min_w_0().child(Input::new(&input).small().aria_label(tr("ref_comment_placeholder")))));
        Some(div().id("context-refs").flex().flex_col().gap(px(6.)).role(Role::Group).aria_label(tr("refs_tray"))
            .child(div().flex().flex_wrap().gap(px(6.)).children(chips))
            .children(editor)
            .into_any_element())
    }

    // ── Mensagem do usuário com referências ──

    /// O digitado como texto, a citação como trecho em destaque com o comentário fora, o elemento como cartão.
    pub(super) fn render_ref_segments(&mut self, id: &str, message: &RefMessage, clamp: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        message.pieces.iter().map(|(n, piece)| match piece {
            Piece::Text(markdown) => {
                let key = format!("{id}#u{n}");
                let view = self.text_view(&key, id, markdown.clone(), cx);
                collapse(chat_text(&view, cx).on_link_click(open_web_link)
                    .markdown_extensions(citation_extensions(&key, cx.weak_entity())), clamp, false).into_any_element()
            }
            Piece::Quote(quote, markdown) => self.render_quote(id, *n, quote, markdown, clamp, cx),
            Piece::Element(element) => self.render_element(id, *n, element, cx),
        }).collect()
    }

    fn render_quote(&mut self, id: &str, n: usize, quote: &Quote, markdown: &str, clamp: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("{id}#q{n}");
        let view = self.text_view(&key, id, markdown.to_owned(), cx);
        let comment = quote.comment.trim().to_owned();
        let has_source = !quote.source.is_empty();
        let (go, back) = (quote.clone(), quote.clone());
        let head = div().flex().items_center().gap(px(6.)).text_xs().text_color(theme::muted())
            .child(chrome::small_icon(IconName::TextQuote, 13., theme::muted())).child(tr("ref_quote_title"))
            .when(has_source, |el| el.child(Button::new(SharedString::from(format!("{key}-go"))).ghost().xsmall().label(tr("ref_quote_go"))
                .on_click(cx.listener(move |this, _, _, cx| this.go_to_quote(go.clone(), cx)))));
        let body = div().id(SharedString::from(format!("{key}-body"))).pl(px(10.)).border_l_2().border_color(theme::accent())
            .when(has_source, |el| el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.go_to_quote(back.clone(), cx))))
            .child(collapse(chat_text(&view, cx).on_link_click(open_web_link)
                .markdown_extensions(citation_extensions(&key, cx.weak_entity())), clamp, false).text_color(theme::muted()));
        div().flex().flex_col().gap(px(4.)).child(head).child(body)
            .when(!comment.is_empty(), |el| el.child(div().pt(px(2.)).whitespace_normal().child(comment)))
            .into_any_element()
    }

    fn render_element(&mut self, id: &str, n: usize, element: &context_refs::Element, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("{id}#e{n}");
        let open = self.expanded.contains(&key);
        let label = element.label();
        let toggle = key.clone();
        let html = (!element.html.is_empty()).then(|| Button::new(SharedString::from(format!("{key}-html"))).ghost().xsmall()
            .label(if open { tr("ref_element_html_hide") } else { tr_shared("cite_element_html", &[]) })
            .icon(if open { IconName::ChevronUp } else { IconName::ChevronDown })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_in_row(toggle.clone(), cx))));
        let comment = element.comment.trim().to_owned();
        div().flex().flex_col().gap(px(4.))
            .child(div().flex().flex_col().gap(px(3.)).px(px(10.)).py(px(8.)).rounded(px(10.)).bg(theme::inset()).border_1().border_color(theme::border())
                .child(div().flex().items_center().gap(px(6.)).min_w_0()
                    .child(chrome::small_icon(IconName::Globe, 13., theme::muted()))
                    .child(div().text_xs().text_color(theme::muted()).flex_shrink_0().child(tr_shared("cite_element", &[])))
                    .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(label)))
                .when(!element.title.is_empty(), |el| el.child(div().truncate().text_xs().text_color(theme::muted()).child(element.title.clone())))
                .child(div().truncate().font_family(theme::MONO).text_xs().text_color(theme::faint()).child(element.url.clone()))
                .when_some(html, |el, button| el.child(div().flex().child(button)))
                .when(open, |el| el.child(div().p(px(8.)).rounded(px(6.)).bg(theme::raised()).font_family(theme::MONO).text_xs()
                    .text_color(theme::muted()).whitespace_normal().child(element.html.clone()))))
            .when(!comment.is_empty(), |el| el.child(div().pt(px(2.)).whitespace_normal().child(comment)))
            .into_any_element()
    }

    /// Abre ou fecha um detalhe dentro da linha (`{id}#…`): só remede e redesenha ela.
    fn toggle_in_row(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) { self.expanded.insert(key.clone()); }
        if let Some(row) = self.row_ids.iter().position(|id| key.strip_prefix(id.as_str()).is_some_and(|rest| rest.starts_with('#'))) {
            self.list_state.remeasure_items(row..row + 1);
        }
        self.redraw(panes::Area::Conversation, cx);
    }

    // ── Voltar à origem ──

    /// Rola até a resposta citada e realça o trecho. Fora do histórico carregado, carrega o resto antes; sem a
    /// resposta na conversa, avisa e nada mais muda.
    fn go_to_quote(&mut self, quote: Quote, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        if !self.row_ids.iter().any(|id| *id == quote.source) && self.history_limit != 0 { self.load_all(cx); }
        self.quotes.flash = Some(Flash { key, row: quote.source.clone(), quote, tries: FLASH_TRIES, jumped: false });
        self.redraw(panes::Area::Conversation, cx);
    }

    fn quote_gone(&mut self, cx: &mut Context<Self>) {
        let Some(flash) = self.quotes.flash.take() else { return };
        self.action_feedback.insert(flash.key, (tr("ref_quote_gone"), true));
        cx.notify();
    }

    /// Passo da volta à origem, a cada desenho da conversa até o trecho estar pintado.
    pub(super) fn paint_quote_flash(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_key();
        let Some(flash) = self.quotes.flash.as_mut() else { return };
        // Outra conversa aberta no meio: a volta era da de antes.
        if selected.as_ref() != Some(&flash.key) { self.quotes.flash = None; return; }
        let pane = self.panes.conversation.entity_id();
        if !flash.jumped {
            match self.row_ids.iter().position(|id| *id == flash.row) {
                Some(row) => { flash.jumped = true; self.jump_to_row(row, cx); }
                // O histórico inteiro ainda está chegando: o fim da carga redesenha a conversa, sem quadro a quadro.
                None if self.loading => return,
                None => return self.quote_gone(cx),
            }
            cx.defer(move |cx| cx.notify(pane));
            return;
        }
        let row = flash.row.clone();
        let quote = flash.quote.clone();
        let views: Vec<(String, String, Entity<TextViewState>)> = self.rich.iter().filter(|(_, rich)| rich.row == row)
            .map(|(key, rich)| (key.clone(), rich.source.clone(), rich.view.clone())).collect();
        let ready = !views.is_empty() && views.iter().all(|(_, _, view)| !view.read(cx).rendered_text().is_empty());
        if !ready {
            let Some(flash) = self.quotes.flash.as_mut() else { return };
            flash.tries = flash.tries.saturating_sub(1);
            if flash.tries == 0 { self.quotes.flash = None; } else { cx.defer(move |cx| cx.notify(pane)); }
            return;
        }
        self.quotes.flash = None;
        let full = self.drawn_source(&row).unwrap_or_default();
        // O trecho é achado na fonte e levado ao desenhado, onde estão os pedaços.
        let (source, map) = self.cite_source(&row, &full);
        let located = context_refs::locate(&source, &quote).map(|found| match &map { Some(map) => map.to_drawn(found), None => found });
        let mut painted = Vec::new();
        let mut revealed = false;
        for (key, source, view) in views {
            let Some(offset) = self.piece_offset(&key, &source, &full) else { continue };
            let piece = offset..offset + source.len();
            let local = match &located {
                Some(found) if found.start < piece.end && found.end > piece.start =>
                    found.start.max(piece.start) - offset..found.end.min(piece.end) - offset,
                Some(_) => continue,
                // O trecho mudou ou sumiu da resposta: realça a resposta inteira, só para mostrar onde ela está.
                None => 0..source.len(),
            };
            let rendered = view.read(cx).rendered_text();
            let range = if located.is_some() { rendered_range(rendered.as_str(), &source, local) } else { None }
                .unwrap_or(0..rendered.len());
            let reveal = !revealed;
            revealed = true;
            view.update(cx, |state, cx| {
                let _ = state.set_range_highlights([RangeHighlight::new(range.clone(), theme::accent().opacity(0.32))], cx);
                if reveal { let _ = state.reveal_range(range.start..range.start, cx); }
            });
            painted.push(key);
        }
        self.quotes.flash_seq += 1;
        let seq = self.quotes.flash_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |this, cx| {
                if this.quotes.flash_seq != seq { return; }
                for key in &painted {
                    if let Some(rich) = this.rich.get(key) { rich.view.update(cx, |state, cx| state.clear_range_highlights(cx)); }
                }
            });
        }).detach();
    }
}

/// Menu de contexto da mensagem: "Citar seleção" quando a seleção está nela; desligado acima do limite.
pub(super) fn cite_menu_item(view: &WeakEntity<Hangar>, row: &str, window: &mut Window, cx: &mut App) -> Option<PopupMenuItem> {
    let chars = window_selection_chars(window, cx);
    let (key, quote) = view.upgrade()?.read(cx).context_menu_cite(row, chars, cx)?;
    let long = too_long(&quote);
    let view = view.clone();
    Some(PopupMenuItem::new(if long { tr_shared("cite_shorten", &[]) } else { tr("cite_selection") }).icon(IconName::TextQuote).disabled(long)
        .on_click(move |_, window, cx| { let _ = view.update(cx, |this, cx| this.cite(key.clone(), quote.clone(), window, cx)); }))
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{as_shown, rendered_range, split_refs, summary};
    use crate::context_refs::{self, ContextRef, Quote};

    #[test]
    fn rendered_range_skips_markup_and_keeps_the_repetition() {
        let source = "Use **dois** passos. Depois, `dois` passos de novo.";
        let rendered = "Use dois passos. Depois, dois passos de novo.";
        let second = source.rfind("`dois` passos").unwrap();
        let range = rendered_range(rendered, source, second..second + "`dois` passos".len()).unwrap();
        assert_eq!(&rendered[range.clone()], "dois passos");
        assert_eq!(range.start, rendered.rfind("dois").unwrap());
        let first = source.find("**dois**").unwrap();
        let range = rendered_range(rendered, source, first..first + "**dois** passos".len()).unwrap();
        assert_eq!(range.start, rendered.find("dois").unwrap());
        assert_eq!(&rendered[rendered_range(rendered, source, 0..2).unwrap()], "Us");
        // A seleção começou no meio de uma palavra.
        let cut = source.find("ois** passos").unwrap();
        assert_eq!(&rendered[rendered_range(rendered, source, cut..cut + "ois** passos".len()).unwrap()], "ois passos");
        assert_eq!(rendered_range(rendered, source, 0..0), None);
    }

    #[test]
    fn rendered_range_crosses_a_file_link_as_drawn() {
        // O texto desenhado da citação de arquivo é o caminho inteiro com a linha, não o rótulo.
        let source = "veja o [envio.rs:12](hangar-file:?path=src%2Fenvio.rs&line=12) e o [site](https://x.y/a) botão";
        let rendered = "veja o src/envio.rs:12 e o site botão";
        let from = source.find("o [").unwrap();
        let range = rendered_range(rendered, source, from..source.len()).unwrap();
        assert_eq!(&rendered[range], "o src/envio.rs:12 e o site botão");
        assert_eq!(as_shown(r"[a\[b\]](https://x)"), "a[b]");
        assert_eq!(as_shown("lista [x] sem link"), "lista [x] sem link");
    }

    #[test]
    fn rendered_range_reads_escaped_markup() {
        let source = "o tipo &lt;Vec&gt; volta";
        let rendered = "o tipo <Vec> volta";
        let range = rendered_range(rendered, source, 2..source.len()).unwrap();
        assert_eq!(&rendered[range], "tipo <Vec> volta");
    }

    #[test]
    fn split_and_summary_keep_the_typed_text_apart() {
        let quote = ContextRef::Quote(Quote { source: "ev".into(), before: String::new(), after: String::new(), text: "trecho citado".into(), comment: String::new() });
        let message = context_refs::compose(std::slice::from_ref(&quote), "  explique  ");
        let (refs, typed) = split_refs(&message);
        assert_eq!((refs, typed.as_str()), (vec![quote], "explique"));
        assert_eq!(split_refs("só texto"), (Vec::new(), "só texto".to_owned()));
        let line = summary(&message);
        assert!(line.contains("trecho citado") && line.ends_with("explique") && !line.contains("<hangar-"), "{line}");
        assert_eq!(summary("sem bloco"), "sem bloco");
    }
}
