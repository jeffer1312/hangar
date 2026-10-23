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
pub const PRIVATE: [&str; 7] = ["send_headless", "send_terminal", "terminal_observe", "workspace_bridge", "list_bridge", "transcription_bridge", "dictation_bridge"];

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
    let table: [(&[&str], &str); 18] = [
        (&["/api/sessions"], "list"),
        (&["/api/jev/delivery"], "send"),
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
    if matches!(*method, Method::GET | Method::PUT) && path == "/api/session-defaults" { return true; }
    if *method == Method::POST && path == "/api/push/expo/subscribe" { return true; }
    if crate::orq_conductor::matches(method, path) { return true; }
    if crate::prewarm::http::matches(method, path) { return true; }
    if crate::session_media::is_media(method, path) { return true; }
    if crate::accounts::http::matches(method, path) { return true; }
    // SideStore no aparelho do cabo (`device_install::routes`): nunca repassado.
    if *method == Method::GET && matches!(path, "/api/device-install/devices" | "/api/device-install/progress") { return true; }
    if *method == Method::POST && matches!(path, "/api/device-install/trust" | "/api/device-install/start" | "/api/device-install/code" | "/api/device-install/certs") { return true; }
    if *method == Method::GET {
        if matches!(path, "/api/sessions" | "/api/sessions/events" | "/api/sessions/creation-progress" | "/api/costs" | "/api/cotacao" | "/api/uso" | "/api/migration/status" | "/api/presence"
            | "/api/voice" | "/api/voice/settings" | "/api/voice/memory" | "/api/jev/aux/cliproxy" | "/api/jev/delivery" | "/api/client-api") {
            return true;
        }
        let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
        // `term`: o painel abre no Rust; login de convidado e Connect passam pelo Python e voltam ao mesmo PTY.
        if matches!(tail, Some("history" | "events" | "artifacts" | "cost" | "term" | "models" | "limits" | "commands" | "codex-permissions" | "conta")) || tail.is_some_and(|t| t.starts_with("pages/")) {
            return true;
        }
        if tail.is_some_and(|t| t == "subagents" || t.strip_prefix("subagents/").is_some_and(|id| !id.is_empty() && !id.contains('/'))) {
            return true;
        }
        if path.strip_prefix("/api/hangar-terminals/").is_some_and(|r| r.ends_with("/term")) {
            return true;
        }
    }
    if *method == Method::PUT && path == "/api/voice/settings" {
        return true;
    }
    if *method == Method::DELETE && path.starts_with("/api/voice/memory/") {
        return true;
    }
    // Interface dos mods: o Rust atende as sessões dele e repassa as outras (os contadores mostram).
    if *method == Method::POST {
        if matches!(path, "/api/sessions" | "/api/claude/defaults" | "/api/presence" | "/api/presence/heartbeat" | "/api/jev/aux/models" | "/api/mermaid") {
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
    // Texto a um subagente em segundo plano: o plugin do Claude entrega pelo `/pull`.
    if *method == Method::POST && tail.and_then(|t| t.strip_prefix("subagents/")).and_then(|t| t.strip_suffix("/message"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/')) {
        return true;
    }
    // Rotas só do Codex, modos de permissão do Claude e as reaberturas (troca de modo, recarregar, conta,
    // conta ChatGPT fixa): o Rust atende as sessões dele.
    if *method == Method::POST && matches!(tail, Some("model" | "service-tier" | "codex/mode" | "codex/plan/implement" | "question/skip" | "codex-permissions"
        | "permission-mode" | "modo-execucao" | "recarregar" | "conta" | "engine/model" | "model-effort" | "bastao" | "resume" | "rename")) {
        return true;
    }
    // A pergunta lateral do Claude, com e sem terminal, e o histórico dela.
    if matches!(*method, Method::POST | Method::GET) && tail == Some("btw") {
        return true;
    }
    // Retomar do Arquivo: a abertura do Rust com a conversa que já existe.
    if *method == Method::POST && path.strip_prefix("/api/archive/").and_then(|r| r.strip_suffix("/resume"))
        .is_some_and(|r| r.split('/').count() == 2) {
        return true;
    }
    if *method == Method::GET && tail == Some("permission-modes") {
        return true;
    }
    // Encerrar (o Codex com terminal fecha aqui antes da limpeza do Python), broadcast e a ponte do plugin.
    if *method == Method::DELETE && path.strip_prefix("/api/sessions/").is_some_and(|name| !name.is_empty() && !name.contains('/')) {
        return true;
    }
    if *method == Method::POST && (path == "/api/broadcast" || matches!(path.strip_prefix("/api/plugin/"),
        Some("whoami" | "pull" | "ask" | "ask-fim" | "plan" | "filled" | "submitted" | "state" | "suggest" | "rate" | "agent" | "agent-sent"))) {
        return true;
    }
    if tail.is_some_and(|t| crate::session_write::control::queue_route(method, t).is_some()) {
        return true;
    }
    // Grupos (`groups::routes`): o convidado segue ao Python, que recusa.
    if matches!((method.as_str(), tail), ("POST", Some("pair" | "group-message" | "pair-remote" | "unpair-remote"))
        | ("DELETE", Some("pair")) | ("GET", Some("pair/contract"))) {
        return true;
    }
    (*method == Method::GET && path == crate::claude_customizations::CATALOG_PATH)
        || crate::plans::routes::matches(method, path) || crate::share_reach::matches(method, path) || share_route(method, path)
        || crate::workspace_routes::matches(method, path) || crate::worktree_routes::matches(method, path)
}

/// Convites e par externo do dono (`share::owner_routes`, `share::pair_routes`).
fn share_route(method: &Method, path: &str) -> bool {
    let tail = path.strip_prefix("/api/sessions/").and_then(|r| r.split_once('/')).map(|(_, t)| t);
    match method.as_str() {
        "GET" => matches!(path, "/api/share/prereqs" | "/api/external-pairs") || tail == Some("share"),
        "POST" => path == "/api/external-pairs/send" || matches!(tail, Some("share" | "pair-invite" | "pair-accept" | "pair-access")),
        "DELETE" => tail.is_some_and(|t| t == "share" || t.strip_prefix("share/").is_some_and(|id| !id.is_empty() && !id.contains('/')))
            || path.strip_prefix("/api/external-pairs/").is_some_and(|alias| !alias.is_empty() && !alias.contains('/')),
        _ => false,
    }
}

/// Um ou mais pedidos reais por área, só para a tabela; a decisão é de `rust_route`.
const PROBES: &[(&str, &str)] = &[
    ("GET", "/api/sessions/x/history"), ("GET", "/api/sessions/x/events"),
    ("GET", "/api/sessions"), ("GET", "/api/sessions/events"), ("POST", "/api/sessions"),
    ("POST", "/api/sessions/x/input"), ("POST", "/api/sessions/x/interrupt"), ("POST", "/api/sessions/x/answer"),
    ("POST", "/api/sessions/x/steer"), ("POST", "/api/sessions/x/select"), ("POST", "/api/sessions/x/select/submit"),
    ("POST", "/api/sessions/x/term-input"), ("DELETE", "/api/sessions/x/queue/e1"),
    ("PATCH", "/api/sessions/x/queue/e1"), ("POST", "/api/sessions/x/queue/reorder"), ("POST", "/api/sessions/x/queue/e1/steer"),
    ("POST", "/api/sessions/x/model"), ("POST", "/api/sessions/x/rename"), ("DELETE", "/api/sessions/x"),
    ("GET", "/api/sessions/x/term"), ("POST", "/api/sessions/x/keys"),
    ("GET", "/api/sessions/x/git/log"), ("GET", "/api/sessions/x/files/list"), ("POST", "/api/sessions/x/git/commit"),
    ("POST", "/api/sessions/x/files/write"), ("GET", "/api/fs/roots"),
    ("GET", "/api/worktrees"), ("GET", "/api/worktrees/detail"), ("GET", "/api/worktrees/settings"), ("PUT", "/api/worktrees/settings"), ("POST", "/api/worktrees/delete-merged"), ("POST", "/api/worktrees/create"), ("POST", "/api/worktrees/delete"),
    ("GET", "/api/costs"), ("GET", "/api/uso"), ("GET", "/api/cotacao"), ("GET", "/api/sessions/x/cost"),
    ("GET", "/api/cotas"), ("GET", "/api/sessions/x/limits"),
    ("GET", "/api/codex-contas"), ("POST", "/api/sessions/x/codex-permissions"),
    ("GET", "/api/orq"), ("GET", "/api/orq/x/conductor"), ("GET", "/api/sessions/x/pi/models"), ("POST", "/api/sessions/x/pi/model"),
    ("GET", "/api/sessions/x/kimi/models"), ("POST", "/api/sessions/x/kimi/model"), ("GET", "/api/omp/plugin-sync"),
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
        path if path.starts_with("/__hangar_server/transcription/") => "transcription_bridge",
        path if path.starts_with("/__hangar_server/dictation/") => "dictation_bridge",
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
        assert!(rust_route(&Method::POST, "/api/mermaid") && !get("/api/mermaid"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/input") && rust_route(&Method::DELETE, "/api/sessions/a/queue/e1")
            && !rust_route(&Method::POST, "/api/sessions/a/queue/e1") && rust_route(&Method::POST, "/api/sessions/a/queue/e1/steer")
            && rust_route(&Method::PATCH, "/api/sessions/a/queue/e1") && get("/api/cotas") && rust_route(&Method::POST, "/api/worktrees/create"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/pair") && rust_route(&Method::DELETE, "/api/sessions/a/pair")
            && get("/api/sessions/a/pair/contract") && rust_route(&Method::POST, "/api/sessions/a/unpair-remote")
            && !get("/api/sessions/a/pair") && rust_route(&Method::POST, "/api/sessions/a/pair-invite"));
        assert!(rust_route(&Method::POST, "/api/sessions/a/share") && rust_route(&Method::DELETE, "/api/sessions/a/share/s1")
            && get("/api/share/prereqs") && rust_route(&Method::POST, "/api/share/reachability")
            && !rust_route(&Method::DELETE, "/api/sessions/a/share/s1/x"));
        // O corte: relançar e recarregar (Claude e Codex), trocar de conta, modos do Claude, encerrar, broadcast e
        // a ponte do plugin são do Rust.
        let post = |p: &str| rust_route(&Method::POST, p);
        assert!(post("/api/sessions/a/modo-execucao") && post("/api/sessions/a/recarregar") && post("/api/sessions/a/conta") && get("/api/sessions/a/conta"));
        assert!(post("/api/sessions/a/permission-mode")
            && get("/api/sessions/a/permission-modes") && rust_route(&Method::DELETE, "/api/sessions/a") && post("/api/broadcast"));
        assert!(post("/api/sessions/a/subagents/x1/message") && !post("/api/sessions/a/subagents//message") && !get("/api/sessions/a/subagents/x1/message"));
        assert!(post("/api/sessions/a/transcribe") && post("/api/sessions/a/bash-output") && get("/api/sessions/a/tool-progress/toolu_x")
            && get("/api/sessions/a/transcript-image/u/0") && !get("/api/sessions/a/bash-output"));
        assert!(["whoami", "pull", "ask", "ask-fim", "filled", "submitted", "state", "suggest", "rate", "agent", "agent-sent"].iter().all(|e| post(&format!("/api/plugin/{e}"))));
        assert!(!rust_route(&Method::DELETE, "/api/sessions") && !post("/api/plugin/outra"));
        assert!(get("/api/sessions/a/plan") && get("/api/sessions/a/plans") && get("/api/sessions/a/plan-preview")
            && get("/api/claude/customizations") && rust_route(&Method::POST, "/api/sessions/a/plan-step")
            && !rust_route(&Method::POST, "/api/sessions/a/plan") && !get("/api/sessions/a/plan-step"));
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
        // As rotas de sessão moram em `session_routes`, montadas pelas duas portas; o resto, em `public_routes`.
        let fn_body = |name: &str| {
            let body = &source[source.find(name).unwrap()..];
            body[..body.find("\n}\n").unwrap()].to_owned()
        };
        let body = fn_body("fn public_routes(") + "\n" + &fn_body("fn session_routes<");
        let body = body.as_str();
        let (mut seen, mut api) = (0, 0);
        for line in body.lines().map(str::trim).filter(|l| l.starts_with(".route(\"")) {
            seen += 1;
            let path = line.split('"').nth(1).unwrap().replace("{name}", "x");
            if !path.starts_with("/api/") { continue; }
            // Uma rota pode juntar métodos (`get(…).post(…)`): cada um tem de estar na tabela.
            let methods:Vec<Method> = [("get(",Method::GET),("post(",Method::POST),("delete(",Method::DELETE),("put(",Method::PUT),("patch(",Method::PATCH)]
                .into_iter().filter(|(call,_)| line.contains(&format!("::{call}")) || line.contains(&format!(" {call}")) || line.contains(&format!(".{call}")) || line.contains(&format!("({call}")))
                .map(|(_,method)| method).collect();
            assert!(!methods.is_empty(), "método que o leitor não conhece: {line}");
            for method in methods { assert!(rust_route(&method, &path), "rota do roteador fora da tabela: {method} {path}"); }
            api += 1;
        }
        // Rota quebrada em várias linhas escaparia do filtro acima: a contagem bruta a denuncia.
        assert_eq!(seen, body.matches(".route(").count(), "há `.route(` fora de uma linha `.route(\"…\"`");
        assert!(api >= 10, "o leitor do roteador não achou as rotas ({api})");
        // Montadas por `share::pair_routes`, fora do texto lido acima.
        assert!(rust_route(&Method::POST, "/api/sessions/a/pair-access"));
        assert!(rust_route(&Method::DELETE, "/api/external-pairs/ana"));
        assert!(!rust_route(&Method::DELETE, "/api/external-pairs/a/b"));
    }

    #[test]
    fn counts_split_rust_and_python_by_area() {
        let before = totals().0[area("dictation")];
        with_bucket(|b| { b.public[area("dictation")][0] += 2; b.public[area("dictation")][1] += 1; });
        let after = totals().0[area("dictation")];
        assert_eq!([after[0] - before[0], after[1] - before[1]], [2, 1]);
    }
}
