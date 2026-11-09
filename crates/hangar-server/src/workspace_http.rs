//! Contrato HTTP de Git e arquivos; o motor continua em hangar-workspace.
use axum::{http::{HeaderMap, Method}, response::{IntoResponse, Response}};
use serde_json::{Value, json};
use std::collections::HashMap;
use crate::orchestration::{body, codec};

pub(crate) const ROUTES: &[(&str, &str)] = &[
    ("/api/fs/roots","GET"), ("/api/fs/scan","GET"), ("/api/fs/mkdir","POST"),
    ("/api/fs/branches","GET"), ("/api/fs/git","GET"), ("/api/fs/git/fetch","POST"),
    ("/api/fs/git/pull","POST"), ("/api/fs/git/switch","POST"), ("/api/fs/git/branch","POST"),
    ("/api/sessions/{name}/branches","GET"), ("/api/sessions/{name}/checkout","POST"),
    ("/api/sessions/{name}/git","POST"), ("/api/sessions/{name}/git/files","GET"),
    ("/api/sessions/{name}/git/log","GET"), ("/api/sessions/{name}/git/last-message","GET"),
    ("/api/sessions/{name}/git/diff","POST"), ("/api/sessions/{name}/git/path-diff","POST"),
    ("/api/sessions/{name}/git/discard","POST"), ("/api/sessions/{name}/git/commit","POST"),
    ("/api/sessions/{name}/git/revert","POST"), ("/api/sessions/{name}/git/cherry-pick","POST"),
    ("/api/sessions/{name}/git/push","POST"), ("/api/sessions/{name}/git/reset","POST"),
    ("/api/sessions/{name}/git/branch","POST"), ("/api/sessions/{name}/git/tag","POST"),
    ("/api/sessions/{name}/git/commit/{sha}/files","GET"), ("/api/sessions/{name}/git/commit/{sha}/diff","GET"),
    ("/api/sessions/{name}/git/commit/{sha}/diff-full","GET"), ("/api/sessions/{name}/git/commit/{sha}/diff-worktree","GET"),
    ("/api/sessions/{name}/git/commit/{sha}/branches","GET"),
    ("/api/sessions/{name}/files/list","GET"), ("/api/sessions/{name}/files/read","GET"),
    ("/api/sessions/{name}/files/write","POST"), ("/api/sessions/{name}/files/search","GET"),
    ("/api/sessions/{name}/files/resolver","POST"), ("/api/sessions/{name}/file","GET"),
    ("/api/sessions/{name}/file/text","GET"), ("/api/sessions/{name}/file/text","POST"),
];

pub(crate) fn decoded(path: &str) -> std::borrow::Cow<'_, str> {
    percent_encoding::percent_decode_str(path).decode_utf8_lossy()
}
pub(crate) fn owns(path: &str) -> bool {
    let path = decoded(path);
    if path.starts_with("/api/fs/") { return true; }
    let Some((_, tail)) = path.strip_prefix("/api/sessions/").and_then(|p|p.split_once('/')) else { return false };
    matches!(tail.split('/').next().unwrap_or(""), "branches" | "checkout" | "git" | "files" | "file")
}
pub(crate) fn name_tail(path: &str) -> (Option<String>, String) {
    let path = decoded(path);
    match path.strip_prefix("/api/sessions/").and_then(|p|p.split_once('/')) {
        Some((name, tail)) => (Some(name.to_owned()), tail.to_owned()),
        None => (None, path.into_owned()),
    }
}

#[derive(Clone, Copy)]
enum Kind { Text, Name, NullableText, Bool, Texts, Enum(&'static [&'static str]) }
type Field = (&'static str, Kind, bool);
use Kind::{Text as T, Name as M, NullableText as N, Bool as B, Texts as L, Enum as E};
fn schema(tail: &str) -> Option<(&'static [Field], bool)> {
    Some(match tail {
        "checkout" => (&[("branch",T,true)][..], true),
        "git" => (&[("action",E(&["status","pull","fetch","stash","stash-pop","log","revert-abort","cherry-pick-abort"]),true)], true),
        "git/diff" | "git/discard" => (&[("path",T,true)], true),
        "git/path-diff" => (&[("path",T,true),("escopo",T,false)], true),
        "git/commit" => (&[("message",M,true),("paths",L,false),("amend",B,false),("new_branch",N,false)], true),
        "git/revert" | "git/cherry-pick" => (&[("sha",T,true)], true),
        "git/reset" => (&[("sha",T,true),("mode",E(&["soft","mixed","hard"]),true)], true),
        "git/branch" => (&[("name",T,true),("sha",N,false),("switch_after",B,false)], true),
        "git/tag" => (&[("name",T,true),("sha",N,false),("message",N,false)], true),
        "files/write" | "file/text" => (&[("path",T,true),("text",T,true),("digest",N,false)], true),
        "files/resolver" => (&[("caminhos",L,true)], true),
        "/api/fs/mkdir" => (&[("root",T,true),("path",N,false),("name",T,true)], false),
        "/api/fs/git/fetch" | "/api/fs/git/pull" => (&[("root",T,true),("path",N,false)], true),
        "/api/fs/git/switch" => (&[("root",T,true),("path",N,false),("branch",M,true),("confirm_sessions",B,false)], true),
        "/api/fs/git/branch" => (&[("root",T,true),("path",N,false),("name",M,true),("base",N,false),("checkout",B,false),("confirm_sessions",B,false)], true),
        _ => return None,
    })
}
fn error(kind: &str, loc: Value, input: &Value, ctx: Option<Value>) -> Value {
    let msg = match kind {
        "missing" => "Field required", "model_attributes_type" => "Input should be a valid dictionary or object to extract fields from",
        "string_type" => "Input should be a valid string", "string_too_short" => "String should have at least 1 character",
        "list_type" => "Input should be a valid list", "bool_type" => "Input should be a valid boolean",
        "bool_parsing" => "Input should be a valid boolean, unable to interpret input", "extra_forbidden" => "Extra inputs are not permitted",
        "int_parsing" => "Input should be a valid integer, unable to parse string as an integer", _ => "",
    };
    let mut out = json!({"type":kind,"loc":loc,"msg":msg,"input":codec::clone_value(input)});
    if let Some(ctx) = ctx { out["ctx"] = ctx; }
    out
}
fn invalid(errors: Vec<Value>) -> Response {
    let value = json!({"detail":errors});
    if codec::contains_nonfinite(&value) || codec::contains_surrogate(&value) {
        codec::discard(value);
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    }
    codec::response(axum::http::StatusCode::UNPROCESSABLE_ENTITY, value)
}
fn native(value: &Value, fields: &[Field]) -> bool {
    let Some(object) = value.as_object() else { return false };
    object.keys().all(|key| fields.iter().any(|(name,_,_)|key == name)) && fields.iter().all(|(name,kind,required)| {
        let Some(v) = object.get(*name) else { return !required };
        if codec::contains_nonfinite(v) { return false; }
        match kind {
            T => v.is_string(), M => v.as_str().is_some_and(|s|!s.is_empty()), N => v.is_null() || v.is_string(),
            B => v.is_boolean(), L => v.as_array().is_some_and(|a|a.iter().all(Value::is_string)),
            E(values) => v.as_str().is_some_and(|s|values.contains(&s)),
        }
    })
}

pub(crate) fn frame(tail: &str, method: &Method, headers: &HeaderMap, bytes: &[u8]) -> Result<Value, Response> {
    let Some((fields,_)) = schema(tail).filter(|_| *method == Method::POST) else { return Ok(json!({})); };
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        if native(&value, fields) { return Ok(value); }
        codec::discard(value);
    }
    // O caminho nativo já publicado aceita o objeto tipado sem depender do MIME.
    if !headers.contains_key("content-type") || crate::http_body_input::is_json(headers) {
        body::parse(bytes).map_err(|(status,value)|codec::response(status,value))
    } else { Ok(if bytes.is_empty() { Value::Null } else { Value::String(String::from_utf8_lossy(bytes).into_owned()) }) }
}

pub(crate) fn payload(tail: &str, method: &Method, value: &Value) -> Result<Value, Response> {
    let Some((fields,strict)) = schema(tail).filter(|_| *method == Method::POST) else { return Ok(json!({})); };
    let Some(input) = value.as_object() else {
        return Err(invalid(vec![error(if value.is_null(){"missing"}else{"model_attributes_type"},json!(["body"]),value,None)]));
    };
    let mut out = serde_json::Map::new();
    let mut errors = Vec::new();
    for (name,kind,required) in fields {
        let loc = json!(["body",name]);
        let Some(v) = input.get(*name) else { if *required { errors.push(error("missing",loc,value,None)); } continue; };
        let finite = !codec::contains_nonfinite(v);
        let mut converted = None;
        match kind {
            T | M | N => {
                if !(v.is_string() && finite || matches!(kind,N) && v.is_null()) { errors.push(error("string_type",loc,v,None)); }
                else if matches!(kind,M) && v.as_str().is_some_and(str::is_empty) { errors.push(error("string_too_short",loc,v,Some(json!({"min_length":1})))); }
            }
            B => match crate::value_coercion::boolean(v).filter(|_|finite) {
                Some(b) => converted = Some(json!(b)),
                None => errors.push(error(if v.is_string(){"bool_parsing"}else{"bool_type"},loc,v,None)),
            },
            L => match v.as_array() {
                Some(items) => for (index,item) in items.iter().enumerate() {
                    if !item.is_string() || codec::contains_nonfinite(item) { errors.push(error("string_type",json!(["body",name,index]),item,None)); }
                },
                None => errors.push(error("list_type",loc,v,None)),
            },
            E(choices) => if !v.as_str().is_some_and(|s|finite && choices.contains(&s)) {
                let quoted: Vec<_> = choices.iter().map(|choice|format!("'{choice}'")).collect();
                let expected = format!("{} or {}", quoted[..quoted.len()-1].join(", "), quoted.last().unwrap());
                errors.push(json!({"type":"literal_error","loc":loc,"msg":format!("Input should be {expected}"),
                    "input":codec::clone_value(v),"ctx":{"expected":expected}}));
            },
        }
        out.insert((*name).into(), converted.unwrap_or_else(||codec::clone_value(v)));
    }
    if strict { for (name,v) in input { if !fields.iter().any(|(field,_,_)|*field == name) {
        errors.push(error("extra_forbidden",json!(["body",name]),v,None));
    } } }
    if errors.is_empty() { Ok(Value::Object(out)) } else { codec::discard(Value::Object(out)); Err(invalid(errors)) }
}

pub(crate) fn query(tail: &str, method: &Method, raw: Option<&str>) -> Result<HashMap<String,String>, Response> {
    let mut q: HashMap<String,String> = form_urlencoded::parse(raw.unwrap_or("").as_bytes()).into_owned().collect();
    let mut errors = Vec::new();
    let required = match tail {
        "files/read" | "file" => Some("path"), "file/text" if *method == Method::GET => Some("path"),
        "files/search" => Some("q"),
        "/api/fs/scan" | "/api/fs/branches" | "/api/fs/git" => Some("root"),
        t if t.starts_with("git/commit/") && t.ends_with("/diff") => Some("path"), _ => None,
    };
    if let Some(key) = required.filter(|key|!q.contains_key(*key)) { errors.push(error("missing",json!(["query",key]),&Value::Null,None)); }
    if tail == "git/log" { if let Some(raw) = q.get("n") {
        match crate::value_coercion::integer(&json!(raw)) {
            Some(n) => { let n = n.as_i64().unwrap_or_else(|| if n.to_string().starts_with('-') {1}else{2000}).clamp(1,2000); q.insert("n".into(),n.to_string()); }
            None => errors.push(error("int_parsing",json!(["query","n"]),&json!(raw),None)),
        }
    } }
    let bools: &[&str] = match tail { "files/list" => &["so_modificados"], "file" => &["download"], _ => &[] };
    for key in bools { if let Some(raw) = q.get(*key) { match crate::value_coercion::boolean(&json!(raw)) {
        Some(value) => { q.insert((*key).into(), value.to_string()); }
        None => errors.push(error("bool_parsing",json!(["query",key]),&json!(raw),None)),
    } } }
    if errors.is_empty() { Ok(q) } else { Err(invalid(errors)) }
}
