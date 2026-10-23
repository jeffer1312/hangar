//! Ponte privada dos convites (`/__hangar_server/share`) e as rotas privadas do dono para o
//! Connect: depois da virada o Python só pede, o Rust é quem grava `shares.json` e
//! `external_pairs.json`. Cada op muda a loja como a função Python que ela substitui.
#![cfg(unix)]
use crate::fake;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use fake::{OWNER, SECRET, client};
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use hangar_server::list::facts;
use hangar_server::routes::{AppState, router, terminal_router};
use hangar_server::share::ShareState;
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives, Verdict, verdict};
use hangar_server::share::pairs::{ExternalPair, PairStore};
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::Real;
use serde_json::{Value, json};

/// Vida de cada sessão pela chave do sidecar; sem chave, o tmux diz que não há.
#[derive(Default)]
struct Probe { keys: Mutex<HashMap<String, String>> }

impl LifeProbe for Probe {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { None }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(false) }) }
    fn sidecar(&self, name: &str) -> Option<(String, bool)> { self.keys.lock().unwrap().get(name).cloned().map(|k| (k, false)) }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

struct Srv { public: SocketAddr, private: SocketAddr, python: Arc<fake::Fake>, share: Arc<ShareState>, probe: Arc<Probe> }

async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    addr
}

async fn server(dir: &Path, with_share: bool) -> Srv {
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, ""));
    let probe = Arc::new(Probe::default());
    let share = Arc::new(ShareState {
        store: ShareStore::open(dir.join("shares.json"), Arc::new(|| 100.0)),
        pairs: PairStore::open(dir.join("external_pairs.json")),
        lives: Lives::new(probe.clone(), Arc::new(LifeFacts::default())),
        tailscale: Arc::new(Real),
        tunnel: Default::default(),
    });
    if with_share { state.share = Some(share.clone()); }
    let state = Arc::new(state);
    let public = serve(router(state.clone())).await;
    let private = serve(terminal_router(state)).await;
    Srv { public, private, python, share, probe }
}

async fn bridge(addr: SocketAddr, op: &str, args: Value, secret: Option<&str>) -> (u16, Value) {
    let mut req = client().post(format!("http://{addr}/__hangar_server/share")).header("content-type", "application/json")
        .body(json!({"op": op, "args": args}).to_string());
    if let Some(secret) = secret { req = req.header("x-hangar-internal", secret); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
}

async fn result(srv: &Srv, op: &str, args: Value) -> Value {
    let (status, body) = bridge(srv.private, op, args, Some(SECRET)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{op}: {body}");
    body["result"].clone()
}

async fn json_of(resp: reqwest::Response) -> Value {
    serde_json::from_str(&resp.text().await.unwrap()).unwrap()
}

fn pair_record(share_id: &str, local: &str) -> ExternalPair {
    ExternalPair { share_id: share_id.into(), local_session: local.into(), alias: "ana".into(), peer_owner: "ana".into(),
        peer_session: "b".into(), peer_address: "https://127.0.0.1:1".into(), peer_token: "t".repeat(32), created_at: 1.0 }
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_needs_the_secret_and_the_private_port() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    assert_eq!(bridge(srv.private, "active_sessions", json!({}), None).await.0, 404);
    assert_eq!(bridge(srv.private, "active_sessions", json!({}), Some("outro")).await.0, 404);
    assert_eq!(bridge(srv.public, "active_sessions", json!({}), Some(SECRET)).await.0, 404, "a porta pública não tem a ponte");
    assert_eq!(bridge(srv.private, "nada", json!({}), Some(SECRET)).await.0, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_share_state_the_bridge_says_off() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), false).await;
    let (status, body) = bridge(srv.private, "active_sessions", json!({}), Some(SECRET)).await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "share_bridge_off");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_session_revokes_every_kind_like_kill_session() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    srv.share.store.create("s", Some("k:A".into()), Kind::Share, false).unwrap();
    srv.share.store.create("s", Some("k:A".into()), Kind::Pair, false).unwrap();
    srv.share.store.create("t", Some("k:T".into()), Kind::Share, false).unwrap();
    assert_eq!(result(&srv, "revoke_session", json!({"name": "s"})).await, json!({"revoked": 2}));
    let all = srv.share.store.all();
    assert!(all.iter().filter(|s| s.session == "s").all(|s| s.revoked_at.is_some()));
    assert!(all.iter().filter(|s| s.session == "t").all(|s| s.revoked_at.is_none()));
    assert_eq!(result(&srv, "active_sessions", json!({})).await, json!({"sessions": ["t"]}));
}

#[tokio::test(flavor = "multi_thread")]
async fn set_life_moves_the_invite_to_the_new_life() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    srv.share.store.create("s", Some("k:A".into()), Kind::Share, false).unwrap();
    result(&srv, "set_life", json!({"name": "s", "life": "k:B"})).await;
    assert_eq!(srv.share.store.list_for("s")[0].life.as_deref(), Some("k:B"));
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_releases_the_verdict_and_mode_change_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    let (_, token) = srv.share.store.create_redeemed("s", Some("k:A".into()), Kind::Share).unwrap();
    srv.probe.keys.lock().unwrap().insert("s".into(), "B".into());
    let guest = || srv.share.store.lookup_token(&token).unwrap();
    // Sem a fotografia do Python, vida diferente pode ser transferência: "não sei", nunca 410.
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Unsure);
    result(&srv, "facts_snapshot", json!({"lives": {}, "changing": ["s"]})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Unsure, "trocando de modo");
    result(&srv, "mode_change_end", json!({"name": "s"})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Ended, "outra vida de verdade");
    result(&srv, "mode_change_begin", json!({"name": "s"})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Unsure);
    result(&srv, "mode_change_end", json!({"name": "s"})).await;
    // Transferência em curso quando o Rust subiu (terminal e sidecar sumidos): a vida de origem vale.
    srv.probe.keys.lock().unwrap().clear();
    result(&srv, "facts_snapshot", json!({"lives": {"s": "k:A"}, "changing": []})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Ok);
    result(&srv, "mode_change_end", json!({"name": "s"})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Ended, "o fim da troca solta a vida de origem");
}

#[tokio::test(flavor = "multi_thread")]
async fn mode_change_end_keeps_the_transfer_still_incomplete() {
    // Restauração que falhou: a transferência segue incompleta depois da troca de modo.
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    let (_, token) = srv.share.store.create_redeemed("s", Some("k:A".into()), Kind::Share).unwrap();
    let guest = || srv.share.store.lookup_token(&token).unwrap();
    result(&srv, "facts_snapshot", json!({"lives": {"s": "k:A"}, "changing": []})).await;
    result(&srv, "mode_change_begin", json!({"name": "s"})).await;
    result(&srv, "mode_change_end", json!({"name": "s", "source_life": "k:A"})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Ok);
    result(&srv, "mode_change_end", json!({"name": "s", "source_life": null})).await;
    assert_eq!(verdict(&guest(), Some("s"), &srv.share.lives).await, Verdict::Ended);
}

#[tokio::test(flavor = "multi_thread")]
async fn session_died_removes_the_external_pair_and_its_invite() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    let (s, _) = srv.share.store.create("s", Some("k:A".into()), Kind::Pair, false).unwrap();
    srv.share.pairs.add(pair_record(&s.id, "s")).unwrap();
    result(&srv, "session_died", json!({"name": "s"})).await;
    assert!(srv.share.pairs.all().is_empty());
    assert!(srv.share.store.all()[0].revoked_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn end_pair_of_who_has_no_external_pair() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    assert_eq!(result(&srv, "end_pair", json!({"local": "s", "address": "ana::b"})).await, json!({"found": false, "errors": []}));
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_reaches_owner_invites_only_by_the_explicit_private_routes() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    srv.share.store.create("s", Some("k:A".into()), Kind::Share, false).unwrap();
    let url = format!("http://{}/__hangar_server/share/owner/list/s", srv.private);
    assert_eq!(client().post(&url).send().await.unwrap().status(), 404, "sem segredo");
    let resp = client().post(&url).header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = json_of(resp).await;
    assert_eq!(body["shares"].as_array().unwrap().len(), 1);
    let public = format!("http://{}/__hangar_server/share/owner/list/s", srv.public);
    assert_eq!(client().post(&public).header("x-hangar-internal", SECRET).send().await.unwrap().status(), 404);
    let revoked = client().post(format!("http://{}/__hangar_server/share/owner/revoke_all/s", srv.private))
        .header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(json_of(revoked).await, json!({"ok": true, "revoked": 1}));
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_invite_routes_on_the_public_port_are_answered_by_rust() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), true).await;
    srv.share.store.create("s", Some("k:A".into()), Kind::Share, false).unwrap();
    let url = format!("http://{}/api/sessions/s/share", srv.public);
    let resp = client().get(&url).bearer_auth(OWNER).header("origin", "http://outro").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().contains_key("access-control-allow-origin"), "o PWA de outra origem lê a resposta");
    assert_eq!(json_of(resp).await["shares"].as_array().unwrap().len(), 1);
    assert_eq!(srv.python.hits_to("/api/sessions/s/share"), 0, "o dono não passa pelo Python");
    // Quem não é o dono segue ao Python, que recusa como antes.
    client().get(&url).send().await.unwrap();
    assert_eq!(srv.python.hits_to("/api/sessions/s/share"), 1);
    let pairs = client().get(format!("http://{}/api/external-pairs", srv.public)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(json_of(pairs).await, json!([]));
}

#[test]
fn shared_mark_comes_from_the_share_store_and_python_may_omit_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ShareStore::open(dir.path().join("shares.json"), Arc::new(|| 100.0));
    store.create("a", Some("k:A".into()), Kind::Share, false).unwrap();
    store.create("p", Some("k:P".into()), Kind::Pair, false).unwrap();
    let facts: facts::ListFacts = serde_json::from_value(json!({"states": {}, "overrides": [], "frozen": [], "orq": [],
        "owners": {}, "hidden": [], "problems": {}, "held": {}, "stall_seconds": 300.0, "nav": {}, "shortcuts": null,
        "shadow": null})).expect("sem `shared` do Python");
    let row = |n: &str| -> SessionRow { serde_json::from_value(json!({"name": n, "provider": "claude"})).unwrap() };
    let (mut work, _) = facts::apply(vec![row("a"), row("p"), row("z")], &facts, true);
    facts::mark_shared(&mut work, &store.active_sessions());
    let shared: HashSet<&str> = work.iter().filter(|r| r.shared).map(|r| r.name.as_str()).collect();
    assert_eq!(shared, HashSet::from(["a"]), "o par não marca compartilhada");
}
