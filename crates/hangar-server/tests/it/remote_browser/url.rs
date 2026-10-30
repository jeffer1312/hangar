#![cfg(unix)]
use super::support::{self, Scene, OWNER};
use serde_json::json;

#[tokio::test]
async fn reads_sidecar_without_python() {
    if support::isolated("remote_browser::url::reads_sidecar_without_python", |root| {
        assert!(!root.join("git.calls").exists(), "GET iniciou Git");
    }).await { return; }
    let scene = Scene::new().await;
    std::fs::write(scene.nav_dir().join("native.json"), json!({"chave":"server::missing-session", "url":"https://example.invalid/", "targetId":null,"pid":99999999}).to_string()).unwrap();
    scene.upstream.server.close().await;
    let answer = reqwest::Client::new().get(format!("http://{}/api/sessions/missing-session/navegador", scene.owner.address))
        .bearer_auth(OWNER).send().await.unwrap();
    let status = answer.status().as_u16();
    let body = answer.text().await.unwrap();
    scene.owner.close().await;
    scene.guest.close().await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap(), json!({"url":"https://example.invalid/"}));
}

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, HeaderMap, HeaderName, HeaderValue};
use hangar_server::routes::{router, terminal_router};
use serde_json::Value;
use tower_service::Service;

fn unhex(raw: &str) -> Vec<u8> {
    raw.as_bytes().chunks_exact(2).map(|x| u8::from_str_radix(std::str::from_utf8(x).unwrap(),16).unwrap()).collect()
}

fn reset_files(dir: &Path, input: &Value, snapshot: &Value) {
    if dir.exists() { std::fs::remove_dir_all(dir).unwrap(); }
    let Some(files) = input.as_object() else { return };
    std::fs::create_dir_all(dir).unwrap();
    for name in files.keys() {
        let name = name.trim_end_matches('/');
        let state = &snapshot[name];
        if state["directory"] == true { std::fs::create_dir(dir.join(name)).unwrap(); }
        else { std::fs::write(dir.join(name), unhex(state["bytes_hex"].as_str().unwrap())).unwrap(); }
    }
}

fn snapshot(dir: &Path) -> BTreeMap<String, Value> {
    if !dir.is_dir() { return BTreeMap::new(); }
    std::fs::read_dir(dir).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        let value = if path.is_dir() { json!({"directory":true}) }
            else { json!({"bytes_hex":std::fs::read(&path).unwrap().iter().map(|b|format!("{b:02x}")).collect::<String>()}) };
        (path.file_name().unwrap().to_str().unwrap().to_owned(), value)
    }).collect()
}

async fn get(scene: &Scene, name: &str, headers: HeaderMap, query: &str, ip: &str, https: bool) -> (u16, HeaderMap, String) {
    let name = form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+',"%20");
    let mut request = Request::builder().uri(format!("/api/sessions/{name}/navegador?{query}"))
        .extension(ConnectInfo("127.0.0.1:12345".parse::<SocketAddr>().unwrap())).body(Body::empty()).unwrap();
    *request.headers_mut() = headers;
    request.headers_mut().insert("x-forwarded-for", HeaderValue::from_str(ip).unwrap());
    request.headers_mut().insert("x-forwarded-proto", HeaderValue::from_static(if https {"https"} else {"http"}));
    let response = router(scene.state.clone()).call(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let raw = String::from_utf8(to_bytes(response.into_body(),1024*1024).await.unwrap().to_vec()).unwrap();
    (status, headers, raw)
}

fn owner_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_str(&format!("Bearer {OWNER}")).unwrap());
    headers
}

#[tokio::test]
async fn head_keeps_the_method_refusal() {
    if support::isolated("remote_browser::url::head_keeps_the_method_refusal", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let scene = Scene::new().await;
    std::fs::write(scene.nav_dir().join("native.json"),json!({"chave":"session","url":"https://example.invalid/"}).to_string()).unwrap();
    let before = snapshot(&scene.nav_dir());
    let want = json!({"status": 405, "body": "", "headers": {"allow": "GET", "content-type": "application/json", "content-length": "31"}});
    let client = reqwest::Client::new();
    let mut differences = Vec::new();
    for token in [None,Some(OWNER)] {
        let request = client.head(format!("http://{}/api/sessions/session/navegador",scene.owner.address));
        let response = if let Some(token) = token { request.bearer_auth(token) } else { request }.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap();
        if json!(status) != want["status"] || body != want["body"].as_str().unwrap() {
            differences.push(format!("HEAD com credencial={}: ({status},{body:?}) != ({},{})",token.is_some(),want["status"],want["body"]));
        }
        for (key,value) in want["headers"].as_object().unwrap() {
            if headers.get(key).and_then(|value|value.to_str().ok()) != value.as_str() {
                differences.push(format!("HEAD com credencial={}: header {key}: {:?} != {value}",token.is_some(),headers.get(key)));
            }
        }
    }
    assert_eq!(snapshot(&scene.nav_dir()),before);
    let paths: Vec<_> = scene.upstream.paths().into_iter().filter(|path|path.ends_with("/navegador")).collect();
    if paths != ["/api/sessions/session/navegador","/api/sessions/session/navegador"] {
        differences.push(format!("HEAD não seguiu ao encaminhamento existente: {paths:?}"));
    }
    scene.close().await;
    assert!(differences.is_empty(),"{}",differences.join("\n"));
}

// Quebra se o handler mudar seleção, recarga, URL de topo ou autenticação dos casos congelados.
#[tokio::test]
async fn frozen_get_cases_match_status_json_headers_and_files() {
    if support::isolated("remote_browser::url::frozen_get_cases_match_status_json_headers_and_files", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let scene = Scene::new().await;
    let corpus = support::contract("url-commands");
    let mut count = 0;
    let mut differences = Vec::new();
    for case in corpus["cases"].as_array().unwrap().iter().filter(|c| {
        let name = c["name"].as_str().unwrap();
        name.starts_with("get_") && name != "get_signed_share_reaches_handler" && name != "get_login_guest_is_blocked"
    }) {
        let input = &case["inputs"];
        let dir = scene.nav_dir();
        reset_files(&dir, &input["files"], &case["expected"]["files_before"]);
        assert_eq!(json!(snapshot(&dir)),case["expected"]["files_before"],"{} antes",case["name"]);
        scene.state.auth.clear_failures(input["client_host"].as_str().unwrap());
        let mut headers = HeaderMap::new();
        for (key,value) in input["headers"].as_object().unwrap() {
            headers.insert(HeaderName::from_bytes(key.as_bytes()).unwrap(),HeaderValue::from_str(&value.as_str().unwrap().replace("contract-owner-token",OWNER)).unwrap());
        }
        let query = form_urlencoded::Serializer::new(String::new()).extend_pairs(input["query"].as_object().unwrap().iter()
            .map(|(k,v)|(k.clone(),v.as_str().unwrap().replace("contract-owner-token",OWNER)))).finish();
        for (index,want) in case["expected"]["responses"].as_array().unwrap().iter().enumerate() {
            if index > 0 && !input["after_files"].is_null() {
                reset_files(&dir,&input["after_files"],&case["expected"]["files_after"]);
            }
            let (status, got_headers, raw) = get(&scene,input["session"].as_str().unwrap(),headers.clone(),&query,
                input["client_host"].as_str().unwrap(),input["scheme"]=="https").await;
            if json!(status) != want["status"] {
                differences.push(format!("{} #{index}: status {status} != {}: {raw}",case["name"],want["status"]));
            }
            if serde_json::from_str::<Value>(&raw).ok().as_ref() != Some(&want["body"]) {
                differences.push(format!("{} #{index}: corpo {raw:?} != {}",case["name"],want["body"]));
            }
            for (key,value) in want["headers"].as_object().unwrap() {
                if got_headers.get(key).and_then(|v|v.to_str().ok()) != value.as_str() {
                    differences.push(format!("{} #{index}: header {key}: {:?} != {value}",case["name"],got_headers.get(key)));
                }
            }
        }
        assert_eq!(json!(snapshot(&dir)),case["expected"]["files_after"],"{} depois",case["name"]);
        count += 1;
    }
    assert_eq!(count,35);
    for path in scene.upstream.paths() {
        if path.contains("/navegador") || path.ends_with("/info") || path.contains("/internal/list/facts") {
            differences.push(format!("consulta indevida ao Python: {path}"));
        }
    }
    scene.close().await;
    assert!(differences.is_empty(),"{} divergências nos {count} casos:\n{}",differences.len(), differences.join("\n"));
}

#[tokio::test]
async fn forwarded_matches_public_without_auth_or_python_and_decodes_once() {
    if support::isolated("remote_browser::url::forwarded_matches_public_without_auth_or_python_and_decodes_once", |_| {}).await { return; }
    let scene = Scene::new().await;
    for name in ["session", "sessão %20 🚀"] {
        std::fs::write(scene.nav_dir().join("native.json"),json!({"chave":name,"url":"https://example.invalid/página?q=ação#parte"}).to_string()).unwrap();
        let (_,_,public) = get(&scene,name,owner_headers(),"","127.0.0.1",false).await;
        let escaped = form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+',"%20");
        let request = Request::builder().uri("/__hangar_server/forwarded")
            .header("x-hangar-internal",support::SECRET).header("x-hangar-path",format!("/api/sessions/{escaped}/navegador"))
            .extension(ConnectInfo("127.0.0.1:12345".parse::<SocketAddr>().unwrap())).body(Body::empty()).unwrap();
        let response = terminal_router(scene.state.clone()).call(request).await.unwrap();
        assert_eq!(response.status().as_u16(),200);
        assert_eq!(to_bytes(response.into_body(),1024*1024).await.unwrap(), public);
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[derive(Clone)]
struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Log {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(data); Ok(data.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Self;
    fn make_writer(&'a self) -> Self { self.clone() }
}

#[tokio::test]
async fn unreadable_sidecars_warn_without_stopping_or_writing() {
    use tracing::instrument::WithSubscriber;
    if support::isolated("remote_browser::url::unreadable_sidecars_warn_without_stopping_or_writing", |_| {}).await { return; }
    let scene = Scene::new().await;
    std::fs::write(scene.nav_dir().join("broken.json"),b"{").unwrap();
    std::fs::write(scene.nav_dir().join("bytes.json"),[255]).unwrap();
    std::fs::create_dir(scene.nav_dir().join("directory.json")).unwrap();
    std::fs::write(scene.nav_dir().join(".hidden.json"),json!({"chave":"other","url":"u"}).to_string()).unwrap();
    let before = snapshot(&scene.nav_dir());
    let log = Log(Default::default());
    let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false).with_max_level(tracing::Level::WARN).with_writer(log.clone()).finish();
    let (status,_,raw) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).with_subscriber(subscriber).await;
    assert_eq!(status,200);
    assert_eq!(raw,r#"{"url":null}"#);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    for file in ["broken.json","bytes.json","directory.json"] {
        assert!(text.contains(file),"sem warning de {file}: {text}");
    }
    assert_eq!(text.lines().filter(|line|line.contains("WARN")).count(),3,"{text}");
    assert_eq!(snapshot(&scene.nav_dir()),before);
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn sidecars_and_url_values_never_panic_or_consult_python() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};
    if support::isolated("remote_browser::url::sidecars_and_url_values_never_panic_or_consult_python", |_| {}).await { return; }
    let scene = Scene::new().await;
    let (_, private, status, body) = EXTRA_SIDECARS.iter().find(|case|case.0=="url_private_unicode").unwrap();
    std::fs::write(scene.nav_dir().join("arbitrary.json"),private).unwrap();
    let (got,_,raw) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).await;
    assert_eq!(got,*status,"{raw}");
    assert_eq!(raw,*body);
    let strategy = (proptest::collection::vec(any::<u8>(),0..512), ".{0,80}");
    let mut runner = TestRunner::new(Config { cases:64,..Config::default() });
    for _ in 0..64 {
        use proptest::strategy::ValueTree;
        let (bytes,url) = strategy.new_tree(&mut runner).unwrap().current();
        std::fs::write(scene.nav_dir().join("arbitrary.json"),bytes).unwrap();
        let (status,_,_) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).await;
        assert!([200,500].contains(&status));
        std::fs::write(scene.nav_dir().join("arbitrary.json"),json!({"chave":"session","url":url}).to_string()).unwrap();
        let (status,_,raw) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).await;
        assert_eq!(status,200,"URL {url:?}: {raw}");
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(),json!({"url":url}));
    }
    std::fs::write(scene.nav_dir().join("arbitrary.json"),br#"{"chave":"session","url":{"big":123456789012345678901234567890,"float":1.0,"order":{"z":1,"a":2}}}"#).unwrap();
    let (status,_,raw) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).await;
    assert_eq!(status,200);
    assert_eq!(raw,r#"{"url":{"big":123456789012345678901234567890,"float":1.0,"order":{"z":1,"a":2}}}"#);
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

/// (caso, bytes do sidecar, status, corpo exato): o tipo e o texto do número passam intactos.
const EXTRA_SIDECARS: [(&str, &str, u16, &str); 13] = [
    ("url_bigint_float_order", r#"{"chave":"session","url":{"big":123456789012345678901234567890,"float":1.0,"order":{"z":1,"a":2}}}"#, 200, r#"{"url":{"big":123456789012345678901234567890,"float":1.0,"order":{"z":1,"a":2}}}"#),
    ("url_lone_surrogate", r#"{"chave":"session","url":"\ud800"}"#, 500, "Internal Server Error"),
    ("key_lone_surrogate_suffix", r#"{"chave":"\ud800::session","url":"u"}"#, 200, r#"{"url":"u"}"#),
    ("url_nan", r#"{"chave":"session","url":NaN}"#, 500, "Internal Server Error"),
    ("unused_nan", r#"{"chave":"session","url":"u","unused":NaN}"#, 200, r#"{"url":"u"}"#),
    ("url_infinite_float", r#"{"chave":"session","url":1e999}"#, 500, "Internal Server Error"),
    ("key_null", r#"{"chave":null,"url":"u"}"#, 500, "Internal Server Error"),
    ("key_number", r#"{"chave":12,"url":"u"}"#, 500, "Internal Server Error"),
    ("key_nan", r#"{"chave":NaN,"url":"u"}"#, 500, "Internal Server Error"),
    ("json_bom", "\u{feff}{\"chave\":\"session\",\"url\":\"u\"}", 200, r#"{"url":null}"#),
    ("url_float_rounding", r#"{"chave":"session","url":0.12345678901234567890123456789}"#, 200, r#"{"url":0.12345678901234568}"#),
    ("url_float_exponent", r#"{"chave":"session","url":1e-7}"#, 200, r#"{"url":1e-07}"#),
    ("url_private_unicode", "{\"chave\":\"session\",\"url\":\"\u{10f800}\"}", 200, "{\"url\":\"\u{10f800}\"}"),
];

#[tokio::test]
async fn extra_sidecar_types_keep_literal_http_result() {
    if support::isolated("remote_browser::url::extra_sidecar_types_keep_literal_http_result", |_| {}).await { return; }
    let scene = Scene::new().await;
    let mut differences = Vec::new();
    for (name, sidecar, want, body) in EXTRA_SIDECARS {
        std::fs::write(scene.nav_dir().join("extra.json"),sidecar).unwrap();
        let (status,_,raw) = get(&scene,"session",owner_headers(),"","127.0.0.1",false).await;
        if status != want || raw != body {
            differences.push(format!("{name}: ({status},{raw:?}) != ({want},{body})"));
        }
    }
    scene.close().await;
    assert!(differences.is_empty(),"{}",differences.join("\n"));
}
