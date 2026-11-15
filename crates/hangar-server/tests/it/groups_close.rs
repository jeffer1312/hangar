//! Fechar pelo dono uma sessão que o próprio Rust encerra: a saída do grupo que falha vai no `warning`
//! da resposta, como no fechamento que o Python fazia, sem passar por ele.
#![cfg(unix)]
use std::collections::BTreeMap;
use std::sync::Arc;

use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::{GroupService, JoinOwned};
use hangar_server::groups::store::PairDir;
use serde_json::Value;

use crate::fake::{self, OWNER};
use crate::runtime_codex_watch::{finish, fixture, tui};
use crate::runtime_open::{app_server_process, fake_app_server, registry};

#[tokio::test]
async fn closing_a_rust_owned_grouped_session_returns_the_leave_warning() {
    let fx = fixture();
    let app_server = fake_app_server(true).await;
    let (mut app, mut tui) = (app_server_process(&app_server.endpoint), tui());
    fx.put_sidecar("cx-proof", fx.meta("cx-proof", &app_server.endpoint, Some(app.id()), &tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));

    let dir = tempfile::Builder::new().prefix("grp-sf-close-").tempdir().unwrap();
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let groups = Arc::new(GroupService::new(PairDir::new(dir.path().join("pair"), dir.path().join("arquivo")),
        Arc::new(PythonOrq::from_state(&state)), "casa".into()));
    // Par em outra máquina que não está entre os peers: o aviso de saída não sai.
    let harness: BTreeMap<String, String> = [("cx-proof", "codex"), ("longe::x", "claude")].into_iter().map(|(n, h)| (n.into(), h.into())).collect();
    groups.join(JoinOwned { name: "cx-proof".into(), others: vec!["longe::x".into()], task: String::new(), replace_task: false,
        harness, orq: false, outside: Default::default(), linked: false }).await.ok().unwrap();
    state.groups = Some(groups);
    let server = fake::spawn_state(state).await;

    let response = fake::client().delete(format!("http://{server}/api/sessions/cx-proof")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null);
    assert!(!registry.owns_codex_terminal("cx-proof"), "fechada no Rust");
    assert_eq!(python.hits_to("/api/sessions/cx-proof"), 0, "o fechamento inteiro é do Rust");
    assert_eq!(body["warning"]["code"], "erro_pareamento_saida_falhou", "{body}");
    assert_eq!(body["warning"]["params"]["avisos"][0]["erro"]["code"], "erro_peer_nao_avisado", "{body}");
    finish(&mut [&mut app, &mut tui]);
}
