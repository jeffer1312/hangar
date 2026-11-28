//! O retrato da máquina e da sessão na hora de uma falha, em valores fechados de `events::CONTEXT`.
use crate::routes::AppState;

/// Servidor de pé há, memória e disco livres, carga e sessões vivas por provedor e superfície.
pub(crate) fn machine(st: &AppState) -> Vec<(&'static str, String)> {
    let _ = st;
    Vec::new()
}

/// Provedor, superfície, modelo, idade, estado e versão da CLI da sessão `name`, pela última lista boa.
pub(crate) fn session(st: &AppState, name: &str) -> Vec<(&'static str, String)> {
    let _ = (st, name);
    Vec::new()
}
