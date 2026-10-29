//! Contratos ENG-001–015: catálogo e escrita independem do Python.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn engines_normalization_matches_python() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/normalization.json")).unwrap();
    for (i, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let actual = match hangar_server::engines::store::normalize(case["name"].as_str().unwrap(), case["body"].as_object().unwrap()) {
            Ok(value) => json!({"ok":value}), Err(error) => json!({"error":error}),
        };
        assert_eq!(actual, case["expected"], "cenário {i}");
    }
}

#[test]
fn engines_environment_matches_python() {
    use hangar_server::engines::{environment::{derive, Selection}, store};
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/environment.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let service = AccountService::new(AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned()),("USERPROFILE".into(),home.path().to_string_lossy().into_owned())].into()));
    std::fs::create_dir_all(store::path(&service.env).parent().unwrap()).unwrap();
    for (i,case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        std::fs::write(store::path(&service.env),json!({"motor":case["record"]}).to_string()).unwrap();
        let actual = derive(&service,"motor",Selection { model:case["selected"].as_str(), ..Default::default() });
        // Arquivo manual que exportava valores não textuais passa a ser recusado explicitamente.
        if case["expected"]["ok"].as_object().is_some_and(|o|o.values().any(|v| !v.is_string())) { assert!(actual.is_err(),"{i}"); continue; }
        if case["expected"]["type"] == "TypeError" { assert!(actual.is_err(),"{i}"); continue; }
        let actual = match actual { Ok(v) => json!({"ok":v}), Err(e) => json!({"error":e,"type":"ValueError"}) };
        assert_eq!(actual,case["expected"],"cenário {i}");
    }
}

#[test]
fn engines_provider_models_match_python() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/models.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let result = match hangar_server::engines::probe::models(&case["data"]) { Ok(v) => json!({"ok":v}), Err(e) => json!({"error":e}) };
        assert_eq!(result,case["expected"]);
    }
}

#[tokio::test]
async fn engines_catalog_without_python() {
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(), home.path().to_string_lossy().into_owned()), ("USERPROFILE".into(), home.path().to_string_lossy().into_owned())].into()));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let response = reqwest::Client::new().get(format!("http://{address}/api/engines")).bearer_auth("fixture-owner").send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    task.abort();
    assert_eq!(status, 200, "catálogo próprio com o Python fora: {body}");
    assert_eq!(body, json!({"motores": {}, "arquivo_corrompido": false, "arquivo_caminho": home.path().join(".claude/engines.json")}));
}

#[test]
fn engines_mirrors_match_python() {
    use hangar_server::engines::mirrors;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/sync.json")).unwrap();
    for (i, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let env = AccountEnvironment::from_map([("HOME".into(),home.path().to_string_lossy().into_owned()),("USERPROFILE".into(),home.path().to_string_lossy().into_owned())].into());
        let folder = home.path().join(case["folder"].as_str().unwrap());
        std::fs::create_dir_all(&folder).unwrap();
        if let Some(initial) = case["initial"].as_str() { std::fs::write(folder.join(case["config"].as_str().unwrap()),initial).unwrap(); }
        let targets = vec![case["target"].as_str().unwrap().to_owned()];
        let normalize = |v: Value| -> Value { serde_json::from_str(&v.to_string().replace(&home.path().to_string_lossy().to_string(),"$ROOT")).unwrap() };
        let actual = mirrors::sync(&env,"motor",case["base"].as_str().unwrap(),"fixture-key",case["models"].as_array().unwrap(),&targets);
        assert_eq!(normalize(actual),case["expected"],"sincronizar {i}");
        let check = |expected: &Value| {
            for (name,content) in expected.as_object().unwrap() {
                let actual = std::fs::read_to_string(home.path().join(name)).unwrap();
                let expected = content.as_str().unwrap();
                match (serde_json::from_str::<Value>(&actual),serde_json::from_str::<Value>(expected)) {
                    (Ok(a),Ok(b)) => assert_eq!(a,b,"arquivo {name}, cenário {i}"),
                    _ => assert_eq!(actual,expected,"arquivo {name}, cenário {i}"),
                }
            }
        };
        check(&case["files"]);
        assert_eq!(normalize(mirrors::remove(&env,"motor",&targets)),case["removed"],"remover {i}");
        check(&case["after"]);
    }
}

#[test]
fn engines_proxy_management_uses_shared_home() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/proxy-management.json")).unwrap();
    let expected = &fixture["expected"];
    let root = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map([
        ("HOME".into(), root.path().to_string_lossy().into_owned()),
        ("USERPROFILE".into(), root.path().to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".into(), root.path().join("alternative").to_string_lossy().into_owned()),
    ].into());
    use hangar_server::engines::proxy;
    proxy::set_management_key(&env, " fixture-management ").unwrap();
    let path = root.path().join(expected["path"].as_str().unwrap());
    assert!(path.is_file(), "chave administrativa precisa ficar no diretório compartilhado");
    assert!(!env.claude_base.join("cliproxy-management.key").exists());
    assert_eq!(json!(proxy::management_key(&env)), expected["read"]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected["content"]);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(json!(path.metadata().unwrap().permissions().mode() & 0o777), expected["mode"]);
    }
    proxy::set_management_key(&env, "").unwrap();
    assert_eq!(json!(!path.exists()), expected["removed"]);
    assert_eq!(json!(proxy::management_key(&env)), expected["empty"]);
}

proptest::proptest! {
    #[test]
    fn engines_reject_misplaced_numeric_separators(digits in "[0-9]{1,40}") {
        for malformed in [format!("_{digits}"),format!("{digits}_"),format!("1__{digits}")] {
            let result = hangar_server::engines::store::positive("context_window", &json!(malformed));
            proptest::prop_assert!(result.is_err());
        }
    }
}

