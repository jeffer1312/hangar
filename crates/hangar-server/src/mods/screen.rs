//! Leitura da tela dos mods numa captura com atributos (`capture-pane -p -e`): painel ao lado ou em
//! caixa, abas e a ativa, a célula do `✕`, a faixa (inteira, encolhida ou recolhida), diálogo, pesquisa,
//! foco e rascunho, no tmux (SGR relativo) e no psmux (SGR absoluto). Só no Rust, nos três sistemas; o
//! `plugin_screen.py` de hoje fica para a reserva sem Rust. Conferida contra as capturas da medição em
//! `tests/it/mods_screen.rs`.
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::terminal_state::is_rule;

/// Borda do painel ao lado da conversa quando ele tem o teclado ((i), (z)).
pub const FOCUS_BORDER: (u8, u8, u8) = (177, 185, 249);
pub const COLLAPSED_TEXT: &str = "plugin panel hidden";
pub const SURVEY_TEXT: &str = "How is Claude doing this session?";
/// Linhas acima da caixa de digitar onde a faixa pode estar.
pub const MAX_BAND_ROWS: usize = 20;
static SHRUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[↓↑] \d+ more").unwrap());

/// Faixas de largura dupla (East Asian Wide e Fullwidth do Unicode, as mesmas que o `cell_width` do
/// `plugin_screen.py` usa). Um caractere das capturas com largura errada desalinha o retrato.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC), (0x23F0, 0x23F0), (0x23F3, 0x23F3),
    (0x25FD, 0x25FE), (0x2614, 0x2615), (0x2648, 0x2653), (0x267F, 0x267F), (0x2693, 0x2693), (0x26A1, 0x26A1),
    (0x26AA, 0x26AB), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4), (0x26EA, 0x26EA),
    (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705), (0x270A, 0x270B),
    (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797),
    (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x2E80, 0x303E),
    (0x3041, 0x33FF), (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xA000, 0xA4CF), (0xA960, 0xA97F), (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE6F), (0xFF00, 0xFF60), (0xFFE0, 0xFFE6), (0x16FE0, 0x16FE4),
    (0x17000, 0x18AFF), (0x1B000, 0x1B2FF), (0x1F004, 0x1F004), (0x1F0CF, 0x1F0CF), (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A), (0x1F200, 0x1F251), (0x1F300, 0x1F320), (0x1F32D, 0x1F335), (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393), (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3), (0x1F3E0, 0x1F3F0), (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E), (0x1F440, 0x1F440), (0x1F442, 0x1F4FC), (0x1F4FF, 0x1F53D), (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567), (0x1F57A, 0x1F57A), (0x1F595, 0x1F596), (0x1F5A4, 0x1F5A4), (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5), (0x1F6CC, 0x1F6CC), (0x1F6D0, 0x1F6D2), (0x1F6D5, 0x1F6D7), (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC), (0x1F6F4, 0x1F6FC), (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0), (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945), (0x1F947, 0x1F9FF), (0x1FA70, 0x1FAFF), (0x20000, 0x2FFFD), (0x30000, 0x3FFFD),
];

/// Células que o caractere ocupa no terminal.
pub fn cell_width(c: char) -> usize {
    let u = c as u32;
    if (0x0300..=0x036F).contains(&u) || matches!(u, 0x200B | 0x200D | 0xFE0F) {
        return 0;
    }
    if WIDE.iter().any(|&(a, b)| a <= u && u <= b) { 2 } else { 1 }
}

pub fn cells(text: &str) -> usize {
    text.chars().map(cell_width).sum()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    /// Segunda célula de um caractere largo: não tem texto próprio.
    pub wide_tail: bool,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
    pub fg: Option<(u8, u8, u8)>,
}

fn blank() -> Cell {
    Cell { ch: ' ', wide_tail: false, bold: false, dim: false, inverse: false, fg: None }
}

/// Aplica um SGR, parâmetro a parâmetro: o psmux escreve o estado absoluto (`ESC[0;1;7;48;2;…m`) e o tmux
/// só a diferença (`ESC[1;7m`).
fn sgr(mut state: Cell, params: &str) -> Cell {
    let codes: Vec<u32> = if params.is_empty() { vec![0] } else { params.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect() };
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => { state.bold = false; state.dim = false; state.inverse = false; state.fg = None; }
            1 => state.bold = true,
            2 => state.dim = true,
            7 => state.inverse = true,
            22 => { state.bold = false; state.dim = false; }
            27 => state.inverse = false,
            39 => state.fg = None,
            code @ (38 | 48 | 58) => {
                // O `2` de `38;2;r;g;b` é cor exata, não esmaecido.
                if codes.get(i + 1) == Some(&2) {
                    if code == 38 && i + 4 < codes.len() {
                        state.fg = Some((codes[i + 2] as u8, codes[i + 3] as u8, codes[i + 4] as u8));
                    }
                    i += 4;
                } else if codes.get(i + 1) == Some(&5) {
                    if code == 38 { state.fg = None; }
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    state.ch = ' ';
    state.wide_tail = false;
    state
}

fn sgr_at(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i + 1) != Some(&'[') { return None; }
    let mut j = i + 2;
    while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';' || chars[j] == ':') { j += 1; }
    (chars.get(j) == Some(&'m')).then(|| (chars[i + 2..j].iter().collect(), j + 1))
}

fn other_escape_end(chars: &[char], i: usize) -> Option<usize> {
    match chars.get(i + 1)? {
        '[' => {
            let mut j = i + 2;
            while j < chars.len() && (chars[j].is_ascii_digit() || matches!(chars[j], ';' | ':' | '?')) { j += 1; }
            chars.get(j).filter(|c| ('@'..='~').contains(*c)).map(|_| j + 1)
        }
        ']' => {
            let mut j = i + 2;
            while j < chars.len() && chars[j] != '\u{7}' && chars[j] != '\u{1b}' { j += 1; }
            match chars.get(j) {
                Some('\u{7}') => Some(j + 1),
                Some('\u{1b}') if chars.get(j + 1) == Some(&'\\') => Some(j + 2),
                _ => None,
            }
        }
        '(' | ')' => chars.get(i + 2).filter(|c| c.is_ascii_alphanumeric()).map(|_| i + 3),
        _ => None,
    }
}

/// A captura em células. O estado de cor atravessa a quebra de linha, como no terminal; linha a menos no
/// fim (o psmux corta as linhas em branco) vira linha vazia.
pub fn parse_ansi(ansi: &str, columns: usize, rows: usize) -> Vec<Vec<Cell>> {
    let mut grid = vec![vec![blank(); columns]; rows];
    let mut state = blank();
    for (r, line) in ansi.split('\n').take(rows).enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let (mut col, mut i) = (0usize, 0usize);
        while i < chars.len() {
            let ch = chars[i];
            if ch == '\u{1b}' {
                if let Some((params, end)) = sgr_at(&chars, i) {
                    state = sgr(state, &params);
                    i = end;
                    continue;
                }
                i = other_escape_end(&chars, i).unwrap_or(i + 1);
                continue;
            }
            if ch == '\r' { i += 1; continue; }
            let width = cell_width(ch);
            if width > 0 && col + width <= columns {
                grid[r][col] = Cell { ch, wide_tail: false, ..state };
                if width == 2 { grid[r][col + 1] = Cell { ch: ' ', wide_tail: true, ..state }; }
            }
            col += width;
            i += 1;
        }
    }
    grid
}

fn text_of(cells: &[Cell]) -> String {
    cells.iter().filter(|c| !c.wide_tail).map(|c| c.ch).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Region {
    pub rows: (usize, usize),
    pub lo: usize,
    pub hi: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tab {
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Screen {
    #[serde(skip)]
    pub rows: usize,
    #[serde(skip)]
    pub columns: usize,
    #[serde(skip)]
    pub text: Vec<String>,
    pub prompt: Option<usize>,
    pub dialog: bool,
    pub survey: bool,
    pub placement: Option<&'static str>,
    pub border: Option<usize>,
    #[serde(rename = "box")]
    pub boxed: Option<(usize, usize)>,
    pub tab_row: Option<usize>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    pub close: Option<(usize, usize)>,
    pub body: Option<Region>,
    pub band: Option<Region>,
    pub band_state: &'static str,
    pub collapsed_row: Option<usize>,
    pub focus: Option<&'static str>,
    pub draft: String,
}

/// A régua de cima da caixa de digitar: régua, linha do ❯ e outra régua logo abaixo. Sem ela há um
/// diálogo ou seletor por cima ((y)).
fn prompt_row(text: &[String]) -> Option<usize> {
    (0..text.len().saturating_sub(1)).rev().find(|&i| is_rule(&text[i]) && text[i + 1].trim_start().starts_with('❯')
        && (i + 2..text.len().min(i + 12)).any(|j| is_rule(&text[j])))
}

/// Painel em caixa: `╭…✕─╮` (o `✕` na antepenúltima coluna) e a `╰…╯` de baixo ((g)).
fn find_box(grid: &[Vec<Cell>], columns: usize, rows: usize, limit: usize) -> Option<(usize, usize)> {
    if columns < 3 { return None; }
    let mut found = None;
    for (r, line) in grid.iter().enumerate().take(limit) {
        if line[0].ch == '╭' && line[columns - 3].ch == '✕' && line[columns - 2].ch == '─' && line[columns - 1].ch == '╮'
            && let Some(bottom) = (r + 1..rows).find(|&b| grid[b][0].ch == '╰' && grid[b][columns - 1].ch == '╯') {
            found = Some((r, bottom));
        }
    }
    found
}

/// Painel ao lado: a borda `│` que se repete da primeira linha até a régua do prompt (ou até o fim, com
/// um diálogo na tela); a coluna e a primeira linha sem ela.
fn find_border(grid: &[Vec<Cell>], columns: usize, rows: usize, limit: usize) -> (Option<usize>, Option<usize>) {
    if limit < 3 { return (None, None); }
    for (col, top) in grid[0].iter().enumerate().take(columns.saturating_sub(2)).skip(10) {
        if top.ch != '│' || top.wide_tail { continue; }
        let mut end = 1;
        while end < rows && grid[end][col].ch == '│' && !grid[end][col].wide_tail { end += 1; }
        if end >= limit { return (Some(col), Some(end)); }
    }
    (None, None)
}

fn as_cells(text: &str) -> Vec<Option<char>> {
    let mut out = Vec::new();
    for ch in text.chars() {
        match cell_width(ch) {
            0 => {}
            2 => { out.push(Some(ch)); out.push(None); }
            _ => out.push(Some(ch)),
        }
    }
    out
}

/// Cada título como ` título `, na ordem de abertura; ativa é a aba toda em negrito e inverso (o anel
/// sobre uma aba inativa é inverso sem negrito). Título que saiu da linha fica de fora ((x)).
fn find_tabs(grid: &[Vec<Cell>], row: usize, start: usize, titles: &[String]) -> Vec<Tab> {
    let line: Vec<Option<char>> = grid[row].iter().map(|c| (!c.wide_tail).then_some(c.ch)).collect();
    let mut tabs = Vec::new();
    let mut pos = start;
    for (index, title) in titles.iter().enumerate() {
        let target = as_cells(&format!(" {title} "));
        if target.len() > line.len() { continue; }
        let Some(at) = (pos..=line.len() - target.len()).find(|&p| line[p..p + target.len()] == target[..]) else { continue };
        let end = at + target.len() - 1;
        tabs.push(Tab { index, start: at, end, active: (at..=end).all(|c| grid[row][c].bold && grid[row][c].inverse) });
        pos = end + 1;
    }
    tabs
}

/// A célula exata do `✕`, nunca por busca do glifo, que aparece no conteúdo dos mods ((b)).
fn find_close(grid: &[Vec<Cell>], columns: usize, placement: Option<&str>, boxed: Option<(usize, usize)>) -> Option<(usize, usize)> {
    let (row, col) = match (placement, boxed) {
        (Some("dock"), _) => (0, columns - 2),
        (Some("inline"), Some((top, _))) => (top, columns - 3),
        _ => return None,
    };
    (grid[row][col].ch == '✕').then_some((row, col))
}

#[allow(clippy::too_many_arguments)]
fn find_band(grid: &[Vec<Cell>], text: &[String], prompt: Option<usize>, boxed: Option<(usize, usize)>, border: Option<usize>,
    columns: usize, anchor: Option<&str>, survey: bool) -> (Option<Region>, &'static str, Option<usize>) {
    let Some(prompt) = prompt else { return (None, "absent", None) };
    let first = boxed.map_or(0, |(_, bottom)| bottom + 1);
    let (lo, hi) = (0, border.unwrap_or(columns));
    if let Some(r) = (first.max(prompt.saturating_sub(3))..prompt).find(|&r| text[r].contains(COLLAPSED_TEXT)) {
        return (Some(Region { rows: (r, r + 1), lo, hi }), "collapsed", Some(r));
    }
    if let Some(r) = (first.max(prompt.saturating_sub(MAX_BAND_ROWS))..prompt).find(|&r| SHRUNK.is_match(text[r].trim_start())) {
        return (Some(Region { rows: (r, r + 1), lo, hi }), "shrunk", None);
    }
    let Some(anchor) = anchor.filter(|a| !a.is_empty() && !survey) else { return (None, "absent", None) };
    let lower = first.max(prompt.saturating_sub(MAX_BAND_ROWS));
    match (lower..prompt).rev().find(|&r| text_of(&grid[r][lo..hi.min(grid[r].len())]).contains(anchor)) {
        Some(row) => (Some(Region { rows: (row, prompt), lo, hi }), "full", None),
        None => (None, "absent", None),
    }
}

/// Quem tem o teclado, pela tela: não há evento quando ele volta ao prompt ((z)).
fn find_focus(grid: &[Vec<Cell>], prompt: Option<usize>, border: Option<usize>, border_end: Option<usize>,
    boxed: Option<(usize, usize)>, columns: usize) -> Option<&'static str> {
    if let (Some(col), Some(end)) = (border, border_end)
        && (1..end).any(|r| grid[r][col].fg == Some(FOCUS_BORDER)) {
        return Some("pane");
    }
    if let Some((top, _)) = boxed
        && !grid[top][0].dim {
        return Some("pane");
    }
    let prompt = prompt?;
    let first = boxed.map_or(0, |(_, bottom)| bottom + 1);
    let hi = border.unwrap_or(columns);
    if (first.max(prompt.saturating_sub(MAX_BAND_ROWS))..prompt).any(|r| grid[r][..hi].iter().any(|c| c.inverse)) {
        return Some("band");
    }
    Some("prompt")
}

/// O que está digitado no prompt, sem a sugestão esmaecida (`Try "…"`) nem o NBSP do psmux.
fn find_draft(grid: &[Vec<Cell>], text: &[String], prompt: Option<usize>) -> String {
    let Some(prompt) = prompt else { return String::new() };
    let mut parts = Vec::new();
    for r in prompt + 1..text.len() {
        if is_rule(&text[r]) { break; }
        parts.push(grid[r].iter().filter(|c| !c.dim && !c.wide_tail).map(|c| c.ch).collect::<String>());
    }
    let joined = parts.join("\n");
    let trimmed = joined.trim();
    trimmed.strip_prefix('❯').unwrap_or(trimmed).trim().to_string()
}

/// A tela dos mods numa captura com atributos. `titles` são os títulos dos painéis abertos, na ordem de
/// abertura (o espelho do plugin); `anchor` é o começo do primeiro texto da faixa (`tree::anchor`).
pub fn read_screen(ansi: &str, columns: usize, rows: usize, titles: &[String], anchor: Option<&str>) -> Screen {
    let grid = parse_ansi(ansi, columns, rows);
    let text: Vec<String> = grid.iter().map(|line| text_of(line)).collect();
    let prompt = prompt_row(&text);
    let limit = prompt.unwrap_or(rows);
    let boxed = find_box(&grid, columns, rows, limit);
    let (border, border_end) = if boxed.is_some() { (None, None) } else { find_border(&grid, columns, rows, limit) };
    let placement = if boxed.is_some() { Some("inline") } else if border.is_some() { Some("dock") } else { None };
    let tab_row = match (placement, boxed) { (Some("dock"), _) => Some(0), (Some("inline"), Some((top, _))) => Some(top + 1), _ => None };
    let many = titles.len() >= 2;
    let tabs = match (placement, tab_row) {
        (Some(p), Some(row)) if many => find_tabs(&grid, row, if p == "dock" { border.unwrap() + 1 } else { 1 }, titles),
        _ => Vec::new(),
    };
    let active = if many { tabs.iter().find(|t| t.active).map(|t| t.index) }
        else if placement.is_some() && !titles.is_empty() { Some(0) } else { None };
    let body = match (placement, boxed) {
        (Some("dock"), _) => Some(Region { rows: (1, border_end.unwrap()), lo: border.unwrap() + 1, hi: columns }),
        (Some("inline"), Some((top, bottom))) => Some(Region { rows: (top + 1 + usize::from(many), bottom), lo: 1, hi: columns - 1 }),
        _ => None,
    };
    let survey = prompt.is_some_and(|p| (p.saturating_sub(6)..p).any(|r| text[r].contains(SURVEY_TEXT)));
    let (band, band_state, collapsed_row) = find_band(&grid, &text, prompt, boxed, border, columns, anchor, survey);
    let close = find_close(&grid, columns, placement, boxed);
    let focus = find_focus(&grid, prompt, border, border_end, boxed, columns);
    let draft = find_draft(&grid, &text, prompt);
    Screen { rows, columns, prompt, dialog: prompt.is_none(), survey, placement, border, boxed, tab_row, tabs, active, close,
        body, band, band_state, collapsed_row, focus, draft, text }
}

/// Cada ocorrência inteira de `label` dentro da região: (linha, célula do meio do rótulo).
pub fn find_label(text: &[String], label: &str, rows: Range<usize>, lo: usize, hi: Option<usize>) -> Vec<(usize, usize)> {
    let target = label.trim();
    if target.is_empty() { return Vec::new(); }
    let width = cells(target);
    let mut hits = Vec::new();
    for r in rows {
        let Some(line) = text.get(r) else { continue };
        let mut from = 0;
        while let Some(found) = line[from..].find(target) {
            let at = from + found;
            let c0 = cells(&line[..at]);
            if c0 >= lo && hi.is_none_or(|hi| c0 + width <= hi) {
                hits.push((r, c0 + (width - 1) / 2));
            }
            from = at + line[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    hits
}

/// O rótulo só dentro da região: rótulos se repetem entre a faixa, os títulos e o corpo ((t)).
pub fn find_in(screen: &Screen, label: &str, region: &Region) -> Vec<(usize, usize)> {
    find_label(&screen.text, label, region.rows.0..region.rows.1, region.lo, Some(region.hi))
}
