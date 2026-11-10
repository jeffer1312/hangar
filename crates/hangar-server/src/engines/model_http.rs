//! Seleção dos motores nas telas de abertura e da sessão, sem fatos do Python.
use std::collections::HashMap;
use axum::{http::{Method,StatusCode},response::Response};
use serde_json::json;
use crate::{routes::AppState,session_write::json_response};
use super::{catalog,store};

async fn account_catalog(st: &AppState, context: crate::dictation::context::Context, refresh: bool, current: Option<&str>) -> Response {
    use crate::dictation::catalog::parse;
    match st.dictation_models.get_context(&st.accounts, &st.dictation_slots, &context, refresh).await {
        Ok((models, raw)) => {
            let models = if context.provider == "claude" {
                match parse("claude", &json!({"models": raw}), current) {
                    Ok(models) => models,
                    Err(_) => return catalog_failure("dictation_catalog_unavailable"),
                }
            } else { models };
            json_response(StatusCode::OK, json!({"kind":context.provider,"engine":null,"reduced":false,"models":models}))
        }
        Err(code) => catalog_failure(code),
    }
}

fn catalog_failure(code: &str) -> Response {
    crate::provider_models::ModelError::coded(503, "erro_modelos_indisponiveis",
        "Não consegui consultar os modelos. Tente atualizar o catálogo novamente.", json!({"cause":code})).response()
}

pub async fn response(st:&AppState,method:&Method,path:&str,query:Option<&str>) -> Option<Response> {
    if method!=Method::GET {return None;}
    let params:HashMap<String,String>=form_urlencoded::parse(query.unwrap_or("").as_bytes()).into_owned().collect();
    let refresh = params.get("refresh").is_some_and(|v| v == "1" || v == "true");
    if path=="/api/model-options" {
        let engine=params.get("engine").filter(|e|!e.is_empty());
        let account=params.get("engine_account").map(String::as_str);
        if account.is_some() && (engine.is_none() || params.get("provider").is_some_and(|p|p!="claude")) {
            return Some(crate::provider_models::ModelError::coded(400,"erro_cliproxy_conta","conta ChatGPT exige Claude com motor CLIProxyAPI local",json!({})).response());
        }
        if engine.is_none() {
            let provider = params.get("provider").map(String::as_str).unwrap_or("claude");
            if matches!(provider, "pi" | "omp" | "kimi") { return None; }
            let home = if provider == "codex" {
                let id = params.get("codex_account").filter(|s| !s.is_empty()).map(String::as_str).unwrap_or("default");
                match st.accounts.resolve(crate::accounts::Provider::Codex, id) {
                    Ok(account) => account.home,
                    Err(error) => return Some(crate::provider_models::ModelError::coded(error.status, error.code, &error.message, error.params).response()),
                }
            } else if provider == "claude" {
                match params.get("config_dir").map(|p| p.trim()).filter(|p| !p.is_empty() && *p != "~") {
                    None => st.accounts.env.claude_base.clone(),
                    Some(path) => path.strip_prefix("~/").map(|p| st.accounts.env.home.join(p)).unwrap_or_else(|| path.into()),
                }
            } else {
                return Some(crate::provider_models::ModelError::coded(400, "erro_provider_invalido", "Provedor inválido.", json!({})).response());
            };
            return Some(match crate::dictation::context::account(provider, &home, &st.accounts.env) {
                Ok(context) => account_catalog(st, context, refresh, None).await,
                Err(code) => catalog_failure(code),
            });
        }
        let engine=engine?;
        if params.get("provider").is_some_and(|p|p!="claude") {return None;}
        return Some(match catalog::models(&st.accounts,engine,account,refresh).await {
            Ok(models)=>{let models=catalog::picker(&st.accounts,engine,models);let fast=models.iter().any(|m|m["supports_fast"]==true);json_response(StatusCode::OK,json!({"kind":"engine","reduced":false,"models":models,"supports_fast":fast}))},
            Err(error)=>error.response(),
        });
    }
    let name=path.strip_prefix("/api/sessions/")?.strip_suffix("/model/options")?;
    let name=percent_encoding::percent_decode_str(name).decode_utf8().ok()?;
    let row=match st.list.discovered_session(&name).await {
        Ok(Some((row,_)))=>row,
        Ok(None)=>return Some(crate::provider_models::ModelError::coded(404,"erro_sessao_inexistente","sessao nao encontrada",json!({})).response()),
        Err(_)=>return Some(crate::provider_models::ModelError::coded(503,"erro_modelos_indisponiveis","não consegui localizar a sessão",json!({})).response()),
    };
    if row.provider != "claude" {
        return Some(crate::provider_models::ModelError::coded(400,"erro_rota_so_claude","Esta rota só existe para sessões Claude Code.",json!({})).response());
    }
    let Some(engine) = row.engine.as_ref() else {
        let context = crate::dictation::context::for_row(st, &row, &Default::default());
        let current = if let Some(runtime) = st.state.runtime.get() { runtime.claude_model_in_use(&name).await } else { None }.or(row.model.clone());
        return Some(match context {
            Ok(context) => account_catalog(st, context, refresh, current.as_deref()).await,
            Err(code) => catalog_failure(code),
        });
    };
    Some(match catalog::models(&st.accounts,engine,row.engine_account.as_deref(),refresh).await {
        Ok(models)=>{
            let models=catalog::picker(&st.accounts,&engine,models);
            let fast=models.iter().any(|m|m["supports_fast"]==true);
            let mut result=json!({"kind":"engine","engine":engine,"models":models});
            if fast {
                let runtime=st.state.runtime.get();
                let model=if let Some(runtime)=runtime {runtime.claude_model_in_use(&name).await} else {None}.or_else(||store::state(&st.accounts.env).0.get(engine).and_then(|v|v["model"].as_str()).map(str::to_owned));
                let tier=if let Some(runtime)=runtime {runtime.claude_spec(&name).await.and_then(Result::ok).and_then(|s|s.service_tier).map(|s|s.name())} else {None}.unwrap_or("default");
                result["supports_fast"]=json!(true);result["current"]=json!({"model":model.map(|s|s.rsplit('/').next().unwrap_or("").to_owned()),"service_tier":tier});
            }
            json_response(StatusCode::OK,result)
        },
        Err(error)=>error.response(),
    })
}
pub fn candidate(method:&Method,path:&str)->bool {
    method==Method::GET && (path=="/api/model-options" || (path.starts_with("/api/sessions/")&&path.ends_with("/model/options")))
}
