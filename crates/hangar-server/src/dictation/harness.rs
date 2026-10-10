//! Uma chamada curta, sem histórico, ferramentas ou customizações da sessão principal.
use super::{
    catalog,
    context::{self, Context},
    guards,
    model::*,
    process::limited_output_guarded,
    prompts,
};
use crate::routes::AppState;
use serde_json::{Value, json};

pub fn detail(code: &str) -> &'static str {
    match code {
        "dictation_claude_oauth_unsupported" => {
            "O organizador mínimo do Claude não aceita login OAuth nesta versão. Use uma conta API ou outro modo; a transcrição foi preservada."
        }
        "dictation_target_missing" => {
            "A conversa de destino não está disponível; a transcrição foi preservada."
        }
        "dictation_target_changed" => {
            "A conversa de destino foi recriada ou mudou durante o ditado; a transcrição foi preservada."
        }
        "dictation_harness_unsupported" => {
            "O harness ou a conta da conversa não oferece organização mínima nesta versão; a transcrição foi preservada."
        }
        "dictation_model_unavailable" => {
            "O modelo de organização escolhido não está disponível nesta conta; a transcrição foi preservada."
        }
        "dictation_organization_timeout" => {
            "A organização excedeu o prazo; a transcrição foi preservada."
        }
        "dictation_cli_missing" => {
            "O executável do harness não está disponível neste servidor; a transcrição foi preservada."
        }
        "dictation_context_unavailable" => {
            "Não foi possível ler a referência de grafia da conversa; a transcrição foi preservada."
        }
        "dictation_account_busy" => {
            "A conta da conversa está sendo alterada; a transcrição foi preservada. Tente organizar novamente após a alteração."
        }
        "dictation_catalog_unavailable" | "dictation_catalog_rate_limited" => {
            "O catálogo da conta não está disponível agora; a transcrição foi preservada."
        }
        "dictation_cli_incompatible" => {
            "Esta versão do CLI não oferece o isolamento exigido para organizar o ditado; a transcrição foi preservada."
        }
        _ => "Não foi possível organizar pelo harness da conversa; a transcrição foi preservada.",
    }
}

pub async fn organize(
    state: &AppState,
    request: &OrganizationRequest,
    config: &OrganizationConfig,
    style: DictationStyle,
    original: OrganizationResult,
) -> OrganizationResult {
    let context = match context::resolve(state, request).await {
        Ok(context) => context,
        Err(code) => return original.failed(code, detail(code)),
    };
    if context.provider == "claude" && !context.compatible {
        return original.failed(
            "dictation_claude_oauth_unsupported",
            detail("dictation_claude_oauth_unsupported"),
        );
    }
    let model = request
        .model
        .as_deref()
        .unwrap_or(if context.provider == "claude" {
            &config.dictation_claude_model
        } else {
            &config.dictation_codex_model
        });
    if model.is_empty() {
        return original.failed(
            "dictation_model_unavailable",
            detail("dictation_model_unavailable"),
        );
    }
    let models = match state
        .dictation_models
        .get_context(&state.accounts, &context, false)
        .await
    {
        Ok((models, _)) => models,
        Err(code) => return original.failed(code, detail(code)),
    };
    if !models.iter().any(|m| m["id"].as_str() == Some(model)) {
        return original.failed(
            "dictation_model_unavailable",
            detail("dictation_model_unavailable"),
        );
    }
    let raw = original.raw.trim();
    let output = match run(
        state,
        &context,
        model,
        style,
        raw,
        original.recent_messages.as_deref(),
    )
    .await
    {
        Ok(output) => guards::normalized(&output),
        Err(code) => return original.failed(code, detail(code)),
    };
    if let Err(detail) = guards::validate(raw, &output, style) {
        return original.failed("dictation_output_rejected", detail);
    }
    let current = match context::resolve(state, request).await {
        Ok(current) => current,
        Err(code) => return original.failed(code, detail(code)),
    };
    if current.generation != context.generation
        || current.home != context.home
        || current.provider != context.provider
    {
        return original.failed(
            "dictation_target_changed",
            detail("dictation_target_changed"),
        );
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

pub async fn run(
    state: &AppState,
    context: &Context,
    model: &str,
    style: DictationStyle,
    raw: &str,
    references: Option<&[ReferenceMessage]>,
) -> Result<String, &'static str> {
    let guard = std::sync::Arc::new(account_guard(state, context)?);
    let (command, directory) = build_command(state, context, model, style, references)?;
    let bytes =
        limited_output_guarded(command, directory, raw, style.timeout(), Some(guard)).await?;
    parse_output(&bytes, context.provider == "claude")
}

pub fn account_guard(
    state: &AppState,
    context: &Context,
) -> Result<crate::accounts::AccountGuard, &'static str> {
    use crate::accounts::{AccountKey, GuardMode, Provider};
    let key = AccountKey::new(
        if context.provider == "claude" {
            Provider::Claude
        } else {
            Provider::Codex
        },
        &context.home,
    )
    .map_err(|_| "dictation_target_missing")?;
    state
        .accounts
        .locks
        .try_acquire(&key, GuardMode::Shared)
        .map_err(|_| "dictation_account_busy")
}

pub fn build_command(
    state: &AppState,
    context: &Context,
    model: &str,
    style: DictationStyle,
    references: Option<&[ReferenceMessage]>,
) -> Result<(tokio::process::Command, tempfile::TempDir), &'static str> {
    let directory = tempfile::tempdir().map_err(|_| "dictation_organization_failed")?;
    let mut command = if context.provider == "claude" {
        state
            .accounts
            .claude_command()
            .ok_or("dictation_cli_missing")?
    } else {
        state
            .accounts
            .codex_command()
            .ok_or("dictation_cli_missing")?
    };
    let mut environment = context.env.clone();
    // Somente os leitores explicitamente pedidos podem usar a autenticação da conta.
    if context.provider == "claude" {
        command
            .args([
                "-p",
                "--bare",
                "--no-session-persistence",
                "--tools",
                "",
                "--disable-slash-commands",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
                "--output-format",
                "json",
                "--model",
                model,
                "--system-prompt",
            ])
            .arg(prompts::with_references(style, references));
        if !context.settings.as_object().is_none_or(|m| m.is_empty()) {
            let file = directory.path().join("auth-settings.json");
            std::fs::write(&file, context.settings.to_string())
                .map_err(|_| "dictation_organization_failed")?;
            command.arg("--settings").arg(file);
        }
        environment.insert("CLAUDE_CODE_DISABLE_BUNDLED_SKILLS".into(), "1".into());
        environment.insert("ENABLE_TOOL_SEARCH".into(), "false".into());
        environment.insert("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".into(), "1".into());
    } else {
        command.args([
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--json",
            "--model",
            model,
        ]);
        catalog::codex_overrides(&mut command);
        let file = directory.path().join("instructions.txt");
        std::fs::write(&file, prompts::with_references(style, references))
            .map_err(|_| "dictation_organization_failed")?;
        command.arg("-c").arg(format!(
            "model_instructions_file={}",
            json!(file.to_string_lossy())
        ));
        command.arg("-c").arg(format!(
            "log_dir={}",
            json!(directory.path().join("logs").to_string_lossy())
        ));
        command.arg("-c").arg(format!(
            "sqlite_home={}",
            json!(directory.path().join("state").to_string_lossy())
        ));
        command.args(["-c", r#"history.persistence="none""#]);
        let stored = catalog::codex_configuration(&context.home)?;
        catalog::codex_auth_override(&mut command, &state.accounts, context, &stored)?;
        {
            if let Some(provider) = stored
                .get("model_provider")
                .and_then(toml::Value::as_str)
                .filter(|p| *p != "openai")
            {
                let settings = stored
                    .get("model_providers")
                    .and_then(|providers| providers.get(provider))
                    .ok_or("dictation_harness_unsupported")?;
                command
                    .arg("-c")
                    .arg(format!("model_provider={}", json!(provider)));
                let settings = settings.as_table().ok_or("dictation_harness_unsupported")?;
                for (key, value) in settings {
                    // Credenciais literais permanecem em memória, nunca no argv.
                    if key == "experimental_bearer_token" {
                        let token = value.as_str().ok_or("dictation_harness_unsupported")?;
                        environment.insert("HANGAR_DICTATION_PROVIDER_TOKEN".into(), token.into());
                        command.arg("-c").arg(format!(
                            "model_providers.{provider}.env_key=\"HANGAR_DICTATION_PROVIDER_TOKEN\""
                        ));
                    } else if key == "env_key" {
                        let name = value.as_str().ok_or("dictation_harness_unsupported")?;
                        let secret = state
                            .accounts
                            .env
                            .base
                            .get(name)
                            .ok_or("dictation_organization_failed")?;
                        environment.insert(name.into(), secret.clone());
                        provider_config(
                            &mut command,
                            &format!("model_providers.{provider}.{key}"),
                            value,
                        )?;
                    } else if matches!(
                        key.as_str(),
                        "name" | "base_url" | "wire_api" | "requires_openai_auth" | "auth"
                    ) {
                        provider_config(
                            &mut command,
                            &format!("model_providers.{provider}.{key}"),
                            value,
                        )?;
                    }
                }
            }
            if stored.get("chatgpt_base_url").is_some() {
                return Err("dictation_harness_unsupported");
            }
        }
        command.arg("-");
    }
    command.env_clear().envs(environment);
    Ok((command, directory))
}

fn parse_output(bytes: &[u8], claude: bool) -> Result<String, &'static str> {
    if claude {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| "dictation_organization_failed")?;
        if value["is_error"].as_bool() == Some(true) {
            return Err("dictation_organization_failed");
        }
        return value["result"]
            .as_str()
            .map(str::to_owned)
            .ok_or("dictation_organization_failed");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "dictation_organization_failed")?;
    let mut output = None;
    let mut completed = false;
    for line in text.lines() {
        let value: Value =
            serde_json::from_str(line).map_err(|_| "dictation_organization_failed")?;
        if value["type"] == "turn.failed" {
            return Err("dictation_organization_failed");
        }
        if value["type"] == "turn.completed" {
            completed = true;
        }
        if value["type"] == "item.completed" {
            let item = &value["item"];
            match item["type"].as_str() {
                Some("agent_message") => output = item["text"].as_str().map(str::to_owned),
                Some("error") => {
                    if item["message"]
                        .as_str()
                        .is_some_and(|m| m.contains("unrecognized configuration"))
                    {
                        return Err("dictation_cli_incompatible");
                    }
                }
                Some("command_execution" | "mcp_tool_call" | "web_search" | "image_generation") => {
                    return Err("dictation_cli_incompatible");
                }
                _ => {}
            }
        }
    }
    output
        .filter(|_| completed)
        .ok_or("dictation_organization_failed")
}

fn provider_config(
    command: &mut tokio::process::Command,
    key: &str,
    value: &toml::Value,
) -> Result<(), &'static str> {
    if let Some(table) = value.as_table() {
        for (name, value) in table {
            if !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err("dictation_harness_unsupported");
            }
            provider_config(command, &format!("{key}.{name}"), value)?;
        }
    } else {
        let value = serde_json::to_value(value).map_err(|_| "dictation_harness_unsupported")?;
        command.arg("-c").arg(format!("{key}={value}"));
    }
    Ok(())
}
