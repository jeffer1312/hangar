//! Contrato CORS das rotas migradas, aplicado antes de autenticação e limite de corpo.
use axum::{body::Body,extract::Request,http::{HeaderMap,HeaderValue,Method,StatusCode},middleware::Next,response::Response};

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

pub async fn layer(request:Request,next:Next)->Response{
    if let Some(response)=preflight(request.method(),request.headers()){return response;}
    let headers=request.headers().clone();
    let mut response=next.run(request).await;
    apply(&headers,&mut response);
    response
}
