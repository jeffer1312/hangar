//! Fechamento de sessão (`DELETE /api/sessions/{name}`) e a limpeza depois dele.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::body::Body;
use serde_json::{Value, json};

use crate::routes::AppState;

/// `DELETE /api/sessions/{name}` do dono e do convidado que alcança a sessão (`caller.can`). Toda sessão
/// encerra aqui: Pi/omp/Kimi pelo ator; Codex e Claude abertos no Rust pela vida guardada; o resto (sem
/// terminal parado, agente morto, pane de outro programa, a que já acabou) pelo nome. Depois, o `then`, o
/// nome fechado, a saída do grupo e os convites. Nada vai ao Python; falha responde com o código.
pub async fn delete(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let Some(name) = super::session_name(req.uri().path()) else { return crate::routes::not_found(req.headers()) };
    let (_, caller) = match crate::routes::who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let owner = match &caller {
        Some(who) if who.can(&name, req.method()) => matches!(who, crate::share::caller::Caller::Owner),
        // Convidado fora desta sessão (par de leitura): 403, sem contar falha de senha.
        Some(_) => return crate::routes::refuse_without_python(&st, peer, req, caller.as_ref()).await,
        // Na 8766 o token do dono não decide: sem convidado, 404, como o `pass` responde lá.
        None if crate::routes::on_guest_port(&req) => return crate::routes::not_found(req.headers()),
        // Sem identidade: o `require_auth` do Python, aqui; aceito só sem senha configurada.
        None => match crate::routes::owner_gate(&st, peer, &req).await {
            Ok(_) => true,
            Err(mut response) => { crate::routes::cors(req.headers(), response.headers_mut()); return response; }
        },
    };
    if let Some(response) = super::orchestration_guard(&st, &name, req.headers()).await { return response; }
    let Some(runtime) = st.state.runtime.get().cloned() else {
        let mut response = super::detail(StatusCode::SERVICE_UNAVAILABLE, "runtime_watcher", "o servidor ainda não abriu as sessões", json!({}));
        crate::routes::cors(req.headers(), response.headers_mut());
        return response;
    };
    // A chave antes do kill (depois dele o pane não a diz mais); os arquivos do cofre saem só no sucesso.
    let vault_files = crate::vault::files::before_close(&st, &name).await;
    if let Some(closed) = runtime.close_other(&name).await {
        match closed {
            Ok(body) => {
                if let Some(files) = vault_files { files.done(); }
                closed_line(&st, &name, owner, req.headers(), peer);
                let mut response = super::json_response(StatusCode::OK, body);
                crate::routes::cors(req.headers(), response.headers_mut());
                return response;
            },
            // A sessão já acabou: fecha pelo nome, como qualquer outra que acabou (D17).
            Err(error) if error.code == "other_terminal_dead" => {},
            Err(error) => return close_failed(&st, &name, &error, req.headers()),
        }
    }
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
    // Ainda com o nome reservado: atalho de uma sessão nova com o mesmo nome não pode ser alcançado.
    let mut cleanup = if closed.is_ok() { close_shortcuts(&st, &name).await } else { Vec::new() };
    drop(closing);
    let closed = match closed {
        Ok(closed) => closed,
        Err(error) => return close_failed(&st, &name, &error, req.headers()),
    };
    // Fechada de propósito não volta no próximo reboot, de qualquer agente; recusada, segue voltando.
    if let Some(dirs) = st.list.env().dirs.as_ref() { crate::session_open::restore::forget(&dirs.home, &name); }
    closed_line(&st, &name, owner, req.headers(), peer);
    if let Some(files) = vault_files { files.done(); }
    // O `then` da vida que acabou não dispara na próxima sessão com o nome (o Claude e o Codex abertos
    // aqui não passam pela limpeza do nome).
    if let Err(error) = crate::loops::store(&st).clear_chain(&name) {
        tracing::error!(session = %name, code = "close_chain_failed", kind = ?error.kind(), "vínculo then da sessão encerrada não saiu");
        cleanup.push(cleanup_failed(&st, &name, "close_chain_failed", "o vínculo then da sessão encerrada não saiu"));
    }
    record_closed_name(&st, &name, closed["transcript"].as_str()).await;
    let leave = match st.groups.clone() { Some(groups) => leave_warnings(&st, &groups, &name, "session_closed").await, None => Vec::new() };
    cleanup.extend(after_close(&st, &name).await);
    let mut response = super::json_response(StatusCode::OK, json!({"ok": true, "warning": null}));
    crate::routes::cors(req.headers(), response.headers_mut());
    with_warnings(&name, response, leave, cleanup).await
}

/// Limpeza que não terminou depois de a sessão fechar: vai ao diário e volta no `warning`, com o código.
fn cleanup_failed(st: &AppState, name: &str, code: &str, reason: &'static str) -> Value {
    st.diag.report("rust.close_failed", name, code, reason);
    json!({"code": "erro_limpeza_fechamento_falhou", "params": {"codigo": code},
        "msg": format!("a sessão fechou, mas a limpeza não terminou ({code})")})
}

/// O nome que a conversa tinha, para o Arquivo e o "reabre a X" (o `closed_names.record` do Python, na
/// mesma pasta que o Arquivo lê). Falhar não desfaz o fechamento: só vai ao log e ao diário.
async fn record_closed_name(st: &AppState, name: &str, transcript: Option<&str>) {
    let Some(path) = transcript.filter(|path| !path.is_empty()).map(std::path::PathBuf::from) else {
        tracing::info!(session = %name, "nome fechado não guardado: sem conversa conhecida");
        return;
    };
    let (config, owned) = (st.accounts.env.claude_base.clone(), name.to_owned());
    if !matches!(tokio::task::spawn_blocking(move || record_other_closed_name(&config, &path, &owned)).await, Ok(Ok(()))) {
        tracing::warn!(session = %name, code = "close_name_record_failed", "nome fechado não guardado");
        st.diag.report("rust.close_failed", name, "close_name_record_failed", "o nome da sessão fechada não foi guardado");
    }
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

/// Sai do grupo; os avisos que não saíram e a falha da própria saída viram `{sessao, erro}`.
async fn leave_warnings(st: &AppState, groups: &crate::groups::service::GroupService, name: &str, why: &'static str) -> Vec<Value> {
    match crate::groups::exit::leave_and_notify(st, groups, name, why).await {
        Ok((_, warnings)) => warnings,
        Err(error) => {
            tracing::warn!(session = %name, error = %error, "saída do grupo falhou ao encerrar a sessão");
            let code = crate::groups::bridge::group_failed(error).code;
            vec![json!({"sessao": name, "erro": {"code": "erro_grupo_indisponivel", "params": {"detalhe": code},
                "msg": "os grupos estão indisponíveis agora"}})]
        }
    }
}

/// Os avisos de saída e os da limpeza (convite, loop, `then`, atalhos) no `warning` da resposta do
/// fechamento, na ordem do Python; corpo que não é objeto JSON vira `{"ok": true}`.
async fn with_warnings(name: &str, response: Response, mut errs: Vec<Value>, mut cleanup: Vec<Value>) -> Response {
    if errs.is_empty() && cleanup.is_empty() { return response; }
    let (mut parts, body) = response.into_parts();
    let read = match axum::body::to_bytes(body, 1 << 20).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    let mut value = match read {
        Ok(value) if value.is_object() => value,
        other => {
            let cause = other.err().unwrap_or_else(|| "não é objeto".to_owned());
            tracing::warn!(session = %name, avisos = errs.len(), %cause, "corpo do fechamento ilegível; a resposta leva só os avisos de saída");
            json!({"ok": true})
        }
    };
    let current = value.get("warning").filter(|w| !w.is_null()).cloned();
    value["warning"] = if errs.is_empty() && current.is_none() && cleanup.len() == 1 {
        cleanup.remove(0)
    } else {
        errs.extend(cleanup.into_iter().map(|warning| json!({"sessao": name, "erro": warning})));
        with_leave_warning(current, name, errs)
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

/// O fechamento que deu certo vai ao diário com o cliente, a origem e quem pediu (dono ou convidado),
/// no destino que o servidor escolheu para o diário.
fn closed_line(st: &AppState, name: &str, owner: bool, headers: &axum::http::HeaderMap, peer: SocketAddr) {
    st.diag.record_line(close_line_by(name, owner, headers, peer));
}

fn close_line_by(name: &str, owner: bool, headers: &axum::http::HeaderMap, peer: SocketAddr) -> Value {
    match owner {
        true => close_line(name, headers, peer),
        false => asked_line("sessao.fechamento_pedido", "session_close_requested_by_guest", name, headers, peer),
    }
}

/// Depois de o Rust encerrar a sessão: revoga os convites e some da lista. A saída do grupo é de quem chamou.
/// Devolve os avisos do que falhou: o fechamento segue, e a pessoa fica sabendo.
async fn after_close(st: &Arc<AppState>, name: &str) -> Vec<Value> {
    let mut warnings = Vec::new();
    if let Some(share) = st.share.clone() {
        match share.store.revoke_session(name, None) {
            Ok(0) => {}
            Ok(_) => crate::share::owner_routes::sync_tunnel(&share).await,
            Err(error) => {
                tracing::error!(session = %name, code = "share_store_failed", "convites da sessão encerrada não revogados: {error}");
                st.diag.report("rust.close_failed", name, "share_store_failed", "os convites da sessão encerrada não foram revogados");
                warnings.push(json!({"code": "erro_convite_nao_acompanhou", "params": {"codigo": "share_store_failed"},
                    "msg": "o convite desta sessão não acompanhou a mudança (share_store_failed)"}));
            }
        }
    }
    // O loop é da vida que acabou: nada de acordar na próxima sessão com o nome.
    if let Err(error) = crate::loops::store(st).forget(name) {
        tracing::error!(session = %name, code = "loop_forget_failed", kind = ?error.kind(), "loop da sessão encerrada não saiu");
        warnings.push(cleanup_failed(st, name, "loop_forget_failed", "o loop da sessão encerrada não saiu"));
    }
    st.list.forget(name);
    st.list.invalidate();
    warnings
}

/// Os terminais de atalho da sessão encerrada; cada um que não fechou volta como aviso.
async fn close_shortcuts(st: &AppState, name: &str) -> Vec<Value> {
    crate::shortcuts::close_for_session(st, name, || Box::pin(async { true })).await.into_iter().map(|warning| {
        let code = warning["code"].as_str().unwrap_or("shortcut_close_failed").to_owned();
        tracing::warn!(session = %name, code = %code, "terminal de atalho da sessão encerrada não fechou");
        cleanup_failed(st, name, &code, "terminal de atalho da sessão encerrada não fechou")
    }).collect()
}

/// Quem pediu o fechamento: o cliente e a origem que a requisição traz (o dono não manda identidade própria).
/// No `DELETE` do dono a linha sai depois do encerramento; a falha vai como `rust.close_failed`, com o código.
fn close_line(name: &str, headers: &axum::http::HeaderMap, peer: SocketAddr) -> serde_json::Value {
    asked_line("sessao.fechamento_pedido", "session_close_requested_by_owner", name, headers, peer)
}

pub(crate) fn asked_line(evento: &str, codigo: &str, name: &str, headers: &axum::http::HeaderMap, peer: SocketAddr) -> serde_json::Value {
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

    /// A limpeza que falhou depois do fechamento volta no `warning`: sozinha, ela mesma; com outra, no
    /// aviso composto, na ordem em que aconteceu.
    #[tokio::test]
    async fn cleanup_failures_reach_the_close_warning() {
        use serde_json::{Value, json};
        let ok = || super::super::json_response(axum::http::StatusCode::OK, json!({"ok": true, "warning": null}));
        let body = |response: axum::response::Response| async move {
            serde_json::from_slice::<Value>(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
        };
        let failed = |code: &str| json!({"code": "erro_limpeza_fechamento_falhou", "params": {"codigo": code},
            "msg": format!("a sessão fechou, mas a limpeza não terminou ({code})")});
        let (looped, chained) = (failed("loop_forget_failed"), failed("close_chain_failed"));
        let one = body(super::with_warnings("s0", ok(), vec![], vec![looped.clone()]).await).await;
        assert_eq!(one, json!({"ok": true, "warning": looped}));
        let two = body(super::with_warnings("s0", ok(), vec![], vec![chained.clone(), looped.clone()]).await).await;
        assert_eq!(two["warning"]["code"], "erro_pareamento_saida_falhou");
        assert_eq!(two["warning"]["params"]["avisos"], json!([{"sessao": "s0", "erro": chained}, {"sessao": "s0", "erro": looped}]));
        let none = body(super::with_warnings("s0", ok(), vec![], vec![]).await).await;
        assert_eq!(none["warning"], Value::Null, "nada falhou: sem aviso");
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

    /// R1-20, decisão 3: o fechamento pedido pelo convidado vai ao diário com o código próprio.
    #[test]
    fn the_guest_close_has_its_own_code() {
        let (headers, peer) = (axum::http::HeaderMap::new(), "127.0.0.1:5000".parse().unwrap());
        let line = super::close_line_by("s1", false, &headers, peer);
        assert_eq!((line["evento"].as_str(), line["codigo"].as_str()), (Some("sessao.fechamento_pedido"), Some("session_close_requested_by_guest")));
        assert_eq!(super::close_line_by("s1", true, &headers, peer)["codigo"], "session_close_requested_by_owner");
    }
}

/// O callback retém apenas o AppState fraco: fechar o servidor não deixa um ciclo de proprietários.
pub fn other_cleanup(state:std::sync::Weak<AppState>)->crate::runtime::other_terminal::OtherAfterClosed {
    Arc::new(move|closed|{let state=state.clone();Box::pin(async move {
        let Some(st)=state.upgrade()else{return Ok(vec![]);};let Some(runtime)=st.state.runtime.get().cloned()else{return Ok(vec![json!({"code":"other_cleanup_unavailable"})]);};
        let life=closed.life;let mut warnings=Vec::new();let name=life.name().to_owned();
        if !runtime.other_cleanup_allowed(&life).await? {return Ok(vec![json!({"code":"other_cleanup_replaced"})]);}
        if let Some(groups)=st.groups.clone(){warnings.extend(leave_warnings(&st,&groups,&name,"other_session_closed").await);}
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
            // A pasta que o Arquivo lê, a mesma do fechamento do dono.
            let config=st.accounts.env.claude_base.clone();let closed_name=name.clone();let result=tokio::task::spawn_blocking(move||record_other_closed_name(&config,&path,&closed_name)).await;
            if !matches!(result,Ok(Ok(()))){warnings.push(json!({"code":"other_closed_name_failed"}));}
        }
        Ok(warnings)
    })})
}
fn record_other_closed_name(config:&std::path::Path,path:&std::path::Path,name:&str)->std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    // Pasta real: a trava é o mesmo arquivo antes e depois de o `closed-sessions.json` existir.
    let target=std::fs::canonicalize(config)?.join("closed-sessions.json");
    // Ler, separar o ilegível e gravar sob uma trava só: dois fechamentos juntos não perdem nome.
    let _lock=hangar_core::config_lock::lock_owned(&target).map_err(std::io::Error::other)?;
    let mut data:serde_json::Map<String,Value>=match crate::runtime::queue::read_replaced(&target){
        Ok(bytes)=>match serde_json::from_slice::<Value>(&bytes){Ok(Value::Object(map))=>map,_=>{set_aside(&target)?;Default::default()}},
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Default::default(),Err(error)=>return Err(error)};
    let real=std::fs::canonicalize(path).unwrap_or_else(|_|path.into());let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();data.insert(real.to_string_lossy().into_owned(),json!({"name":name,"closed_at":now}));
    if data.len()>500 {let mut oldest=data.iter().map(|(path,value)|(path.clone(),value["closed_at"].as_f64().unwrap_or(0.0))).collect::<Vec<_>>();oldest.sort_by(|a,b|a.1.total_cmp(&b.1));for(key,_)in oldest.into_iter().take(data.len()-500){data.remove(&key);}}
    crate::runtime::queue::atomic_write(&target,&serde_json::to_vec(&data)?)
}
/// O arquivo ilegível vai para o lado (`.bad`, `.bad.2`…) antes da gravação nova apagar o que ele tinha.
fn set_aside(path:&std::path::Path)->std::io::Result<()> {
    let name=path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut dest=path.with_file_name(format!("{name}.bad"));
    for n in 2.. { if !dest.exists() { break; } dest=path.with_file_name(format!("{name}.bad.{n}")); }
    tracing::warn!(file=%dest.display(),"nomes de sessão fechada ilegíveis; guardados ao lado");
    std::fs::rename(path,dest)
}
