//! Pontes privadas do Rust para o que entra pelo Python (convidado 8766, Connect 8768, broadcast, grupo,
//! par, MCP): a do Codex (com e sem terminal), que também recebe o `DELETE` que ele administra, e a das
//! entrada, respostas de pergunta e painel da fila do Claude. O Python já autenticou e só repassa, num sentido: aqui a escrita roda
//! como a do dono, com o corpo intacto, e o que o Rust não atende volta como 421, para o Python tratar
//! sozinho (nunca outro repasse ao Rust).
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use axum::body::Body;
use serde_json::{Value, json};

use crate::routes::AppState;

pub(crate) use hangar_core::edge::BridgedOwner;

fn not_ours() -> Response { StatusCode::MISDIRECTED_REQUEST.into_response() }

/// Nome (sem o escape) e rota de `<prefixo><nome>/<rota>`.
fn split(req: &Request, prefix: &str) -> Option<(String, String)> {
    let (raw, rest) = req.uri().path().strip_prefix(prefix)?.split_once('/')?;
    Some((percent_decode_str(raw).decode_utf8().ok()?.into_owned(), rest.to_owned()))
}

/// Vira o pedido do dono em `/api/sessions/<nome>/<rota>`, com a mesma consulta e sem as credenciais de
/// quem chegou ao Python.
fn as_owner(req: &mut Request, name: &str, rest: &str) -> bool {
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let Ok(uri) = format!("/api/sessions/{}/{rest}{query}", utf8_percent_encode(name, NON_ALPHANUMERIC)).parse::<Uri>() else { return false };
    *req.uri_mut() = uri;
    for header in ["x-hangar-internal", "x-forwarded-for", "authorization", "cookie"] { req.headers_mut().remove(header); }
    req.extensions_mut().insert(BridgedOwner);
    true
}

/// `/__hangar_server/claude/<nome>/<rota>`: `POST` em `input`, `answer`, `select`, `select/submit`,
/// `permission-mode` e `model-effort`, `GET` em `permission-modes`, e as rotas do painel da fila (`DELETE`/`PATCH
/// queue/<id>`, `POST queue/reorder` e `queue/<id>/steer`), de sessão Claude com entrada no Rust, pela
/// mesma rota do dono; `POST`/`GET btw` e `POST recarregar` de qualquer sessão (a rota decide); `POST close`, o
/// `DELETE` do Claude aberto aqui que entrou pelo Connect. Outra rota, método ou sessão: 421.
pub async fn claude(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    use super::{answer, control, input, permissions};
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let Some((name, rest)) = split(&req, "/__hangar_server/claude/") else { return not_ours() };
    // Abrir Claude com terminal pelo que entrou direto no Python (Connect, MCP): o nome vem no corpo.
    if rest == "open" && req.method() == Method::POST {
        return crate::session_open::server::bridge(State(st), ConnectInfo(peer), req).await;
    }
    if rest == "close" && req.method() == Method::POST { return close_bridged(st, peer, &name, req.headers()).await; }
    // Reabrir que entrou pelo Connect: a rota da 8765 decide Codex, Claude e o que segue ao Python (421).
    if rest == "recarregar" && req.method() == Method::POST {
        let line = asked_line("sessao.recarga_pedida", "session_reload_requested_by_owner", &name, req.headers(), peer);
        crate::diag::record(crate::diag::diary_dir(), vec![line], "sessão recarregada");
        if !as_owner(&mut req, &name, &rest) { return not_ours(); }
        return super::relaunch::reload(State(st), ConnectInfo(peer), req).await;
    }
    // Trocar o nome pelo que entrou no Python (Connect, convidado com conta): a rota inteira é do Rust.
    if rest == "rename" && req.method() == Method::POST {
        if !as_owner(&mut req, &name, &rest) { return not_ours(); }
        return crate::session_rename::route(State(st), ConnectInfo(peer), req).await;
    }
    // Pergunta lateral (`POST`/`GET btw`): a própria rota decide sessão, provedor e modo, inclusive o
    // Claude sem terminal parado, que ainda não tem entrada.
    if rest == "btw" && matches!(*req.method(), Method::POST | Method::GET) {
        if !as_owner(&mut req, &name, &rest) { return not_ours(); }
        let (s, c) = (State(st), ConnectInfo(peer));
        return if req.method() == Method::POST { super::btw::ask(s, c, req).await } else { super::btw::list(s, c, req).await };
    }
    let queue = control::queue_route(req.method(), &rest);
    let allowed = queue.is_some() || match *req.method() {
        Method::POST => matches!(rest.as_str(), "input" | "answer" | "select" | "select/submit" | "permission-mode" | "model-effort"),
        Method::GET => rest == "permission-modes",
        _ => false,
    };
    if !allowed { return not_ours(); }
    let Some(runtime) = st.state.runtime.get().cloned() else { return not_ours() };
    // Texto que nasce no Python (grupo, par, MCP) para o Claude com terminal: o Rust o adota aqui, e a
    // fila da sessão é uma só. O seletor do `/model` do convidado também só existe no ator.
    if matches!(rest.as_str(), "input" | "model-effort") && !runtime.has_entry(None, &name).await {
        match runtime.adopt_now(&name).await {
            None => return not_ours(),
            // Sem sidecar o Python toma o Codex que ainda abre por Claude: o `/input` o guarda na fila dele.
            // A adormecida também: o `/input` guarda o texto na fila dela, e o acordar já corre à parte.
            Some(Err(error)) if (error.code == crate::runtime::codex_relaunch::STARTING || error.code == crate::runtime::gateway::ASLEEP_WAKING) && rest == "input" => {
                if !as_owner(&mut req, &name, &rest) { return not_ours(); }
                return input::input(State(st), ConnectInfo(peer), req).await;
            }
            Some(Err(error)) => return super::detail(super::failed_status(&error), &error.code, &error.message, json!({})),
            Some(Ok(())) => {}
        }
    }
    if !runtime.writable(&name).await.is_some_and(|target| target.provider == "claude") { return not_ours(); }
    if !as_owner(&mut req, &name, &rest) { return not_ours(); }
    if let Some(route) = queue { return control::queue_dispatch(route, st, peer, req).await; }
    let (s, c) = (State(st), ConnectInfo(peer));
    match rest.as_str() {
        "input" => input::input(s, c, req).await,
        "answer" => answer::answer(s, c, req).await,
        "select" => control::select(s, c, req).await,
        "permission-mode" => permissions::set_mode(s, c, req).await,
        "permission-modes" => permissions::modes(s, c, req).await,
        "model-effort" => super::engine::effort(s, c, req).await,
        _ => control::select_submit(s, c, req).await,
    }
}

/// `/__hangar_server/codex/<nome>/<rota>`: `kill` (POST) encerra a vida; o resto é a rota de
/// `/api/sessions/<nome>/<rota>`, com o mesmo método, corpo e consulta.
pub async fn codex(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let Some((name, rest)) = split(&req, "/__hangar_server/codex/") else { return not_ours() };
    let Some(runtime) = st.state.runtime.get().cloned() else { return not_ours() };
    let method = req.method().as_str().to_owned();
    let outcome = match (method.as_str(), rest.as_str()) {
        ("POST", "kill") => runtime.kill_codex_terminal(&name).await,
        _ => {
            // Leitura não acorda a adormecida: a rota responde por ela.
            let asleep = if method == "GET" { runtime.asleep(&name).await } else { None };
            // Adormecida de outro agente não é desta rota, e ler não a acorda.
            if asleep.as_ref().is_some_and(|mark| mark.provider != "codex") { return not_ours(); }
            let reading = asleep.is_some();
            if !reading && !runtime.owns_codex_terminal(&name) && !runtime.owns_codex_headless(&name).await {
                match runtime.adopt_now(&name).await {
                    None => return not_ours(),
                    // Conversa ainda abrindo ou adormecida acordando: a rota decide (texto vai à fila, o resto espera).
                    Some(Err(error)) if error.code == crate::runtime::codex_relaunch::STARTING
                        || error.code == crate::runtime::gateway::ASLEEP_WAKING => {}
                    Some(Err(error)) => return super::detail(super::failed_status(&error), &error.code, &error.message, json!({})),
                    Some(Ok(())) => {}
                }
            }
            if !as_owner(&mut req, &name, &rest) { return not_ours(); }
            return dispatch(st, peer, &method, &rest, req).await;
        }
    };
    match outcome {
        None => not_ours(),
        Some(Ok(value)) => super::json_response(StatusCode::OK, value),
        Some(Err(error)) => super::detail(StatusCode::CONFLICT, &error.code, &error.message, json!({})),
    }
}

/// Marca do `DELETE` que o Rust já encerrou e repassa: o valor é o segredo interno, e o Python, ao vê-la,
/// limpa só as lojas dele, sem voltar ao Rust.
pub const RUST_CLOSED: &str = "x-hangar-rust-closed";
/// Transcript (percent-encoded) da sessão que o Rust encerrou, junto da marca.
pub const CLOSED_TRANSCRIPT: &str = "x-hangar-closed-transcript";

/// `DELETE /api/sessions/{name}` do dono. Toda sessão encerra aqui: Pi/omp/Kimi pelo ator; Codex e Claude
/// abertos no Rust pela vida guardada; o resto (sem terminal parado, agente morto, pane de outro programa, a
/// que já acabou) pelo nome. Depois do encerramento, a saída do grupo e o pedido marcado ao Python, para
/// "then", terminais de atalho e caches dele. Falha no encerramento responde com o código e não repassa.
pub async fn delete(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    let (fwd, owner) = match crate::routes::gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    // Só o próprio Rust põe a marca: a que vier de fora sai antes de qualquer decisão.
    req.headers_mut().remove(RUST_CLOSED);
    req.headers_mut().remove(CLOSED_TRANSCRIPT);
    let name = super::session_name(req.uri().path());
    if owner && let Some(name) = name.as_deref()
        && let Some(response) = super::orchestration_guard(&st, name, req.headers()).await {
        return response;
    }
    // A chave antes do kill (depois dele o pane não a diz mais); os arquivos do cofre saem só no sucesso.
    let vault_files = match (owner, name.as_deref()) { (true, Some(name)) => crate::vault::files::before_close(&st, name).await, _ => None };
    if let (Some(runtime),Some(name))=(st.state.runtime.get(),name.as_deref()) {
        let (_,caller)=match crate::routes::who(&st,peer,&req).await {Ok(value)=>value,Err(error)=>return crate::auth::unavailable(error)};
        if caller.as_ref().is_some_and(|caller|caller.can(name,req.method())) && let Some(closed)=runtime.close_other(name).await {
            match closed {
                Ok(body)=>{
                    if let Some(files) = vault_files { files.done(); }
                    if owner { closed_line(name, req.headers(), peer); }
                    return super::json_response(StatusCode::OK,body);
                },
                // A sessão já acabou: fecha pelo nome, como qualquer outra que acabou (D17).
                Err(error) if error.code=="other_terminal_dead"=>{},
                Err(error)=>return close_failed(&st, name, &error, req.headers()),
            }
        }
    }
    let (Some(runtime), true, Some(name)) = (st.state.runtime.get().cloned(), owner, name)
        else { return vault_files_after(vault_files, crate::routes::pass(&st, req, &fwd).await) };
    // Na janela da subida, a vigia é quem lê o sidecar do Codex: sem ela o fechamento pelo nome deixaria o
    // cano de um Codex sem terminal vivo. Subida que não liga vigia segue pelo nome.
    if let Err(error) = runtime.wait_watcher().await { return close_failed(&st, &name, &error, req.headers()); }
    // O nome e a porta de entrada antes de qualquer encerramento: abertura e escrita do mesmo nome esperam.
    let closing = match runtime.begin_close(st.list.env().dirs.as_ref().map(|dirs| dirs.home.clone()), &name).await {
        Ok(closing) => closing,
        Err(error) => return close_failed(&st, &name, &error, req.headers()),
    };
    // Claude aberto aqui (com ou sem terminal) também fecha aqui: só matar pelo nome deixava a entrada da
    // vida morta presa ao nome, e a próxima sessão com ele herdava a conversa dela.
    let closed = match runtime.kill_codex_terminal(&name).await {
        Some(codex) => codex,
        None => {
            // A conversa da vida guardada, lida antes de a entrada sair do registro.
            let known = runtime.context_transcript(&name).await.ok().map(|(_, path)| path);
            match runtime.kill_claude(&name).await {
                Some(claude) => claude,
                None => runtime.close_by_name(name_env(&st), &name, known, listed_transcript(&st, &name)).await,
            }
        }
    };
    drop(closing);
    let closed = match closed {
        Ok(closed) => closed,
        Err(error) => return close_failed(&st, &name, &error, req.headers()),
    };
    // Fechada de propósito não volta no próximo reboot, de qualquer agente; recusada, segue voltando.
    if let Some(dirs) = st.list.env().dirs.as_ref() { crate::session_open::restore::forget(&dirs.home, &name); }
    closed_line(&name, req.headers(), peer);
    if let Some(files) = vault_files { files.done(); }
    // Só o caminho da conversa, para o Python guardar o nome fechado; sem ele, o Python registra a falta.
    if let Some(path) = closed["transcript"].as_str() {
        let encoded = percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string();
        if let Ok(value) = axum::http::HeaderValue::from_str(&encoded) { req.headers_mut().insert(CLOSED_TRANSCRIPT, value); }
    }
    let leave = match st.groups.clone() { Some(groups) => leave_warnings(&st, &groups, &name).await, None => Vec::new() };
    let share = after_close(&st, &name).await;
    match axum::http::HeaderValue::from_str(&st.cfg.internal_secret) {
        Ok(mark) => { req.headers_mut().insert(RUST_CLOSED, mark); }
        // Sem a marca o Python tentaria encerrar de novo pela ponte: não repassa, e a limpeza dele que
        // não rodou vai no aviso.
        Err(_) => {
            tracing::error!(session = %name, code = "close_python_cleanup_skipped", "segredo interno não vira cabeçalho; limpeza do Python não rodou");
            let warning = json!({"code": "close_python_cleanup_skipped", "params": {},
                "msg": "Sessão encerrada, mas convites, atalhos e caches dela não foram limpos."});
            let mut response = super::json_response(StatusCode::OK, json!({"ok": true, "warning": warning}));
            crate::routes::cors(req.headers(), response.headers_mut());
            return with_warnings(&name, response, leave, share).await;
        }
    }
    let response = crate::routes::pass(&st, req, &fwd).await;
    with_warnings(&name, response, leave, share).await
}

/// Pastas e multiplexador da lista para o fechamento pelo nome sem a vigia do Codex.
fn name_env(st: &AppState) -> Option<crate::runtime::close_name::NameEnv> {
    let env = st.list.env();
    env.dirs.as_ref().map(|dirs| crate::runtime::close_name::NameEnv { home: dirs.home.clone(),
        queue_dir: dirs.claude.join(".hangar-queue"), mux_program: env.capture_program.clone() })
}

/// A conversa que a lista conhece para o nome (o `_jsonl_of_session` do Python).
async fn listed_transcript(st: &AppState, name: &str) -> Option<std::path::PathBuf> {
    st.list.discovered_session(name).await.ok().flatten().and_then(|(row, _)| row.jsonl).map(std::path::PathBuf::from)
}

/// Os arquivos do cofre da sessão fechada pelo Python saem só com o fechamento confirmado.
fn vault_files_after(files: Option<crate::vault::files::Closing>, response: Response) -> Response {
    if let Some(files) = files.filter(|_| response.status().is_success()) { files.done(); }
    response
}

/// Sai do grupo; os avisos que não saíram e a falha da própria saída viram `{sessao, erro}`.
async fn leave_warnings(st: &AppState, groups: &crate::groups::service::GroupService, name: &str) -> Vec<Value> {
    match crate::groups::exit::leave_and_notify(st, groups, name).await {
        Ok((_, warnings)) => warnings,
        Err(error) => {
            tracing::warn!(session = %name, error = %error, "saída do grupo falhou ao encerrar a sessão");
            let code = crate::groups::bridge::group_failed(error).code;
            vec![json!({"sessao": name, "erro": {"code": "erro_grupo_indisponivel", "params": {"detalhe": code},
                "msg": "os grupos estão indisponíveis agora"}})]
        }
    }
}

/// Os avisos de saída e o do convite (`share`) no `warning` da resposta do fechamento, na ordem do Python;
/// corpo que não é objeto JSON vira `{"ok": true}`.
async fn with_warnings(name: &str, response: Response, mut errs: Vec<Value>, share: Option<Value>) -> Response {
    if errs.is_empty() && share.is_none() { return response; }
    let (mut parts, body) = response.into_parts();
    let read = axum::body::to_bytes(body, 1 << 20).await.ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let mut value = read.filter(Value::is_object).unwrap_or_else(|| {
        tracing::warn!(session = %name, avisos = errs.len(), "corpo do fechamento ilegível; a resposta leva só os avisos de saída");
        json!({"ok": true})
    });
    let current = value.get("warning").filter(|w| !w.is_null()).cloned();
    value["warning"] = match share {
        Some(share) if errs.is_empty() && current.is_none() => share,
        share => {
            errs.extend(share.map(|share| json!({"sessao": name, "erro": share})));
            with_leave_warning(current, name, errs)
        }
    };
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    parts.headers.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/json"));
    Response::from_parts(parts, Body::from(value.to_string()))
}

/// O `warning` do `kill_session` do Python: os avisos de saída, com o aviso que já havia no fim.
fn with_leave_warning(current: Option<Value>, name: &str, mut errs: Vec<Value>) -> Value {
    if let Some(current) = current { errs.push(json!({"sessao": name, "erro": current})); }
    let text = crate::groups::routes::failures_text(&errs);
    json!({"code": "erro_pareamento_saida_falhou", "params": {"avisos": errs}, "msg": format!("aviso de saída falhou: {text}")})
}

fn close_refusal(error: &crate::runtime::protocol::RuntimeError) -> Response {
    if error.code == crate::runtime::close_name::NAME_BUSY {
        return super::detail(StatusCode::CONFLICT, &error.code, &error.message, json!({}));
    }
    match super::busy_refusal(error) {
        Some((status, body)) => super::json_response(status, body),
        None => super::detail(StatusCode::INTERNAL_SERVER_ERROR, &error.code, &error.message, json!({})),
    }
}

/// O fechamento do dono recusado: o código vai ao diário e na resposta, e nada segue ao Python.
fn close_failed(st: &AppState, name: &str, error: &crate::runtime::protocol::RuntimeError, headers: &axum::http::HeaderMap) -> Response {
    st.diag.report("rust.close_failed", name, &error.code, "o fechamento da sessão falhou");
    let mut response = close_refusal(error);
    crate::routes::cors(headers, response.headers_mut());
    response
}

/// O fechamento que deu certo vai ao diário com o cliente e a origem.
fn closed_line(name: &str, headers: &axum::http::HeaderMap, peer: SocketAddr) {
    crate::diag::record(crate::diag::diary_dir(), vec![close_line(name, headers, peer)], "sessão fechada");
}

/// Depois de o Rust encerrar a sessão: revoga os convites e some da lista. A saída do grupo é de quem chamou.
/// Devolve o aviso da revogação que falhou: o fechamento segue, e a pessoa fica sabendo.
async fn after_close(st: &Arc<AppState>, name: &str) -> Option<Value> {
    let mut warning = None;
    // Convites e par da sessão são do Rust: o Python marcado não volta a pedir a revogação.
    if let Some(share) = st.share.clone() {
        match share.store.revoke_session(name, None) {
            Ok(0) => {}
            Ok(_) => crate::share::owner_routes::sync_tunnel(&share).await,
            Err(error) => {
                tracing::error!(session = %name, code = "share_store_failed", "convites da sessão encerrada não revogados: {error}");
                warning = Some(json!({"code": "erro_convite_nao_acompanhou", "params": {"codigo": "share_store_failed"},
                    "msg": "o convite desta sessão não acompanhou a mudança (share_store_failed)"}));
            }
        }
    }
    // O loop é da vida que acabou: nada de acordar na próxima sessão com o nome.
    if let Err(error) = crate::loops::store(st).forget(name) {
        tracing::error!(session = %name, code = "loop_forget_failed", kind = ?error.kind(), "loop da sessão encerrada não saiu");
    }
    st.list.forget(name);
    st.list.invalidate();
    warning
}

/// O `DELETE` do Claude que entrou pelo Connect (8768): o encerramento da 8765, sem repasse e sem a saída
/// do grupo, que o Python faz depois com a fase do `orq` já lida (o Rust não pergunta de volta numa chamada
/// dele). 421 = não é Claude aberto aqui, e o Python segue.
async fn close_bridged(st: Arc<AppState>, peer: SocketAddr, name: &str, headers: &axum::http::HeaderMap) -> Response {
    let Some(runtime) = st.state.runtime.get().cloned() else { return not_ours() };
    let closed = runtime.kill_claude(name).await;
    match &closed {
        Some(Err(error)) => st.diag.report("rust.close_failed", name, &error.code, "o fechamento da sessão falhou"),
        _ => { crate::diag::record(crate::diag::diary_dir(), vec![close_line(name, headers, peer)], "sessão fechada"); }
    }
    // Recusado, a sessão segue viva e volta no reboot; no 421 o Python a fecha pelo nome, então ela sai.
    if !matches!(closed, Some(Err(_))) && let Some(dirs) = st.list.env().dirs.as_ref() {
        crate::session_open::restore::forget(&dirs.home, name);
    }
    match closed {
        None => not_ours(),
        Some(Err(error)) => close_refusal(&error),
        Some(Ok(mut closed)) => {
            if let Some(warning) = after_close(&st, name).await { closed["warning"] = warning; }
            super::json_response(StatusCode::OK, closed)
        }
    }
}

/// As rotas que o Rust atende para o Codex dele; outra cai no Python (421).
async fn dispatch(st: Arc<AppState>, peer: SocketAddr, method: &str, rest: &str, req: Request) -> Response {
    use super::{answer, codex, control, input, relaunch};
    if let Some(route) = control::queue_route(req.method(), rest) { return control::queue_dispatch(route, st, peer, req).await; }
    let (s, c) = (State(st), ConnectInfo(peer));
    match (method, rest) {
        ("POST", "input") => input::input(s, c, req).await,
        ("POST", "steer") => input::steer(s, c, req).await,
        ("POST", "interrupt") => control::interrupt(s, c, req).await,
        ("POST", "select") => control::select(s, c, req).await,
        ("POST", "select/submit") => control::select_submit(s, c, req).await,
        ("POST", "answer") => answer::answer(s, c, req).await,
        ("POST", "keys") => control::keys(s, c, req).await,
        ("POST", "term-input") => control::term_input(s, c, req).await,
        ("GET", "models") => codex::models(s, c, req).await,
        ("POST", "model") => codex::model(s, c, req).await,
        ("POST", "service-tier") => codex::service_tier(s, c, req).await,
        ("POST", "codex/mode") => codex::mode(s, c, req).await,
        ("POST", "codex/plan/implement") => codex::plan_implement(s, c, req).await,
        ("GET", "limits") => codex::limits(s, c, req).await,
        ("POST", "question/skip") => codex::skip_question(s, c, req).await,
        ("POST", "codex/terminate") => codex::terminate(s, c, req).await,
        ("GET", "commands") => codex::commands(s, c, req).await,
        ("GET", "codex-permissions") => codex::permissions(s, c, req).await,
        ("POST", "codex-permissions") => codex::set_permission(s, c, req).await,
        ("POST", "modo-execucao") => relaunch::mode(s, c, req).await,
        ("POST", "recarregar") => relaunch::reload(s, c, req).await,
        _ => not_ours(),
    }
}

/// Quem pediu o fechamento: o cliente e a origem que a requisição traz (o dono não manda identidade própria).
/// No `DELETE` do dono a linha sai depois do encerramento; a falha vai como `rust.close_failed`, com o código.
fn close_line(name: &str, headers: &axum::http::HeaderMap, peer: SocketAddr) -> serde_json::Value {
    asked_line("sessao.fechamento_pedido", "session_close_requested_by_owner", name, headers, peer)
}

fn asked_line(evento: &str, codigo: &str, name: &str, headers: &axum::http::HeaderMap, peer: SocketAddr) -> serde_json::Value {
    let header = |key: &str| headers.get(key).and_then(|v| v.to_str().ok()).map(|v| v.chars().take(80).collect::<String>());
    json!({"evento": evento, "nivel": "ok", "sessao": name, "codigo": codigo,
        "cliente": header("user-agent"), "req": header("x-hangar-req"), "origem": header("x-forwarded-for").unwrap_or_else(|| peer.ip().to_string())})
}

#[cfg(test)]
mod close_tests {
    /// O aviso que o `kill_session` montava com os avisos da ponte: o de convite que já havia vai no fim.
    #[test]
    fn leave_warning_keeps_the_python_warning_last() {
        use serde_json::json;
        let err = json!({"sessao": "lab::x", "erro": {"code": "erro_peer_nao_avisado", "params": {}, "msg": "lab inacessível"}});
        let share = json!({"code": "erro_x", "params": {}, "msg": "convite"});
        let warning = super::with_leave_warning(Some(share.clone()), "s0", vec![err.clone()]);
        assert_eq!(warning, json!({"code": "erro_pareamento_saida_falhou", "params": {"avisos": [err, {"sessao": "s0", "erro": share}]},
            "msg": "aviso de saída falhou: lab::x: lab inacessível; s0: convite"}));
    }

    /// O fechamento pelo dono vai ao diário com o cliente e a origem.
    #[test]
    fn the_owner_close_names_the_client_and_the_origin() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "curl/8.10".parse().unwrap());
        let line = super::close_line("s1", &headers, "127.0.0.1:5000".parse().unwrap());
        assert_eq!((line["evento"].as_str(), line["sessao"].as_str(), line["codigo"].as_str()), (Some("sessao.fechamento_pedido"), Some("s1"), Some("session_close_requested_by_owner")));
        assert_eq!((line["cliente"].as_str(), line["origem"].as_str()), (Some("curl/8.10"), Some("127.0.0.1")));
    }
}

/// O callback retém apenas o AppState fraco: fechar o servidor não deixa um ciclo de proprietários.
pub fn other_cleanup(state:std::sync::Weak<AppState>)->crate::runtime::other_terminal::OtherAfterClosed {
    Arc::new(move|closed|{let state=state.clone();Box::pin(async move {
        let Some(st)=state.upgrade()else{return Ok(vec![]);};let Some(runtime)=st.state.runtime.get().cloned()else{return Ok(vec![json!({"code":"other_cleanup_unavailable"})]);};
        let life=closed.life;let mut warnings=Vec::new();let name=life.name().to_owned();
        if !runtime.other_cleanup_allowed(&life).await? {return Ok(vec![json!({"code":"other_cleanup_replaced"})]);}
        if let Some(groups)=st.groups.clone(){warnings.extend(leave_warnings(&st,&groups,&name).await);}
        if runtime.other_cleanup_allowed(&life).await? && let Some(share)=st.share.clone() {
            match share.store.revoke_session(&name,None) {Ok(0)=>{},Ok(_)=>crate::share::owner_routes::sync_tunnel(&share).await,Err(_)=>warnings.push(json!({"code":"other_share_cleanup_failed"}))}
        }
        if runtime.other_cleanup_allowed(&life).await? {if crate::loops::store(&st).forget(&name).is_err(){warnings.push(json!({"code":"other_loop_cleanup_failed"}));}}
        if runtime.other_cleanup_allowed(&life).await? {st.list.forget(&name);st.list.invalidate();}
        if runtime.other_cleanup_allowed(&life).await? {
            if crate::loops::store(&st).clear_chain(&name).is_err(){warnings.push(json!({"code":"other_chain_cleanup_failed"}));}
            if closed.reason==crate::runtime::other_terminal::OtherEnd::Requested {crate::session_open::restore::forget(&life.dirs().home,&name);}
        }
        let guard_life=life.clone();let weak=Arc::downgrade(&runtime);
        warnings.extend(crate::shortcuts::close_for_session(&st,&name,move||{let weak=weak.clone();let life=guard_life.clone();Box::pin(async move{match weak.upgrade(){Some(runtime)=>runtime.other_cleanup_allowed(&life).await.unwrap_or(false),None=>false}})}).await);
        if runtime.other_cleanup_allowed(&life).await? && let Some(path)=life.transcript().map(std::path::Path::to_path_buf) {
            let config=life.dirs().claude.clone();let closed_name=name.clone();let result=tokio::task::spawn_blocking(move||record_other_closed_name(&config,&path,&closed_name)).await;
            if !matches!(result,Ok(Ok(()))){warnings.push(json!({"code":"other_closed_name_failed"}));}
        }
        Ok(warnings)
    })})
}
fn record_other_closed_name(config:&std::path::Path,path:&std::path::Path,name:&str)->std::io::Result<()> {
    static SERIAL:std::sync::Mutex<()>=std::sync::Mutex::new(());let _guard=SERIAL.lock().unwrap_or_else(|error|error.into_inner());let target=config.join("closed-sessions.json");
    let mut data:serde_json::Map<String,Value>=match crate::runtime::queue::read_replaced(&target){Ok(bytes)=>serde_json::from_slice(&bytes).map_err(|error|std::io::Error::new(std::io::ErrorKind::InvalidData,error))?,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Default::default(),Err(error)=>return Err(error)};
    let real=std::fs::canonicalize(path).unwrap_or_else(|_|path.into());let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();data.insert(real.to_string_lossy().into_owned(),json!({"name":name,"closed_at":now}));
    if data.len()>500 {let mut oldest=data.iter().map(|(path,value)|(path.clone(),value["closed_at"].as_f64().unwrap_or(0.0))).collect::<Vec<_>>();oldest.sort_by(|a,b|a.1.total_cmp(&b.1));for(key,_)in oldest.into_iter().take(data.len()-500){data.remove(&key);}}
    std::fs::create_dir_all(config)?;crate::runtime::queue::atomic_write(&target,&serde_json::to_vec(&data)?)
}
