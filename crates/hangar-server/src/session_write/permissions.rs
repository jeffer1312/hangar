//! Modos Claude, no ator que já serializa suas escritas; o relançamento só começa sem IngressPass.
use std::{net::SocketAddr,sync::Arc};
use axum::{extract::{ConnectInfo,Request,State},body::to_bytes,http::StatusCode,response::Response};
use serde::Deserialize;
use serde_json::{Value,json};
use crate::{routes::{AppState,gate,pass,cors},runtime::{gateway::EntryHandle,protocol::{OperationKind,RuntimeCommand,Disposition}}};
use super::{BODY_LIMIT,enter_then_find,json_response,detail_body,session_name};
const MODES:[&str;6]=["plan","auto","manual","acceptEdits","bypassPermissions","dontAsk"];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeBody {mode:Option<String>,permission_mode:Option<String>}
fn mode_body(bytes:&[u8])->Result<String,(&'static str,StatusCode)> {
    let body:ModeBody=serde_json::from_slice(bytes).map_err(|_|("erro_permissao_invalida",StatusCode::UNPROCESSABLE_ENTITY))?;
    let mode=body.mode.or(body.permission_mode).filter(|s|!s.is_empty()).ok_or(("erro_permissao_invalida",StatusCode::UNPROCESSABLE_ENTITY))?;
    if !MODES.contains(&mode.as_str()) {return Err(("erro_permissao_invalida",StatusCode::CONFLICT));}
    Ok(mode)
}
/// As recusas que o app já explica, nos códigos que ele conhece (os do Python).
fn public_code(code:&str)->&str {
    match code {"terminal_panel_active"|"terminal_panel_open"=>"erro_terminal_aberto","permission_menu_open"=>"erro_menu_aberto",_=>code}
}
fn result(reply:crate::runtime::protocol::RuntimeReply)->(StatusCode,Value) {
    if reply.disposition==Disposition::Accepted {return (StatusCode::OK,reply.payload);}
    let code=public_code(reply.payload["error_code"].as_str().or_else(||reply.payload["code"].as_str()).unwrap_or("erro_permissao_leitura"));
    // A frase do app (`erro_permissao_teto`) lê `alvo` e `ficou`, os nomes do Python.
    let mut params=reply.payload.clone();
    if params["requested"].is_string() {params["alvo"]=params["requested"].clone();params["ficou"]=params["current"].clone();}
    (StatusCode::CONFLICT,detail_body(code,"não consegui confirmar o modo de permissão; confira a sessão",params))
}
async fn serve(st:Arc<AppState>,peer:SocketAddr,req:Request,set:bool)->Response {
    let (forward,owner)=gate(&st,peer,&req);
    let Some(runtime)=st.state.runtime.get().cloned().filter(|_|owner) else {return pass(&st,req,&forward).await;};
    let Some(name)=session_name(req.uri().path()) else {return pass(&st,req,&forward).await;};
    // Claude com terminal ainda sem ator aqui: o Rust o adota agora, em vez de mandar ao Python.
    let found=match enter_then_find(&runtime,&name,st.write_gate_wait).await {
        Ok(None)=>match runtime.adopt_now(&name).await {
            Some(Ok(()))=>enter_then_find(&runtime,&name,st.write_gate_wait).await,
            // Codex abrindo a conversa: esta rota é do Claude, e segue como antes.
            Some(Err(error)) if error.code==crate::runtime::codex_relaunch::STARTING=>Ok(None),
            Some(Err(error))=>{let mut response=json_response(StatusCode::CONFLICT,detail_body(public_code(&error.code),&error.message,json!({})));cors(req.headers(),response.headers_mut());return response},
            None=>Ok(None),
        },
        other=>other,
    };
    let target=match found {
        Ok(Some((held,target))) if target.provider=="claude"=>(held,target),
        Ok(_)=>return pass(&st,req,&forward).await,
        Err(_)=>{let mut response=json_response(StatusCode::CONFLICT,super::busy_body());cors(req.headers(),response.headers_mut());return response},
    };
    let (held,target)=target;
    let headers=req.headers().clone();
    let id=headers.get("x-hangar-operation-id").and_then(|v|v.to_str().ok()).filter(|id|!id.is_empty() && id.len()<=200 && id.bytes().all(|b|b.is_ascii_alphanumeric() || b"._:-".contains(&b))).map(str::to_owned).unwrap_or_else(||crate::mods::state::random_hex(16));
    let query=req.uri().query().unwrap_or("");
    let probe=form_urlencoded::parse(query.as_bytes()).find(|(key,_)|key=="sondar").is_some_and(|(_,value)|matches!(value.to_ascii_lowercase().as_str(),"1"|"true"|"on"|"yes"));
    let mode=if set {
        match to_bytes(req.into_body(),BODY_LIMIT).await {
            Ok(bytes)=>match mode_body(&bytes) {Ok(mode)=>Some(mode),Err((code,status))=>{let mut response=json_response(status,detail_body(code,"modo de permissão inválido",json!({})));cors(&headers,response.headers_mut());return response}},
            Err(_)=>return super::too_large(&headers),
        }
    } else {None};
    // A tarefa carrega o passe só pelo comando; o freeze nunca espera o próprio pedido.
    let life_st=st.clone();
    let job=tokio::spawn(async move {
        let reply=match &target.handle {
            EntryHandle::Terminal {handle,..}=>handle.control(id.clone(),if set {"set_permission_mode"} else {"permission_modes"}.into(),if let Some(mode)=&mode {json!({"mode":mode})} else {json!({"probe":probe})}).await?,
            EntryHandle::Headless(handle) if set=> {
                let mut reply=handle.command(RuntimeCommand {operation_id:id.clone(),kind:OperationKind::SetPermissionMode,payload:json!({"mode":mode})}).await?;
                if reply.disposition==Disposition::Accepted && reply.payload["reopened"]!=true {
                    let view=handle.view().await?;
                    reply.payload=json!({"mode":view["permission_mode"],"current":view["permission_mode"],"previous_non_plan":view["previous_non_plan"]});
                }
                reply
            },
            EntryHandle::Headless(handle)=> {
                let view=handle.view().await?;
                return Ok((StatusCode::OK,json!({"current":view["permission_mode"],"modes":MODES,"sondavel":false,"previous_non_plan":view["previous_non_plan"]})));
            },
            _=>return Err(crate::runtime::protocol::RuntimeError::new("lifecycle_provider","sessão não é Claude")),
        };
        drop(held);
        let terminal_reopen=target.terminal && reply.payload["requires_reopen"]==true && reply.payload["cycle_closed"]==true;
        let headless_reopen=!target.terminal && reply.disposition==Disposition::Rejected && reply.payload["permission_boot_rejected"]==true;
        if set && mode.as_deref()==Some("bypassPermissions") && terminal_reopen && reply.payload["uncertain"]!=true {
            return Ok(bypass_reopen(&life_st,runtime.clone(),&name).await);
        }
        if set && mode.as_deref()==Some("bypassPermissions") && headless_reopen && reply.payload["uncertain"]!=true {
            return runtime.reopen_claude(name,target.key,target.generation,id).await.map(|value|(StatusCode::OK,value));
        }
        Ok(result(reply))
    });
    let (status,body)=match job.await {
        Ok(Ok(value))=>value,
        Ok(Err(error))=>(StatusCode::CONFLICT,detail_body(public_code(&error.code),&error.message,json!({}))),
        Err(_)=>(StatusCode::SERVICE_UNAVAILABLE,detail_body("lifecycle_panic","não consegui confirmar o modo de permissão",json!({}))),
    };
    let mut response=json_response(status,body);cors(&headers,response.headers_mut());response
}
/// Bypass fora do ciclo do terminal: a sessão reabre pelo spec já em bypass. A falha depois de encerrar
/// volta pelo spec de antes e diz onde a sessão ficou.
async fn bypass_reopen(st:&Arc<AppState>,runtime:Arc<crate::runtime::gateway::RuntimeRegistry>,name:&str)->(StatusCode,Value) {
    use crate::session_open::{ChangeReason,PermissionMode,SessionChange};
    let change=SessionChange {reason:ChangeReason::Bypass,surface:None,account:None,permission_mode:Some(PermissionMode::BypassPermissions),model:None,effort:None,service_tier:None};
    let owned=name.to_owned();
    match crate::session_write::relaunch::reopen_detached(st,name,async move {runtime.reopen_claude_session(&owned,change,None).await}).await {
        Some(Ok(reopened))=>(StatusCode::OK,reopened.answer(json!({"mode":"bypassPermissions","current":"bypassPermissions","previous_non_plan":"bypassPermissions","reopened":true}))),
        Some(Err(failure))=>{
            let error=&failure.error;
            if let Some(message)=crate::runtime::claude_reopen::busy_message(&error.code) {
                return (StatusCode::CONFLICT,detail_body(&error.code,&format!("para entrar em bypass a sessão reinicia: {message}"),json!({})));
            }
            let message=if failure.stopped {error.message.clone()} else {format!("não reabri em bypass: {}",error.message)};
            (StatusCode::CONFLICT,detail_body("erro_permissao_reabrir",&message,json!({"erro":error.message,"code":error.code})))
        },
        None=>(StatusCode::CONFLICT,detail_body("erro_permissao_reabrir","não reabri em bypass: a sessão não está aberta aqui",json!({}))),
    }
}
pub async fn modes(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request)->Response {serve(st,peer,req,false).await}
pub async fn set_mode(State(st):State<Arc<AppState>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,req:Request)->Response {serve(st,peer,req,true).await}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_alias_and_rejection_status_are_preserved() {
        assert_eq!(mode_body(br#"{"permission_mode":"bypassPermissions"}"#).unwrap(),"bypassPermissions");
        assert_eq!(mode_body(br#"{"mode":"plan","permission_mode":"manual"}"#).unwrap(),"plan");
        assert_eq!(mode_body(b"{}").unwrap_err().1,StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(mode_body(br#"{"mode":"wrong"}"#).unwrap_err().1,StatusCode::CONFLICT);
    }
    #[test]
    fn uncertain_control_cannot_report_requested_bypass() {
        let reply=crate::runtime::protocol::RuntimeReply {operation_id:"id".into(),disposition:Disposition::Unknown,payload:json!({"current":null,"requested":"bypassPermissions","uncertain":true,"error_code":"permission_key_timeout"})};
        let (status,body)=result(reply);
        assert_eq!(status,StatusCode::CONFLICT);
        assert_eq!(body["detail"]["params"]["current"],Value::Null);
        assert_eq!(body["detail"]["code"],"permission_key_timeout");
    }
    /// Parity B2 (`test_perm_post_teto_409_devolve_ficou`).
    #[test]
    fn ceiling_refusal_carries_the_python_names() {
        let reply=crate::runtime::protocol::RuntimeReply {operation_id:"id".into(),disposition:Disposition::Rejected,payload:json!({"current":"plan","mode":"plan","requested":"dontAsk","error_code":"erro_permissao_teto"})};
        let (status,body)=result(reply);
        assert_eq!(status,StatusCode::CONFLICT);
        assert_eq!((body["detail"]["params"]["alvo"].as_str(),body["detail"]["params"]["ficou"].as_str(),body["detail"]["params"]["mode"].as_str()),(Some("dontAsk"),Some("plan"),Some("plan")));
        // Parity B3: painel e menu abertos recusam com os códigos que o app explica.
        for (internal,public) in [("terminal_panel_active","erro_terminal_aberto"),("permission_menu_open","erro_menu_aberto")] {
            let reply=crate::runtime::protocol::RuntimeReply {operation_id:"id".into(),disposition:Disposition::Rejected,payload:json!({"error_code":internal})};
            assert_eq!(result(reply).1["detail"]["code"],public);
        }
    }
}
