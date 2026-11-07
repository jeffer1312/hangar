//! Rotas do convidado (`test_share_guest_api.py` e o `attach` de `test_external_pair_api.py`).
//!
//! Ficam para a Task 9 (lista, stream e handlers da sessão): `lista_do_convidado_so_tem_a_sessao_dele`,
//! `lista_do_convidado_nao_cita_outras_sessoes`, `guest_cannot_start_a_hangar_copy`,
//! `guest_does_not_reach_the_hangar_terminal_routes`, `stream_da_lista_filtrado_e_sem_contar_app`,
//! `stream_da_lista_nao_cita_outras_sessoes`, `stream_da_sessao_do_convidado_nao_conta_como_app_do_dono`,
//! `stream_da_lista_acompanha_a_sessao_renomeada`, `lista_do_convidado_marca_o_tipo_de_cada_sessao`,
//! `stream_da_lista_enxerga_sessao_ligada_ao_token_depois_de_aberto`.
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{Extension, Router};
use futures_util::future::BoxFuture;
use hangar_server::share::ShareState;
use hangar_server::share::caller::Caller;
use hangar_server::share::gate::{Gate, GuestConn, guard};
use hangar_server::share::guest_routes::{LocalAddr, guest_router};
use hangar_server::share::life::{LifeFacts, LifeProbe, Lives};
use hangar_server::share::pairs::PairStore;
use hangar_server::share::store::{Kind, ShareStore};
use hangar_server::share::tunnel::{Output, Tailscale};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeTs { down: Mutex<bool> }

impl Tailscale for FakeTs {
    fn run<'a>(&'a self, args: &'a [&'a str]) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            if *self.down.lock().unwrap() { return Err("sem tailscale".into()); }
            assert_eq!(args, ["status", "--json"]);
            let out = json!({"Self": {"ID": "n1", "DNSName": "nb.ts.net."}}).to_string();
            Ok(Output { success: true, stdout: out, stderr: String::new() })
        })
    }
}

/// Sessões vivas no multiplexador, nascidas em 1.
struct FakeLife;

impl LifeProbe for FakeLife {
    fn birth<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<u64>> { Box::pin(async { Some(1) }) }
    fn has_session<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<bool>> { Box::pin(async { Some(true) }) }
    fn sidecar(&self, _name: &str) -> Option<(String, bool)> { None }
    fn sidecar_exists(&self, _name: &str) -> bool { false }
}

struct Env {
    _dir: tempfile::TempDir,
    ts: Arc<FakeTs>,
    clock: Arc<Mutex<f64>>,
    share: Arc<ShareState>,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(1_000.0));
    let c = clock.clone();
    let store = ShareStore::open(dir.path().join("shares.json"), Arc::new(move || *c.lock().unwrap()));
    let facts = Arc::new(LifeFacts::default());
    facts.apply_snapshot(HashMap::new(), HashSet::new());
    let ts = Arc::new(FakeTs::default());
    let share = Arc::new(ShareState {
        store,
        pairs: PairStore::open(dir.path().join("external_pairs.json")),
        lives: Lives::new(Arc::new(FakeLife), facts),
        tailscale: ts.clone(),
        tunnel: Default::default(),
        connect: None,
    });
    Env { _dir: dir, ts, clock, share }
}

struct Client { base: String, http: reqwest::Client }

impl Env {
    /// Pelo porteiro de verdade; `local` é o endereço em que a conexão do convidado chegou.
    async fn app(&self, local: &str) -> Client {
        let gate = Arc::new(Gate::new(self.share.clone()));
        // Por dentro do porteiro: troca o endereço que ele leu da conexão (loopback no teste).
        let router = guest_router(self.share.clone(), "nb".into())
            .layer(Extension(LocalAddr(local.parse::<SocketAddr>().unwrap())))
            .layer(axum::middleware::from_fn_with_state(gate, guard));
        serve(router).await
    }

    fn invite(&self, session: &str) -> String {
        self.share.store.create(session, Some("t:1".into()), Kind::Share, false).unwrap().1
    }
}

async fn serve(router: Router) -> Client {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<GuestConn>()).await.unwrap()
    });
    Client { base: format!("http://{addr}"), http: reqwest::Client::new() }
}

impl Client {
    async fn page(&self, path: &str) -> (u16, reqwest::header::HeaderMap, String) {
        let r = self.http.get(format!("{}{path}", self.base)).send().await.unwrap();
        (r.status().as_u16(), r.headers().clone(), r.text().await.unwrap())
    }
    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (u16, Value) {
        let mut req = self.http.post(format!("{}{path}", self.base))
            .header("content-type", "application/json").body(body.to_string());
        if let Some(t) = token { req = req.bearer_auth(t); }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

const FUNNEL: &str = "127.0.0.1:8766";
const LAN: &str = "192.168.77.142:8766";

fn squash(s: &str) -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") }

#[tokio::test]
async fn pagina_do_convite_nao_gasta_o_codigo() {
    let e = env();
    let code = e.invite("proj");
    let (status, headers, text) = e.app(FUNNEL).await.page(&format!("/convite/{code}")).await;
    assert_eq!(status, 200);
    assert!(text.contains("proj"));
    assert!(text.contains(&format!("hangar://convite/nb.ts.net:8443/{code}")));
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(e.share.store.all()[0].redeemed_at.is_none());
}

#[tokio::test]
async fn pagina_com_tunel_fora_responde_503() {
    let e = env();
    let code = e.invite("proj");
    *e.ts.down.lock().unwrap() = true;
    let (status, headers, text) = e.app(FUNNEL).await.page(&format!("/convite/{code}")).await;
    assert_eq!(status, 503);
    assert!(text.contains("indisponível por instantes"));
    assert!(!text.contains("hangar://"));
    assert_eq!(headers["cache-control"], "no-store");
    assert!(headers.get("referrer-policy").is_none());
}

#[tokio::test]
async fn link_do_app_escapa_o_codigo() {
    // O código aceita espaço (normaliza para o mesmo hash); no link ele sai codificado como no `quote`.
    let e = env();
    let code = e.invite("proj");
    let (status, _, text) = e.app(FUNNEL).await.page(&format!("/convite/{code}%20%09")).await;
    assert_eq!(status, 200);
    assert!(text.contains(&format!("convite/nb.ts.net:8443/{code}%20%09\"")), "{text}");
    assert!(!text.contains(&format!("{code} ")));
}

#[tokio::test]
async fn pagina_mostra_o_motivo() {
    let e = env();
    let app = e.app(FUNNEL).await;
    let used = e.invite("proj");
    e.share.store.redeem(&used, "", None, Kind::Share).unwrap();
    let revoked = e.invite("proj");
    let id = e.share.store.all().into_iter().find(|s| s.redeemed_at.is_none()).unwrap().id;
    e.share.store.revoke(&id).unwrap();
    let expired = e.invite("proj");
    *e.clock.lock().unwrap() += 25.0 * 3600.0;
    for (code, text) in [(used.as_str(), "já foi usado"), (expired.as_str(), "venceu"),
                         (revoked.as_str(), "cancelado"), ("NAOEXISTE", "não encontrado")] {
        let (status, headers, page) = app.page(&format!("/convite/{code}")).await;
        assert_eq!(status, 200);
        assert!(page.contains(text), "{code}: {page}");
        assert!(!page.contains("hangar://"));
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["referrer-policy"], "no-referrer");
    }
}

#[tokio::test]
async fn pagina_escapa_o_nome_da_sessao() {
    let e = env();
    let code = e.invite("<script>x</script>");
    let text = e.app(FUNNEL).await.page(&format!("/convite/{code}")).await.2;
    assert!(!text.contains("<script>x</script>"));
    assert!(text.contains("&lt;script&gt;x&lt;/script&gt;"));
}

#[tokio::test]
async fn pagina_igual_a_do_python() {
    let e = env();
    let code = e.invite("proj");
    let mut text = e.app(FUNNEL).await.page(&format!("/convite/{code}")).await.2;
    text = text.replace(&code, "ABCD");
    assert_eq!(squash(&text), squash(include_str!("../fixtures/share/invite_valid.html")));
}

#[tokio::test]
async fn pagina_indisponivel_igual_a_do_python() {
    let e = env();
    let code = e.invite("proj");
    e.share.store.redeem(&code, "", None, Kind::Share).unwrap();
    let text = e.app(FUNNEL).await.page(&format!("/convite/{code}")).await.2;
    assert_eq!(squash(&text), squash(include_str!("../fixtures/share/invite_used.html")));
}

#[tokio::test]
async fn resgate_devolve_token_e_endereco() {
    let e = env();
    let code = e.invite("proj");
    let (status, body) = e.app(FUNNEL).await.post("/api/guest/redeem", None, json!({"code": code, "device": "Pixel"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["session"], "proj");
    assert_eq!(body["address"], "https://nb.ts.net:8443");
    assert_eq!(body["owner"], "nb");
    let token = body["token"].as_str().unwrap();
    assert!(e.share.store.lookup_token(token).is_some());
    assert_eq!(e.share.store.all()[0].device.as_deref(), Some("Pixel"));
}

#[tokio::test]
async fn convite_pela_rede_local_usa_o_ip_e_nao_o_tunel() {
    let e = env();
    *e.ts.down.lock().unwrap() = true;
    let code = e.invite("proj");
    let app = e.app(LAN).await;
    assert!(app.page(&format!("/convite/{code}")).await.2.contains(&format!("hangar://convite/192.168.77.142:8766/{code}")));
    let (status, body) = app.post("/api/guest/redeem", None, json!({"code": code, "device": "Pixel"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["address"], "http://192.168.77.142:8766");
}

#[tokio::test]
async fn funnel_em_ipv6_mapeado_tambem_e_loopback() {
    let e = env();
    let code = e.invite("proj");
    let text = e.app("[::ffff:127.0.0.1]:8766").await.page(&format!("/convite/{code}")).await.2;
    assert!(text.contains("hangar://convite/nb.ts.net:8443/"), "{text}");
}

#[tokio::test]
async fn resgate_com_tunel_fora_nao_gasta_o_codigo() {
    let e = env();
    let code = e.invite("proj");
    *e.ts.down.lock().unwrap() = true;
    let (status, body) = e.app(FUNNEL).await.post("/api/guest/redeem", None, json!({"code": code, "device": "Pixel"})).await;
    assert_eq!(status, 503);
    assert_eq!(body["detail"]["code"], "erro_sessao_indisponivel");
    assert!(e.share.store.all()[0].redeemed_at.is_none());
}

#[tokio::test]
async fn resgate_recusado_com_motivo() {
    let e = env();
    let app = e.app(FUNNEL).await;
    let used = e.invite("proj");
    e.share.store.redeem(&used, "", None, Kind::Share).unwrap();
    let revoked = e.invite("proj");
    let id = e.share.store.all().into_iter().find(|s| s.redeemed_at.is_none()).unwrap().id;
    e.share.store.revoke(&id).unwrap();
    let expired = e.invite("proj");
    *e.clock.lock().unwrap() += 25.0 * 3600.0;
    for (code, reason, status, err) in [
        (used.as_str(), "used", 410, "erro_convite_usado"), (expired.as_str(), "expired", 410, "erro_convite_vencido"),
        (revoked.as_str(), "revoked", 410, "erro_convite_revogado"), ("NAOEXISTE", "unknown", 404, "erro_convite_inexistente"),
    ] {
        let (got, body) = app.post("/api/guest/redeem", None, json!({"code": code, "device": "Pixel"})).await;
        assert_eq!(got, status, "{reason}: {body}");
        assert_eq!(body["detail"]["code"], err);
        assert_eq!(body["detail"]["params"]["reason"], reason);
    }
}

#[tokio::test]
async fn resgate_sem_codigo_e_422() {
    let e = env();
    let (status, body) = e.app(FUNNEL).await.post("/api/guest/redeem", None, json!({"device": "Pixel"})).await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["loc"], json!(["body", "code"]));
}

#[tokio::test]
async fn resgate_com_token_existente_devolve_o_mesmo_token() {
    let e = env();
    let app = e.app(FUNNEL).await;
    let (ca, cb) = (e.invite("proj-a"), e.invite("proj-b"));
    let tok = app.post("/api/guest/redeem", None, json!({"code": ca, "device": "PC"})).await.1["token"].as_str().unwrap().to_owned();
    let (_, r) = app.post("/api/guest/redeem", None, json!({"code": cb, "device": "PC", "token": tok})).await;
    assert_eq!(r["token"], tok.as_str());
    assert_eq!(r["session"], "proj-b");
}

#[tokio::test]
async fn attach_liga_as_sessoes_do_outro_token() {
    let e = env();
    let (_, a) = e.share.store.create_redeemed("X", Some("t:1".into()), Kind::Share).unwrap();
    let (_, b) = e.share.store.create_redeemed("W", Some("t:1".into()), Kind::Share).unwrap();
    let (status, body) = e.app(FUNNEL).await.post("/api/guest/attach", Some(&a), json!({"token": b})).await;
    assert_eq!((status, body), (200, json!({"attached": 1})));
    assert!(e.share.store.lookup_token(&a).unwrap().sees("W"));
}

#[tokio::test]
async fn attach_sem_ser_convidado_e_negado() {
    // Sem o porteiro na frente (como na 8765): o dono não liga tokens.
    let e = env();
    let app = serve(guest_router(e.share.clone(), "nb".into()).layer(Extension(Caller::Owner))).await;
    let (status, body) = app.post("/api/guest/attach", None, json!({"token": "x"})).await;
    assert_eq!(status, 403);
    assert_eq!(body["detail"]["code"], "erro_fora_do_convite");
    assert_eq!(body["detail"]["msg"], "só convidado");
}

#[tokio::test]
async fn attach_com_token_desconhecido_ou_revogado_da_409() {
    let e = env();
    let app = e.app(FUNNEL).await;
    let (_, a) = e.share.store.create_redeemed("X", Some("t:1".into()), Kind::Share).unwrap();
    let (s, b) = e.share.store.create_redeemed("W", Some("t:1".into()), Kind::Share).unwrap();
    assert_eq!(app.post("/api/guest/attach", Some(&a), json!({"token": "nao-existe"})).await.0, 409);
    e.share.store.revoke(&s.id).unwrap();
    let (status, body) = app.post("/api/guest/attach", Some(&a), json!({"token": b})).await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "erro_par_attach_invalido");
}
