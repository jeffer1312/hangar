//! Falhas recentes de cada servidor, para o relatório de diagnóstico (`POST /api/diag/report`): só a rota genérica, o
//! status e o código. Nunca corpo, consulta nem nome de sessão.
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use serde::Serialize;
use url::Url;

const PER_SERVER: usize = 50;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecentFailure { pub ts: f64, pub route: String, pub status: u16, pub code: Option<String> }

/// Chave = origem do endereço que respondeu (ou não respondeu); mais recente na frente.
static FAILURES: LazyLock<Mutex<HashMap<String, VecDeque<RecentFailure>>>> = LazyLock::new(Default::default);

// Segmentos cujo valor seguinte é nome ou caminho (o `rotaGenerica` do core): viram `*`.
const OPAQUE: [&str; 8] = ["sessions", "servers", "agents", "archive", "claude-configs", "projects", "codex-contas", "conta-estado"];

pub(super) fn origin(url: &Url) -> String { url.origin().ascii_serialization() }

/// `status` 0 = sem resposta. Código fora do formato do servidor vai como `null` (senão o relatório inteiro é recusado).
pub(super) fn record(method: &str, url: &Url, status: u16, code: Option<&str>) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
    let failure = RecentFailure { ts, route: format!("{method} {}", generic_route(url.path())), status,
        code: code.filter(|c| valid_code(c)).map(str::to_owned) };
    let mut table = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    let list = table.entry(origin(url)).or_default();
    list.push_front(failure);
    list.truncate(PER_SERVER);
}

/// Mais recentes primeiro, até 50, juntando as origens pelas quais o mesmo servidor foi alcançado.
pub(super) fn recent(origins: &[String]) -> Vec<RecentFailure> {
    let table = FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<RecentFailure> = origins.iter().collect::<BTreeSet<_>>().into_iter()
        .filter_map(|o| table.get(o)).flatten().cloned().collect();
    all.sort_by(|a, b| b.ts.total_cmp(&a.ts));
    all.truncate(PER_SERVER);
    all
}

fn generic_route(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        let hide = match out.last() {
            Some(prev) if OPAQUE.contains(prev) => true,
            Some(&"queue") => segment != "reorder",
            _ => false,
        };
        out.push(if hide && !segment.is_empty() { "*" } else { segment });
    }
    out.join("/")
}

fn valid_code(code: &str) -> bool {
    (1..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.:-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_hides_session_and_queue_id_but_keeps_reorder() {
        assert_eq!(generic_route("/api/sessions/api-front/queue/q1/steer"), "/api/sessions/*/queue/*/steer");
        assert_eq!(generic_route("/api/sessions/x/queue/reorder"), "/api/sessions/*/queue/reorder");
        assert_eq!(generic_route("/api/archive/-home-fulano-proj"), "/api/archive/*");
        assert_eq!(generic_route("/api/diag/report"), "/api/diag/report");
    }

    #[test]
    fn keeps_fifty_per_server_newest_first_and_drops_bad_codes() {
        let url = Url::parse("http://recent-test.invalid:9/api/sessions/a/input?token=x").unwrap();
        for n in 0..55 { record("POST", &url, 500 + (n % 3), Some(if n == 54 { "Bad Code" } else { "runtime_unhealthy" })); }
        let other = Url::parse("http://recent-other.invalid/api/x").unwrap();
        record("GET", &other, 0, Some("transport"));
        let list = recent(&[origin(&url), origin(&url)]);
        assert_eq!(list.len(), 50);
        assert_eq!(list[0].route, "POST /api/sessions/*/input");
        assert_eq!(list[0].code, None);
        assert!(list.iter().all(|f| f.status != 0));
        assert_eq!(recent(&[origin(&other)])[0].code.as_deref(), Some("transport"));
    }
}
