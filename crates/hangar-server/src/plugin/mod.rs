//! Memória e rotas da ponte do plugin do Claude.
pub mod routes;
pub mod sources;
mod store;

pub use store::{AGENT_MESSAGE_MODE, AgentLife, AgentSend, AgentState, Ask, PermissionGate, PlanNotice, PluginQuestion, Poll, Pull, PullStatus, Receipt, Store};
pub use crate::terminal_input::{PluginMode, PluginReply, PluginRequest, ServiceError};
