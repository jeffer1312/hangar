//! Aviso no celular decidido e enviado pelo Rust (Web Push e Expo): o que o `push.py`, as
//! transições do `hook_state` e o `stall_watch` do Python faziam para as sessões.
pub mod http;
pub mod send;
pub mod store;
pub mod watch;
pub mod webpush;

pub use watch::{start, subscribe, TurnEvent};

/// O ambiente do processo com as opções `CP_*` do `backend/.env` (onde ficam as chaves VAPID).
pub fn server_env() -> std::collections::BTreeMap<String, String> {
    let dotenv = std::env::current_dir().map(|d| d.join(".env")).unwrap_or_default();
    crate::session_open::server::server_env(std::env::vars().collect(), &dotenv)
}
