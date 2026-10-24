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

/// Marca do pedido repassado pela ponte: o `gate` o trata como do dono e o `pass` nunca o devolve ao Python.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BridgedOwner;

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
            Some(Err(error)) => return super::detail(StatusCode::SERVICE_UNAVAILABLE, &error.code, &error.message, json!({})),
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
        "input" => super::input::input(s, c, req).await,
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
            if !runtime.owns_codex_terminal(&name) && !runtime.owns_codex_headless(&name).await {
                match runtime.adopt_now(&name).await {
                    None => return not_ours(),
                    Some(Err(error)) => return super::detail(StatusCode::SERVICE_UNAVAILABLE, &error.code, &error.message, json!({})),
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
/// Marca do `DELETE` que o Rust só repassa e cuja saída do grupo ele faz depois; o valor é o segredo interno.
pub const RUST_LEAVES: &str = "x-hangar-rust-leaves";
/// Transcript (percent-encoded) da sessão que o Rust encerrou, junto da marca.
pub const CLOSED_TRANSCRIPT: &str = "x-hangar-closed-transcript";

/// `DELETE /api/sessions/{name}` do dono. Codex com terminal e Claude abertos no Rust: encerra aqui (pane ou
/// processo, ator, sidecar, fila e saída do grupo) e só então repassa o pedido marcado ao Python, para convites, "then",
/// terminais de atalho e caches dele. Falha no encerramento responde com o código e não repassa. Outra
/// sessão segue ao Python como chegou.
pub async fn delete(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    // Só o próprio Rust põe a marca: a que vier de fora sai antes de qualquer decisão.
    req.headers_mut().remove(RUST_CLOSED);
    req.headers_mut().remove(CLOSED_TRANSCRIPT);
    req.headers_mut().remove(RUST_LEAVES);
    let name = super::session_name(req.uri().path());
    // Sem vigia (sem as pastas da lista), `kill_codex_terminal` é `None`.
    // Fechada de propósito não volta no próximo reboot, de qualquer agente.
    if let (true, Some(name), Some(dirs)) = (owner, name.as_deref(), st.list.env().dirs.as_ref()) {
        crate::session_open::restore::forget(&dirs.home, name);
    }
    if let (true, Some(name)) = (owner, name.as_deref()) {
        crate::diag::record(crate::diag::diary_dir(), vec![close_line(name, req.headers(), peer)], "sessão fechada");
    }
    let (Some(runtime), true, Some(name)) = (st.state.runtime.get().cloned(), owner, name)
        else { return crate::routes::pass(&st, req, &fwd).await };
    // Claude aberto aqui (com ou sem terminal) também fecha aqui: só matar pelo nome no Python deixava a
    // entrada da vida morta presa ao nome, e a próxima sessão com ele herdava a conversa dela.
    let closed = match runtime.kill_codex_terminal(&name).await {
        None => runtime.kill_claude(&name).await,
        codex => codex,
    };
    match closed {
        // Quem fecha é o Python; a saída do grupo é daqui, depois do fechamento que deu certo (BRG-25).
        None => {
            // Marca com o segredo: o Python não pede a saída pela ponte, que seria ida e volta.
            if let Ok(mark) = axum::http::HeaderValue::from_str(&st.cfg.internal_secret) { req.headers_mut().insert(RUST_LEAVES, mark); }
            let response = crate::routes::pass(&st, req, &fwd).await;
            return leave_after_forward(&st, &name, response).await;
        }
        Some(Err(error)) => {
            let mut response = close_refusal(&error);
            crate::routes::cors(req.headers(), response.headers_mut());
            return response;
        }
        Some(Ok(closed)) => {
            // Só o caminho da conversa, para o Python guardar o nome fechado; sem ele, o Python registra a falta.
            if let Some(path) = closed["transcript"].as_str() {
                let encoded = percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string();
                if let Ok(value) = axum::http::HeaderValue::from_str(&encoded) { req.headers_mut().insert(CLOSED_TRANSCRIPT, value); }
            }
        }
    }
    let leave = match st.groups.clone() { Some(groups) => leave_warnings(&st, &groups, &name).await, None => Vec::new() };
    after_close(&st, &name).await;
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
            return with_warnings(&name, response, leave).await;
        }
    }
    let response = crate::routes::pass(&st, req, &fwd).await;
    with_warnings(&name, response, leave).await
}

/// Fechamento feito pelo Python (sessão sem entrada aqui: Pi, omp, Kimi): sai do grupo depois do
/// sucesso, e a falha do aviso vai no `warning` da resposta: a sessão some da lista e ninguém mais a veria.
async fn leave_after_forward(st: &AppState, name: &str, response: Response) -> Response {
    let Some(groups) = st.groups.clone() else { return response };
    if !response.status().is_success() { return response; }
    let errs = leave_warnings(st, &groups, name).await;
    with_warnings(name, response, errs).await
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

/// Os avisos de saída no `warning` da resposta do fechamento; corpo que não é objeto JSON vira `{"ok": true}`.
async fn with_warnings(name: &str, response: Response, errs: Vec<Value>) -> Response {
    if errs.is_empty() { return response; }
    let (mut parts, body) = response.into_parts();
    let read = axum::body::to_bytes(body, 1 << 20).await.ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let mut value = read.filter(Value::is_object).unwrap_or_else(|| {
        tracing::warn!(session = %name, avisos = errs.len(), "corpo do fechamento ilegível; a resposta leva só os avisos de saída");
        json!({"ok": true})
    });
    let current = value.get("warning").filter(|w| !w.is_null()).cloned();
    value["warning"] = with_leave_warning(current, name, errs);
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
    match super::busy_refusal(error) {
        Some((status, body)) => super::json_response(status, body),
        None => super::detail(StatusCode::INTERNAL_SERVER_ERROR, &error.code, &error.message, json!({})),
    }
}

/// Depois de o Rust encerrar a sessão: revoga os convites e some da lista. A saída do grupo é de quem chamou.
async fn after_close(st: &Arc<AppState>, name: &str) {
    // Convites e par da sessão são do Rust: o Python marcado não volta a pedir a revogação.
    if let Some(share) = st.share.clone() {
        match share.store.revoke_session(name, None) {
            Ok(0) => {}
            Ok(_) => crate::share::owner_routes::sync_tunnel(&share).await,
            Err(error) => tracing::error!(session = %name, code = "share_store_failed", "convites da sessão encerrada não revogados: {error}"),
        }
    }
    // O loop é da vida que acabou: nada de acordar na próxima sessão com o nome.
    if let Err(error) = crate::loops::store(st).forget(name) {
        tracing::error!(session = %name, code = "loop_forget_failed", kind = ?error.kind(), "loop da sessão encerrada não saiu");
    }
    st.list.forget(name);
    st.list.invalidate();
}

/// O `DELETE` do Claude que entrou pelo Connect (8768): o encerramento da 8765, sem repasse e sem a saída
/// do grupo, que o Python faz depois com a fase do `orq` já lida (o Rust não pergunta de volta numa chamada
/// dele). 421 = não é Claude aberto aqui, e o Python segue.
async fn close_bridged(st: Arc<AppState>, peer: SocketAddr, name: &str, headers: &axum::http::HeaderMap) -> Response {
    let Some(runtime) = st.state.runtime.get().cloned() else { return not_ours() };
    crate::diag::record(crate::diag::diary_dir(), vec![close_line(name, headers, peer)], "sessão fechada");
    let closed = runtime.kill_claude(name).await;
    // Recusado, a sessão segue viva e volta no reboot; no 421 o Python a fecha pelo nome, então ela sai.
    if !matches!(closed, Some(Err(_))) && let Some(dirs) = st.list.env().dirs.as_ref() {
        crate::session_open::restore::forget(&dirs.home, name);
    }
    match closed {
        None => not_ours(),
        Some(Err(error)) => close_refusal(&error),
        Some(Ok(closed)) => {
            after_close(&st, name).await;
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
        ("GET", "commands") => codex::commands(s, c, req).await,
        ("GET", "codex-permissions") => codex::permissions(s, c, req).await,
        ("POST", "codex-permissions") => codex::set_permission(s, c, req).await,
        ("POST", "modo-execucao") => relaunch::mode(s, c, req).await,
        ("POST", "recarregar") => relaunch::reload(s, c, req).await,
        _ => not_ours(),
    }
}

/// Quem pediu o fechamento: o cliente e a origem que a requisição traz (o dono não manda identidade própria). O
/// pedido, não o resultado: o encerramento pode falhar depois, e a sessão que some sem ele vai como `sessao.sumiu`.
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
