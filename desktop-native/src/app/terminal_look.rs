//! Pele Terminal: a chamada como o Claude Code desenha no terminal. "● Update(arquivo)", a linha do desfecho com o
//! cotovelo e, nas edições, o diff com uma numeração só, a do arquivo.
use super::*;
use crate::editdiff::{self, Edit, Op};

#[derive(Default)]
struct BodyGesture {
    origin: Option<Point<Pixels>>,
    dragged: bool,
    epoch: u64,
}

type BodyCollapse = std::rc::Rc<dyn Fn(&String, &mut Window, &mut App)>;

#[derive(IntoElement)]
struct ToolBodySurface {
    key: String,
    body: AnyElement,
    on_collapse: BodyCollapse,
}

impl RenderOnce for ToolBodySurface {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // A seleção redesenha entre pressionar e soltar; o gesto precisa sobreviver a esse quadro.
        let id = SharedString::from(format!("term-body-{}", self.key));
        let gesture = window.use_keyed_state(id.clone(), cx, |_, _| BodyGesture::default());
        let (down, movement) = (gesture.clone(), gesture.clone());
        div().id(id)
            // O arrasto pode sair do corpo e voltar; a janela observa sem tomar a seleção do filho.
            .child(canvas(|_, _, _| (), move |_, _, window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || event.pressed_button != Some(MouseButton::Left) { return; }
                    movement.update(cx, |gesture, _| {
                        if gesture.origin.is_some_and(|origin| (event.position - origin).magnitude() > 3.) {
                            gesture.dragged = true;
                        }
                    });
                });
            }).size(px(0.)))
            .child(self.body)
            .on_mouse_down(MouseButton::Left, move |event, _, cx| down.update(cx, |gesture, _| {
                gesture.origin = Some(event.position);
                gesture.dragged = false;
                gesture.epoch += 1;
            }))
            .on_click(move |event, window, cx| {
                let ClickEvent::Mouse(event) = event else { return; };
                if event.down.button != MouseButton::Left || event.up.button != MouseButton::Left
                    || event.down.click_count != 1 || event.up.click_count != 1
                    || event.down.modifiers != Modifiers::default() || event.up.modifiers != Modifiers::default()
                    || gesture.read(cx).dragged || (event.up.position - event.down.position).magnitude() > 3.
                    || gpui_kit::base::TextSelection::has_selection(window, cx) { return; }
                // A primeira soltura também chega no duplo clique; esperar mantém o texto montado para selecionar.
                let (epoch, pending, owner) = (gesture.read(cx).epoch, gesture.downgrade(), window.window_handle());
                let (on_collapse, key) = (self.on_collapse.clone(), self.key.clone());
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let _ = owner.update(cx, |_, window, cx| {
                        let Some(pending) = pending.upgrade() else { return; };
                        if pending.read(cx).epoch == epoch && !pending.read(cx).dragged
                            && !gpui_kit::base::TextSelection::has_selection(window, cx) {
                            on_collapse(&key, window, cx);
                        }
                    });
                }).detach();
            })
    }
}

/// Superfície do corpo aberto; não transforma texto selecionável em botão.
fn terminal_tool_body(
    key: String,
    body: impl IntoElement,
    on_collapse: impl Fn(&String, &mut Window, &mut App) + 'static,
) -> Div {
    div().child(ToolBodySurface { key, body: body.into_any_element(), on_collapse: std::rc::Rc::new(on_collapse) })
}

pub(super) fn tool_body_control(
    look: appearance::ToolLook,
    key: &str,
    control: impl IntoElement,
) -> AnyElement {
    if look != appearance::ToolLook::Terminal { return control.into_any_element(); }
    // Só o clique do controle para aqui; seleção, foco e teclado continuam com o filho.
    div().id(SharedString::from(format!("term-control-{key}"))).child(control)
        .on_click(|_, _, cx| cx.stop_propagation()).into_any_element()
}

fn thinking_markdown(view: &Entity<TextViewState>, cx: &App) -> gpui_kit::base::TextView {
    let dim = theme::muted();
    let style = theme::conversation_markdown(cx).with_foreground(dim).with_muted_foreground(dim)
        .with_link(dim).with_list_style(StyleRefinement::default().text_color(dim))
        .with_inline_code(HighlightStyle { color: Some(dim), ..Default::default() });
    gpui_kit::base::TextView::new(view).selectable(true).scrollable(false).style(style)
}

/// Raciocínio terminado: o brain no lugar do `∴`, parado, centrado na primeira linha como o ponto do texto.
fn terminal_thinking_text(view: &Entity<TextViewState>, cx: &App) -> Div {
    let dim = theme::muted();
    div().flex().items_start().gap(px(chrome::THINKING_GAP)).pl(px(INDENT - 8.)).text_color(dim)
        .child(div().flex_none().h(px(conversation_line(false))).flex().items_center()
            .child(chrome::small_icon(IconName::Brain, chrome::THINKING_ICON, dim)))
        .child(div().flex_1().min_w_0().child(thinking_markdown(view, cx)))
}

/// Raciocínio recolhido: o brain e a primeira frase numa linha, num botão que abre o texto inteiro.
fn terminal_thinking_fold(key: String, summary: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    let dim = theme::muted();
    Button::new(SharedString::from(format!("term-thought-{key}"))).ghost().w_full().h_auto().p_0().justify_start()
        .accessibility_label(format!("{}: {summary}", tr("thinking")))
        .child(div().w_full().flex().items_center().gap(px(chrome::THINKING_GAP)).pl(px(INDENT - 8.)).text_color(dim)
            .child(div().flex_none().h(px(conversation_line(false))).flex().items_center()
                .child(chrome::small_icon(IconName::Brain, chrome::THINKING_ICON, dim)))
            .child(div().flex_1().min_w_0().truncate().italic().child(summary)))
        .on_click(on_click).into_any_element()
}

/// Raciocínio chegando na pele Terminal: "brain Pensando" com o brilho do T3 e, embaixo, o texto alinhado ao rótulo.
fn terminal_live_thinking(view: &Entity<TextViewState>, focus: Option<AnyElement>, cx: &App) -> Div {
    div().flex().flex_col().gap(px(4.))
        .child(div().flex().pl(px(INDENT - 8.)).child(chrome::thinking_label_row(tr("thinking_live").into(), focus)))
        .child(div().pl(px(INDENT - 8. + chrome::THINKING_ICON + chrome::THINKING_GAP)).text_color(theme::muted())
            .child(thinking_markdown(view, cx)))
}

pub(super) fn live_thinking_content(
    look: appearance::ToolLook,
    view: &Entity<TextViewState>,
    focus: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    if look == appearance::ToolLook::Terminal { return terminal_live_thinking(view, focus, cx).into_any_element(); }
    div().flex().flex_col().gap_1().px_3().py_2()
        .child(div().flex().items_center().gap(px(6.)).text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::accent())
            .child(chrome::small_icon(IconName::Brain, 14., theme::accent()))
            .child(tr("thinking_live")))
        .child(chat_text(view, cx).text_sm().text_color(theme::muted()))
        .into_any_element()
}

#[cfg(test)]
mod interaction_tests;

/// Linhas de um arquivo novo antes do "… +N linhas": o mesmo corte do Claude Code.
pub(super) const WRITE_SHOWN: usize = 10;
/// Linhas de uma edição antes do "… +N linhas": acima disso o cabeçalho expande.
pub(super) const EDIT_SHOWN: usize = 40;
pub(super) const INDENT: f32 = 22.;

/// O ponto do Claude Code: 7 px, centrado numa linha de `line` px. A coluna tem a altura da linha para o ponto
/// ficar na primeira, mesmo com o texto quebrando.
pub(super) fn bullet(color: Hsla, line: f32) -> Div {
    div().flex_none().w(px(7.)).h(px(line)).flex().items_center().child(div().size(px(7.)).rounded_full().bg(color))
}

/// Começo da linha dobrada: o ponto enquanto alguma chamada roda; terminada, só o espaço dele, como o Claude Code.
pub(super) fn fold_lead(live: bool) -> Div {
    if live { bullet(theme::accent(), 19.).debug_selector(|| "term-fold-dot".into()) }
    else { div().flex_none().w(px(7.)).h(px(19.)) }
}

/// Recuo das linhas `⎿` da dobra: o ponto (7 px) e o vão (8 px) da linha de cima, para o `⎿` cair sob o título.
pub(super) const FOLD_SUB_INDENT: f32 = 7. + 8.;

/// Linha de cima da dobra: o ponto e o título num botão, que foca pelo teclado, ativa com Enter/Espaço e leva o rótulo,
/// como o cabeçalho da chamada.
pub(super) fn fold_header(id: SharedString, title: String, open: bool, live: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    div().flex().gap(px(8.)).child(fold_lead(live))
        .child(Button::new(id).ghost().flex_1().min_w_0().h_auto().p_0().justify_start().toggled(open)
            .accessibility_label(title.clone())
            .child(div().w_full().font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.)).text_color(theme::muted()).child(title))
            .on_click(on_click)).into_any_element()
}

/// Linha `⎿` da dobra recolhida: o que roda agora ou a saída de uma falha.
pub(super) fn fold_sub(text: String, color: Hsla) -> AnyElement {
    div().flex().gap(px(8.)).pl(px(FOLD_SUB_INDENT)).font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.))
        .child(div().flex_none().text_color(theme::faint()).child("⎿"))
        .child(div().min_w_0().truncate().text_color(color).child(text)).into_any_element()
}

/// Texto do agente na pele Terminal: o ponto branco antes do primeiro parágrafo, como o "● texto" do Claude Code.
pub(super) fn assistant_text_row(content: Div, line: f32) -> Div {
    div().w_full().flex().items_start().gap(px(8.))
        .child(bullet(theme::text(), line).debug_selector(|| "term-text-dot".into()))
        .child(content.flex_1().min_w_0())
}

/// Edit, MultiEdit e o patch do Codex aparecem como "Update", e MCP como "servidor - ferramenta (MCP)": os nomes que
/// o terminal usa.
pub(super) fn display_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "edit" | "multiedit" | "notebookedit" | "apply_patch" => "Update".into(),
        _ => match mcp_tool(name) {
            Some((server, tool)) => format!("{server} - {tool} (MCP)"),
            None => conversation::tool_display_name(name),
        },
    }
}

/// Ferramentas que agem sobre um arquivo: só nelas o `path` é o arquivo e o cabeçalho mostra o nome dele.
const FILE_TOOLS: [&str; 7] = ["read", "notebookread", "edit", "multiedit", "notebookedit", "write", "apply_patch"];

/// O arquivo que a chamada cita, pela mesma busca do `editdiff` (lista vale o primeiro item); vazio é "sem arquivo".
fn input_path(input: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<String> {
    Some(editdiff::input_path(input)).filter(|p| !p.is_empty()).map(str::to_owned)
}

/// O argumento do cabeçalho e, quando é um nome de arquivo, o caminho inteiro para o tooltip. Busca (Grep, Glob) tem
/// `path` de diretório: fica com o resumo de sempre.
pub(super) fn header_arg(name: Option<&str>, input: Option<&serde_json::Map<String, serde_json::Value>>) -> (String, Option<String>) {
    let file_tool = name.is_some_and(|n| FILE_TOOLS.contains(&n.to_ascii_lowercase().as_str()));
    match file_tool.then(|| input_path(input)).flatten() {
        Some(path) => (crate::composer::basename(&path).to_owned(), Some(path)),
        None => (conversation::summarize_input(name, input), None),
    }
}

fn count(one: &str, many: &str, n: usize) -> String { count_with(one, many, n, &[]) }

fn count_with(one: &str, many: &str, n: usize, extra: &[(&str, &str)]) -> String {
    let n_text = n.to_string();
    let params: Vec<(&str, &str)> = std::iter::once(("n", n_text.as_str())).chain(extra.iter().copied()).collect();
    crate::i18n::tr_shared(if n == 1 { one } else { many }, &params)
}

/// "2 linhas adicionadas, 1 removida", ou a linha do arquivo novo.
pub(super) fn edit_summary(edits: &[Edit], created: bool) -> String {
    let (added, removed) = editdiff::totals(edits);
    if created {
        let key = if added == 1 { "term_gravou_1" } else { "term_gravou_n" };
        let file = edits.first().map_or("", |e| crate::composer::basename(&e.path));
        return crate::i18n::tr_shared(key, &[("n", &added.to_string()), ("arquivo", file)]);
    }
    match (added, removed) {
        (0, 0) => activity::web("editdiff_sem_mudanca"),
        (a, 0) => count("term_adicionadas_1", "term_adicionadas_n", a),
        (0, r) => count("term_so_removidas_1", "term_so_removidas_n", r),
        (a, r) => format!("{}, {}", count("term_adicionadas_1", "term_adicionadas_n", a), count("term_removidas_1", "term_removidas_n", r)),
    }
}

/// A cor do fundo e do marcador de uma linha: verde na adicionada, vermelho na removida, nenhuma no contexto.
fn tint(op: Op) -> Option<Hsla> {
    match op { Op::Add => Some(theme::success()), Op::Del => Some(theme::removed()), Op::Same => None }
}

/// Linha do diff no desenho do terminal: um número, o marcador e a linha inteira com fundo.
fn line(op: Op, number: Option<u32>, width: usize, code: impl IntoElement) -> Div {
    let (marker, tint) = (match op { Op::Add => "+", Op::Del => "-", Op::Same => " " }, tint(op));
    let ink = tint.unwrap_or_else(|| theme::faint().opacity(0.8));
    div().w_full().flex().font_family(theme::MONO).text_size(px(12.)).line_height(px(19.))
        .when_some(tint, |el, tint| el.bg(tint.opacity(0.14)))
        .child(div().flex_none().w(px(width as f32 * 7.5 + 8.)).flex().justify_end().text_color(ink).child(number.map(|n| n.to_string()).unwrap_or_default()))
        .child(div().flex_none().w(px(18.)).flex().justify_center().text_color(ink).child(marker))
        .child(div().flex_1().min_w_0().text_color(theme::text().opacity(0.92)).child(code))
}

/// O que separa uma edição da anterior: nada na primeira, o intervalo `…` no mesmo arquivo, o nome do arquivo quando muda.
#[derive(Debug, PartialEq, Eq)]
enum Between { Nothing, Gap, File(String) }

fn between(previous: Option<&str>, current: &str) -> Between {
    match previous {
        None => Between::Nothing,
        Some(p) if p == current => Between::Gap,
        Some(_) => Between::File(crate::composer::basename(current).to_owned()),
    }
}

/// Linha sem número, marcador nem fundo: o intervalo e o nome do arquivo, alinhados ao código.
fn note_row(text: String, tip: Option<(String, String)>) -> AnyElement {
    let row = div().w_full().font_family(theme::MONO).text_size(px(12.)).line_height(px(19.)).text_color(theme::muted());
    match tip {
        Some((id, path)) => row.id(SharedString::from(id))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx))
            .child(text).into_any_element(),
        None => row.child(text).into_any_element(),
    }
}

/// Reparte o orçamento de linhas entre as edições, na ordem: cada uma leva o que cabe e o resto vai para as seguintes.
fn split_budget(lens: &[usize], budget: usize) -> Vec<usize> {
    let mut left = budget;
    lens.iter().map(|len| { let take = (*len).min(left); left -= take; take }).collect()
}

/// As edições que têm linha a desenhar, com a posição de cada uma. Uma sem linhas não esconde as seguintes: o
/// orçamento de linhas só some quando acaba, e aí as que sobram também têm `take == 0`.
fn visible_edits(takes: &[usize]) -> Vec<(usize, usize)> {
    takes.iter().copied().enumerate().filter(|(_, take)| *take > 0).collect()
}

/// O corpo de uma edição: os trechos, um embaixo do outro, nunca mais que `SHOWN_MAX` linhas de diff. `limit` corta o
/// arquivo novo e a edição recolhidos. Devolve as linhas, quantas ficaram de fora e quantas linhas de diff a chamada tem no total.
pub(super) fn diff_body(call: &ChatEvent, edits: &[Edit], limit: Option<usize>, cx: &App) -> (Vec<AnyElement>, usize, usize) {
    let painted = super::edits::painted(call, edits, cx);
    let total: usize = edits.iter().map(|e| e.lines.len()).sum();
    let takes = split_budget(&edits.iter().map(|e| e.lines.len()).collect::<Vec<_>>(), limit.unwrap_or(usize::MAX).min(super::edits::SHOWN_MAX));
    let visible = visible_edits(&takes);
    let width = edits.iter().zip(&takes).flat_map(|(e, t)| &e.lines[..*t]).filter_map(|l| l.new.max(l.old)).max().unwrap_or(1).to_string().len();
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut previous: Option<&str> = None;
    for (n, take) in visible {
        let edit = &edits[n];
        match between(previous, &edit.path) {
            Between::Nothing => {}
            Between::Gap => rows.push(note_row("…".into(), None)),
            Between::File(name) => rows.push(note_row(name, Some((format!("term-file-{}-{n}", call.id), edit.path.clone())))),
        }
        previous = Some(&edit.path);
        let new_marks = &painted[n].1;
        rows.extend(edit.lines[..take].iter().zip(&edit.spans).map(|(l, spans)| {
            let tint = tint(l.op);
            let len = l.text.len();
            let words = tint.into_iter().flat_map(|tint| spans.iter().filter(|r| r.start < len).map(move |r| (r.start..r.end.min(len),
                HighlightStyle { background_color: Some(tint.opacity(0.3)), ..Default::default() })));
            // Como no terminal, a removida fica sem cor de sintaxe: só o vermelho e o trecho trocado.
            let syntax = l.new.filter(|_| l.op != Op::Del).and_then(|no| new_marks.get(no.saturating_sub(edit.start.1) as usize)).into_iter().flatten()
                .filter(|(r, _)| r.start < len).map(|(r, s)| (r.start..r.end.min(len), *s));
            let highlights = gpui::combine_highlights(syntax, words).collect::<Vec<_>>();
            line(l.op, l.new.or(l.old), width, StyledText::new(l.text.clone()).with_highlights(highlights)).into_any_element()
        }));
    }
    (rows, total - takes.iter().sum::<usize>(), total)
}

/// O que uma chamada conta na linha dobrada; a mesma regra do `terminalFoldKind` do core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FoldKind { Search, Read, List, Mcp, Shell }

// Os conjuntos do Claude Code: comando de shell cujas partes só usam estes conta como busca, leitura ou listagem; os
// neutros não decidem nada.
const SHELL_SEARCH: [&str; 8] = ["find", "grep", "rg", "ag", "ack", "locate", "which", "whereis"];
const SHELL_READ: [&str; 15] = ["cat", "head", "tail", "less", "more", "wc", "stat", "file", "strings", "jq", "awk", "cut", "sort", "uniq", "tr"];
const SHELL_LIST: [&str; 3] = ["ls", "tree", "du"];
const SHELL_NEUTRAL: [&str; 5] = ["echo", "printf", "true", "false", ":"];

/// Se a parte grava ou roda outra coisa por baixo, ela não é leitura e a dobra não pode escondê-la: redirecionamento de
/// saída (fora `/dev/null` e a cópia de descritor `2>&1`), substituição de comando, ou as opções de `find`/`sort` que
/// apagam, executam ou gravam. Espelho do `parteGrava` do core.
fn part_writes(part: &str) -> bool {
    let b = part.as_bytes();
    let (mut i, mut quote) = (0, None::<u8>);
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && quote != Some(b'\'') { i += 2; continue; }
        if quote == Some(b'\'') { if c == b'\'' { quote = None; } i += 1; continue; }
        if c == b'`' || (c == b'$' && b.get(i + 1) == Some(&b'(')) { return true; }
        if quote == Some(b'"') { if c == b'"' { quote = None; } i += 1; continue; }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'<' if b.get(i + 1) == Some(&b'(') => return true,
            b'>' => {
                let mut j = i + 1;
                if matches!(b.get(j), Some(b'>' | b'|')) { j += 1; }
                let dup = b.get(j) == Some(&b'&');
                if dup { j += 1; }
                while matches!(b.get(j), Some(b' ' | b'\t')) { j += 1; }
                let end = b[j..].iter().position(|c| c.is_ascii_whitespace() || b";&|<>()".contains(c)).map_or(b.len(), |n| j + n);
                let target = &part[j..end];
                let fd = target == "-" || (!target.is_empty() && target.bytes().all(|c| c.is_ascii_digit()));
                if target != "/dev/null" && !(dup && fd) { return true; }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let mut words = part.split_whitespace();
    match words.next() {
        Some("find") => words.any(|w| matches!(w, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fls") || w.starts_with("-fprint")),
        Some("sort") => words.any(|w| w.starts_with("--o") || (w.starts_with('-') && !w.starts_with("--") && w.contains('o'))),
        _ => false,
    }
}

/// Classifica o comando parte a parte, separando em `|`, `&`, `;` e quebra de linha fora de aspas e parênteses, como o
/// `separarComando` do core. Para no primeiro comando que não é de leitura: roda a cada quadro e não aloca.
fn shell_fold_kind(cmd: &str) -> FoldKind {
    let (mut search, mut read, mut list) = (false, false, false);
    let mut classify = |part: &str| {
        if part_writes(part) { return false; }
        let word = part.split_whitespace().next().unwrap_or_default();
        if word.is_empty() || SHELL_NEUTRAL.contains(&word) { return true; }
        if SHELL_SEARCH.contains(&word) { search = true; }
        else if SHELL_READ.contains(&word) { read = true; }
        else if SHELL_LIST.contains(&word) { list = true; }
        else { return false; }
        true
    };
    let (mut start, mut depth, mut quote, mut escaped) = (0, 0usize, None::<u8>, false);
    let bytes = cmd.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        if escaped { escaped = false; continue; }
        if let Some(q) = quote {
            if c == b'\\' && q == b'"' { escaped = true; } else if c == q { quote = None; }
            continue;
        }
        match c {
            b'\\' => escaped = true,
            b'\'' | b'"' | b'`' => quote = Some(c),
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth = depth.saturating_sub(1),
            // O `&` de `2>&1`, `<&3` e `&>` é redirecionamento, não separa comando.
            b'&' if i > 0 && matches!(bytes[i - 1], b'>' | b'<') || bytes.get(i + 1) == Some(&b'>') => {}
            b'|' | b'&' | b';' | b'\n' if depth == 0 => {
                if !classify(&cmd[start..i]) { return FoldKind::Shell; }
                start = i + 1;
            }
            _ => {}
        }
    }
    if !classify(&cmd[start..]) { return FoldKind::Shell; }
    // A mesma precedência do Claude Code: listagem, depois busca, depois leitura.
    if list { FoldKind::List } else if search { FoldKind::Search } else if read { FoldKind::Read } else { FoldKind::Shell }
}

pub(super) fn is_any(name: &str, names: &[&str]) -> bool { names.iter().any(|n| name.eq_ignore_ascii_case(n)) }

/// O servidor de `mcp__<servidor>__<ferramenta>` como está no nome: é o que a lista publica.
fn mcp_server_raw(name: &str) -> Option<&str> { conversation::mcp_parts(name).map(|(server, _)| server) }

/// Servidor (sem o prefixo `hangar-`, como o cartão mostra) e ferramenta.
fn mcp_tool(name: &str) -> Option<(&str, &str)> {
    conversation::mcp_parts(name).map(|(server, tool)| (server.strip_prefix("hangar-").unwrap_or(server), tool))
}

pub(super) const SHELL_TOOLS: &[&str] = &["bash", "powershell", "shell", "exec", "exec_command"];

/// Tipo da chamada na linha dobrada, como o Claude Code conta; `None` não dobra (edição, gravação, agente e o carregador
/// de ferramentas ficam sempre à vista).
pub(super) fn fold_kind(name: Option<&str>, input: Option<&serde_json::Map<String, serde_json::Value>>, plugin_servers: &[String]) -> Option<FoldKind> {
    let name = name.unwrap_or_default();
    if is_any(name, &["read", "notebookread"]) { return Some(FoldKind::Read); }
    if is_any(name, &["grep", "glob", "find", "websearch", "webfetch"]) { return Some(FoldKind::Search); }
    if name.eq_ignore_ascii_case("ls") { return Some(FoldKind::List); }
    if is_any(name, SHELL_TOOLS) {
        let kind = match input.and_then(|i| i.get("command").or_else(|| i.get("cmd"))) {
            Some(serde_json::Value::String(command)) => shell_fold_kind(command),
            // O Codex manda o comando em lista de palavras.
            Some(serde_json::Value::Array(words)) => shell_fold_kind(&words.iter().filter_map(|w| w.as_str()).collect::<Vec<_>>().join(" ")),
            _ => FoldKind::Shell,
        };
        return Some(kind);
    }
    // Servidor criado por plugin fica à vista, com o ponto, como no Claude Code; os outros somam na dobra.
    let server = mcp_server_raw(name)?;
    (!plugin_servers.iter().any(|s| s == server)).then_some(FoldKind::Mcp)
}

/// A linha dobrada, na ordem e no tempo verbal do Claude Code: "Buscou 1 padrão, leu 2 arquivos, rodou 3 comandos de
/// shell"; enquanto alguma roda, "Lendo 2 arquivos…". `kinds` é o tipo de cada chamada, já calculado pelo grupo.
pub(super) fn fold_title(events: &[ChatEvent], tools: &[Tool], kinds: &[FoldKind], running: bool) -> String {
    let (mut counts, mut files, mut servers) = ([0usize; 5], std::collections::HashSet::new(), Vec::<&str>::new());
    let mut pathless_reads = 0;
    for (tool, &kind) in tools.iter().zip(kinds) {
        counts[kind as usize] += 1;
        let call = &events[tool.call];
        match kind {
            FoldKind::Read => match editdiff::input_path(call.tool_input.as_ref()) { "" => pathless_reads += 1, path => { files.insert(path); } },
            FoldKind::Mcp => if let Some(server) = call.tool_name.as_deref().and_then(mcp_tool).map(|(server, _)| server) { if !servers.contains(&server) { servers.push(server); } },
            _ => {}
        }
    }
    // Arquivos distintos, mais as leituras sem caminho (um `cat` no shell não diz qual arquivo leu).
    counts[FoldKind::Read as usize] = files.len() + pathless_reads;
    let names = servers.join(", ");
    // Na ordem de `FoldKind`: a chave da frase concluída e a da que está em andamento.
    const KEYS: [(&str, &str); 5] = [("term_dobra_buscou", "term_dobra_buscando"), ("term_dobra_leu", "term_dobra_lendo"),
        ("term_dobra_listou", "term_dobra_listando"), ("term_dobra_chamou", "term_dobra_chamando"), ("term_dobra_rodou", "term_dobra_rodando")];
    let parts: Vec<String> = KEYS.iter().zip(counts).filter(|(_, n)| *n > 0).map(|(&(done, live), n)| {
        let key = if running { live } else { done };
        count_with(&format!("{key}_1"), key, n, &[("nome", &names)])
    }).collect();
    let text = super::controls::capitalized(&parts.join(", "));
    if running { format!("{text}…") } else { text }
}

impl Hangar {
    pub(super) fn render_terminal_thinking(&mut self, row: &str, parts: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let mut body = Vec::new();
        for &i in parts {
            let event = &self.chat.events[i];
            if event.tool_name.as_deref() == Some("ToolSearch") { continue; }
            if event.kind == "thinking" {
                let (key, source) = (format!("{}:thought", event.id), safe_markdown(event.text.as_deref().unwrap_or("")));
                if source.trim().is_empty() { continue; }
                // A chave é o id do evento: o `toggle` acha a linha pelas partes do bloco.
                let fold = event.id.clone();
                if appearance::get().thinking_collapsed && !self.expanded.contains(&fold) {
                    let summary = conversation::thought_summary(event.text.as_deref().unwrap_or(""));
                    body.push(terminal_thinking_fold(fold.clone(), summary, cx.listener(move |this, _, _, cx| this.toggle(fold.clone(), cx))));
                    continue;
                }
                let view = self.text_view(&key, row, source, cx);
                let text = terminal_thinking_text(&view, cx);
                body.push(if appearance::get().thinking_collapsed {
                    terminal_tool_body(fold, text, cx.listener(|this, key: &String, _, cx| {
                        if this.expanded.contains(key) { this.toggle(key.clone(), cx); }
                    })).into_any_element()
                } else { text.into_any_element() });
            } else {
                let result = self.conversation.paired.get(&i).copied();
                body.push(self.render_tool(Tool { call: i, result }, row, cx));
            }
        }
        div().flex().flex_col().gap_2().children(body).into_any_element()
    }

    /// Uma chamada na pele Terminal. Edição mostra o diff sempre; o resto abre entrada e saída no clique.
    pub(super) fn render_terminal_tool(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        if let Some(card) = self.render_agent_card(tool, cx) { return card; }
        let (key, name, error, edits, path, arg, created, outcome, status_color, dot, expandable, shown);
        {
            // Empresta o evento em vez de clonar: o resultado pode ser uma saída enorme e isto roda a cada quadro.
            let call = &self.chat.events[tool.call];
            let result = tool.result.map(|i| &self.chat.events[i]);
            key = call.id.clone();
            error = result.is_some_and(|r| r.is_error == Some(true));
            name = display_name(call.tool_name.as_deref().unwrap_or_default());
            edits = (!error).then(|| editdiff::shown(call, result)).flatten();
            (arg, path) = header_arg(call.tool_name.as_deref(), call.tool_input.as_ref());
            let status;
            (status, status_color) = self.tool_status(tool);
            dot = if error { theme::danger() } else if result.is_none() { theme::accent() } else { theme::success() };
            // Write sem patch e sem linha removida é arquivo novo; com patch os números já são os do arquivo.
            created = call.tool_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("write"))
                && edits.as_deref().is_some_and(|e| e.iter().all(|e| !e.patched && e.removed == 0));
            // Só o diff acima do corte (10 no arquivo novo, 40 na edição) tem o que abrir ou recolher.
            shown = if created { WRITE_SHOWN } else { EDIT_SHOWN };
            expandable = edits.as_deref().is_some_and(|e| e.iter().map(|e| e.lines.len()).sum::<usize>() > shown);
            outcome = match &edits { Some(edits) if result.is_some() => edit_summary(edits, created), _ => status };
        }
        let open = self.expanded.contains(&key);

        let toggle_key = key.clone();
        // Botão sem ícone: foca pelo teclado, ativa com Enter/Espaço e leva o rótulo de acessibilidade, como o `disclosure`.
        let inner = |name: String| div().w_full().flex().items_start().justify_start().gap(px(8.)).font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.))
            .child(bullet(dot, 19.))
            .child(div().min_w_0().flex().flex_wrap()
                .child(div().font_weight(FontWeight::BOLD).text_color(theme::text()).child(name))
                .child(div().min_w_0().text_color(theme::text()).child(format!("({arg})"))));
        // Edição com o diff todo à vista não tem o que abrir: o cabeçalho fica sem foco e sem clique, só com a dica do caminho.
        let inert = edits.is_some() && !expandable;
        let header: AnyElement = if inert {
            div().id(SharedString::from(format!("term-{key}"))).w_full()
                .when_some(path.clone(), |el, path| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)))
                .child(inner(name)).into_any_element()
        } else {
            // O rótulo só existe aqui: o cabeçalho inerte não o usa, e isto roda a cada quadro.
            let label = format!("{name}: {}. {outcome}", path.as_deref().unwrap_or(&arg));
            Button::new(SharedString::from(format!("term-{key}"))).ghost().w_full().h_auto().p_0().justify_start().toggled(open)
                .accessibility_label(label)
                .when_some(path.clone(), |el, path| el.tooltip(path))
                .child(inner(name))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx))).into_any_element()
        };
        let outcome_line = div().flex().gap(px(8.)).pl(px(INDENT - 8.)).font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.))
            .child(div().flex_none().text_color(theme::faint()).child("⎿"))
            .child(div().min_w_0().text_color(if error { theme::danger() } else if edits.is_some() { theme::text() } else { status_color }).child(outcome));

        let body = match &edits {
            Some(edits) => {
                // Recolhido, corta no `shown`; o clique no cabeçalho mostra o resto.
                let (rows, hidden, total) = diff_body(&self.chat.events[tool.call], edits, (!open).then_some(shown), cx);
                let faint = |text: String| div().text_size(px(12.)).font_family(theme::MONO).text_color(theme::faint()).child(text);
                let more = (!open && hidden > 0).then(|| faint(crate::i18n::tr_shared("term_mais_linhas", &[("n", &hidden.to_string())])));
                let clipped = (more.is_none() && total > super::edits::SHOWN_MAX).then(|| faint(tr("diff_clipped")
                    .replace("{shown}", &super::edits::SHOWN_MAX.to_string()).replace("{total}", &total.to_string())));
                let collapse = (open && expandable).then(|| faint(crate::i18n::tr_shared("term_recolher", &[])));
                Some(div().pl(px(INDENT + 16.)).flex().flex_col().children(rows).children(more).children(clipped).children(collapse).into_any_element())
            }
            None => open.then(|| terminal_tool_body(key.clone(), self.tool_body(tool, row, cx),
                cx.listener(|this, key: &String, _, cx| {
                    if this.expanded.contains(key) { this.toggle(key.clone(), cx); }
                }))
                .pl(px(INDENT + 16.)).into_any_element()),
        };
        // O que o SendUserFile mandou fica à vista: é a razão da chamada, e no Claude Code a imagem chega à pessoa.
        let call = &self.chat.events[tool.call];
        let ts = tool.result.and_then(|i| self.chat.events[i].ts).or(call.ts);
        let sent = sends_files(call).then(|| tool_file_refs(call)).filter(|refs| !refs.is_empty())
            .map(|refs| div().pl(px(INDENT + 16.)).pt_1().child(self.render_refs(&format!("{row}-{key}-sent"), refs, ts, cx)));
        div().flex().flex_col().child(header).child(outcome_line).children(sent).children(body).into_any_element()
    }

    /// Chamadas seguidas na pele Terminal: uma embaixo da outra, sem moldura de grupo. Buscas, leituras, MCP e comandos
    /// seguidos somam numa linha cinza que abre no clique, rodando ou com erro, como no Claude Code; edição nunca fica
    /// escondida.
    pub(super) fn render_terminal_group(&mut self, row: &str, tools: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        // Uma classificação por chamada e por quadro: o parse do comando não se repete no laço nem no título.
        let servers: &[String] = self.selected.as_ref().map_or(&[], |s| s.plugin_tool_servers.as_slice());
        let kinds: Vec<Option<FoldKind>> = tools.iter().map(|t| {
            let call = &self.chat.events[t.call];
            fold_kind(call.tool_name.as_deref(), call.tool_input.as_ref(), servers)
        }).collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut i = 0;
        while i < tools.len() {
            if kinds[i].is_none() { rows.push(self.render_terminal_tool(tools[i], row, cx)); i += 1; continue; }
            let run = kinds[i..].iter().take_while(|k| k.is_some()).count();
            let folded = &tools[i..i + run];
            let folded_kinds: Vec<FoldKind> = kinds[i..i + run].iter().flatten().copied().collect();
            let key = format!("{row}#fold-{}", self.chat.events[tools[i].call].id);
            let open = self.expanded.contains(&key);
            let live = folded.iter().rev().find(|t| t.result.is_none() && self.running(t.call)).copied();
            let title = fold_title(&self.chat.events, folded, &folded_kinds, live.is_some());
            let toggle_key = key.clone();
            let mut block = vec![fold_header(SharedString::from(key), title, open, live.is_some(),
                cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)))];
            if !open {
                // Recolhida, a linha mostra o que roda agora e a saída de cada falha, como o Claude Code.
                if let Some(tool) = live {
                    let call = &self.chat.events[tool.call];
                    let summary = conversation::summarize_input(call.tool_name.as_deref(), call.tool_input.as_ref());
                    // Comando em voo com o `$` na frente, como o Claude Code desenha.
                    let shell = call.tool_name.as_deref().is_some_and(|name| is_any(name, SHELL_TOOLS));
                    block.push(fold_sub(if shell { format!("$ {summary}") } else { summary }, theme::muted()));
                }
                for tool in folded.iter().filter(|t| t.result.is_some_and(|r| self.chat.events[r].is_error == Some(true))) {
                    block.push(fold_sub(self.tool_status(*tool).0, theme::danger()));
                }
            }
            rows.push(div().flex().flex_col().children(block).into_any_element());
            if open { for tool in folded { rows.push(self.render_terminal_tool(*tool, row, cx)); } }
            i += run;
        }
        div().flex().flex_col().gap(px(10.)).children(rows).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Between, ChatEvent, EDIT_SHOWN, FoldKind, Tool, between, count_with, display_name, fold_kind, fold_title, header_arg, mcp_tool, shell_fold_kind, split_budget, visible_edits};
    use serde_json::json;

    #[test]
    fn file_tools_show_the_file_name_and_keep_the_full_path() {
        let read = json!({"file_path": "/a/b/models.py"});
        assert_eq!(header_arg(Some("Read"), read.as_object()), ("models.py".to_owned(), Some("/a/b/models.py".to_owned())));
        let pi = json!({"path": "src/x.rs"});
        assert_eq!(header_arg(Some("edit"), pi.as_object()).0, "x.rs");
    }

    #[test]
    fn search_and_shell_tools_keep_the_summary() {
        let grep = json!({"pattern": "patch", "path": "backend/app"});
        let (arg, path) = header_arg(Some("Grep"), grep.as_object());
        assert_eq!((arg, path), (crate::conversation::summarize_input(Some("Grep"), grep.as_object()), None));
        let bash = json!({"command": "npm run check"});
        assert_eq!(header_arg(Some("Bash"), bash.as_object()), (crate::conversation::summarize_input(Some("Bash"), bash.as_object()), None));
    }

    #[test]
    fn between_edits_picks_nothing_gap_or_file_name() {
        assert_eq!(between(None, "/a/x.rs"), Between::Nothing);
        assert_eq!(between(Some("/a/x.rs"), "/a/x.rs"), Between::Gap);
        assert_eq!(between(Some("/a/x.rs"), "/a/y.rs"), Between::File("y.rs".into()));
    }

    #[test]
    fn an_edit_without_lines_does_not_hide_the_next_ones() {
        assert_eq!(visible_edits(&[0, 3, 2]), vec![(1, 3), (2, 2)]);
        assert_eq!(visible_edits(&[4, 0, 0]), vec![(0, 4)]);
    }

    #[test]
    fn the_line_budget_is_split_across_edits_in_order() {
        assert_eq!(split_budget(&[30], EDIT_SHOWN), vec![30]);
        assert_eq!(split_budget(&[100], EDIT_SHOWN), vec![40]);
        assert_eq!(split_budget(&[30, 30], EDIT_SHOWN), vec![30, 10]);
        assert_eq!(split_budget(&[450], 400), vec![400]);
        assert_eq!(split_budget(&[40, 5], EDIT_SHOWN), vec![40, 0]);
    }

    #[test]
    fn fold_kind_counts_like_claude_code() {
        let kind = |name: &str| fold_kind(Some(name), None, &[]);
        assert_eq!(kind("Read"), Some(FoldKind::Read));
        assert_eq!(kind("Grep"), Some(FoldKind::Search));
        assert_eq!(kind("mcp__hangar__send"), Some(FoldKind::Mcp));
        assert_eq!(kind("Bash"), Some(FoldKind::Shell));
        assert_eq!(kind("mcp__x"), None);
        assert_eq!(kind("ToolSearch"), None);
        assert_eq!(kind("Edit"), None);
        assert_eq!(kind("Write"), None);
        assert_eq!(kind("Agent"), None);
        let codex = json!({"cmd": ["rg", "-n", "x"]});
        assert_eq!(fold_kind(Some("exec_command"), codex.as_object(), &[]), Some(FoldKind::Search));
    }

    #[test]
    fn plugin_mcp_servers_stay_out_of_the_fold() {
        let plugin = ["pmedico".to_owned()];
        assert_eq!(fold_kind(Some("mcp__pmedico__progresso"), None, &plugin), None);
        assert_eq!(fold_kind(Some("mcp__ai-memory__memory_status"), None, &plugin), Some(FoldKind::Mcp));
        // O conjunto tem o nome cru da ferramenta: o `hangar-` não é tirado na comparação.
        assert_eq!(fold_kind(Some("mcp__hangar-computer-control__objetivo"), None, &["computer-control".to_owned()]), Some(FoldKind::Mcp));
        assert_eq!(fold_kind(Some("mcp__hangar-computer-control__objetivo"), None, &["hangar-computer-control".to_owned()]), None);
    }

    #[test]
    fn mcp_calls_get_the_claude_code_name() {
        assert_eq!(display_name("mcp__pmedico__progresso"), "pmedico - progresso (MCP)");
        assert_eq!(display_name("mcp__hangar-computer-control__objetivo"), "computer-control - objetivo (MCP)");
        assert_eq!(display_name("Edit"), "Update");
        assert_eq!(display_name("Bash"), "Bash");
    }

    #[test]
    fn a_shell_command_made_only_of_reads_counts_as_a_read() {
        assert_eq!(shell_fold_kind("rg -n foo src | head -5"), FoldKind::Search);
        assert_eq!(shell_fold_kind("cat a.txt | jq .x"), FoldKind::Read);
        assert_eq!(shell_fold_kind("ls -la && tree"), FoldKind::List);
        assert_eq!(shell_fold_kind("echo oi; cat a"), FoldKind::Read);
        assert_eq!(shell_fold_kind("cd src && ls"), FoldKind::Shell);
        assert_eq!(shell_fold_kind("echo \"a | rg\""), FoldKind::Shell);
        assert_eq!(shell_fold_kind("npm run build"), FoldKind::Shell);
        assert_eq!(shell_fold_kind("(cd src && ls)"), FoldKind::Shell);
        assert_eq!(shell_fold_kind("rg x | grep y"), FoldKind::Search);
    }

    #[test]
    fn a_read_that_writes_deletes_or_runs_something_else_does_not_fold() {
        for cmd in [
            "sort a > b", "cat x > y", "cat x >> y", "awk '{print $1}' f > out", "echo oi > f", "cat x &> log", "cat x >&log",
            "cat x >| y", "cat x | tee y", "sort -o out in", "sort -uo out in", "sort --output=out in",
            "find . -delete", "find . -exec rm {} +", "find . -execdir rm {} \\;", "find . -ok rm {} \\;", "find . -okdir rm {} \\;",
            "find . -fprint f", "find . -fprintf f %p", "find . -fls f",
            "cat $(rm -rf x)", "cat `rm -rf x`", "grep \"$(rm x)\" f", "diff <(ls a) b", "cat x | grep y > z",
        ] {
            assert_eq!(shell_fold_kind(cmd), FoldKind::Shell, "{cmd}");
        }
    }

    #[test]
    fn discarding_output_or_merging_stderr_is_not_a_write_and_its_ampersand_does_not_split() {
        assert_eq!(shell_fold_kind("grep foo f 2>&1 | head"), FoldKind::Search);
        assert_eq!(shell_fold_kind("cat x 2>/dev/null"), FoldKind::Read);
        assert_eq!(shell_fold_kind("cat x > /dev/null 2>&1"), FoldKind::Read);
        assert_eq!(shell_fold_kind("rg x &>/dev/null"), FoldKind::Search);
        assert_eq!(shell_fold_kind("cat x >&2"), FoldKind::Read);
        assert_eq!(shell_fold_kind("awk '$1 > 5' f"), FoldKind::Read);
        assert_eq!(shell_fold_kind("grep '$(x)' f"), FoldKind::Search);
        assert_eq!(shell_fold_kind("grep -o foo f | sort"), FoldKind::Search);
        assert_eq!(shell_fold_kind("sort -n f"), FoldKind::Read);
    }

    #[test]
    fn pathless_reads_add_to_the_distinct_files() {
        let call = |name: &str, input: serde_json::Value| ChatEvent { kind: "tool_use".into(), tool_name: Some(name.into()),
            tool_input: input.as_object().cloned(), ..Default::default() };
        let events = [call("Read", json!({"file_path": "/a"})), call("Read", json!({"file_path": "/a"})),
            call("Bash", json!({"command": "cat b"})), call("Bash", json!({"command": "cat c"}))];
        let tools: Vec<Tool> = (0..events.len()).map(|call| Tool { call, result: None }).collect();
        let expected = crate::app::controls::capitalized(&count_with("term_dobra_leu_1", "term_dobra_leu", 3, &[("nome", "")]));
        assert_eq!(fold_title(&events, &tools, &[FoldKind::Read; 4], false), expected);
    }

    #[test]
    fn mcp_server_drops_the_hangar_prefix_like_the_card() {
        assert_eq!(mcp_tool("mcp__hangar-computer-control__objetivo"), Some(("computer-control", "objetivo")));
        assert_eq!(mcp_tool("mcp__pmedico__progresso__x"), Some(("pmedico", "progresso__x")));
        assert_eq!(mcp_tool("mcp__x"), None);
        assert_eq!(mcp_tool("Bash"), None);
    }
}
