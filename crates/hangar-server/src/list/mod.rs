//! Descoberta da lista de sessões: leitores do sistema (processos, panes) que o resto da lista
//! consome sem saber de que plataforma veio cada dado.
pub mod bridge;
pub mod capped;
pub mod classify;
pub mod context;
pub mod discover;
pub mod discover_other;
pub mod facts;
pub mod facts_files;
pub mod hub;
pub mod limit;
pub mod links;
pub mod local_facts;
pub mod local_shortcuts;
pub mod local_transfers;
pub mod mux;
pub mod plan;
pub mod plugin_tools;
pub mod procs;
pub mod reply;
pub mod sig;
