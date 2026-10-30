//! Contrato CORS das rotas migradas, aplicado antes de autenticação e limite de corpo.
use axum::{body::Body,http::{HeaderMap,HeaderValue,Method,StatusCode},response::Response};

#[derive(Clone, Copy, Debug)]
pub struct UnhandledServerError;

const METHODS:&str="DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT";
const EXPOSE:&str="ETag, x-hangar-css-width, x-hangar-css-height";

pub fn preflight(method:&Method,headers:&HeaderMap)->Option<Response>{
    if method!=Method::OPTIONS || !headers.contains_key("origin"){return None;}
    let requested=headers.get("access-control-request-method")?;
    let allowed=requested.to_str().ok().is_some_and(|method|METHODS.split(", ").any(|allowed|allowed==method));
    let body=if allowed{"OK"}else{"Disallowed CORS method"};
    let mut response=Response::builder().status(if allowed{StatusCode::OK}else{StatusCode::BAD_REQUEST})
        .header("access-control-allow-origin","*").header("access-control-allow-methods",METHODS)
        .header("access-control-max-age","600").header("content-type","text/plain; charset=utf-8")
        .header("content-length",body.len()).body(Body::from(body)).unwrap();
    if let Some(value)=headers.get("access-control-request-headers"){
        response.headers_mut().insert("access-control-allow-headers",value.clone());
    }
    Some(response)
}

pub fn apply(headers:&HeaderMap,response:&mut Response){
    if response.extensions().get::<UnhandledServerError>().is_some() || !headers.contains_key("origin"){return;}
    response.headers_mut().insert("access-control-allow-origin",HeaderValue::from_static("*"));
    response.headers_mut().insert("access-control-expose-headers",HeaderValue::from_static(EXPOSE));
}

/// 500 em texto puro, como a exceção que o Python levantava fora do CORSMiddleware.
pub fn internal_error()->Response{
    use axum::response::IntoResponse;
    let mut response=(StatusCode::INTERNAL_SERVER_ERROR,"Internal Server Error").into_response();
    response.extensions_mut().insert(UnhandledServerError);
    response
}

/// JSON compacto em UTF-8 do `JSONResponse`; surrogate solto ou não finito não serializa e vira 500.
pub fn json_guarded(status:StatusCode,value:serde_json::Value)->Response{
    use crate::orchestration::codec;
    if codec::contains_surrogate(&value)||codec::contains_nonfinite(&value){
        codec::discard(value);
        return internal_error();
    }
    let text=crate::transcript::pyjson::dumps_compact_unicode(&value);
    codec::discard(value);
    use axum::response::IntoResponse;
    (status,[(axum::http::header::CONTENT_TYPE,"application/json")],text).into_response()
}

/// O mesmo contrato com o JSON ASCII de `codec::response`.
pub fn json_guarded_ascii(status:StatusCode,value:serde_json::Value)->Response{
    use crate::orchestration::codec;
    if codec::contains_surrogate(&value)||codec::contains_nonfinite(&value){
        codec::discard(value);
        return internal_error();
    }
    codec::response(status,value)
}
