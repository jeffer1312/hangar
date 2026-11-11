//! Leitura dos artefatos de workflows associados ao transcript da sessão.
#![forbid(unsafe_code)]

mod agent;
mod artifacts;
mod live;
mod reader;

pub use agent::get_agent;
pub use reader::{get_workflow, list_workflows};

/// Rotas de leitura dos três níveis do painel de workflows.
pub const ROUTES: &[(&str, &str)] = &[
    ("/api/sessions/{name}/workflows", "GET"),
    ("/api/sessions/{name}/workflows/{run_id}", "GET"),
    (
        "/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}",
        "GET",
    ),
];
