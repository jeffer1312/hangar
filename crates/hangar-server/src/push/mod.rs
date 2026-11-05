//! Avisos de sessão pelos canais Web Push e Expo, com produtores e preferências compartilhados.
mod config;
pub mod http;
pub mod send;
pub mod store;
pub mod watch;
pub mod webpush;

pub use watch::{start, subscribe, TurnEvent};

/// O ambiente do processo com as opções `CP_*` do `.env` do servidor (onde ficam as chaves VAPID).
pub fn server_env() -> std::collections::BTreeMap<String, String> {
    let dotenv = crate::user_data::env_file(std::env::current_dir().ok().as_deref())
        .inspect_err(|e| tracing::warn!(code = "user_data_env_unreadable", "push: .env dos dados do usuário ilegível, segue sem ele: {e}"))
        .ok().flatten().unwrap_or_default();
    crate::session_open::server::server_env(std::env::vars().collect(), &dotenv)
}
