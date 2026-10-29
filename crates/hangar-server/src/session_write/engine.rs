//! O Claude num motor do CLIProxyAPI local com conta ChatGPT fixa: trocar a conta (`POST /conta` com
//! `engine_account`), o modelo (`/engine/model`), o esforço (`/model-effort`) e o Fast (`/service-tier`)
//! reabre a mesma conversa pelo spec com a mudança, como o `_trocar_conta` do Python. Sem conta fixa, o
//! `/model-effort` é do `model_effort`; o `/engine/model` usa o ator da sessão para preservar
//! a conversa e o modelo padrão global.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo,Request,State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value,json};

use super::{BODY_LIMIT,detail_body,session_name,too_large};
fn reply(headers:Option<&axum::http::HeaderMap>,status:StatusCode,body:Value)->Response {
    use crate::orchestration::codec;
    if codec::contains_surrogate(&body) || (!status.is_success() && codec::contains_nonfinite(&body)) {
        let mut response=crate::engines::body::internal_error();
        if let Some(headers)=headers {crate::http_cors::apply(headers,&mut response);}
        response
    } else {crate::mods::http::reply(headers,status,codec::normalize_response(body))}
}
use crate::routes::{AppState,gate};
use crate::runtime::gateway::RuntimeRegistry;
use crate::session_open::{Account,ChangeReason,OpenFacts,ServiceTier,SessionChange,SessionSpec};

pub(crate) fn local_cliproxy(env:&crate::accounts::environment::AccountEnvironment,engine:&str) -> bool {
    crate::session_open::facts::local_engine(env,engine)
}


/// O que o pedido muda na sessão de motor.
pub(crate) struct EngineChange { pub account:Option<String>, pub model:Option<String>, pub effort:Option<String>, pub tier:Option<ServiceTier> }

/// Os fatos do motor (catálogo da conta, Fast) relidos agora; bloqueante.
async fn facts<T:Send+'static>(env:crate::accounts::environment::AccountEnvironment,
    work:impl FnOnce(&crate::session_open::facts::LocalFacts)->T+Send+'static) -> Option<T> {
    let handle=tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move||work(&crate::session_open::facts::LocalFacts::new(env,handle))).await.ok()
}

/// A troca na sessão de motor `name` cujo spec é `spec`. `None` = não é sessão de motor com conta fixa
/// (quem chama segue o caminho de sempre).
pub(crate) async fn switch(st:&Arc<AppState>,runtime:&Arc<RuntimeRegistry>,name:&str,spec:SessionSpec,wanted:EngineChange) -> (StatusCode,Value) {
    let env=runtime.codex_environment();
    let Account::Engine { name:engine,account:current,config_dir,context_window,.. }=spec.account.clone() else {
        return (StatusCode::BAD_REQUEST,detail_body("erro_cliproxy_conta","esta sessão não usa o CLIProxyAPI local",json!({})));
    };
    let account=wanted.account.clone().or(current.clone());
    let Some(account)=account else {
        return (StatusCode::BAD_REQUEST,detail_body("erro_cliproxy_conta","esta sessão não usa uma conta ChatGPT fixa",json!({})));
    };
    if (wanted.model.is_some() || wanted.effort.is_some()) && current.as_deref()!=Some(account.as_str()) {
        return (StatusCode::CONFLICT,detail_body("erro_cliproxy_conta","a conta da sessão mudou; atualize a lista de modelos",json!({})));
    }
    let parked=spec.parked();
    let same=current.as_deref()==Some(account.as_str()) && wanted.model.is_none() && wanted.effort.is_none() && wanted.tier.is_none();
    if same && !parked {return (StatusCode::OK,json!({"ok":true,"engine_account":account}));}
    // O modelo com o prefixo da conta, conferido no catálogo de agora: o cache não sustenta a recusa aqui.
    let model=wanted.model.clone().or(spec.model.clone());
    let checked={let (engine,account,model)=(engine.clone(),account.clone(),model.clone());
        facts(env.clone(),move|facts|{
            let base=facts.fixed_engine_model(&engine,&account,model.as_deref())?;
            let window=match &model {Some(model)=>facts.engine_window(&engine,model,Some(&account))?,None=>None};
            Ok::<_,crate::session_open::OpenError>((base,window))
        }).await};
    let (base,window)=match checked {
        Some(Ok(found))=>found,
        Some(Err(error))=>return (StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_REQUEST),error.body()),
        None=>return (StatusCode::SERVICE_UNAVAILABLE,detail_body("erro_cliproxy_conta","o catálogo do motor não respondeu",json!({}))),
    };
    let window=if wanted.model.is_some() {window} else {context_window};
    let change=SessionChange { reason:ChangeReason::AccountSwitch,surface:parked.then_some(crate::session_open::Surface::Terminal),
        account:Some(Account::Engine { name:engine,account:Some(account.clone()),config_dir,context_window:window,context_window_unknown:false }),
        permission_mode:None,model:Some(base),effort:wanted.effort.clone(),service_tier:wanted.tier };
    let (runtime,owned)=(runtime.clone(),name.to_owned());
    match super::relaunch::reopen_detached(st,name,async move {runtime.reopen_claude_session(&owned,change,None).await}).await {
        Some(Ok(reopened))=>(StatusCode::OK,reopened.answer(match wanted.tier {
            Some(tier)=>json!({"ok":true,"service_tier":tier.name()}),
            None if wanted.model.is_some()=>json!({"ok":true,"model":wanted.model}),
            None=>json!({"ok":true,"engine_account":account}),
        })),
        Some(Err(failure))=>super::account::account_refusal(&failure,"não troquei"),
        None=>(StatusCode::CONFLICT,detail_body("erro_troca_conta","a sessão não está aberta aqui",json!({}))),
    }
}

/// Lê o corpo e acha o spec da sessão Claude; `Err` = a resposta (repasse incluído).
async fn claude_engine(st:&Arc<AppState>,peer:SocketAddr,req:Request) -> Result<(Arc<RuntimeRegistry>,String,SessionSpec,Value,axum::http::HeaderMap),Response> {
    let name=session_name(req.uri().path()).ok_or_else(||super::invalid_body(req.headers()))?;
    if crate::routes::on_guest_port(&req) {
        if !crate::share::caller::caller(&req).is_some_and(|caller|caller.can(&name,req.method())) {
            return Err(reply(Some(req.headers()),StatusCode::FORBIDDEN,json!({"detail":"forbidden"})));
        }
    } else { crate::routes::owner_gate(st,peer,&req).await?; }
    let runtime=st.state.runtime.get().cloned().ok_or_else(||reply(Some(req.headers()),StatusCode::SERVICE_UNAVAILABLE,detail_body("runtime_unavailable","o runtime da sessão está indisponível",json!({}))))?;
    let (parts,body)=req.into_parts();
    let bytes=match crate::http_body_limit::read(body).await {
        Ok(bytes)=>bytes,
        Err(error) if crate::http_body_limit::is_limit(&error)=>return Err(crate::http_body_limit::rejected()),
        Err(_)=>return Err(crate::engines::body::internal_error()),
    };
    let body=parse_model_body(&bytes,&parts.headers)?;
    if st.term.is_active(&name) {
        return Err(reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_terminal_aberto","Terminal aberto nesta sessao. Feche o painel pra responder por aqui.",json!({}))));
    }
    let spec=match runtime.claude_spec(&name).await {
        Some(Ok(spec)) if matches!(&spec.account,Account::Engine {..})=>spec,
        Some(Ok(_))=>return Err(reply(Some(&parts.headers),StatusCode::BAD_REQUEST,detail_body("erro_rota_so_motor","esta rota so existe pra sessoes que rodam num motor",json!({})))),
        Some(Err(error))=>return Err(reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({})))),
        None=>{
            let (status,code,message)=match st.list.discovered_session(&name).await {
                Ok(Some(_))=>(StatusCode::BAD_REQUEST,"erro_rota_so_motor","esta rota so existe pra sessoes que rodam num motor"),
                Ok(None)=>(StatusCode::NOT_FOUND,"erro_sessao_inexistente","sessao nao encontrada"),
                Err(_)=>(StatusCode::SERVICE_UNAVAILABLE,"runtime_unavailable","não consegui localizar a sessão"),
            };
            return Err(reply(Some(&parts.headers),status,detail_body(code,message,json!({}))));
        },
    };
    Ok((runtime,name,spec,body,parts.headers))
}

fn parse_model_body(bytes:&[u8],headers:&axum::http::HeaderMap)->Result<Value,Response> {
    crate::engines::body::typed(crate::engines::body::Schema::Model,bytes,headers)
}

fn text(body:&Value,field:&str) -> Result<Option<String>,()> {
    match &body[field] {Value::Null=>Ok(None),Value::String(s) if !s.trim().is_empty()=>Ok(Some(s.trim().to_owned())),Value::String(_)=>Ok(None),_=>Err(())}
}

/// `POST /engine/model` da conta fixa: o modelo do catálogo da conta, com a janela dele.
pub async fn model(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request) -> Response {
    let (runtime,name,spec,body,headers)=match claude_engine(&st,peer,req).await {Ok(found)=>found,Err(response)=>return response};
    if matches!(&spec.account,Account::Engine {account:None,..}) {
        return unfixed_model(&st,runtime,&name,spec,body,&headers).await;
    }
    let (Ok(Some(model)),Ok(effort))=(text(&body,"model"),text(&body,"effort")) else {return super::invalid_body(&headers)};
    // Fast ligado não vai para um modelo que não o aceita.
    if spec.service_tier==Some(ServiceTier::Priority) {
        let engine=match &spec.account {Account::Engine { name,.. }=>name.clone(),_=>String::new()};
        let fast={let model=model.clone();facts(runtime.codex_environment(),move|facts|facts.supports_fast(Some(&engine),Some(&model))).await};
        if fast!=Some(true) {
            return reply(Some(&headers),StatusCode::CONFLICT,detail_body("erro_fast_indisponivel","Desligue Fast antes de escolher um modelo que não o suporta",json!({})));
        }
    }
    let (status,body)=switch(&st,&runtime,&name,spec,EngineChange { account:None,model:Some(model),effort,tier:None }).await;
    st.list.invalidate();
    reply(Some(&headers),status,body)
}

/// `POST /model-effort`. Conta fixa: só a sessão (`scope: session`), reabrindo; o modelo vai pelo
/// `/engine/model`. As outras sessões Claude, pelo seletor do `/model` ou pelo ator sem terminal
/// (`model_effort`).
pub async fn effort(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request) -> Response {
    let (fwd,owner)=match gate(&st,peer,&req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let runtime=st.state.runtime.get().cloned();
    let (Some(runtime),true,Some(name))=(runtime,owner,session_name(req.uri().path())) else {return crate::routes::pass(&st,req,&fwd).await};
    let (parts,body)=req.into_parts();
    let Ok(bytes)=to_bytes(body,BODY_LIMIT).await else {return too_large(&parts.headers)};
    let spec=match runtime.claude_spec(&name).await {
        Some(Ok(spec)) if matches!(&spec.account,Account::Engine { account:Some(_),.. })=>spec,
        _=>match super::model_effort::serve(&st,runtime,name,&parts.headers,&bytes).await {
            Some(response)=>return response,
            None=>return crate::routes::pass(&st,Request::from_parts(parts,axum::body::Body::from(bytes)),&fwd).await,
        },
    };
    let Ok(body)=serde_json::from_slice::<Value>(&bytes) else {return super::invalid_body(&parts.headers)};
    if st.term.is_active(&name) {
        return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_terminal_aberto","Terminal aberto nesta sessao. Feche o painel pra responder por aqui.",json!({})));
    }
    let headers=parts.headers;
    let (Ok(model),Ok(effort))=(text(&body,"model"),text(&body,"effort")) else {return super::invalid_body(&headers)};
    if body.get("scope").is_some_and(|scope|scope!="session") {
        return reply(Some(&headers),StatusCode::CONFLICT,detail_body("erro_cliproxy_modelo_rota","a conta ChatGPT fixa não altera o padrão global de modelo ou esforço",json!({})));
    }
    if model.is_none() && effort.is_none() {return reply(Some(&headers),StatusCode::OK,json!({"ok":true,"scope":"session","result":null}));}
    let (status,result)=switch(&st,&runtime,&name,spec,EngineChange { account:None,model,effort,tier:None }).await;
    st.list.invalidate();
    if status!=StatusCode::OK {return reply(Some(&headers),status,result);}
    reply(Some(&headers),StatusCode::OK,json!({"ok":true,"scope":"session","result":null}))
}

/// `POST /service-tier`: o Fast do Claude com GPT no CLIProxyAPI local reabre a mesma identidade com o
/// tier novo; o resto (o Codex) segue à rota do Codex com o pedido inteiro.
pub async fn service_tier(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request) -> Response {
    let (_,owner)=match gate(&st,peer,&req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let runtime=st.state.runtime.get().cloned();
    let (Some(runtime),true,Some(name))=(runtime,owner,session_name(req.uri().path())) else {
        return super::codex::service_tier(State(st),ConnectInfo(peer),req).await;
    };
    let spec=match runtime.claude_spec(&name).await {
        Some(Ok(spec))=>spec,
        _=>return super::codex::service_tier(State(st),ConnectInfo(peer),req).await,
    };
    let (parts,body)=req.into_parts();
    let Ok(bytes)=to_bytes(body,BODY_LIMIT).await else {return too_large(&parts.headers)};
    let tier=serde_json::from_slice::<Value>(&bytes).ok().filter(|b|b.as_object().is_some_and(|o|o.len()==1))
        .and_then(|b|serde_json::from_value::<ServiceTier>(b["service_tier"].clone()).ok());
    let Some(tier)=tier else {return super::invalid_body(&parts.headers)};
    let engine=match &spec.account {Account::Engine { name,.. }=>Some(name.clone()),_=>None};
    let in_use=runtime.claude_model_in_use(&name).await.or(spec.model.clone());
    let fast={let (engine,model)=(engine.clone(),in_use);facts(runtime.codex_environment(),move|facts|facts.supports_fast(engine.as_deref(),model.as_deref())).await};
    if fast!=Some(true) {
        return reply(Some(&parts.headers),StatusCode::BAD_REQUEST,detail_body("erro_fast_indisponivel","Fast exige GPT no CLIProxyAPI local",json!({})));
    }
    if st.term.is_active(&name) {
        return reply(Some(&parts.headers),StatusCode::CONFLICT,detail_body("erro_terminal_aberto","Terminal aberto nesta sessao. Feche o painel pra responder por aqui.",json!({})));
    }
    let (status,body)=switch(&st,&runtime,&name,spec,EngineChange { account:None,model:None,effort:None,tier:Some(tier) }).await;
    st.list.invalidate();
    reply(Some(&parts.headers),status,body)
}

/// `POST /conta` com `engine_account`.
pub(crate) async fn account_route(st:Arc<AppState>,runtime:Arc<RuntimeRegistry>,name:String,account:String,headers:axum::http::HeaderMap) -> Response {
    let spec=match runtime.claude_spec(&name).await {
        Some(Ok(spec))=>spec,
        Some(Err(error))=>return reply(Some(&headers),StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({}))),
        None=>return reply(Some(&headers),StatusCode::CONFLICT,detail_body("erro_conta_so_claude","só sessão Claude troca de conta por esta rota",json!({}))),
    };
    let (status,body)=switch(&st,&runtime,&name,spec,EngineChange { account:Some(account),model:None,effort:None,tier:None }).await;
    st.list.invalidate();
    reply(Some(&headers),status,body)
}

/// Motor sem conta fixa: o catálogo valida a troca, e o ator conserva a identidade e o padrão global.
async fn unfixed_model(st:&Arc<AppState>,runtime:Arc<RuntimeRegistry>,name:&str,spec:SessionSpec,body:Value,headers:&axum::http::HeaderMap)->Response {
    use crate::runtime::{gateway::EntryHandle,protocol::{RuntimeCommand,OperationKind,Disposition}};
    let found=match super::enter_then_find(&runtime,name,st.write_gate_wait).await {
        Ok(None)=>match runtime.adopt_now(name).await {
            Some(Ok(()))=>super::enter_then_find(&runtime,name,st.write_gate_wait).await,
            Some(Err(error))=>return reply(Some(headers),StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({}))),
            None=>Ok(None),
        },
        other=>other,
    };
    let found=match found {Ok(found)=>found,Err(_)=>return reply(Some(headers),StatusCode::CONFLICT,super::busy_body())};
    let Some((held,target))=found else {return reply(Some(headers),StatusCode::SERVICE_UNAVAILABLE,detail_body("runtime_unhealthy","o ator da sessão está indisponível",json!({})))};
    let spec=match runtime.claude_spec(name).await {
        Some(Ok(current)) if current.account==spec.account && current.conversation==spec.conversation=>current,
        Some(Err(error))=>return reply(Some(headers),StatusCode::CONFLICT,detail_body(&error.code,&error.message,json!({}))),
        _=>return reply(Some(headers),StatusCode::CONFLICT,detail_body("session_transfer_busy","a identidade da sessão mudou; tente novamente",json!({}))),
    };
    let Account::Engine {name:engine,..}=&spec.account else {unreachable!()};
    let model=body["model"].as_str().unwrap();
    let models=match crate::engines::catalog::models(&st.accounts,engine,None,true).await {Ok(models)=>models,Err(error)=>{let mut response=error.response();crate::routes::cors(headers,response.headers_mut());return response}};
    let catalog_id=crate::engines::environment::catalog_model(model);
    if !models.iter().any(|entry|entry["id"]==catalog_id) {
        let quoted=crate::model_picker::py_repr(engine);
        return reply(Some(headers),StatusCode::UNPROCESSABLE_ENTITY,detail_body("erro_modelo_fora_catalogo",&format!("modelo fora do catalogo do motor {quoted}: {model}"),json!({"motor":engine,"modelo":model})));
    }
    if spec.service_tier==Some(ServiceTier::Priority) && !crate::engines::catalog::supports_fast(&st.accounts,engine,Some(model)) {
        return reply(Some(headers),StatusCode::CONFLICT,detail_body("erro_fast_indisponivel","Desligue Fast antes de escolher um modelo que não o suporta",json!({})));
    }
    let id=crate::mods::state::random_hex(16);
    let response=match &target.handle {
        EntryHandle::Terminal {handle,..}=>{
            if let Err(error)=crate::model_picker::normalize_engine(model) {return reply(Some(headers),StatusCode::UNPROCESSABLE_ENTITY,json!({"detail":error}));}
            match handle.control(id,"set_engine_model".into(),body.clone()).await {
                Ok(result) if result.disposition==Disposition::Accepted=>reply(Some(headers),StatusCode::OK,result.payload),
                Ok(result)=>{
                    let code=result.payload["error_code"].as_str();
                    let status=if code.is_some_and(|code|matches!(code,"engine_default_read_failed"|"engine_default_restore_failed")) {StatusCode::INTERNAL_SERVER_ERROR} else {StatusCode::CONFLICT};
                    let detail=result.payload["detail"].as_str().unwrap_or("model picker failed");
                    reply(Some(headers),status,match code {Some(code)=>detail_body(super::model_effort::public_code(code),detail,json!({})),None=>json!({"detail":detail})})
                },
                Err(error)=>reply(Some(headers),StatusCode::CONFLICT,detail_body(super::model_effort::public_code(&error.code),&error.message,json!({}))),
            }
        },
        EntryHandle::Headless(handle)=>{
            let effort=body["effort"].as_str().filter(|value|!value.is_empty());
            let result=tokio::time::timeout(std::time::Duration::from_secs(40),handle.command(RuntimeCommand {operation_id:id,kind:OperationKind::SetModel,payload:json!({"model":model,"effort":effort})})).await;
            match result {
                Ok(Ok(result)) if result.disposition==Disposition::Accepted || (result.disposition==Disposition::Deferred && effort.is_some())=>{
                    let mut answer=json!({"ok":true,"model":model});
                    if result.disposition==Disposition::Deferred {answer["effort_error"]=json!("esforço entra no fim do turno em andamento");}
                    reply(Some(headers),StatusCode::OK,answer)
                },
                other=>{
                    let message=match other {Ok(Ok(result))=>result.payload["error"].as_str().unwrap_or("resultado incerto da troca de modelo").to_owned(),Ok(Err(error))=>error.message,Err(_)=>"o Claude não respondeu à troca de modelo a tempo".into()};
                    reply(Some(headers),StatusCode::CONFLICT,detail_body("erro_headless_modelo",&format!("troca de modelo falhou: {message}"),json!({"erro":message})))
                },
            }
        },
        _=>reply(Some(headers),StatusCode::CONFLICT,detail_body("erro_rota_so_motor","esta rota so existe pra sessoes que rodam num motor",json!({}))),
    };
    drop(held);
    response
}
