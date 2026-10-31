//! `POST /api/ask-history` (Perguntar): termos da pergunta, uma busca por termo no índice e um
//! `claude -p` que diz em qual sessão o assunto apareceu. Só o dono; o resto segue ao Python.
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
fn search_terms(index: &Index, terms: &[String], live: &HashMap<String, String>) -> Result<Vec<Hit>, String> {
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
    if !crate::loops::automations(st) {
        return json_response(StatusCode::CONFLICT, detail_body("erro_automacoes_desligadas", "automações desligadas (kill-switch)", json!({})));
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
        None => {
            let cli = crate::claude_p::Cli { accounts: st.accounts.clone(), bridge: crate::accounts::http::bridge(st).ok() };
            crate::claude_p::claude_p(&prompt, &cli).await
        }
    };
    match result {
        Ok(answer) => json_response(StatusCode::OK, json!({"answer": answer, "hits": hits})),
        Err(msg) => {
            tracing::warn!(code = "ask_history_failed", "ask-history falhou: {msg}");
            json_response(StatusCode::BAD_GATEWAY, json!({"detail": msg}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
