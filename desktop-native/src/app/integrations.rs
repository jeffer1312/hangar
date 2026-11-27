//! Página Integrações do servidor conectado: catálogo pronto, cadastro HTTP/stdio sem expor segredos,
//! OAuth 2.1, teste de conexão e liberação por ferramenta.
use super::*;
use super::settings::{section_head, settings_box};
use super::vault::{alert, chip, muted};
use gpui_kit::component::switch::Switch;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const READ_SECONDS: u64 = 25;
const MUTATION_SECONDS: u64 = 30;
const TEST_SECONDS: u64 = 45;
const MAX_SLUG_BYTES: usize = 64;
const MAX_NAME_BYTES: usize = 200;
const MAX_URL_BYTES: usize = 2048;

const RESERVED_HTTP_HEADERS: [&str; 5] = [
    "authorization",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) enum PresetAuthKind {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "oauth_dcr")]
    OAuthDcr,
    #[serde(rename = "oauth_manual_client")]
    OAuthManualClient,
    /// Só chega de servidor anterior ao preset por token.
    #[serde(rename = "oauth_dcr_or_bearer")]
    OAuthDcrOrBearer,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct SetupGuide {
    pub title: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub docs_hint: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct CatalogPreset {
    pub id: String,
    pub name: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub url: String,
    #[serde(default)]
    pub url_editable: bool,
    pub auth_kind: PresetAuthKind,
    #[serde(default)]
    pub default_scopes: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub requires_loopback_redirect: bool,
    #[serde(default)]
    pub setup_guide: Option<SetupGuide>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Transport {
    RemoteHttp {
        url: String,
        #[serde(default)]
        header_names: Vec<String>,
    },
    LocalStdio {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        env_keys: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum AuthConfig {
    None,
    Bearer,
    #[serde(rename = "oauth_dcr")]
    OAuthDcr,
    #[serde(rename = "oauth_manual_client")]
    OAuthManualClient {
        client_id: String,
        #[serde(default)]
        scopes: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermissionPolicy {
    #[default]
    ReadFreeAskWrite,
    AllowAll,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum IntegrationStatus {
    Ready,
    AuthRequired,
    AuthExpired,
    Disabled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolAccess {
    Read,
    AllowedAlways,
    AskWrite,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct ToolSummary {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    pub effective_access: ToolAccess,
}

/// Representação pública da integração recebida do servidor (nunca carrega segredos).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct IntegrationItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub transport: Transport,
    pub auth: AuthConfig,
    #[serde(default)]
    pub policy: PermissionPolicy,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub created_at: f64,
    #[allow(dead_code)]
    #[serde(default)]
    pub updated_at: f64,
    pub status: IntegrationStatus,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Deserialize)]
struct IntegrationsListResponse {
    integrations: Vec<IntegrationItem>,
}

#[derive(Deserialize)]
struct CatalogResponse {
    presets: Vec<CatalogPreset>,
}

#[derive(Deserialize)]
struct ToolsResponse {
    tools: Vec<ToolSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum OAuthStartResponse {
    OpenUrl {
        authorize_url: String,
        #[allow(dead_code)]
        #[serde(default)]
        redirect_uri: String,
    },
    HandoffDesktop {
        authorize_url: String,
        #[allow(dead_code)]
        #[serde(default)]
        redirect_uri: String,
        #[serde(default)]
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransportKind {
    RemoteHttp,
    LocalStdio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthKindChoice {
    OAuthDcr,
    OAuthManualClient,
    Bearer,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FormDraft {
    pub is_edit: bool,
    pub id: String,
    pub name: String,
    pub preset: Option<String>,
    pub enabled: bool,
    pub transport: TransportKind,
    pub url: String,
    pub existing_headers: Vec<String>,
    pub removed_headers: BTreeSet<String>,
    pub new_headers_raw: String,
    pub command_raw: String,
    pub cwd: String,
    pub existing_env_keys: Vec<String>,
    pub removed_env_keys: BTreeSet<String>,
    pub new_env_raw: String,
    pub auth_kind: AuthKindChoice,
    pub client_id: String,
    pub client_secret: String,
    pub scopes_raw: String,
    pub bearer_token: String,
    pub policy: PermissionPolicy,
    pub pinned: bool,
    pub projects_raw: String,
    /// Comando formatado na abertura da edição junto do vetor original, para preservar argumentos com espaços/quebras sem reedição.
    pub original_command: Option<(String, Vec<String>)>,
    /// Transporte salvo ao abrir a edição, para omitir `transport` do PATCH quando nada nele mudar.
    pub original_transport: Option<Transport>,
    /// Política salva ao abrir a edição, para omitir `policy` do PATCH quando não for alterada.
    pub original_policy: Option<PermissionPolicy>,
}

enum FormMode {
    Preset(CatalogPreset),
    Custom,
    Edit(IntegrationItem, Option<CatalogPreset>),
}

/// Retorno da última ação numa integração (Testar, Autenticar, Ativa…), mostrado no card dela.
#[derive(Clone)]
enum Feedback {
    Info(String),
    Error(String),
}

struct PendingHandoff {
    handoff_id: u64,
    integration_id: String,
    authorize_url: String,
    reason: String,
}

pub(super) struct IntegrationsPage {
    api: Api,
    runtime: Arc<Runtime>,
    hangar: WeakEntity<Hangar>,
    seq: u64,
    catalog_seq: u64,
    oauth_start_seq: u64,
    tools_epoch: u64,
    tools_seq: HashMap<String, u64>,
    handoff_seq: u64,
    items: Option<Result<Vec<IntegrationItem>, String>>,
    catalog: Option<Result<Vec<CatalogPreset>, String>>,
    tools: HashMap<String, Vec<ToolSummary>>,
    tools_errors: HashMap<String, String>,
    loading_tools: HashSet<String>,
    pending_tools_reload: HashSet<String>,
    open_tools: HashSet<String>,
    busy: HashSet<String>,
    testing: HashSet<String>,
    feedback: HashMap<String, Feedback>,
    notice: Option<String>,
    handoff: Option<PendingHandoff>,
}

struct IntegrationForm {
    api: Api,
    runtime: Arc<Runtime>,
    page: WeakEntity<IntegrationsPage>,
    is_edit: bool,
    preset: Option<String>,
    url_editable: bool,
    preset_auth: Option<PresetAuthKind>,
    setup_guide: Option<SetupGuide>,
    enabled: bool,
    transport: TransportKind,
    auth_kind: AuthKindChoice,
    policy: PermissionPolicy,
    pinned: bool,
    id_input: Entity<InputState>,
    name_input: Entity<InputState>,
    url_input: Entity<InputState>,
    existing_headers: Vec<String>,
    removed_headers: BTreeSet<String>,
    headers_input: Entity<TextareaState>,
    command_input: Entity<TextareaState>,
    cwd_input: Entity<InputState>,
    existing_env_keys: Vec<String>,
    removed_env_keys: BTreeSet<String>,
    env_input: Entity<TextareaState>,
    client_id_input: Entity<InputState>,
    client_secret_input: Entity<InputState>,
    scopes_input: Entity<InputState>,
    bearer_input: Entity<InputState>,
    projects_input: Entity<TextareaState>,
    original_command: Option<(String, Vec<String>)>,
    original_transport: Option<Transport>,
    original_policy: Option<PermissionPolicy>,
    saving: bool,
    error: Option<String>,
}

struct InitialFormValues {
    is_edit: bool,
    id: String,
    name: String,
    preset: Option<String>,
    url_editable: bool,
    preset_auth: Option<PresetAuthKind>,
    setup_guide: Option<SetupGuide>,
    enabled: bool,
    transport: TransportKind,
    url: String,
    existing_headers: Vec<String>,
    command: String,
    cwd: String,
    existing_env_keys: Vec<String>,
    auth_kind: AuthKindChoice,
    client_id: String,
    scopes: String,
    policy: PermissionPolicy,
    pinned: bool,
    projects: String,
    original_command: Option<(String, Vec<String>)>,
    original_transport: Option<Transport>,
    original_policy: Option<PermissionPolicy>,
}

impl IntegrationsPage {
    fn new(
        api: Api,
        runtime: Arc<Runtime>,
        hangar: WeakEntity<Hangar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut page = Self {
            api,
            runtime,
            hangar,
            seq: 0,
            catalog_seq: 0,
            oauth_start_seq: 0,
            tools_epoch: 0,
            tools_seq: HashMap::new(),
            handoff_seq: 0,
            items: None,
            catalog: None,
            tools: HashMap::new(),
            tools_errors: HashMap::new(),
            loading_tools: HashSet::new(),
            pending_tools_reload: HashSet::new(),
            open_tools: HashSet::new(),
            busy: HashSet::new(),
            testing: HashSet::new(),
            feedback: HashMap::new(),
            notice: None,
            handoff: None,
        };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.seq += 1;
        self.catalog_seq += 1;
        self.tools_epoch += 1;
        let seq = self.seq;
        let catalog_seq = self.catalog_seq;
        self.items = None;
        self.catalog = None;
        self.tools_errors.clear();
        self.pending_tools_reload.clear();
        self.tools_seq.clear();
        self.feedback.clear();
        let api_list = self.api.clone();
        run(
            &self.runtime,
            async move { api_list.server_read(&["integrations"], &[], READ_SECONDS).await },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                if seq != this.seq {
                    return;
                }
                let parsed = result
                    .map_err(|e| integrations_failure(&e))
                    .and_then(|v| {
                        serde_json::from_value::<IntegrationsListResponse>(v)
                            .map(|r| r.integrations)
                            .map_err(|_| tr("invalid_response"))
                    });
                if let Ok(list) = parsed {
                    this.reconcile_loaded_items(list, window, cx);
                    return;
                }
                this.items = Some(parsed);
            },
        );
        let api_cat = self.api.clone();
        run(
            &self.runtime,
            async move { api_cat.server_read(&["integrations", "catalog"], &[], READ_SECONDS).await },
            window,
            cx,
            move |this: &mut Self, result, _, _| {
                if catalog_seq != this.catalog_seq {
                    return;
                }
                this.catalog = Some(
                    result
                        .map_err(|e| integrations_failure(&e))
                        .and_then(|v| {
                            serde_json::from_value::<CatalogResponse>(v)
                                .map(|r| r.presets)
                                .map_err(|_| tr("invalid_response"))
                        }),
                );
            },
        );
        cx.notify();
    }

    fn reconcile_loaded_items(
        &mut self,
        list: Vec<IntegrationItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let alive: HashSet<String> = list.iter().map(|it| it.id.clone()).collect();
        // Invalida cache das seções fechadas e revalida `effective_access` nas seções abertas.
        self.tools.clear();
        self.tools_errors.retain(|id, _| alive.contains(id));
        self.open_tools.retain(|id| alive.contains(id));
        let open_ids: Vec<String> = self.open_tools.iter().cloned().collect();
        self.items = Some(Ok(list));
        for id in open_ids {
            self.load_tools(&id, window, cx);
        }
    }

    fn reload_items(&mut self, reconcile_tools: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.seq;
        let api = self.api.clone();
        run(
            &self.runtime,
            async move { api.server_read(&["integrations"], &[], READ_SECONDS).await },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                if seq != this.seq {
                    return;
                }
                match result
                    .map_err(|e| integrations_failure(&e))
                    .and_then(|v| {
                        serde_json::from_value::<IntegrationsListResponse>(v)
                            .map(|r| r.integrations)
                            .map_err(|_| tr("invalid_response"))
                    }) {
                    Ok(list) if reconcile_tools => this.reconcile_loaded_items(list, window, cx),
                    Ok(list) => this.items = Some(Ok(list)),
                    Err(msg) if this.items.is_none() => this.items = Some(Err(msg)),
                    Err(_) => {}
                }
            },
        );
    }

    fn next_tools_seq(&mut self, id: &str) -> u64 {
        let entry = self.tools_seq.entry(id.to_owned()).or_insert(0);
        *entry += 1;
        *entry
    }

    fn current_tools_seq(&self, id: &str) -> u64 {
        self.tools_seq.get(id).copied().unwrap_or(0)
    }

    fn bump_tools_seq(&mut self, id: &str) {
        let _ = self.next_tools_seq(id);
    }

    fn preset_for(&self, preset_id: Option<&str>) -> Option<CatalogPreset> {
        let id = preset_id?;
        self.catalog
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    fn open_form(&mut self, mode: FormMode, window: &mut Window, cx: &mut Context<Self>) {
        let title = match &mode {
            FormMode::Preset(p) => p.name.clone(),
            FormMode::Custom => tr_shared("integrations_add_custom", &[]),
            FormMode::Edit(item, _) => format!("{} · {}", tr_shared("integrations_edit", &[]), item.name),
        };
        let existing = self
            .items
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (api, runtime, page) = (self.api.clone(), self.runtime.clone(), cx.entity().downgrade());
        let form = cx.new(|cx| IntegrationForm::new(api, runtime, page, mode, existing, window, cx));
        let focus = form.read(cx).initial_focus().clone();
        window.open_dialog(cx, move |dialog, _, _| {
            popup::dialog(dialog)
                .w(px(560.))
                .title(title.clone())
                .child(form.clone())
        });
        window.on_next_frame(move |window, cx| focus.update(cx, |input, cx| input.focus(window, cx)));
    }

    fn saved(
        &mut self,
        created: bool,
        item: IntegrationItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Invalida leituras de lista anteriores à gravação confirmada.
        self.seq += 1;
        self.feedback.remove(&item.id);
        self.notice = Some(tr_shared(
            if created { "integrations_created" } else { "integrations_saved" },
            &[],
        ));
        if let Some(Ok(list)) = self.items.as_mut() {
            if let Some(slot) = list.iter_mut().find(|existing| existing.id == item.id) {
                *slot = item.clone();
            } else {
                list.push(item.clone());
            }
        } else {
            // Se ainda não havia lista utilizável (ex.: POST terminou antes do primeiro GET), busca e reconcilia a lista.
            self.items = None;
            self.reload_items(true, window, cx);
        }
        self.tools_errors.remove(&item.id);
        self.tools.remove(&item.id);
        self.load_tools(&item.id, window, cx);
        if should_auto_start_oauth(created, &item) {
            self.start_oauth(item.id.clone(), window, cx);
        }
        cx.notify();
    }

    fn patch_field(&mut self, id: String, body: Value, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert(id.clone()) {
            return;
        }
        self.notice = None;
        self.feedback.remove(&id);
        let (api, path) = (self.api.clone(), id.clone());
        run(
            &self.runtime,
            async move {
                api.server_send(reqwest::Method::PATCH, &["integrations", &path], Some(body), MUTATION_SECONDS)
                    .await
            },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                this.busy.remove(&id);
                match result {
                    Ok(value) => {
                        // Evita que um reload_items iniciado antes do PATCH sobrescreva o item atualizado.
                        this.seq += 1;
                        if let Some(updated) = value
                            .get("integration")
                            .cloned()
                            .and_then(|v| serde_json::from_value::<IntegrationItem>(v).ok())
                            && let Some(Ok(list)) = this.items.as_mut()
                            && let Some(slot) = list.iter_mut().find(|item| item.id == updated.id)
                        {
                            *slot = updated;
                        } else if this.items.is_none() {
                            // Se Atualizar estava em voo e foi invalidado por `this.seq += 1`, dispara leitura substituta.
                            this.reload_items(true, window, cx);
                        }
                        if this.open_tools.contains(&id) {
                            this.load_tools(&id, window, cx);
                        } else {
                            this.bump_tools_seq(&id);
                            this.tools.remove(&id);
                            this.tools_errors.remove(&id);
                        }
                    }
                    Err(error) => {
                        this.feedback.insert(id, Feedback::Error(integrations_failure(&error)));
                    }
                }
            },
        );
        cx.notify();
    }

    fn set_enabled(&mut self, id: String, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.patch_field(id, json!({ "enabled": on }), window, cx);
    }

    fn set_policy(&mut self, id: String, policy: PermissionPolicy, window: &mut Window, cx: &mut Context<Self>) {
        self.patch_field(id, json!({ "policy": policy }), window, cx);
    }

    fn set_pinned(&mut self, id: String, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.patch_field(id, json!({ "pinned": pinned }), window, cx);
    }

    fn toggle_allowed_tool(
        &mut self,
        id: String,
        tool_name: String,
        allow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.patch_field(id, build_tool_toggle_patch(&tool_name, allow), window, cx);
    }

    fn test_connection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.testing.insert(id.clone()) {
            return;
        }
        self.notice = None;
        self.feedback.remove(&id);
        let (api, path) = (self.api.clone(), id.clone());
        run(
            &self.runtime,
            async move {
                api.server_send(reqwest::Method::POST, &["integrations", &path, "test"], None, TEST_SECONDS)
                    .await
            },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                this.testing.remove(&id);
                match result {
                    Ok(value) => {
                        let tools = value
                            .get("tools")
                            .cloned()
                            .and_then(|v| serde_json::from_value::<Vec<ToolSummary>>(v).ok())
                            .unwrap_or_default();
                        let count = tools.len().to_string();
                        this.tools_errors.remove(&id);
                        this.bump_tools_seq(&id);
                        this.tools.insert(id.clone(), tools);
                        this.open_tools.insert(id.clone());
                        this.feedback
                            .insert(id.clone(), Feedback::Info(tr_shared("integrations_test_ok", &[("n", &count)])));
                        this.seq += 1;
                        this.reload_items(false, window, cx);
                    }
                    Err(error) => {
                        this.feedback.insert(id.clone(), Feedback::Error(integrations_failure(&error)));
                        this.seq += 1;
                        this.reload_items(false, window, cx);
                    }
                }
            },
        );
        cx.notify();
    }

    fn start_oauth(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert(id.clone()) {
            return;
        }
        self.oauth_start_seq += 1;
        let start_seq = self.oauth_start_seq;
        self.notice = None;
        self.feedback
            .insert(id.clone(), Feedback::Info(tr_shared("integrations_oauth_opening", &[])));
        self.handoff = None;
        let (api, path) = (self.api.clone(), id.clone());
        let start_body = oauth_start_body(&api);
        run(
            &self.runtime,
            async move {
                api.server_send(
                    reqwest::Method::POST,
                    &["integrations", &path, "oauth", "start"],
                    Some(start_body),
                    MUTATION_SECONDS,
                )
                .await
            },
            window,
            cx,
            move |this: &mut Self, result, _, cx| {
                this.busy.remove(&id);
                if start_seq != this.oauth_start_seq {
                    return;
                }
                match result {
                    Ok(value) => match serde_json::from_value::<OAuthStartResponse>(value) {
                        Ok(OAuthStartResponse::OpenUrl { authorize_url, .. }) => {
                            if let Some(safe) = safe_authorize_url(&authorize_url) {
                                cx.open_url(&safe);
                                this.feedback.insert(
                                    id,
                                    Feedback::Info(tr_shared("integrations_oauth_started", &[])),
                                );
                            } else {
                                this.feedback.insert(id, Feedback::Error(tr("invalid_response")));
                            }
                        }
                        Ok(OAuthStartResponse::HandoffDesktop {
                            authorize_url,
                            reason,
                            ..
                        }) => {
                            this.feedback.remove(&id);
                            this.handoff_seq += 1;
                            this.handoff = Some(PendingHandoff {
                                handoff_id: this.handoff_seq,
                                integration_id: id,
                                authorize_url,
                                reason,
                            });
                        }
                        Err(_) => {
                            this.feedback.insert(id, Feedback::Error(tr("invalid_response")));
                        }
                    },
                    Err(error) => {
                        this.feedback.insert(id, Feedback::Error(integrations_failure(&error)));
                    }
                }
            },
        );
        cx.notify();
    }

    fn open_handoff_on_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handoff) = self.handoff.as_ref() else {
            return;
        };
        let (handoff_id, id, authorize_url) = (
            handoff.handoff_id,
            handoff.integration_id.clone(),
            handoff.authorize_url.clone(),
        );
        if !self.busy.insert(id.clone()) {
            return;
        }
        let api = self.api.clone();
        let path = id.clone();
        run(
            &self.runtime,
            async move {
                api.server_send(
                    reqwest::Method::POST,
                    &["integrations", &path, "oauth", "open-on-desktop"],
                    Some(json!({ "authorize_url": authorize_url })),
                    MUTATION_SECONDS,
                )
                .await
            },
            window,
            cx,
            move |this: &mut Self, result, _, _| {
                this.busy.remove(&id);
                let same_handoff = this
                    .handoff
                    .as_ref()
                    .is_some_and(|h| h.handoff_id == handoff_id);
                match result {
                    Ok(_) => {
                        // Não apaga o handoff de outra integração aberta enquanto esta chamada estava em voo.
                        if same_handoff {
                            this.handoff = None;
                        }
                        if same_handoff || this.handoff.is_none() {
                            this.feedback.insert(
                                id,
                                Feedback::Info(tr_shared("integrations_oauth_desktop_opened", &[])),
                            );
                        }
                    }
                    Err(error) => {
                        if same_handoff || this.handoff.is_none() {
                            this.feedback.insert(id, Feedback::Error(integrations_failure(&error)));
                        }
                    }
                }
            },
        );
        cx.notify();
    }

    fn toggle_tools(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_tools.remove(&id) {
            cx.notify();
            return;
        }
        self.open_tools.insert(id.clone());
        if !self.tools.contains_key(&id) {
            self.load_tools(&id, window, cx);
        }
        cx.notify();
    }

    fn load_tools(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = id.to_owned();
        if !self.loading_tools.insert(id.clone()) {
            // Nova leitura pedida após mudança de política: reexecuta assim que o GET em voo voltar.
            self.pending_tools_reload.insert(id);
            return;
        }
        self.tools_errors.remove(&id);
        let seq = self.next_tools_seq(&id);
        let epoch = self.tools_epoch;
        let (api, path) = (self.api.clone(), id.clone());
        run(
            &self.runtime,
            async move { api.server_read(&["integrations", &path, "tools"], &[], READ_SECONDS).await },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                this.loading_tools.remove(&id);
                if epoch != this.tools_epoch {
                    // Descarta o resultado obsoleto sem perder eventual releitura pedida após Atualizar.
                    if this.pending_tools_reload.remove(&id)
                        && this
                            .items
                            .as_ref()
                            .is_some_and(|res| res.as_ref().is_ok_and(|list| list.iter().any(|it| it.id == id)))
                    {
                        this.load_tools(&id, window, cx);
                    }
                    return;
                }
                if this.pending_tools_reload.remove(&id) {
                    this.load_tools(&id, window, cx);
                    return;
                }
                if seq != this.current_tools_seq(&id) {
                    return;
                }
                match result {
                    Ok(value) => match serde_json::from_value::<ToolsResponse>(value) {
                        Ok(parsed) => {
                            this.tools_errors.remove(&id);
                            this.tools.insert(id, parsed.tools);
                        }
                        Err(_) => {
                            this.tools_errors.insert(id, tr("invalid_response"));
                        }
                    },
                    Err(error) => {
                        this.tools_errors.insert(id, integrations_failure(&error));
                    }
                }
            },
        );
    }

    fn confirm_remove(&mut self, item: &IntegrationItem, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let (id, name) = (item.id.clone(), item.name.clone());
        let title = tr_shared("integrations_remove_confirm", &[("name", &name)]);
        let remove_label = tr_shared("integrations_remove", &[]);
        chrome::confirm_alert(
            window,
            cx,
            title,
            String::new(),
            remove_label,
            ButtonVariant::Danger,
            move |window, cx| {
                let id = id.clone();
                let _ = page.update(cx, |this, cx| this.remove(id, window, cx));
                true
            },
        );
    }

    fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy.insert(id.clone()) {
            return;
        }
        self.notice = None;
        self.feedback.remove(&id);
        let (api, path) = (self.api.clone(), id.clone());
        run(
            &self.runtime,
            async move {
                api.server_send(reqwest::Method::DELETE, &["integrations", &path], None, MUTATION_SECONDS)
                    .await
            },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                this.busy.remove(&id);
                match result {
                    Ok(_) => {
                        this.seq += 1;
                        this.open_tools.remove(&id);
                        this.tools.remove(&id);
                        this.tools_errors.remove(&id);
                        this.pending_tools_reload.remove(&id);
                        this.bump_tools_seq(&id);
                        if this.handoff.as_ref().is_some_and(|h| h.integration_id == id) {
                            this.handoff = None;
                        }
                        if let Some(Ok(list)) = this.items.as_mut() {
                            list.retain(|item| item.id != id);
                        } else if this.items.is_none() {
                            // Se Atualizar estava em voo e foi invalidado por `this.seq += 1`, dispara leitura substituta.
                            this.reload_items(true, window, cx);
                        }
                        this.feedback.remove(&id);
                        this.notice = Some(tr_shared("integrations_removed", &[]));
                    }
                    Err(error) => {
                        this.feedback.insert(id, Feedback::Error(integrations_failure(&error)));
                    }
                }
            },
        );
        cx.notify();
    }

    fn mark_section(&self, el: Div, key: &'static str, cx: &App) -> Div {
        match self.hangar.upgrade() {
            Some(hangar) => hangar.read(cx).mark(el, key),
            None => el,
        }
    }

    fn render_configured(&self, items: &[IntegrationItem], cx: &mut Context<Self>) -> AnyElement {
        let head = self.mark_section(
            section_head(
                IconName::Plug,
                tr_shared("integrations_configured_heading", &[]),
                None,
                None,
                px(16.),
            ),
            "integrations_search_row_configured",
            cx,
        );
        let body = if items.is_empty() {
            settings_box()
                .px_4()
                .py(px(18.))
                .child(muted(tr_shared("integrations_empty", &[])))
                .into_any_element()
        } else {
            settings_box()
                .id("integrations-configured")
                .role(Role::List)
                .aria_label(tr_shared("integrations_configured_heading", &[]))
                .children(items.iter().flat_map(|item| {
                    let open = self.open_tools.contains(&item.id);
                    std::iter::once(self.render_item(item, open, cx))
                        .chain(open.then(|| self.render_tools(item, cx)))
                }))
                .into_any_element()
        };
        div().flex().flex_col().gap_2().child(head).child(body).into_any_element()
    }

    fn render_item(&self, item: &IntegrationItem, tools_open: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id.clone();
        let busy = self.busy.contains(&id);
        let testing = self.testing.contains(&id);
        let locked = busy || testing;
        let (status_text, status_fg, status_bg) = status_presentation(item.status);
        let transport_summary = match &item.transport {
            Transport::RemoteHttp { url, .. } => url.clone(),
            Transport::LocalStdio { command, .. } => command.join(" "),
        };
        let is_oauth = matches!(
            item.auth,
            AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
        );
        let needs_auth = is_oauth
            && matches!(
                item.status,
                IntegrationStatus::AuthRequired | IntegrationStatus::AuthExpired | IntegrationStatus::Error
            );
        let is_google_expiring = item
            .preset
            .as_deref()
            .is_some_and(|p| p.starts_with("google-"))
            && matches!(
                item.status,
                IntegrationStatus::AuthRequired | IntegrationStatus::AuthExpired
            );

        let (projects_badge, projects_tooltip) = if item.projects.is_empty() {
            (
                tr_shared("integrations_projects_all", &[]),
                tr_shared("integrations_field_projects_hint", &[]),
            )
        } else {
            (
                tr_shared(
                    "integrations_projects_count",
                    &[("n", &item.projects.len().to_string())],
                ),
                item.projects.join("\n"),
            )
        };
        let projects_item = item.clone();
        let projects_preset = self.preset_for(item.preset.as_deref());
        let projects_btn = Button::new(SharedString::from(format!("integrations-projects-{id}")))
            .ghost()
            .xsmall()
            .label(projects_badge)
            .tooltip(projects_tooltip)
            .disabled(locked)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_form(
                    FormMode::Edit(projects_item.clone(), projects_preset.clone()),
                    window,
                    cx,
                );
            }));

        let page = cx.entity().downgrade();
        let id_policy = id.clone();
        let current_policy = item.policy;
        let policy_btn = Button::new(SharedString::from(format!("integrations-policy-{id}")))
            .outline()
            .xsmall()
            .disabled(locked)
            .label(policy_label(current_policy, true))
            .tooltip(policy_label(current_policy, false))
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = sidebar::menu_style(menu);
                for policy in [
                    PermissionPolicy::ReadFreeAskWrite,
                    PermissionPolicy::AllowAll,
                    PermissionPolicy::ReadOnly,
                ] {
                    let page = page.clone();
                    let id_policy = id_policy.clone();
                    menu = menu.item(
                        PopupMenuItem::new(policy_label(policy, false))
                            .checked(current_policy == policy)
                            .on_click(move |_, window, cx| {
                                let _ = page.update(cx, |this, cx| {
                                    this.set_policy(id_policy.clone(), policy, window, cx);
                                });
                            }),
                    );
                }
                menu
            });

        let id_pin = id.clone();
        let pinned = item.pinned;
        let pin_check = Checkbox::new(SharedString::from(format!("integrations-pin-{id}")))
            .small()
            .label(tr_shared("integrations_field_pinned", &[]))
            .checked(pinned)
            .disabled(locked)
            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                this.set_pinned(id_pin.clone(), *on, window, cx);
            }));

        let id_enable = id.clone();
        let enable_switch = Switch::new(SharedString::from(format!("integrations-enable-{id}")))
            .checked(item.enabled)
            .disabled(locked)
            .accessibility_label(format!("{} · {}", tr_shared("integrations_enabled", &[]), item.name))
            .tooltip(tr_shared("integrations_enabled", &[]))
            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                this.set_enabled(id_enable.clone(), *on, window, cx);
            }));

        let id_oauth = id.clone();
        let oauth_label = tr_shared(
            if item.status == IntegrationStatus::AuthRequired {
                "integrations_authorize"
            } else {
                "integrations_reconnect"
            },
            &[],
        );
        let feedback = self.feedback.get(&id).cloned();
        // O erro da ação que a pessoa acabou de fazer explica mais que o `last_error` guardado: um aviso vermelho por vez.
        let shown_error = match &feedback {
            Some(Feedback::Error(err)) => Some(err.clone()),
            _ => item.last_error.clone().filter(|err| !err.is_empty() && item.status != IntegrationStatus::Ready),
        };
        let info = match feedback {
            Some(Feedback::Info(text)) => Some(text),
            _ => None,
        };
        let oauth_btn = is_oauth.then(|| {
            let btn = Button::new(SharedString::from(format!("integrations-oauth-{id}")))
                .xsmall()
                .disabled(locked || !item.enabled)
                .label(oauth_label);
            let btn = if needs_auth { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, window, cx| {
                this.start_oauth(id_oauth.clone(), window, cx);
            }))
        });

        let id_test = id.clone();
        let test_btn = Button::new(SharedString::from(format!("integrations-test-{id}")))
            .outline()
            .xsmall()
            .loading(testing)
            .disabled(locked || !item.enabled)
            .label(tr_shared(
                if testing { "integrations_testing" } else { "integrations_test" },
                &[],
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.test_connection(id_test.clone(), window, cx);
            }));

        let id_tools = id.clone();
        let tools_btn = Button::new(SharedString::from(format!("integrations-tools-btn-{id}")))
            .ghost()
            .xsmall()
            .icon(if tools_open { IconName::ChevronUp } else { IconName::ChevronDown })
            .label(tr_shared("integrations_tools_heading", &[]))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_tools(id_tools.clone(), window, cx);
            }));

        let edit_item = item.clone();
        let edit_preset = self.preset_for(item.preset.as_deref());
        let edit_btn = Button::new(SharedString::from(format!("integrations-edit-{id}")))
            .ghost()
            .xsmall()
            .icon(IconName::Pencil)
            .tooltip(tr_shared("integrations_edit", &[]))
            .accessibility_label(format!("{} · {}", tr_shared("integrations_edit", &[]), item.name))
            .disabled(locked)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_form(
                    FormMode::Edit(edit_item.clone(), edit_preset.clone()),
                    window,
                    cx,
                );
            }));

        let remove_item = item.clone();
        let remove_btn = Button::new(SharedString::from(format!("integrations-remove-{id}")))
            .ghost()
            .xsmall()
            .icon(IconName::Trash)
            .tooltip(tr_shared("integrations_remove", &[]))
            .accessibility_label(format!("{} · {}", tr_shared("integrations_remove", &[]), item.name))
            .disabled(locked)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_remove(&remove_item, window, cx);
            }));

        div()
            .id(SharedString::from(format!("integrations-row-{id}")))
            .role(Role::ListItem)
            .aria_label(item.name.clone())
            .mt(px(-1.))
            .border_t_1()
            .border_color(theme::border())
            .px_4()
            .py(px(12.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(div().font_weight(FontWeight::MEDIUM).child(item.name.clone()))
                            .child(
                                div()
                                    .font_family(theme::MONO)
                                    .text_xs()
                                    .text_color(theme::faint())
                                    .child(item.id.clone()),
                            )
                            .child(chip(status_text, status_fg, status_bg)),
                    )
                    .child(enable_switch),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(theme::MONO)
                    .text_color(theme::muted())
                    .truncate()
                    .child(transport_summary),
            )
            .when_some(shown_error, |el, err| {
                el.child(alert(SharedString::from(format!("integrations-item-err-{id}")), err))
            })
            .when_some(info, |el, text| {
                el.child(
                    div()
                        .id(SharedString::from(format!("integrations-item-info-{id}")))
                        .role(Role::Status)
                        .text_sm()
                        .text_color(theme::muted())
                        .whitespace_normal()
                        .child(text),
                )
            })
            .when(is_google_expiring, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme::warning_text())
                        .whitespace_normal()
                        .child(tr_shared("integrations_google_testing_hint", &[])),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(policy_btn)
                    .child(projects_btn)
                    .child(pin_check)
                    .child(div().flex_1())
                    .children(oauth_btn)
                    .child(test_btn)
                    .child(tools_btn)
                    .child(edit_btn)
                    .child(remove_btn),
            )
            .into_any_element()
    }

    fn render_tools(&self, item: &IntegrationItem, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id.clone();
        let loading = self.loading_tools.contains(&id);
        let busy = self.busy.contains(&id);
        let body = if loading && !self.tools.contains_key(&id) {
            muted(tr_shared("integrations_loading", &[])).into_any_element()
        } else if let Some(reason) = self.tools_errors.get(&id) {
            let retry_id = id.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(alert(
                    SharedString::from(format!("integrations-tools-err-{id}")),
                    tr_shared("integrations_tools_load_error", &[("reason", reason)]),
                ))
                .child(
                    div().child(
                        Button::new(SharedString::from(format!("integrations-tools-retry-{id}")))
                            .outline()
                            .xsmall()
                            .disabled(loading || busy)
                            .label(tr_shared("integrations_retry", &[]))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.load_tools(&retry_id, window, cx);
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element()
        } else {
            match self.tools.get(&id) {
                None => muted(tr_shared("integrations_tools_empty", &[])).into_any_element(),
                Some(tools) if tools.is_empty() => {
                    muted(tr_shared("integrations_tools_empty", &[])).into_any_element()
                }
                Some(tools) => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(tools.iter().map(|tool| {
                        let tool_name = tool.name.clone();
                        let is_read = tool.read_only_hint == Some(true);
                        let (kind_text, kind_color) = if is_read {
                            (tr_shared("integrations_tool_read_only", &[]), theme::success_text())
                        } else {
                            (tr_shared("integrations_tool_write", &[]), theme::warning_text())
                        };
                        let (access_text, access_color) = tool_access_label(tool.effective_access);
                        let allowed = item.allowed_tools.contains(&tool_name);
                        let id_allow = id.clone();
                        let tool_allow = tool_name.clone();
                        div()
                            .p_2()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::boxed())
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_family(theme::MONO)
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(tool_name.clone()),
                                    )
                                    .child(chip(kind_text, kind_color, theme::inset()))
                                    .child(chip(access_text, access_color, theme::inset()))
                                    .child(div().flex_1())
                                    .when(!is_read, |el| {
                                        el.child(
                                            Switch::new(SharedString::from(format!(
                                                "integrations-tool-allow-{id}-{tool_name}"
                                            )))
                                            .small()
                                            .label(tr_shared("integrations_tool_allow_always", &[]))
                                            .checked(allowed)
                                            .disabled(busy)
                                            .on_click(cx.listener(move |this, on: &bool, window, cx| {
                                                this.toggle_allowed_tool(
                                                    id_allow.clone(),
                                                    tool_allow.clone(),
                                                    *on,
                                                    window,
                                                    cx,
                                                );
                                            })),
                                        )
                                    }),
                            )
                            .when_some(
                                tool.description.clone().filter(|d| !d.trim().is_empty()),
                                |el, desc| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::muted())
                                            .whitespace_normal()
                                            .child(desc),
                                    )
                                },
                            )
                            .into_any_element()
                    }))
                    .into_any_element(),
            }
        };
        div()
            .id(SharedString::from(format!("integrations-tools-{id}")))
            .mt(px(-1.))
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::inset())
            .px_4()
            .py(px(12.))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::muted())
                    .child(tr_shared("integrations_tools_heading", &[])),
            )
            .child(body)
            .into_any_element()
    }

    fn render_catalog(&self, existing: &[IntegrationItem], cx: &mut Context<Self>) -> AnyElement {
        let head = self.mark_section(
            section_head(
                IconName::Globe,
                tr_shared("integrations_catalog_heading", &[]),
                Some(tr_shared("integrations_catalog_hint", &[])),
                None,
                px(16.),
            ),
            "integrations_search_row_catalog",
            cx,
        );
        let body = match &self.catalog {
            None => div()
                .px_4()
                .py(px(14.))
                .child(muted(tr_shared("integrations_loading", &[])))
                .into_any_element(),
            Some(Err(err)) => settings_box()
                .px_4()
                .py(px(14.))
                .child(alert("integrations-catalog-error", err.clone()))
                .into_any_element(),
            Some(Ok(presets)) if presets.is_empty() => settings_box()
                .px_4()
                .py(px(14.))
                .child(muted(tr_shared("integrations_catalog_empty", &[])))
                .into_any_element(),
            Some(Ok(presets)) => settings_box()
                .id("integrations-catalog")
                .role(Role::List)
                .aria_label(tr_shared("integrations_catalog_heading", &[]))
                .children(presets.iter().map(|preset| {
                    let already = existing
                        .iter()
                        .any(|item| item.preset.as_deref() == Some(preset.id.as_str()));
                    let connect_label = tr_shared(
                        if already {
                            "integrations_add_account"
                        } else {
                            "integrations_connect"
                        },
                        &[],
                    );
                    let preset_cloned = preset.clone();
                    div()
                        .id(SharedString::from(format!("integrations-preset-{}", preset.id)))
                        .role(Role::ListItem)
                        .aria_label(preset.name.clone())
                        .mt(px(-1.))
                        .border_t_1()
                        .border_color(theme::border())
                        .px_4()
                        .py(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(preset.name.clone()),
                                        )
                                        .child(chip(
                                            tr_shared("integrations_badge_official", &[]),
                                            theme::accent_text(),
                                            theme::accent_dim(),
                                        ))
                                        .child(chip(
                                            preset_auth_summary(preset.auth_kind),
                                            theme::muted(),
                                            theme::inset(),
                                        )),
                                )
                                .when(!preset.description.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme::muted())
                                            .whitespace_normal()
                                            .child(preset.description.clone()),
                                    )
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "integrations-connect-{}",
                                preset.id
                            )))
                            .primary()
                            .small()
                            .label(connect_label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(FormMode::Preset(preset_cloned.clone()), window, cx);
                            })),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        };
        div().flex().flex_col().gap_2().child(head).child(body).into_any_element()
    }
}

impl InitialFormValues {
    fn from_mode(mode: FormMode, existing: &[IntegrationItem]) -> Self {
        match mode {
            FormMode::Preset(p) => {
                let auth_kind = match p.auth_kind {
                    PresetAuthKind::None => AuthKindChoice::None,
                    PresetAuthKind::Bearer => AuthKindChoice::Bearer,
                    PresetAuthKind::OAuthDcr | PresetAuthKind::OAuthDcrOrBearer => {
                        AuthKindChoice::OAuthDcr
                    }
                    PresetAuthKind::OAuthManualClient => AuthKindChoice::OAuthManualClient,
                };
                Self {
                    is_edit: false,
                    id: suggest_preset_id(&p.id, existing),
                    name: p.name,
                    preset: Some(p.id),
                    url_editable: p.url_editable,
                    preset_auth: Some(p.auth_kind),
                    setup_guide: p.setup_guide,
                    enabled: true,
                    transport: TransportKind::RemoteHttp,
                    url: p.url,
                    existing_headers: Vec::new(),
                    command: String::new(),
                    cwd: String::new(),
                    existing_env_keys: Vec::new(),
                    auth_kind,
                    client_id: String::new(),
                    scopes: p.default_scopes.join(", "),
                    policy: PermissionPolicy::ReadFreeAskWrite,
                    pinned: false,
                    projects: String::new(),
                    original_command: None,
                    original_transport: None,
                    original_policy: None,
                }
            }
            FormMode::Custom => Self {
                is_edit: false,
                id: String::new(),
                name: String::new(),
                preset: None,
                url_editable: true,
                preset_auth: None,
                setup_guide: None,
                enabled: true,
                transport: TransportKind::RemoteHttp,
                url: String::new(),
                existing_headers: Vec::new(),
                command: String::new(),
                cwd: String::new(),
                existing_env_keys: Vec::new(),
                auth_kind: AuthKindChoice::OAuthDcr,
                client_id: String::new(),
                scopes: String::new(),
                policy: PermissionPolicy::ReadFreeAskWrite,
                pinned: false,
                projects: String::new(),
                original_command: None,
                original_transport: None,
                original_policy: None,
            },
            FormMode::Edit(item, preset_meta) => {
                let original_transport = Some(item.transport.clone());
                let (transport, url, existing_headers, command, cwd, existing_env_keys, original_command) =
                    match &item.transport {
                        Transport::RemoteHttp { url, header_names } => (
                            TransportKind::RemoteHttp,
                            url.clone(),
                            header_names.clone(),
                            String::new(),
                            String::new(),
                            Vec::new(),
                            None,
                        ),
                        Transport::LocalStdio {
                            command,
                            cwd,
                            env_keys,
                        } => {
                            let formatted = format_command_for_input(command);
                            (
                                TransportKind::LocalStdio,
                                String::new(),
                                Vec::new(),
                                formatted.clone(),
                                cwd.clone().unwrap_or_default(),
                                env_keys.clone(),
                                Some((formatted, command.clone())),
                            )
                        }
                    };
                let (auth_kind, client_id, scopes) = match &item.auth {
                    AuthConfig::None => (AuthKindChoice::None, String::new(), String::new()),
                    AuthConfig::Bearer => (AuthKindChoice::Bearer, String::new(), String::new()),
                    AuthConfig::OAuthDcr => (AuthKindChoice::OAuthDcr, String::new(), String::new()),
                    AuthConfig::OAuthManualClient { client_id, scopes } => (
                        AuthKindChoice::OAuthManualClient,
                        client_id.clone(),
                        scopes.join(", "),
                    ),
                };
                Self {
                    is_edit: true,
                    id: item.id,
                    name: item.name,
                    preset: item.preset,
                    url_editable: preset_meta.as_ref().is_none_or(|p| p.url_editable),
                    preset_auth: preset_meta.as_ref().map(|p| p.auth_kind),
                    setup_guide: preset_meta.and_then(|p| p.setup_guide),
                    enabled: item.enabled,
                    transport,
                    url,
                    existing_headers,
                    command,
                    cwd,
                    existing_env_keys,
                    auth_kind,
                    client_id,
                    scopes,
                    policy: item.policy,
                    pinned: item.pinned,
                    projects: item.projects.join("\n"),
                    original_command,
                    original_transport,
                    original_policy: Some(item.policy),
                }
            }
        }
    }
}

impl IntegrationForm {
    fn new(
        api: Api,
        runtime: Arc<Runtime>,
        page: WeakEntity<IntegrationsPage>,
        mode: FormMode,
        existing: &[IntegrationItem],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = InitialFormValues::from_mode(mode, existing);
        let input = |val: String, hint: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(val).placeholder(hint))
        };
        let masked_input = |hint: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(hint))
        };
        let area = |val: String, hint: String, rows: usize, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(rows)
                    .default_value(val)
                    .placeholder(hint)
            })
        };

        let secret_hint = if initial.is_edit {
            tr_shared("integrations_secret_kept_hint", &[])
        } else {
            String::new()
        };

        let id_input = input(initial.id, tr_shared("integrations_field_id_hint", &[]), window, cx);
        let name_input = input(
            initial.name,
            tr_shared("integrations_field_name_placeholder", &[]),
            window,
            cx,
        );
        let url_input = input(initial.url, "https://...".into(), window, cx);
        let headers_hint = if initial.existing_headers.is_empty() { String::new() } else { secret_hint.clone() };
        let headers_input = area(String::new(), headers_hint, 2, window, cx);
        let command_input = area(
            initial.command,
            tr_shared("integrations_field_command_hint", &[]),
            3,
            window,
            cx,
        );
        let cwd_input = input(initial.cwd, tr_shared("integrations_optional", &[]), window, cx);
        let env_input = area(
            String::new(),
            if initial.is_edit {
                secret_hint.clone()
            } else {
                tr_shared("integrations_field_env_hint", &[])
            },
            3,
            window,
            cx,
        );
        let client_id_input = input(initial.client_id, String::new(), window, cx);
        let client_secret_input = masked_input(secret_hint.clone(), window, cx);
        let scopes_input = input(initial.scopes, tr_shared("integrations_optional", &[]), window, cx);
        let bearer_input = masked_input(secret_hint, window, cx);
        let projects_input = area(
            initial.projects,
            tr_shared("integrations_field_projects_hint", &[]),
            2,
            window,
            cx,
        );

        Self {
            api,
            runtime,
            page,
            is_edit: initial.is_edit,
            preset: initial.preset,
            url_editable: initial.url_editable,
            preset_auth: initial.preset_auth,
            setup_guide: initial.setup_guide,
            enabled: initial.enabled,
            transport: initial.transport,
            auth_kind: initial.auth_kind,
            policy: initial.policy,
            pinned: initial.pinned,
            id_input,
            name_input,
            url_input,
            existing_headers: initial.existing_headers,
            removed_headers: BTreeSet::new(),
            headers_input,
            command_input,
            cwd_input,
            existing_env_keys: initial.existing_env_keys,
            removed_env_keys: BTreeSet::new(),
            env_input,
            client_id_input,
            client_secret_input,
            scopes_input,
            bearer_input,
            projects_input,
            original_command: initial.original_command,
            original_transport: initial.original_transport,
            original_policy: initial.original_policy,
            saving: false,
            error: None,
        }
    }

    fn initial_focus(&self) -> &Entity<InputState> {
        if self.is_edit {
            &self.name_input
        } else {
            &self.id_input
        }
    }

    fn draft(&self, cx: &App) -> FormDraft {
        FormDraft {
            is_edit: self.is_edit,
            id: self.id_input.read(cx).value().to_string(),
            name: self.name_input.read(cx).value().to_string(),
            preset: self.preset.clone(),
            enabled: self.enabled,
            transport: self.transport,
            url: self.url_input.read(cx).value().to_string(),
            existing_headers: self.existing_headers.clone(),
            removed_headers: self.removed_headers.clone(),
            new_headers_raw: self.headers_input.read(cx).value().to_string(),
            command_raw: self.command_input.read(cx).value().to_string(),
            cwd: self.cwd_input.read(cx).value().to_string(),
            existing_env_keys: self.existing_env_keys.clone(),
            removed_env_keys: self.removed_env_keys.clone(),
            new_env_raw: self.env_input.read(cx).value().to_string(),
            auth_kind: self.effective_auth(),
            client_id: self.client_id_input.read(cx).value().to_string(),
            client_secret: self.client_secret_input.read(cx).value().to_string(),
            scopes_raw: self.scopes_input.read(cx).value().to_string(),
            bearer_token: self.bearer_input.read(cx).value().to_string(),
            policy: self.policy,
            pinned: self.pinned,
            projects_raw: self.projects_input.read(cx).value().to_string(),
            original_command: self.original_command.clone(),
            original_transport: self.original_transport.clone(),
            original_policy: self.original_policy,
        }
    }

    fn clear_secrets(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.bearer_input.update(cx, |input, cx| input.set_value("", window, cx));
        self.client_secret_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.headers_input.update(cx, |input, cx| input.set_value("", window, cx));
        self.env_input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let draft = self.draft(cx);
        let payload = if self.is_edit {
            build_patch_body(&draft)
        } else {
            build_create_body(&draft)
        };
        let body = match payload {
            Ok(body) => body,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };

        (self.saving, self.error) = (true, None);
        let (api, is_edit, id) = (self.api.clone(), self.is_edit, draft.id.trim().to_owned());
        run(
            &self.runtime,
            async move {
                if is_edit {
                    api.server_send(reqwest::Method::PATCH, &["integrations", &id], Some(body), MUTATION_SECONDS)
                        .await
                } else {
                    api.server_send(reqwest::Method::POST, &["integrations"], Some(body), MUTATION_SECONDS)
                        .await
                }
            },
            window,
            cx,
            move |this: &mut Self, result, window, cx| {
                this.saving = false;
                match result {
                    Ok(value) => {
                        this.clear_secrets(window, cx);
                        if let Some(item) = value
                            .get("integration")
                            .cloned()
                            .and_then(|v| serde_json::from_value::<IntegrationItem>(v).ok())
                        {
                            let created = !this.is_edit;
                            let _ = this.page.update(cx, |page, cx| {
                                page.saved(created, item, window, cx);
                            });
                        }
                        window.close_dialog(cx);
                    }
                    Err(error) => this.error = Some(integrations_failure(&error)),
                }
            },
        );
        cx.notify();
    }

    /// Autenticação salva que o preset não oferece mais (GitHub antigo em OAuth) vira a primeira opção dele.
    fn effective_auth(&self) -> AuthKindChoice {
        let options = self.auth_options();
        if options.is_empty() || options.contains(&self.auth_kind) { self.auth_kind } else { options[0] }
    }

    fn auth_options(&self) -> Vec<AuthKindChoice> {
        if self.transport == TransportKind::LocalStdio {
            return Vec::new();
        }
        match self.preset_auth {
            // A URL trocada de um preset público pode apontar para um endpoint com login.
            Some(PresetAuthKind::None) if self.url_editable => vec![AuthKindChoice::None, AuthKindChoice::OAuthDcr],
            Some(PresetAuthKind::None) => vec![AuthKindChoice::None],
            Some(PresetAuthKind::Bearer) => vec![AuthKindChoice::Bearer],
            Some(PresetAuthKind::OAuthDcr) => vec![AuthKindChoice::OAuthDcr],
            Some(PresetAuthKind::OAuthManualClient) => vec![AuthKindChoice::OAuthManualClient],
            Some(PresetAuthKind::OAuthDcrOrBearer) => {
                vec![AuthKindChoice::OAuthDcr, AuthKindChoice::Bearer]
            }
            None => vec![
                AuthKindChoice::OAuthDcr,
                AuthKindChoice::OAuthManualClient,
                AuthKindChoice::Bearer,
                AuthKindChoice::None,
            ],
        }
    }
}

impl Hangar {
    /// A página é refeita a cada abertura e a cada conexão: nenhum rascunho ou dado de formulário sobrevive à saída.
    pub(super) fn render_integrations(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr_shared("integrations_title", &[])),
                )
                .child(div().mt(px(6.)).text_color(theme::muted()).child(tr("settings_offline")))
                .into_any_element();
        };
        if self
            .integrations_page
            .as_ref()
            .is_none_or(|(connection, _)| *connection != self.connection)
        {
            let (runtime, hangar) = (self.runtime.clone(), cx.entity().downgrade());
            let page = cx.new(|cx| IntegrationsPage::new(api, runtime, hangar, window, cx));
            self.integrations_page = Some((self.connection, page));
        }
        self.integrations_page
            .as_ref()
            .map(|(_, page)| page.clone().into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }
}

impl Render for IntegrationsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let custom_btn = self.mark_section(
            div().child(
                Button::new("integrations-add-custom")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label(tr_shared("integrations_add_custom", &[]))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(FormMode::Custom, window, cx);
                    })),
            ),
            "integrations_search_row_custom",
            cx,
        );
        let refresh_btn = Button::new("integrations-refresh")
            .outline()
            .small()
            .icon(IconName::RefreshCw)
            .label(tr_shared("integrations_refresh", &[]))
            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx)));

        let header = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr_shared("integrations_title", &[])),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .max_w(px(super::settings::READING_WIDTH))
                            .text_color(theme::muted())
                            .whitespace_normal()
                            .child(tr_shared("integrations_subtitle", &[])),
                    ),
            )
            .child(div().flex().items_center().gap_2().child(refresh_btn).child(custom_btn));

        let mut page = div().flex().flex_col().gap_4().child(header);
        if let Some(notice) = self.notice.clone() {
            page = page.child(
                div()
                    .id("integrations-notice")
                    .role(Role::Status)
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::inset())
                    .text_sm()
                    .text_color(theme::text())
                    .whitespace_normal()
                    .child(notice),
            );
        }
        if let Some(handoff) = &self.handoff {
            let reason = handoff.reason.clone();
            page = page.child(
                settings_box()
                    .id("integrations-oauth-handoff")
                    .px_4()
                    .py(px(12.))
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::warning_text())
                            .child(tr_shared("integrations_oauth_handoff_title", &[])),
                    )
                    .when(!reason.is_empty(), |el| el.child(muted(reason)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("integrations-handoff-open")
                                    .primary()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label(tr_shared("integrations_oauth_open_desktop", &[]))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_handoff_on_desktop(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("integrations-handoff-close")
                                    .ghost()
                                    .small()
                                    .label(tr_shared("integrations_close", &[]))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.handoff = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }

        let items = match &self.items {
            None => {
                return page.child(
                    div()
                        .id("integrations-loading")
                        .role(Role::Status)
                        .child(muted(tr_shared("integrations_loading", &[]))),
                );
            }
            Some(Err(err)) => {
                let err_text = if err.is_empty() {
                    tr_shared("integrations_load_error", &[])
                } else {
                    err.clone()
                };
                return page
                    .child(alert("integrations-load-error", err_text))
                    .child(
                        div().child(
                            Button::new("integrations-retry")
                                .outline()
                                .small()
                                .label(tr_shared("integrations_retry", &[]))
                                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                        ),
                    );
            }
            Some(Ok(items)) => items,
        };

        page.child(self.render_configured(items, cx))
            .child(self.render_catalog(items, cx))
    }
}

impl Render for IntegrationForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let off = self.saving;
        let field = |label: String, hint: Option<String>, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(div().font_weight(FontWeight::MEDIUM).child(label))
                .child(control)
                .when_some(hint, |el, h| el.child(muted(h).text_size(px(12.))))
        };

        let mut body = div().flex().flex_col().gap(px(12.)).text_size(px(13.));

        if let Some(guide) = &self.setup_guide {
            let docs_url = guide.docs_url.clone().and_then(|url| safe_authorize_url(&url));
            body = body.child(
                div()
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::inset())
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::accent_text())
                            .child(format!(
                                "{} · {}",
                                tr_shared("integrations_setup_guide_title", &[]),
                                guide.title
                            )),
                    )
                    .children(guide.steps.iter().enumerate().map(|(ix, step)| {
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .whitespace_normal()
                            .child(format!("{}. {}", ix + 1, step))
                    }))
                    .when_some(guide.docs_hint.clone(), |el, hint| {
                        el.child(
                            div()
                                .font_family(theme::MONO)
                                .text_xs()
                                .text_color(theme::faint())
                                .whitespace_normal()
                                .child(hint),
                        )
                    })
                    .when_some(guide.redirect_uri.clone(), |el, uri| {
                        let copy = uri.clone();
                        el.child(
                            div()
                                .mt(px(4.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(muted(tr_shared("integrations_setup_redirect", &[])).text_xs())
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_2()
                                                .py_1()
                                                .rounded(px(6.))
                                                .bg(theme::boxed())
                                                .font_family(theme::MONO)
                                                .text_xs()
                                                .whitespace_normal()
                                                .child(uri),
                                        )
                                        .child(
                                            Button::new("integrations-copy-redirect")
                                                .outline()
                                                .xsmall()
                                                .icon(IconName::Copy)
                                                .label(tr_shared("integrations_copy", &[]))
                                                .on_click(move |_, _, cx| {
                                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                                }),
                                        ),
                                ),
                        )
                    })
                    .when_some(docs_url, |el, url| {
                        el.child(
                            div().mt(px(4.)).child(
                                Button::new("integrations-open-docs")
                                    .primary()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label(tr_shared("integrations_setup_open_docs", &[]))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                        )
                    }),
            );
        }

        body = body
            .child(field(
                tr_shared("integrations_field_id", &[]),
                Some(tr_shared("integrations_field_id_hint", &[])),
                Input::new(&self.id_input)
                    .small()
                    .disabled(off || self.is_edit)
                    .aria_label(tr_shared("integrations_field_id", &[]))
                    .into_any_element(),
            ))
            .child(field(
                tr_shared("integrations_field_name", &[]),
                None,
                Input::new(&self.name_input)
                    .small()
                    .disabled(off)
                    .aria_label(tr_shared("integrations_field_name", &[]))
                    .into_any_element(),
            ));

        if self.preset.is_none() {
            let transport_row = div()
                .flex()
                .gap_2()
                .children([
                    (TransportKind::RemoteHttp, "integrations_transport_remote"),
                    (TransportKind::LocalStdio, "integrations_transport_local"),
                ].map(|(kind, key)| {
                    Button::new(key)
                        .outline()
                        .small()
                        .flex_1()
                        .selected(self.transport == kind)
                        .disabled(off)
                        .label(tr_shared(key, &[]))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.transport = kind;
                            if kind == TransportKind::LocalStdio {
                                this.auth_kind = AuthKindChoice::None;
                            } else if this.auth_kind == AuthKindChoice::None && !this.is_edit {
                                this.auth_kind = AuthKindChoice::OAuthDcr;
                            }
                            cx.notify();
                        }))
                }));
            body = body.child(field(
                tr_shared("integrations_field_transport", &[]),
                None,
                transport_row.into_any_element(),
            ));
        }

        match self.transport {
            TransportKind::RemoteHttp => {
                body = body.child(field(
                    tr_shared("integrations_field_url", &[]),
                    None,
                    Input::new(&self.url_input)
                        .small()
                        .disabled(off || !self.url_editable)
                        .aria_label(tr_shared("integrations_field_url", &[]))
                        .into_any_element(),
                ));

                let auth_options = self.auth_options();
                if auth_options.len() > 1 {
                    let radios = div().flex().flex_wrap().gap_2().children(auth_options.into_iter().map(
                        |choice| {
                            let id = SharedString::from(format!("integrations-form-auth-{choice:?}"));
                            Radio::new(id)
                                .label(auth_choice_label(choice))
                                .checked(self.effective_auth() == choice)
                                .disabled(off)
                                .on_change(cx.listener(move |this, _, _, cx| {
                                    this.auth_kind = choice;
                                    cx.notify();
                                }))
                        },
                    ));
                    body = body.child(field(
                        tr_shared("integrations_field_auth", &[]),
                        None,
                        radios.into_any_element(),
                    ));
                }

                match self.effective_auth() {
                    AuthKindChoice::OAuthManualClient => {
                        body = body
                            .child(field(
                                tr_shared("integrations_field_client_id", &[]),
                                None,
                                Input::new(&self.client_id_input)
                                    .small()
                                    .disabled(off)
                                    .aria_label(tr_shared("integrations_field_client_id", &[]))
                                    .into_any_element(),
                            ))
                            .child(field(
                                tr_shared("integrations_field_client_secret", &[]),
                                self.is_edit
                                    .then(|| tr_shared("integrations_secret_kept_hint", &[])),
                                Input::new(&self.client_secret_input)
                                    .small()
                                    .mask_toggle()
                                    .disabled(off)
                                    .aria_label(tr_shared("integrations_field_client_secret", &[]))
                                    .into_any_element(),
                            ))
                            .child(field(
                                format!(
                                    "{} ({})",
                                    tr_shared("integrations_field_scopes", &[]),
                                    tr_shared("integrations_optional", &[])
                                ),
                                None,
                                Input::new(&self.scopes_input)
                                    .small()
                                    .disabled(off)
                                    .aria_label(tr_shared("integrations_field_scopes", &[]))
                                    .into_any_element(),
                            ));
                    }
                    AuthKindChoice::Bearer => {
                        body = body.child(field(
                            tr_shared("integrations_field_bearer", &[]),
                            self.is_edit
                                .then(|| tr_shared("integrations_secret_kept_hint", &[])),
                            Input::new(&self.bearer_input)
                                .small()
                                .mask_toggle()
                                .disabled(off)
                                .aria_label(tr_shared("integrations_field_bearer", &[]))
                                .into_any_element(),
                        ));
                    }
                    AuthKindChoice::OAuthDcr | AuthKindChoice::None => {}
                }

                let existing_headers: Vec<String> = self
                    .existing_headers
                    .iter()
                    .filter(|h| !self.removed_headers.contains(*h))
                    .cloned()
                    .collect();
                let headers_hint = (!existing_headers.is_empty()).then(|| {
                    format!(
                        "{} ({})",
                        tr_shared("integrations_secret_kept_hint", &[]),
                        existing_headers.join(", ")
                    )
                });
                body = body.child(field(
                    format!(
                        "{} ({})",
                        tr_shared("integrations_field_headers", &[]),
                        tr_shared("integrations_optional", &[])
                    ),
                    headers_hint,
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(!existing_headers.is_empty(), |el| {
                            el.child(div().flex().flex_wrap().gap_1().children(existing_headers.into_iter().map(
                                |name| {
                                    let rm_name = name.clone();
                                    Button::new(SharedString::from(format!("integrations-rm-hdr-{name}")))
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::Trash)
                                        .label(name)
                                        .disabled(off)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.removed_headers.insert(rm_name.clone());
                                            cx.notify();
                                        }))
                                },
                            )))
                        })
                        .child(
                            Textarea::new(&self.headers_input)
                                .disabled(off)
                                .aria_label(tr_shared("integrations_field_headers", &[])),
                        )
                        .into_any_element(),
                ));
            }
            TransportKind::LocalStdio => {
                let existing_env: Vec<String> = self
                    .existing_env_keys
                    .iter()
                    .filter(|k| !self.removed_env_keys.contains(*k))
                    .cloned()
                    .collect();
                let env_hint = if !existing_env.is_empty() {
                    Some(format!(
                        "{} ({})",
                        tr_shared("integrations_secret_kept_hint", &[]),
                        existing_env.join(", ")
                    ))
                } else {
                    Some(tr_shared("integrations_field_env_hint", &[]))
                };
                body = body
                    .child(field(
                        tr_shared("integrations_field_command", &[]),
                        Some(tr_shared("integrations_field_command_hint", &[])),
                        Textarea::new(&self.command_input)
                            .disabled(off)
                            .aria_label(tr_shared("integrations_field_command", &[]))
                            .into_any_element(),
                    ))
                    .child(field(
                        format!(
                            "{} ({})",
                            tr_shared("integrations_field_cwd", &[]),
                            tr_shared("integrations_optional", &[])
                        ),
                        None,
                        Input::new(&self.cwd_input)
                            .small()
                            .disabled(off)
                            .aria_label(tr_shared("integrations_field_cwd", &[]))
                            .into_any_element(),
                    ))
                    .child(field(
                        format!(
                            "{} ({})",
                            tr_shared("integrations_field_env", &[]),
                            tr_shared("integrations_optional", &[])
                        ),
                        env_hint,
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(!existing_env.is_empty(), |el| {
                                el.child(div().flex().flex_wrap().gap_1().children(existing_env.into_iter().map(
                                    |key| {
                                        let rm_key = key.clone();
                                        Button::new(SharedString::from(format!("integrations-rm-env-{key}")))
                                            .outline()
                                            .xsmall()
                                            .icon(IconName::Trash)
                                            .label(key)
                                            .disabled(off)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.removed_env_keys.insert(rm_key.clone());
                                                cx.notify();
                                            }))
                                    },
                                )))
                            })
                            .child(
                                Textarea::new(&self.env_input)
                                    .disabled(off)
                                    .aria_label(tr_shared("integrations_field_env", &[])),
                            )
                            .into_any_element(),
                    ));
            }
        }

        let policy_radios = div().flex().flex_col().gap_1().children([
            PermissionPolicy::ReadFreeAskWrite,
            PermissionPolicy::AllowAll,
            PermissionPolicy::ReadOnly,
        ].map(|pol| {
            let id = SharedString::from(format!("integrations-form-policy-{pol:?}"));
            Radio::new(id)
                .label(policy_label(pol, false))
                .checked(self.policy == pol)
                .disabled(off)
                .on_change(cx.listener(move |this, _, _, cx| {
                    this.policy = pol;
                    cx.notify();
                }))
        }));

        body = body
            .child(field(
                tr_shared("integrations_field_policy", &[]),
                None,
                policy_radios.into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        Checkbox::new("integrations-form-pinned")
                            .small()
                            .label(tr_shared("integrations_field_pinned", &[]))
                            .checked(self.pinned)
                            .disabled(off)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.pinned = *on;
                                cx.notify();
                            })),
                    )
                    .child(muted(tr_shared("integrations_field_pinned_hint", &[])).text_size(px(12.))),
            )
            .child(field(
                format!(
                    "{} ({})",
                    tr_shared("integrations_field_projects", &[]),
                    tr_shared("integrations_optional", &[])
                ),
                Some(tr_shared("integrations_field_projects_hint", &[])),
                Textarea::new(&self.projects_input)
                    .disabled(off)
                    .aria_label(tr_shared("integrations_field_projects", &[]))
                    .into_any_element(),
            ));

        div()
            .id("integrations-form")
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .id("integrations-form-scroll")
                    .max_h(px(520.))
                    .overflow_y_scroll()
                    .pr_1()
                    .child(body),
            )
            .children(
                self.error
                    .clone()
                    .map(|err| alert("integrations-form-error", err)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(10.))
                    .child(
                        Button::new("integrations-form-cancel")
                            .ghost()
                            .small()
                            .label(tr_shared("integrations_cancel", &[]))
                            .disabled(off)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("integrations-form-save")
                            .primary()
                            .small()
                            .label(tr_shared(
                                if off { "integrations_saving" } else { "integrations_save" },
                                &[],
                            ))
                            .loading(off)
                            .disabled(off)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// Traduz códigos de erro HTTP das rotas `/api/integrations/*`.
pub(super) fn integrations_failure(error: &Failure) -> String {
    match error.code.as_deref() {
        Some("auth_required") => tr_shared("integrations_err_auth_required", &[]),
        Some("auth_expired") => tr_shared("integrations_err_auth_expired", &[]),
        Some("integration_owner_only") => tr_shared("integrations_err_owner_only", &[]),
        Some("integration_disabled") => tr_shared("integrations_err_disabled", &[]),
        Some("integration_already_exists") => tr_shared("integrations_err_already_exists", &[]),
        Some("integration_secrets_key_lost" | "integration_secrets_unavailable") => {
            tr_shared("integrations_err_keyring", &[])
        }
        Some("approval_not_found") => tr_shared("integrations_approval_lost", &[]),
        Some("approval_conflict") => {
            let lower = error.detail.to_ascii_lowercase();
            if lower.contains("atualizado") || lower.contains("stale") {
                tr_shared("integrations_err_stale", &[])
            } else {
                tr_shared("integrations_err_answered", &[])
            }
        }
        _ => Hangar::fetch_failure(error),
    }
}

pub(super) fn status_presentation(status: IntegrationStatus) -> (String, Hsla, Hsla) {
    match status {
        IntegrationStatus::Ready => (
            tr_shared("integrations_status_ready", &[]),
            theme::success_text(),
            theme::success().opacity(0.14),
        ),
        IntegrationStatus::AuthRequired => (
            tr_shared("integrations_status_auth_required", &[]),
            theme::warning_text(),
            theme::warning().opacity(0.14),
        ),
        IntegrationStatus::AuthExpired => (
            tr_shared("integrations_status_auth_expired", &[]),
            theme::warning_text(),
            theme::warning().opacity(0.14),
        ),
        IntegrationStatus::Disabled => (
            tr_shared("integrations_status_disabled", &[]),
            theme::muted(),
            theme::inset(),
        ),
        IntegrationStatus::Error => (
            tr_shared("integrations_status_error", &[]),
            theme::danger_text(),
            theme::danger().opacity(0.14),
        ),
    }
}

pub(super) fn policy_label(policy: PermissionPolicy, short: bool) -> String {
    match policy {
        PermissionPolicy::ReadFreeAskWrite => tr_shared(
            if short {
                "integrations_policy_read_free_ask_write_short"
            } else {
                "integrations_policy_read_free_ask_write"
            },
            &[],
        ),
        PermissionPolicy::AllowAll => tr_shared("integrations_policy_allow_all", &[]),
        PermissionPolicy::ReadOnly => tr_shared("integrations_policy_read_only", &[]),
    }
}

pub(super) fn tool_access_label(access: ToolAccess) -> (String, Hsla) {
    match access {
        ToolAccess::Read => (tr_shared("integrations_tool_access_read", &[]), theme::success_text()),
        ToolAccess::AllowedAlways => (
            tr_shared("integrations_tool_access_allowed_always", &[]),
            theme::accent_text(),
        ),
        ToolAccess::AskWrite => (
            tr_shared("integrations_tool_access_ask_write", &[]),
            theme::warning_text(),
        ),
        ToolAccess::Blocked => (tr_shared("integrations_tool_access_blocked", &[]), theme::danger_text()),
    }
}

pub(super) fn auth_choice_label(choice: AuthKindChoice) -> String {
    match choice {
        AuthKindChoice::OAuthDcr => tr_shared("integrations_auth_oauth_dcr", &[]),
        AuthKindChoice::OAuthManualClient => tr_shared("integrations_auth_oauth_manual", &[]),
        AuthKindChoice::Bearer => tr_shared("integrations_auth_bearer", &[]),
        AuthKindChoice::None => tr_shared("integrations_auth_none", &[]),
    }
}

pub(super) fn preset_auth_summary(kind: PresetAuthKind) -> String {
    match kind {
        PresetAuthKind::None => tr_shared("integrations_auth_none", &[]),
        PresetAuthKind::Bearer => tr_shared("integrations_auth_bearer", &[]),
        PresetAuthKind::OAuthDcr => tr_shared("integrations_auth_oauth_dcr", &[]),
        PresetAuthKind::OAuthManualClient => tr_shared("integrations_auth_oauth_manual", &[]),
        PresetAuthKind::OAuthDcrOrBearer => format!(
            "{} · {}",
            tr_shared("integrations_auth_oauth_dcr", &[]),
            tr_shared("integrations_auth_bearer", &[])
        ),
    }
}

pub(super) fn suggest_preset_id(preset_id: &str, existing: &[IntegrationItem]) -> String {
    let taken = |candidate: &str| existing.iter().any(|item| item.id == candidate);
    if !taken(preset_id) {
        return preset_id.to_owned();
    }
    for suffix in 2..=99 {
        let candidate = format!("{preset_id}-{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{preset_id}-new")
}

fn should_auto_start_oauth(created: bool, item: &IntegrationItem) -> bool {
    created
        && item.enabled
        && matches!(item.transport, Transport::RemoteHttp { .. })
        && matches!(
            item.auth,
            AuthConfig::OAuthDcr | AuthConfig::OAuthManualClient { .. }
        )
}

pub(super) fn validate_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    !value.is_empty()
        && value.len() <= MAX_SLUG_BYTES
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn validate_http_url(url: &str) -> bool {
    if url.is_empty()
        || url.len() > MAX_URL_BYTES
        || url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((host, tail)) = after_open.split_once(']') else {
            return false;
        };
        if host.is_empty()
            || !host.chars().all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.')
        {
            return false;
        }
        return valid_optional_port(tail);
    }
    match authority.split_once(':') {
        Some((host, port)) => valid_hostname(host) && valid_port(port),
        None => valid_hostname(authority),
    }
}

pub(super) fn safe_authorize_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    validate_http_url(trimmed)
        .then(|| url::Url::parse(trimmed).ok())
        .flatten()
        .filter(|parsed| matches!(parsed.scheme(), "http" | "https"))
        .map(|parsed| parsed.to_string())
}

pub(super) fn oauth_start_body(api: &Api) -> Value {
    if api.is_loopback() {
        return json!({ "origin_client": "desktop" });
    }
    let route = api.route();
    let public_base_url = (route.scheme() == "https"
        && route.has_host()
        && route.username().is_empty()
        && route.password().is_none())
    .then(|| format!("{}{}", route.origin().ascii_serialization(), route.path().trim_end_matches('/')));
    match public_base_url {
        Some(url) => json!({ "origin_client": "remote_desktop", "public_base_url": url }),
        None => json!({ "origin_client": "remote_desktop" }),
    }
}

/// Divide o comando local em argumentos: várias linhas = 1 argumento por linha; linha única = espaço com aspas simples/duplas.
pub(super) fn parse_command_args(raw: &str) -> Result<Vec<String>, String> {
    let lines: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.len() > 1 {
        return Ok(lines);
    }
    let Some(single) = lines.into_iter().next() else {
        return Ok(Vec::new());
    };
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        Single,
        Double,
        AnsiSingle,
    }

    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<Quote> = None;
    let mut chars = single.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(Quote::Double) => match ch {
                // Em aspas duplas comuns, só escapa `"` e `\` para preservar caminhos Windows (`"C:\nodejs\npx.cmd"`).
                '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '"' => quote = None,
                _ => current.push(ch),
            },
            Some(Quote::AnsiSingle) => match ch {
                '\\' if matches!(chars.peek(), Some('\'' | '\\' | 'n' | 'r')) => {
                    if let Some(next) = chars.next() {
                        match next {
                            'n' => current.push('\n'),
                            'r' => current.push('\r'),
                            other => current.push(other),
                        }
                    }
                }
                '\'' => quote = None,
                _ => current.push(ch),
            },
            Some(Quote::Single) => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None if ch == '$' && chars.peek() == Some(&'\'') => {
                let _ = chars.next();
                quote = Some(Quote::AnsiSingle);
                in_token = true;
            }
            None if ch == '\'' => {
                quote = Some(Quote::Single);
                in_token = true;
            }
            None if ch == '"' => {
                quote = Some(Quote::Double);
                in_token = true;
            }
            None if ch.is_whitespace() => {
                if in_token {
                    args.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            None => {
                current.push(ch);
                in_token = true;
            }
        }
    }
    if quote.is_some() {
        return Err(tr_shared("integrations_validation_command_quotes", &[]));
    }
    if in_token {
        args.push(current);
    }
    Ok(args)
}

pub(super) fn validate_draft(draft: &FormDraft) -> Result<(), String> {
    if !draft.is_edit && !validate_slug(draft.id.trim()) {
        return Err(tr_shared("integrations_validation_id", &[]));
    }
    let name = draft.name.trim();
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.contains('\0') {
        return Err(tr_shared("integrations_validation_name", &[]));
    }
    match draft.transport {
        TransportKind::RemoteHttp => {
            let url = draft.url.trim();
            if !validate_http_url(url) {
                return Err(tr_shared("integrations_validation_url", &[]));
            }
            let new_headers = parse_headers_input(&draft.new_headers_raw)?;
            let has_kept_headers = draft
                .existing_headers
                .iter()
                .any(|header| !draft.removed_headers.contains(header.as_str()));
            let sends_credentials = draft.auth_kind != AuthKindChoice::None
                || has_kept_headers
                || !new_headers.is_empty();
            if sends_credentials && url.starts_with("http://") && !is_loopback_http_url(url) {
                return Err(tr_shared("integrations_validation_url_insecure", &[]));
            }
        }
        TransportKind::LocalStdio => {
            let args = resolve_draft_command(draft)?;
            if args.is_empty()
                || args[0].trim().is_empty()
                || args.iter().any(|part| part.is_empty() || part.contains('\0'))
            {
                return Err(tr_shared("integrations_validation_command", &[]));
            }
            let _ = parse_env_input(&draft.new_env_raw)?;
        }
    }
    if draft.transport == TransportKind::RemoteHttp
        && draft.auth_kind == AuthKindChoice::OAuthManualClient
        && draft.client_id.trim().is_empty()
    {
        return Err(tr_shared("integrations_validation_client_id", &[]));
    }
    Ok(())
}

pub(super) fn build_create_body(draft: &FormDraft) -> Result<Value, String> {
    validate_draft(draft)?;
    let (transport, secrets) = build_transport_and_secrets(draft)?;
    let auth = build_auth_config(draft);
    let mut body = json!({
        "id": draft.id.trim(),
        "name": draft.name.trim(),
        "preset": draft.preset,
        "enabled": draft.enabled,
        "transport": transport,
        "auth": auth,
        "policy": draft.policy,
        "pinned": draft.pinned,
        "projects": parse_projects(&draft.projects_raw),
    });
    if !secrets.is_empty() {
        body["secrets"] = Value::Object(secrets);
    }
    Ok(body)
}

pub(super) fn build_patch_body(draft: &FormDraft) -> Result<Value, String> {
    validate_draft(draft)?;
    let (transport, secrets) = build_transport_and_secrets(draft)?;
    let auth = build_auth_config(draft);
    let mut body = json!({
        "name": draft.name.trim(),
        "enabled": draft.enabled,
        "auth": auth,
        "pinned": draft.pinned,
        "projects": parse_projects(&draft.projects_raw),
    });
    // Só envia `policy` quando alterada para não reverter mudanças concorrentes feitas em outro cliente.
    if draft.original_policy != Some(draft.policy) {
        body["policy"] = json!(draft.policy);
    }
    // Só envia `transport` se algo nele mudou; evita reiniciar o cliente MCP ou limpar aprovações sem necessidade.
    let transport_unchanged = draft
        .original_transport
        .as_ref()
        .is_some_and(|orig| transport_matches(orig, &transport));
    if !transport_unchanged {
        body["transport"] = json!(transport);
    }
    if !secrets.is_empty() {
        body["secrets"] = Value::Object(secrets);
    }
    Ok(body)
}

pub(super) fn build_tool_toggle_patch(tool_name: &str, allow: bool) -> Value {
    if allow {
        json!({ "add_allowed_tools": [tool_name] })
    } else {
        json!({ "remove_allowed_tools": [tool_name] })
    }
}

fn default_true() -> bool {
    true
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && host.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
}

fn valid_optional_port(tail: &str) -> bool {
    if tail.is_empty() {
        return true;
    }
    tail.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0)
}

fn is_loopback_http_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn is_http_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

fn quote_command_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
    {
        return arg.to_owned();
    }
    // Quebras de linha usam ANSI-C quoting ($'...') para manter linha única sem colidir com caminhos Windows em "...".
    if arg.contains('\n') || arg.contains('\r') {
        let escaped = arg
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        return format!("$'{escaped}'");
    }
    if !arg.contains('"') && !arg.contains('\\') {
        return format!("\"{arg}\"");
    }
    if !arg.contains('\'') {
        return format!("'{arg}'");
    }
    let escaped = arg.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn format_command_for_input(command: &[String]) -> String {
    command
        .iter()
        .map(|part| quote_command_arg(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_draft_command(draft: &FormDraft) -> Result<Vec<String>, String> {
    if let Some((initial_raw, original)) = &draft.original_command
        && draft.command_raw == *initial_raw
    {
        return Ok(original.clone());
    }
    parse_command_args(&draft.command_raw)
}

fn parse_scopes(raw: &str) -> Vec<String> {
    raw.split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_projects(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_headers_input(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(tr_shared("integrations_field_headers", &[]));
        };
        let raw_name = name.trim();
        let key = raw_name.to_ascii_lowercase();
        let val = value.trim();
        if raw_name.is_empty() {
            return Err(tr_shared("integrations_field_headers", &[]));
        }
        if key.len() > 256
            || !key.bytes().all(is_http_tchar)
            || RESERVED_HTTP_HEADERS.contains(&key.as_str())
        {
            return Err(tr_shared(
                "integrations_validation_header_name",
                &[("name", raw_name)],
            ));
        }
        if val.is_empty() || val.chars().any(|c| c == '\0' || (c.is_control() && c != '\t')) {
            return Err(tr_shared("integrations_field_headers", &[]));
        }
        map.insert(key, val.to_owned());
    }
    Ok(map)
}

fn parse_env_input(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for line in raw.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(tr_shared("integrations_field_env", &[]));
        };
        let key = key.trim();
        if key.is_empty()
            || key.len() > 256
            || key.contains('=')
            || key.chars().any(|c| c.is_whitespace() || c.is_control())
            || value.contains('\0')
        {
            return Err(tr_shared("integrations_field_env", &[]));
        }
        map.insert(key.to_owned(), value.to_owned());
    }
    Ok(map)
}

fn transport_matches(a: &Transport, b: &Transport) -> bool {
    match (a, b) {
        (
            Transport::RemoteHttp {
                url: url_a,
                header_names: hdr_a,
            },
            Transport::RemoteHttp {
                url: url_b,
                header_names: hdr_b,
            },
        ) => {
            let set_a: BTreeSet<String> = hdr_a.iter().map(|h| h.to_ascii_lowercase()).collect();
            let set_b: BTreeSet<String> = hdr_b.iter().map(|h| h.to_ascii_lowercase()).collect();
            url_a.trim() == url_b.trim() && set_a == set_b
        }
        (
            Transport::LocalStdio {
                command: cmd_a,
                cwd: cwd_a,
                env_keys: env_a,
            },
            Transport::LocalStdio {
                command: cmd_b,
                cwd: cwd_b,
                env_keys: env_b,
            },
        ) => {
            fn norm_cwd(c: &Option<String>) -> Option<&str> {
                c.as_deref().map(str::trim).filter(|s| !s.is_empty())
            }
            let set_a: BTreeSet<&str> = env_a.iter().map(String::as_str).collect();
            let set_b: BTreeSet<&str> = env_b.iter().map(String::as_str).collect();
            cmd_a == cmd_b && norm_cwd(cwd_a) == norm_cwd(cwd_b) && set_a == set_b
        }
        _ => false,
    }
}

fn build_transport_and_secrets(
    draft: &FormDraft,
) -> Result<(Transport, serde_json::Map<String, Value>), String> {
    let mut secrets = serde_json::Map::new();
    let transport = match draft.transport {
        TransportKind::RemoteHttp => {
            let new_headers = parse_headers_input(&draft.new_headers_raw)?;
            let mut header_names: BTreeSet<String> = draft
                .existing_headers
                .iter()
                .filter(|name| !draft.removed_headers.contains(name.as_str()))
                .cloned()
                .collect();
            let mut headers_patch = serde_json::Map::new();
            for removed in &draft.removed_headers {
                headers_patch.insert(removed.clone(), Value::Null);
            }
            for (k, v) in new_headers {
                header_names.insert(k.clone());
                headers_patch.insert(k, Value::String(v));
            }
            if !headers_patch.is_empty() {
                secrets.insert("headers".into(), Value::Object(headers_patch));
            }
            Transport::RemoteHttp {
                url: draft.url.trim().to_owned(),
                header_names: header_names.into_iter().collect(),
            }
        }
        TransportKind::LocalStdio => {
            let new_env = parse_env_input(&draft.new_env_raw)?;
            let mut env_keys: BTreeSet<String> = draft
                .existing_env_keys
                .iter()
                .filter(|key| !draft.removed_env_keys.contains(key.as_str()))
                .cloned()
                .collect();
            let mut env_patch = serde_json::Map::new();
            for removed in &draft.removed_env_keys {
                env_patch.insert(removed.clone(), Value::Null);
            }
            for (k, v) in new_env {
                env_keys.insert(k.clone());
                env_patch.insert(k, Value::String(v));
            }
            if !env_patch.is_empty() {
                secrets.insert("env".into(), Value::Object(env_patch));
            }
            let cwd = draft.cwd.trim();
            Transport::LocalStdio {
                command: resolve_draft_command(draft)?,
                cwd: (!cwd.is_empty()).then(|| cwd.to_owned()),
                env_keys: env_keys.into_iter().collect(),
            }
        }
    };

    match draft.auth_kind {
        AuthKindChoice::Bearer if draft.transport == TransportKind::RemoteHttp => {
            let tok = draft.bearer_token.trim();
            if !tok.is_empty() {
                secrets.insert("bearer_token".into(), Value::String(tok.to_owned()));
            }
        }
        AuthKindChoice::OAuthManualClient if draft.transport == TransportKind::RemoteHttp => {
            let sec = draft.client_secret.trim();
            if !sec.is_empty() {
                secrets.insert("oauth_client_secret".into(), Value::String(sec.to_owned()));
            }
        }
        _ => {}
    }

    Ok((transport, secrets))
}

fn build_auth_config(draft: &FormDraft) -> AuthConfig {
    if draft.transport == TransportKind::LocalStdio {
        return AuthConfig::None;
    }
    match draft.auth_kind {
        AuthKindChoice::None => AuthConfig::None,
        AuthKindChoice::Bearer => AuthConfig::Bearer,
        AuthKindChoice::OAuthDcr => AuthConfig::OAuthDcr,
        AuthKindChoice::OAuthManualClient => AuthConfig::OAuthManualClient {
            client_id: draft.client_id.trim().to_owned(),
            scopes: parse_scopes(&draft.scopes_raw),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn sample_draft() -> FormDraft {
        FormDraft {
            is_edit: false,
            id: "gmail-pessoal".into(),
            name: "Gmail Pessoal".into(),
            preset: Some("google-gmail".into()),
            enabled: true,
            transport: TransportKind::RemoteHttp,
            url: "https://gmailmcp.googleapis.com/mcp/v1".into(),
            existing_headers: Vec::new(),
            removed_headers: BTreeSet::new(),
            new_headers_raw: String::new(),
            command_raw: String::new(),
            cwd: String::new(),
            existing_env_keys: Vec::new(),
            removed_env_keys: BTreeSet::new(),
            new_env_raw: String::new(),
            auth_kind: AuthKindChoice::OAuthManualClient,
            client_id: "client-123.apps.googleusercontent.com".into(),
            client_secret: "super-secret-value".into(),
            scopes_raw: "https://www.googleapis.com/auth/gmail.readonly".into(),
            bearer_token: String::new(),
            policy: PermissionPolicy::ReadFreeAskWrite,
            pinned: true,
            projects_raw: "/home/user/proj-a\n/home/user/proj-b".into(),
            original_command: None,
            original_transport: None,
            original_policy: None,
        }
    }

    #[test]
    fn validates_slug_url_command_and_client_id() {
        assert!(validate_slug("gmail-pessoal"));
        assert!(validate_slug("a1-b2"));
        assert!(!validate_slug(""));
        assert!(!validate_slug("1abc"));
        assert!(!validate_slug("Gmail"));
        assert!(!validate_slug("gmail_pessoal"));

        assert!(validate_http_url("https://mcp.linear.app/mcp"));
        assert!(validate_http_url("http://127.0.0.1:8765/mcp"));
        assert!(!validate_http_url("ftp://example.com/mcp"));
        assert!(!validate_http_url("https://user:pass@example.com/mcp"));
        assert!(!validate_http_url("not a url"));

        assert_eq!(
            parse_command_args("npx -y @pkg/mcp-server \"/tmp/my dir\"").unwrap(),
            vec!["npx", "-y", "@pkg/mcp-server", "/tmp/my dir"]
        );
        assert_eq!(
            parse_command_args("node\n/opt/mcp/server.js\n--port\n3000").unwrap(),
            vec!["node", "/opt/mcp/server.js", "--port", "3000"]
        );
        assert_eq!(
            parse_command_args("npx \"unclosed"),
            Err(tr_shared("integrations_validation_command_quotes", &[]))
        );
        assert_eq!(
            parse_command_args("npx 'unclosed"),
            Err(tr_shared("integrations_validation_command_quotes", &[]))
        );

        let mut bad_slug = sample_draft();
        bad_slug.id = "Gmail_1".into();
        assert!(validate_draft(&bad_slug).is_err());

        let mut bad_url = sample_draft();
        bad_url.url = "http://user:pass@host/mcp".into();
        assert!(validate_draft(&bad_url).is_err());

        let mut bad_client = sample_draft();
        bad_client.client_id = "   ".into();
        assert!(validate_draft(&bad_client).is_err());

        let mut local_empty = sample_draft();
        local_empty.preset = None;
        local_empty.transport = TransportKind::LocalStdio;
        local_empty.auth_kind = AuthKindChoice::None;
        local_empty.command_raw = "   ".into();
        assert!(validate_draft(&local_empty).is_err());
    }

    #[test]
    fn command_formatting_round_trips_spaces_quotes_and_backslashes() {
        for command in [
            vec!["/opt/My Tools/server".to_owned()],
            vec![
                "node".to_owned(),
                "/opt/My Tools/server.js".to_owned(),
                "--label".to_owned(),
                "a \"b\" and 'c' \\ d".to_owned(),
            ],
            vec![
                "C:\\Program Files\\MCP\\server.exe".to_owned(),
                "--dir".to_owned(),
                "C:\\Users\\Alice".to_owned(),
            ],
        ] {
            let formatted = format_command_for_input(&command);
            let reparsed = parse_command_args(&formatted).expect("comando formatado válido");
            assert_eq!(reparsed, command);
        }
    }

    #[test]
    fn env_and_header_parsers_and_url_security_match_server_rules() {
        let env = parse_env_input("  \n  LABEL =production \r\nFOO= bar  ").unwrap();
        assert_eq!(env.get("LABEL").map(String::as_str), Some("production "));
        assert_eq!(env.get("FOO").map(String::as_str), Some(" bar  "));

        assert_eq!(
            parse_headers_input("X(Key): secret"),
            Err(tr_shared("integrations_validation_header_name", &[("name", "X(Key)")]))
        );
        assert_eq!(
            parse_headers_input("Authorization: Bearer abc"),
            Err(tr_shared("integrations_validation_header_name", &[("name", "Authorization")]))
        );
        assert_eq!(
            parse_headers_input("X-Api-Key: secret").unwrap().get("x-api-key").map(String::as_str),
            Some("secret")
        );

        let mut insecure_bearer = sample_draft();
        insecure_bearer.preset = None;
        insecure_bearer.auth_kind = AuthKindChoice::Bearer;
        insecure_bearer.url = "http://mcp.example.com/mcp".into();
        assert_eq!(
            validate_draft(&insecure_bearer),
            Err(tr_shared("integrations_validation_url_insecure", &[]))
        );

        let mut insecure_headers = sample_draft();
        insecure_headers.preset = None;
        insecure_headers.auth_kind = AuthKindChoice::None;
        insecure_headers.url = "http://mcp.example.com/mcp".into();
        insecure_headers.new_headers_raw = "X-Api-Key: secret".into();
        assert_eq!(
            validate_draft(&insecure_headers),
            Err(tr_shared("integrations_validation_url_insecure", &[]))
        );

        for loopback in [
            "http://localhost:8080/mcp",
            "http://127.0.0.1:8765/mcp",
            "http://[::1]:8765/mcp",
        ] {
            let mut local_http = insecure_bearer.clone();
            local_http.url = loopback.into();
            assert!(validate_draft(&local_http).is_ok(), "{loopback} deve aceitar HTTP");
        }

        let mut open_http = sample_draft();
        open_http.preset = None;
        open_http.auth_kind = AuthKindChoice::None;
        open_http.url = "http://192.168.1.50:3000/mcp".into();
        assert!(validate_draft(&open_http).is_ok(), "http sem credenciais nem cabeçalhos fica liberado");
    }

    #[test]
    fn create_and_edit_payloads_never_leak_secrets_when_empty_or_saved() {
        let create = build_create_body(&sample_draft()).expect("payload criação válido");
        assert_eq!(create["id"], "gmail-pessoal");
        assert_eq!(create["auth"]["kind"], "oauth_manual_client");
        assert_eq!(create["secrets"]["oauth_client_secret"], "super-secret-value");
        assert_eq!(create["projects"], json!(["/home/user/proj-a", "/home/user/proj-b"]));

        // Na edição sem digitar segredos novos, `secrets` é omitido e `header_names`/`env_keys` salvos são preservados.
        let mut edit = sample_draft();
        edit.is_edit = true;
        edit.client_secret.clear();
        edit.existing_headers = vec!["x-tenant".into()];
        let patch = build_patch_body(&edit).expect("payload edição válido");
        assert!(patch.get("secrets").is_none(), "edição sem segredos novos não envia secrets");
        assert_eq!(patch["transport"]["header_names"], json!(["x-tenant"]));

        // Deserialização da resposta pública nunca retém campos secretos no estado em memória.
        let view: IntegrationItem = serde_json::from_value(json!({
            "id": "gmail-pessoal",
            "name": "Gmail Pessoal",
            "preset": "google-gmail",
            "enabled": true,
            "transport": { "kind": "remote_http", "url": "https://gmailmcp.googleapis.com/mcp/v1", "header_names": ["x-tenant"] },
            "auth": { "kind": "oauth_manual_client", "client_id": "cid", "scopes": [] },
            "policy": "read_free_ask_write",
            "pinned": false,
            "projects": [],
            "allowed_tools": [],
            "created_at": 1.0,
            "updated_at": 1.0,
            "status": "ready",
            "bearer_token": "should-never-be-kept"
        }))
        .expect("IntegrationItem válido");
        assert_eq!(view.status, IntegrationStatus::Ready);
        assert_eq!(
            view.transport,
            Transport::RemoteHttp {
                url: "https://gmailmcp.googleapis.com/mcp/v1".into(),
                header_names: vec!["x-tenant".into()]
            }
        );
    }

    #[test]
    fn edit_patch_omits_unchanged_transport_and_preserves_original_command_on_cwd_or_env_change() {
        let orig_cmd = vec!["/opt/My Tools/server".to_owned(), "--flag".to_owned()];
        let item = IntegrationItem {
            id: "local-mcp".into(),
            name: "Local MCP".into(),
            preset: None,
            enabled: true,
            transport: Transport::LocalStdio {
                command: orig_cmd.clone(),
                cwd: Some("/work/a".into()),
                env_keys: vec!["API_KEY".into()],
            },
            auth: AuthConfig::None,
            policy: PermissionPolicy::ReadFreeAskWrite,
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 1.0,
            updated_at: 1.0,
            status: IntegrationStatus::Ready,
            last_error: None,
        };
        let initial = InitialFormValues::from_mode(FormMode::Edit(item, None), &[]);
        let base_draft = FormDraft {
            is_edit: initial.is_edit,
            id: initial.id,
            name: "Local MCP v2".into(),
            preset: initial.preset,
            enabled: initial.enabled,
            transport: initial.transport,
            url: initial.url,
            existing_headers: initial.existing_headers,
            removed_headers: BTreeSet::new(),
            new_headers_raw: String::new(),
            command_raw: initial.command,
            cwd: initial.cwd,
            existing_env_keys: initial.existing_env_keys,
            removed_env_keys: BTreeSet::new(),
            new_env_raw: String::new(),
            auth_kind: initial.auth_kind,
            client_id: initial.client_id,
            client_secret: String::new(),
            scopes_raw: initial.scopes,
            bearer_token: String::new(),
            policy: initial.policy,
            pinned: initial.pinned,
            projects_raw: initial.projects,
            original_command: initial.original_command,
            original_transport: initial.original_transport,
            original_policy: initial.original_policy,
        };

        let no_transport_change = build_patch_body(&base_draft).expect("patch válido");
        assert!(no_transport_change.get("transport").is_none());
        assert!(
            no_transport_change.get("policy").is_none(),
            "editar sem mudar policy deve omitir policy no PATCH"
        );
        let multiline = format_command_for_input(&[
            "mcp-server".into(),
            "--label".into(),
            "linha 1\nlinha 2".into(),
            "--quiet".into(),
        ]);
        let edited_multiline = multiline.replace("--quiet", "--verbose");
        assert_eq!(
            parse_command_args(&edited_multiline).unwrap(),
            vec!["mcp-server", "--label", "linha 1\nlinha 2", "--verbose"]
        );
        assert_eq!(
            parse_command_args(r#""C:\nodejs\npx.cmd" --yes pacote"#).unwrap(),
            vec![r"C:\nodejs\npx.cmd", "--yes", "pacote"]
        );

        let mut cwd_change = base_draft.clone();
        cwd_change.cwd = "/work/b".into();
        let patch_cwd = build_patch_body(&cwd_change).expect("patch cwd válido");
        assert_eq!(patch_cwd["transport"]["command"], json!(orig_cmd));
        assert_eq!(patch_cwd["transport"]["cwd"], "/work/b");

        let mut secret_value_only = base_draft;
        secret_value_only.new_env_raw = "API_KEY=rotated ".into();
        let patch_env_secret = build_patch_body(&secret_value_only).expect("patch env válido");
        assert!(patch_env_secret.get("transport").is_none());
        assert_eq!(patch_env_secret["secrets"]["env"]["API_KEY"], "rotated ");

        assert_eq!(
            build_tool_toggle_patch("create_issue", true),
            json!({ "add_allowed_tools": ["create_issue"] })
        );
        assert_eq!(
            build_tool_toggle_patch("create_issue", false),
            json!({ "remove_allowed_tools": ["create_issue"] })
        );
    }

    #[test]
    fn display_helpers_and_preset_id_suggestions_work() {
        crate::i18n::english_in_this_thread(false);
        assert_eq!(status_presentation(IntegrationStatus::Ready).0, "Conectada");
        assert_eq!(status_presentation(IntegrationStatus::AuthExpired).0, "Token expirado");
        assert_eq!(policy_label(PermissionPolicy::ReadFreeAskWrite, true), "Leitura livre");
        assert_eq!(tool_access_label(ToolAccess::AllowedAlways).0, "Sempre liberada");

        let item = |id: &str| IntegrationItem {
            id: id.into(),
            name: id.into(),
            preset: Some("google-gmail".into()),
            enabled: true,
            transport: Transport::RemoteHttp {
                url: "https://gmailmcp.googleapis.com/mcp/v1".into(),
                header_names: vec![],
            },
            auth: AuthConfig::OAuthManualClient {
                client_id: "c".into(),
                scopes: vec![],
            },
            policy: PermissionPolicy::ReadFreeAskWrite,
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 0.0,
            updated_at: 0.0,
            status: IntegrationStatus::Ready,
            last_error: None,
        };
        assert_eq!(suggest_preset_id("google-gmail", &[]), "google-gmail");
        assert_eq!(suggest_preset_id("google-gmail", &[item("google-gmail")]), "google-gmail-2");
        assert_eq!(
            suggest_preset_id("google-gmail", &[item("google-gmail"), item("google-gmail-2")]),
            "google-gmail-3"
        );
    }

    #[test]
    fn oauth_start_body_distinguishes_local_desktop_from_remote_connections() {
        let local = Api::new("http://127.0.0.1:8765", "t").unwrap();
        assert_eq!(oauth_start_body(&local), json!({ "origin_client": "desktop" }));

        let remote_https = Api::new("https://casa.tail.ts.net/", "t").unwrap();
        assert_eq!(
            oauth_start_body(&remote_https),
            json!({
                "origin_client": "remote_desktop",
                "public_base_url": "https://casa.tail.ts.net"
            })
        );

        let remote_lan = Api::new("http://192.168.0.7:8765", "t").unwrap();
        assert_eq!(
            oauth_start_body(&remote_lan),
            json!({ "origin_client": "remote_desktop" })
        );
    }

    #[test]
    fn should_auto_start_oauth_matches_newly_created_oauth_remote_integrations() {
        let mk = |auth: AuthConfig, transport: Transport, enabled: bool| IntegrationItem {
            id: "figma".into(),
            name: "Figma".into(),
            preset: Some("figma".into()),
            enabled,
            transport,
            auth,
            policy: PermissionPolicy::ReadFreeAskWrite,
            pinned: false,
            projects: vec![],
            allowed_tools: BTreeSet::new(),
            created_at: 0.0,
            updated_at: 0.0,
            status: IntegrationStatus::AuthRequired,
            last_error: None,
        };
        let remote = Transport::RemoteHttp {
            url: "https://mcp.figma.com/mcp".into(),
            header_names: vec![],
        };
        let stdio = Transport::LocalStdio {
            command: vec!["npx".into()],
            cwd: None,
            env_keys: vec![],
        };

        assert!(should_auto_start_oauth(true, &mk(AuthConfig::OAuthDcr, remote.clone(), true)));
        assert!(should_auto_start_oauth(
            true,
            &mk(
                AuthConfig::OAuthManualClient {
                    client_id: "cid".into(),
                    scopes: vec![],
                },
                remote.clone(),
                true
            )
        ));
        assert!(!should_auto_start_oauth(false, &mk(AuthConfig::OAuthDcr, remote.clone(), true)));
        assert!(!should_auto_start_oauth(true, &mk(AuthConfig::Bearer, remote.clone(), true)));
        assert!(!should_auto_start_oauth(true, &mk(AuthConfig::OAuthDcr, remote, false)));
        assert!(!should_auto_start_oauth(true, &mk(AuthConfig::OAuthDcr, stdio, true)));
    }
}
