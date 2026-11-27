//! Continuar sozinho o turno que parou por queda passageira: a classificação da parada, os textos da
//! continuação e o registro que limita toda continuação automática da sessão, de qualquer origem.

pub mod classify;
pub mod ledger;
pub mod text;

pub use classify::{StopKind, classify_claude_line, classify_codex_turn};
pub use ledger::{Cap, Ledger, Source};
