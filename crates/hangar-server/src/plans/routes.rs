//! Rotas de plano (`/plan`, `/plans`, `/plan-preview`, `/plan-pin`, `/plan-step`, `/plan-archive`),
//! com o contrato que o Python respondia: mesmos campos, status e envelope `detail`. O dono entra pela
//! porta pública; convidado e Connect chegam pela ponte `crate::forwarded`, já autenticados.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use hangar_api::session::SessionRow;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::list::plan::{PIN_NONE, Plan, plans_dir, safe_stem};
use crate::plans::files::{self, WriteError};
use crate::routes::{AppState, cors, gate, pass};
use crate::session_write::{busy_body, detail_body, json_response, session_name, too_large};

/// Corpo das escritas de plano: um stem e um índice, nunca mais que isto.
const BODY_LIMIT: usize = 64 * 1024;

/// O fim do caminho de cada rota e o método dela: uma tabela para o roteador e para a ponte.
const ROUTES: [(&str, Method); 6] = [
    ("plan", Method::GET), ("plans", Method::GET), ("plan-preview", Method::GET),
    ("plan-pin", Method::POST), ("plan-step", Method::POST), ("plan-archive", Method::POST),
];

/// Caminho e método que estas rotas atendem.
pub fn matches(method: &Method, path: &str) -> bool {
    let Some(tail) = path.strip_prefix("/api/sessions/").and_then(|p| p.split_once('/')).map(|(_, t)| t) else { return false };
    ROUTES.iter().any(|(t, m)| *t == tail && m == method)
}

/// As rotas na porta pública, cada uma com o repasse do que não é dela.
pub fn router() -> axum::Router<Arc<AppState>> {
    use crate::routes::pass_any;
    ROUTES.into_iter().fold(axum::Router::new(), |router, (tail, method)| {
        let filter = axum::routing::MethodFilter::try_from(method).expect("método de rota conhecido");
        router.route(&format!("/api/sessions/{{name}}/{tail}"), axum::routing::on(filter, public).fallback(pass_any))
    })
}

pub async fn public(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (forward, owner) = gate(&st, peer, &req);
    if !owner {
        return pass(&st, req, &forward).await;
    }
    let headers = req.headers().clone();
    let mut response = serve(&st, req).await;
    cors(&headers, response.headers_mut());
    response
}

/// Atende um pedido já autorizado (o dono pela porta pública, ou a ponte do Python).
pub async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    let query: HashMap<String, String> =
        form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let (Some(name), Some(tail)) = (session_name(&path), path.rsplit('/').next().map(str::to_owned)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let response = if req.method() == Method::POST {
        let headers = req.headers().clone();
        let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return too_large(&headers) };
        write(st, &name, &tail, &bytes).await
    } else {
        match tail.as_str() {
            "plan" => detail(st, &name).await,
            "plans" => catalog(st, &name).await,
            _ => preview(st, &name, &query).await,
        }
    };
    // Sem o Python nessas rotas, a falha só chega ao diário exportável por aqui.
    st.diag.report_response("rust.plans_failed", &name, &response, "a operação de plano falhou");
    response
}

fn refused(status: StatusCode, code: &'static str, msg: &str, params: Value) -> Response {
    crate::diag::coded(json_response(status, detail_body(code, msg, params)), code)
}

/// O 422 do FastAPI para o corpo, no formato que os clientes já leem.
pub(crate) fn unprocessable_body(msg: &str) -> Response {
    json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "value_error", "loc": ["body"], "msg": msg, "input": null}]}))
}

async fn session(st: &AppState, name: &str) -> Result<SessionRow, Response> {
    match tokio::time::timeout(crate::locate::LOCATE_TIMEOUT, st.list.find_row(name)).await {
        Ok(Ok(Some(row))) => Ok(row),
        Ok(Ok(None)) => Err(missing_session()),
        Ok(Err(e)) => Err(refused(StatusCode::SERVICE_UNAVAILABLE, e.code, e.detail, json!({}))),
        Err(_) => Err(refused(StatusCode::SERVICE_UNAVAILABLE, "locate_timeout", "a lista de sessões não respondeu a tempo", json!({}))),
    }
}

fn missing_session() -> Response {
    refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão não encontrada", json!({}))
}

async fn session_cwd(st: &AppState, name: &str) -> Result<PathBuf, Response> {
    let row = session(st, name).await?;
    row.cwd.filter(|c| !c.is_empty()).map(PathBuf::from).ok_or_else(missing_session)
}

/// Disco fora do executor assíncrono; pânico vira 500 com código, nunca a conexão caída.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Response> {
    tokio::task::spawn_blocking(f).await
        .map_err(|_| refused(StatusCode::INTERNAL_SERVER_ERROR, "erro_plano_interno", "a operação de plano foi interrompida", json!({})))
}

fn active(st: &AppState, cwd: PathBuf) -> impl FnOnce() -> Option<Arc<Plan>> + Send + 'static {
    let list = st.list.clone();
    // A base do relógio da lista: o prazo da descoberta compara números da mesma origem.
    let mono = || crate::list::classify::process_start().elapsed().as_secs_f64();
    move || list.with_plans(|t| t.active(&cwd, crate::state::monitor::wall_now(), mono()))
}

async fn detail(st: &Arc<AppState>, name: &str) -> Response {
    let cwd = match session_cwd(st, name).await { Ok(cwd) => cwd, Err(r) => return r };
    let Some(p) = (match blocking(active(st, cwd)).await { Ok(p) => p, Err(r) => return r }) else {
        return refused(StatusCode::NOT_FOUND, "erro_sem_plano_ativo", "sem plano ativo", json!({}));
    };
    // O markdown vai junto: o `/file` só serve caminho citado no transcript, e um plano achado pela
    // varredura nunca aparece lá.
    let markdown = match tokio::fs::read(&p.path).await {
        Ok(raw) => String::from_utf8_lossy(&raw).into_owned(),
        Err(e) => {
            // O plano vem do cache; o arquivo pode ter sumido entre as duas leituras.
            tracing::warn!(code = "plan_markdown_unreadable", kind = ?e.kind(), "markdown do plano ilegível");
            String::new()
        }
    };
    json_response(StatusCode::OK, json!({
        "name": p.name, "path": p.path.to_string_lossy(),
        "stem": p.path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default(),
        "task": p.task_idx, "task_total": p.task_total,
        "done": p.done, "total": p.total, "complete": p.complete,
        "tasks": p.tasks, "markdown": markdown,
    }))
}

fn no_plans_dir() -> Response {
    refused(StatusCode::NOT_FOUND, "erro_sem_pasta_planos", "repositório sem pasta de planos", json!({}))
}

async fn catalog(st: &Arc<AppState>, name: &str) -> Response {
    let cwd = match session_cwd(st, name).await { Ok(cwd) => cwd, Err(r) => return r };
    let list = st.list.clone();
    match blocking(move || list.with_plans(|t| t.catalog(&cwd))).await {
        Ok(Some(v)) => json_response(StatusCode::OK, v),
        Ok(None) => no_plans_dir(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinBody { #[serde(default)] stem: Option<String> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepBody { stem: String, idx: i64, done: bool }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveBody { stem: String }

enum WriteOp { Pin(PinBody), Step(StepBody), Archive(ArchiveBody) }

fn parse_write(tail: &str, bytes: &[u8]) -> Result<WriteOp, String> {
    let e = |e: serde_json::Error| e.to_string();
    match tail {
        "plan-pin" => serde_json::from_slice(bytes).map(WriteOp::Pin).map_err(e),
        "plan-step" => serde_json::from_slice(bytes).map(WriteOp::Step).map_err(e),
        _ => serde_json::from_slice(bytes).map(WriteOp::Archive).map_err(e),
    }
}

/// `require_available`: transferência gravada em curso para o nome (a leitura do disco da lista).
pub(crate) fn transfer_busy(facts: &crate::list::facts::ListFacts, name: &str) -> bool { facts.frozen.contains(name) }

/// `_transfer_guard`: troca de conversa ou de conta em curso recusa com o código do Python, pela
/// porta em memória e pela transferência gravada no disco. O passe segura a porta até a escrita
/// acabar; sem runtime, nenhum.
pub(crate) async fn enter(st: &AppState, name: &str) -> Result<Option<crate::runtime::ingress::IngressPass>, Response> {
    let busy = || crate::diag::coded(json_response(StatusCode::CONFLICT, busy_body()), "session_transfer_busy");
    let Some(runtime) = st.state.runtime.get() else { return Ok(None) };
    let pass = runtime.ingress().enter(name, st.write_gate_wait).await.map_err(|_| busy())?;
    match st.list.snapshot().await {
        Ok(produced) if transfer_busy(&produced.facts, name) => return Err(busy()),
        Ok(produced) if produced.facts.unknown => tracing::warn!(session = name, code = "list_facts_unknown", "transferências sem leitura; a escrita segue só com a porta em memória"),
        Err(e) => tracing::warn!(session = name, code = e.code, "lista indisponível; a escrita segue só com a porta em memória"),
        Ok(_) => {}
    }
    Ok(Some(pass))
}

async fn write(st: &Arc<AppState>, name: &str, tail: &str, bytes: &[u8]) -> Response {
    // A trava vem antes do corpo, como a dependência `_transfer_guard` antes da validação.
    let _pass = match enter(st, name).await { Ok(pass) => pass, Err(r) => return r };
    let op = match parse_write(tail, bytes) { Ok(op) => op, Err(msg) => return unprocessable_body(&msg) };
    let cwd = match session_cwd(st, name).await { Ok(cwd) => cwd, Err(r) => return r };
    let at = cwd.clone();
    let Some(root) = (match blocking(move || plans_dir(&at)).await { Ok(root) => root, Err(r) => return r }) else { return no_plans_dir() };
    match op {
        WriteOp::Pin(body) => pin(st, root, body.stem).await,
        WriteOp::Step(body) => step(st, root, cwd, body).await,
        WriteOp::Archive(body) => archive(st, root, body.stem).await,
    }
}

async fn pin(st: &AppState, root: PathBuf, stem: Option<String>) -> Response {
    // Só um plano que existe nesta pasta: senão o stem do cliente viraria nome de arquivo. A
    // guarda de separador vem antes do `is_file`, que já diria se existe `.md` fora da pasta.
    if let Some(stem) = stem.as_deref().filter(|s| *s != PIN_NONE) {
        let params = json!({"nome": stem});
        if !safe_stem(stem) {
            return refused(StatusCode::BAD_REQUEST, "erro_nome_plano_invalido", &format!("nome de plano inválido: {stem}"), params);
        }
        let candidate = root.join(format!("{stem}.md"));
        if !matches!(blocking(move || candidate.is_file()).await, Ok(true)) {
            return refused(StatusCode::NOT_FOUND, "erro_plano_nao_encontrado", &format!("plano não encontrado: {stem}"), params);
        }
    }
    let (written, at) = (stem.clone(), root.clone());
    match blocking(move || files::write_pin(&at, written.as_deref())).await {
        Ok(Ok(())) => {
            st.list.forget_plans(root);
            json_response(StatusCode::OK, json!({"pinned": stem}))
        }
        Ok(Err(WriteError(e))) => refused(StatusCode::INTERNAL_SERVER_ERROR, "erro_gravar_pin", &format!("não deu pra gravar o pin: {e}"), json!({"erro": e})),
        Err(r) => r,
    }
}

async fn step(st: &AppState, root: PathBuf, cwd: PathBuf, body: StepBody) -> Response {
    let marked = blocking(move || files::plan_path(&root, &body.stem).and_then(|path| files::mark_step(&path, body.idx, body.done))).await;
    match marked {
        Ok(Ok(())) => {}
        // 409: o arquivo mudou ou não serve, não é falha do servidor; a tela mostra o texto.
        Ok(Err(WriteError(e))) => return refused(StatusCode::CONFLICT, "erro_marcar_step", &format!("não deu pra marcar o passo: {e}"), json!({"erro": e})),
        Err(r) => return r,
    }
    match blocking(active(st, cwd)).await {
        Ok(p) => json_response(StatusCode::OK, json!({"done": p.as_ref().map(|p| p.done), "total": p.as_ref().map(|p| p.total),
            "complete": p.is_some_and(|p| p.complete)})),
        Err(r) => r,
    }
}

async fn archive(st: &AppState, root: PathBuf, stem: String) -> Response {
    let at = root.clone();
    let moved = blocking(move || files::archive(&at, &stem)).await;
    // Mesmo parcial, o `.md` pode já ter saído: a descoberta não pode descrever o antes.
    st.list.forget_plans(root);
    match moved {
        Ok(Ok(moved)) => json_response(StatusCode::OK, json!({"moved": moved})),
        Ok(Err(WriteError(e))) => refused(StatusCode::CONFLICT, "erro_arquivar_plano", &format!("não deu pra arquivar: {e}"), json!({"erro": e})),
        Err(r) => r,
    }
}

async fn preview(st: &Arc<AppState>, name: &str, query: &HashMap<String, String>) -> Response {
    let content = match crate::query::bool_param_or(query, "content", true) { Ok(c) => c, Err(r) => return *r };
    let row = match session(st, name).await { Ok(row) => row, Err(r) => return r };
    let Some(jsonl) = row.jsonl.clone().filter(|j| !j.is_empty()) else {
        return refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão ou transcript não encontrado", json!({}));
    };
    if row.provider != "claude" {
        return json_response(StatusCode::OK, Value::Null);
    }
    // Sem pasta, configuração e caminho relativo seriam lidos da pasta do próprio servidor.
    let Some(cwd) = row.cwd.filter(|c| !c.is_empty()).map(PathBuf::from) else { return missing_session() };
    let found = match blocking(move || super::claude::discover(Path::new(&jsonl), &cwd)).await { Ok(f) => f, Err(r) => return r };
    let Some(plan) = found else { return json_response(StatusCode::OK, Value::Null) };
    let mut reply = json!({"name": plan.name, "path": plan.path.to_string_lossy()});
    if let Some(anchor) = plan.anchor_id {
        reply["anchor_id"] = json!(anchor);
    }
    if !content {
        return json_response(StatusCode::OK, reply);
    }
    match tokio::fs::read(&plan.path).await.map(String::from_utf8) {
        Ok(Ok(markdown)) => {
            reply["markdown"] = json!(markdown);
            json_response(StatusCode::OK, reply)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
            refused(StatusCode::NOT_FOUND, "erro_plano_removido", "arquivo do plano não encontrado", json!({})),
        _ => refused(StatusCode::INTERNAL_SERVER_ERROR, "erro_plano_ilegivel", "não foi possível ler o plano", json!({})),
    }
}
