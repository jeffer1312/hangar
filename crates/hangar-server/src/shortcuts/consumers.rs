//! Consumidores dos terminais já criados pelos atalhos.
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use crate::routes::AppState;

use super::terminal::{Service, Terminal};

fn missing(owner: &str) -> Response {
    if owner.is_empty() { super::guest::error(404,"erro_hangar_terminal_inexistente","terminal No Hangar nao encontrado",json!({})) }
    else { super::guest::error(404,"erro_shortcut_terminal_inexistente","terminal do atalho nao encontrado",json!({})) }
}
fn unavailable() -> Response {
    super::guest::error(500,"erro_shortcut_mux_indisponivel","o multiplexador nao respondeu; tente de novo",json!({}))
}

pub(super) async fn execute(st: &AppState, owner: &str, operation: &str, value: &Value) -> Response {
    let service = super::guest::service(st).await;
    if operation.is_empty() {
        return match list(st,&service,owner).await {
            Ok(value)=>crate::http_cors::json_guarded(StatusCode::OK,json!({"terminals":value})),
            Err(response)=>response,
        };
    }
    let Some((id, action)) = operation.split_once('/') else { return missing(owner); };
    if action=="answer" && value["text"].as_str().unwrap_or("").chars().any(|c| (c as u32)<0x20 || c=='\u{7f}') {
        return super::guest::error(400,"erro_shortcut_resposta_invalida","a resposta e uma linha so",json!({}));
    }
    let row = match service.find(owner,id).await {
        Ok(Some(row))=>row, Ok(None)=>return missing(owner), Err(_)=>return unavailable(),
    };
    let result = match action {
        "answer"=> {
            if !service.answer(&row.tmux,value["text"].as_str().unwrap_or("")).await {
                return super::guest::error(500,"erro_shortcut_resposta","o terminal recusou a resposta",json!({}));
            }
            json!({"ok":true})
        }
        "close"=> {
            if !service.close_row(&row).await {
                return if owner.is_empty() { super::guest::error(500,"erro_hangar_terminal_fechar","o terminal No Hangar nao fechou",json!({})) }
                else { super::guest::error(500,"erro_shortcut_terminal_fechar","o terminal do atalho nao fechou",json!({})) };
            }
            json!({"ok":true})
        }
        "focus"=>json!({"focused":super::focus::focus(st,&service,&Terminal { id:row.id,label:row.label,tmux:row.tmux }).await}),
        _=>return missing(owner),
    };
    crate::http_cors::json_guarded(StatusCode::OK,result)
}

async fn list(st: &AppState, service: &Service, owner: &str) -> Result<Value,Response> {
    let mut rows = service.rows().await.map_err(|_|unavailable())?;
    if owner.is_empty() { crate::list::local_shortcuts::maintain(&rows); }
    rows.retain(|row|row.owner==owner);
    rows.sort_by(|a,b|(a.created,a.seq,&a.tmux).cmp(&(b.created,b.seq,&b.tmux)));
    let children=if rows.iter().any(|row|row.alive && row.ask) {
        let procs=st.list.env().procs.clone();
        tokio::task::spawn_blocking(move || procs.children(Duration::ZERO)).await.ok().and_then(Result::ok)
    } else {None};
    let mut terminals=Vec::with_capacity(rows.len());
    for row in rows {
        let question=if row.alive && row.ask && let Some(children)=&children {
            crate::list::local_shortcuts::pending_question(&st.list.env().capture_program,&row.tmux,row.pid,children).await
        } else {None};
        let mut value=json!({"id":row.id,"label":row.label,"alive":row.alive,"exit_code":row.exit_code,"created":row.created,
            "ask":row.ask,"key":row.key,"question":question});
        if owner.is_empty() { value["owner"]=json!(row.owner);value["origin"]=json!(row.origin); }
        terminals.push(value);
    }
    Ok(Value::Array(terminals))
}

pub(super) async fn hangar(st: Arc<AppState>, req: Request, operation: String) -> Response {
    let (parts,body)=req.into_parts();
    let value=if operation.ends_with("/answer") {
        let bytes=match crate::http_body_limit::read(body).await {
            Ok(bytes)=>bytes,Err(_)=>return crate::http_cors::internal_error(),
        };
        match super::body::parse(super::body::Schema::Answer,&bytes,&parts.headers) {
            Ok(value)=>value,Err(response)=>return response,
        }
    } else {Value::Null};
    tokio::spawn(async move { execute(&st,"",&operation,&value).await }).await.unwrap_or_else(|_|unavailable())
}
