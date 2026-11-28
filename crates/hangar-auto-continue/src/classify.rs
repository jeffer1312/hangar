//! Por que o turno parou. A lista é do que É passageiro (rede, stream, 5xx); tudo o que ela não conhece
//! cai em `Other` e nunca continua sozinho.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Normal,
    Network,
    StreamLost,
    #[serde(rename = "server_5xx")]
    Server5xx,
    Other,
}

impl StopKind {
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Network | Self::StreamLost | Self::Server5xx)
    }
}

/// Recusas da conta e do pedido: os `AUTH_ERRORS` do `limit_switch`, o limite e o contexto cheio. Vêm
/// antes de tudo para que uma recusa com texto de rede nunca seja lida como queda.
const REFUSALS: [&str; 7] = [
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "verification_required",
    "rate_limit",
    "usage_limit_reached",
    "prompt_too_long",
];

const NETWORK_CODES: [&str; 5] = ["EAI_AGAIN", "ECONNREFUSED", "ECONNRESET", "ENOTFOUND", "ETIMEDOUT"];

const SERVER_STATUS: [u64; 5] = [500, 502, 503, 504, 529];

/// A linha do transcript do Claude em que o turno parou.
pub fn classify_claude_line(line: &Value) -> StopKind {
    if line["type"] != "assistant" {
        return StopKind::Other;
    }
    if line["isApiErrorMessage"] != true {
        // A resposta sintética (o "No response requested." da interrupção) não é do modelo.
        return if line["message"]["model"] == "<synthetic>" { StopKind::Other } else { StopKind::Normal };
    }
    let error = line["error"].as_str();
    let api_error = line["apiError"].as_str();
    let status = line["apiErrorStatus"].as_u64();
    if [error, api_error].into_iter().flatten().any(|code| REFUSALS.contains(&code)) || matches!(status, Some(401 | 403 | 429)) {
        return StopKind::Other;
    }
    let text = text_of(line);
    if api_error == Some("connection_failed") || NETWORK_CODES.iter().any(|code| text.contains(code)) {
        return StopKind::Network;
    }
    // O CLI antigo grava o stream cortado sem `apiError`; com ele a linha já saiu como rede.
    if text.contains("Connection lost mid-response") {
        return StopKind::StreamLost;
    }
    let server_error = error == Some("server_error") && status.is_none();
    if server_error || api_error == Some("overloaded") || status.is_some_and(|status| SERVER_STATUS.contains(&status)) {
        return StopKind::Server5xx;
    }
    StopKind::Other
}

fn text_of(line: &Value) -> String {
    let content = &line["message"]["content"];
    match content.as_str() {
        Some(text) => text.to_owned(),
        None => content.as_array().into_iter().flatten().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n"),
    }
}

/// O fim de um turno do Codex: o `status` do turno e o `codexErrorInfo` do erro (texto, ou objeto de chave única).
pub fn classify_codex_turn(status: &str, info: Option<&Value>) -> StopKind {
    match status {
        "completed" => StopKind::Normal,
        "interrupted" => StopKind::Other,
        _ => info.and_then(info_name).map_or(StopKind::Other, codex_kind),
    }
}

fn info_name(info: &Value) -> Option<&str> {
    info.as_str().or_else(|| info.as_object().and_then(|fields| fields.keys().next()).map(String::as_str))
}

fn codex_kind(name: &str) -> StopKind {
    match name {
        "httpConnectionFailed" | "responseStreamConnectionFailed" => StopKind::Network,
        "responseStreamDisconnected" | "responseTooManyFailedAttempts" => StopKind::StreamLost,
        "internalServerError" | "serverOverloaded" => StopKind::Server5xx,
        _ => StopKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn api_error(fields: Value, text: &str) -> Value {
        let mut line = json!({"type": "assistant", "isApiErrorMessage": true, "entrypoint": "cli",
            "message": {"model": "<synthetic>", "content": [{"type": "text", "text": text}]}});
        for (key, value) in fields.as_object().unwrap() {
            line[key] = value.clone();
        }
        line
    }

    #[test]
    fn a_connection_failed_line_is_network() {
        let reach = "API Error: Can't reach the API server — check your internet or DNS (EAI_AGAIN)";
        for entrypoint in ["cli", "sdk-cli"] {
            let mut line = api_error(json!({"error": "server_error", "apiError": "connection_failed"}), reach);
            line["entrypoint"] = json!(entrypoint);
            assert_eq!(classify_claude_line(&line), StopKind::Network);
        }
        // O CLI antigo, sem `apiError`: o código do sistema no texto diz que foi a rede.
        assert_eq!(classify_claude_line(&api_error(json!({"error": "server_error"}), reach)), StopKind::Network);
        assert_eq!(classify_claude_line(&api_error(json!({"error": "server_error"}), "connect ECONNRESET 1.2.3.4:443")), StopKind::Network);
        let lost = "API Error: Connection lost mid-response. The response above may be incomplete.";
        assert_eq!(classify_claude_line(&api_error(json!({"error": "server_error"}), lost)), StopKind::StreamLost);
        let overloaded = api_error(json!({"error": "server_error", "apiErrorStatus": 529}), "API Error: 529 Overloaded.");
        assert_eq!(classify_claude_line(&overloaded), StopKind::Server5xx);
        assert_eq!(classify_claude_line(&api_error(json!({"error": "server_error"}), "API Error")), StopKind::Server5xx);
        assert_eq!(classify_claude_line(&api_error(json!({"apiError": "overloaded"}), "Overloaded")), StopKind::Server5xx);
        let normal = json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": "Pronto."}]}});
        assert_eq!(classify_claude_line(&normal), StopKind::Normal);
    }

    #[test]
    fn a_login_refusal_or_403_never_is_transient() {
        let refusals = [
            api_error(json!({"error": "authentication_failed", "apiErrorStatus": 403}), "Please run /login · API Error: 403 Attention Required! | Cloudflare"),
            api_error(json!({"error": "authentication_failed"}), "Not logged in · Please run /login"),
            api_error(json!({"error": "account_on_hold"}), "Your account is on hold"),
            api_error(json!({"error": "oauth_org_not_allowed"}), "ETIMEDOUT"),
            api_error(json!({"error": "verification_required"}), "verify"),
            api_error(json!({"error": "server_error", "apiErrorStatus": 401}), "API Error: 401"),
            api_error(json!({"error": "server_error", "apiErrorStatus": 403}), "API Error: 403 ECONNRESET"),
            api_error(json!({"error": "rate_limit", "apiError": "usage_limit_reached", "apiErrorStatus": 429}), "You've hit your 5-hour limit"),
            api_error(json!({"error": "rate_limit"}), "Rate limited"),
            api_error(json!({"error": "invalid_request", "apiError": "prompt_too_long"}), "Prompt is too long"),
            api_error(json!({"error": "unknown"}), "hook bloqueou"),
            api_error(json!({"error": "server_error", "apiErrorStatus": 400}), "API Error: 400"),
        ];
        for line in refusals {
            assert_eq!(classify_claude_line(&line), StopKind::Other, "{line}");
        }
    }

    #[test]
    fn an_interrupt_reply_never_continues() {
        let reply = json!({"type": "assistant", "isApiErrorMessage": false,
            "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "No response requested."}]}});
        assert!(!classify_claude_line(&reply).is_transient());
        assert_eq!(classify_claude_line(&reply), StopKind::Other);
        let marker = json!({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user]"}]}});
        assert!(!classify_claude_line(&marker).is_transient());
        assert_eq!(classify_codex_turn("interrupted", None), StopKind::Other);
        assert_eq!(classify_codex_turn("interrupted", Some(&json!("serverOverloaded"))), StopKind::Other);
    }

    /// O schema guardado pelo `hangar-codex`, lido do disco: o workflow do schema troca o nome do arquivo a
    /// cada versão.
    fn codex_infos() -> Vec<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hangar-codex/schema");
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json")).collect();
        files.sort();
        let schema: Value = serde_json::from_str(&std::fs::read_to_string(files.last().unwrap()).unwrap()).unwrap();
        let mut names = Vec::new();
        for branch in schema["definitions"]["CodexErrorInfo"]["anyOf"].as_array().unwrap() {
            names.extend(branch["enum"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
            names.extend(branch["required"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
        }
        names
    }

    #[test]
    fn every_codex_info_in_the_schema_has_a_kind() {
        let names = codex_infos();
        assert!(names.len() > 10, "{names:?}");
        let expected = [
            ("httpConnectionFailed", StopKind::Network),
            ("responseStreamConnectionFailed", StopKind::Network),
            ("responseStreamDisconnected", StopKind::StreamLost),
            ("responseTooManyFailedAttempts", StopKind::StreamLost),
            ("internalServerError", StopKind::Server5xx),
            ("serverOverloaded", StopKind::Server5xx),
        ];
        for (name, _) in expected {
            assert!(names.iter().any(|known| known == name), "{name} saiu do schema do Codex");
        }
        for name in &names {
            let want = expected.iter().find(|(known, _)| known == name).map_or(StopKind::Other, |(_, kind)| *kind);
            assert_eq!(classify_codex_turn("failed", Some(&json!(name))), want, "{name} como texto");
            assert_eq!(classify_codex_turn("failed", Some(&json!({name.as_str(): {"httpStatusCode": null}}))), want, "{name} como objeto");
        }
        for name in ["usageLimitExceeded", "unauthorized", "contextWindowExceeded", "badRequest", "sandboxError", "other"] {
            assert!(names.iter().any(|known| known == name), "{name}");
        }
        assert_eq!(classify_codex_turn("failed", None), StopKind::Other);
        assert_eq!(classify_codex_turn("completed", None), StopKind::Normal);
    }
}
