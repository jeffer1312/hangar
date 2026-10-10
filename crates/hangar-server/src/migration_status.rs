//! Tela temporária "Migração para Rust" (sai na parte 7): quem atende cada área e quanto cada
//! processo consome. A tabela sai das funções que o roteador usa para decidir; os contadores,
//! do que cada pedido de fato teve: resposta do Rust ou repasse ao Python.
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::{Value, json};

use crate::routes::{AppState, cors, gate, pass};

/// Marca da resposta que veio do Python: posta por `routes::pass`, lida pelo contador.
#[derive(Clone, Copy)]
pub(crate) struct Forwarded;

pub const AREAS: [&str; 22] = [
    "history", "list", "send", "session", "terminal", "workspace", "worktrees", "costs", "quotas",
    "codex", "providers", "accounts", "guests", "pairing", "mcp", "push", "update", "uploads",
    "dictation", "mods", "static", "other",
];

/// Trabalho que o Python pede ao Rust por trás (porta privada e canal do runtime).
pub const PRIVATE: [&str; 7] = ["send_headless", "send_terminal", "terminal_observe", "workspace_bridge", "list_bridge", "pages_bridge", "transcription_bridge"];

const WINDOW_MINUTES: u64 = 10;
const FACTS_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FACTS: usize = 64 * 1024;

fn area(key: &str) -> usize {
    AREAS.iter().position(|a| *a == key).expect("área conhecida")
}

fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Área de um caminho público; todo caminho cai em uma, `other` no fim.
pub fn area_of(path: &str) -> usize {
    if let Some(rest) = path.strip_prefix("/api/sessions/") {
        if matches!(rest, "events" | "creation-progress") {
            return area("list");
        }
        let tail = rest.split_once('/').map_or("", |(_, t)| t);
        let first = tail.split('/').next().unwrap_or("");
        return area(match first {
            "history" | "events" => "history",
            "git" | "files" | "file" | "branches" | "checkout" => "workspace",
            "cost" => "costs",
            "limits" => "quotas",
            "input" | "steer" | "interrupt" | "answer" | "select" | "queue" | "question" | "then" | "btw"
            | "recarregar" | "resume" => "send",
            "term" | "term-input" | "keys" | "shell" | "shortcut-shell" | "shortcut-terminals" | "pane"
            | "open-terminal" | "bash-output" => "terminal",
            "codex" | "codex-permissions" => "codex",
            "pi" | "kimi" | "orq" => "providers",
            "pair" | "pair-remote" | "unpair-remote" | "pair-accept" | "pair-invite" | "peer-address"
            | "group-message" => "pairing",
            "share" => "guests",
            "upload" | "uploads" | "transcript-image" => "uploads",
            "transcribe" => "dictation",
            "plugin" => "mods",
            "conta" => "accounts",
            _ => "session",
        });
    }
    let table: [(&[&str], &str); 17] = [
        (&["/api/sessions"], "list"),
        (&["/mcp"], "mcp"),
        (&["/api/fs"], "workspace"),
        (&["/api/worktrees"], "worktrees"),
        (&["/api/costs", "/api/uso", "/api/cotacao"], "costs"),
        (&["/api/cotas"], "quotas"),
        (&["/api/hangar-terminals"], "terminal"),
        (&["/api/codex-contas"], "codex"),
        (&["/api/orq", "/api/orquestracao", "/api/omp", "/api/pi"], "providers"),
        (&["/api/claude-configs", "/api/claude", "/api/conta-estado", "/api/credenciais", "/api/engines", "/api/harness"], "accounts"),
        (&["/api/guests", "/api/guest", "/api/share", "/api/me", "/par"], "guests"),
        (&["/api/peers", "/api/alcance", "/api/external-pairs", "/api/pair"], "pairing"),
        (&["/api/push"], "push"),
        (&["/api/atualizacao", "/api/update-channel", "/api/deploy"], "update"),
        (&["/api/dictation", "/api/ditado", "/api/tts"], "dictation"),
        (&["/api/plugin"], "mods"),
        (&["/api/migration"], "other"),
    ];
    let fallback = if under(path, "/api") { "other" } else { "static" };
    table.iter().find(|(prefixes, _)| prefixes.iter().any(|p| under(path, p))).map_or(area(fallback), |(_, a)| area(a))
}

/// O Rust atende este pedido do dono sem repassar? Mesma decisão do `routes::router`: as rotas
/// GET registradas lá, mais os `matches` de Git/arquivos e de worktrees que o `pass_any` consulta.
pub fn rust_route(method: &Method, path: &str) -> bool {
    if crate::uploads::http::matches(method, path) { return true; }
    if crate::accounts::http::matches(method, path) { return true; }
    if *method == Method::GET {
        if matches!(path, "/api/sessions" | "/api/sessions/events" | "/api/costs" | "/api/cotacao" | "/api/uso" | "/api/migration/status") {
            return true;
        }
        let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
        // `term`: o painel do dono abre no Rust; o resto (convidado, Connect) passa pelo Python e liga ao PTY dele.
        if matches!(tail, Some("history" | "events" | "cost" | "term" | "models" | "limits" | "commands" | "codex-permissions")) || tail.is_some_and(|t| t.starts_with("pages/")) {
            return true;
        }
        if path.strip_prefix("/api/hangar-terminals/").is_some_and(|r| r.ends_with("/term")) {
            return true;
        }
    }
    // Interface dos mods: o Rust atende as sessões dele e repassa as outras (os contadores mostram).
    if *method == Method::POST {
        if path == "/api/claude/defaults" {
            return true;
        }
        let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
        if matches!(tail, Some("plugin/press" | "plugin/close" | "plugin/show" | "plugin/input")) || matches!(path.strip_prefix("/api/plugin/"),
            Some("press-start" | "opened" | "ui" | "toast" | "pressed" | "copied" | "focus-target" | "focused" | "scroll")) {
            return true;
        }
    }
    // Escritas Claude: o Rust decide por pedido; o que não é dele ele repassa (os contadores mostram).
    let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
    if *method == Method::POST && matches!(tail, Some("input" | "steer" | "interrupt" | "select" | "select/submit" | "answer" | "keys" | "term-input")) {
        return true;
    }
    // Rotas só do Codex: o Rust atende o Codex sem terminal dele e repassa o resto.
    if *method == Method::POST && matches!(tail, Some("model" | "service-tier" | "codex/mode" | "question/skip" | "codex-permissions")) {
        return true;
    }
    if *method == Method::DELETE && tail.and_then(|t| t.strip_prefix("queue/")).is_some_and(|id| !id.is_empty() && !id.contains('/')) {
        return true;
    }
    // Grupos (`groups::routes`): o convidado segue ao Python, que recusa.
    if matches!((method.as_str(), tail), ("POST", Some("pair" | "group-message" | "pair-remote" | "unpair-remote"))
        | ("DELETE", Some("pair")) | ("GET", Some("pair/contract"))) {
        return true;
    }
    crate::workspace_routes::matches(method, path) || crate::worktree_routes::matches(method, path)
}

/// Um ou mais pedidos reais por área, só para a tabela; a decisão é de `rust_route`.
const PROBES: &[(&str, &str)] = &[
    ("GET", "/api/sessions/x/history"), ("GET", "/api/sessions/x/events"),
    ("GET", "/api/sessions"), ("GET", "/api/sessions/events"), ("POST", "/api/sessions"),
    ("POST", "/api/sessions/x/input"), ("POST", "/api/sessions/x/interrupt"), ("POST", "/api/sessions/x/answer"),
    ("POST", "/api/sessions/x/steer"), ("POST", "/api/sessions/x/select"), ("POST", "/api/sessions/x/select/submit"),
    ("POST", "/api/sessions/x/term-input"), ("DELETE", "/api/sessions/x/queue/e1"),
    ("POST", "/api/sessions/x/model"), ("POST", "/api/sessions/x/rename"), ("DELETE", "/api/sessions/x"),
    ("GET", "/api/sessions/x/term"), ("POST", "/api/sessions/x/keys"),
    ("GET", "/api/sessions/x/git/log"), ("GET", "/api/sessions/x/files/list"), ("POST", "/api/sessions/x/git/commit"),
    ("POST", "/api/sessions/x/files/write"), ("GET", "/api/fs/roots"),
    ("GET", "/api/worktrees"), ("GET", "/api/worktrees/detail"), ("POST", "/api/worktrees/create"), ("POST", "/api/worktrees/delete"),
    ("GET", "/api/costs"), ("GET", "/api/uso"), ("GET", "/api/cotacao"), ("GET", "/api/sessions/x/cost"),
    ("GET", "/api/cotas"), ("GET", "/api/sessions/x/limits"),
    ("GET", "/api/codex-contas"), ("POST", "/api/sessions/x/codex-permissions"),
    ("GET", "/api/orq"), ("POST", "/api/sessions/x/pi"), ("POST", "/api/sessions/x/kimi"),
    ("GET", "/api/claude-configs"), ("GET", "/api/engines"), ("POST", "/api/sessions/x/conta"),
    ("GET", "/api/guests"), ("POST", "/api/sessions/x/share"),
    ("GET", "/api/peers"), ("POST", "/api/sessions/x/pair"),
    ("POST", "/mcp"),
    ("GET", "/api/push/settings"),
    ("GET", "/api/atualizacao"), ("GET", "/api/update-channel"),
    ("POST", "/api/sessions/x/upload"),
    ("POST", "/api/dictation/transcribe"),
    ("POST", "/api/sessions/x/plugin/press"), ("POST", "/api/sessions/x/plugin/close"), ("POST", "/api/plugin/ui"), ("POST", "/api/plugin/ask"),
    ("GET", "/"), ("GET", "/assets/index.js"),
    ("GET", "/api/config"),
];

struct Bucket {
    minute: u64,
    public: [[u32; 2]; AREAS.len()],
    private: [u32; PRIVATE.len()],
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
// ponytail: global; a tela sai na parte 7 junto com o contador.
static COUNTS: Mutex<VecDeque<Bucket>> = Mutex::new(VecDeque::new());

fn with_bucket(f: impl FnOnce(&mut Bucket)) {
    let minute = START.elapsed().as_secs() / 60;
    let mut counts = COUNTS.lock().unwrap();
    while counts.front().is_some_and(|b| b.minute + WINDOW_MINUTES <= minute) {
        counts.pop_front();
    }
    if counts.back().is_none_or(|b| b.minute != minute) {
        counts.push_back(Bucket { minute, public: [[0; 2]; AREAS.len()], private: [0; PRIVATE.len()] });
    }
    f(counts.back_mut().unwrap());
}

pub fn count_private(key: &str) {
    let Some(i) = PRIVATE.iter().position(|k| *k == key) else { return };
    with_bucket(|b| b.private[i] += 1);
}

/// Camada da porta pública: conta cada pedido na área dele, pelo Rust ou repassado.
pub async fn count_public(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let skip = path.starts_with("/__hangar_server/") || path == "/api/migration/status";
    let area = area_of(path);
    let resp = next.run(req).await;
    if !skip {
        let python = usize::from(resp.extensions().get::<Forwarded>().is_some());
        with_bucket(|b| b.public[area][python] += 1);
    }
    resp
}

/// Camada da porta privada: o que o Python pediu ao Rust por trás.
pub async fn count_bridge(req: Request, next: Next) -> Response {
    let key = match req.uri().path() {
        "/__hangar_server/terminal" => "terminal_observe",
        "/__hangar_server/workspace" => "workspace_bridge",
        "/__hangar_server/list" => "list_bridge",
        "/__hangar_server/pages" => "pages_bridge",
        path if path.starts_with("/__hangar_server/transcription/") => "transcription_bridge",
        _ => "",
    };
    count_private(key);
    next.run(req).await
}

fn totals() -> ([[u64; 2]; AREAS.len()], [u64; PRIVATE.len()]) {
    let mut public = [[0u64; 2]; AREAS.len()];
    let mut private = [0u64; PRIVATE.len()];
    with_bucket(|_| {});
    for b in COUNTS.lock().unwrap().iter() {
        for (sum, n) in public.iter_mut().zip(b.public) {
            sum[0] += u64::from(n[0]);
            sum[1] += u64::from(n[1]);
        }
        for (sum, n) in private.iter_mut().zip(b.private) {
            *sum += u64::from(n);
        }
    }
    (public, private)
}

fn areas_json(counts: &[[u64; 2]; AREAS.len()]) -> Value {
    AREAS.iter().enumerate().map(|(i, key)| {
        let routes: Vec<Value> = PROBES.iter()
            .filter(|(_, path)| area_of(path) == i)
            .map(|(method, path)| {
                let m = Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET);
                json!({"method": method, "path": path, "rust": rust_route(&m, path)})
            })
            .collect();
        json!({"key": key, "routes": routes, "rust": counts[i][0], "python": counts[i][1]})
    }).collect()
}

const FACTS_TTL: Duration = Duration::from_secs(2);
// Várias telas abertas (nativo, web, script) viram uma leitura do Python a cada 2 s, uma por vez.
// O erro também fica guardado: com o Python travado, a fila de telas recebe o código em vez de
// esperar o prazo uma atrás da outra.
static FACTS: LazyLock<tokio::sync::Mutex<Option<(Instant, Result<Value, &'static str>)>>> = LazyLock::new(Default::default);

async fn cached_python_facts(st: &AppState) -> Result<Value, &'static str> {
    let mut cache = FACTS.lock().await;
    if let Some((at, result)) = cache.as_ref() {
        if at.elapsed() < FACTS_TTL {
            return result.clone();
        }
    }
    let result = python_facts(st).await;
    if let Err(code) = result {
        st.diag.report("migration_status.python", "", code, "o backend não devolveu os fatos da migração");
    }
    *cache = Some((Instant::now(), result.clone()));
    result
}

async fn python_facts(st: &AppState) -> Result<Value, &'static str> {
    let req = axum::http::Request::get(format!("http://{}/internal/migration/status", st.cfg.upstream))
        .header("x-hangar-internal", &st.cfg.internal_secret)
        .body(Body::empty())
        .map_err(|_| "pedido")?;
    let resp = tokio::time::timeout(FACTS_TIMEOUT, st.http.request(req)).await
        .map_err(|_| "prazo")?
        .map_err(|_| "conexao")?;
    if !resp.status().is_success() {
        return Err("status");
    }
    let body = http_body_util::Limited::new(resp.into_body(), MAX_FACTS);
    let bytes = tokio::time::timeout(FACTS_TIMEOUT, body.collect()).await
        .map_err(|_| "prazo")?
        .map_err(|_| "tamanho")?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| "json")
}

/// `GET /api/migration/status` do dono. Convidado e outro método seguem ao Python, que recusa.
pub async fn status(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    if !owner || req.method() != Method::GET {
        return pass(&st, req, &fwd).await;
    }
    let (python, python_error) = match cached_python_facts(&st).await {
        Ok(v) => (v, Value::Null),
        Err(code) => (Value::Null, json!(code)),
    };
    let (public, private) = totals();
    let body = json!({
        "served_by": "rust",
        "rust": {
            "version": env!("CARGO_PKG_VERSION"),
            // O CI compila com o GITHUB_SHA no ambiente; build local fica sem.
            "commit": option_env!("GITHUB_SHA"),
            "protocol": crate::INTERNAL_PROTOCOL,
            "pid": std::process::id(),
        },
        "python": python,
        "python_error": python_error,
        "window_minutes": WINDOW_MINUTES,
        "areas": areas_json(&public),
        "private": PRIVATE.iter().zip(private).map(|(k, n)| json!({"key": k, "rust": n})).collect::<Vec<_>>(),
    });
    let mut resp = ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], body.to_string()).into_response();
    cors(req.headers(), resp.headers_mut());
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_area_has_a_probe_and_the_table_follows_the_router() {
        for (i, key) in AREAS.iter().enumerate() {
            assert!(PROBES.iter().any(|(_, p)| area_of(p) == i), "área sem pedido de exemplo: {key}");
        }
        let get = |p| rust_route(&Method::GET, p);
        assert!(get("/api/sessions/a/history") && get("/api/sessions") && get("/api/uso"));
        assert!(get("/api/sessions/a/git/log") && get("/api/worktrees"), "Git e worktrees vêm dos matches");
        assert!(get("/api/sessions/a/pages/p1") && get("/api/sessions/a/pages/p1/shot") && !get("/api/sessions/a/pages"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/input") && rust_route(&Method::DELETE, "/api/sessions/a/queue/e1")
            && !rust_route(&Method::POST, "/api/sessions/a/queue/e1") && get("/api/cotas") && !rust_route(&Method::POST, "/api/worktrees/create"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/pair") && rust_route(&Method::DELETE, "/api/sessions/a/pair")
            && get("/api/sessions/a/pair/contract") && rust_route(&Method::POST, "/api/sessions/a/unpair-remote")
            && !get("/api/sessions/a/pair") && !rust_route(&Method::POST, "/api/sessions/a/pair-invite"));
        assert_eq!(AREAS[area_of("/api/sessions/a/git/commit/abc/files")], "workspace");
        assert_eq!(AREAS[area_of("/api/sessions-x")], "other", "prefixo só casa por segmento inteiro");
        assert_eq!(AREAS[area_of("/assets/index.js")], "static");
        assert_eq!(AREAS[area_of("/api/sessions/a")], "session");
    }

    /// Rota nova no `routes::router` sem entrar em `rust_route` faria a tabela dizer "Python" para o
    /// que o Rust atende. O roteador não se deixa listar; o texto dele, sim.
    #[test]
    fn every_route_registered_in_the_router_is_known_to_the_table() {
        // O checkout do Windows pode trazer CRLF; o fim do roteador é procurado por "\n}\n".
        let source = include_str!("routes.rs").replace("\r\n", "\n");
        let body = &source[source.find("pub fn router(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let (mut seen, mut api) = (0, 0);
        for line in body.lines().map(str::trim).filter(|l| l.starts_with(".route(\"")) {
            seen += 1;
            let path = line.split('"').nth(1).unwrap().replace("{name}", "x");
            if !path.starts_with("/api/") { continue; }
            let method = if line.contains("post(") { Method::POST } else if line.contains("delete(") { Method::DELETE } else if line.contains("get(") { Method::GET }
                else { panic!("método que o leitor não conhece: {line}") };
            assert!(!line.contains(").get(") && !line.contains(").post("), "dois métodos numa rota: {line}");
            assert!(rust_route(&method, &path), "rota do roteador fora da tabela: {method} {path}");
            api += 1;
        }
        // Rota quebrada em várias linhas escaparia do filtro acima: a contagem bruta a denuncia.
        assert_eq!(seen, body.matches(".route(").count(), "há `.route(` fora de uma linha `.route(\"…\"`");
        assert!(api >= 10, "o leitor do roteador não achou as rotas ({api})");
    }

    #[test]
    fn counts_split_rust_and_python_by_area() {
        let before = totals().0[area("dictation")];
        with_bucket(|b| { b.public[area("dictation")][0] += 2; b.public[area("dictation")][1] += 1; });
        let after = totals().0[area("dictation")];
        assert_eq!([after[0] - before[0], after[1] - before[1]], [2, 1]);
    }
}
