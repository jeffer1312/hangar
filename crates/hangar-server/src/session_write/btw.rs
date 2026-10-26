//! Pergunta lateral (`POST|GET /api/sessions/{name}/btw`), toda no Rust. Com e sem terminal, a
//! pergunta vira um fork descartável da conversa (`claude -p --resume … --fork-session`), sem tocar o
//! pane. O histórico fica no mesmo arquivo do Python.
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::Arc, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use serde_json::{Value, json};

use crate::routes::{AppState, cors, pass, who};
use crate::runtime::gateway::EntryHandle;
use super::{BODY_LIMIT, busy_body, detail, enter_then_find, json_response, session_name};

/// Recusa com o status HTTP e o código que o app já traduz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtwError { pub status: u16, pub code: String, pub message: String }
pub fn fail(status: u16, code: &str, message: &str) -> BtwError { BtwError { status, code: code.into(), message: message.into() } }

/// O `isspace` do Python: o Rust não conta os separadores U+001C–U+001F.
fn py_space(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }
/// `" ".join(pergunta.split())`.
pub fn normalize(question: &str) -> String { question.split(py_space).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ") }
fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

pub const HISTORY_MAX: usize = 50;
const QUESTION_MAX: usize = 4000;
/// Prazo generoso: o fork lê o contexto inteiro antes de responder, e não segura nada da sessão.
pub const FORK_TIMEOUT: Duration = Duration::from_secs(180);
// O fork é uma PERGUNTA, não um turno de trabalho: nada que aja sai daqui; as de leitura ficam.
// `--permission-mode plan` emenda "quer que eu chame ExitPlanMode?" na resposta, `--disallowed-tools "*"`
// tira até a leitura (o modelo inventa chamadas no texto) e `default` não guarda: o allow-list da conta vale.
// ponytail: lista de nomes, então ferramenta de ação NOVA entra permitida até alguém somar aqui.
const FORK_DENIED: &str = "Bash BashOutput KillShell Edit Write NotebookEdit Task TodoWrite WebFetch WebSearch SlashCommand mcp__*";

// ── Corpo ────────────────────────────────────────────────────────────────────────────────────────

fn issue(kind: &str, loc: Value, msg: &str, input: &Value, ctx: Option<Value>) -> Value {
    let mut item = json!({"type": kind, "loc": loc, "msg": msg, "input": input});
    if let Some(ctx) = ctx { item["ctx"] = ctx; }
    item
}

/// O `BtwBody` do Python (`question` de 1 a 4000 caracteres, nada a mais), com o 422 do FastAPI.
/// A pergunta volta crua: a normalização é de quem a usa.
pub fn parse_body(bytes: &[u8]) -> Result<String, Value> {
    let parsed: Value = match serde_json::from_slice(&super::input::scrub_lone_surrogates(bytes)) {
        Ok(value) => value,
        Err(error) => return Err(json!({"detail": [issue("json_invalid", json!(["body", error.column()]), "JSON decode error", &json!({}), Some(json!({"error": error.to_string()})))]})),
    };
    let Value::Object(fields) = &parsed else {
        return Err(json!({"detail": [issue("model_attributes_type", json!(["body"]), "Input should be a valid dictionary or object to extract fields from", &parsed, None)]}));
    };
    let mut issues = Vec::new();
    match fields.get("question") {
        None => issues.push(issue("missing", json!(["body", "question"]), "Field required", &parsed, None)),
        Some(Value::String(q)) if q.chars().count() < 1 =>
            issues.push(issue("string_too_short", json!(["body", "question"]), "String should have at least 1 character", &json!(q), Some(json!({"min_length": 1})))),
        Some(Value::String(q)) if q.chars().count() > QUESTION_MAX =>
            issues.push(issue("string_too_long", json!(["body", "question"]), "String should have at most 4000 characters", &json!(q), Some(json!({"max_length": QUESTION_MAX})))),
        Some(Value::String(_)) => {}
        Some(other) => issues.push(issue("string_type", json!(["body", "question"]), "Input should be a valid string", other, None)),
    }
    for (key, value) in fields.iter().filter(|(k, _)| *k != "question") {
        issues.push(issue("extra_forbidden", json!(["body", key]), "Extra inputs are not permitted", value, None));
    }
    if !issues.is_empty() { return Err(json!({"detail": issues})); }
    Ok(fields["question"].as_str().unwrap_or_default().to_owned())
}

// ── Histórico: `<projects_dir>/../.hangar-btw/<nome>.jsonl`, as últimas 50 ─────────────────────────

/// O `pqueue._sanitize` do Python: o nome do arquivo é o mesmo.
fn file_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '-' }).collect()
}
fn history_file(claude_dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = claude_dir.join(".hangar-btw");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{}.jsonl", file_name(name))))
}
fn lines(path: &Path) -> std::io::Result<Vec<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.split('\n').filter(|l| !l.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)).is_empty()).map(str::to_owned).collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error),
    }
}

/// O `json.dumps(item, ensure_ascii=False)` do Python: `", "` e `": "` entre os campos.
struct PyFormatter;
impl serde_json::ser::Formatter for PyFormatter {
    fn begin_array_value<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()> { if first { Ok(()) } else { writer.write_all(b", ") } }
    fn begin_object_key<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()> { if first { Ok(()) } else { writer.write_all(b", ") } }
    fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> { writer.write_all(b": ") }
}
pub fn python_json(value: &Value) -> String {
    use serde::Serialize;
    let mut out = Vec::new();
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, PyFormatter)).expect("Value sempre serializa");
    String::from_utf8(out).expect("serde escreve UTF-8")
}

/// Guarda a pergunta respondida. `Err` só com arquivo existente ilegível (UTF-8 inválido), que o Python
/// também não tratava; falha de disco é `Ok(false)`: a resposta já foi entregue.
pub fn record(claude_dir: &Path, name: &str, item: &Value) -> Result<bool, std::io::Error> {
    let path = match history_file(claude_dir, name) { Ok(path) => path, Err(_) => return Ok(false) };
    let mut kept = match lines(&path) {
        Ok(kept) => kept,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => return Err(error),
        Err(_) => return Ok(false),
    };
    kept.push(python_json(item));
    let start = kept.len().saturating_sub(HISTORY_MAX);
    Ok(std::fs::write(&path, kept[start..].join("\n") + "\n").is_ok())
}

/// As últimas 50, pulando linha que não é JSON. Sem guarda e sem conferir a sessão, como no Python.
pub fn history(claude_dir: &Path, name: &str) -> std::io::Result<Vec<Value>> {
    let items: Vec<Value> = lines(&history_file(claude_dir, name)?)?.iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let start = items.len().saturating_sub(HISTORY_MAX);
    Ok(items[start..].to_vec())
}

// ── Sem terminal: a pergunta é um fork descartável da conversa ──────────────────────────────────

/// Os programas do fork; os testes trocam por falsos.
#[derive(Clone, Debug)]
pub struct Programs { pub claude: PathBuf, pub engine: PathBuf }
impl Default for Programs { fn default() -> Self { Self { claude: "claude".into(), engine: "hangar-engine".into() } } }

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
fn text(value: &Value) -> String { value.as_str().map_or_else(|| value.to_string(), str::to_owned) }

/// O argv do fork com os nomes lógicos `claude` e `hangar-engine`.
pub fn fork_argv(meta: &Value, sid: &str, question: &str) -> Vec<String> {
    let mut argv: Vec<String> = ["claude", "-p", "--resume", &text(&meta["session_id"]), "--fork-session", "--session-id", sid,
        "--disallowed-tools", FORK_DENIED, "--output-format", "json"].map(str::to_owned).into();
    let model = truthy(meta.get("model")).then(|| text(&meta["model"]));
    if let Some(model) = &model { argv.extend(["--model".into(), model.clone()]); }
    if truthy(meta.get("engine")) {
        let mut pre = vec!["hangar-engine".to_owned(), "--exec".into(), text(&meta["engine"])];
        if let Some(model) = &model {
            pre.extend(["--model".into(), model.clone()]);
            if truthy(meta.get("context_window")) { pre.extend(["--context".into(), text(&meta["context_window"])]); }
        }
        pre.push("--".into());
        argv = pre.into_iter().chain(argv).collect();
    }
    // `--` antes da pergunta: ela é texto de quem usa o app, e a CLI lê o último argumento como OPÇÃO
    // quando ele casa com uma flag — inclusive as que derrubam o `--disallowed-tools`.
    argv.extend(["--".into(), question.into()]);
    argv
}

/// A resposta do `--output-format json`; `is_error` vem com código 0 e o motivo no `result`.
pub fn fork_answer(stdout: &[u8]) -> Result<String, BtwError> {
    let parsed: Option<Value> = serde_json::from_slice(String::from_utf8_lossy(stdout).as_bytes()).ok();
    if let Some(Value::Object(fields)) = &parsed && truthy(fields.get("is_error")) {
        tracing::warn!(subtype=%fields.get("subtype").map(text).unwrap_or_default(), "btw sem terminal: o claude devolveu erro");
        return Err(fail(502, "erro_btw_fork_falhou", "a pergunta lateral falhou"));
    }
    let answer = match &parsed { Some(Value::Object(fields)) => fields.get("result").and_then(Value::as_str).unwrap_or_default(), _ => "" };
    let answer = answer.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
    if answer.is_empty() { return Err(fail(502, "erro_btw_ilegivel", "a pergunta lateral respondeu, mas não consegui ler a resposta")); }
    Ok(answer.to_owned())
}

/// `<base>/<cwd>/<sid>.jsonl`, ou onde o `EnterWorktree` o levou (o sid não repete entre pastas).
fn transcript(base: &Path, cwd: &str, sid: &str) -> PathBuf {
    let file = format!("{sid}.jsonl");
    let expected = base.join(hangar_workspace::worktrees::sanitize_cwd(cwd)).join(&file);
    if expected.exists() { return expected; }
    let mut moved: Vec<PathBuf> = std::fs::read_dir(base).into_iter().flatten().flatten().map(|d| d.path().join(&file)).filter(|p| p.exists()).collect();
    moved.sort();
    moved.into_iter().next().unwrap_or(expected)
}

async fn run_fork(argv: &[String], programs: &Programs, cwd: &str, config_dir: Option<&str>, timeout: Duration) -> Result<(bool, Vec<u8>), BtwError> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let program = if argv[0] == "hangar-engine" { &programs.engine } else { &programs.claude };
    let mut command = tokio::process::Command::new(program);
    command.args(&argv[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
    if let Some(dir) = config_dir { command.env("CLAUDE_CONFIG_DIR", dir); }
    let spawn_failed = || fail(502, "erro_btw_fork_falhou", "não consegui abrir a pergunta lateral");
    let mut tree = crate::terminal_process::CommandTree::configure(&mut command).map_err(|_| spawn_failed())?;
    let mut child = command.spawn().map_err(|error| {
        tracing::warn!(kind=?error.kind(), "btw sem terminal: não consegui rodar o claude");
        spawn_failed()
    })?;
    let _ = tree.attach(&child);
    let mut stdout = child.stdout.take().expect("stdout com pipe");
    let work = async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status.success(), bytes))
    };
    let result = tokio::time::timeout(timeout, work).await;
    crate::terminal_process::finish(&mut tree).await;
    match result {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(_)) => Err(fail(502, "erro_btw_fork_falhou", "a pergunta lateral falhou")),
        Err(_) => Err(fail(504, "erro_btw_sem_resposta", "a pergunta lateral não respondeu a tempo")),
    }
}

/// Responde no contexto da conversa sem entrar nela. `--fork-session` preserva o transcript original;
/// o que nasce é outro, apagado no fim (senão apareceria na lista de conversas para retomar). O fork vê
/// o transcript GRAVADO, não o turno em voo.
pub async fn ask_headless(meta: &Value, question: &str, projects: &Path, programs: &Programs, timeout: Duration) -> Result<Value, BtwError> {
    let question = normalize(question);
    if question.is_empty() { return Err(fail(400, "erro_btw_vazia", "pergunta vazia")); }
    let no_conversation = |status| fail(status, "erro_btw_sem_conversa", "esta sessão ainda não tem conversa pra perguntar");
    if !truthy(meta.get("session_id")) { return Err(no_conversation(404)); }
    let cwd = meta["cwd"].as_str().unwrap_or_default().to_owned();
    let config_dir = truthy(meta.get("config_dir")).then(|| text(&meta["config_dir"]));
    let base = config_dir.as_ref().map_or_else(|| projects.to_owned(), |dir| Path::new(dir).join("projects"));
    // Sessão que nasceu e nunca conversou: o `--resume` falharia com "No conversation found".
    let (session_id, base_check, cwd_check) = (text(&meta["session_id"]), base.clone(), cwd.clone());
    if !tokio::task::spawn_blocking(move || transcript(&base_check, &cwd_check, &session_id).exists()).await.unwrap_or(false) {
        return Err(no_conversation(409));
    }
    let sid = crate::session_open::open::uuid4(&crate::mods::state::random_hex(16));
    let argv = fork_argv(meta, &sid, &question);
    let ran = run_fork(&argv, programs, &cwd, config_dir.as_deref(), timeout).await;
    let (base_clean, cwd_clean, sid_clean) = (base.clone(), cwd.clone(), sid.clone());
    let removed = tokio::task::spawn_blocking(move || {
        match std::fs::remove_file(transcript(&base_clean, &cwd_clean, &sid_clean)) { Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e), _ => Ok(()) }
    }).await;
    // Apagar é limpeza, não o resultado: no Windows um neto do motor ainda com o arquivo aberto recusa.
    if !matches!(removed, Ok(Ok(()))) { tracing::warn!("btw sem terminal: sobrou o transcript do fork"); }
    let (success, stdout) = ran?;
    if !success {
        // A saída do CLI fica fora do log: ela transcreve a conversa e pode carregar credencial.
        tracing::warn!("btw sem terminal: o claude saiu com erro");
        return Err(fail(502, "erro_btw_fork_falhou", "a pergunta lateral falhou"));
    }
    let answer = fork_answer(&stdout)?;
    Ok(json!({"question": question, "answer": answer, "fonte": "fork", "ts": now()}))
}

// ── Rotas ─────────────────────────────────────────────────────────────────────────────────────────

/// `<projects_dir>/..`: a pasta da conta padrão que a lista conhece.
fn claude_dir(st: &AppState) -> Option<PathBuf> { st.list.env().dirs.as_ref().map(|dirs| dirs.claude.clone()) }

fn refusal(error: &BtwError) -> (StatusCode, Value) {
    (StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY), super::detail_body(&error.code, &error.message, json!({})))
}

enum Mode { Terminal, Headless }

/// Quem responde a pergunta, pelo provedor e pelo terminal da sessão; `Err` com status e código.
#[derive(Debug, PartialEq, Eq)]
pub enum Route { Terminal, Headless, Unowned }
pub fn route(provider: &str, terminal: bool) -> Result<Route, (StatusCode, &'static str)> {
    // O /btw é da TUI do Claude Code: Codex, Pi, omp e Kimi não têm o comando nem o overlay.
    if provider != "claude" { return Err((StatusCode::BAD_REQUEST, "erro_btw_so_claude")); }
    Ok(if terminal { Route::Terminal } else { Route::Headless })
}
/// Sem executor aberto: a linha da lista; Claude com terminal sem executor não tem quem digite.
pub fn route_of_row(row: Option<&hangar_api::session::SessionRow>) -> Result<Route, (StatusCode, &'static str)> {
    let Some(row) = row else { return Err((StatusCode::NOT_FOUND, "erro_sessao_inexistente")) };
    match route(&row.provider, !row.headless)? { Route::Terminal => Ok(Route::Unowned), other => Ok(other) }
}
fn route_message(code: &str) -> &'static str {
    match code {
        "erro_btw_so_claude" => "pergunta lateral só existe em sessão Claude",
        "erro_sessao_inexistente" => "sessão não encontrada",
        _ => "o terminal desta sessão ainda não está com o servidor; tente de novo",
    }
}

pub async fn ask(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, caller) = who(&st, peer, &req);
    let Some(name) = session_name(req.uri().path()).filter(|n| caller.as_ref().is_some_and(|c| c.can(n, req.method()))) else {
        return pass(&st, req, &fwd).await;
    };
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    let answer = |status: StatusCode, body: Value| { let mut response = json_response(status, body); cors(&headers, response.headers_mut()); response };
    let Some(runtime) = st.state.runtime.get().cloned() else {
        return answer(StatusCode::SERVICE_UNAVAILABLE, super::detail_body("btw_runtime_unavailable", "o servidor ainda não abriu as sessões", json!({})));
    };
    // Como o `_transfer_guard` do Python: a porta da sessão antes do corpo, e segura até o fim.
    let found = match enter_then_find(&runtime, &name, st.write_gate_wait).await {
        Err(_) => return answer(StatusCode::CONFLICT, busy_body()),
        Ok(Some(found)) => Some(found),
        Ok(None) => match runtime.adopt_now(&name).await {
            Some(Ok(())) => match enter_then_find(&runtime, &name, st.write_gate_wait).await { Err(_) => return answer(StatusCode::CONFLICT, busy_body()), Ok(found) => found },
            Some(Err(error)) => return answer(super::failed_status(&error), super::detail_body(&error.code, &error.message, json!({}))),
            None => None,
        },
    };
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return super::too_large(&headers) };
    let question = match parse_body(&bytes) { Ok(question) => question, Err(body) => return answer(StatusCode::UNPROCESSABLE_ENTITY, body) };
    let refused = |(status, code): (StatusCode, &'static str)| answer(status, super::detail_body(code, route_message(code), json!({})));
    let (_held, mode) = match found {
        Some((held, target)) => match (route(&target.provider, target.terminal), target.handle) {
            (Err(refusal), _) => return refused(refusal),
            (Ok(Route::Terminal), EntryHandle::Terminal { .. }) => (Some(held), Mode::Terminal),
            (Ok(Route::Headless), _) => (Some(held), Mode::Headless),
            _ => return refused((StatusCode::SERVICE_UNAVAILABLE, "btw_terminal_unowned")),
        },
        // Sem executor aberto: a lista diz se a sessão existe, e de quem ela é.
        None => match tokio::time::timeout(crate::locate::LOCATE_TIMEOUT, st.list.find_row(&name)).await {
            Ok(Ok(row)) => match route_of_row(row.as_ref()) {
                Ok(Route::Headless) => (None, Mode::Headless),
                Ok(_) => return refused((StatusCode::SERVICE_UNAVAILABLE, "btw_terminal_unowned")),
                Err(refusal) => return refused(refusal),
            },
            Ok(Err(error)) => return answer(StatusCode::SERVICE_UNAVAILABLE, super::detail_body(error.code, error.detail, json!({}))),
            Err(_) => return answer(StatusCode::SERVICE_UNAVAILABLE, super::detail_body("locate_timeout", "a lista de sessões não respondeu a tempo", json!({}))),
        },
    };
    let projects = claude_dir(&st).unwrap_or_else(|| runtime.codex_environment().claude_base).join("projects");
    let item = match mode {
        // Digitado no pane, o /btw cai no plugin (que o responde no painel dele) e a rota não acha o overlay.
        Mode::Terminal => match runtime.claude_fork_meta(&name).await {
            Ok(meta) => ask_headless(&meta, &question, &projects, &st.btw_programs, FORK_TIMEOUT).await,
            Err(error) => Err(fail(503, &error.code, &error.message)),
        },
        Mode::Headless => {
            let path = crate::runtime::codex_sidecar::path(&runtime.codex_environment().home.join(".hangar").join("claude-headless"), &name);
            let meta = match tokio::task::spawn_blocking(move || crate::runtime::codex_sidecar::read(&path)).await {
                Ok(Ok(Some(meta))) => Value::Object(meta),
                other => {
                    if !matches!(other, Ok(Ok(None))) { tracing::warn!(session=%name, "btw sem terminal: sidecar ilegível"); }
                    Value::Null
                }
            };
            // Sem sidecar o `meta` é nulo: a pergunta vazia e o 404 saem do mesmo lugar.
            ask_headless(&meta, &question, &projects, &st.btw_programs, FORK_TIMEOUT).await
        }
    };
    let mut item = match item { Ok(item) => item, Err(error) => { let (status, body) = refusal(&error); return answer(status, body); } };
    // A TUI já respondeu e gastou a chamada: falha ao guardar não pode virar erro e levar a perguntar de novo.
    let dir = projects.parent().map(Path::to_owned).unwrap_or_default();
    let (stored, history_name) = (item.clone(), name.clone());
    let saved = match tokio::task::spawn_blocking(move || record(&dir, &history_name, &stored)).await {
        Ok(Ok(saved)) => saved,
        Ok(Err(_)) => return answer(StatusCode::INTERNAL_SERVER_ERROR, super::detail_body("erro_btw_historico_ilegivel", "o histórico da pergunta lateral está ilegível", json!({}))),
        Err(_) => false,
    };
    if !saved { tracing::warn!(session=%name, "btw: resposta entregue, histórico não gravado"); }
    item["salvo"] = json!(saved);
    answer(StatusCode::OK, item)
}

pub async fn list(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, caller) = who(&st, peer, &req);
    let Some(name) = session_name(req.uri().path()).filter(|n| caller.as_ref().is_some_and(|c| c.can(n, req.method()))) else {
        return pass(&st, req, &fwd).await;
    };
    let headers = req.headers().clone();
    let Some(dir) = claude_dir(&st).or_else(|| st.state.runtime.get().map(|r| r.codex_environment().claude_base)) else {
        let mut response = detail(StatusCode::SERVICE_UNAVAILABLE, "list_dirs_missing", "pastas da lista ausentes", json!({}));
        cors(&headers, response.headers_mut());
        return response;
    };
    let mut response = match tokio::task::spawn_blocking(move || history(&dir, &name)).await {
        Ok(Ok(items)) => json_response(StatusCode::OK, Value::Array(items)),
        _ => json_response(StatusCode::INTERNAL_SERVER_ERROR, super::detail_body("erro_btw_historico_ilegivel", "o histórico da pergunta lateral está ilegível", json!({}))),
    };
    cors(&headers, response.headers_mut());
    response
}
