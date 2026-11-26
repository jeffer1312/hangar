//! Tools MCP das integrações externas (`integrations_list`, `integration_tools`, `integration_call`
//! e ferramentas fixadas `int__<id>__<tool>`), com guarda de sessão compartilhada, filtro por
//! projeto, aprovações com cancelamento, ramo de voz e envelope de conteúdo externo.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::http::HeaderMap;
use hangar_integrations::{
    ApprovalError, ApprovalResolution, ClientError, DiscoveredTool, IntegrationConfig,
    IntegrationStatus, MAX_REASON_CHARS, OpenApprovalRequest, RegistryError, SecretKeyError,
    SecretStoreError, ToolAccess, ToolsCache, classify_tool_access, compute_status,
    config_fingerprint, format_arguments_preview, format_origin, guard::wrap_external_result,
    truncate_preview_chars,
};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::identity::{self, Local, Origin};
use super::shared_guard;
use crate::routes::AppState;

const MAX_MCP_TOOL_NAME_BYTES: usize = 64;
const LIST_TOOLS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Mapeamento em memória `(nome_mcp -> (integration_id, original_tool_name))` para rotear `int__*`.
#[derive(Default)]
pub(super) struct PinnedToolIndex {
    map: Mutex<HashMap<String, (String, String)>>,
}

impl PinnedToolIndex {
    pub fn replace_for_integration(&self, integration_id: &str, pairs: &[(String, String)]) {
        let mut guard = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        guard.retain(|_, (id, _)| id != integration_id);
        for (mcp_name, original_tool) in pairs {
            guard.insert(
                mcp_name.clone(),
                (integration_id.to_owned(), original_tool.clone()),
            );
        }
    }

    pub fn get(&self, mcp_name: &str) -> Option<(String, String)> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mcp_name)
            .cloned()
    }
}

#[derive(Deserialize)]
pub(super) struct IntegrationsListArgs {
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct IntegrationToolsArgs {
    pub integration: String,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct IntegrationCallArgs {
    pub integration: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: Option<Value>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub confirmed: Option<bool>,
    #[serde(default)]
    pub session: Option<String>,
}

fn dict_result(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]);
    result.structured_content = Some(value);
    result
}

fn local_home(st: &AppState) -> PathBuf {
    st.list
        .env()
        .dirs
        .as_ref()
        .map(|d| d.home.clone())
        .or_else(std::env::home_dir)
        .unwrap_or_default()
}

async fn resolve_caller(st: &AppState, headers: &HeaderMap) -> Result<(String, Origin), String> {
    let home = local_home(st);
    let local = Local {
        home: &home,
        mux: &st.list.env().mux,
    };
    identity::resolve(headers, &local, &st.cfg.internal_secret)
        .await
        .map_err(|e| format!("sessao_desconhecida: {e}"))
}

/// Sessões normais agem sempre como elas mesmas; só a voz pode indicar `session` explícita.
fn resolve_target_session<'a>(
    origin: Origin,
    caller_name: &'a str,
    explicit_session: Option<&'a str>,
) -> Option<&'a str> {
    if origin == Origin::Voice {
        explicit_session.map(str::trim).filter(|s| !s.is_empty())
    } else {
        Some(caller_name)
    }
}

/// Normaliza componentes léxicos (`./` e `../`) sem depender do caminho existir no disco.
pub(super) fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = out
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else if !out.has_root() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

pub(super) fn path_matches_project(raw_session_path: &str, project_entry: &str) -> bool {
    let session_trimmed = raw_session_path.trim();
    let project_trimmed = project_entry.trim();
    if session_trimmed.is_empty() || project_trimmed.is_empty() {
        return false;
    }
    if hangar_projects::slug(session_trimmed) == project_trimmed {
        return true;
    }
    let norm_session = normalize_path(Path::new(session_trimmed));
    if norm_session
        .ancestors()
        .filter(|ancestor| ancestor.file_name().is_some())
        .any(|ancestor| hangar_projects::slug(&ancestor.to_string_lossy()) == project_trimmed)
    {
        return true;
    }
    let norm_project = normalize_path(Path::new(project_trimmed));
    // Em Rust `Path::starts_with("")` é true: nunca deixe projeto vazio/`.` casar qualquer pasta.
    if norm_project.as_os_str().is_empty() || norm_session.as_os_str().is_empty() {
        return false;
    }
    norm_session.starts_with(&norm_project)
}

pub(super) fn paths_match_projects(session_paths: &[String], projects: &[String]) -> bool {
    session_paths
        .iter()
        .any(|path| projects.iter().any(|project| path_matches_project(path, project)))
}

async fn resolve_session_paths(st: &AppState, session_name: &str) -> Result<Vec<String>, String> {
    let snapshot = st.list.snapshot().await.map_err(|_| {
        "integration_project_denied: não foi possível consultar os caminhos da sessão".to_owned()
    })?;
    let Some(row) = snapshot.rows.iter().find(|r| r.name == session_name) else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    if let Some(cwd) = row.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        paths.push(cwd.to_owned());
    }
    if let Some(git_cwd) = row.git_cwd.as_deref().map(str::trim).filter(|s| !s.is_empty())
        && !paths.iter().any(|p| p == git_cwd)
    {
        paths.push(git_cwd.to_owned());
    }
    Ok(paths)
}

async fn ensure_project_allowed(
    st: &AppState,
    config: &IntegrationConfig,
    target_session: Option<&str>,
) -> Result<(), String> {
    if config.projects.is_empty() {
        return Ok(());
    }
    let Some(session_name) = target_session.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(format!(
            "integration_project_denied: a integração '{}' é restrita por projeto e exige uma sessão elegível",
            config.id
        ));
    };
    let paths = resolve_session_paths(st, session_name).await?;
    if paths_match_projects(&paths, &config.projects) {
        Ok(())
    } else {
        Err(format!(
            "integration_project_denied: a sessão '{session_name}' não pertence aos projetos permitidos pela integração '{}'",
            config.id
        ))
    }
}

fn load_integration(st: &AppState, id: &str) -> Result<IntegrationConfig, String> {
    match st.integrations.registry.get(id) {
        Ok(Some(cfg)) => Ok(cfg),
        Ok(None) => Err(format!(
            "integration_not_found: integração '{id}' não encontrada"
        )),
        Err(err) => Err(map_registry_error(err)),
    }
}

/// Garante que integrações marcadas como expiradas localmente não sejam chamadas sem reconexão.
async fn check_local_auth_state(st: &AppState, config: &IntegrationConfig) -> Result<(), String> {
    let secrets = st
        .integrations
        .secrets
        .get(&config.id)
        .await
        .map_err(map_secret_store_error)?;
    match compute_status(config, secrets.as_ref()) {
        IntegrationStatus::AuthExpired => Err(format!(
            "integration_auth_expired: a autorização OAuth da integração '{}' expirou; reconecte nas configurações",
            config.id
        )),
        IntegrationStatus::AuthRequired => Err(format!(
            "integration_auth_required: a integração '{}' exige autenticação antes do uso",
            config.id
        )),
        IntegrationStatus::Disabled => Err(format!(
            "integration_disabled: integração '{}' está desabilitada",
            config.id
        )),
        IntegrationStatus::Ready | IntegrationStatus::Error => Ok(()),
    }
}

pub(super) async fn integrations_list(
    st: &AppState,
    api: &Router,
    headers: &HeaderMap,
    args: IntegrationsListArgs,
) -> Result<Value, String> {
    let (caller_name, origin) = resolve_caller(st, headers).await?;
    let target_session = resolve_target_session(origin, &caller_name, args.session.as_deref());
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }

    let configs = st.integrations.registry.list().map_err(map_registry_error)?;
    let enabled: Vec<IntegrationConfig> = configs.into_iter().filter(|c| c.enabled).collect();
    let session_paths = match target_session {
        Some(session_name) if enabled.iter().any(|c| !c.projects.is_empty()) => {
            resolve_session_paths(st, session_name).await.unwrap_or_default()
        }
        _ => Vec::new(),
    };

    let last_errors = st.integrations.last_errors_snapshot();
    let cancel_warnings = st.integrations.cancel_warnings_snapshot();
    let mut out = Vec::new();
    for cfg in enabled {
        if !cfg.projects.is_empty()
            && (target_session.is_none() || !paths_match_projects(&session_paths, &cfg.projects))
        {
            continue;
        }
        let secrets = st
            .integrations
            .secrets
            .get(&cfg.id)
            .await
            .map_err(map_secret_store_error)?;
        let mut status = compute_status(&cfg, secrets.as_ref());
        if status == IntegrationStatus::Ready && last_errors.contains_key(&cfg.id) {
            status = IntegrationStatus::Error;
        }
        let tools_count = st
            .integrations
            .tools_cache
            .get(&cfg.id)
            .map_or(0, |tools| tools.len());
        let mut item = json!({
            "id": cfg.id,
            "name": cfg.name,
            "status": status,
            "policy": cfg.policy,
            "pinned": cfg.pinned,
            "tools_count": tools_count,
        });
        if let Some(warning) = cancel_warnings.get(&cfg.id) {
            item["last_warning"] = json!(warning);
        }
        out.push(item);
    }
    // Revalida o compartilhamento antes de retornar metadados caso um convite tenha aberto durante as leituras.
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }
    Ok(json!({ "integrations": out }))
}

async fn discover_tools_checked(
    st: &AppState,
    api: &Router,
    integration_id: &str,
    target_session: Option<&str>,
) -> Result<Vec<DiscoveredTool>, String> {
    match st.integrations.clients.list_tools(integration_id).await {
        Ok(tools) => {
            st.integrations.clear_error(integration_id);
            Ok(tools)
        }
        Err(err) => {
            if let Some(session_name) = target_session {
                shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS)
                    .await?;
            }
            Err(map_client_error(st, integration_id, err))
        }
    }
}

pub(super) async fn integration_tools(
    st: &AppState,
    api: &Router,
    headers: &HeaderMap,
    args: IntegrationToolsArgs,
) -> Result<Value, String> {
    let (caller_name, origin) = resolve_caller(st, headers).await?;
    let target_session = resolve_target_session(origin, &caller_name, args.session.as_deref());
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }

    let config = load_integration(st, &args.integration)?;
    ensure_project_allowed(st, &config, target_session).await?;
    if !config.enabled {
        return Err(format!(
            "integration_disabled: integração '{}' está desabilitada",
            config.id
        ));
    }

    let discovered = match st.integrations.tools_cache.get(&config.id) {
        Some(tools) => tools,
        None => {
            check_local_auth_state(st, &config).await?;
            discover_tools_checked(st, api, &config.id, target_session).await?
        }
    };
    // Revalida o compartilhamento após qualquer espera assíncrona antes de expor metadados externos.
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }
    let summaries = ToolsCache::into_tool_summaries(&config, discovered);
    Ok(json!({ "tools": summaries }))
}

pub(super) async fn integration_call(
    st: &AppState,
    api: &Router,
    server_id: &str,
    headers: &HeaderMap,
    args: IntegrationCallArgs,
    external_started: Arc<AtomicBool>,
) -> Result<CallToolResult, String> {
    let args_value = match args.arguments {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(Value::Object(map)) => Value::Object(map),
        Some(_) => {
            return Err("integration_invalid_arguments: `arguments` deve ser um objeto JSON".into());
        }
    };
    let (caller_name, origin) = resolve_caller(st, headers).await?;
    let target_session = resolve_target_session(origin, &caller_name, args.session.as_deref());
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }

    let config = load_integration(st, &args.integration)?;
    ensure_project_allowed(st, &config, target_session).await?;
    if !config.enabled {
        return Err(format!(
            "integration_disabled: integração '{}' está desabilitada",
            config.id
        ));
    }

    execute_checked_call(
        st,
        api,
        server_id,
        headers,
        &caller_name,
        origin,
        target_session,
        config,
        &args.tool,
        args_value,
        args.reason,
        args.confirmed,
        false,
        external_started,
    )
    .await
}

pub(super) async fn call_pinned_tool(
    st: &AppState,
    api: &Router,
    server_id: &str,
    index: &PinnedToolIndex,
    headers: &HeaderMap,
    mcp_name: &str,
    raw_args: JsonObject,
    external_started: Arc<AtomicBool>,
) -> Result<CallToolResult, String> {
    let (caller_name, origin) = resolve_caller(st, headers).await?;
    let target_session = if origin == Origin::Voice {
        None
    } else {
        Some(caller_name.as_str())
    };
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }

    let (config, original_tool) =
        resolve_pinned_target(st, api, index, target_session, mcp_name).await?;

    execute_checked_call(
        st,
        api,
        server_id,
        headers,
        &caller_name,
        origin,
        target_session,
        config,
        &original_tool,
        Value::Object(raw_args),
        None,
        None,
        true,
        external_started,
    )
    .await
}

async fn resolve_pinned_target(
    st: &AppState,
    api: &Router,
    index: &PinnedToolIndex,
    target_session: Option<&str>,
    mcp_name: &str,
) -> Result<(IntegrationConfig, String), String> {
    if let Some((integration_id, original_tool)) = index.get(mcp_name) {
        let config = load_integration(st, &integration_id)?;
        if !config.pinned {
            return Err(format!("Unknown tool: {mcp_name}"));
        }
        ensure_project_allowed(st, &config, target_session).await?;
        if !config.enabled {
            return Err(format!(
                "integration_disabled: integração '{}' está desabilitada",
                config.id
            ));
        }
        return Ok((config, original_tool));
    }

    let rest = mcp_name
        .strip_prefix("int__")
        .ok_or_else(|| format!("Unknown tool: {mcp_name}"))?;

    if let Some((id, tool_part)) = rest.split_once("__")
        && let Some(config) = st.integrations.registry.get(id).map_err(map_registry_error)?
    {
        if !config.pinned {
            return Err(format!("Unknown tool: {mcp_name}"));
        }
        ensure_project_allowed(st, &config, target_session).await?;
        if !config.enabled {
            return Err(format!(
                "integration_disabled: integração '{}' está desabilitada",
                config.id
            ));
        }
        let tools = match st.integrations.tools_cache.get(&config.id) {
            Some(tools) => tools,
            None => {
                check_local_auth_state(st, &config).await?;
                discover_tools_checked(st, api, &config.id, target_session).await?
            }
        };
        let pairs = build_pairs_for_integration(&config.id, &tools);
        index.replace_for_integration(&config.id, &pairs);
        if let Some((_, original)) = pairs.into_iter().find(|(mcp, _)| mcp == mcp_name) {
            return Ok((config, original));
        }
        return Err(format!(
            "integration_tool_not_found: ferramenta '{tool_part}' não encontrada na integração '{}'",
            config.id
        ));
    }

    // Caso o `id` (>55 bytes) tenha sido truncado com hash antes do separador `__`, varre o catálogo.
    let configs = st.integrations.registry.list().map_err(map_registry_error)?;
    for config in configs {
        if !config.enabled || !config.pinned {
            continue;
        }
        let safe_id = sanitize_segment(&config.id);
        let prefix_len = safe_id.len().min(48);
        if !rest.starts_with(&safe_id[..prefix_len]) {
            continue;
        }
        if ensure_project_allowed(st, &config, target_session).await.is_err() {
            continue;
        }
        let tools = match st.integrations.tools_cache.get(&config.id) {
            Some(cached) => cached,
            None => {
                if check_local_auth_state(st, &config).await.is_err() {
                    continue;
                }
                let Ok(discovered) = st.integrations.clients.list_tools(&config.id).await else {
                    continue;
                };
                st.integrations.clear_error(&config.id);
                discovered
            }
        };
        let pairs = build_pairs_for_integration(&config.id, &tools);
        index.replace_for_integration(&config.id, &pairs);
        if let Some((_, original)) = pairs.into_iter().find(|(mcp, _)| mcp == mcp_name) {
            return Ok((config, original));
        }
    }

    Err(format!("Unknown tool: {mcp_name}"))
}

struct CancelWarningGuard<'a> {
    st: &'a AppState,
    integration_id: String,
    tool: String,
    started: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for CancelWarningGuard<'_> {
    fn drop(&mut self) {
        if self.armed && self.started.load(Ordering::SeqCst) {
            self.st
                .integrations
                .record_cancel_warning(&self.integration_id, &self.tool);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_checked_call(
    st: &AppState,
    api: &Router,
    server_id: &str,
    headers: &HeaderMap,
    caller_name: &str,
    origin: Origin,
    target_session: Option<&str>,
    mut config: IntegrationConfig,
    tool_name: &str,
    args_value: Value,
    reason: Option<String>,
    confirmed: Option<bool>,
    from_pinned_alias: bool,
    external_started: Arc<AtomicBool>,
) -> Result<CallToolResult, String> {
    let expected_config_fp = config_fingerprint(&config);
    let discovered = match st.integrations.tools_cache.get_tool(&config.id, tool_name) {
        Some(found) => found,
        None => {
            check_local_auth_state(st, &config).await?;
            let tools = discover_tools_checked(st, api, &config.id, target_session).await?;
            tools.into_iter().find(|t| t.name == tool_name).ok_or_else(|| {
                format!(
                    "integration_tool_not_found: ferramenta '{tool_name}' não encontrada na integração '{}'",
                    config.id
                )
            })?
        }
    };

    let access = classify_tool_access(
        config.policy,
        &config.allowed_tools,
        discovered.read_only_hint,
        &discovered.name,
    );

    let mut waited_for_approval = false;
    match access {
        ToolAccess::Blocked => {
            return Err(format!(
                "integration_read_only: a integração '{}' está em modo somente leitura e bloqueia a ferramenta '{}'",
                config.id, discovered.name
            ));
        }
        ToolAccess::Read | ToolAccess::AllowedAlways => {}
        ToolAccess::AskWrite => {
            if origin == Origin::Voice {
                let voice_token = headers
                    .get("x-hangar-voice")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                let args_hash = canonical_args_hash(&args_value);
                if confirmed != Some(true)
                    || !st.integrations.consume_voice_confirmation(
                        voice_token,
                        target_session,
                        &config.id,
                        &discovered.name,
                        &args_hash,
                    )
                {
                    // Chamada `confirmed: true` sem desafio prévio nunca arma nova confirmação reutilizável.
                    if confirmed != Some(true) {
                        st.integrations.record_voice_confirmation(
                            voice_token,
                            target_session,
                            &config.id,
                            &discovered.name,
                            args_hash,
                        );
                    }
                    let preview = format_arguments_preview(&args_value);
                    return Ok(dict_result(json!({
                        "status": "voice_confirmation_required",
                        "integration": config.id,
                        "tool": discovered.name,
                        "arguments_preview": preview,
                        "message": format!(
                            "A ferramenta '{}' da integração '{}' pode alterar dados externos. Resuma em voz a integração, ferramenta e argumentos principais, peça o sim falado do dono e só depois chame integration_call novamente com confirmed: true.",
                            discovered.name, config.id
                        ),
                    })));
                }
                waited_for_approval = true;
            } else {
                check_local_auth_state(st, &config).await?;
                let origin_label = truncate_preview_chars(
                    &format_origin(&server_id_or_local(st, server_id), caller_name)
                        .replace('\0', ""),
                    256,
                );
                let session_key = resolve_approval_session_key(st, headers, caller_name).await;
                let effective_reason = reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(|r| truncate_preview_chars(&r.replace('\0', ""), MAX_REASON_CHARS))
                    .unwrap_or_else(|| {
                        truncate_preview_chars(
                            &format!("Executar {} na integração {}", discovered.name, config.name)
                                .replace('\0', ""),
                            MAX_REASON_CHARS,
                        )
                    });
                let waiter = st
                    .integrations
                    .approvals
                    .open_request_for_config(
                        OpenApprovalRequest {
                            origin: origin_label,
                            session_key,
                            integration_id: config.id.clone(),
                            integration_name: config.name.clone(),
                            tool: discovered.name.clone(),
                            arguments_preview: format_arguments_preview(&args_value),
                            reason: effective_reason,
                        },
                        Some(expected_config_fp),
                    )
                    .map_err(map_approval_error)?;

                match waiter.wait().await {
                    ApprovalResolution::AllowOnce | ApprovalResolution::AllowAlways => {}
                    ApprovalResolution::Denied => {
                        return Err(format!(
                            "integration_denied: o dono negou a execução de '{}' na integração '{}'",
                            discovered.name, config.id
                        ));
                    }
                    ApprovalResolution::TimedOut => {
                        return Err(format!(
                            "integration_approval_timeout: tempo esgotado aguardando aprovação do dono para '{}' na integração '{}'",
                            discovered.name, config.id
                        ));
                    }
                    ApprovalResolution::Cancelled => {
                        return Err(format!(
                            "integration_approval_cancelled: o pedido de aprovação para '{}' na integração '{}' foi cancelado",
                            discovered.name, config.id
                        ));
                    }
                }
                waited_for_approval = true;
            }
        }
    }

    check_local_auth_state(st, &config).await?;

    // Revalidação obrigatória na admissão de todo envio (com ou sem cartão): impede TOCTOU de
    // compartilhamento, destino alterado, integração desafixada/desabilitada ou `AllowAlways` revogado.
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }
    config = load_integration(st, &config.id)?;
    if !config.enabled {
        return Err(format!(
            "integration_disabled: integração '{}' está desabilitada",
            config.id
        ));
    }
    if from_pinned_alias && !config.pinned {
        return Err(format!(
            "integration_unpinned: integração '{}' foi desafixada; use integration_call",
            config.id
        ));
    }
    if config_fingerprint(&config) != expected_config_fp {
        return Err(format!(
            "integration_config_changed: o destino ou autenticação da integração '{}' mudou; tente novamente",
            config.id
        ));
    }
    ensure_project_allowed(st, &config, target_session).await?;
    let admitted_access = classify_tool_access(
        config.policy,
        &config.allowed_tools,
        discovered.read_only_hint,
        &discovered.name,
    );
    match admitted_access {
        ToolAccess::Blocked => {
            return Err(format!(
                "integration_read_only: a integração '{}' está em modo somente leitura e bloqueia a ferramenta '{}'",
                config.id, discovered.name
            ));
        }
        ToolAccess::AskWrite if !waited_for_approval => {
            return Err(format!(
                "integration_approval_required: a permissão de '{}' na integração '{}' mudou e exige nova aprovação",
                discovered.name, config.id
            ));
        }
        ToolAccess::Read | ToolAccess::AllowedAlways | ToolAccess::AskWrite => {}
    }

    let mut cancel_guard = CancelWarningGuard {
        st,
        integration_id: config.id.clone(),
        tool: discovered.name.clone(),
        started: external_started.clone(),
        armed: true,
    };
    let call_result = st
        .integrations
        .clients
        .call_tool_tracked(
            &config.id,
            &discovered.name,
            args_value,
            Some(external_started),
        )
        .await;
    cancel_guard.armed = false;

    let output = match call_result {
        Ok(output) => output,
        Err(err) => {
            if let Some(session_name) = target_session {
                shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS)
                    .await?;
            }
            return Err(map_client_error(st, &config.id, err));
        }
    };
    st.integrations.clear_error(&config.id);

    // Se convite/par abriu enquanto a chamada externa rodava, nunca devolve a resposta.
    if let Some(session_name) = target_session {
        shared_guard::ensure_unshared(st, api, session_name, shared_guard::INTEGRATIONS).await?;
    }

    let wrapped = wrap_external_result(&config.id, &output.text);
    if output.is_error {
        Ok(CallToolResult::error(vec![ContentBlock::text(wrapped)]))
    } else {
        Ok(CallToolResult::success(vec![ContentBlock::text(wrapped)]))
    }
}

pub(super) async fn list_pinned_tools(
    st: &AppState,
    api: &Router,
    index: &PinnedToolIndex,
    headers: &HeaderMap,
) -> Vec<Tool> {
    let Ok((me, origin)) = resolve_caller(st, headers).await else {
        return Vec::new();
    };
    if origin == Origin::Voice {
        return Vec::new();
    }
    if shared_guard::ensure_unshared(st, api, &me, shared_guard::INTEGRATIONS)
        .await
        .is_err()
    {
        return Vec::new();
    }
    let Ok(configs) = st.integrations.registry.list() else {
        return Vec::new();
    };
    let mut pinned: Vec<IntegrationConfig> = configs
        .into_iter()
        .filter(|cfg| cfg.enabled && cfg.pinned)
        .collect();
    if pinned.is_empty() {
        return Vec::new();
    }
    if pinned.iter().any(|cfg| !cfg.projects.is_empty()) {
        let session_paths = resolve_session_paths(st, &me).await.unwrap_or_default();
        pinned.retain(|cfg| {
            cfg.projects.is_empty() || paths_match_projects(&session_paths, &cfg.projects)
        });
    }
    if pinned.is_empty() {
        return Vec::new();
    }

    // Descobre integrações prontas ainda ausentes do cache com teto curto de 3 s em paralelo.
    let discovery_futures = pinned.iter().map(|cfg| async move {
        if let Some(cached) = st.integrations.tools_cache.get(&cfg.id) {
            return Some(cached);
        }
        let secrets = st.integrations.secrets.get(&cfg.id).await.ok()?;
        if compute_status(cfg, secrets.as_ref()) != IntegrationStatus::Ready {
            return None;
        }
        match tokio::time::timeout(
            LIST_TOOLS_DISCOVERY_TIMEOUT,
            st.integrations.clients.list_tools(&cfg.id),
        )
        .await
        {
            Ok(Ok(tools)) => {
                st.integrations.clear_error(&cfg.id);
                Some(tools)
            }
            _ => None,
        }
    });
    let discovered_lists = futures_util::future::join_all(discovery_futures).await;

    // Se convite/par abriu durante a descoberta, omite as tools int__* preservando o catálogo fixo.
    if shared_guard::ensure_unshared(st, api, &me, shared_guard::INTEGRATIONS)
        .await
        .is_err()
    {
        return Vec::new();
    }

    let mut used_names = HashSet::new();
    let mut out = Vec::new();
    for (cfg, maybe_tools) in pinned.into_iter().zip(discovered_lists) {
        let Some(tools) = maybe_tools else {
            continue;
        };
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        let mcp_names = assign_mcp_tool_names(&cfg.id, &tool_names);
        let mut pairs = Vec::with_capacity(tools.len());
        for (mcp_name, tool) in mcp_names.into_iter().zip(tools) {
            if !used_names.insert(mcp_name.clone()) {
                continue;
            }
            pairs.push((mcp_name.clone(), tool.name.clone()));
            out.push(to_rmcp_tool(mcp_name, &cfg, tool));
        }
        index.replace_for_integration(&cfg.id, &pairs);
    }
    out
}

fn build_pairs_for_integration(
    integration_id: &str,
    tools: &[DiscoveredTool],
) -> Vec<(String, String)> {
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assign_mcp_tool_names(integration_id, &tool_names)
        .into_iter()
        .zip(tools.iter().map(|t| t.name.clone()))
        .collect()
}

/// Resolve nomes `int__<id>__<tool>` independentemente da ordem em que o servidor listar as tools.
pub(super) fn assign_mcp_tool_names(integration_id: &str, tool_names: &[&str]) -> Vec<String> {
    let safe_id = sanitize_segment(integration_id);
    let candidates: Vec<String> = tool_names
        .iter()
        .map(|name| format!("int__{safe_id}__{}", sanitize_segment(name)))
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::with_capacity(candidates.len());
    for cand in &candidates {
        *counts.entry(cand.as_str()).or_insert(0) += 1;
    }

    let mut used = HashSet::with_capacity(tool_names.len());
    let mut out = vec![String::new(); tool_names.len()];
    let mut need_hash = Vec::new();

    for (idx, cand) in candidates.iter().enumerate() {
        if cand.len() <= MAX_MCP_TOOL_NAME_BYTES && counts[cand.as_str()] == 1 {
            used.insert(cand.clone());
            out[idx] = cand.clone();
        } else {
            need_hash.push(idx);
        }
    }

    // Ordena colisões pelo nome bruto antes de atribuir sufixos para manter estabilidade entre descobertas.
    need_hash.sort_by(|&a, &b| tool_names[a].cmp(tool_names[b]));
    for idx in need_hash {
        let raw_name = tool_names[idx];
        let cand = &candidates[idx];
        let hash = short_tool_hash(integration_id, raw_name);
        let prefix_budget = MAX_MCP_TOOL_NAME_BYTES - 1 - hash.len();
        let prefix = &cand[..cand.len().min(prefix_budget)];
        let hashed = format!("{prefix}_{hash}");
        if used.insert(hashed.clone()) {
            out[idx] = hashed;
            continue;
        }
        for n in 2u32.. {
            let suffix = format!("_{hash}_{n}");
            let budget = MAX_MCP_TOOL_NAME_BYTES.saturating_sub(suffix.len());
            let prefix = &cand[..cand.len().min(budget)];
            let alt = format!("{prefix}{suffix}");
            if used.insert(alt.clone()) {
                out[idx] = alt;
                break;
            }
        }
    }

    out
}

fn hash_canonical_value(ctx: &mut ring::digest::Context, val: &Value) {
    match val {
        Value::Null => ctx.update(b"n"),
        Value::Bool(b) => {
            ctx.update(b"b");
            ctx.update(&[u8::from(*b)]);
        }
        Value::Number(num) => {
            ctx.update(b"d");
            let text = num.to_string();
            ctx.update(&(text.len() as u64).to_le_bytes());
            ctx.update(text.as_bytes());
        }
        Value::String(s) => {
            ctx.update(b"s");
            ctx.update(&(s.len() as u64).to_le_bytes());
            ctx.update(s.as_bytes());
        }
        Value::Array(items) => {
            ctx.update(b"a");
            ctx.update(&(items.len() as u64).to_le_bytes());
            for item in items {
                hash_canonical_value(ctx, item);
            }
        }
        Value::Object(map) => {
            ctx.update(b"o");
            ctx.update(&(map.len() as u64).to_le_bytes());
            let ordered: BTreeMap<&str, &Value> =
                map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for (k, v) in ordered {
                ctx.update(&(k.len() as u64).to_le_bytes());
                ctx.update(k.as_bytes());
                hash_canonical_value(ctx, v);
            }
        }
    }
}

fn canonical_args_hash(val: &Value) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    hash_canonical_value(&mut ctx, val);
    ctx.finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sanitize_segment(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("tool");
    }
    out
}

fn short_tool_hash(integration_id: &str, tool_name: &str) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(integration_id.as_bytes());
    ctx.update(b"\0");
    ctx.update(tool_name.as_bytes());
    let digest = ctx.finish();
    digest.as_ref()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn to_rmcp_tool(mcp_name: String, config: &IntegrationConfig, tool: DiscoveredTool) -> Tool {
    let mut schema_map = match tool.input_schema {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    schema_map
        .entry("type")
        .or_insert_with(|| Value::String("object".to_owned()));
    let description = match tool
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(desc) => format!("[{}] {desc}", config.name),
        None => format!(
            "[{}] Ferramenta '{}' da integração '{}'",
            config.name, tool.name, config.id
        ),
    };
    let mut out = Tool::new(mcp_name, description, Arc::new(schema_map));
    if let Some(read_only) = tool.read_only_hint {
        out = out.with_annotations(ToolAnnotations::new().read_only(read_only));
    }
    out
}

fn server_id_or_local(st: &AppState, fallback_server_id: &str) -> String {
    let current = st.current_server_id();
    if !current.trim().is_empty() {
        current
    } else if !fallback_server_id.trim().is_empty() {
        fallback_server_id.to_owned()
    } else {
        "local".to_owned()
    }
}

async fn resolve_approval_session_key(
    st: &AppState,
    headers: &HeaderMap,
    caller_name: &str,
) -> String {
    if let Some(key) = headers
        .get("x-hangar-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return truncate_preview_chars(&key.replace('\0', ""), 256);
    }
    if let Ok(snapshot) = st.list.snapshot().await
        && let Some(life) = snapshot
            .rows
            .iter()
            .find(|r| r.name == caller_name)
            .and_then(|r| r.lifecycle_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return truncate_preview_chars(&life.replace('\0', ""), 256);
    }
    truncate_preview_chars(&format!("session:{}", caller_name.replace('\0', "")), 256)
}

fn map_registry_error(err: RegistryError) -> String {
    match err {
        RegistryError::Validation(v) => format!("integration_invalid: {v}"),
        RegistryError::AlreadyExists(id) => {
            format!("integration_already_exists: integração '{id}' já existe")
        }
        RegistryError::NotFound(id) => {
            format!("integration_not_found: integração '{id}' não encontrada")
        }
        RegistryError::Corrupted(reason) => {
            format!("integration_registry_corrupted: catálogo de integrações ilegível ({reason})")
        }
        RegistryError::Io(reason) => {
            format!("integration_registry_io: erro ao acessar catálogo de integrações ({reason})")
        }
    }
}

fn map_secret_store_error(err: SecretStoreError) -> String {
    match err {
        SecretStoreError::InvalidId => {
            "integration_invalid: identificador de integração inválido".into()
        }
        SecretStoreError::KeyLost
        | SecretStoreError::Key(SecretKeyError::Absent | SecretKeyError::Lost) => {
            "integration_secrets_key_lost: chave de segredos das integrações ausente no chaveiro"
                .into()
        }
        SecretStoreError::Key(
            SecretKeyError::Locked | SecretKeyError::Bus | SecretKeyError::Unsupported,
        ) => "integration_secrets_unavailable: chaveiro de segredos das integrações indisponível"
            .into(),
        SecretStoreError::Corrupted => {
            "integration_secrets_corrupted: arquivo de segredos das integrações ilegível".into()
        }
        SecretStoreError::Io(reason) => {
            format!("integration_secrets_io: erro ao ler segredos das integrações ({reason})")
        }
    }
}

fn map_client_error(st: &AppState, id: &str, err: ClientError) -> String {
    match err {
        ClientError::Disabled => {
            format!("integration_disabled: integração '{id}' está desabilitada")
        }
        ClientError::AuthRequired(reason) => {
            st.integrations.record_error(id, reason.clone());
            format!("integration_auth_required: {reason}")
        }
        ClientError::AuthExpired(reason) => {
            st.integrations.record_error(id, reason.clone());
            format!("integration_auth_expired: {reason}")
        }
        ClientError::Timeout(reason) => {
            st.integrations.record_error(id, reason.clone());
            format!("integration_timeout: {reason}")
        }
        ClientError::Transport(reason) | ClientError::Protocol(reason) => {
            st.integrations.record_error(id, reason.clone());
            format!("integration_upstream_error: {reason}")
        }
        ClientError::ToolNotFound(name) => {
            format!("integration_tool_not_found: ferramenta '{name}' não encontrada na integração '{id}'")
        }
        ClientError::RemoteError { code, message } => {
            // Não persiste texto bruto remoto em `last_error` para nunca armazenar eco de segredo.
            let summary = format!("erro remoto MCP ({code}) retornado pelo servidor externo");
            st.integrations.record_error(id, summary);
            format!(
                "integration_upstream_error: erro remoto MCP ({code}):\n{}",
                wrap_external_result(id, &message)
            )
        }
        ClientError::Registry(e) => map_registry_error(e),
        ClientError::Secrets(e) => map_secret_store_error(e),
    }
}

fn map_approval_error(err: ApprovalError) -> String {
    match err {
        ApprovalError::Invalid(reason) => format!("integration_approval_invalid: {reason}"),
        ApprovalError::TooMany => {
            "integration_approval_too_many: muitos pedidos de aprovação aguardando resposta".into()
        }
        ApprovalError::Cooldown => {
            "integration_approval_cooldown: o dono acabou de negar um pedido desta sessão; aguarde antes de tentar de novo".into()
        }
        ApprovalError::Lost
        | ApprovalError::Stale
        | ApprovalError::Answered
        | ApprovalError::Claimed => {
            format!("integration_approval_cancelled: {err}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_filter_distinguishes_sibling_prefixes_nested_paths_and_slugs() {
        assert!(path_matches_project("/tmp/proj", "/tmp/proj"));
        assert!(path_matches_project("/tmp/proj/sub", "/tmp/proj"));
        assert!(path_matches_project("/tmp/proj/sub/", "/tmp/proj/"));
        assert!(path_matches_project("/tmp/proj/sub/..", "/tmp/proj"));
        assert!(!path_matches_project("/tmp/proj2", "/tmp/proj"), "prefixo irmão não pode casar");
        assert!(!path_matches_project("/tmp/proj", ""), "projeto vazio é fail-closed");
        assert!(!path_matches_project("/tmp/proj", "."), "ponto relativo não casa caminho absoluto");

        let slug = hangar_projects::slug("/tmp/proj");
        assert!(path_matches_project("/tmp/proj", &slug));
        assert!(path_matches_project("/tmp/proj/", &slug));
        assert!(path_matches_project("/tmp/proj/sub", &slug));
        assert!(!path_matches_project("/tmp/proj2", &slug));
        assert!(!path_matches_project("/tmp/proj2/sub", &slug));
        assert!(!path_matches_project("/tmp/proj", &hangar_projects::slug("/")));
    }

    #[test]
    fn mcp_tool_names_are_deterministic_sanitized_capped_at_64_and_collision_free() {
        let order_a = assign_mcp_tool_names("github", &["get_issue", "issues.create", "issues:create"]);
        let order_b = assign_mcp_tool_names("github", &["issues:create", "issues.create", "get_issue"]);
        assert_eq!(order_a[0], "int__github__get_issue");
        assert_eq!(order_b[2], "int__github__get_issue");

        // Colisão pós-sanitização recebe hash em todas as envolvidas, independente da ordem da lista.
        assert_ne!(order_a[1], "int__github__issues_create");
        assert_ne!(order_a[2], "int__github__issues_create");
        assert_ne!(order_a[1], order_a[2]);
        assert_eq!(order_a[1], order_b[1]);
        assert_eq!(order_a[2], order_b[0]);
        assert!(order_a[1].len() <= MAX_MCP_TOOL_NAME_BYTES);
        assert!(order_a[2].len() <= MAX_MCP_TOOL_NAME_BYTES);

        let long_name = "tool_with_a_very_long_name_that_certainly_exceeds_sixty_four_bytes_total";
        let out_a = assign_mcp_tool_names("my-integration", &[long_name]);
        let out_b = assign_mcp_tool_names("my-integration", &[long_name]);
        assert_eq!(out_a, out_b);
        assert_eq!(out_a[0].len(), MAX_MCP_TOOL_NAME_BYTES);
        assert!(out_a[0].starts_with("int__my-integration__"));
        assert!(
            out_a[0]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }
}
