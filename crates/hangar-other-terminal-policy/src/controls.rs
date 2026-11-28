use std::sync::LazyLock;

use hangar_transcript::Provider;
use regex::Regex;

static QUESTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\?\s+(\S.*?)\s*$").unwrap());
static PI_CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*>\s*(\d+)\.").unwrap());
static MULTIPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*[❯>]?\s*\d+\.\s*\[.?\]\s").unwrap());
pub(crate) static CURSOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*[>❯›]\s*(\d+)\.").unwrap());

pub fn multiple_choice(pane: &str) -> bool {
    MULTIPLE.is_match(pane)
}

pub fn menu_cursor(pane: &str) -> Option<usize> {
    CURSOR
        .captures_iter(pane)
        .last()
        .and_then(|c| c[1].parse().ok())
}

pub fn same_question(pane: &str, question: &str) -> bool {
    if question.is_empty() {
        return true;
    }
    let Some(capture) = QUESTION.captures(pane) else {
        return false;
    };
    let visible = capture[1].split_whitespace().collect::<Vec<_>>().join(" ");
    let expected = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let head_visible: String = visible.chars().take(40).collect();
    let head_expected: String = expected.chars().take(40).collect();
    !visible.is_empty()
        && !expected.is_empty()
        && (visible.starts_with(&head_expected) || expected.starts_with(&head_visible))
}
pub fn pi_cursor(provider: Provider, pane: &str) -> Option<usize> {
    if provider != Provider::Omp {
        return PI_CURSOR
            .captures_iter(pane)
            .last()
            .and_then(|c| c[1].parse().ok());
    }
    let lines: Vec<_> = pane.lines().collect();
    let top = lines
        .iter()
        .rposition(|line| line.contains('╭') && line.contains('─'))
        .or_else(|| {
            lines
                .iter()
                .rposition(|line| line.contains('┌') && line.contains('─'))
        })?;
    let mut position = 0;
    for line in &lines[top + 1..] {
        if line.trim_start().starts_with(['╰', '└']) && line.contains('─') {
            break;
        }
        let Some(line) = line.trim().strip_prefix('│') else {
            continue;
        };
        let line = line.trim();
        let selected = line.strip_prefix('\u{f054}').map(str::trim_start);
        if selected.unwrap_or(line).starts_with('\u{f10c}') {
            position += 1;
            if selected.is_some() {
                return Some(position);
            }
        }
    }
    None
}

pub fn allowed_key(value: &str, interactive: bool) -> Option<&str> {
    match value {
        "Up" | "Down" | "Left" | "Right" | "Enter" | "Escape" | "Tab" | "BTab" | "Space" => {
            Some(value)
        }
        "PageUp" | "PageDown" => Some(value),
        "Backspace" | "Delete" if interactive => Some(value),
        "Home" | "End" | "C-c" | "C-d" | "C-r" | "C-u" | "C-k" | "C-w" | "C-a" | "C-e" | "C-l"
        | "C-z" | "C-p" | "C-n" | "C-b" | "C-f" | "C-g"
            if interactive =>
        {
            Some(value)
        }
        _ => None,
    }
}

const KIMI_MODE_LABELS: [&str; 3] = ["Always Ask", "Ask When Needed", "Never Ask"];

fn kimi_mode_row(line: &str) -> Option<usize> {
    let label = line.trim().trim_start_matches('❯').trim();
    KIMI_MODE_LABELS.iter().position(|name| {
        label == *name
            || label
                .strip_prefix(name)
                .is_some_and(|tail| tail.trim() == "← current")
    })
}

pub struct KimiPermissionMenu {
    pub selected: usize,
    pub current: usize,
}

pub fn kimi_permission_menu(pane: &str) -> Option<KimiPermissionMenu> {
    let lines: Vec<_> = pane.lines().collect();
    let title = lines
        .iter()
        .rposition(|line| line.trim() == "Select permission mode")?;
    let rule =
        |line: &&str| line.matches('─').count() >= 10 && line.trim().chars().all(|ch| ch == '─');
    if !lines[..title]
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(rule)
    {
        return None;
    }
    let bottom = title + 1 + lines[title + 1..].iter().position(rule)?;
    // O seletor substitui a caixa: um título citado no histórico acima da caixa não é menu aberto.
    if lines[bottom + 1..]
        .iter()
        .any(|line| line.trim_start().starts_with('╭'))
    {
        return None;
    }
    let mut selected = None;
    let mut current = None;
    let mut present = [false; 3];
    for line in &lines[title + 1..bottom] {
        if let Some(index) = kimi_mode_row(line) {
            present[index] = true;
            if line.trim_start().starts_with('❯') {
                selected = Some(index);
            }
            if line.trim_end().ends_with("← current") {
                current = Some(index);
            }
        }
    }
    present
        .into_iter()
        .all(|seen| seen)
        .then_some(KimiPermissionMenu {
            selected: selected?,
            current: current?,
        })
}
