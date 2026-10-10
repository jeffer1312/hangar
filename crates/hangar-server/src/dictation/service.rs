use super::{guards, model::*, prompts};
use serde_json::{Value, json};

pub async fn organize(
    state: &crate::routes::AppState,
    request: OrganizationRequest,
    config: &OrganizationConfig,
) -> OrganizationResult {
    let mode = request
        .mode
        .unwrap_or_else(|| OrganizationMode::saved(&config.dictation_organization_mode));
    let mut original = OrganizationResult::original(request.raw.clone(), mode);
    if mode == OrganizationMode::None {
        return original;
    }
    let raw = original.raw.trim();
    if raw.is_empty() || raw.starts_with('/') || raw.split_whitespace().count() < 5 {
        return original;
    }
    let saved = match config.ditado_estilo.as_str() {
        "limpar" => DictationStyle::Clean,
        "briefing" => DictationStyle::Briefing,
        _ => DictationStyle::Prose,
    };
    let style = request.style.unwrap_or(saved).effective(raw);
    if request
        .include_recent_messages
        .unwrap_or(config.dictation_include_recent_messages)
        && request.session.is_some()
    {
        if !request.harness_allowed {
            return original.failed(
                "dictation_context_unavailable",
                "O contexto da conversa não está autorizado; a transcrição foi preservada.",
            );
        }
        match super::references::read(state,&request).await {
            Ok(references)=>original.recent_messages=Some(references),
            Err(code)=>return original.failed(code,"Não foi possível ler a referência de grafia da conversa; a transcrição foi preservada."),
        }
    }
    let raw = original.raw.trim();
    if mode == OrganizationMode::Harness {
        if !request.harness_allowed {
            return original.failed("dictation_harness_unauthorized","A organização pela conta do harness é exclusiva do dono; foi mantida a transcrição original.");
        }
        return super::harness::organize(state, &request, config, style, original).await;
    }
    let briefing =
        style == DictationStyle::Briefing && !config.llm_briefing_base_url.trim().is_empty();
    let (url, key, model) = if briefing {
        (
            &config.llm_briefing_base_url,
            &config.llm_briefing_api_key,
            &config.llm_briefing_model,
        )
    } else {
        (&config.llm_base_url, &config.llm_api_key, &config.llm_model)
    };
    if key.trim().is_empty() {
        return original.failed("dictation_api_key_missing", "Configure a chave da API de organização em Configurações → Voz; foi mantida a transcrição original.");
    }
    let endpoint = if url.trim().is_empty() {
        "https://api.groq.com/openai/v1"
    } else {
        url.trim()
    };
    let model = if model.trim().is_empty() {
        "openai/gpt-oss-120b"
    } else {
        model.trim()
    };
    let mut body = json!({"model":model,"messages":[{"role":"system","content":prompts::with_references(style,original.recent_messages.as_deref())},{"role":"user","content":raw}],"temperature":0});
    if !config.llm_reasoning_effort.is_empty() {
        body["reasoning_effort"] = json!(config.llm_reasoning_effort);
    }
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return original.failed(
                "dictation_organization_failed",
                "Não foi possível iniciar a organização; foi mantida a transcrição original.",
            );
        }
    };
    let response = client
        .post(format!(
            "{}/chat/completions",
            endpoint.trim_end_matches('/')
        ))
        .bearer_auth(key)
        .header("content-type", "application/json")
        .header("user-agent", "hangar/1.0")
        .body(body.to_string())
        .timeout(style.timeout())
        .send()
        .await;
    let output = match response {
        Ok(response) if response.status().is_success() => match response.bytes().await {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
            _ => None,
        },
        Err(error) if error.is_timeout() => {
            return original.failed(
                "dictation_organization_timeout",
                "A organização excedeu o prazo; foi mantida a transcrição original.",
            );
        }
        _ => None,
    };
    let Some(output) = output else {
        return original.failed(
            "dictation_organization_failed",
            "A API de organização não devolveu texto válido; foi mantida a transcrição original.",
        );
    };
    let output = guards::normalized(&output);
    if let Err(detail) = guards::validate(raw, &output, style) {
        return original.failed("dictation_output_rejected", detail);
    }
    let applied = if output == original.raw {
        "cru"
    } else {
        style.id()
    };
    OrganizationResult {
        text: output,
        estilo_aplicado: applied.into(),
        ..original
    }
}
