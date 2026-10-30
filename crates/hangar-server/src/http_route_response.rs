//! Respostas de roteamento iguais às da borda HTTP original, sem decidir autenticação.
use axum::{body::{Body,HttpBody},http::{HeaderMap,HeaderValue,Method,Uri},response::Response};
use crate::http_contract::Decision;
const SAFE:&percent_encoding::AsciiSet=&percent_encoding::NON_ALPHANUMERIC
    .remove(b'-').remove(b'.').remove(b'_').remove(b'~').remove(b':').remove(b'/')
    .remove(b'%').remove(b'#').remove(b'?').remove(b'=').remove(b'@').remove(b'[').remove(b']')
    .remove(b'!').remove(b'$').remove(b'&').remove(b'\'').remove(b'(').remove(b')')
    .remove(b'*').remove(b'+').remove(b',').remove(b';');
pub fn response(decision:&Decision,method:&Method,uri:&Uri,headers:&HeaderMap,https:bool,fallback_authority:&str)->Option<Response>{
    let (status,body,allow)=match decision{
        Decision::Matched=>return None,
        Decision::NotFound=>(404,"{\"detail\":\"Not Found\"}",None),
        Decision::MethodNotAllowed(allow)=>(405,"{\"detail\":\"Method Not Allowed\"}",Some(allow)),
        Decision::Redirect(path)=>{
            let authority=headers.get("host").and_then(|v|v.to_str().ok()).or_else(||uri.authority().map(|a|a.as_str())).unwrap_or(fallback_authority);
            let mut location=if authority.is_empty(){path.clone()}else{format!("{}://{authority}{path}",if https{"https"}else{"http"})};
            if let Some(query)=uri.query(){location.push('?');location.push_str(query);}
            let location=percent_encoding::utf8_percent_encode(&location,SAFE).to_string();
            return Some(Response::builder().status(307).header("location",location).header("content-length","0").body(Body::empty()).unwrap());
        },
    };
    let mut builder=Response::builder().status(status).header("content-type","application/json").header("content-length",body.len());
    if let Some(allow)=allow{builder=builder.header("allow",allow);}
    Some(builder.body(if method==Method::HEAD{Body::empty()}else{Body::from(body)}).unwrap())
}
pub fn suppress_head(method:&Method,response:&mut Response){
    if method!=Method::HEAD{return;}
    if !response.headers().contains_key("content-length")&&!matches!(response.status().as_u16(),204|304){
        if let Some(length)=response.body().size_hint().exact(){response.headers_mut().insert("content-length",HeaderValue::from_str(&length.to_string()).unwrap());}
    }
    // Corpo de tamanho desconhecido não vira Content-Length: 0 na camada HTTP externa.
    *response.body_mut()=Body::from_stream(futures_util::stream::empty::<Result<axum::body::Bytes,std::convert::Infallible>>());
}
