//! Valores do Settings congelados na inicialização; o arquivo de overrides é independente.
use crate::{machine_peers::dotenv::SettingsSnapshot, value_coercion};
use serde_json::{Map, Value, json};

pub(super) fn load(settings: &SettingsSnapshot) -> Result<Map<String, Value>, String> {
    let fields = [
        ("sync", json!(false)), ("dictation_include_recent_messages", json!(false)),
        ("jev_padrao", json!(false)), ("notify_finished", json!(true)),
        ("notify_dead", json!(true)), ("automations", json!(true)),
        ("traduzir_pensamento", json!(true)), ("codex_sync", json!(true)),
        ("codex_extended_context", json!(true)),
        ("claude_statusline_update", json!(true)), ("claude_function_hooks", json!(true)),
        ("headless_default", json!(true)), ("groq_api_key", json!("")),
        ("dictation_claude_model", json!("")), ("dictation_codex_model", json!("")),
        ("term_origins", json!("")), ("shortcuts", json!("")),
        ("upload_retention_days", json!(30)), ("finish_min_seconds", json!(45)),
        ("stall_seconds", json!(300)), ("editor", json!("code")),
        ("dictation_organization_mode", json!("none")),
        ("scan_roots", json!(if cfg!(windows) { "~,~/Documents,~/Desktop" } else { "~/pessoal,~/sistemas" })),
    ];
    let mut output = Map::new();
    for (key, default) in fields {
        let name = format!("CP_{}", key.to_ascii_uppercase());
        let raw = if key == "groq_api_key" {
            settings.aliases(&[&name, "GROQ_API_KEY"])?
        } else { settings.value(&name)? };
        let value = match raw {
            None => default,
            Some(raw) => {
                let input = Value::String(raw);
                if default.is_boolean() {
                    value_coercion::boolean(&input).map(Value::Bool)
                        .ok_or_else(|| format!("configuração {name} inválida: booleano esperado"))?
                } else if default.is_number() {
                    value_coercion::integer(&input)
                        .ok_or_else(|| format!("configuração {name} inválida: inteiro esperado"))?
                } else { input }
            }
        };
        output.insert(key.into(), value);
    }
    Ok(output)
}
