//! Continuar sozinho o turno que parou por queda passageira: a classificação da parada, os textos da
//! continuação, o registro que limita toda continuação automática da sessão, de qualquer origem, e a
//! agenda que espera a rede voltar para continuar.

pub mod classify;
pub mod ledger;
pub mod probe;
pub mod scheduler;
pub mod text;

pub use classify::{StopKind, classify_claude_line, classify_codex_turn};
pub use ledger::{Cap, Ledger, Source};
pub use probe::{HttpProbe, Probe};
pub use scheduler::{Command, Facts, Host, StopHit, spawn, spawn_with_probe};
