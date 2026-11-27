//! `analyze` fornece os fatos da captura usada em produção; `reduce` é o redutor temporal do
//! `state::Monitor` (porte do `StateMonitor` do Python), com a memória que atravessa as rodadas.
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;
use unicode_width::UnicodeWidthChar;

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

/// Painel de agentes do Claude Code abaixo da caixa (a linha `main` e uma por subagente vivo, marcadas com `●` ou
/// `◯`), sem quadro da animação do turno acima dela: é o único sinal na tela de que só o subagente trabalha quando a
/// CLI não mostra a linha "Waiting for …". O quadro é procurado desde o último prompt da pessoa, não só abaixo da
/// última resposta: uma linha `●` de outro painel entre ele e a caixa o esconderia do `analyze`.
pub fn background_agents_listed(pane: &str) -> bool {
    let lines = lines(pane);
    let frame = |l: &str| {
        let s = trim(l);
        let mut chars = s.chars();
        let glyph = chars.next().is_some_and(|c| SPINNERS.contains(c) || c == '*') && chars.next() == Some(' ');
        glyph && s.split_once('…').is_some_and(|(_, rest)| rest.trim_start().starts_with('('))
    };
    let Some(end) = composer_end(&lines, lines.len()) else { return false };
    let Some(top) = lines[..end].iter().rposition(|l| P.rule.is_match(l)) else { return false };
    let turn = lines[..top].iter().rposition(|l| left(l).starts_with('❯')).map_or(0, |prompt| prompt + 1);
    let rows: Vec<&str> = lines[end + 1..].iter().map(|l| left(l).trim_start_matches('❯').trim_start()).collect();
    fn entry(row: &str) -> Option<&str> { row.strip_prefix("● ").or_else(|| row.strip_prefix("◯ ")).map(str::trim_start) }
    !lines[turn..top].iter().any(|l| frame(l))
        && rows.iter().any(|row| entry(row).is_some_and(|name| name.split_whitespace().next() == Some("main")))
        && rows.iter().any(|row| entry(row).is_some_and(|name| name.split_whitespace().next() != Some("main")))
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

/// A caixa de mensagem logo abaixo: a régua e, em seguida, a linha do `❯` (ou do `!` no modo de comando).
/// Régua solta numa resposta não conta. A faixa dos mods, quando começa por um título de painel, fica entre as duas.
fn composer_below(lines: &[&str]) -> bool {
    let mut rest = lines.iter().filter(|l| !trim(l).is_empty()).peekable();
    if rest.peek().is_some_and(|l| mods_band_title(l)) {
        while rest.peek().is_some_and(|l| !P.rule.is_match(l)) { rest.next(); }
    }
    rest.next().is_some_and(|l| P.rule.is_match(l)) && rest.next().is_some_and(|l| trim(l).starts_with(['❯', '!']))
}

/// O `*` comum também é um quadro da animação do Claude ("* Precipitating… (2m 12s)"), mas só vale com esse
/// formato e logo acima da caixa de mensagem: fora disso é texto, como um item de lista. Linha com o símbolo
/// acima de texto mais novo da conversa (`●`) é o resumo de um turno que já acabou e segue na tela enquanto
/// uma tarefa de fundo roda: não é a linha viva. Sem isso, os quadros que a leitura não reconhece pulavam
/// para ele. O Claude Code nem sempre limpa a linha inteira: o que sobra de outro texto fica à direita, depois
/// de uma fileira de espaços, e sai daqui.
fn live_spinner(lines: &[&str]) -> Option<String> {
    let ascii = |s: &str| s.strip_prefix("* ").and_then(|rest| rest.split_once(' '))
        .is_some_and(|(verb, more)| verb.ends_with('…') && more.starts_with('('));
    let composer = (0..lines.len()).rev().find(|&i| composer_below(&lines[i..]));
    let newest_reply = composer.and_then(|top| lines[..top].iter().rposition(|l| trim(l).starts_with('●') && !mods_band_title(l)));
    lines.iter().enumerate().rev().map(|(i, l)| (i, trim(l))).find(|(i, s)| {
        let mut chars = s.chars();
        newest_reply.is_none_or(|reply| *i > reply)
            && (chars.next().is_some_and(|c| SPINNERS.contains(c)) && chars.next() == Some(' ')
                || ascii(s) && composer_below(&lines[i + 1..]))
    }).map(|(_, s)| String::from(s.split("   ").next().unwrap_or(s)))
}

/// Título de painel da faixa dos mods ("● ▾ PM-20124 coleta…   [-]"): o ponto é o estado do painel, não uma resposta
/// do Claude. Tomado por resposta, ele escondia a linha viva acima da faixa, e "Waiting for 1 background agent" sumia.
/// O controle de minimizar no fim separa o título de uma resposta que comece com a mesma seta.
fn mods_band_title(line: &str) -> bool {
    let line = trim(line);
    line.strip_prefix('●').map(left).is_some_and(|rest| rest.starts_with(['▾', '▸']))
        && (line.ends_with("[-]") || line.ends_with("[+]"))
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

/// Painel mais estreito que isto não existe; o limite evita confundir recuo de código com painel.
const PANEL_MIN_COLUMN: usize = 20;

/// Células ocupadas da linha, na largura do terminal.
fn cells(line: &str) -> Vec<bool> {
    let mut out = Vec::with_capacity(line.len());
    for c in line.chars() {
        out.extend(std::iter::repeat_n(!whitespace(c), c.width().unwrap_or(1)));
    }
    out
}

/// A linha até a coluna `column`, sem o espaço que sobra no fim.
fn cells_before(line: &str, column: usize) -> String {
    let mut used = 0;
    let end = line.char_indices().find(|(_, c)| {
        used += c.width().unwrap_or(1);
        used > column
    }).map_or(line.len(), |(at, _)| at);
    right(&line[..end]).to_owned()
}

/// Onde começa o painel de diff que o Claude Code abre ao lado da conversa, na mesma linha do
/// terminal: a régua `─` que vai da borda dele até a borda direita da tela, com essa coluna vazia em
/// todas as outras linhas. Régua da resposta não chega à borda; as da caixa de digitar começam na
/// coluna 0 e ficam de fora.
fn side_panel_column(lines: &[&str]) -> Option<usize> {
    let screen = lines.iter().map(|l| cells(right(l)).len()).max()?;
    let lines: Vec<&str> = lines.iter().copied().filter(|l| !l.starts_with('─')).collect();
    let rows: Vec<Vec<bool>> = lines.iter().map(|l| cells(l)).collect();
    lines.iter().find_map(|line| {
        let line = right(line);
        let prefix = line.trim_end_matches('─');
        let column = cells(prefix).len();
        (line[prefix.len()..].chars().count() >= 10 && column >= PANEL_MIN_COLUMN && prefix.ends_with(whitespace)
            && cells(line).len() + 2 >= screen
            && rows.iter().all(|r| !r.get(column - 1).copied().unwrap_or(false))).then_some(column)
    })
}

/// As linhas da conversa: acima da caixa de digitar e da faixa dos mods.
fn conversation<'a, 'b>(lines: &'a [&'b str], anchor: Option<&str>) -> &'a [&'b str] {
    let end = lines.iter().rposition(|l| P.rule.is_match(l) || P.overlay_rule.is_match(l) || P.pi_box.is_match(l))
        .unwrap_or(lines.len());
    let end = anchor.filter(|a| !a.is_empty()).and_then(|a| lines[..end].iter().rposition(|l| l.contains(a))).unwrap_or(end);
    &lines[..end]
}

/// A prévia sem o painel lateral do Claude Code: sem o corte, as linhas dele entram grudadas na prosa.
fn preview(lines: &[&str], anchor: Option<&str>, skip: impl Fn(&[&str], usize, &str) -> bool, stop: impl Fn(&str) -> bool) -> String {
    let Some(column) = side_panel_column(conversation(lines, anchor)) else { return preview_rows(lines, anchor, skip, stop) };
    // A régua da caixa de digitar fica inteira: cortada no nome da sessão, deixaria de ser régua.
    let cut: Vec<String> = lines.iter().map(|l| if l.starts_with('─') { (*l).to_owned() } else { cells_before(l, column) }).collect();
    preview_rows(&cut.iter().map(String::as_str).collect::<Vec<_>>(), anchor, skip, stop)
}

/// `anchor`: começo da faixa dos mods, que fica entre a conversa e a caixa de digitar e pode
/// começar com ●; a ocorrência mais baixa dela fecha a conversa.
fn preview_rows(lines: &[&str], anchor: Option<&str>, skip: impl Fn(&[&str], usize, &str) -> bool, stop: impl Fn(&str) -> bool) -> String {
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

/// A tela do `agy` (Antigravity CLI). Pergunta: opções numeradas com cursor `>` acima do rodapé `↑/↓ Navigate`;
/// trabalhando: rodapé `esc to cancel`, com o rótulo do spinner em braile; parado: o resto (`? for shortcuts`).
pub fn agy_analyze(pane: &str) -> PaneAnalysis {
    let lines = lines(pane);
    let mut result = PaneAnalysis { preview: other_pane_preview("antigravity", pane), ..Default::default() };
    let onboarding = agy_onboarding_list(pane).map(|(title, options, _)| TerminalQuestion { question: Some(title), options });
    if let Some(menu) = agy_terms_question(&lines).or(onboarding).or_else(|| agy_menu(&lines)) {
        result.state = "awaiting_input".into();
        result.question = menu.question;
        result.options = Some(menu.options);
        return result;
    }
    let footer = lines.iter().rev().map(|l| trim(l)).find(|l| !l.is_empty()).unwrap_or("");
    if footer.starts_with("esc to cancel") {
        result.state = "working".into();
        result.label = lines.iter().rev().find(|l| kimi_spinner(l))
            .map(|l| trim(&left(l).chars().skip(1).collect::<String>()).to_owned())
            .filter(|l| !l.is_empty());
        result.spinner = result.label.clone();
    }
    result
}

/// O título da tela dos Termos do Google, a primeira vez que uma conta abre o `agy`.
pub const AGY_TERMS_TITLE: &str = "Terms of Service & Data Use";
/// As respostas do cartão dos Termos, na ordem do `option` do `/select` (1 = primeira). Quem escolhe é a
/// pessoa: o Hangar só aperta as teclas da opção que ela tocou.
pub const AGY_TERMS_OPTIONS: [&str; 3] = [
    "Done — [x] Yes, I agree to help improve Antigravity CLI (o Google coleta os dados de interação)",
    "Done — [ ] sem compartilhar os dados de interação",
    "Previous",
];

/// Onde está o cursor na tela dos Termos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgyTermsFocus { Checkbox, Previous, Done }

/// A tela dos Termos lida: a caixa de compartilhar os dados marcada ou não e o foco.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgyTerms { pub checked: bool, pub focus: AgyTermsFocus }

/// A próxima tecla na tela dos Termos para chegar à opção tocada (`AGY_TERMS_OPTIONS`, 1 = primeira).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgyTermsKey { Up, Down, Left, Right, Enter }

impl AgyTermsKey {
    /// O nome da tecla no `send-keys`.
    pub fn name(self) -> &'static str {
        match self { Self::Up => "Up", Self::Down => "Down", Self::Left => "Left", Self::Right => "Right", Self::Enter => "Enter" }
    }
}

/// Uma volta: acerta a caixa de dados como a pessoa escolheu e leva o foco ao botão. `Enter` fora do checkbox é o
/// aperto final do botão; no checkbox, só troca a caixa.
pub fn agy_terms_key(terms: AgyTerms, option: usize) -> Option<AgyTermsKey> {
    use AgyTermsFocus::{Checkbox, Done, Previous};
    let (checked, button) = match option { 1 => (Some(true), Done), 2 => (Some(false), Done), 3 => (None, Previous), _ => return None };
    Some(if checked.is_some_and(|want| want != terms.checked) {
        if terms.focus == Checkbox { AgyTermsKey::Enter } else { AgyTermsKey::Up }
    } else if terms.focus == button {
        AgyTermsKey::Enter
    } else {
        match (terms.focus, button) {
            (Checkbox, _) => AgyTermsKey::Down,
            (Previous, Done) => AgyTermsKey::Right,
            _ => AgyTermsKey::Left,
        }
    })
}

/// A tela de entrar do `agy` sem login: o aviso de que não há conta e a lista dos jeitos de entrar.
pub fn agy_login_screen(screen: &str) -> bool {
    let lines = lines(screen);
    let Some(notice) = lines.iter().position(|l| trim(l).ends_with("You are currently not signed in.")) else { return false };
    lines[notice..].iter().any(|l| trim(l) == "Select login method:")
}

pub fn agy_terms(pane: &str) -> Option<AgyTerms> {
    let lines = lines(pane);
    lines.iter().position(|l| trim(l) == AGY_TERMS_TITLE)?;
    let check = lines.iter().map(|l| trim(l)).find(|l| (l.contains("[x]") || l.contains("[ ]")) && l.contains("I agree"))?;
    let buttons = lines.iter().map(|l| trim(l)).rfind(|l| l.contains("Previous") && l.contains("Done"))?;
    let focus = if check.starts_with('>') { AgyTermsFocus::Checkbox }
        else if buttons.starts_with('>') { AgyTermsFocus::Previous }
        else if buttons.contains("> Done") || buttons.contains(">  Done") { AgyTermsFocus::Done }
        else { return None };
    Some(AgyTerms { checked: check.contains("[x]"), focus })
}

/// A primeira tela do primeiro uso (`Choose your color scheme:`): as opções da coluna da esquerda, com o cursor
/// `>`; o quadro à direita é só a amostra. `(título, opções, índice do cursor)`.
pub fn agy_onboarding_list(pane: &str) -> Option<(String, Vec<String>, usize)> {
    let lines = lines(pane);
    let title = lines.iter().position(|l| trim(l).starts_with("Choose your color scheme"))?;
    let next = lines.iter().rposition(|l| trim(l) == "[Next]" || trim(l) == "> Next" || trim(l) == ">  Next")?;
    // Os marcadores fora de ordem (texto de conversa, não a tela) fariam o fatiamento entrar em pânico.
    if next <= title { return None; }
    let left_of = |line: &str| trim(line.split('│').next().unwrap_or(line)).to_owned();
    let mut options = Vec::new();
    let mut cursor = None;
    for line in &lines[title + 1..next] {
        let text = left_of(line);
        // A amostra continua abaixo da lista: a primeira linha sem texto à esquerda encerra as opções.
        if text.is_empty() || text.starts_with(['╭', '╰']) { if options.is_empty() { continue } else { break } }
        if let Some(rest) = text.strip_prefix('>') { cursor = Some(options.len()); options.push(trim(rest).to_owned()); } else { options.push(text); }
    }
    let title = lines[title].split('╭').next().unwrap_or_default().trim().to_owned();
    (options.len() >= 2).then_some((title, options, cursor?))
}

/// O cartão dos Termos: o aviso, o texto da caixa e os links, como a tela mostra.
fn agy_terms_question(lines: &[&str]) -> Option<TerminalQuestion> {
    let title = lines.iter().position(|l| trim(l) == AGY_TERMS_TITLE)?;
    let buttons = lines.iter().rposition(|l| trim(l).contains("Previous") && trim(l).contains("Done"))?;
    if buttons <= title { return None; }
    let body: Vec<&str> = lines[title..buttons].iter().map(|l| trim(l)).filter(|l| !l.is_empty() && !l.chars().all(|c| c == '-'))
        .map(|l| l.trim_start_matches('>').trim()).collect();
    Some(TerminalQuestion { question: Some(body.join("\n")), options: AGY_TERMS_OPTIONS.iter().map(|o| (*o).to_owned()).collect() })
}

fn agy_option(line: &str) -> Option<String> {
    static OPTION: LazyLock<Regex> = LazyLock::new(|| terminal_regex(r"^\s*>?\s*\d+\.\s+(.*\S)\s*$"));
    OPTION.captures(line).map(|c| c[1].to_owned())
}

fn agy_menu(lines: &[&str]) -> Option<TerminalQuestion> {
    let nav = lines.iter().rposition(|l| l.contains("↑/↓ Navigate"))?;
    let mut end = nav;
    while end > 0 && trim(lines[end - 1]).is_empty() { end -= 1; }
    let mut start = end;
    while start > 0 && agy_option(lines[start - 1]).is_some() { start -= 1; }
    let options: Vec<String> = lines[start..end].iter().filter_map(|l| agy_option(l)).collect();
    if options.len() < 2 { return None; }
    let above = lines[..start].iter().rposition(|l| !trim(l).is_empty())?;
    let mut question = trim(lines[above]).to_owned();
    // "Requesting permission for:" e o comando, logo acima da pergunta.
    if let Some(request) = lines[..above].iter().rposition(|l| trim(l) == "Requesting permission for:") {
        let target: Vec<&str> = lines[request + 1..above].iter().map(|l| trim(l)).filter(|l| !l.is_empty()).collect();
        if !target.is_empty() { question = format!("{question}\n{}", target.join("\n")); }
    }
    Some(TerminalQuestion { question: Some(question), options })
}

/// O rodapé do `agy`, à direita da última linha: `[modo · ]Modelo · esforço` (`auto · Gemini 4 Neon · low`).
/// O modo padrão, o que pergunta antes de agir, não tem rótulo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgyFooter {
    /// Nos nomes do Claude: `manual` (o padrão do `agy`), `acceptEdits`, `auto` ou `plan`. `None`: rótulo
    /// que o Hangar não conhece, que nunca vira um modo chutado.
    pub mode: Option<&'static str>,
    /// O nome que a tela mostra (`Gemini 4 Neon`).
    pub model: String,
    pub effort: Option<String>,
}

impl AgyFooter {
    /// O id do modelo como o `agy` o escreve (`gemini-4-neon-low`), comparável por `agy_model_key`.
    pub fn model_id(&self) -> String {
        let base = agy_model_key(&self.model);
        match &self.effort {
            Some(effort) => format!("{base}-{effort}"),
            None => base,
        }
    }
    /// O id (do catálogo ou do transcript) é o modelo deste rodapé?
    pub fn is_model(&self, id: &str) -> bool { agy_model_key(id) == self.model_id() }
}

/// Nome ou id de modelo do `agy` comparáveis: minúsculas, com espaço, ponto e `_` virando hífen.
pub fn agy_model_key(text: &str) -> String {
    trim(text).chars().flat_map(|c| if c.is_whitespace() || c == '.' || c == '_' { vec!['-'] } else { c.to_lowercase().collect() }).collect()
}

/// O rodapé do `agy` na última linha com texto; à esquerda pode vir `esc to cancel` ou `? for shortcuts`.
pub fn agy_footer(pane: &str) -> Option<AgyFooter> {
    static GAP: LazyLock<Regex> = LazyLock::new(|| terminal_regex(r"\s{2,}"));
    let lines = lines(pane);
    let last = lines.iter().rev().map(|l| trim(l)).find(|l| !l.is_empty())?;
    let segment = GAP.split(last).last()?;
    let parts: Vec<&str> = segment.split(" \u{b7} ").map(trim).collect();
    let named = |part: &str| part.chars().next().is_some_and(|c| c.is_uppercase() || c.is_ascii_digit());
    let label = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let at = parts.iter().position(|part| named(part))?;
    let (before, after) = (&parts[..at], &parts[at + 1..]);
    // Sem `·` não é o rodapé: qualquer linha com maiúscula passaria por nome de modelo.
    if parts.len() < 2 || before.len() > 1 || after.len() > 1 || !before.iter().chain(after).all(|part| label(part)) {
        return None;
    }
    let mode = match before.first().copied() {
        None => Some("manual"),
        Some("auto") => Some("auto"),
        Some("plan") => Some("plan"),
        Some("accept-edits") => Some("acceptEdits"),
        Some(_) => None,
    };
    Some(AgyFooter { mode, model: parts[at].to_owned(), effort: after.first().map(|effort| (*effort).to_owned()) })
}

/// As confirmações do `/model` na tela (`⎿  Model set to Gemini 4 Neon (Medium)`), da mais antiga à mais nova.
pub fn agy_model_set(pane: &str) -> Vec<String> {
    static SET: LazyLock<Regex> = LazyLock::new(|| terminal_regex(r"Model set to (\S.*\S)\s*$"));
    lines(pane).iter().filter_map(|line| SET.captures(line).map(|c| c[1].to_owned())).collect()
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
    // Sem pergunta e sem a linha viva (a do verbo com "…"), `awaiting_input` é o aviso de ociosidade do Claude
    // Code: a sessão está parada, e o resumo do turno anterior na tela não pode voltar a valer como `working`.
    // Com a linha viva, é a permissão já respondida e o turno segue: fica como antes, a tela decide.
    let live = analysis.spinner.as_deref().is_some_and(|s| s.contains('…'));
    let quiet = |s: Option<String>| s.map(|s| if s == "awaiting_input" && !live { "idle".to_owned() } else { s });
    let (plugin_state, hook_state) = (quiet(facts.plugin_state), quiet(facts.hook_state));
    if matches!(analysis.state.as_str(), "working" | "idle") {
        if let Some(plugin) = plugin_state.filter(|s| matches!(s.as_str(), "working" | "idle")) {
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
        match hook_state.as_deref() {
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

    /// Uma linha do terminal com a conversa à esquerda e o painel lateral do Claude Code a partir da coluna 111.
    fn beside(conversation: &str, panel: &str) -> String { format!("{conversation:<111}{panel}") }

    #[test]
    fn the_side_diff_panel_is_not_part_of_the_reply() {
        let rows = [
            beside("● Com pilha de 8 MB o teste passa, então a recursão termina: no Windows, em debug, esse caminho só",
                "3 files changed +21 -12                                                      ✕"),
            beside("  precisa de mais de 2 MB de pilha.", "crates/hangar-server/src/mcp/integrations.rs   +9 -7"),
            beside("", ""),
            beside("  Thought for 23s, searched for 1 pattern, listed 1 directory · 1 failed", &"─".repeat(89)),
            beside("", " 434         std::fs::create_dir_all(run.join(\"hypr/morta_1\")).unwrap();"),
            beside("● Com 8 MB de pilha o teste passa, então a recursão é só o caminho list_tools → filtro de projeto,",
                " 435         let vars = vec![(\"HYPRLAND_INSTANCE_SIGNATURE\".to_owned(), \"morta\""),
            beside("  que estoura no debug do Windows com os 2 MB padrão do tokio.", " 437 -       assert_eq!(register(&home.paths, || O"),
            beside("", " 438         assert!(!home.paths.main_file().exists());"),
            "● Running scp -q -o BatchMode=yes /tmp/x.patch".to_owned(),
        ];
        let wide = "─".repeat(200);
        let pane = format!("{}\n✻ Puzzling… (29m 39s · ↓ 21.4k tokens)\n{wide}\n❯ \n{wide}\n", rows.join("\n"));
        assert_eq!(pane_preview(&pane, None).0,
            "Com 8 MB de pilha o teste passa, então a recursão é só o caminho list_tools → filtro de projeto,\n  que estoura no debug do Windows com os 2 MB padrão do tokio.");
    }

    #[test]
    fn a_rule_inside_the_reply_is_not_a_side_panel() {
        let wide = "─".repeat(200);
        let pane = format!("● Antes                       ────────────\n  depois da régua, texto que passa da coluna trinta\n{wide}\n❯ \n{wide}\n");
        assert_eq!(pane_preview(&pane, None).0, "Antes                       ────────────\n  depois da régua, texto que passa da coluna trinta");
    }

    #[test]
    fn the_named_input_rule_is_not_a_side_panel() {
        let reply = "● Resposta longa que passa da coluna vinte e cinco com espaço simples entre as palavras.";
        let pane = format!("{reply}\n  src/a.rs     | 3 +-\n  src/bb.rs    | 9 ++++----\n──────────── minha-sessao {}\n❯ \n{}\n", "─".repeat(170), "─".repeat(200));
        assert_eq!(pane_preview(&pane, None).0, "Resposta longa que passa da coluna vinte e cinco com espaço simples entre as palavras.\n  src/a.rs     | 3 +-\n  src/bb.rs    | 9 ++++----");
    }

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

    /// Resumo do turno anterior, que o Claude Code deixa na tela com uma tarefa de fundo rodando.
    const DONE: &str = "✻ Cooked for 2m 52s · done 22:55 · 1 monitor still running";

    #[test]
    fn the_idle_notice_after_a_finished_turn_keeps_the_session_idle() {
        let pane = format!("● resposta\n\n{DONE}\n\n{RULE}\n❯ \n{RULE}\n");
        let facts = |plugin: Option<&str>, hook: Option<&str>| super::ReducerFacts {
            plugin_state: plugin.map(String::from), hook_state: hook.map(String::from), ..Default::default() };
        for (plugin, hook) in [(Some("awaiting_input"), None), (None, Some("awaiting_input")), (Some("awaiting_input"), Some("awaiting_input"))] {
            let state = super::reduce(&pane, super::ReducerMemory::default(), facts(plugin, hook));
            assert_eq!(state.analysis.state, "idle", "{plugin:?} {hook:?}");
            assert_eq!(state.analysis.label, None, "{plugin:?} {hook:?}");
        }
        // Permissão respondida e o turno seguindo: a linha viva decide, mesmo na primeira rodada, sem animação vista.
        let live = format!("● resposta\n\n✻ Precipitating… (2m 12s)\n\n{RULE}\n❯ \n{RULE}\n");
        for (plugin, hook) in [(Some("awaiting_input"), None), (None, Some("awaiting_input"))] {
            let state = super::reduce(&live, super::ReducerMemory::default(), facts(plugin, hook));
            assert_eq!(state.analysis.state, "working", "{plugin:?} {hook:?}");
        }
    }

    #[test]
    fn every_frame_of_the_live_spinner_wins_over_the_previous_turn_summary() {
        for glyph in ['✻', '✽', '*', '✶', '✢', '·'] {
            let pane = format!("{DONE}\n\n● resposta em andamento\n\n{glyph} Precipitating… (2m 12s · ↓ 9.3k tokens)\n\n{RULE}\n❯ \n{RULE}\n");
            let analysis = super::analyze(&pane);
            assert_eq!(analysis.state, "working", "{glyph}");
            assert_eq!(analysis.label.as_deref(), Some("Precipitating… (2m 12s · ↓ 9.3k tokens)"), "{glyph}");
        }
        // Painel de mod entre a linha viva e a caixa: no quadro do `*` não há linha viva reconhecida, e o resumo do
        // turno anterior, com texto mais novo abaixo dele, não vale no lugar dela.
        let band = "▸ PR #381 fix(state): keep the Claude working line ✓14 juntado\n? ▸ Terceiro PR (aguardando)";
        let pane = format!("{DONE}\n\n● resposta nova\n\n* Kerfuffling… (2m 26s · ↓ 4.9k tokens)\n{band}\n\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(super::analyze(&pane).spinner, None);
        let pane = format!("{DONE}\n\n● resposta nova\n\n✻ Kerfuffling… (2m 26s · ↓ 4.9k tokens)\n{band}\n\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(super::analyze(&pane).label.as_deref(), Some("Kerfuffling… (2m 26s · ↓ 4.9k tokens)"));
        // Painel de mod aberto com o ponto de estado ("● ▾ …") entre a linha viva e a caixa: não é resposta nova.
        let band = "● ▾ PM-20124 coleta: troca de sessão no 403 (Execução manual)        8/9 passos · 2h 30m [-]\n  ━━━━━━━━ ━━━━━━━━  89%\n▸ ms_def · develop · Jenkins sem build";
        let pane = format!("● resposta\n\n✻ Waiting for 1 background agent to finish\n{band}\n{RULE}\n❯ \n{RULE}\n  ⏵⏵ bypass permissions on · 2 background tasks · ↓ to manage\n  ● main\n  ◯ pmedico:kubectl  Aguardando conclusão   40s · ↓ 79.2k tokens\n");
        assert_eq!(super::analyze(&pane).label.as_deref(), Some("Waiting for 1 background agent to finish"));
        // O quadro do `*` acima do painel com título também é a linha viva.
        let pane = format!("{DONE}\n\n● resposta nova\n\n* Kerfuffling… (2m 26s)\n{band}\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(super::analyze(&pane).label.as_deref(), Some("Kerfuffling… (2m 26s)"));
        // Resposta que começa com a mesma seta segue sendo resposta: o resumo do turno anterior acima dela não volta.
        let pane = format!("● anterior\n\n{DONE}\n\n❯ pergunta\n\n● ▸ primeiro item da lista\n\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(super::analyze(&pane).spinner, None);
        // Caixa no modo de comando.
        let pane = format!("{DONE}\n\n● resposta\n\n* Precipitating… (2m 12s)\n\n{RULE}\n! ls\n{RULE}\n");
        assert_eq!(super::analyze(&pane).label.as_deref(), Some("Precipitating… (2m 12s)"));
        // Resto de outro texto que ficou à direita na mesma linha não entra no rótulo.
        let pane = format!("● resposta\n\n✶ Thundering… (1m 15s · ↓ 5.3k tokens)                    -n\\n{{RULE}}\\n❯ \\n\");\n\n{RULE}\n❯ \n{RULE}\n");
        assert_eq!(super::analyze(&pane).label.as_deref(), Some("Thundering… (1m 15s · ↓ 5.3k tokens)"));
        // Fora do lugar da animação, ou sem o formato dela, o `*` segue sendo texto.
        for tail in ["* Precipitating… (2m 12s)\nfora", "* item da lista (veja acima)", "* passo seguinte…",
            &format!("* Precipitating… (2m)\n\n{RULE}\nconteúdo da resposta")] {
            let pane = format!("{DONE}\n\n● resposta\n{tail}\n\n{RULE}\n❯ \n{RULE}\n");
            assert_eq!(super::analyze(&pane).spinner, None, "{tail}");
        }
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
