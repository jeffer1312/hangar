use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::runtime::actor::PolicyClient;
use serde_json::json;

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() }

/// Casa isolada com a conta padrão (`~/.claude`) e uma cota lida há pouco no cache em disco.
fn home_with_cached_quota(expires_ms:f64) -> (tempfile::TempDir,AccountService,std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join(".credentials.json"),
        json!({"claudeAiOauth":{"accessToken":"t","expiresAt":expires_ms}}).to_string()).unwrap();
    // O mesmo caminho do catálogo: no Windows o `canonicalize` cru leva o prefixo `\\?\`.
    let id = format!("claude:{}",hangar_server::accounts::catalog::resolved(&claude).to_string_lossy());
    let service = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),home.path().to_string_lossy().into_owned()),
        ("USERPROFILE".into(),home.path().to_string_lossy().into_owned()),
    ].into()));
    // No Windows o cache mora em `AppData/Local`, não em `~/.hangar`.
    let cache = service.quota_path();
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(cache, json!({id:{"gravado_em":now(),"cota":{"estado":"lida",
        "janelas":[{"rotulo":"5h","pct":42,"reset_ts":null,"por_modelo":false},{"rotulo":"Fable","pct":9,"reset_ts":null,"por_modelo":true}]}}})
        .to_string()).unwrap();
    (home,service,claude)
}

#[tokio::test]
async fn status_line_windows_come_from_the_accounts_module() {
    let (_home,service,claude) = home_with_cached_quota((now() + 3600.0) * 1000.0);
    let client = PolicyClient::new().with_accounts(service);
    for config_dir in ["",claude.to_str().unwrap()] {
        let quota = client.quota_windows("key",config_dir).await.unwrap();
        assert_eq!(quota,json!({"windows":[{"rotulo":"5h","pct":42,"reset_ts":null,"por_modelo":false}]}),"{config_dir:?}");
    }
    assert!(client.quota_windows("key","/conta/inexistente").await.is_none());
}

#[tokio::test]
async fn changed_credential_drops_the_old_quota_and_an_expired_token_does_not_renew() {
    let (_home,service,claude) = home_with_cached_quota((now() + 3600.0) * 1000.0);
    let config_dir = claude.to_str().unwrap().to_owned();
    assert_eq!(service.claude_windows(&config_dir).await.unwrap().len(),1);
    // Login trocado e já vencido: a cota da identidade anterior sai, e sem ponte não há renovação.
    std::fs::write(claude.join(".credentials.json"),
        json!({"claudeAiOauth":{"accessToken":"outro","expiresAt":1000.0}}).to_string()).unwrap();
    assert_eq!(service.claude_windows(&config_dir).await,Err("quota_expired"));
}

#[tokio::test]
async fn without_the_accounts_module_the_line_has_no_windows() {
    let client = PolicyClient::new();
    assert!(client.quota_windows("key","").await.is_none());
}
