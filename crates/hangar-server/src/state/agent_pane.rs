//! Em qual pane da sessão o agente roda (`agentpane.resolve_target`). O alvo `=nome:` aponta para
//! o pane ativo, e uma janela nova do usuário faria a captura ler o shell. Guardado por 60 s: o
//! pane do agente só muda quando o agente morre, e a resolução lê processos.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::list::capped::{Capped, SESSION_CAP};
use crate::list::discover::{basename, descendants, exec_provider, is_aux};
use crate::list::mux::Pane;
use crate::list::procs::ChildrenMap;

pub const TTL: Duration = Duration::from_secs(60);

fn agent_tree(pid: i64, children: &ChildrenMap, argv_of: &dyn Fn(i64) -> Vec<String>) -> bool {
    descendants(pid, children).into_iter().any(|p| {
        let argv = argv_of(p);
        !is_aux(&argv) && argv.first().is_some_and(|argv0| exec_provider(basename(argv0)).is_some())
    })
}

/// Alvo do pane do agente entre os panes da sessão; `None` = vale `=nome:` (um pane só, nenhum
/// pane de agente ou sem alvo preciso). Com dois panes de agente vence o ativo, como na lista.
pub fn resolve(panes: &[Pane], children: &ChildrenMap, argv_of: &dyn Fn(i64) -> Vec<String>) -> Option<String> {
    if panes.len() <= 1 {
        return None;
    }
    let mut ordered: Vec<&Pane> = panes.iter().collect();
    ordered.sort_by_key(|p| !p.active);
    let found = ordered.into_iter()
        .find(|p| p.pid.is_some_and(|pid| agent_tree(i64::from(pid), children, argv_of)));
    match found {
        Some(p) => p.target(),
        None => {
            let session = panes[0].session.as_str();
            if crate::warn_limit::allow(Some(session), "state_agent_pane_unknown") {
                tracing::warn!(code = "state_agent_pane_unknown", session, panes = panes.len(),
                    "estado: nenhum pane parece do agente; a captura usa a janela ativa");
            }
            None
        }
    }
}

/// Cache de `resolve` por sessão.
pub struct AgentPanes { cache: Mutex<Capped<String, (Option<String>, Instant)>> }

impl Default for AgentPanes {
    fn default() -> Self { Self { cache: Mutex::new(Capped::new(SESSION_CAP)) } }
}

impl AgentPanes {
    /// `compute` só roda sem resposta guardada há menos de `TTL`, fora da trava (lê processos e
    /// o multiplexador: chamar por `spawn_blocking`).
    pub fn target(&self, name: &str, now: Instant, compute: impl FnOnce() -> Option<String>) -> Option<String> {
        let lock = || self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((target, _)) = lock().get(name).filter(|(_, at)| now.saturating_duration_since(*at) < TTL) {
            return target.clone();
        }
        let target = compute();
        lock().insert(name.to_owned(), (target.clone(), now));
        target
    }

    /// Resposta guardada há menos de `TTL`, sem calcular: quem calcula precisa da descoberta, que
    /// é assíncrona (`target` com o resultado dela).
    pub fn cached(&self, name: &str, now: Instant) -> Option<Option<String>> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.peek(name).filter(|(_, at)| now.saturating_duration_since(*at) < TTL).map(|(t, _)| t.clone())
    }

    pub fn forget(&self, name: &str) { self.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(name); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn matches_python_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/agent_pane.json");
        let rows: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for row in rows.as_array().unwrap() {
            let panes: Vec<Pane> = row["panes"].as_array().unwrap().iter().enumerate().map(|(i, p)| Pane {
                session: "fixture".into(), active: p["active"].as_bool().unwrap(), pid: p["pid"].as_u64().map(|v| v as u32),
                pane_id: p["pane_id"].as_str().unwrap().into(), window_index: Some(0), pane_index: Some(i as u32), ..Default::default()
            }).collect();
            let children: ChildrenMap = row["children"].as_object().unwrap().iter()
                .map(|(k, v)| (k.parse().unwrap(), serde_json::from_value(v.clone()).unwrap())).collect();
            let cmdline = |pid: i64| row["cmdline"][pid.to_string()].as_str().unwrap_or("").split_whitespace().map(str::to_owned).collect::<Vec<_>>();
            // O golden vem do Python no Linux (`%N`); no Windows o alvo do mesmo pane é `=s:w.p`.
            let expected = row["expected"].as_str().and_then(|id| panes.iter().find(|p| p.pane_id == id).and_then(Pane::target));
            assert_eq!(resolve(&panes, &children, &cmdline), expected, "{}", row["name"]);
        }
    }

    #[test]
    fn cached_for_ttl() {
        let panes = AgentPanes::default();
        let t0 = Instant::now();
        let mut calls = 0;
        for at in [t0, t0 + Duration::from_secs(59)] {
            assert_eq!(panes.target("s", at, || { calls += 1; Some("%2".into()) }).as_deref(), Some("%2"));
        }
        assert_eq!(calls, 1);
        panes.target("s", t0 + TTL, || { calls += 1; None });
        assert_eq!(calls, 2, "vencido, resolve de novo");
        panes.forget("s");
        panes.target("s", t0 + TTL, || { calls += 1; None });
        assert_eq!(calls, 3, "esquecido, resolve de novo");
    }
}
