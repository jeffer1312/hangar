//! Binário próprio: o teste esvazia o PATH do processo para o `git` não iniciar.
#[path = "it/fake/mod.rs"]
mod fake;
#[path = "it/workspace_fixture/mod.rs"]
mod workspace_fixture;
use fake::{OWNER, client};
use workspace_fixture::refusal;

#[tokio::test]
async fn unavailable_answers_503_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    // SAFETY: único teste deste binário; nenhuma outra thread lê o ambiente ainda.
    unsafe { std::env::set_var("PATH", empty.path()) };
    let f = workspace_fixture::fixture(dir.path(), true).await;
    let response = client()
        .get(format!("http://{}/api/sessions/fixture/branches", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    let body = refusal(response, "workspace_unavailable").await;
    assert!(body["detail"]["params"]["motivo"].as_str().is_some_and(|m| m.contains("git")), "{body}");
    assert_eq!(f.hits(), 0, "o Python não pode atender");
    assert_eq!(f.diag("rust.workspace_failed").await["codigo"], "workspace_unavailable");
}
