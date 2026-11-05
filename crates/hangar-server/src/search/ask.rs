//! `POST /api/ask-history` (Perguntar): termos da pergunta, uma busca por termo no índice e um
//! `claude -p` que diz em qual sessão o assunto apareceu. Só o dono; o resto segue ao Python, que responde 403 `erro_busca_so_dono`.
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use serde_json::{Map, Value, json};

use super::hits::{Hit, search};
use super::Index;
use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::session_write::{detail_body, json_response};

/// O índice reconhece a conversa do próprio `claude -p` pelos 60 primeiros caracteres: não mudar.
pub(crate) const SYSTEM: &str = "Você responde onde um assunto apareceu nas conversas do usuário, a partir de TRECHOS de \
transcripts. Responda curto em pt-BR: diga em QUAL(is) sessão(ões) o assunto apareceu e o \
contexto, citando os NOMES EXATOS das sessões como aparecem nos rótulos. Se os trechos não \
responderem à pergunta, diga que não encontrou. Sem preâmbulo, sem markdown.";
const NOTHING: &str = "não achei nada sobre isso nas conversas";
const QUESTION_MAX: usize = 500;
const MAX_TERMS: usize = 6;
/// Teto por termo e no total: segura o tamanho do prompt.
const CAP: usize = 30;
const BODY_LIMIT: usize = 256 * 1024;

// Só o ruído comum que degradaria o OU de termos; palavra de conteúdo fica.
const STOPWORDS: [&str; 78] = [
    "que", "qual", "quais", "quando", "onde", "como", "por", "porque", "para", "pra", "com", "sem",
    "dos", "das", "uma", "uns", "umas", "meu", "minha", "seu", "sua", "nos", "nas", "isso", "isto",
    "aquilo", "sobre", "falei", "falamos", "conversa", "conversamos", "disse", "mencionei", "assunto",
    "ele", "ela", "eles", "elas", "voce", "vc", "eu", "tem", "ter", "foi", "era", "sao", "num", "numa",
    "the", "and", "for", "with", "was", "were", "did", "does", "what", "when", "where", "which", "who",
    "about", "said", "told", "talked", "spoke", "mentioned", "this", "that", "these", "those", "you",
    "have", "has", "had", "are", "our", "their",
];
// `\w` do `re` do Python: letras, números e sublinhado (marca combinante separa a palavra).
static WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{N}_]+").unwrap());

/// Termos da pergunta: minúsculos, sem stopwords nem palavras com menos de 3 caracteres, sem repetir,
/// na ordem, até 6.
pub(crate) fn extract_terms(question: &str) -> Vec<String> {
    let lower = question.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for word in WORD.find_iter(&lower).map(|m| m.as_str()) {
        if word.chars().count() < 3 || STOPWORDS.contains(&word) || out.iter().any(|o| o == word) { continue; }
        out.push(word.to_owned());
        if out.len() >= MAX_TERMS { break; }
    }
    out
}

/// Busca OU: uma busca por termo, sem repetir (sessão, trecho), conversa mais nova primeiro, até 30.
pub(super) fn search_terms(index: &Index, terms: &[String], live: &HashMap<String, String>) -> Result<Vec<Hit>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for term in terms {
        for hit in search(index, term, live, CAP)? {
            if seen.insert((hit.session_id.clone(), hit.line.clone())) { out.push(hit); }
        }
    }
    out.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(CAP);
    Ok(out)
}

fn prompt(question: &str, hits: &[Hit]) -> String {
    let mut lines = vec![SYSTEM.to_owned(), String::new(), format!("PERGUNTA: {question}"), String::new(), "Trechos das conversas:".to_owned()];
    for h in hits {
        let short: String = h.session_id.chars().take(8).collect();
        let name = h.session_name.as_deref().filter(|n| !n.is_empty())
            .or(Some(h.project.as_str()).filter(|p| !p.is_empty())).unwrap_or(&short);
        lines.push(format!("[sessão {name} — {}]: {}", if h.live { "viva" } else { "arquivada" }, h.line));
    }
    lines.join("\n")
}

fn error(kind: &str, loc: Value, msg: &str, input: Value) -> Value {
    json!({"type": kind, "loc": loc, "msg": msg, "input": input})
}

/// `AskHistoryBody` (`_StrictBody`): a pergunta, ou a lista de erros do 422 do FastAPI.
fn question(bytes: &[u8]) -> Result<String, Vec<Value>> {
    if bytes.is_empty() { return Err(vec![error("missing", json!(["body"]), "Field required", Value::Null)]); }
    let value = match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => v,
        Err(e) => {
            // Posição em caracteres, como o `json.JSONDecodeError.pos`.
            let text = String::from_utf8_lossy(bytes);
            let pos = if e.is_eof() { text.chars().count() } else {
                let before: usize = text.split('\n').take(e.line().saturating_sub(1)).map(|l| l.chars().count() + 1).sum();
                before + e.column().saturating_sub(1)
            };
            let mut err = error("json_invalid", json!(["body", pos]), "JSON decode error", json!({}));
            err["ctx"] = json!({"error": e.to_string()});
            return Err(vec![err]);
        }
    };
    let m: Map<String, Value> = match value {
        Value::Null => return Err(vec![error("missing", json!(["body"]), "Field required", Value::Null)]),
        Value::Object(m) => m,
        other => return Err(vec![error("model_attributes_type", json!(["body"]),
            "Input should be a valid dictionary or object to extract fields from", other)]),
    };
    let loc = json!(["body", "question"]);
    let mut errors = Vec::new();
    let mut found = None;
    match m.get("question") {
        None => errors.push(error("missing", loc, "Field required", Value::Object(m.clone()))),
        Some(Value::String(s)) => match s.chars().count() {
            0 => errors.push(json!({"type": "string_too_short", "loc": loc, "msg": "String should have at least 1 character",
                "input": s, "ctx": {"min_length": 1}})),
            n if n > QUESTION_MAX => errors.push(json!({"type": "string_too_long", "loc": loc,
                "msg": format!("String should have at most {QUESTION_MAX} characters"), "input": s, "ctx": {"max_length": QUESTION_MAX}})),
            _ => found = Some(s.clone()),
        },
        Some(other) => errors.push(error("string_type", loc, "Input should be a valid string", other.clone())),
    }
    errors.extend(m.iter().filter(|(k, _)| *k != "question")
        .map(|(k, v)| error("extra_forbidden", json!(["body", k]), "Extra inputs are not permitted", v.clone())));
    match found {
        Some(q) if errors.is_empty() => Ok(q),
        _ => Err(errors),
    }
}

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (forward, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::POST {
        return pass(&st, req, &forward).await;
    }
    let headers = req.headers().clone();
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return crate::plans::routes::unprocessable_body("corpo grande demais") };
    let mut resp = answer(&st, &headers, &bytes).await;
    cors(&headers, resp.headers_mut());
    resp
}

async fn answer(st: &Arc<AppState>, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let question = match question(bytes) {
        Ok(q) => q,
        Err(detail) => return json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": detail})),
    };
    match crate::loops::automations(st) {
        Ok(true) => {}
        Ok(false) => return json_response(StatusCode::CONFLICT, detail_body("erro_automacoes_desligadas", "automações desligadas (kill-switch)", json!({}))),
        Err(code) => return json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "a configuração das automações está inválida", json!({}))),
    }
    let live = match super::http::live_names(st).await {
        Ok(live) => live,
        Err(code) => return json_response(StatusCode::SERVICE_UNAVAILABLE,
            detail_body("erro_lista_indisponivel", "a lista de sessões está indisponível", json!({"detalhe": code}))),
    };
    let (index, terms) = (st.search.clone(), extract_terms(&question));
    let hits = match tokio::task::spawn_blocking(move || search_terms(&index, &terms, &live)).await {
        Ok(Ok(hits)) => hits,
        Ok(Err(code)) => return json_response(StatusCode::SERVICE_UNAVAILABLE,
            detail_body("erro_busca_contas", "as contas Claude não puderam ser lidas", json!({"detalhe": code}))),
        Err(_) => return route_failed(st, headers, "rust.ask_history_failed", "", "ask_history_panic", "o Perguntar caiu no servidor"),
    };
    if hits.is_empty() {
        return json_response(StatusCode::OK, json!({"answer": NOTHING, "hits": []}));
    }
    let prompt = prompt(&question, &hits);
    let result = match &st.claude_p {
        Some(runner) => crate::claude_p::claude_p(&prompt, runner.as_ref()).await,
        None => crate::claude_p::claude_p(&prompt, &cli(st)).await,
    };
    match result {
        Ok(answer) => json_response(StatusCode::OK, json!({"answer": answer, "hits": hits})),
        Err(msg) => {
            // `msg` leva a saída do `claude -p`, montada com trechos de conversa: fora do relatório.
            tracing::warn!(code = "ask_history_failed", output = %msg, "ask-history falhou");
            json_response(StatusCode::BAD_GATEWAY, json!({"detail": msg}))
        }
    }
}

/// O `claude` do servidor, com a ponte das contas para ler as cotas. Sem ponte tudo vai para a conta
/// padrão: a falha vai ao diário quando começa.
fn cli(st: &AppState) -> crate::claude_p::Cli {
    let bridge = match crate::accounts::http::bridge(st) {
        Ok(bridge) => { st.search.recovered("busca.ponte_contas_falhou", ""); Some(bridge) }
        Err(e) => {
            st.search.report_once("busca.ponte_contas_falhou", "", || format!("{}: {}", e.code, e.message));
            None
        }
    };
    crate::claude_p::Cli { accounts: st.accounts.clone(), bridge }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_failure_reaches_the_diary_once() {
        // Sem a ponte as cotas não são lidas e tudo vai para a conta padrão: isso não pode ser calado.
        if crate::config::Config::runtime_instance().is_ok_and(|i| i.is_some()) { return; }
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::new(crate::config::Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
            auth_token: "dono".into(), internal_secret: "interno".into(), log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1") });
        let reports = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let diary = reports.clone();
        st.search = super::super::Index::new(dir.path(), std::sync::Arc::new(|| Ok(Vec::new())),
            std::sync::Arc::new(move |code, detail| diary.lock().unwrap().push((code, detail))), Default::default());
        assert!(cli(&st).bridge.is_none());
        assert!(cli(&st).bridge.is_none());
        let got: Vec<_> = reports.lock().unwrap().iter().map(|(c, _)| *c).collect();
        assert_eq!(got, ["busca.ponte_contas_falhou"]);
    }

    #[test]
    fn extract_terms_matches_the_capture() {
        // ASK-10 e ASK-11: a captura do Python, mais "você" (stopword só sem acento).
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search/contract/ask/ask.json");
        let cap: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for (q, want) in cap["data"]["extract_terms"].as_object().unwrap() {
            assert_eq!(json!(extract_terms(q)), *want, "q={q:?}");
        }
        assert_eq!(extract_terms("você voce falou"), ["você", "falou"]);
    }

    #[test]
    fn kill_switch_reads_cp_automations_from_the_environment() {
        // ASK-05: sem `runtime-config.json`, `CP_AUTOMATIONS=0` desliga; o arquivo vence o ambiente.
        let dir = tempfile::tempdir().unwrap();
        let read = |env: &[(&str, &str)]| crate::push::watch::Settings::read(dir.path(),
            &env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()).automations;
        assert!(read(&[]));
        assert!(!read(&[("CP_AUTOMATIONS", "0")]));
        std::fs::write(dir.path().join("runtime-config.json"), r#"{"automations": true}"#).unwrap();
        assert!(read(&[("CP_AUTOMATIONS", "0")]));
    }

    fn hit(session_name: Option<&str>, project: &str, live: bool) -> Hit {
        Hit { project: project.into(), session_id: "abcdef0123456789".into(), session_name: session_name.map(Into::into), cwd: None,
            line: "trecho".into(), mtime: 1.0, live, role: None, event_id: None, ts: None, provider: "codex".into(), conta: String::new(),
            config_dir: None, codex_account: None, whole: false, name: None, preview: None }
    }

    #[test]
    fn prompt_label_is_the_name_then_the_folder_then_the_short_id() {
        // ASK-17: nome da sessão viva, senão a pasta do projeto, senão `session_id[:8]`.
        let got = prompt("p?", &[hit(Some("viva"), "-proj", true), hit(Some(""), "-proj", false), hit(None, "", false)]);
        let labels: Vec<&str> = got.lines().skip(5).collect();
        assert_eq!(labels, ["[sessão viva — viva]: trecho", "[sessão -proj — arquivada]: trecho", "[sessão abcdef01 — arquivada]: trecho"]);
    }

    #[test]
    fn or_search_caps_at_thirty_newest_first() {
        // ASK-13: dois termos com 18 trechos cada; o total passa de 30 e ficam os 30 mais novos.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let projects = root.join("acct/projects/-home-u-proj");
        std::fs::create_dir_all(&projects).unwrap();
        for i in 0..12u64 {
            let term = if i % 2 == 0 { "alfa" } else { "beta" };
            let path = projects.join(format!("{i:08x}-0000-4000-8000-000000000000.jsonl"));
            let body: String = (0..3).map(|k| format!("{}\n", json!({"type": "user", "uuid": format!("u{k}"), "cwd": "/home/u/proj",
                "message": {"role": "user", "content": format!("{term} {i} {k}")}}))).collect();
            std::fs::write(&path, body).unwrap();
            std::fs::File::options().write(true).open(&path).unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000 + i)).unwrap();
        }
        let account = super::super::Account { config_dir: None, dir: root.join("acct"), projects: root.join("acct/projects") };
        let index = Index::new(&root.join("idx"), Arc::new(move || Ok(vec![account.clone()])), Arc::new(|_, _| {}), Default::default());
        let hits = search_terms(&index, &["alfa".into(), "beta".into()], &HashMap::new()).unwrap();
        assert_eq!(hits.len(), 30);
        assert!(hits.windows(2).all(|w| w[0].mtime >= w[1].mtime));
        assert!(hits.iter().all(|h| !h.session_id.starts_with("00000000") && !h.session_id.starts_with("00000001")), "os 2 mais velhos ficam fora");
    }
}
