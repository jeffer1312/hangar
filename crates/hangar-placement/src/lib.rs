//! Em qual máquina nasce uma sessão nova: fatos de cada máquina e regras do dono entram, a máquina
//! e o motivo saem, mais o clone do repositório que falta na máquina escolhida. Quem coleta os fatos, registra no
//! diário e encaminha o pedido é o servidor.
#![forbid(unsafe_code)]

mod choose;
pub mod clone;
mod rules;
pub mod store;
mod text;

pub use choose::{
    choose, pinned, CloneTarget, Exclusion, KnownRepo, MachineFacts, OpenContext, Placement, ReasonCode,
    Unpublished, Warning,
};
pub use rules::{AgentMode, Limits, PlacementConfig, ProjectRule, ScheduleRule, UiMode};
pub use text::{text, why_text};
