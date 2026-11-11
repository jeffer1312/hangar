//! `analyze` fornece os fatos da captura usada em produção; `reduce` é o redutor temporal do
//! `state::Monitor` (porte do `StateMonitor` do Python), com a memória que atravessa as rodadas.
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

const SPINNERS: &str = "✻✽✶✺✢·∗✳✦✧";
pub const STALE_LIMIT: u32 = 3;
pub const IDLE_DEBOUNCE: u32 = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TerminalQuestion {
    pub question: Option<String>,
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PaneAnalysis {
    pub state: String,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    pub spinner: Option<String>,
    pub status_line: Option<String>,
    pub overlay: bool,
    pub login: bool,
    pub limit_reset: Option<String>,
    pub preview: String,
    pub codex_menu: Option<TerminalQuestion>,
}

impl Default for PaneAnalysis {
    fn default() -> Self {
        Self { state: "idle".into(), label: None, question: None, options: None,
            spinner: None, status_line: None, overlay: false, login: false,
            limit_reset: None, preview: String::new(), codex_menu: None }
    }
}

/// Memória do redutor entre rodadas.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ReducerMemory {
    pub prev_spinner: Option<String>,
    pub frozen: u32,
    pub no_spinner: u32,
    pub held_state: String,
    pub held_label: Option<String>,
    /// Última divergência (plugin, pane) já registrada: uma linha por divergência, não por rodada.
    #[serde(skip)]
    pub divergence: Option<(String, String)>,
}

impl Default for ReducerMemory {
    fn default() -> Self {
        Self { prev_spinner: None, frozen: 0, no_spinner: 0, held_state: "idle".into(), held_label: None, divergence: None }
    }
}

/// Fatos da rodada que não vêm do pane.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ReducerFacts {
    pub open_question: Option<TerminalQuestion>,
    pub plugin_question: Option<Value>,
    pub plugin_state: Option<String>,
    pub hook_state: Option<String>,
    pub hook_grace: Option<u32>,
    pub status_line: Option<String>,
}

impl Default for ReducerFacts {
    fn default() -> Self {
        Self { open_question: None, plugin_question: None, plugin_state: None,
            hook_state: None, hook_grace: Some(8), status_line: None }
    }
}

/// Resultado da rodada, comparado às fixtures Python.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ReducedState {
    pub analysis: PaneAnalysis,
    pub memory: ReducerMemory,
}

struct Patterns {
    lines: Regex, option: Regex, omp_option: Regex, box_split: Regex,
    cursor: Regex, pi_cursor: Regex, omp_cursor: Regex, embedded: Regex,
    rule: Regex, box_bottom: Regex, frame: Regex, footer: Regex,
    login: Regex, composer: Regex, limit: Regex, codex_option: Regex,
    unnumbered: Regex, user: Regex, banner: Regex, warning: Regex, effort: Regex,
    tool: Regex, task_notice: Regex, activity: Regex, mcp: Regex,
    pi_box: Regex, overlay_rule: Regex, todo: Regex, ascii_spinner: Regex,
    subagent: Regex, subagent_body: Regex, digit: Regex, word: Regex, agent_cursor: Regex,
}

fn terminal_regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace(r"\s", r"[\s\x1c-\x1f]")
        .replace(r"\S", r"[^\s\x1c-\x1f]").replace(r"\w", r"[\p{L}\p{N}_]")).unwrap()
}

static P: LazyLock<Patterns> = LazyLock::new(|| {
    // Python considera também os separadores de informação como espaço.
    let r = terminal_regex;
    // O re.I do Python reúne também as duas formas turcas de i.
    let insensitive = |s: &str| {
        let expanded: String = s.chars().map(|c| match c {
            'i' | 'I' => "[iIıİ]".into(),
            _ => c.to_string(),
        }).collect();
        r(&format!("(?i){expanded}"))
    };
    Patterns {
        lines: r(r"\r\n|[\n\r\x0b\x0c\x1c-\x1e\u{85}\u{2028}\u{2029}]"),
        option: r(r"^\s*[❯>]?\s*\d+\.\s+(.*\S)\s*$"),
        omp_option: r(r"^\s*│\s*(?:\u{f054}\s+)?\u{f10c}\s+(.*\S)\s*$"),
        box_split: r(r"\s{2,}[│─╭╮╰╯┌┐└┘├┤┬┴┼]|[│╭╮╰╯┌┐└┘├┤┬┴┼]|\s{3,}"),
        cursor: r(r"^\s*❯\s*\d+\.\s"),
        pi_cursor: r(r"^\s*>\s*\d+\.\s|^\s*│\s*\u{f054}\s+\u{f10c}\s"),
        omp_cursor: r(r"^\s*│\s*\u{f054}\s+\u{f10c}\s"),
        embedded: r(r"\s\d+\.\s"),
        // A régua de cima da caixa de digitar leva no fim o nome da sessão (`──── nome ─`) quando ela tem um.
        rule: r(r"^[\s─]*─{10,}(?: [^─│]+ ─+)?[\s─]*$"),
        box_bottom: r(r"^\s*╰[─\s]*╯\s*$"),
        frame: r(r"^[\s│─╭╮╰╯┌┐└┘├┤┬┴┼]*$"),
        footer: r(r"to navigate|Esc to cancel|Enter to select|Enter select"),
        login: insensitive(r"/oauth/authorize|Paste code here|Select login method|Choose the text style"),
        composer: r(r"⏵⏵|⏸"),
        limit: insensitive(r"(?:usage limit reached|hit your \w+ limit|limit reached)[^\n]{0,80}?(?:resets?|continuing automatically|try again)\s*(?:at\s*)?([0-9]{1,2}(?::[0-9]{2})?\s*(?:am|pm)?)"),
        codex_option: r(r"^\s*[›>]?\s*(\d+)\.\s+(.*\S)\s*$"),
        unnumbered: r(r"^(\s*❯\s+)\S"),
        // Texto da resposta vem recuado em duas colunas: uma citação dessa linha não fecha a resposta.
        user: r(r"^(?:\s*❯| ?› Prompt from the .+ plugin\s*$)"),
        banner: r(r"^[\s▐▛█▝▜▀]*Claude Code v\d"),
        warning: r(r"^[a-z][\w.-]*: (?:hooks\.json: [a-z]|reloaded \(\d+ hooks?\b)"),
        // Aviso temporário do nível de esforço, à direita acima da caixa de digitar; o ícone de `high` é ●.
        effort: r(r"^[○◐●◉◈◎] \S+(?: · ultracode)? · /effort\s*$"),
        tool: r(r"^([A-Z][\w-]*\(|(Running|Reading|Writing|Editing|Searching|Listing|Fetching|Updating|Creating|Deleting|Crawling|Downloading|Globbing|Grepping|Waiting|Loading|Compiling|Building|Installing|Ran|Making))"),
        // Linha que a TUI escreve para cada `<task-notification>`: o resumo dela, não texto do Claude.
        // Só o começo: a descrição entre aspas pode quebrar de linha antes do status.
        task_notice: r(r#"^(?:Agent "|Background (?:command|agent) "|Background shell command\b|Monitor (?:event:|"))"#),
        activity: r(r"ran \d+ shell commands?\s*$"),
        mcp: r(r"^Calling[^\n]*(…|\.\.\.)\s*$"),
        pi_box: r(r"^\s*[╭╰][─\s]*[╮╯]\s*$"),
        overlay_rule: r(r"^[\s▔]*▔{10,}[\s▔]*$"),
        todo: r(r"^\s*[●○]?\s*Todos \(\d+/\d+\)\s*$"),
        ascii_spinner: r(r"^\*\s+\S[^\n]*(…|\))\s*$"),
        subagent: r(r"^Subagent\s+\S"),
        agent_cursor: r(r"^\s*❯\s+[●◯]"),
        subagent_body: r(r"^\s*└"),
        digit: r(r"^\d$"),
        word: r(r"^\w$"),
    }
});

pub fn whitespace(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }
pub fn trim(s: &str) -> &str { s.trim_matches(whitespace) }
fn left(s: &str) -> &str { s.trim_start_matches(whitespace) }
pub fn right(s: &str) -> &str { s.trim_end_matches(whitespace) }
fn word(c: char) -> bool {
    let mut encoded = [0; 4];
    P.word.is_match(c.encode_utf8(&mut encoded))
}
fn tool(s: &str) -> bool {
    P.tool.find(s).is_some_and(|m| m.as_str().ends_with('(')
        || s[m.end()..].chars().next().is_none_or(|c| !word(c)))
}
fn mcp(s: &str) -> bool {
    P.mcp.is_match(s) && s.chars().nth(7).is_none_or(|c| !word(c))
}
fn activity(s: &str) -> bool {
    P.activity.find(s).is_some_and(|m| s[..m.start()].chars().next_back().is_none_or(|c| !word(c)))
}
fn decimal_number(s: &str) -> Option<usize> {
    s.chars().try_fold(0usize, |number, c| {
        let mut start = c as u32;
        while let Some(previous) = start.checked_sub(1).and_then(char::from_u32) {
            if !P.digit.is_match(&previous.to_string()) { break; }
            start -= 1;
        }
        // Blocos decimais Unicode têm dez posições, inclusive os blocos matemáticos contíguos.
        number.checked_mul(10)?.checked_add(((c as u32 - start) % 10) as usize)
    })
}
/// `str.splitlines()` do Python.
pub fn lines(pane: &str) -> Vec<&str> {
    if pane.is_empty() { return Vec::new(); }
    let mut out: Vec<_> = P.lines.split(pane).collect();
    if out.last() == Some(&"") { out.pop(); }
    out
}
fn tail(lines: &[&str], n: usize) -> String {
    let end = lines.iter().rposition(|l| !trim(l).is_empty()).map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}
fn boundary(line: &str) -> bool {
    left(line).chars().next().is_some_and(|c| c == '●' || c == '⎿' || SPINNERS.contains(c))
}
/// Faixa de abas da AskUserQuestion, topo do menu: "☐ …" na escolha única, "←  ☐ … ✔ Submit  →" na múltipla.
fn tab_bar(line: &str) -> bool {
    let line = left(line);
    left(line.strip_prefix('←').unwrap_or(line)).starts_with(['☐', '☑'])
}
/// Logo abaixo da régua acima do cursor está a linha de prompt do composer (`❯` sem número de opção).
fn composer_prompt_above(lines: &[&str], cursor: usize) -> bool {
    (0..cursor).rev().find(|&i| P.rule.is_match(lines[i]))
        .and_then(|rule| lines.get(rule + 1))
        .is_some_and(|line| left(line).starts_with('❯') && !P.cursor.is_match(line))
}
fn option(line: &str) -> Option<&str> {
    P.option.captures(line).or_else(|| P.omp_option.captures(line))
        .map(|c| c.get(1).unwrap().as_str())
}
fn option_label(label: &str) -> String { trim(P.box_split.split(label).next().unwrap_or("")).into() }

fn menu_block(lines: &[&str]) -> Option<(usize, usize)> {
    let mut cursor = None;
    for (i, line) in lines.iter().enumerate() {
        if P.cursor.is_match(line) { cursor = Some((i, false)); }
        else if P.pi_cursor.is_match(line) { cursor = Some((i, true)); }
    }
    let (cursor, pi) = cursor?;
    if pi && !P.footer.is_match(&tail(lines, 8)) { return None; }
    if P.omp_cursor.is_match(lines[cursor]) {
        let bottom = lines.iter().rposition(|l| P.footer.is_match(l))?;
        let top = lines[..bottom].iter().rposition(|l| left(l).starts_with('╭'))?;
        return Some((top + 1, bottom));
    }
    if lines[cursor + 1..].iter().any(|l| left(l).starts_with('❯') && !P.cursor.is_match(l)) {
        return None;
    }
    if let Some(mark) = P.cursor.find(lines[cursor]).or_else(|| P.pi_cursor.find(lines[cursor])) {
        let label = P.box_split.split(&lines[cursor][mark.end()..]).next().unwrap_or("");
        if P.embedded.is_match(label) { return None; }
    }
    // Rascunho numerado em várias linhas: o `❯ N.` é a própria linha de prompt, colada na régua de
    // cima do composer. Opção de menu de verdade nunca encosta na régua: vem depois do título ou da pergunta.
    // Rascunho não tem rodapé de navegação; com rodapé, segue como menu.
    if !pi && cursor > 0 && P.rule.is_match(lines[cursor - 1]) && !lines[cursor + 1..].iter().any(|l| P.footer.is_match(l)) {
        return None;
    }
    // A régua de cima do diálogo também fecha o menu: acima dela fica o corpo (o plano do
    // ExitPlanMode tem passos numerados que virariam opções).
    let top = (0..cursor).rev().find(|&i| boundary(lines[i]) || tab_bar(lines[i]) || P.rule.is_match(lines[i]))
        .map_or(0, |i| i + 1);
    let bottom = (cursor + 1..lines.len()).find(|&i| P.footer.is_match(lines[i]) || boundary(lines[i]))
        .unwrap_or(lines.len());
    // Menu de verdade some com o composer: rodapé com régua embaixo é um menu desenhado no TEXTO do
    // composer (recado colado), e tocar nele mandaria teclas para o prompt.
    if !pi && bottom < lines.len() && P.footer.is_match(lines[bottom]) && lines[bottom + 1..].iter().any(|l| P.rule.is_match(l))
        && composer_prompt_above(lines, cursor) {
        return None;
    }
    Some((top, bottom))
}

fn question(lines: &[&str]) -> Option<String> {
    let mut found = None;
    for line in lines {
        if option(line).is_some() { break; }
        let s = trim(trim(line).trim_matches('│'));
        if s.is_empty() || P.rule.is_match(line) || P.frame.is_match(line) || tab_bar(line) { continue; }
        found = Some(s.into());
    }
    found
}

/// Régua horizontal do Claude Code (bordas da caixa de digitar e dos diálogos).
pub fn is_rule(line: &str) -> bool { P.rule.is_match(line) }

/// A régua de baixo da última caixa do composer (régua, `❯`, régua) antes de `end`: abaixo dela mora o
/// rodapé do Claude Code.
fn composer_end(lines: &[&str], end: usize) -> Option<usize> {
    let rules: Vec<usize> = (0..end).filter(|&i| P.rule.is_match(lines[i])).collect();
    let [.., upper, lower] = rules[..] else { return None };
    lines[upper + 1..lower].iter().any(|l| left(l).starts_with('❯')).then_some(lower)
}

/// O rodapé de navegação de um menu (picker) ainda está em algum lugar do pane inteiro, não só nas
/// últimas linhas: pergunta longa empurra o rodapé para fora da janela do `overlay`.
pub fn footer_visible(pane: &str) -> bool { P.footer.is_match(pane) }

/// O teclado está no rodapé do Claude Code (painel de agentes ou pílula de tarefas), não no composer:
/// o que se digita some, e o `x` do painel para um subagente.
pub fn footer_focus(pane: &str) -> bool {
    let lines = lines(pane);
    composer_end(&lines, lines.len()).is_some_and(|end| lines[end + 1..].iter()
        .any(|l| P.agent_cursor.is_match(l) || l.contains("Enter to view") || l.contains("↑/↓ to select")))
}

fn unnumbered_menu(lines: &[&str]) -> Option<TerminalQuestion> {
    let cursor = lines.iter().rposition(|l| P.unnumbered.is_match(l) && !P.cursor.is_match(l))?;
    if lines[cursor + 1..].iter().any(|l| P.rule.is_match(l)) { return None; }
    let col = P.unnumbered.captures(lines[cursor])?.get(1)?.as_str().chars().count();
    let aligned = |line: &str| {
        let prefix: String = line.chars().take(col).collect();
        line.chars().nth(col).is_some_and(|c| trim(&prefix).is_empty() && c != ' ')
    };
    let mut top = cursor;
    while top > 0 && aligned(lines[top - 1]) { top -= 1; }
    let mut bottom = cursor + 1;
    while bottom < lines.len() && aligned(lines[bottom]) { bottom += 1; }
    let options: Vec<String> = lines[top..bottom].iter().map(|l| trim(&l.chars().skip(col).collect::<String>()).into()).collect();
    if options.len() < 2 { return None; }
    // O painel de agentes ("← for agents": ● principal, ◯ subagente) com foco põe o `❯` num agente e
    // parece um menu sem número; responder ao cartão dele navegava no painel.
    if options.iter().all(|o| o.starts_with(['●', '◯'])) && composer_end(lines, top).is_some() { return None; }
    let start = lines[..top].iter().rposition(|l| P.rule.is_match(l)).map_or(0, |i| i + 1);
    let question = lines[start..top].iter().find(|l| !trim(l).is_empty()).map(|l| trim(l).into());
    Some(TerminalQuestion { question, options })
}

fn live_spinner(lines: &[&str]) -> Option<String> {
    lines.iter().rev().map(|l| trim(l)).find(|s| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| SPINNERS.contains(c)) && chars.next() == Some(' ')
    }).map(String::from)
}

fn status_line(lines: &[&str]) -> Option<String> {
    let anchor = lines.iter().rposition(|l| P.rule.is_match(l) || P.box_bottom.is_match(l));
    let mut chrome: Vec<_> = lines[anchor.map_or(0, |i| i + 1)..].iter()
        .filter(|l| !trim(l).is_empty()).map(|l| right(l)).collect();
    if anchor.is_none() && chrome.len() > 2 { chrome.drain(..chrome.len() - 2); }
    if chrome.is_empty() { None } else { Some(chrome.join("\n")) }
}

/// `state.menu_codex`: (pergunta, opções) do seletor do Codex num pane capturado.
pub fn codex_menu_of(pane: &str) -> Option<(Option<String>, Vec<String>)> {
    codex_menu(&lines(pane)).map(|q| (q.question, q.options))
}

// A TUI mais nova encurtou o rodapé para "enter continue · esc skip"; as duas formas valem.
const CODEX_FOOTERS: [&str; 4] = ["enter to confirm", "enter to continue", "enter confirm", "enter continue"];
fn codex_menu(lines: &[&str]) -> Option<TerminalQuestion> {
    if !CODEX_FOOTERS.iter().any(|f| tail(lines, 8).to_lowercase().contains(f)) { return None; }
    let mut options: Vec<String> = Vec::new();
    let mut first = None;
    let mut column = 0;
    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = P.codex_option.captures(line) else {
            if options.is_empty() { continue; }
            let text = trim(line);
            if !text.is_empty() && line.chars().count() - left(line).chars().count() == column
                && !CODEX_FOOTERS.iter().any(|f| text.to_lowercase().contains(f)) {
                let last = options.last_mut().unwrap();
                last.push(' '); last.push_str(text); continue;
            }
            break;
        };
        let label = caps.get(2).unwrap();
        column = line[..label.start()].chars().count();
        if decimal_number(&caps[1]) != Some(options.len() + 1) { return None; }
        if first.is_none() { first = Some(i); }
        options.push(label.as_str().into());
    }
    if options.len() < 2 { return None; }
    let question = lines[..first?].iter().rev().find(|l| !trim(l).is_empty()).map(|l| trim(l).into());
    Some(TerminalQuestion { question, options })
}

/// Chamada em voo do Claude Code: a 1ª linha é a descrição escrita pelo modelo, não "Bash(", e o ● dela pisca. O que
/// a separa da prosa é a forma: o parágrafo termina colado no `⎿`.
fn tool_header(lines: &[&str], i: usize) -> bool {
    lines[i + 1..].iter().find(|l| trim(l).is_empty() || boundary(l)).is_some_and(|l| left(l).starts_with('⎿'))
}

/// `anchor`: começo da faixa dos mods, que fica entre a conversa e a caixa de digitar e pode
/// começar com ●; a ocorrência mais baixa dela fecha a conversa.
fn preview(lines: &[&str], anchor: Option<&str>, skip: impl Fn(&[&str], usize, &str) -> bool, stop: impl Fn(&str) -> bool) -> String {
    let mut end = lines.iter().rposition(|l| P.rule.is_match(l) || P.overlay_rule.is_match(l) || P.pi_box.is_match(l))
        .unwrap_or(lines.len());
    // Só a faixa corta a continuação: o `end` do quadro pode ser a borda de uma caixa da própria resposta.
    let mut band = lines.len();
    if let Some(row) = anchor.filter(|a| !a.is_empty()).and_then(|a| lines[..end].iter().rposition(|l| l.contains(a))) {
        (end, band) = (row, row);
    }
    let begin = lines[..end].iter().enumerate().filter(|(i, l)| P.user.is_match(l) && !(*i > 0 && P.rule.is_match(lines[*i - 1])))
        .map(|(i, _)| i + 1).last().unwrap_or(0);
    if begin == 0 && lines[..end].iter().any(|l| P.banner.is_match(l)) { return String::new(); }
    let mut start = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(begin) {
        let s = left(line);
        let body = left(s.chars().next().map_or("", |c| &s[c.len_utf8()..]));
        if !s.starts_with('●') { continue; }
        if P.warning.is_match(body) {
            // A recarga pedida no meio do turno não desfaz a resposta que veio antes dela.
            if body.contains(": hooks.json: ") { start = None; }
            continue;
        }
        if P.effort.is_match(s) { continue; }
        let subagent = P.subagent.is_match(body) && lines[i + 1..].iter().find(|l| !trim(l).is_empty())
            .is_some_and(|l| P.subagent_body.is_match(l));
        if !tool(body) && !mcp(body) && !P.task_notice.is_match(body)
            && !P.todo.is_match(line) && !subagent && !tool_header(lines, i) && !skip(lines, i, body) { start = Some(i); }
    }
    let Some(start) = start else { return String::new(); };
    let first = left(lines[start]);
    let mut out = vec![right(left(first.strip_prefix('●').unwrap_or(first)))];
    for (j, line) in lines.iter().enumerate().take(band).skip(start + 1) {
        let s = left(line);
        if P.rule.is_match(line) || boundary(line) || P.user.is_match(line) || tool(s)
            || mcp(s) || P.todo.is_match(line) || P.ascii_spinner.is_match(s) || P.effort.is_match(s) || stop(line)
            // Só no começo de parágrafo: as linhas do mesmo parágrafo dão a mesma resposta.
            || activity(s) || (!trim(s).is_empty() && trim(lines[j - 1]).is_empty() && tool_header(lines, j - 1)) { break; }
        out.push(right(line));
    }
    while out.last().is_some_and(|l| trim(l).is_empty()) { out.pop(); }
    out.join("\n")
}

/// Prévia do pane e se o spinner corre, sem o resto da análise: o `Monitor` a chama no quadro
/// cortado na largura da conversa ou com a faixa dos mods à vista.
pub fn pane_preview(pane: &str, anchor: Option<&str>) -> (String, bool) {
    pane_preview_filtered(pane, anchor, |_, _, _| false, |_| false)
}

/// A mesma extração, com as fronteiras e os blocos adicionais da TUI do provedor.
fn pane_preview_filtered(
    pane: &str,
    anchor: Option<&str>,
    skip: impl Fn(&[&str], usize, &str) -> bool,
    stop: impl Fn(&str) -> bool,
) -> (String, bool) {
    let lines = lines(pane);
    (preview(&lines, anchor, skip, stop), live_spinner(&lines).is_some())
}

/// A moldura do composer compartilhada pelos provedores de terminal.
fn is_composer_box(line: &str) -> bool { P.pi_box.is_match(line) }

/// Fronteira da continuação cujo início já rolou para fora do quadro.
fn is_preview_boundary(line: &str) -> bool {
    boundary(line) || P.rule.is_match(line) || P.overlay_rule.is_match(line)
        || P.user.is_match(line) || is_composer_box(line)
}

struct OtherPreviewPatterns {
    tool: Regex,
    used: Regex,
    ansi: Regex,
    folded: Regex,
}

static OTHER_PREVIEW: LazyLock<OtherPreviewPatterns> = LazyLock::new(|| OtherPreviewPatterns {
    tool: terminal_regex(r"^(Bash|Read|Write|Edit|MultiEdit|Grep|Glob|Task|Agent|Skill|WebFetch|WebSearch|NotebookEdit|TodoWrite|Multiple Tools|Chrome Devtools)[\s:(]"),
    used: terminal_regex(r"^Us(?:ed|ing)\s+[A-Z][\w-]*\s*\("),
    ansi: terminal_regex(r"\x1b\[[0-9;?]*[a-zA-Z]"),
    folded: terminal_regex(r"^\s*\.\.\.\s*\(\d+ more lines"),
});

fn kimi_spinner(line: &str) -> bool {
    matches!(left(line).chars().next(), Some('\u{2800}'..='\u{28ff}' | '\u{1f311}'..='\u{1f318}'))
}

fn kimi_prompt(line: &str) -> bool {
    left(line).strip_prefix('\u{2728}').is_some_and(|rest| rest.chars().next().is_some_and(whitespace))
}

fn kimi_todo_item(line: &str) -> bool {
    let mut chars = left(line).chars();
    matches!(chars.next(), Some('✓' | '●' | '○')) && chars.next().is_some_and(whitespace)
}

fn kimi_in_todo(lines: &[&str], at: usize) -> bool {
    for line in lines[..=at].iter().rev() {
        if trim(line) == "Todo" { return true; }
        if !trim(line).is_empty() && !kimi_todo_item(line) { return false; }
    }
    false
}

fn kimi_plain(pane: &str) -> String {
    lines(pane).into_iter().filter_map(|line| {
        // Cor e posição do itálico juntas separam raciocínio de ênfase na resposta.
        if let Some(at) = line.find("\x1b[3m")
            && line.contains("38;2;136;136;136")
            && OTHER_PREVIEW.ansi.replace_all(&line[..at], "").trim_matches([' ', '●']).is_empty()
        { return None; }
        let clean = OTHER_PREVIEW.ansi.replace_all(line, "");
        (!OTHER_PREVIEW.folded.is_match(&clean)).then(|| clean.into_owned())
    }).collect::<Vec<_>>().join("\n")
}

fn pi_tool(lines: &[&str], at: usize, body: &str) -> bool {
    OTHER_PREVIEW.tool.is_match(body) && lines[at + 1..].iter().find(|line| !trim(line).is_empty())
        .is_some_and(|line| matches!(left(line).chars().next(), Some('├' | '└' | '│' | '▌' | '⎿')))
}

fn other_stop(provider: &str, line: &str) -> bool {
    is_composer_box(line) || (provider == "kimi" && (kimi_spinner(line) || kimi_prompt(line)
        || trim(line) == "Todo" || OTHER_PREVIEW.used.is_match(left(line))))
}

/// Prosa do quadro Pi/omp/Kimi, usando a mesma eleição de blocos dos outros terminais.
pub fn other_pane_preview(provider: &str, pane: &str) -> String {
    if !matches!(provider, "pi" | "omp" | "kimi") { return pane_preview(pane, None).0; }
    let plain = (provider == "kimi").then(|| kimi_plain(pane));
    let pane = plain.as_deref().unwrap_or(pane);
    pane_preview_filtered(pane, None, |lines, at, body| {
        if provider == "kimi" { OTHER_PREVIEW.used.is_match(body) || kimi_in_todo(lines, at) }
        else { pi_tool(lines, at, body) }
    }, |line| other_stop(provider, line)).0
}

/// Texto visível do começo do quadro que só pode ser aceito quando a costura comprovar a continuação.
pub fn kimi_continuation(pane: &str) -> String {
    let plain = kimi_plain(pane);
    let mut out = Vec::new();
    for line in lines(&plain) {
        if is_preview_boundary(line) || kimi_spinner(line) || kimi_todo_item(line)
            || other_stop("kimi", line)
        { break; }
        out.push(right(line));
    }
    let begin = out.iter().position(|line| !trim(line).is_empty()).unwrap_or(out.len());
    let end = out.iter().rposition(|line| !trim(line).is_empty()).map_or(begin, |at| at + 1);
    out[begin..end].join("\n")
}

/// Costura em pontos de código; o booleano distingue crescimento de troca de bloco.
pub fn stitch_preview(accumulated: &str, frame: &str) -> (String, bool) {
    if frame.is_empty() { return (accumulated.into(), true); }
    if accumulated.is_empty() || frame.starts_with(accumulated) { return (frame.into(), true); }
    // A tabela de prefixos encontra a sobreposição sem repetir fatias de um quadro longo.
    let pattern: Vec<_> = frame.chars().collect();
    let mut prefixes = vec![0; pattern.len()];
    for at in 1..pattern.len() {
        let mut matched = prefixes[at - 1];
        while matched > 0 && pattern[at] != pattern[matched] { matched = prefixes[matched - 1]; }
        if pattern[at] == pattern[matched] { matched += 1; }
        prefixes[at] = matched;
    }
    let mut matched = 0;
    for c in accumulated.chars() {
        while matched > 0 && (matched == pattern.len() || pattern[matched] != c) {
            matched = prefixes[matched - 1];
        }
        if pattern[matched] == c { matched += 1; }
    }
    // Sobreposição menor que uma frase é coincidência, não continuação do mesmo bloco.
    if matched < 24 { return (frame.into(), false); }
    let cut = frame.char_indices().nth(matched).map_or(frame.len(), |(at, _)| at);
    let mut combined = accumulated.to_owned();
    combined.push_str(&frame[cut..]);
    (combined, true)
}


pub fn analyze(pane: &str) -> PaneAnalysis {
    let lines = lines(pane);
    let spinner = live_spinner(&lines);
    let mut result = PaneAnalysis {
        spinner: spinner.clone(), status_line: status_line(&lines),
        overlay: P.footer.is_match(&tail(&lines, 8)),
        login: P.login.is_match(pane) && !P.composer.is_match(&tail(&lines, 12)),
        limit_reset: P.limit.captures(&lines[lines.len().saturating_sub(8)..].join("\n"))
            .map(|c| trim(c.get(1).unwrap().as_str()).into()),
        preview: preview(&lines, None, |_, _, _| false, |_| false), codex_menu: codex_menu(&lines), ..Default::default()
    };
    let menu = if let Some((top, bottom)) = menu_block(&lines) {
        let region = &lines[top..bottom];
        let options: Vec<_> = region.iter().filter_map(|l| option(l)).map(option_label).filter(|s| !s.is_empty()).collect();
        if options.len() >= 2 { Some(TerminalQuestion { question: question(region), options }) } else { None }
    } else { unnumbered_menu(&lines) };
    if let Some(menu) = menu {
        result.state = "awaiting_input".into(); result.question = menu.question; result.options = Some(menu.options);
    } else if let Some(spinner) = spinner {
        result.state = "working".into(); result.label = Some(trim(&spinner.chars().skip(2).collect::<String>()).into());
    }
    result
}

/// O diálogo de aprovação do `ExitPlanMode`. Os rótulos das opções mudam com a versão e o modo;
/// a pergunta é a âncora.
pub fn plan_dialog(analysis: &PaneAnalysis) -> bool {
    analysis.state == "awaiting_input" && analysis.question.as_deref().is_some_and(|q| q.contains("Would you like to proceed?"))
}

fn set_question(analysis: &mut PaneAnalysis, question: TerminalQuestion) {
    analysis.state = "awaiting_input".into(); analysis.label = None;
    analysis.question = question.question; analysis.options = Some(question.options);
}

/// A pergunta que o hook do plugin segura (`pergunta_pendente`), como a sessão a mostra: permissão
/// vira "ferramenta: resumo" com Yes/No; pergunta sem itens não vale.
pub fn held_question(q: &Value) -> Option<TerminalQuestion> {
    if q.get("id").and_then(Value::as_str).is_some_and(|s| s.starts_with("perm:")) {
        let tool = q.get("tool").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("?");
        let summary = q.get("resumo").and_then(Value::as_str).unwrap_or("");
        return Some(TerminalQuestion { question: Some(format!("{tool}: {summary}")), options: vec!["Yes".into(), "No".into()] });
    }
    let q = q.get("questions").and_then(Value::as_array).and_then(|q| q.first())?;
    let question = q.get("question").and_then(Value::as_str).map(String::from);
    let options = q.get("options").and_then(Value::as_array).map(|options| options.iter()
        .map(|o| o.get("label").and_then(Value::as_str).unwrap_or("").into()).collect()).unwrap_or_default();
    Some(TerminalQuestion { question, options })
}

/// O que a rodada viu além do estado.
#[derive(Serialize)]
pub struct ReducerDiagnostic {
    pub before_plugin: String,
    pub plugin_applied: bool,
    /// O plugin corrigiu o pane, e essa divergência (plugin, pane) é nova: vai ao log.
    pub divergence: Option<(String, String)>,
}

pub fn reduce(pane: &str, memory: ReducerMemory, facts: ReducerFacts) -> ReducedState {
    reduce_with_diagnostics(pane, memory, facts).0
}

pub fn reduce_with_diagnostics(pane: &str, memory: ReducerMemory, facts: ReducerFacts) -> (ReducedState, ReducerDiagnostic) {
    reduce_analysis(analyze(pane), memory, facts)
}

/// `reduce` sobre a análise que o `TerminalPool` já fez da captura.
pub fn reduce_analysis(mut analysis: PaneAnalysis, mut memory: ReducerMemory, facts: ReducerFacts) -> (ReducedState, ReducerDiagnostic) {
    if analysis.state != "awaiting_input" && analysis.options.as_ref().is_none_or(Vec::is_empty) {
        if let Some(q) = facts.open_question { set_question(&mut analysis, q); }
    }
    if analysis.state != "awaiting_input" {
        if let Some(q) = facts.plugin_question.as_ref().and_then(held_question) {
            set_question(&mut analysis, q);
        }
    }
    let mut animating = false;
    if analysis.state == "awaiting_input" {
        memory.prev_spinner = None; memory.frozen = 0; memory.no_spinner = 0;
    } else if let Some(spinner) = &analysis.spinner {
        memory.no_spinner = 0;
        animating = memory.prev_spinner.as_ref().is_some_and(|s| s != spinner) && spinner.contains('…');
        memory.frozen = if memory.prev_spinner.as_ref() == Some(spinner) { memory.frozen.saturating_add(1) } else { 0 };
        memory.prev_spinner = Some(spinner.clone());
        if memory.frozen >= STALE_LIMIT { analysis.state = "idle".into(); analysis.label = None; }
        else { analysis.state = "working".into(); }
    } else {
        memory.no_spinner = memory.no_spinner.saturating_add(1);
        memory.prev_spinner = None; memory.frozen = 0;
        if memory.held_state == "working" && memory.no_spinner < IDLE_DEBOUNCE {
            analysis.state = "working".into(); analysis.label = memory.held_label.clone();
        }
    }
    let mut diagnostic = ReducerDiagnostic { before_plugin: analysis.state.clone(), plugin_applied: false, divergence: None };
    if matches!(analysis.state.as_str(), "working" | "idle") {
        if let Some(plugin) = facts.plugin_state.filter(|s| matches!(s.as_str(), "working" | "idle")) {
            if !(plugin == "idle" && animating) {
                diagnostic.plugin_applied = true;
                let seen = (plugin != analysis.state).then(|| (plugin.clone(), analysis.state.clone()));
                if seen.is_some() && seen != memory.divergence { diagnostic.divergence.clone_from(&seen); }
                memory.divergence = seen;
                analysis.state = plugin;
                if analysis.state == "idle" { analysis.label = None; }
                memory.prev_spinner = None; memory.frozen = 0; memory.no_spinner = 0;
            }
        }
    }
    if matches!(analysis.state.as_str(), "working" | "idle") {
        match facts.hook_state.as_deref() {
            Some("idle") if analysis.state == "working" && !animating => { analysis.state = "idle".into(); analysis.label = None; }
            Some("working") if analysis.state == "idle" && facts.hook_grace.is_none_or(|g| memory.no_spinner < g) => { analysis.state = "working".into(); }
            _ => {}
        }
    }
    if let Some(status) = facts.status_line.filter(|s| !s.is_empty()) { analysis.status_line = Some(status); }
    // Sem emissão, estado e rótulo já são os mesmos que o monitor guardou.
    memory.held_state = analysis.state.clone(); memory.held_label = analysis.label.clone();
    (ReducedState { analysis, memory }, diagnostic)
}

#[cfg(test)]
mod tests {
    use super::pane_preview;

    const RELOAD: &str = "● hangar: reloaded (22 hooks: session.start, turn.start)";

    #[test]
    fn the_plugin_reload_notice_is_not_assistant_text() {
        assert_eq!(pane_preview(&format!("{RELOAD}\n\n"), None).0, "");
        assert_eq!(pane_preview(&format!("{RELOAD}\n\n› Prompt from the hangar plugin\n"), None).0, "");
    }

    #[test]
    fn a_prompt_from_the_plugin_ends_the_previous_reply() {
        assert_eq!(pane_preview("● resposta antiga\n\n› Prompt from the hangar plugin\n", None).0, "");
        assert_eq!(pane_preview("› Prompt from the hangar plugin\n\n● resposta nova\n", None).0, "resposta nova");
        assert_eq!(pane_preview("● citou:\n  › Prompt from the hangar plugin\n", None).0, "citou:\n  › Prompt from the hangar plugin");
        assert_eq!(pane_preview(&format!("● resposta\n\n{RELOAD}\n"), None).0, "resposta");
        assert_eq!(pane_preview("● nginx: reloaded (pid 42), tudo certo\n", None).0, "nginx: reloaded (pid 42), tudo certo");
    }

    const RULE: &str = "────────────────────────────────────────";

    #[test]
    fn the_mods_band_is_not_part_of_the_reply() {
        let pane = format!("● resposta\n▾ PR #374 perf(native): stop keeping   [ abrir PR ] [-]\n\n  ✓ ▸ CI 51490cc   7/7 jobs  [ abrir CI ]\n    ━━━━━━━ ━━━━━━━\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(pane_preview(&pane, Some("PR #374")).0, "resposta");
    }

    #[test]
    fn background_task_notices_are_not_assistant_text() {
        for notice in [
            "Background shell command didn't finish before the previous session ended",
            "Background agent \"revisor\" didn't finish before the previous session ended",
            "Background command \"Run tests\" completed (exit code 0)",
            "Background command \"Run tests\" failed with exit code 1",
            "Background command \"Run tests\" was stopped",
            "Agent \"revisor\" finished",
            "Agent \"revisor\" is waiting on its own background work",
            "Agent \"revisor\" was stopped by Claude",
            "Agent \"revisor\" failed: Agent terminated early due to an API error",
            "Monitor event: \"build\"",
            "Monitor \"build\" stream ended",
            "Monitor \"build\" script failed (exit 1)",
            "Monitor event: build finished",
            "Agent \"revisor\" completed",
            "Agent \"revisor\" killed",
            "Background agent \"Sleep 240 seconds then report done and\n  verify\" didn't finish before the previous session ended",
        ] {
            assert_eq!(pane_preview(&format!("● {notice}\n{RULE}\n❯ \n{RULE}\n"), None).0, "", "{notice}");
            assert_eq!(pane_preview(&format!("● resposta\n\n● {notice}\n{RULE}\n❯ \n{RULE}\n"), None).0, "resposta", "{notice}");
        }
        assert_eq!(pane_preview("● Background command line tools are handy.\n", None).0, "Background command line tools are handy.");
    }
    #[test]
    fn other_preview_panes_preserve_provider_contract() {
        let cases = [
            ("PANE_PI_TOOL_DEPOIS_DA_PROSA", "pi", "● Fechei o wire e subi o commit; o back valida na sequência.\n\n● Bash cd \"/home/jefferson/x\" && dotnet run\n └ pid 326487\n   t+35s: vivo\n\n✻ Unfurling… (7m 11s)\n", "Fechei o wire e subi o commit; o back valida na sequência."),
            ("PANE_PI_SPINNER_ASCII", "pi", "● Duas respostas — deixa eu confirmar a segunda\n\n   Thinking…\n\n * Boondoggling… (thinking with high effort · 12s)\n\n○ Todos (3/3)\n├─ ✓ Marcar RESPONSAVEL_FALLBACK com badge onde renderiza\n└─ ✓ Atualizar MOCKS.md com o que a auditoria encontrar\n\n────────────────────────────────────────────────────────────\n", "Duas respostas — deixa eu confirmar a segunda\n\n   Thinking…"),
            ("pane_pi_working.txt", "pi", "\n   Na teoria são complementares, mas na prática dois pacotes que mexem na interface do pi podem sim\n   pisar um no outro. A documentação pode estar otimista.\n\n   A única forma de ter certeza é testar. Se você acha que são concorrentes, acho justo remover os\n   dois e instalar de um em um:\n\n   ╭· bash ······················································································ ╮\n   │   pi remove npm:pi-claude-code-tui                                                           │\n   │   pi remove npm:pi-claude-code-ui                                                            │\n   ╰······························································································╯\n\n   Reinicia o pi. Aí instala só um deles:\n\n   ╭· bash ······················································································ ╮\n   │   pi install npm:pi-claude-code-tui                                                          │\n   ╰······························································································╯\n\n   Testa. Se ainda não resolver, remove e instala só o outro:\n\n   ╭· bash ······················································································ ╮\n   │   pi remove npm:pi-claude-code-tui                                                           │\n   │   pi install npm:pi-claude-code-ui                                                           │\n   ╰······························································································╯\n\n   Quer fazer esse teste? Posso te acompanhar nos comandos.\n\n   ✻ Turn took 17s (Total time 18m 5s · 10 turns)\n\n›  eu nao consigo copiar do conteudo do pi?\n\n   Thought for 3s\n\n ● Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:\n\n╭─────────────────────────────────────────────────────────────────────────────────────────────────╮\n\n╰─────────────────────────────────────────────────────────────────────────────────────────────────╯\n🤖 kimi-for-coding (high) │ 📁 jefferson │ 📟 jefferson-2 │ 💬 sessão 44kin/16kout · cache 1.1M ...", "Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:"),
            ("PANE_PI_TOOL_DEPOIS_DA_PROSA", "omp", "● Fechei o wire e subi o commit; o back valida na sequência.\n\n● Bash cd \"/home/jefferson/x\" && dotnet run\n └ pid 326487\n   t+35s: vivo\n\n✻ Unfurling… (7m 11s)\n", "Fechei o wire e subi o commit; o back valida na sequência."),
            ("PANE_PI_SPINNER_ASCII", "omp", "● Duas respostas — deixa eu confirmar a segunda\n\n   Thinking…\n\n * Boondoggling… (thinking with high effort · 12s)\n\n○ Todos (3/3)\n├─ ✓ Marcar RESPONSAVEL_FALLBACK com badge onde renderiza\n└─ ✓ Atualizar MOCKS.md com o que a auditoria encontrar\n\n────────────────────────────────────────────────────────────\n", "Duas respostas — deixa eu confirmar a segunda\n\n   Thinking…"),
            ("pane_pi_working.txt", "omp", "\n   Na teoria são complementares, mas na prática dois pacotes que mexem na interface do pi podem sim\n   pisar um no outro. A documentação pode estar otimista.\n\n   A única forma de ter certeza é testar. Se você acha que são concorrentes, acho justo remover os\n   dois e instalar de um em um:\n\n   ╭· bash ······················································································ ╮\n   │   pi remove npm:pi-claude-code-tui                                                           │\n   │   pi remove npm:pi-claude-code-ui                                                            │\n   ╰······························································································╯\n\n   Reinicia o pi. Aí instala só um deles:\n\n   ╭· bash ······················································································ ╮\n   │   pi install npm:pi-claude-code-tui                                                          │\n   ╰······························································································╯\n\n   Testa. Se ainda não resolver, remove e instala só o outro:\n\n   ╭· bash ······················································································ ╮\n   │   pi remove npm:pi-claude-code-tui                                                           │\n   │   pi install npm:pi-claude-code-ui                                                           │\n   ╰······························································································╯\n\n   Quer fazer esse teste? Posso te acompanhar nos comandos.\n\n   ✻ Turn took 17s (Total time 18m 5s · 10 turns)\n\n›  eu nao consigo copiar do conteudo do pi?\n\n   Thought for 3s\n\n ● Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:\n\n╭─────────────────────────────────────────────────────────────────────────────────────────────────╮\n\n╰─────────────────────────────────────────────────────────────────────────────────────────────────╯\n🤖 kimi-for-coding (high) │ 📁 jefferson │ 📟 jefferson-2 │ 💬 sessão 44kin/16kout · cache 1.1M ...", "Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:"),
            ("_KIMI_PENSANDO", "kimi", " \u{001b}[38;2;136;136;136m● \u{001b}[3mCount letter r in \"strawberry raspberry blueberry\".\u{001b}[0m\n   \u{001b}[2m... (9 more lines, ctrl+o to expand)\u{001b}[0m\n\n  \u{001b}[38;2;136;136;136m⠋ working...\u{001b}[0m\n", ""),
            ("_KIMI_RESPONDENDO", "kimi", " \u{001b}[38;2;224;224;224m● \u{001b}[39mSao 6 letras r no total.\n", "Sao 6 letras r no total."),
            ("_KIMI_RESPOSTA_COM_ITALICO", "kimi", " \u{001b}[38;2;224;224;224m● \u{001b}[39mO \u{001b}[3mteste\u{001b}[0m \u{001b}[1mpassou\u{001b}[0m com sucesso.\n", "O teste passou com sucesso."),
            ("_KIMI_USED", "kimi", "● Used ReadMediaFile (…rojetos/hangar/.hangar-uploads/1787141544-3efdfc.png) · image (image/jpeg, 49.1 KB)\n", ""),
            ("_KIMI_USING", "kimi", "● Using Write (docs/superpowers/plans/2026-08-24-folha-de-exames.md)\n      1  # Folha de exames — Implementation Plan\n", ""),
            ("_PANE_KIMI_TODO", "kimi", " ● Enquanto a captura roda, vou aproveitar pra deixar registrado o desenho completo.\n   O texto continua aqui, segundo paragrafo do bloco em voo.\n\n  ⠸ working... · Tip: Try /dance for a hidden Easter egg\n ─────────────────────────────────────────────────────────────────────────\n   Todo\n   ✓ Backend: costura (_costurar) + acumulador no PreviewBroker (só kimi)\n   ● Testes backend (costura, broker, unpacks) e suíte verde\n ╭───────────────────────────────────────────────────────────────────────╮\n │ >                                                                     │\n ╰───────────────────────────────────────────────────────────────────────╯\n", "Enquanto a captura roda, vou aproveitar pra deixar registrado o desenho completo.\n   O texto continua aqui, segundo paragrafo do bloco em voo."),
            ("pane_kimi_eco_e_dica.txt", "kimi", "\n ● Resposta curta: não, só parcialmente. Sou o Kimi Code CLI, não o Claude — então as configurações dele não vêm automaticamente. O que existe:\n\n   Skills que eu realmente tenho carregadas (escopo User, em ~/.agents/skills/):\n   • agent-browser, emil-design-eng, falar, find-skills, humanizer, impeccable\n\n   Skills suas em ~/.claude/skills/ que eu NÃO vejo (o Claude tem, eu não):\n   • conselho-5-agentes, design-sprint, graphify, handoff, hooked-ux, ios-hig-design, learned, orchestrating-idea-to-push, orquestrar, refactoring-ui, scout-fix, supply-chain-hardening, ux-heuristics\n\n   Hooks (~/.claude/hooks/): não tenho acesso — são específicos do Claude Code. O Kimi Code tem um sistema próprio de hooks configurável no config.toml, mas seus scripts (skill-suggester.py,\n   guard_tmux.py, etc.) não estão conectados a ele. Aliás, vejo que existe um sync-skills.sh lá — talvez seja justamente pra isso.\n\n   Se quiser, posso:\n   1. Ler o sync-skills.sh e ver se ele já faz a ponte ~/.claude/skills → ~/.agents/skills (e rodá-lo, se fizer sentido), ou\n   2. Criar symlinks das skills faltantes para o diretório que eu leio.\n\n   Qual prefere?\n\n ✨ onde ta esse sync-skills.sh?\n\n ● It's in ~/.claude/hooks/sync-skills.sh. Let me confirm path and maybe show it briefly.\n\n ● Ran a command\n   $ ls -la ~/.claude/hooks/sync-skills.sh && head -30 ~/.claude/hooks/sync-skills.sh\n   lrwxrwxrwx 1 jefferson jefferson 52 jul 15 11:55 /home/jefferson/.claude/hooks/sync-skills.sh -> /home/jefferson/Projetos/skills/hooks/sync-skills.sh\n   #!/usr/bin/env bash\n   # Pulls jeffer1312/skills repo and syncs symlinks into ~/.claude/skills/\n   ... (28 more lines, ctrl+o to expand)\n\n ● Está em ~/.claude/hooks/sync-skills.sh, que é um symlink para /home/jefferson/Projetos/skills/hooks/sync-skills.sh.\n\n   O que ele faz (pelo início do script):\n\n   • Faz git pull do repo ~/Projetos/skills (seu repo de skills, jeffer1312/skills)\n   • Cria symlinks de cada pasta com SKILL.md para ~/.claude/skills/\n   • Sincroniza scripts de hooks (*.sh, *.py) para ~/.claude/hooks/\n   • É disparado como hook SessionStart do Claude Code\n\n   Ou seja: ele sincroniza só para o Claude — nada aponta para ~/.agents/skills/ (onde eu leio skills). Por isso eu só vejo as 6 que já existem lá.\n\n   Se quiser que eu também enxergue as outras, dá pra estender o script com um segundo destino (~/.agents/skills/) ou só criar os symlinks manualmente agora. Quer que eu faça?\n\n ✨ sim\n\n  🌓 · Tip: Try /dance for a hidden Easter egg\n ╭────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╮\n │ >                                                                                                                                                                                                  │\n ╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯\n 🤖 K3 │ 📁 projeto-exemplo [ABC-123] │ ⚡5h:25% ↺1h25m │ 📅7d:30% ↺seg 14h·5d22h │ 🕐 16:34\n                                                                                                                                                                                 context: 3% (23.5k/1M)\n", "Está em ~/.claude/hooks/sync-skills.sh, que é um symlink para /home/jefferson/Projetos/skills/hooks/sync-skills.sh.\n\n   O que ele faz (pelo início do script):\n\n   • Faz git pull do repo ~/Projetos/skills (seu repo de skills, jeffer1312/skills)\n   • Cria symlinks de cada pasta com SKILL.md para ~/.claude/skills/\n   • Sincroniza scripts de hooks (*.sh, *.py) para ~/.claude/hooks/\n   • É disparado como hook SessionStart do Claude Code\n\n   Ou seja: ele sincroniza só para o Claude — nada aponta para ~/.agents/skills/ (onde eu leio skills). Por isso eu só vejo as 6 que já existem lá.\n\n   Se quiser que eu também enxergue as outras, dá pra estender o script com um segundo destino (~/.agents/skills/) ou só criar os symlinks manualmente agora. Quer que eu faça?"),
            ("test_bullet_de_markdown_com_asterisco_nao_trunca_a_previa", "pi", "● Ficou assim:\n* primeiro item\n* segundo item\n\n✻ Unfurling… (2s)\n", "Ficou assim:\n* primeiro item\n* segundo item"),
            ("test_caixa_do_composer_do_pi_tambem_corta", "pi", "● Prosa do Pi em voo.\n╭──────────────────────────────╮\n│ digitar aqui                 │\n╰──────────────────────────────╯\n  🤖 k3 (high)\n\n  ● main\n  ◯ general-purpose  Grepping  1m 34s\n", "Prosa do Pi em voo."),
            ("test_kimi_enfase_na_resposta_nao_e_confundida_com_rascunho", "kimi", "\n  ⠋ working...\n ● O teste passou com sucesso.", "O teste passou com sucesso."),
            ("test_kimi_raciocinio_sai_da_previa", "kimi", "\n  ⠋ working...", ""),
            ("test_kimi_resposta_continua_na_previa", "kimi", "\n  ⠋ working...\n ● Sao 6 letras r no total.", "Sao 6 letras r no total."),
            ("test_kimi_spinner_sem_dica_para_a_continuacao", "kimi", " ● Enquanto a captura roda, vou aproveitar pra deixar registrado o desenho completo.\n   O texto continua aqui, segundo paragrafo do bloco em voo.\n\n  ⠋ working...\n ─────────────────────────────────────────────────────────────────────────\n   Todo\n   ✓ Backend: costura (_costurar) + acumulador no PreviewBroker (só kimi)\n   ● Testes backend (costura, broker, unpacks) e suíte verde\n ╭───────────────────────────────────────────────────────────────────────╮\n │ >                                                                     │\n ╰───────────────────────────────────────────────────────────────────────╯\n", "Enquanto a captura roda, vou aproveitar pra deixar registrado o desenho completo.\n   O texto continua aqui, segundo paragrafo do bloco em voo."),
            ("test_kimi_used_depois_da_prosa_corta_o_bloco", "kimi", "● A resposta comeca aqui.\n● Used ReadMediaFile (…rojetos/hangar/.hangar-uploads/1787141544-3efdfc.png) · image (image/jpeg, 49.1 KB)\n", "A resposta comeca aqui."),
            ("test_kimi_used_em_prosa_inglesa_nao_descarta", "kimi", "● Used correctly, the flag avoids the double render.\n", "Used correctly, the flag avoids the double render."),
            ("test_kimi_using_depois_da_prosa_corta_o_bloco", "kimi", "● A resposta comeca aqui.\n● Using Write (docs/superpowers/plans/2026-08-24-folha-de-exames.md)\n      1  # Folha de exames — Implementation Plan\n", "A resposta comeca aqui."),
            ("test_kimi_using_em_prosa_inglesa_nao_descarta", "kimi", "● Using it twice made the render flicker.\n", "Using it twice made the render flicker."),
            ("test_painel_de_tarefas_nao_vira_previa", "pi", "● Todos (11/13)\n├─ ✓ Avisar back: revisar DDL\n└─ +3 more (3 completed)\n\n✻ Unfurling… (2m)\n", ""),
            ("test_preview_kimi_corta_a_dica_nas_DUAS_formas", "kimi", "● resposta do assistente\n\n  🌓 · Tip: Try /dance for a hidden Easter egg\n ╭───────────╮\n │ >         │\n ╰───────────╯", "resposta do assistente"),
            ("test_preview_kimi_corta_a_dica_nas_DUAS_formas", "kimi", "● resposta do assistente\n\n  ⠋ working... · Tip: /goal for multi-step work with a clear finish line\n ╭───────────╮\n │ >         │\n ╰───────────╯", "resposta do assistente"),
            ("test_preview_pi_pula_as_quatro_formas_de_cabecalho_de_tool", "pi", "● A prosa que deve sobreviver.\n\n● Bash cd \"/home/jefferson/x\" && dotnet run\n └ pid 326487\n\n✻ Unfurling… (2m)\n", "A prosa que deve sobreviver."),
            ("test_preview_pi_pula_as_quatro_formas_de_cabecalho_de_tool", "pi", "● A prosa que deve sobreviver.\n\n● Bash: 2 done • ctrl+o to toggle\n └ pid 326487\n\n✻ Unfurling… (2m)\n", "A prosa que deve sobreviver."),
            ("test_preview_pi_pula_as_quatro_formas_de_cabecalho_de_tool", "pi", "● A prosa que deve sobreviver.\n\n● Write  (81 lines)\n └ pid 326487\n\n✻ Unfurling… (2m)\n", "A prosa que deve sobreviver."),
            ("test_preview_pi_pula_as_quatro_formas_de_cabecalho_de_tool", "pi", "● A prosa que deve sobreviver.\n\n● Multiple Tools: 3 done • bash, chrome_devtools_navigate_page\n └ pid 326487\n\n✻ Unfurling… (2m)\n", "A prosa que deve sobreviver."),
            ("test_preview_pi_pula_edit_com_diff", "pi", "● A prosa que deve sobreviver.\n\n● Edit  (2 edits)\n │ ▌57- const [printOpen, setPrintOpen] = React.useState(false)\n\n✻ Inspecting… (6m)\n", "A prosa que deve sobreviver."),
            ("test_preview_pi_pula_grupo_de_bash_com_dois_pontos", "pi", "● Segue o resumo do que mudou no wire.\n\n● Bash: 2 done • ctrl+o to toggle\n ├ ● grep -rn \"Adicionar\" src/\n └ ● dotnet build\n\n✻ Unfurling… (2m)\n", "Segue o resumo do que mudou no wire."),
            ("test_prosa_pi_que_apresenta_trecho_sem_dois_pontos_sobrevive", "pi", "● Aqui vai o trecho pra comparar\n│ codigo original\n│ codigo novo\n\n✻ Unfurling… (2s)\n", "Aqui vai o trecho pra comparar\n│ codigo original\n│ codigo novo"),
            ("test_prosa_pi_que_termina_em_arvore_continua_sendo_prosa", "pi", "● Estrutura final:\n └ src/\n   └ wire.ts\n\n✻ Unfurling… (1s)\n", "Estrutura final:\n └ src/\n   └ wire.ts"),
        ];
        for (case, provider, pane, expected) in cases {
            let actual = super::other_pane_preview(provider, pane);
            assert_eq!(actual, expected, "{provider}: {case}");
        }
    }

    #[test]
    fn other_preview_stitch_preserves_character_overlap_and_new_blocks() {
        let cases = [
            ("test_costura_cola_pela_sobreposicao_quando_o_topo_some", "Primeira frase comprida pra valer. Segunda frase comprida pra valer.", "Segunda frase comprida pra valer. Terceira frase, recem-chegada.", "Primeira frase comprida pra valer. Segunda frase comprida pra valer. Terceira frase, recem-chegada.", true),
            ("test_costura_cresce_sem_rolar", "", "Primeira frase.", "Primeira frase.", true),
            ("test_costura_cresce_sem_rolar", "Primeira frase.", "Primeira frase. Segunda frase.", "Primeira frase. Segunda frase.", true),
            ("test_costura_overlap_curto_demais_e_coincidencia", "Termina com a", "a resposta nova comeca aqui de verdade", "a resposta nova comeca aqui de verdade", false),
            ("test_costura_quadro_vazio_mantem_o_acumulado", "texto acumulado", "", "texto acumulado", true),
            ("test_costura_sem_sobreposicao_recomeca", "Resposta da primeira pergunta, completa.", "Assunto totalmente diferente agora.", "Assunto totalmente diferente agora.", false),
            ("janela Unicode derivada do pane real Pi", "Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho", "sta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:", "Tem sim, algumas formas:\n\n   Copiar a última resposta do assistente:\n   ◉ Comando: /copy\n   ◉ Atalho: Ctrl+X\n\n   Copiar uma mensagem antiga específica:", true),
        ];
        for (case, accumulated, frame, expected, joined) in cases {
            let actual = super::stitch_preview(accumulated, frame);
            assert_eq!((actual.0.as_str(), actual.1), (expected, joined), "{case}");
        }
    }
    #[test]
    fn other_preview_continuation_stops_at_the_real_kimi_chrome() {
        let cases = [
            ("test_kimi_continuacao_sem_bullet_pega_o_topo_ate_o_chrome", "   meio do paragrafo que estava rolando.\n   A ideia. O PreviewBroker le o pane a cada 150ms.\n\n  ⠸ working... · Tip: Try /dance\n ╭───────────────────────────────────────────────────────────╮\n", "   meio do paragrafo que estava rolando.\n   A ideia. O PreviewBroker le o pane a cada 150ms."),
            ("test_kimi_continuacao_nao_aceita_saida_de_ferramenta", "total 48\n-rw-r--r-- 1 jefferson jefferson  9131 ago 19 09:41 preview.py\n  ⠸ working... · Tip: x\n", "total 48\n-rw-r--r-- 1 jefferson jefferson  9131 ago 19 09:41 preview.py"),
        ];
        for (case, pane, expected) in cases {
            assert_eq!(super::kimi_continuation(pane), expected, "{case}");
        }
    }

}
