//! Domínio, catálogo, cliente MCP e armazenamento cifrado de segredos das integrações do Hangar.
#![forbid(unsafe_code)]

pub mod approvals;
pub mod catalog;
pub mod client;
pub mod guard;
pub mod model;
pub mod oauth;
pub mod registry;
pub mod secrets;
pub mod tools_cache;

pub use approvals::{
    APPROVAL_EXPIRES, AnsweredApproval, ApprovalCard, ApprovalConfig, ApprovalError, ApprovalFeed,
    ApprovalResolution, ApprovalStore, ApprovalWaiter, COOLDOWN, CancelOutcome, ClaimedApproval,
    MAX_ARGUMENTS_PREVIEW_CHARS, MAX_OPEN, MAX_OPEN_PER_SESSION, MAX_REASON_CHARS,
    OpenApprovalRequest, format_arguments_preview, format_origin, truncate_preview_chars,
};
pub use catalog::{CatalogPreset, PresetAuthKind, SetupGuide, catalog, catalog_for_port, find_preset};
pub use client::{
    ClientConfig, ClientError, DEFAULT_CALL_TIMEOUT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_EXIT_GRACE,
    DEFAULT_IDLE_TTL, McpCallOutput, McpClientManager, TokenRefresher,
};
pub use guard::{MAX_RESPONSE_BYTES, TRUNCATION_NOTICE, truncate_utf8, wrap_external_result};
pub use model::{
    ApprovalDecision, AuthConfig, IntegrationConfig, IntegrationStatus, PermissionPolicy,
    ToolAccess, ToolSummary, Transport, ValidationError, config_fingerprint,
    validate_config_and_secrets,
};
pub use oauth::{
    DEFAULT_STATE_TTL, OAUTH_CALLBACK_PATH, OAuthCallbackResult, OAuthError, OAuthManager,
    OAuthOrigin, OAuthStartInput, OAuthStartOutcome, compute_status, loopback_redirect_uri,
};
pub use registry::{IntegrationRegistry, IntegrationUpdate, RegistryError, config_path};
pub use secrets::{
    IntegrationSecrets, MemoryKeyStore, OAuthTokens, SECRET_KEY_LEN, SECRETS_FORMAT_VERSION,
    SecretKey, SecretKeyError, SecretKeyStore, SecretStore, SecretStoreError, secrets_path,
};
pub use tools_cache::{DiscoveredTool, ToolsCache, classify_tool_access};
