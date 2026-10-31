//! Classificação MIME usada antes da autenticação e da validação do modelo.
use axum::http::HeaderMap;
pub fn is_json(headers:&HeaderMap)->bool {
    let Some(value)=headers.get("content-type") else{return false;};
    // ASGI expõe cabeçalhos em Latin-1; parâmetros não ASCII não invalidam o tipo MIME.
    let value:String=value.as_bytes().iter().map(|byte|char::from(*byte)).collect();
    let media_type=crate::transcript::py::strip(value.split(';').next().unwrap_or("")).to_lowercase();
    let Some((main,subtype))=media_type.split_once('/') else{return false;};
    !subtype.contains('/') && main=="application" && (subtype=="json" || subtype.ends_with("+json"))
}
