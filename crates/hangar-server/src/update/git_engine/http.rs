//! Rotas do checkout atendidas no processo Rust, sem consulta de volta ao Python.
use super::Service;
use crate::routes::AppState;
use axum::{extract::Request, http::StatusCode, response::Response};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
fn response(status:u16,body:Value)->Response {
    if status==500 && body.is_null() {return crate::http_cors::internal_error();}
    crate::http_cors::json_guarded_ascii(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),body)
}
use crate::orchestration::codec::ValueGuard;
enum ChannelInput {Json(ValueGuard),Raw(Vec<u8>)}
impl ChannelInput {
    fn parse(bytes:&[u8],headers:&axum::http::HeaderMap)->Result<Self,(u16,Value)> {
        if bytes.is_empty() || crate::http_body_input::is_json(headers) {
            crate::orchestration::body::parse(bytes).map(|value|Self::Json(ValueGuard(value))).map_err(|(status,value)|(status.as_u16(),value))
        } else {Ok(Self::Raw(bytes.to_vec()))}
    }
    fn validate(self)->Result<String,(u16,Value)> {
        match self {
            Self::Json(value)=>validate_channel(&value.0),
            Self::Raw(bytes)=>String::from_utf8(bytes).map_err(|_|(500,Value::Null)).and_then(|value|validate_channel(&Value::String(value))),
        }
    }
}
fn validate_channel(value:&Value)->Result<String,(u16,Value)> {
    use crate::orchestration::codec;
    let invalid=|kind:&str,loc:Value,msg:&str,input:&Value| {
        let mut error=json!({"type":kind,"loc":loc,"msg":msg});
        error["input"]=codec::clone_value(input);
        let mut fields=serde_json::Map::new();fields.insert("detail".into(),Value::Array(vec![error]));let errors=Value::Object(fields);
        if codec::contains_nonfinite(&errors)||codec::contains_surrogate(&errors) {
            codec::discard(errors);(500,Value::Null)
        } else {(422,errors)}
    };
    if value.is_null() {return Err(invalid("missing",json!(["body"]),"Field required",&Value::Null));}
    let Some(fields)=value.as_object() else {return Err(invalid("model_attributes_type",json!(["body"]),"Input should be a valid dictionary or object to extract fields from",value));};
    let Some(branch)=fields.get("branch") else {return Err(invalid("missing",json!(["body","branch"]),"Field required",value));};
    if let Some(text)=branch.as_str().filter(|_|!codec::contains_nonfinite(branch)){Ok(text.to_owned())}
    else {Err(invalid("string_type",json!(["body","branch"]),"Input should be a valid string",branch))}
}
pub fn owns_update_path(path:&str)->bool {path=="/api/atualizacao"||path.starts_with("/api/atualizacao/")}
pub fn owns_channel_path(path:&str)->bool {
    path=="/api/update-channel" || path.starts_with("/api/update-channel/")
}
pub async fn channel(
    axum::extract::State(st):axum::extract::State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer):axum::extract::ConnectInfo<SocketAddr>,
    req:Request,
)->Response { serve(st,peer,req).await }
pub async fn serve(st:Arc<AppState>,peer:SocketAddr,req:Request)->Response {
    if let Some(response)=crate::http_cors::preflight(req.method(),req.headers()){return response;}
    let headers=req.headers().clone();let method=req.method().clone();
    let mut response=if let Some(response)=crate::http_body_limit::precheck(&headers){response}else{serve_inner(st,peer,req).await};
    crate::http_cors::apply(&headers,&mut response);
    crate::http_route_response::suppress_head(&method,&mut response);
    response
}
async fn serve_inner(st:Arc<AppState>,peer:SocketAddr,mut req:Request)->Response {
    let path=percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy().into_owned();
    if owns_channel_path(&path)||owns_update_path(&path) {
        let decision=crate::http_contract::resolve(&[("/api/update-channel","GET"),("/api/update-channel","PUT"),("/api/atualizacao","GET"),("/api/atualizacao/iniciar","POST"),("/api/atualizacao/reiniciar","POST")],req.method().as_str(),&path);
        let (_,https)=st.cfg.trusted.resolve(peer.ip(),req.headers());
        if let Some(response)=crate::http_route_response::response(&decision,req.method(),req.uri(),req.headers(),https,&st.cfg.listen.to_string()){return response;}
    }
    let method=req.method().clone();
    if owns_update_path(&path)&&crate::update::http::mode(&st.accounts.env.home)!=crate::update::http::Mode::Unmigrated {
        return match path.as_str(){
            "/api/atualizacao"=>Box::pin(crate::update::http::status(axum::extract::State(st),axum::extract::ConnectInfo(peer),req)).await,
            "/api/atualizacao/iniciar"=>Box::pin(crate::update::http::start(axum::extract::State(st),axum::extract::ConnectInfo(peer),req)).await,
            "/api/atualizacao/reiniciar"=>Box::pin(crate::update::http::restart(axum::extract::State(st),axum::extract::ConnectInfo(peer),req)).await,
            _=>response(404,json!({"detail":"Not Found"})),
        };
    }
    let package=path=="/api/update-channel" && matches!(crate::update::http::mode(&st.accounts.env.home),crate::update::http::Mode::Package(_));
    let input=if path=="/api/update-channel" && method==axum::http::Method::PUT && !package {
        let bytes=match crate::http_body_limit::read(std::mem::replace(req.body_mut(),axum::body::Body::empty())).await {
            Ok(bytes)=>bytes,
            Err(error) if crate::http_body_limit::is_limit(&error)=>return crate::http_body_limit::rejected(),
            Err(_)=>return response(400,json!({"detail":"There was an error parsing the body"})),
        };
        match ChannelInput::parse(&bytes,req.headers()){Ok(input)=>Some(input),Err((status,body))=>return response(status,body)}
    }else{None};
    if let Err(response)=crate::routes::owner_gate(&st,peer,&req).await{return response;}
    if package {
        return if method==axum::http::Method::PUT {Box::pin(crate::update::http::channel_write(axum::extract::State(st),axum::extract::ConnectInfo(peer),req)).await}
            else {Box::pin(crate::update::http::channel_read(axum::extract::State(st),axum::extract::ConnectInfo(peer),req)).await};
    }
    let refresh=match refresh_query(req.uri().query()){Ok(value)=>value,Err(response)=>return *response};
    let branch=match input {
        Some(input)=>match input.validate(){Ok(branch)=>Some(branch),Err((status,body))=>return response(status,body)},
        None=>None,
    };
    let service = match Service::from_state(&st) {
        Ok(service) => service,
        Err(error) => {
            return crate::session_write::detail(
                StatusCode::SERVICE_UNAVAILABLE,
                "update_checkout_unavailable",
                &error,
                json!({}),
            );
        }
    };
    let port = st.cfg.listen.port();
    let result=tokio::task::spawn_blocking(move||->Result<Value,(u16,Value)>{
 let fail=|error:String|(500,json!({"detail":{"code":"update_checkout_failed","msg":error,"params":{}}}));
 match path.as_str(){
 "/api/update-channel"=>{let result=if let Some(branch)=&branch{service.write_channel(branch)}else{service.read_channel()};result.map_err(|e|{let mut detail=json!({"code":e.code,"msg":e.msg,"params":{}});if e.code=="update_channel_missing"{detail["params"]["branch"]=json!(branch.unwrap_or_default().trim());}(e.status,json!({"detail":detail}))})},
 "/api/atualizacao"=>service.status(refresh,&super::releases::HttpSource,Some(&crate::update::platform())).map(|mut body|{crate::update::http::mark_pending_warning(&mut body,&service.home);body}).map_err(fail),
 "/api/atualizacao/iniciar"=>{let pre=service.preflight().map_err(fail)?;if pre["branch_de_trabalho"]==true{return Err((409,json!({"detail":{"code":"erro_atualizacao_branch","msg":format!("este checkout esta na branch {}, nao na {}",pre["branch"].as_str().unwrap_or(""),pre["alvo"].as_str().unwrap_or("main")),"params":{"branch":pre["branch"],"alvo":pre["alvo"]}}})));}if pre["pode"]!=true{return Err((409,json!({"detail":{"code":"erro_atualizacao_dependencia","msg":format!("falta o que a atualizacao precisa: {}",pre["faltando"].as_array().map(|v|v.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()),"params":{"faltando":pre["faltando"]}}})));}super::status::clear_published();let result=service.start_detached(port,None,false).map_err(fail)?;if result["ok"]!=true{return Err((409,json!({"detail":{"code":"erro_atualizacao_ja_rodando","msg":"ja existe uma atualizacao rodando","params":{}}})));}Ok(result)},
 "/api/atualizacao/reiniciar"=>{let result=service.start_detached(port,None,true).map_err(fail)?;if result["erro"]=="ja_rodando"{return Err((409,json!({"detail":{"code":"erro_atualizacao_ja_rodando","msg":"ja existe uma atualizacao rodando","params":{}}})));}if result["ok"]!=true{return Err((409,json!({"detail":{"code":"erro_reinicio_indisponivel","msg":format!("esta maquina nao reinicia sozinha (topologia {})",result["topologia"].as_str().unwrap_or("")),"params":{"topologia":result["topologia"]}}})));}Ok(result)},
 _=>Err((404,json!({"detail":"Not Found"})))}}).await;
    let output = match result {
        Ok(Ok(body)) => response(200, body),
        Ok(Err((status, body))) => response(status, body),
        Err(_) => response(
            500,
            json!({"detail":"a atualização não conseguiu responder"}),
        ),
    };
    output
}

pub fn refresh_query(query:Option<&str>)->Result<bool,Box<Response>> {
    let values=form_urlencoded::parse(query.unwrap_or("").as_bytes()).into_owned().collect();
    crate::query::bool_param(&values,"procurar")
}
