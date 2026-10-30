use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts,
    config::Config, config_sync::{export::Context, paths::Roots}, routes::AppState};
use base64::Engine;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};

fn files(root: &Path, folder: &Path, output: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { files(root, &path, output); }
        else { output.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read_to_string(path).unwrap()); }
    }
}

#[tokio::test]
async fn config_apply_http_nonbusy_contract_and_effects() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/config_sync/apply-http.json")).unwrap();
    let payload = base64::engine::general_purpose::STANDARD.decode(fixture["payload_base64"].as_str().unwrap()).unwrap();
    let mut differences = Vec::new();
    let cases: Vec<_> = fixture["cases"].as_array().unwrap().iter().filter(|row| row["busy"] == false).collect();
    assert_eq!(cases.len(), 37, "os três casos de exclusão têm prova de concorrência própria");
    for (index, row) in cases.into_iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_string_lossy().into_owned();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config { listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
        state.accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(), root.clone()), ("USERPROFILE".into(), root.clone())].into()));
        state.config_sync_context = Some(Context::new(Roots { home: root.clone(), claude: format!("{root}/.claude"), codex: format!("{root}/.codex"), hangar: format!("{root}/repo") }));
        let router = hangar_server::routes::router(Arc::new(state));
        let server = tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
        let input = match row["input"].as_str().unwrap() { "valid" => payload.clone(), "empty" => vec![], _ => b"invalid".to_vec() };
        let response = reqwest::Client::builder().no_proxy().build().unwrap().request(row["method"].as_str().unwrap().parse().unwrap(), format!("http://{address}/api/config-sync/apply{}", row["query"].as_str().unwrap())).bearer_auth("fixture-owner").body(input).send().await.unwrap();
        let status = response.status().as_u16();
        let headers: BTreeMap<_,_> = ["allow", "content-type", "content-encoding", "cache-control"].into_iter().filter_map(|key| response.headers().get(key).map(|value| (key, value.to_str().unwrap().to_owned()))).collect();
        let raw = response.text().await.unwrap().replace(&root, "⟦HOME⟧");
        let body = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
        let mut disk = BTreeMap::new(); files(home.path(), home.path(), &mut disk);
        if status != row["status"].as_u64().unwrap() as u16 || body != row["body"] || json!(headers) != row["headers"] || json!(disk) != row["files"] {
            differences.push(format!("{index}: status={status}, body={body}, headers={headers:?}, files={disk:?}"));
        }
        server.abort();
        let _ = server.await;
    }
    assert!(differences.is_empty(), "{differences:?}");
}
