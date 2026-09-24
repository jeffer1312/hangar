//! Porta de `parseStatusLine` (core/statusline.ts): cada campo sai da linha crua da sessão, e o que
//! não aparece fica `None`. Desconhecido nunca vira zero.
use crate::{api::dto::SessionInfo, composer::basename};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusFields {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub ctx_used: Option<f64>,
    pub ctx_total: Option<f64>,
    pub ctx_pct: Option<f64>,
    pub turn_in: Option<f64>,
    pub turn_out: Option<f64>,
    pub cost_usd: Option<f64>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_reset: Option<String>,
    pub weekly_pct: Option<f64>,
    pub weekly_reset: Option<String>,
    pub monthly_pct: Option<f64>,
    pub monthly_reset: Option<String>,
    pub session_time: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub dirty: Option<bool>,
}

const BAR: char = '│';

// `parseFloat` sem vírgulas: o maior prefixo numérico vale, o resto é ignorado.
fn to_number(text: &str, unit: Option<char>) -> Option<f64> {
    let clean: String = text.chars().filter(|&c| c != ',').collect();
    let mut end = 0;
    let mut dot = false;
    for (i, c) in clean.char_indices() {
        if c.is_ascii_digit() { end = i + 1; }
        else if c == '.' && !dot { dot = true; }
        else { break; }
    }
    let base: f64 = clean[..end].parse().ok()?;
    Some(match unit.map(|u| u.to_ascii_lowercase()) { Some('k') => base * 1e3, Some('m') => base * 1e6, _ => base })
}

fn pct(value: f64) -> Option<f64> { value.is_finite().then(|| value.clamp(0.0, 100.0)) }

struct Pair { start: usize, end: usize, a: String, ua: Option<char>, b: String, ub: Option<char> }

fn number_run(s: &str, at: usize) -> usize {
    s[at..].char_indices().find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == ',')).map(|(i, _)| at + i).unwrap_or(s.len())
}

fn skip_ws(s: &str, at: usize) -> usize {
    s[at..].char_indices().find(|(_, c)| !c.is_whitespace()).map(|(i, _)| at + i).unwrap_or(s.len())
}

fn unit_at(s: &str, at: usize) -> (Option<char>, usize) {
    match s[at..].chars().next() { Some(c) if "kKmM".contains(c) => (Some(c), at + 1), _ => (None, at) }
}

// `num unit? / num unit?` começando exatamente em `at`, com os espaços opcionais do regex do core.
fn pair_at(s: &str, at: usize) -> Option<Pair> {
    let a_end = number_run(s, at);
    if a_end == at { return None; }
    let (ua, next) = unit_at(s, skip_ws(s, a_end));
    let slash = skip_ws(s, next);
    if !s[slash..].starts_with('/') { return None; }
    let b_start = skip_ws(s, slash + 1);
    let b_end = number_run(s, b_start);
    if b_end == b_start { return None; }
    let after = skip_ws(s, b_end);
    let (ub, end) = unit_at(s, after);
    let end = if ub.is_some() { end } else { after };
    Some(Pair { start: at, end, a: s[at..a_end].into(), ua, b: s[b_start..b_end].into(), ub })
}

fn pairs(s: &str) -> Vec<Pair> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < s.len() {
        if let Some(pair) = pair_at(s, at) { at = pair.end.max(at + 1); out.push(pair); continue; }
        at += s[at..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out
}

// `\bctx\s*par`: rótulo explícito do contexto (Pi, Kimi Code).
fn labelled(s: &str) -> Option<Pair> {
    let mut from = 0;
    while let Some(i) = s[from..].find("ctx").map(|i| i + from) {
        let boundary = s[..i].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if boundary {
            if let Some(mut pair) = pair_at(s, skip_ws(s, i + 3)) { pair.start = i; return Some(pair); }
        }
        from = i + 3;
    }
    None
}

// `EMOJI[^│]*?(\d+)\s*%\s*(?:[↺↻]\s*([^stops]+))?` tentado em cada ocorrência do emoji.
fn window(raw: &str, emoji: char, stops: &[char]) -> Option<(f64, Option<String>)> {
    for (i, _) in raw.match_indices(emoji) {
        let start = i + emoji.len_utf8();
        let segment_end = raw[start..].find(BAR).map(|j| start + j).unwrap_or(raw.len());
        for (j, c) in raw[start..segment_end].char_indices() {
            if !c.is_ascii_digit() { continue; }
            let d0 = start + j;
            let d1 = raw[d0..].char_indices().find(|(_, c)| !c.is_ascii_digit()).map(|(k, _)| d0 + k).unwrap_or(raw.len());
            let p = skip_ws(raw, d1);
            if !raw[p..].starts_with('%') { continue; }
            let value = pct(raw[d0..d1].parse::<f64>().ok()?)?;
            let after = skip_ws(raw, p + 1);
            let reset = raw[after..].strip_prefix(['↺', '↻']).and_then(|rest| {
                let rest = rest.trim_start();
                let end = rest.find(|c: char| c == BAR || stops.contains(&c)).unwrap_or(rest.len());
                let text = rest[..end].trim();
                (!text.is_empty()).then(|| text.to_owned())
            });
            return Some((value, reset));
        }
    }
    None
}

pub fn parse(raw: Option<&str>, session: Option<&SessionInfo>) -> Option<StatusFields> {
    // Codex e Claude sem terminal não publicam Git na linha; a lista já consulta o repositório.
    let git = session.filter(|s| s.provider == "codex" || s.headless).and_then(|s| {
        Some((basename(s.cwd.as_deref()?).to_owned(), s.branch.clone()?, s.git_dirty.map(|n| n > 0)))
    });
    let raw = raw.unwrap_or("");
    if raw.is_empty() && git.is_none() { return None; }
    let mut out = StatusFields::default();

    if let Some(i) = raw.find('🤖') {
        let rest = &raw[i + '🤖'.len_utf8()..];
        let stop = rest.find(['(', BAR, '👤']).unwrap_or(rest.len());
        let (name, tail) = (&rest[..stop], &rest[stop..]);
        let effort = if tail.starts_with('(') {
            tail.find(')').and_then(|close| {
                let after = tail[close + 1..].trim_start();
                (after.is_empty() || after.starts_with([BAR, '👤'])).then(|| &tail[1..close])
            }).map(Some)
        } else { Some(None) };
        if let Some(effort) = effort {
            let name = name.trim();
            if !name.is_empty() { out.model = Some(name.to_owned()); }
            let word: String = effort.unwrap_or("").chars().filter(|c| c.is_alphanumeric() || *c == '+' || *c == '-').collect();
            if !word.is_empty() { out.effort = Some(word); }
        }
    }

    if let Some(i) = raw.find('💬') {
        let rest = &raw[i + '💬'.len_utf8()..];
        let segment = &rest[..rest.find(BAR).unwrap_or(rest.len())];
        let all = pairs(segment);
        let label = labelled(segment);
        // Um par só é do turno; sem rótulo, contexto exige dois pares (in>out não vira 100% falso).
        let (turn, last) = match &label {
            Some(label) => (all.iter().find(|p| p.start < label.start || p.start >= label.end), Some(label)),
            None if all.len() >= 2 => (all.first(), all.last()),
            None => (None, None),
        };
        if let Some(turn) = turn.filter(|t| last.is_none_or(|l| !std::ptr::eq(*t, l))) {
            out.turn_in = to_number(&turn.a, turn.ua);
            out.turn_out = to_number(&turn.b, turn.ub);
        }
        if let Some(last) = last {
            let used = to_number(&last.a, last.ua);
            out.ctx_used = used;
            if let Some(total) = to_number(&last.b, last.ub).filter(|t| *t > 0.0) {
                out.ctx_total = Some(total);
                out.ctx_pct = used.and_then(|u| pct(u / total * 100.0));
            }
        }
    }

    if let Some(i) = raw.find('💵') {
        let rest = raw[i + '💵'.len_utf8()..].trim_start();
        let rest = rest.strip_prefix('$').unwrap_or(rest).trim_start();
        let end = number_run(rest, 0);
        if end > 0 { out.cost_usd = to_number(&rest[..end], None); }
    }

    if let Some((value, reset)) = window(raw, '⚡', &['⚡', '📅', '🕐']) { out.five_hour_pct = Some(value); out.five_hour_reset = reset; }
    if let Some((value, reset)) = window(raw, '📅', &['🕐', '🗓', '⚡']) { out.weekly_pct = Some(value); out.weekly_reset = reset; }
    if let Some((value, reset)) = window(raw, '🗓', &['🕐', '⚡', '📅']) { out.monthly_pct = Some(value); out.monthly_reset = reset; }

    if let Some(i) = raw.find('⏱') {
        let rest = raw[i + '⏱'.len_utf8()..].trim_start();
        let end = rest.find(|c: char| !(c.is_ascii_digit() || "hms:".contains(c))).unwrap_or(rest.len());
        if end > 0 { out.session_time = Some(rest[..end].to_owned()); }
    }

    for (i, _) in raw.match_indices('📁') {
        let rest = &raw[i + '📁'.len_utf8()..];
        let Some(open) = rest.find(['[', ']', BAR]).filter(|&j| rest[j..].starts_with('[')) else { continue };
        let Some(close) = rest[open + 1..].find(']').map(|j| open + 1 + j).filter(|&j| j > open + 1) else { continue };
        let name = rest[..open].trim();
        if name.is_empty() { continue; }
        let inner = rest[open + 1..close].trim();
        out.repo = Some(name.to_owned());
        out.dirty = Some(inner.ends_with('*'));
        out.branch = Some(inner.trim_end_matches('*').trim().to_owned());
        break;
    }

    if let Some((repo, branch, dirty)) = git {
        out.repo = Some(repo);
        out.branch = Some(branch);
        out.dirty = dirty;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex(branch: Option<&str>, dirty: Option<i64>) -> SessionInfo {
        SessionInfo { name: "cx".into(), provider: "codex".into(), cwd: Some("C:\\Projetos\\hangar".into()),
            branch: branch.map(str::to_owned), git_dirty: dirty, ..Default::default() }
    }

    #[test]
    fn codex_and_headless_git_come_from_the_list() {
        let f = parse(None, Some(&codex(Some("main"), Some(2)))).unwrap();
        assert_eq!((f.repo.as_deref(), f.branch.as_deref(), f.dirty), (Some("hangar"), Some("main"), Some(true)));
        let f = parse(Some("🤖 gpt-6-astra │ 💬 ctx 20k/100k"), Some(&codex(Some("fix"), Some(0)))).unwrap();
        assert_eq!((f.model.as_deref(), f.ctx_pct, f.branch.as_deref(), f.dirty), (Some("gpt-6-astra"), Some(20.0), Some("fix"), Some(false)));
        assert!(parse(None, Some(&codex(None, Some(1)))).is_none());
        assert!(parse(None, Some(&SessionInfo { provider: "claude".into(), ..codex(Some("m"), None) })).is_none());
        let headless = SessionInfo { provider: "claude".into(), headless: true, cwd: Some("/projetos/hangar".into()),
            branch: Some("feature/headless".into()), git_dirty: Some(1), ..Default::default() };
        let f = parse(Some("🤖 Opus5·1M │ ⚡5h:50%"), Some(&headless)).unwrap();
        assert_eq!((f.repo.as_deref(), f.branch.as_deref(), f.dirty), (Some("hangar"), Some("feature/headless"), Some(true)));
    }

    #[test]
    fn context_needs_two_pairs_or_a_label() {
        let f = parse(Some("💬 20k/1k 40k/200k"), None).unwrap();
        assert_eq!((f.ctx_used, f.ctx_total, f.ctx_pct), (Some(40_000.0), Some(200_000.0), Some(20.0)));
        let f = parse(Some("💬 20k/1k"), None).unwrap();
        assert_eq!((f.ctx_pct, f.ctx_used, f.turn_in, f.turn_out), (None, None, None, None));
        let f = parse(Some("💬 271k/590 270k/1M"), None).unwrap();
        assert_eq!((f.turn_in, f.turn_out, f.ctx_used), (Some(271_000.0), Some(590.0), Some(270_000.0)));
    }

    #[test]
    fn pi_line() {
        let pi = "🤖 cline-pass/kimi-k3 (high) │ 📁 jefferson │ 📟 jefferson │ 💬 sessão 251kin/10kout · cache 2M · total 2.3M ctx 97k/1M │ ⚡5h:9% 📅7d:4% 🗓30d:2% │ 💵 $1.29 │ ⏱ 3h4m │ 🕐 22:40";
        let f = parse(Some(pi), None).unwrap();
        assert_eq!((f.ctx_used, f.ctx_total), (Some(97_000.0), Some(1_000_000.0)));
        assert_eq!(f.ctx_pct.map(f64::round), Some(10.0));
        assert_eq!((f.model.as_deref(), f.effort.as_deref(), f.cost_usd), (Some("cline-pass/kimi-k3"), Some("high"), Some(1.29)));
        assert_eq!((f.five_hour_pct, f.weekly_pct, f.monthly_pct, f.session_time.as_deref()), (Some(9.0), Some(4.0), Some(2.0), Some("3h4m")));
        assert_eq!(f.repo, None);
        let claude = parse(Some("🤖 Opus5 (high✦) │ 📁 hangar [main*] │ 💬 474k/220 470k/1M │ 💵 $169.89"), None).unwrap();
        assert_eq!((claude.ctx_used, claude.ctx_total, claude.effort.as_deref()), (Some(470_000.0), Some(1_000_000.0), Some("high")));
    }

    #[test]
    fn reset_arrows_and_windows_split_by_space() {
        let kimi = "🤖 k3-256k (high) │ 📁 pi │ 💬 sessão 28kin/4kout · cache 453k · total 484k ctx 28k/262k │ ⚡5h:51% ↻54m 📅7d:10% ↻6d19h │ 💵 $0.00 │ ⏱ 2h32m │ 🕐 19:04";
        let f = parse(Some(kimi), None).unwrap();
        assert_eq!((f.five_hour_pct, f.five_hour_reset.as_deref(), f.weekly_pct, f.weekly_reset.as_deref()), (Some(51.0), Some("54m"), Some(10.0), Some("6d19h")));
        let f = parse(Some("🤖 Opus5 │ ⚡5h:46% ↺34m 📅7d:57% ↺sab 18h │ 💵 $1.00"), None).unwrap();
        assert_eq!((f.five_hour_reset.as_deref(), f.weekly_reset.as_deref()), (Some("34m"), Some("sab 18h")));
        let f = parse(Some("🤖 k3 │ ⚡5h:51% ↻54m 📅7d:10% ↻6d19h 🗓30d:2% │ 💵 $0.00 │ 🕐 19:04"), None).unwrap();
        assert_eq!((f.five_hour_reset.as_deref(), f.weekly_reset.as_deref(), f.monthly_pct, f.monthly_reset.as_deref()), (Some("54m"), Some("6d19h"), Some(2.0), None));
    }

    #[test]
    fn kimi_code_line() {
        let line = "🤖 K3 (high✦) │ 📁 hangar [main*] │ 💬 ctx 480k/1M │ ⚡5h:3% ↺50m │ 📅7d:33% ↺seg 14h·5d6h │ 🕐 08:09 ⏱ 15h13m";
        let f = parse(Some(line), None).unwrap();
        assert_eq!((f.model.as_deref(), f.effort.as_deref(), f.ctx_used, f.ctx_total), (Some("K3"), Some("high"), Some(480_000.0), Some(1_000_000.0)));
        assert_eq!((f.ctx_pct.map(f64::round), f.turn_in), (Some(48.0), None));
        assert_eq!((f.five_hour_pct, f.five_hour_reset.as_deref(), f.weekly_reset.as_deref(), f.session_time.as_deref()), (Some(3.0), Some("50m"), Some("seg 14h·5d6h"), Some("15h13m")));
        assert_eq!((f.repo.as_deref(), f.branch.as_deref(), f.dirty), (Some("hangar"), Some("main"), Some(true)));
    }

    #[test]
    fn empty_line_without_git_is_unknown() {
        assert!(parse(Some(""), None).is_none());
        assert!(parse(None, None).is_none());
    }
}
