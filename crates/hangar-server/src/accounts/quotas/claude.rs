use super::{reading, window};
use serde_json::Value;
use std::path::Path;

pub fn parse(body: &Value) -> Vec<Value> {
    let mut windows: Vec<Value> = [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(key, label)| window(&body[key], label, "utilization", "resets_at", false))
        .collect();
    if windows.is_empty() {
        return windows;
    }
    if let Some(limits) = body["limits"].as_array() {
        for limit in limits {
            if limit["kind"] != "weekly_scoped" {
                continue;
            }
            if let Some(name) = limit["scope"]["model"]["display_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                && let Some(value) = window(limit, name, "percent", "resets_at", true)
            {
                windows.push(value);
            }
        }
    }
    windows
}

pub async fn read(client: &reqwest::Client, home: &Path, now: f64) -> Value {
    let raw: Value = match std::fs::read(home.join(".credentials.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(raw) => raw,
        None => return reading("sem_credencial", vec![], Some("credencial-ilegivel")),
    };
    let oauth = &raw["claudeAiOauth"];
    let Some(token) = oauth["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
    else {
        return reading("sem_credencial", vec![], Some("sem-token"));
    };
    if oauth["expiresAt"]
        .as_f64()
        .is_some_and(|ms| ms / 1000.0 <= now)
    {
        return reading("expirada", vec![], Some("token-expirado"));
    }
    let response = match client
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return reading("indisponivel", vec![], Some("sem-resposta")),
    };
    let status = response.status().as_u16();
    if matches!(status, 401 | 403) {
        return reading("expirada", vec![], Some("login-necessario"));
    }
    if status != 200 {
        return reading("indisponivel", vec![], Some(&format!("http-{status}")));
    }
    let body: Value = response
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let windows = parse(&body);
    if windows.is_empty() {
        reading("indisponivel", windows, Some("formato-desconhecido"))
    } else {
        reading("lida", windows, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn windows_preserve_model_scope_and_percent() {
        let rows = parse(
            &json!({"five_hour":{"utilization":12.5,"resets_at":"2026-01-01T00:00:00Z"},
            "seven_day":{"utilization":95},"limits":[{"kind":"weekly_scoped", "percent":100,
            "scope":{"model":{"display_name":"Fable"}}}]}),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["pct"], 12.5);
        assert_eq!(rows[0]["reset_ts"], 1767225600.0);
        assert_eq!(rows[2]["rotulo"], "Fable");
        assert_eq!(rows[2]["por_modelo"], true);
        assert!(parse(&json!({"five_hour":{"utilization":true}})).is_empty());
    }
}
