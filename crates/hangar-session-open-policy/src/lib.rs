//! Regras que transformam um pedido de abertura em uma configuração de sessão.
#![forbid(unsafe_code)]

mod model;
mod names;
mod quota;
mod resolve;

pub use model::{
    Account, AccountSource, AgentProvider, ChangeReason, ClaudeCustomizations, Defaults, OpenError,
    PermissionMode, SPEC_VERSION, ServiceTier, SessionChange, SessionOpenRequest, SessionOptions,
    SessionSpec, Surface, WorktreeSpec,
};
pub use names::sanitize_session_name;
pub use quota::{FULL_PCT, LOGIN_SPENT_S, SpentAccounts, SpentMark};
pub use resolve::{
    CodexCatalogError, CodexLogin, CreatorFacts, OpenCaller, OpenFacts, ProviderCatalog, Resolved,
    ServerDefaults, choose_provider, creator_codex_mode, creator_mode, fit_name, folder_base,
    folder_pattern, known_config_dir, resolve_open, valid_model,
};

/// A recusa da criação, da reabertura e da troca quando `supports_ultrafast` diz não.
pub const ULTRAFAST_REFUSED: &str = "Ultra Fast exige conta ChatGPT Pro ou Pro Max fixa no CLIProxyAPI local";

#[cfg(test)]
mod tests;
