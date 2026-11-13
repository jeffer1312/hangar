//! Transferência da fileira de atalhos: exportar sem credencial, importar com prévia e desfazer, e
//! rodar o `verify` de cada atalho importado. O servidor só liga as rotas e implementa [`Host`].
#![forbid(unsafe_code)]

mod host;
mod http;
mod lexer;
mod scripts;
mod scrub;
mod transfer;
mod verify;

pub use host::{Error, Host, Machine, Snapshot};
pub use http::{ROUTES, owns_path, router};
pub use lexer::{LexError, split_line};
pub use scrub::{has_placeholder, scrub};
pub use transfer::{ImportRequest, export, import};
pub use verify::{VERIFY_TIMEOUT, run_checks};
