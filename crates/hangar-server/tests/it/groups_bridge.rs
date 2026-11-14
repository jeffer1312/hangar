//! A ponte privada dos grupos (`/__hangar_server/groups`) saiu com o último chamador do Python: a porta
//! privada não a tem mais, nem com o segredo, e os grupos seguem do Rust (`health.groups`).
#![cfg(unix)]
use crate::fake;

use std::net::SocketAddr;
use std::sync::Arc;

use fake::{SECRET, client};
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::GroupService;
use hangar_server::groups::store::PairDir;
use hangar_server::routes::{AppState, router, terminal_router};
use serde_json::{Value, json};

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    addr
}

/// R4 (contrato c): nenhuma operação da ponte responde (`group.route`, `group.leave`, `group.orq_associate`).
#[tokio::test(flavor = "multi_thread")]
async fn r4_16_the_private_group_bridge_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (_python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    let pair = dir.path().join("pair");
    state.groups = Some(Arc::new(GroupService::new(PairDir::new(pair.clone(), dir.path().join("arquivo")),
        Arc::new(PythonOrq::from_state(&state).with_paths(dir.path().to_owned(), pair.clone())), "casa".into())));
    let state = Arc::new(state);
    let (public, private) = (serve(router(state.clone())).await, serve(terminal_router(state)).await);
    for (op, args) in [("group.route", json!({"method": "POST", "name": "s0", "route": "pair", "body": {"peer": "s1"}, "facts": {}})),
        ("group.leave", json!({"name": "s0", "phases": {}})), ("group.orq_associate", json!({"name": "p", "gid": "g", "mtime": 1.0}))] {
        let status = client().post(format!("http://{private}/__hangar_server/groups")).header("content-type", "application/json")
            .header("x-hangar-internal", SECRET).body(json!({"op": op, "args": args}).to_string()).send().await.unwrap().status().as_u16();
        assert_eq!(status, 404, "{op}");
    }
    assert!(!pair.join("s0.json").exists(), "nada pareado pela ponte");
    let health: Value = serde_json::from_str(&client().get(format!("http://{public}/__hangar_server/health")).send().await.unwrap()
        .text().await.unwrap()).unwrap();
    assert_eq!(health["groups"], true);
}
