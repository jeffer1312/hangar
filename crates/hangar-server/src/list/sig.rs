//! Assinatura da lista para não reemitir sem mudança visível (`sse.py`, `_list_sig`).
use std::sync::LazyLock;

use hangar_api::session::{ContextUse, SessionRow};
use regex::Regex;
use serde_json::{json, Value};

use crate::transcript::pyjson;

static MODEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"🤖\s*([^(│]+)").unwrap());
static EFFORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"🤖[^(│]*\(([^)│]*)\)").unwrap());
static H5: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"⚡[^│]*?(\d+)\s*%").unwrap());
static D7: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"📅[^│]*?(\d+)\s*%").unwrap());
static CTX_SEG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"💬([^│]*)").unwrap());
static PAIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?").unwrap());
static LABELED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bctx\s*([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?").unwrap());

fn num(x: &str, unit: Option<&str>) -> f64 {
    let mult = match unit.map(str::to_ascii_lowercase).as_deref() {
        Some("k") => 1e3,
        Some("m") => 1e6,
        _ => 1.0,
    };
    // ponytail: dígito Unicode fora do ASCII casa no `\d` e o float() do Python o aceita; aqui vira 0.
    x.replace(',', "").parse::<f64>().map_or(0.0, |v| v * mult)
}

/// Redução estável da statusline: modelo, contexto em baldes de 5%, ⚡5h%, 📅7d% e esforço.
/// Relógio e custo ficam de fora: mudam a cada captura.
pub fn status_sig(s: Option<&str>) -> Value {
    let Some(s) = s.filter(|s| !s.is_empty()) else { return Value::Null };
    let mut ctx = Value::Null;
    if let Some(seg) = CTX_SEG.captures(s) {
        let seg = seg.get(1).unwrap().as_str();
        // O par rotulado "ctx x/y" (Pi, Kimi) vence; senão, com dois pares ou mais (Claude), o último
        // é uso/janela.
        let pairs: Vec<_> = PAIR.captures_iter(seg).collect();
        let many = pairs.len() >= 2;
        let target = LABELED.captures(seg).or_else(|| pairs.into_iter().last().filter(|_| many));
        if let Some(c) = target {
            let g = |i| c.get(i).map(|m| m.as_str());
            let total = num(g(3).unwrap(), g(4));
            if total > 0.0 {
                // round() do Python: metade vai para o par.
                ctx = json!((num(g(1).unwrap(), g(2)) / total * 20.0).round_ties_even() as i64);
            }
        }
    }
    let group = |re: &Regex| re.captures(s).map(|c| c[1].to_owned());
    json!([
        group(&MODEL).map(|m| m.trim().to_owned()),
        ctx,
        group(&H5),
        group(&D7),
        group(&EFFORT).map(|m| m.trim().to_owned()),
    ])
}

/// Contexto em baldes de 5%, como o da statusline.
pub fn context_sig(ctx: Option<&ContextUse>) -> Option<u64> {
    ctx.filter(|c| c.window > 0).map(|c| c.used * 20 / c.window)
}

/// `json.dumps` da tupla por linha do Python, byte a byte. Ignora `last_activity`: muda a cada
/// escrita do transcript e reemitiria a lista sem nada visível mudar.
pub fn list_sig<'a>(rows: impl IntoIterator<Item = &'a SessionRow>) -> String {
    let items: Vec<Value> = rows
        .into_iter()
        .map(|i| {
            let label = if i.provider == "codex" && !i.tracked {
                json!(i.label)
            } else {
                json!(i.label.as_deref().is_some_and(|l| !l.is_empty()))
            };
            json!([
                i.name, i.cwd, i.branch, i.git_cwd, i.worktree_gone, i.git_dirty,
                i.git_ahead, i.git_behind, i.git_added, i.git_removed,
                i.state, i.tracked, i.headless, i.jsonl, i.question, i.stalled, i.limited,
                i.lifecycle_id, i.transfer_id, i.transfer_phase,
                i.last_reply, i.last_reply_at, i.pending_questions,
                i.limit_reset, i.then_target, status_sig(i.status_line.as_deref()),
                context_sig(i.context.as_ref()), i.model, label, i.startup_steps,
                i.loop_status, i.loop_iter, i.engine, i.conta, i.codex_service_tier,
                i.plan_name, i.plan_done, i.plan_total, i.plan_task, i.plan_task_total, i.plan_complete,
                i.plan_tasks.as_deref().unwrap_or_default(),
                i.plan_hidden, i.problema, i.provider, i.shared, i.owner, i.orq_arbiter,
                i.pair_peers, i.pair_gid, i.pair_task, i.pair_external,
            ])
        })
        .collect();
    pyjson::dumps_unicode(&Value::Array(items), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valores tirados do `sse._status_sig` do Python.
    #[test]
    fn status_sig_matches_python() {
        let cases = [
            ("", json!(null)),
            ("🤖 Opus 4.5 │ 💬 1k/2k 50k/200k", json!(["Opus 4.5", 5, null, null, null])),
            ("🤖 Sonnet 4.5 (high✦) │ 💬 3k/4k 40k/200k │ ⚡5h 12% │ 📅7d 3 %", json!(["Sonnet 4.5", 4, "12", "3", "high✦"])),
            ("π kimi │ 💬 ctx 10k/1M", json!([null, 0, null, null, null])),
            ("🤖 X │ 💬 25k/1k 5k/200k", json!(["X", 0, null, null, null])),
            ("🤖 X │ 💬 1.2.3/4k 1,5k/1,000", json!(["X", 300, null, null, null])),
            ("💬 7/8", json!([null, null, null, null, null])),
            ("🤖  (low) │ 💬 1k/2k 0/0", json!(["", null, null, null, "low"])),
            ("🤖 X │ 💬 1k/1k 12.5k/100k", json!(["X", 2, null, null, null])),
        ];
        for (line, want) in cases {
            assert_eq!(status_sig(Some(line)), want, "{line}");
        }
        assert_eq!(status_sig(None), Value::Null);
    }

    /// Cauda da assinatura do `sse._list_sig` para uma linha com grupo, byte a byte.
    #[test]
    fn group_fields_match_python() {
        let row: SessionRow = serde_json::from_value(json!({"name": "s", "pair_peers": ["a", "é"], "pair_gid": "g",
            "pair_task": "T", "pair_external": {"alias": "x", "owner": "o", "session": "s"}})).unwrap();
        assert!(list_sig([&row]).ends_with(r#"["a", "é"], "g", "T", {"alias": "x", "owner": "o", "session": "s"}]]"#));
    }

    #[test]
    fn context_sig_buckets() {
        assert_eq!(context_sig(None), None);
        assert_eq!(context_sig(Some(&ContextUse { used: 5, window: 0 })), None);
        assert_eq!(context_sig(Some(&ContextUse { used: 149_999, window: 200_000 })), Some(14));
        assert_eq!(context_sig(Some(&ContextUse { used: 10, window: 1_000_000 })), Some(0));
    }
}
