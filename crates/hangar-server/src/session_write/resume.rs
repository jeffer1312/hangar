//! `POST /api/sessions/{name}/resume`: a sessão Claude volta numa conversa que já existe no disco (a
//! sessão aberta sem id passa a ser rastreada). Sem `session_id` o Rust escolhe a mais recente da pasta,
//! ou devolve as candidatas quando outra sessão divide a pasta. A reabertura é a do spec
//! (`RuntimeRegistry::resume_claude_session`), como as outras. Port do `resume_session` com
//! `registry.resume_candidates`/`resume`.
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::{BODY_LIMIT, detail_body, session_name, too_large};
use crate::list::discover::real;
use crate::mods::http::reply;
use crate::routes::{AppState, gate, pass};

/// Candidatas da pasta: as mais recentes, como a tela de confirmação mostra.
const CANDIDATES: usize = 6;

/// `ResumeBody`: só `session_id` (texto ou nulo).
fn parse(bytes: &[u8]) -> Result<Option<String>, Value> {
    let invalid = |detail: &str| detail_body("erro_corpo_invalido", &format!("Pedido em formato inválido: {detail}"), json!({}));
    if bytes.iter().all(u8::is_ascii_whitespace) { return Ok(None); }
    let value: Value = serde_json::from_slice(bytes).map_err(|e| invalid(&e.to_string()))?;
    let fields = value.as_object().ok_or_else(|| invalid("o corpo é um objeto"))?;
    if let Some(extra) = fields.keys().find(|k| *k != "session_id") { return Err(invalid(&format!("{extra}: campo desconhecido"))); }
    match fields.get("session_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(sid)) => Ok(Some(sid.clone())),
        Some(_) => Err(invalid("session_id é texto ou null")),
    }
}

fn uuid(sid: &str) -> bool {
    let parts: Vec<&str> = sid.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A primeira mensagem de quem pediu, nas 60 primeiras linhas, em até 100 caracteres.
fn preview(path: &Path) -> String {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut parser = crate::transcript::LineParser::new(crate::transcript::Provider::Claude);
    for line in std::io::BufReader::new(file).split(b'\n').take(60).flatten() {
        if let Some(text) = parser.feed(&line, 0).into_iter()
            .find(|e| e.kind == hangar_api::chat::ChatKind::UserMsg).and_then(|e| e.text).filter(|t| !t.is_empty()) {
            return text.chars().take(100).collect();
        }
    }
    String::new()
}


/// As conversas `<uuid>.jsonl` da pasta, da mais recente; a cópia `<uuid>.from-<conta>.jsonl` não é conversa.
fn conversations(project: &Path) -> Vec<(PathBuf, f64)> {
    let mut out: Vec<(PathBuf, f64)> = std::fs::read_dir(project).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl") && p.file_stem().and_then(|s| s.to_str()).is_some_and(uuid))
        .filter_map(|p| {
            let mtime = p.metadata().and_then(|m| m.modified()).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
            Some((p, mtime))
        }).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.truncate(CANDIDATES);
    out
}

#[derive(Debug, PartialEq)]
enum Choice { Ambiguous(Vec<Value>), Pick(String), Busy(Option<String>) }

/// A escolha sem `session_id`: com outra sessão na pasta e mais de uma candidata, quem escolhe é a pessoa.
fn choose(found: &[(PathBuf, f64)], in_use: &dyn Fn(&Path) -> Option<String>, siblings: bool) -> Choice {
    let stem = |path: &Path| path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
    if siblings && found.len() > 1 {
        return Choice::Ambiguous(found.iter().map(|(path, mtime)| json!({"session_id": stem(path), "mtime": mtime,
            "preview": preview(path), "in_use": in_use(path).is_some()})).collect());
    }
    // A conversa que outra sessão já escreve nunca é a escolhida.
    match found.iter().find(|(path, _)| in_use(path).is_none()) {
        Some((path, _)) => Choice::Pick(stem(path)),
        None => Choice::Busy(found.first().and_then(|(path, _)| in_use(path))),
    }
}


pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let (true, Some(name)) = (owner, session_name(req.uri().path())) else { return pass(&st, req, &fwd).await };
    let headers = req.headers().clone();
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return too_large(&headers) };
    let (status, body) = serve(&st, &name, &bytes).await;
    reply(Some(&headers),status, body)
}

/// O dono pela porta pública ou pelo repasse do Python (Connect, convidado já filtrado lá).
pub async fn serve(st: &Arc<AppState>, name: &str, bytes: &[u8]) -> (StatusCode, Value) {
    let asked = match parse(bytes) {
        Ok(asked) => asked,
        Err(body) => return (StatusCode::UNPROCESSABLE_ENTITY, body),
    };
    if asked.as_deref().is_some_and(|sid| !uuid(sid)) {
        return (StatusCode::UNPROCESSABLE_ENTITY, detail_body("erro_session_id_invalido", "session_id invalido", json!({})));
    }
    let Some(runtime) = st.state.runtime.get().cloned() else {
        return (StatusCode::SERVICE_UNAVAILABLE, detail_body("runtime_unhealthy", "o runtime do Rust não está de pé", json!({})));
    };
    let (st2, name) = (st.clone(), name.to_owned());
    // Numa tarefa própria: a troca admitida termina mesmo que o cliente desista.
    tokio::spawn(async move { resume(&st2, &runtime, &name, asked).await })
        .await.unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, detail_body("relaunch_panicked", "a retomada parou no meio; confira a sessão", json!({}))))
}

async fn resume(st: &Arc<AppState>, runtime: &crate::runtime::gateway::RuntimeRegistry, name: &str, asked: Option<String>) -> (StatusCode, Value) {
    let refused = |status: StatusCode, code: &str, msg: &str, params: Value| (status, detail_body(code, msg, params));
    let spec = match runtime.claude_resume_spec(name).await {
        Some(Ok(spec)) => spec,
        Some(Err(error)) => return refused(StatusCode::CONFLICT, &error.code, &error.message, json!({})),
        None => {
            let row = st.list.find_row(name).await.ok().flatten();
            return match row {
                // O resto não retoma por aqui: volta pelo Arquivo, que retoma cada agente com o verbo dele.
                Some(row) => refused(StatusCode::CONFLICT, "erro_resume_so_claude", &format!("retomar a sessão só vale para o Claude (esta é {}); \
                    retome a conversa pelo Arquivo", row.provider), json!({"provider": row.provider})),
                None => refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessao nao encontrada", json!({})),
            };
        }
    };
    let env = runtime.codex_environment();
    let projects = spec.account.config_dir().map_or_else(|| env.claude_base.join("projects"), |d| Path::new(d).join("projects"))
        .join(hangar_workspace::worktrees::sanitize_cwd(&spec.cwd));
    let live = match st.list.snapshot().await {
        Ok(produced) => produced.rows.iter().filter(|r| r.name != name).filter_map(|r| Some((real(r.jsonl.as_deref()?), r.name.clone()))).collect::<Vec<_>>(),
        Err(error) => return refused(StatusCode::SERVICE_UNAVAILABLE, "erro_lista_indisponivel", "a lista de sessões está indisponível", json!({"detalhe": error.code})),
    };
    let in_use = |path: &Path| live.iter().find(|(p, _)| *p == real(&path.to_string_lossy())).map(|(_, n)| n.clone());
    let sid = match asked {
        Some(sid) => sid,
        None => {
            let found = { let p = projects.clone(); tokio::task::spawn_blocking(move || conversations(&p)).await.unwrap_or_default() };
            if found.is_empty() { return refused(StatusCode::NOT_FOUND, "erro_transcript_ausente", "nenhum transcript pra retomar neste diretorio", json!({})); }
            // Outra sessão no mesmo diretório: a mais recente pode ser dela, e quem escolhe é a pessoa.
            let siblings = crate::list::mux::Mux::default().list_panes().await.map(|panes| {
                let mut names: Vec<&String> = panes.iter().filter(|p| !p.hidden && p.cwd == spec.cwd).map(|p| &p.session).collect();
                names.sort();
                names.dedup();
                names.iter().any(|n| *n != name)
            }).unwrap_or(false);
            match choose(&found, &in_use, siblings) {
                Choice::Ambiguous(candidates) => return (StatusCode::OK, json!({"ambiguous": true, "candidates": candidates})),
                Choice::Pick(sid) => sid,
                Choice::Busy(other) => return refused(StatusCode::CONFLICT, "erro_conversa_viva", "conversa já está aberta", json!({"sessao": other})),
            }
        }
    };
    let transcript = projects.join(format!("{sid}.jsonl"));
    if !transcript.is_file() { return refused(StatusCode::NOT_FOUND, "erro_transcript_nao_encontrado", "transcript nao encontrado", json!({})); }
    if let Some(other) = in_use(&transcript) {
        return refused(StatusCode::CONFLICT, "erro_conversa_viva", "conversa já está aberta", json!({"sessao": other}));
    }
    let outcome = super::relaunch::during_life_change(st, name, runtime.resume_claude_session(name, &sid)).await;
    st.list.invalidate();
    match outcome {
        None => refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessao nao encontrada", json!({})),
        Some(Err(failure)) => super::relaunch::claude_refusal(super::relaunch::CLAUDE_RESUME, &failure),
        Some(Ok(_)) => {
            st.list.seed(name, &transcript.to_string_lossy());
            let row = st.list.find_row(name).await.ok().flatten().and_then(|row| serde_json::to_value(row).ok());
            (StatusCode::OK, row.unwrap_or_else(|| json!({"name": name, "cwd": spec.cwd, "jsonl": transcript, "provider": "claude",
                "headless": spec.surface == crate::session_open::Surface::Headless, "tracked": true})))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_automatic_choice_never_takes_a_conversation_another_session_writes() {
        let found = vec![(PathBuf::from("/p/a.jsonl"), 3.0), (PathBuf::from("/p/b.jsonl"), 2.0)];
        let a_taken = |p: &Path| (p == Path::new("/p/a.jsonl")).then(|| "outra".to_owned());
        assert_eq!(choose(&found, &a_taken, false), Choice::Pick("b".into()), "A mais recente é de outra sessão");
        let all_taken = |_: &Path| Some("outra".to_owned());
        assert_eq!(choose(&found, &all_taken, false), Choice::Busy(Some("outra".into())));
        let free = |_: &Path| None;
        assert_eq!(choose(&found, &free, false), Choice::Pick("a".into()));
        assert!(matches!(choose(&found, &free, true), Choice::Ambiguous(c) if c.len() == 2), "outra sessão na pasta: a pessoa escolhe");
        assert_eq!(choose(&found[..1], &free, true), Choice::Pick("a".into()), "uma só candidata não é ambígua");
    }

    #[test]
    fn the_body_takes_only_session_id() {
        assert_eq!(parse(b"").unwrap(), None);
        assert_eq!(parse(br#"{"session_id":null}"#).unwrap(), None);
        assert_eq!(parse(br#"{"session_id":"x"}"#).unwrap().as_deref(), Some("x"));
        for bad in [&br#"{"session_id":1}"#[..], br#"{"x":1}"#, b"[]"] {
            assert_eq!(parse(bad).unwrap_err()["detail"]["code"], "erro_corpo_invalido");
        }
        assert!(uuid("11111111-1111-4111-8111-111111111111") && !uuid("nao-e-uuid") && !uuid("x; rm -rf /"));
    }
}
