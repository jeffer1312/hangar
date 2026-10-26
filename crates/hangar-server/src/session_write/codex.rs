//! Rotas só do Codex (`/models`, `/model`, `/service-tier`, `/codex/mode`, `/limits`, `/question/skip`,
//! `/commands`, `/codex-permissions`) da sessão Codex do Rust, com o corpo e o código da rota
//! Python no modo `python` (`api.py`). Com terminal valem as mesmas rotas quando a tabela entrega o
//! modo ao Rust; lá `/codex-permissions` dirige o seletor da TUI (`permission_control::codex`). O que
//! não é sessão Codex do Rust segue ao Python antes da porta: a lista de comandos do Claude não pode
//! esperar uma troca de agente.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Map, Value, json};

use super::input::{MSG_CODEX_CONTROL, answer, codex_control, json_content_type};
use super::{Ctx, Owner, Provider, WriteRoute, admit, decide, detail_body, enter_then_find, invalid_body, json_response, session_name};
use crate::mods::state::random_hex;
use crate::permission_control::PermissionAction;
use crate::routes::{AppState, cors, gate, pass, who};
use crate::runtime::gateway::EntryHandle;
use crate::runtime::protocol::{Disposition, OperationKind, RuntimeCommand, RuntimeError, RuntimeReply};

type Answer = (StatusCode, Value);

const MSG_INVALID_ANSWER: &str = "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente.";
const MSG_ANSWER_UNSENT: &str = "Não foi possível confirmar o envio da resposta ao Codex.";
/// `sem_terminal.MODOS` (nome e descrição), na ordem da tela.
const PERMISSION_MODES: [(&str, &str); 3] = [
    ("Ask for approval", "Codex só lê o workspace; editar ou rodar comando pede aprovação."),
    ("Approve for me", "Codex edita o workspace sozinho; fora dele ou com rede, pede aprovação."),
    ("Full Access", "Codex faz tudo sem perguntar."),
];
const DEFAULT_PERMISSION_MODE: &str = "Full Access";

fn accepted(sent: &Result<RuntimeReply, RuntimeError>) -> Option<&Value> {
    sent.as_ref().ok().filter(|reply| reply.disposition == Disposition::Accepted).map(|reply| &reply.payload)
}

/// Resposta fixa 409 do Codex; o motivo real (ator morto, cano fechado, prazo) vai ao log pela rota (`log_outcome`).
fn refused() -> Answer { codex_control(MSG_CODEX_CONTROL) }

/// Registra o código/disposição real de um desfecho que não foi `Accepted`.
/// O limitador usa só o código estável; a mensagem varia e vai só no texto do log.
pub(super) fn log_outcome(name: &str, what: &str, sent: &Result<RuntimeReply, RuntimeError>, why: &str) {
    if accepted(sent).is_some() { return; }
    let (code, message) = match sent {
        Err(error) => (error.code.clone(), error.message.clone()),
        Ok(reply) => (format!("{:?}", reply.disposition).to_lowercase(), reply.payload["error"].as_str().unwrap_or("").to_owned()),
    };
    if crate::warn_limit::allow(Some(name), &format!("{what}:{code}")) {
        tracing::warn!(session = %name, what, code = %code, message = %message, "{why}");
    }
}

// ── corpos de resposta, sem E/S ─────────────────────────────────────────────────────────────────

/// `/models`: configuração lida antes; a lista que falha sai vazia, como no adapter Python.
pub fn models_answer(settings: &Result<RuntimeReply, RuntimeError>, models: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let Some(current) = accepted(settings) else { return refused() };
    let models = accepted(models).filter(|m| m.is_array()).cloned().unwrap_or_else(|| json!([]));
    (StatusCode::OK, json!({"models": models, "current": current}))
}

pub fn model_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    if accepted(sent).is_some() { (StatusCode::OK, json!({"ok": true})) } else { refused() }
}

pub fn service_tier_answer(tier: &str, sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    match accepted(sent) {
        Some(done) if done["service_tier"] == tier => (StatusCode::OK, json!({"ok": true, "service_tier": tier})),
        _ => refused(),
    }
}

pub fn mode_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    accepted(sent).map_or_else(refused, |settings| (StatusCode::OK, settings.clone()))
}

/// `_normalize_rate_window`: só os três campos que o app lê.
fn rate_window(window: &Value) -> Value {
    if window.is_null() { return Value::Null; }
    json!({"usedPercent": window["usedPercent"], "windowMins": window["windowDurationMins"], "resetsAt": window["resetsAt"]})
}

/// `/limits`: falha de leitura é a resposta neutra, nunca erro (o app só não mostra o chip).
pub fn limits_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let snapshot = accepted(sent).map_or(&Value::Null, |result| &result["rateLimits"]);
    if snapshot.is_null() { return (StatusCode::OK, json!({"primary": null, "secondary": null, "planType": null})); }
    (StatusCode::OK, json!({"primary": rate_window(&snapshot["primary"]), "secondary": rate_window(&snapshot["secondary"]), "planType": snapshot["planType"]}))
}

/// `/question/skip`: pergunta que não está mais lá (recusa do motor) é 409; falha de envio é 503.
pub fn skip_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let invalid = || (StatusCode::CONFLICT, detail_body("erro_codex_resposta_invalida", MSG_INVALID_ANSWER, json!({})));
    let unsent = || (StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_codex_resposta_envio", MSG_ANSWER_UNSENT, json!({})));
    // Identidade ausente, trocada ou ilegível sai com o código próprio, nunca como falha de envio.
    if let Some(failed) = super::control::interaction_result(sent) { return failed; }
    match sent {
        Ok(reply) if reply.disposition == Disposition::Accepted => (StatusCode::OK, json!({"ok": true})),
        // Com terminal o cancelamento escrito não prova quem decidiu: resultado incerto, como em /answer e /select.
        Ok(reply) if reply.disposition == Disposition::Unknown && reply.payload["decision_confirmed"] == false =>
            (StatusCode::CONFLICT, detail_body("erro_sem_confirmacao_resposta", super::control::MSG_UNCONFIRMED_ANSWER, json!({}))),
        // Gravar o "Pular" falhou: a pergunta continua, e o erro é do envio, como o 503 do Python.
        Ok(reply) if reply.disposition == Disposition::Rejected && reply.payload["skip_not_saved"] == true => unsent(),
        Ok(reply) if reply.disposition == Disposition::Rejected => invalid(),
        Err(error) if matches!(error.code.as_str(), "codex_command" | crate::runtime::codex::INVALID_ANSWER) => invalid(),
        _ => unsent(),
    }
}

/// `/commands`: controles embutidos primeiro, sem skills homônimas nem caminhos locais no catálogo.
pub fn commands_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let Some(catalog) = accepted(sent) else { return refused() };
    let mut commands = vec![json!({"name": "clear", "display": "/clear", "source": "builtin",
        "description": "Limpa o histórico da conversa", "destructive": true}), json!({"name": "compact", "display": "/compact", "source": "builtin",
        "description": "Resume e compacta o contexto", "destructive": true})];
    for mut skill in crate::runtime::local_policy::skills(catalog) {
        if matches!(skill["name"].as_str(), Some("clear" | "compact")) { continue; }
        if let Some(fields) = skill.as_object_mut() { fields.remove("path"); fields.remove("native_name"); }
        commands.push(skill);
    }
    (StatusCode::OK, Value::Array(commands))
}

const MSG_COMPACT_ARGS: &str = "O /compact do Codex não aceita argumentos.";

/// `/compact` pelo `/input`, com o corpo do `_send_managed` do Python: aceito é entregue, adiado é
/// ok sem entrega, o resto é 400 `erro_envio_falhou` com o motivo.
pub fn compact_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let failed = |msg: &str| (StatusCode::BAD_REQUEST, detail_body("erro_envio_falhou", msg, json!({"erro": msg})));
    match sent {
        Err(error) if error.code == "terminal_panel_open" => super::control::panel_open_refusal(),
        Ok(reply) if matches!(reply.disposition, Disposition::Accepted | Disposition::Deferred) => (StatusCode::OK,
            json!({"ok": true, "delivered": reply.disposition == Disposition::Accepted, "steered": false, "native": false})),
        Ok(reply) if reply.disposition == Disposition::Unknown => failed("resultado incerto; entrada conservada sem reenvio"),
        Ok(reply) => failed(reply.payload["error"].as_str().filter(|e| !e.is_empty()).unwrap_or("entrada recusada pelo runtime")),
        Err(error) => failed(&error.message),
    }
}

/// O `/compact` do Codex com terminal é controle do motor (`thread/compact/start`), nunca texto na fila.
pub(super) async fn compact(target: &crate::runtime::gateway::WriteTarget, text: &str) -> Answer {
    if text.trim() != "/compact" { return compact_answer(&Err(RuntimeError::new("codex_command", MSG_COMPACT_ARGS))); }
    compact_answer(&target.handle.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::Compact, payload: json!({}) }).await)
}

/// A conversa nova não pode levar nem descartar uma mensagem que ainda espera na fila.
pub(super) async fn clear(target: &crate::runtime::gateway::WriteTarget, text: &str, panels: Option<Arc<crate::term::Terms>>) -> Answer {
    let sent = clear_conversation(target, text, panels).await;
    compact_answer(&sent)
}

async fn clear_conversation(target: &crate::runtime::gateway::WriteTarget, text: &str, panels: Option<Arc<crate::term::Terms>>) -> Result<RuntimeReply, RuntimeError> {
    let failed = |message:&str|RuntimeError::new("codex_clear", message);
    if text.trim() != "/clear" { return Err(failed("O /clear do Codex não aceita argumentos.")); }
    let handle = match &target.handle {
        EntryHandle::Headless(handle) | EntryHandle::CodexTerminal { handle, .. } => handle,
        _ => return Err(failed("Esta sessão não é do Codex.")),
    };
    let _turns = handle.hold_turns().await?;
    let before = handle.view().await?;
    if before["deliverable"] != true || before["async_questions"].as_array().is_some_and(|questions|!questions.is_empty()) {
        return Err(failed("Espere o Codex terminar e responda às perguntas antes de limpar a conversa."));
    }
    let rows = handle.queue(random_hex(16), crate::runtime::queue::Action::Load).await?;
    if rows.as_array().is_some_and(|rows|rows.iter().any(crate::runtime::queue::row_waits)) {
        return Err(failed("Há mensagens na fila esperando entrega. Envie ou descarte antes de limpar a conversa."));
    }
    if let EntryHandle::CodexTerminal { target, cell, .. } = &target.handle {
        let driver = crate::runtime::codex_terminal::pane_driver(&target.terminal, cell.pane_serial(), panels);
        crate::permission_control::codex_clear(&driver).await?;
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let current = handle.view().await?;
            if current["thread_id"].as_str().is_some_and(|id|!id.is_empty()) && current["thread_id"] != before["thread_id"] {
                return Ok(RuntimeReply { operation_id:String::new(), disposition:Disposition::Accepted, payload:json!({}) });
            }
            if tokio::time::Instant::now() >= until { return Err(failed("Não foi possível confirmar a conversa nova. Confira a sessão antes de repetir o /clear.")); }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    handle.command(RuntimeCommand { operation_id:random_hex(16), kind:OperationKind::Clear, payload:json!({}) }).await
}

/// `modos_para_tela`: o mesmo formato do picker da TUI.
pub fn permission_modes(current: Option<&str>) -> Value {
    let current = current.unwrap_or(DEFAULT_PERMISSION_MODE);
    let modes: Vec<Value> = PERMISSION_MODES.iter().enumerate().map(|(i, (name, desc))|
        json!({"numero": i + 1, "nome": name, "desc": desc, "cursor": *name == current, "atual": *name == current})).collect();
    json!({"modes": modes, "current": current})
}

pub fn permission_answer(sent: &Result<RuntimeReply, RuntimeError>) -> Answer {
    let picker = |status: StatusCode, msg: &str| (status, detail_body("erro_permissao_picker", msg, json!({})));
    match sent {
        Ok(reply) if reply.disposition == Disposition::Accepted => (StatusCode::OK, reply.payload.clone()),
        Err(error) if error.code == "erro_modo_desconhecido" => picker(StatusCode::BAD_REQUEST, &error.message),
        Err(error) if error.code == "erro_permissao_ocupada" => (StatusCode::CONFLICT, detail_body("erro_permissao_ocupada", &error.message, json!({}))),
        Err(error) => picker(StatusCode::SERVICE_UNAVAILABLE, &format!("não consegui reabrir o Codex: {}", error.message)),
        Ok(reply) => {
            let why = reply.payload["error"].as_str().unwrap_or("operação recusada pelo runtime");
            picker(StatusCode::SERVICE_UNAVAILABLE, &format!("não consegui reabrir o Codex: {why}"))
        }
    }
}

// ── corpos de pedido: o modelo estrito do FastAPI; o resto é `erro_corpo_invalido` daqui ───────

fn object(bytes: &Bytes, headers: &axum::http::HeaderMap) -> Option<Map<String, Value>> {
    if !json_content_type(headers) { return None; }
    match serde_json::from_slice::<Value>(bytes).ok()? { Value::Object(fields) => Some(fields), _ => None }
}

/// Só os campos de `allowed`, com `required` presente e texto (ou `null` nos opcionais).
fn strings(fields: &Map<String, Value>, required: &str, optional: Option<&str>) -> Option<Map<String, Value>> {
    if fields.keys().any(|key| key != required && Some(key.as_str()) != optional) { return None; }
    fields.get(required)?.as_str()?;
    if let Some(key) = optional && !matches!(fields.get(key), None | Some(Value::Null) | Some(Value::String(_))) { return None; }
    Some(fields.clone())
}

fn literal(fields: &Map<String, Value>, key: &str, values: &[&str]) -> Option<String> {
    strings(fields, key, None)?;
    fields[key].as_str().filter(|value| values.contains(value)).map(str::to_owned)
}

// ── rotas ───────────────────────────────────────────────────────────────────────────────────────

/// Sessão Codex que a tabela entrega ao Rust nesta rota. Sem ator aberto, o Codex do Rust é adotado (com
/// terminal) ou aberto (sem terminal parado) agora: o Python recusaria o próprio cliente.
async fn ours(st: &AppState, name: Option<&str>) -> Result<bool, RuntimeError> {
    let (Some(runtime), Some(name)) = (st.state.runtime.get(), name) else { return Ok(false) };
    if runtime.writable(name).await.is_none() {
        match runtime.adopt_now(name).await {
            None => return Ok(false),
            // É do Rust e não abriu: a falha aparece com o código dela, nunca vira repasse.
            Some(Err(error)) => return Err(error),
            Some(Ok(())) => {}
        }
    }
    Ok(runtime.writable(name).await.is_some_and(|t| codex_owned(&t.provider, t.terminal)))
}

/// A saúde fica com a admissão; aqui só provedor e modo, pela mesma tabela que anuncia a posse.
fn codex_owned(provider: &str, terminal: bool) -> bool {
    provider == "codex" && decide(WriteRoute::CodexControl, Provider::Codex, terminal) == Owner::Rust
}

/// Só a sessão Codex do Rust passa pela porta; o resto vai ao Python como chegou.
async fn admit_codex(st: &Arc<AppState>, peer: SocketAddr, req: Request) -> Result<(Ctx, Bytes), Response> {
    admit_codex_as(st, peer, req, WriteRoute::CodexControl).await
}

async fn admit_codex_as(st: &Arc<AppState>, peer: SocketAddr, req: Request, route: WriteRoute) -> Result<(Ctx, Bytes), Response> {
    let (fwd, owner) = gate(st, peer, &req);
    // Só o dono abre ou adota: o convidado segue ao Python, que o autentica.
    if !owner { return Err(pass(st, req, &fwd).await); }
    match ours(st, session_name(req.uri().path()).as_deref()).await {
        Ok(true) => admit(st, peer, req, route).await,
        Ok(false) => Err(pass(st, req, &fwd).await),
        Err(error) => {
            let mut response = json_response(super::failed_status(&error), detail_body(&error.code, &error.message, json!({})));
            cors(req.headers(), response.headers_mut());
            Err(response)
        }
    }
}

async fn run(ctx: &Ctx, kind: OperationKind, payload: Value) -> Result<RuntimeReply, RuntimeError> {
    ctx.target.handle.command(RuntimeCommand { operation_id: random_hex(16), kind, payload }).await
}

/// Falha que a resposta engole (lista vazia, limites neutros) ainda vai ao log, como no adapter Python.
fn note(name: &str, what: &str, sent: &Result<RuntimeReply, RuntimeError>) {
    if accepted(sent).is_some() { return; }
    let code = match sent { Err(error) => error.code.clone(), Ok(reply) => format!("{:?}", reply.disposition).to_lowercase() };
    if crate::warn_limit::allow(Some(name), &format!("{what}:{code}")) {
        tracing::warn!(session = %name, what, code = %code, "leitura do Codex falhou; a rota responde vazio");
    }
}

macro_rules! admitted {
    ($st:expr, $peer:expr, $req:expr) => {
        match admit_codex(&$st, $peer, $req).await { Ok(admitted) => admitted, Err(response) => return response }
    };
}

pub async fn models(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, _) = admitted!(st, peer, req);
    let settings = run(&ctx, OperationKind::ReadSettings, json!({"include_turns": false})).await;
    if accepted(&settings).is_none() { log_outcome(&ctx.name, "settings", &settings, "leitura de configuração do Codex falhou; a rota responde 409 erro_codex_controle"); return answer(&ctx, refused()); }
    let models = run(&ctx, OperationKind::ListModels, json!({})).await;
    note(&ctx.name, "model_list", &models);
    answer(&ctx, models_answer(&settings, &models))
}

pub async fn model(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = admitted!(st, peer, req);
    let Some(body) = object(&bytes, ctx.headers()).and_then(|f| strings(&f, "model", Some("effort"))) else { return invalid_body(ctx.headers()) };
    let payload = json!({"model": body["model"], "effort": body.get("effort").cloned().unwrap_or(Value::Null)});
    let sent = run(&ctx, OperationKind::SetModel, payload).await;
    log_outcome(&ctx.name, "model", &sent, "troca de modelo do Codex não aceita; a rota responde 409 erro_codex_controle");
    answer(&ctx, model_answer(&sent))
}

pub async fn service_tier(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = admitted!(st, peer, req);
    let Some(tier) = object(&bytes, ctx.headers()).and_then(|f| literal(&f, "service_tier", &["default", "priority"])) else { return invalid_body(ctx.headers()) };
    let sent = run(&ctx, OperationKind::SetServiceTier, json!({"service_tier": tier})).await;
    log_outcome(&ctx.name, "service_tier", &sent, "troca de service tier do Codex não aceita; a rota responde 409 erro_codex_controle");
    answer(&ctx, service_tier_answer(&tier, &sent))
}

pub async fn mode(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = admitted!(st, peer, req);
    let Some(mode) = object(&bytes, ctx.headers()).and_then(|f| literal(&f, "mode", &["default", "plan"])) else { return invalid_body(ctx.headers()) };
    let sent = run(&ctx, OperationKind::SetMode, json!({"mode": mode})).await;
    log_outcome(&ctx.name, "mode", &sent, "troca de modo do Codex não aceita; a rota responde 409 erro_codex_controle");
    answer(&ctx, mode_answer(&sent))
}

/// `/limits` nunca é erro nem espera, como no Python: porta fechada (troca em curso) ou leitura que falha
/// dão a resposta neutra.
pub async fn limits(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, who) = who(&st, peer, &req);
    let name = session_name(req.uri().path()).filter(|n| who.as_ref().is_some_and(|c| c.can(n, req.method())));
    let (Some(runtime), Some(name)) = (st.state.runtime.get(), name) else { return pass(&st, req, &fwd).await };
    match ours(&st, Some(&name)).await {
        Ok(true) => {}
        Ok(false) => return pass(&st, req, &fwd).await,
        // Conversa ainda abrindo: ainda não há cota a ler, e a resposta é a neutra.
        Err(error) if error.code == crate::runtime::codex_relaunch::STARTING => {}
        // Do Rust e não abriu: o código da falha, como nas outras rotas do Codex, nunca a resposta neutra.
        Err(error) => {
            let mut response = json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body(&error.code, &error.message, json!({})));
            cors(req.headers(), response.headers_mut());
            return response;
        }
    }
    let sent = match enter_then_find(runtime, &name, Duration::ZERO).await {
        Ok(Some((_held, target))) if codex_owned(&target.provider, target.terminal) =>
            target.handle.command(RuntimeCommand { operation_id: random_hex(16), kind: OperationKind::ReadRateLimits, payload: json!({}) }).await,
        _ => Err(RuntimeError::new("session_transfer_busy", "porta de entrada fechada")),
    };
    note(&name, "rate_limits", &sent);
    let (status, body) = limits_answer(&sent);
    let mut response = json_response(status, body);
    cors(req.headers(), response.headers_mut());
    response
}

pub async fn skip_question(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = admitted!(st, peer, req);
    let Some(body) = object(&bytes, ctx.headers()).and_then(|f| strings(&f, "request_id", Some("interaction_id"))) else { return invalid_body(ctx.headers()) };
    // O motor exige o `interaction_id` da pergunta exibida; sem ele responde 409 sem identidade.
    let sent = run(&ctx, OperationKind::SkipQuestion, json!({"request_id": body["request_id"], "interaction_id": body.get("interaction_id")})).await;
    log_outcome(&ctx.name, "question_skip", &sent, "pular pergunta do Codex não confirmado");
    answer(&ctx, skip_answer(&sent))
}

pub async fn commands(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, _) = admitted!(st, peer, req);
    let sent = run(&ctx, OperationKind::ListSkills, json!({})).await;
    log_outcome(&ctx.name, "commands", &sent, "lista de comandos do Codex falhou; a rota responde 409 erro_codex_controle");
    answer(&ctx, commands_answer(&sent))
}

pub async fn permissions(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, _) = match admit_codex_as(&st, peer, req, WriteRoute::CodexPermission).await { Ok(admitted) => admitted, Err(response) => return response };
    if matches!(ctx.target.handle, EntryHandle::CodexTerminal { .. }) {
        let result = terminal_permission(&ctx, PermissionAction::Read { probe: false }).await;
        return answer(&ctx, result);
    }
    // O modo do motor, que vale com o processo caído; a vista publicada só chega depois de gravada e
    // logo após abrir ou trocar o modo ainda traria o anterior.
    let result = match ctx.target.handle.view().await {
        // O modo gravado: com o processo caído, a troca em gravação só vale quando o arquivo a aceitar.
        Ok(view) => (StatusCode::OK, permission_modes(view["permission_mode_saved"].as_str().or(view["permission_mode"].as_str()))),
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_permissao_picker", &error.to_string(), json!({}))),
    };
    answer(&ctx, result)
}

pub async fn set_permission(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, bytes) = match admit_codex_as(&st, peer, req, WriteRoute::CodexPermission).await { Ok(admitted) => admitted, Err(response) => return response };
    let Some(body) = object(&bytes, ctx.headers()).and_then(|f| strings(&f, "mode", None)) else { return invalid_body(ctx.headers()) };
    if matches!(ctx.target.handle, EntryHandle::CodexTerminal { .. }) {
        let mode = body["mode"].as_str().unwrap_or_default().to_owned();
        let result = terminal_permission(&ctx, PermissionAction::Set { mode }).await;
        return answer(&ctx, result);
    }
    let sent = run(&ctx, OperationKind::SetPermissionMode, json!({"mode": body["mode"]})).await;
    log_outcome(&ctx.name, "permission", &sent, "troca de permissão do Codex não aceita");
    answer(&ctx, permission_answer(&sent))
}

const MSG_PLAN_NOT_OPEN: &str = "O seletor de implementação não está aberto na sessão.";
const MSG_PLAN_STILL_OPEN: &str = "O Codex não fechou o seletor de implementação.";
/// Espera do seletor aparecer e de fechar depois do Enter, a mesma do Python.
const PLAN_MENU_WAIT: Duration = Duration::from_secs(2);

fn implement_menu(screen: &str) -> Option<bool> {
    let menu = crate::terminal_state::analyze(screen).codex_menu?;
    Some(menu.question.as_deref() == Some("Implement this plan?") && menu.options.first().is_some_and(|o| o.starts_with("Yes, implement this plan")))
}

/// `POST /codex/plan/implement`: o "Yes, implement this plan" do seletor da TUI, pela trava do pane desta
/// vida (a mesma de `/keys`, `/term-input` e do seletor de permissões). Sem terminal não há seletor.
pub async fn plan_implement(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (ctx, _) = admitted!(st, peer, req);
    let not_open = || (StatusCode::CONFLICT, detail_body("erro_codex_controle", MSG_PLAN_NOT_OPEN, json!({})));
    let EntryHandle::CodexTerminal { target, cell, .. } = &ctx.target.handle else { return answer(&ctx, not_open()) };
    if ctx.st.term.is_active(&ctx.name) { return answer(&ctx, super::control::panel_open_refusal()); }
    let driver = crate::runtime::codex_terminal::pane_driver(&target.terminal, cell.pane_serial(), Some(ctx.st.term.clone()));
    let deadline = tokio::time::Instant::now() + PLAN_MENU_WAIT;
    loop {
        let screen = driver.capture().await.unwrap_or_default();
        match implement_menu(&screen) {
            Some(true) => break,
            // Outro seletor aberto: não é o do plano.
            Some(false) => return answer(&ctx, not_open()),
            None if tokio::time::Instant::now() >= deadline => return answer(&ctx, not_open()),
            None => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let selected = driver.select(1, true).await;
    use crate::terminal_input::Disposition as Pane;
    let result = match selected.disposition {
        Pane::Accepted => (StatusCode::OK, json!({"ok": true})),
        Pane::Unknown => (StatusCode::CONFLICT, detail_body("erro_codex_controle", MSG_PLAN_STILL_OPEN, json!({}))),
        _ => (StatusCode::CONFLICT, detail_body("erro_opcao_nao_convergiu", "não consegui iniciar o plano pelo terminal", json!({"detalhe": selected.code}))),
    };
    if result.0 != StatusCode::OK && crate::warn_limit::allow(Some(&ctx.name), &format!("codex_plan:{}", selected.code)) {
        tracing::warn!(session = %ctx.name, code = %selected.code, "seletor de implementação do plano não convergiu");
    }
    answer(&ctx, result)
}

const MSG_PERMISSION_BUSY: &str = "a sessão está trabalhando — espere ela terminar";

/// Corpo e código do `api.py` para o seletor da TUI: modo desconhecido é 400, o resto do seletor 409.
pub fn terminal_permission_answer(sent: &Result<Value, RuntimeError>) -> Answer {
    match sent {
        Ok(value) => (StatusCode::OK, value.clone()),
        Err(error) if error.code == "terminal_panel_open" => super::control::panel_open_refusal(),
        Err(error) if error.code == "erro_modo_desconhecido" => (StatusCode::BAD_REQUEST, detail_body("erro_permissao_picker", &error.message, json!({}))),
        Err(error) => (StatusCode::CONFLICT, detail_body("erro_permissao_picker", &error.message, json!({}))),
    }
}

/// Seletor do `/permissions` no pane, na trava da vida que `/keys` e `/term-input` também usam. Turno em
/// voo recusa antes de tocar no pane: o `/permissions` digitado viraria mensagem enfileirada.
///
/// Ordem das travas: passe da porta de entrada (`admitted!`, nunca fechada daqui) → retenção de turnos
/// do ator (`hold_turns`) → trava do pane. O ator não toma a trava do pane, e a retenção vale a
/// sequência inteira: a fila e as respostas reservadas não abrem `turn/start` no meio do seletor.
async fn terminal_permission(ctx: &Ctx, action: PermissionAction) -> Answer {
    let EntryHandle::CodexTerminal { target, cell, handle } = &ctx.target.handle else { return refused() };
    if ctx.st.term.is_active(&ctx.name) { return super::control::panel_open_refusal(); }
    // Ator caído não despacha nada: segue sem retenção e a leitura do pane decide, como antes.
    let _turns = match handle.hold_turns().await {
        Ok(hold) => Some(hold),
        Err(error) if error.code == crate::runtime::actor::TURN_HOLD_BUSY =>
            return (StatusCode::CONFLICT, detail_body("erro_permissao_ocupada", MSG_PERMISSION_BUSY, json!({}))),
        Err(_) => None,
    };
    let driver = crate::runtime::codex_terminal::pane_driver(&target.terminal, cell.pane_serial(), Some(ctx.st.term.clone()));
    let sent = crate::permission_control::codex(&driver, action).await;
    if let Err(error) = &sent && crate::warn_limit::allow(Some(&ctx.name), &format!("codex_permission:{}", error.code)) {
        tracing::warn!(session = %ctx.name, code = %error.code, message = %error.message, "seletor de permissões do Codex falhou");
    }
    terminal_permission_answer(&sent)
}
