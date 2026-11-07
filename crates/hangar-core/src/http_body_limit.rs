//! Limite do corpo nas rotas migradas, sem consumir pedidos recusados pela autenticação.
use axum::{body::{Body,Bytes},http::{HeaderMap,StatusCode},response::Response};
pub const MAX_BYTES:usize=100*1024*1024;
pub fn rejected()->Response{
    Response::builder().status(StatusCode::PAYLOAD_TOO_LARGE).header("content-type","text/plain; charset=utf-8").body(Body::from_stream(futures_util::stream::once(async{Ok::<_,std::convert::Infallible>(Bytes::from_static(b"request body too large"))}))).unwrap()
}
pub fn precheck(headers:&HeaderMap)->Option<Response>{precheck_with_limit(headers,MAX_BYTES)}
pub fn precheck_with_limit(headers:&HeaderMap,limit:usize)->Option<Response>{
    let raw=headers.get_all("content-length").iter().last()?.as_bytes();
    if !raw.is_empty()&&raw.iter().all(u8::is_ascii_digit)&&std::str::from_utf8(raw).ok().and_then(|s|{let s=s.trim_start_matches('0');if s.is_empty(){Some(0)}else{s.parse::<usize>().ok()}}).is_none_or(|n|n>limit){Some(rejected())}else{None}
}
pub async fn read(body:Body)->Result<Bytes,axum::Error>{axum::body::to_bytes(body,MAX_BYTES).await}
pub fn is_limit(error:&axum::Error)->bool{
    let mut current:Option<&(dyn std::error::Error+'static)>=Some(error);
    while let Some(error)=current{if error.is::<http_body_util::LengthLimitError>(){return true;}current=error.source();}
    false
}
