//! Uso medido que o Codex e a OpenAI devolvem: contexto do organizador, limites da conta e números do canal de eventos.
use serde_json::Value;

/// Janela de limite: porcentagem usada e quando reseta (epoch em segundos).
pub type RateWindow = Option<(f64, Option<i64>)>;

/// `thread/tokenUsage/updated`: o contexto usado é o input do último turno (o total acumulado só cresce).
pub fn context_usage(params: &Value) -> Option<(u64, Option<u64>)> {
    let usage = &params["tokenUsage"];
    let used = usage["last"]["inputTokens"].as_u64().filter(|used| *used > 0)?;
    Some((used, usage["modelContextWindow"].as_u64().filter(|window| *window > 0)))
}

fn rate_window(window: &Value) -> Option<(i64, f64, Option<i64>)> {
    Some((window["windowDurationMins"].as_i64()?, window["usedPercent"].as_f64()?, window["resetsAt"].as_i64()))
}

/// Instantâneo da conta (o do `codex` ou sem id); janelas pela duração, não pela posição. Outros ids não contam.
pub fn account_limits(snapshot: &Value) -> Option<(RateWindow, RateWindow)> {
    if !snapshot.is_object() || !(snapshot["limitId"].is_null() || snapshot["limitId"] == "codex") { return None; }
    let (mut five_hour, mut seven_day) = (None, None);
    for key in ["primary", "secondary"] {
        let Some((mins, pct, reset)) = rate_window(&snapshot[key]) else { continue };
        match mins { 270..=330 => five_hour = Some((pct, reset)), 10020..=10140 => seven_day = Some((pct, reset)), _ => {} }
    }
    Some((five_hour, seven_day))
}

/// Resposta de `account/rateLimits/read`: o instantâneo principal, ou o do `codex` no mapa por id.
pub fn read_limits(result: &Value) -> Option<(RateWindow, RateWindow)> {
    account_limits(&result["rateLimits"]).or_else(|| account_limits(&result["rateLimitsByLimitId"]["codex"]))
}

/// Só números, com o caminho da chave (`response.usage.input_tokens`); nenhum texto sai daqui.
pub fn numeric_leaves(value: &Value) -> Vec<(String, String)> {
    fn walk(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
        if out.len() >= 80 { return; }
        match value {
            Value::Number(n) => out.push((path.to_owned(), n.to_string())),
            Value::Object(map) => for (key, v) in map { walk(v, &if path.is_empty() { key.clone() } else { format!("{path}.{key}") }, out); },
            Value::Array(items) => for (i, v) in items.iter().enumerate() { walk(v, &format!("{path}[{i}]"), out); },
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn context_is_last_input_not_the_running_total() {
        let params = json!({"threadId": "t", "tokenUsage": {"last": {"inputTokens": 19000, "outputTokens": 40},
            "total": {"inputTokens": 97000}, "modelContextWindow": 258400}});
        assert_eq!(context_usage(&params), Some((19000, Some(258400))));
        let no_window = json!({"tokenUsage": {"last": {"inputTokens": 5}, "modelContextWindow": null}});
        assert_eq!(context_usage(&no_window), Some((5, None)));
        assert_eq!(context_usage(&json!({"tokenUsage": {"last": {"inputTokens": 0}}})), None);
        assert_eq!(context_usage(&json!({})), None);
    }

    #[test]
    fn windows_map_by_duration() {
        let snapshot = json!({"limitId": null, "primary": {"usedPercent": 12, "windowDurationMins": 300, "resetsAt": 1000},
            "secondary": {"usedPercent": 40, "windowDurationMins": 10080, "resetsAt": 2000}});
        assert_eq!(account_limits(&snapshot), Some((Some((12., Some(1000))), Some((40., Some(2000))))));
        // Só a semanal, como na conta real: a de 5 h não aparece.
        let week_only = json!({"limitId": "codex", "primary": {"usedPercent": 7, "windowDurationMins": 10080, "resetsAt": 5}, "secondary": null});
        assert_eq!(account_limits(&week_only), Some((None, Some((7., Some(5))))));
        let other = json!({"primary": {"usedPercent": 1, "windowDurationMins": 60, "resetsAt": 5}});
        assert_eq!(account_limits(&other), Some((None, None)), "duração desconhecida é ignorada");
    }

    #[test]
    fn other_limit_ids_are_ignored() {
        let snapshot = json!({"limitId": "codex_other", "primary": {"usedPercent": 1, "windowDurationMins": 300}});
        assert_eq!(account_limits(&snapshot), None);
        let read = json!({"rateLimits": snapshot, "rateLimitsByLimitId": {"codex": {"limitId": "codex",
            "primary": {"usedPercent": 9, "windowDurationMins": 300, "resetsAt": null}}}});
        assert_eq!(read_limits(&read), Some((Some((9., None)), None)));
    }

    #[test]
    fn numeric_leaves_keep_paths_and_drop_strings() {
        let event = json!({"type": "session.usage.updated", "response": {"usage": {"input_tokens": 123, "details": [{"cached": 4.5}], "model": "gpt"}}, "ok": true});
        let mut leaves = numeric_leaves(&event);
        leaves.sort();
        assert_eq!(leaves, vec![("response.usage.details[0].cached".to_owned(), "4.5".to_owned()), ("response.usage.input_tokens".to_owned(), "123".to_owned())]);
    }
}
