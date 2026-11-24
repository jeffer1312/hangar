//! Revisão do trabalho (`work_review/v1`) no painel da sessão: acha o ticket da sessão, relê a revisão
//! no fim de cada turno e guarda o resumo do chip. Não conhece fornecedor: tudo vem do conector.
#![forbid(unsafe_code)]

mod assets;
mod draft;
mod engine;
pub mod http;
mod turns;

pub use draft::{Draft, Pick};
pub use engine::{DecideBody, Diag, Engine, Failure, Linked, SessionRef, Sessions, Target, Task, Via};
pub use turns::{Ended, TurnRow, TurnTracker};
