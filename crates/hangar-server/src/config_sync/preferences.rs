//! Campos portáveis de runtime-config; escolhas da máquina não entram na transferência.
pub const BOOLEAN: &[&str] = &[
    "sync",
    "notify_finished",
    "notify_dead",
    "automations",
    "traduzir_pensamento",
    "codex_sync",
    "codex_memory_import",
    "claude_statusline_update",
    "claude_function_hooks",
    "dictation_include_recent_messages",
    "jev_padrao",
    "headless_default",
];
pub const INTEGER: &[&str] = &[
    "upload_retention_days",
    "finish_min_seconds",
    "stall_seconds",
    "tts_max_chars",
    "tts_stability",
    "tts_similarity_boost",
    "tts_style",
    "tts_speed",
    "dictation_max_minutes",
];
pub const TEXT: &[&str] = &[
    "groq_api_key",
    "transcription_base_url",
    "transcription_model",
    "editor",
    "elevenlabs_api_key",
    "elevenlabs_voice_id",
    "tts_local_cmd",
    "llm_base_url",
    "llm_api_key",
    "llm_model",
    "llm_reasoning_effort",
    "llm_briefing_base_url",
    "llm_briefing_api_key",
    "llm_briefing_model",
    "ditado_vocabulario",
    "ditado_estilo",
    "dictation_organization_mode",
    "dictation_claude_model",
    "dictation_codex_model",
    "term_origins",
    "jev_api_key",
    "last_session_provider",
    "jev_endpoint",
    "jev_model",
    "jev_texto_base_url",
    "jev_texto_api_key",
    "jev_texto_modelo",
    "jev_texto_cmd",
    "jev_aux_base_url",
    "jev_aux_api_key",
    "jev_aux_modelo",
    "jev_aux_esforco",
    "scan_roots",
    "shortcuts",
];
pub fn editable(key: &str) -> bool {
    BOOLEAN.contains(&key)
        || INTEGER.contains(&key)
        || TEXT.contains(&key)
        || key == "transcription_providers"
}

use serde_json::{Value, json};

pub fn validate_shortcuts(text: &str) -> Result<(), String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "shortcuts: JSON invalido".to_owned())?;
    let items = value
        .as_array()
        .ok_or("shortcuts: esperado uma lista de atalhos")?;
    for (index, item) in items.iter().enumerate() {
        validate_shortcut(item, &format!("shortcuts: item {}", index + 1))?;
    }
    Ok(())
}
pub fn validate_shortcut(item: &Value, place: &str) -> Result<(), String> {
    if !item.is_object() {
        return Err(format!("{place} nao e um objeto"));
    }
    let nonempty = |key: &str| item[key].as_str().is_some_and(|v| !v.trim().is_empty());
    if !nonempty("id") {
        return Err(format!("{place} sem id"));
    }
    let kind = item["type"].as_str();
    match kind {
        Some("internal") => {
            if ![
                "terminal",
                "modo",
                "navegador",
                "anexos",
                "rodar",
                "externo",
            ]
            .contains(&item["action"].as_str().unwrap_or(""))
            {
                return Err(format!(
                    "{place} tem action desconhecida (use uma de: anexos, externo, modo, navegador, rodar, terminal)"
                ));
            }
        }
        Some("send_text") => {
            if !nonempty("text") {
                return Err(format!("{place} (send_text) sem texto a enviar"));
            }
        }
        Some("shell") => {
            if !nonempty("command") {
                return Err(format!("{place} (shell) sem comando"));
            }
        }
        _ => {
            let kind = kind.map(str::to_owned).unwrap_or_else(|| {
                if item["type"].is_null() {
                    "None".into()
                } else {
                    item["type"].to_string()
                }
            });
            return Err(format!("{place} tem type desconhecido '{kind}'"));
        }
    }
    if !item["runs_in"].is_null()
        && (kind != Some("shell")
            || !["session", "hangar"].contains(&item["runs_in"].as_str().unwrap_or("")))
    {
        return Err(format!(
            "{place} tem runs_in invalido (so em atalho shell: 'session' ou 'hangar')"
        ));
    }
    for flag in ["hangar_home", "answer_in_app"] {
        if !item[flag].is_null() && (kind != Some("shell") || !item[flag].is_boolean()) {
            return Err(format!(
                "{place} tem {flag} invalido (so em atalho shell, verdadeiro ou falso)"
            ));
        }
    }
    if !item["verify"].is_null() && (kind != Some("shell") || !nonempty("verify")) {
        return Err(format!(
            "{place} tem verify invalido (so em atalho shell, um comando)"
        ));
    }
    if matches!(kind, Some("send_text" | "shell")) && !nonempty("label") {
        return Err(format!("{place} sem rotulo"));
    }
    Ok(())
}

pub fn coerce(key: &str, value: &Value) -> Result<Value, String> {
    if key == "transcription_providers" {
        return providers(value);
    }
    if BOOLEAN.contains(&key) {
        return value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| format!("{key}: esperado true/false"));
    }
    if INTEGER.contains(&key) {
        let number =
            if value.is_boolean() || value.is_null() || value.is_array() || value.is_object() {
                None
            } else if let Some(s) = value.as_str() {
                static INTEGER_TEXT: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| {
                        regex::Regex::new(r"\A[+-]?[0-9](?:_?[0-9])*\z").unwrap()
                    });
                let text = s.trim();
                if INTEGER_TEXT.is_match(text) {
                    let text = text.replace('_', "");
                    let negative = text.starts_with('-');
                    let digits = text.trim_start_matches(['+', '-']).trim_start_matches('0');
                    let normalized = if digits.is_empty() {
                        "0".to_owned()
                    } else {
                        format!("{}{digits}", if negative { "-" } else { "" })
                    };
                    serde_json::from_str::<Value>(&normalized).ok()
                } else {
                    None
                }
            } else if value
                .as_number()
                .is_some_and(|n| !n.to_string().contains(['.', 'e', 'E']))
            {
                Some(value.clone())
            } else {
                value
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .and_then(|n| serde_json::from_str(&format!("{:.0}", n.trunc())).ok())
            };
        let number = number.ok_or_else(|| format!("{key}: esperado número"))?;
        if number.to_string().starts_with('-') && number.as_f64() != Some(0.0) {
            return Err(format!("{key}: não pode ser negativo"));
        }
        if key == "dictation_max_minutes" && number.as_u64().is_none_or(|n| n > 30) {
            return Err("dictation_max_minutes: no máximo 30".into());
        }
        return Ok(number);
    }
    let mut text = value
        .as_str()
        .ok_or_else(|| format!("{key}: esperado texto"))?
        .trim()
        .to_owned();
    if key == "dictation_organization_mode"
        && !["", "none", "harness", "external_api"].contains(&text.as_str())
    {
        return Err("dictation_organization_mode: use none, harness ou external_api".into());
    }
    if key == "editor"
        && (text.contains(['/', '\\']) || text.starts_with('-') || text.contains(".."))
    {
        return Err("editor: use o nome do binario (ex: code), sem caminho".into());
    }
    if key == "ditado_estilo"
        && !text.is_empty()
        && !["limpar", "prosa", "briefing"].contains(&text.as_str())
    {
        return Err(format!(
            "ditado_estilo: '{text}' não existe. Use um de: limpar, prosa, briefing."
        ));
    }
    if key == "shortcuts" && !text.is_empty() {
        validate_shortcuts(&text)?;
    }
    if key == "jev_aux_base_url" {
        if let Ok(mut url) = reqwest::Url::parse(&text) {
            let path = url.path().trim_end_matches('/');
            let path = path
                .strip_suffix("/chat/completions")
                .unwrap_or(path)
                .to_owned();
            url.set_path(&path);
            text = url.to_string();
            if path.is_empty() && url.query().is_none() && url.fragment().is_none() {
                text = text.trim_end_matches('/').to_owned();
            }
        }
    }
    if key == "jev_aux_esforco" && !["", "low", "medium", "high"].contains(&text.as_str()) {
        return Err("jev_aux_esforco: use vazio, low, medium ou high".into());
    }
    if [
        "transcription_base_url",
        "llm_base_url",
        "llm_briefing_base_url",
        "jev_endpoint",
        "jev_aux_base_url",
    ]
    .contains(&key)
        && !text.is_empty()
        && !text.starts_with("http://")
        && !text.starts_with("https://")
    {
        return Err(format!("{key}: use vazio ou uma URL http(s)://"));
    }
    Ok(Value::String(text))
}

fn providers(value: &Value) -> Result<Value, String> {
    let items = value
        .as_array()
        .ok_or("transcription_providers: esperado uma lista")?;
    if items.len() > 10 {
        return Err("transcription_providers: no maximo 10 servicos".into());
    }
    let mut output = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let place = format!("transcription_providers: item {}", index + 1);
        if !item.is_object() {
            return Err(format!("{place} nao e um objeto"));
        }
        let mut fields = json!({});
        for name in ["id", "kind", "name", "base_url", "api_key", "model"] {
            let value = if item[name].is_null() {
                ""
            } else {
                item[name]
                    .as_str()
                    .ok_or_else(|| format!("{place}: {name} deve ser texto"))?
            };
            fields[name] = json!(value.trim());
        }
        let id = fields["id"].as_str().unwrap();
        if id.is_empty() || id.chars().count() > 64 {
            return Err(format!("{place} sem id"));
        }
        if !ids.insert(id.to_owned()) {
            return Err(format!("{place}: id repetido"));
        }
        let kind = fields["kind"].as_str().unwrap().to_owned();
        if !["openai", "elevenlabs", "whisper_cpp"].contains(&kind.as_str()) {
            return Err(format!(
                "{place}: tipo '{kind}' nao existe. Use um de: openai, elevenlabs, whisper_cpp."
            ));
        }
        if kind == "elevenlabs" && fields["api_key"] == "" {
            return Err(format!("{place} sem chave"));
        }
        if fields["api_key"].as_str().unwrap().contains('•') {
            return Err(format!(
                "{place}: a chave esta mascarada; digite a chave de novo"
            ));
        }
        if kind == "elevenlabs" {
            fields["base_url"] = json!("");
        } else if kind == "whisper_cpp" {
            for name in [
                "executable_path",
                "model_path",
                "language",
                "converter_path",
            ] {
                let v = &item[name];
                let v = if v.is_null()
                    || v == false
                    || v == 0
                    || v.as_array().is_some_and(Vec::is_empty)
                    || v.as_object().is_some_and(serde_json::Map::is_empty)
                {
                    ""
                } else {
                    v.as_str()
                        .ok_or_else(|| format!("{place}: {name} deve ser texto"))?
                };
                fields[name] = json!(v.trim());
            }
            for name in ["executable_path", "model_path"] {
                if fields[name] == "" {
                    return Err(format!("{place}: informe {name}"));
                }
            }
            if fields["language"] == "" {
                fields["language"] = json!("pt");
            }
            for name in ["api_key", "base_url", "model"] {
                fields[name] = json!("");
            }
        } else {
            let url = fields["base_url"].as_str().unwrap();
            if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(format!("{place}: use endpoint vazio ou uma URL http(s)://"));
            }
        }
        output.push(fields);
    }
    Ok(Value::Array(output))
}
