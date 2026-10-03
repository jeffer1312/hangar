//! `analyze` fornece os fatos da captura usada em produção.
//! O redutor temporal abaixo é só referência da Parte 2B para as fixtures;
//! na Parte 2C, o Python mantém a memória e calcula o estado final.
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

/// Memória da referência da Parte 2B; não é mantida pelo observador em produção.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ReducerMemory {
    pub prev_spinner: Option<String>,
    pub frozen: u32,
    pub no_spinner: u32,
    pub held_state: String,
    pub held_label: Option<String>,
}

impl Default for ReducerMemory {
    fn default() -> Self {
        Self { prev_spinner: None, frozen: 0, no_spinner: 0, held_state: "idle".into(), held_label: None }
    }
}

/// Entradas da referência da Parte 2B; o contrato privado não as recebe.
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

/// Resultado da referência da Parte 2B, comparado às fixtures Python.
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
    unnumbered: Regex, user: Regex, banner: Regex, warning: Regex,
    tool: Regex, finished: Regex, activity: Regex, mcp: Regex,
    pi_box: Regex, overlay_rule: Regex, todo: Regex, ascii_spinner: Regex,
    subagent: Regex, subagent_body: Regex, digit: Regex, word: Regex,
}

static P: LazyLock<Patterns> = LazyLock::new(|| {
    // Python considera também os separadores de informação como espaço.
    let r = |s: &str| Regex::new(&s.replace(r"\s", r"[\s\x1c-\x1f]")
        .replace(r"\S", r"[^\s\x1c-\x1f]").replace(r"\w", r"[\p{L}\p{N}_]")).unwrap();
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
        rule: r(r"^[\s─]*─{10,}[\s─]*$"),
        box_bottom: r(r"^\s*╰[─\s]*╯\s*$"),
        frame: r(r"^[\s│─╭╮╰╯┌┐└┘├┤┬┴┼]*$"),
        footer: r(r"to navigate|Esc to cancel|Enter to select|Enter select"),
        login: insensitive(r"/oauth/authorize|Paste code here|Select login method|Choose the text style"),
        composer: r(r"⏵⏵|⏸"),
        limit: insensitive(r"(?:usage limit reached|hit your \w+ limit|limit reached)[^\n]{0,80}?(?:resets?|continuing automatically|try again)\s*(?:at\s*)?([0-9]{1,2}(?::[0-9]{2})?\s*(?:am|pm)?)"),
        codex_option: r(r"^\s*[›>]?\s*(\d+)\.\s+(.*\S)\s*$"),
        unnumbered: r(r"^(\s*❯\s+)\S"),
        user: r(r"^\s*❯"),
        banner: r(r"^[\s▐▛█▝▜▀]*Claude Code v\d"),
        warning: r(r"^[a-z][\w.-]*: hooks\.json: [a-z]"),
        tool: r(r"^([A-Z][\w-]*\(|(Running|Reading|Writing|Editing|Searching|Listing|Fetching|Updating|Creating|Deleting|Crawling|Downloading|Globbing|Grepping|Waiting|Loading|Compiling|Building|Installing|Ran|Making))"),
        finished: r(r#"^Agent "[^"]*" finished"#),
        activity: r(r"ran \d+ shell commands?\s*$"),
        mcp: r(r"^Calling[^\n]*(…|\.\.\.)\s*$"),
        pi_box: r(r"^\s*[╭╰][─\s]*[╮╯]\s*$"),
        overlay_rule: r(r"^[\s▔]*▔{10,}[\s▔]*$"),
        todo: r(r"^\s*[●○]?\s*Todos \(\d+/\d+\)\s*$"),
        ascii_spinner: r(r"^\*\s+\S[^\n]*(…|\))\s*$"),
        subagent: r(r"^Subagent\s+\S"),
        subagent_body: r(r"^\s*└"),
        digit: r(r"^\d$"),
        word: r(r"^\w$"),
    }
});

fn whitespace(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }
fn trim(s: &str) -> &str { s.trim_matches(whitespace) }
fn left(s: &str) -> &str { s.trim_start_matches(whitespace) }
fn right(s: &str) -> &str { s.trim_end_matches(whitespace) }
fn word(c: char) -> bool {
    let mut encoded = [0; 4];
    P.word.is_match(c.encode_utf8(&mut encoded))
}
fn end_word(pattern: &Regex, s: &str) -> bool {
    pattern.find(s).is_some_and(|m| s[m.end()..].chars().next().is_none_or(|c| !word(c)))
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
fn lines(pane: &str) -> Vec<&str> {
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
    let top = (0..cursor).rev().find(|&i| boundary(lines[i]) || left(lines[i]).starts_with(['☐', '☑']))
        .map_or(0, |i| i + 1);
    let bottom = (cursor + 1..lines.len()).find(|&i| P.footer.is_match(lines[i]) || boundary(lines[i]))
        .unwrap_or(lines.len());
    Some((top, bottom))
}

fn question(lines: &[&str]) -> Option<String> {
    let mut found = None;
    for line in lines {
        if option(line).is_some() { break; }
        let s = trim(trim(line).trim_matches('│'));
        if s.is_empty() || P.rule.is_match(line) || P.frame.is_match(line) || s.starts_with(['☐', '☑']) { continue; }
        found = Some(s.into());
    }
    found
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
    let options: Vec<_> = lines[top..bottom].iter().map(|l| trim(&l.chars().skip(col).collect::<String>()).into()).collect();
    if options.len() < 2 { return None; }
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

const CODEX_FOOTERS: [&str; 2] = ["press enter to confirm", "press enter to continue"];
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

fn preview(lines: &[&str]) -> String {
    let end = lines.iter().rposition(|l| P.rule.is_match(l) || P.overlay_rule.is_match(l) || P.pi_box.is_match(l))
        .unwrap_or(lines.len());
    let begin = lines[..end].iter().enumerate().filter(|(i, l)| P.user.is_match(l) && !(*i > 0 && P.rule.is_match(lines[*i - 1])))
        .map(|(i, _)| i + 1).last().unwrap_or(0);
    if begin == 0 && lines[..end].iter().any(|l| P.banner.is_match(l)) { return String::new(); }
    let mut start = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(begin) {
        let s = left(line);
        let body = left(s.chars().next().map_or("", |c| &s[c.len_utf8()..]));
        if !s.starts_with('●') { continue; }
        if P.warning.is_match(body) { start = None; continue; }
        let subagent = P.subagent.is_match(body) && lines[i + 1..].iter().find(|l| !trim(l).is_empty())
            .is_some_and(|l| P.subagent_body.is_match(l));
        if !tool(body) && !mcp(body) && !end_word(&P.finished, body)
            && !P.todo.is_match(line) && !subagent { start = Some(i); }
    }
    let Some(start) = start else { return String::new(); };
    let first = left(lines[start]);
    let mut out = vec![right(left(first.strip_prefix('●').unwrap_or(first)))];
    for line in &lines[start + 1..] {
        let s = left(line);
        if P.rule.is_match(line) || boundary(line) || P.user.is_match(line) || tool(s)
            || mcp(s) || P.todo.is_match(line) || P.ascii_spinner.is_match(s)
            || activity(s) { break; }
        out.push(right(line));
    }
    while out.last().is_some_and(|l| trim(l).is_empty()) { out.pop(); }
    out.join("\n")
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
        preview: preview(&lines), codex_menu: codex_menu(&lines), ..Default::default()
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

fn set_question(analysis: &mut PaneAnalysis, question: TerminalQuestion) {
    analysis.state = "awaiting_input".into(); analysis.label = None;
    analysis.question = question.question; analysis.options = Some(question.options);
}

/// Diagnóstico da referência da Parte 2B, sem uso no observador em produção.
#[derive(Serialize)]
pub struct ReducerDiagnostic {
    pub before_plugin: String,
    pub plugin_applied: bool,
}

/// Referência da Parte 2B para as fixtures; o estado final em produção é calculado no Python.
pub fn reduce(pane: &str, memory: ReducerMemory, facts: ReducerFacts) -> ReducedState {
    reduce_with_diagnostics(pane, memory, facts).0
}

/// Referência da Parte 2B com diagnóstico; não participa do contrato privado de captura.
pub fn reduce_with_diagnostics(pane: &str, mut memory: ReducerMemory, facts: ReducerFacts) -> (ReducedState, ReducerDiagnostic) {
    let mut analysis = analyze(pane);
    if analysis.state != "awaiting_input" && analysis.options.as_ref().is_none_or(Vec::is_empty) {
        if let Some(q) = facts.open_question { set_question(&mut analysis, q); }
    }
    if analysis.state != "awaiting_input" {
        if let Some(q) = facts.plugin_question {
            if q.get("id").and_then(Value::as_str).is_some_and(|s| s.starts_with("perm:")) {
                let tool = q.get("tool").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("?");
                let summary = q.get("resumo").and_then(Value::as_str).unwrap_or("");
                set_question(&mut analysis, TerminalQuestion { question: Some(format!("{tool}: {summary}")), options: vec!["Yes".into(), "No".into()] });
            } else if let Some(q) = q.get("questions").and_then(Value::as_array).and_then(|q| q.first()) {
                let question = q.get("question").and_then(Value::as_str).map(String::from);
                let options = q.get("options").and_then(Value::as_array).map(|options| options.iter()
                    .map(|o| o.get("label").and_then(Value::as_str).unwrap_or("").into()).collect()).unwrap_or_default();
                set_question(&mut analysis, TerminalQuestion { question, options });
            }
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
    let mut diagnostic = ReducerDiagnostic { before_plugin: analysis.state.clone(), plugin_applied: false };
    if matches!(analysis.state.as_str(), "working" | "idle") {
        if let Some(plugin) = facts.plugin_state.filter(|s| matches!(s.as_str(), "working" | "idle")) {
            if !(plugin == "idle" && animating) {
                diagnostic.plugin_applied = true;
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
