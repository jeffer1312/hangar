//! Em qual máquina nasce uma sessão nova: fatos de cada máquina e regras do dono entram, a máquina
//! e o motivo saem. Puro: quem coleta os fatos e encaminha o pedido é o servidor.
#![forbid(unsafe_code)]

mod choose;
mod rules;
pub mod store;

pub use choose::{
    choose, pinned, CloneTarget, Exclusion, KnownRepo, MachineFacts, OpenContext, Placement, ReasonCode,
    Unpublished, Warning,
};
pub use rules::{AgentMode, Limits, PlacementConfig, ProjectRule, ScheduleRule, UiMode};
