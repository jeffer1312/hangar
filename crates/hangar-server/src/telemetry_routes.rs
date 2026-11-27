//! Telemetria anônima no servidor: as rotas do dono, o envio diário e a convergência do dono comum
//! entre as máquinas. Contagem, fila e envio são do crate `hangar-telemetry`.
use std::collections::{BTreeMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_telemetry::events::{ClientKind, Code, Dims, Event, PeerKind};
use hangar_telemetry::{Disabled, Failure, Telemetry};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::groups::peers::PeerClient;
use crate::routes::AppState;
use crate::session_write::{detail, json_response};

const OWNER_ONLY: (&str, &str) = ("telemetry_owner_only", "só o dono pode ler ou mudar a telemetria");
const OWNER_PATH: &str = "/api/telemetry/owner";
const CONVERGE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// O retrato de peers sai uma vez por dia (`once`); a hora só decide quando ele é tirado.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(60 * 60);
/// O cliente manda `app.tempo` a cada 5 min: mais que isso num evento é relógio torto ou repetição.
const VISIBLE_CAP_MS: u64 = 5 * 60 * 1000;
/// `detalhe` de `rota.escolhida` (`packages/core/src/rota.ts`).
const ROUTES: &[&str] = &["principal", "rede_local", "tailscale", "connect"];
/// `codigo` de `rota.falhou`, em minúsculas: nome do erro do JS ou o código do Expo.
const ROUTE_FAILURES: &[&str] = &["error", "timeouterror", "aborterror", "typeerror", "route_timeout", "erro"];
/// `codigo` de `app.queda`, em minúsculas: nome do erro do JS, ou `panic` do nativo.
const FATAL_CODES: &[&str] = &["error", "typeerror", "referenceerror", "rangeerror", "syntaxerror", "panic"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice { consent: bool }

/// O motor desta vida. `None` quando `telemetry.json` não se lê nem se grava: o servidor sobe sem
/// telemetria, e as rotas respondem 503. Bloqueante.
pub fn open(home: &Path, diag: &crate::diag::DiagClient) -> Option<Arc<Telemetry>> {
    let package = crate::update::running_package();
    let config = hangar_telemetry::Config {
        home: home.to_path_buf(),
        backend: crate::session_open::cli::backend_dir_for_home(home),
        version: package.map_or(env!("CARGO_PKG_VERSION"), |p| p.version.as_str()).to_owned(),
        // Fora de pacote o servidor saiu de um checkout: o fio leva `dev`.
        release_commit: package.map(|p| p.commit.clone()),
        channel: crate::update::channel(&crate::update::Layout::new(home)),
    };
    match Telemetry::new(config) {
        Ok(telemetry) => Some(Arc::new(telemetry)),
        Err(e) => {
            tracing::warn!(code = "telemetry_unavailable", kind = ?e.kind(), "telemetria desligada nesta vida: estado ilegível");
            diag.report("telemetry.unavailable", "", "telemetry_unavailable", "telemetria desligada nesta vida");
            None
        }
    }
}

/// O envio diário e a convergência do dono; nada sem telemetria nesta vida.
pub fn spawn(st: &Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    let Some(telemetry) = st.telemetry.clone() else { return Vec::new() };
    let mut tasks = Vec::new();
    match telemetry.http_post() {
        Ok(post) => {
            let diag = st.diag.clone();
            tasks.push(tokio::spawn(telemetry.clone().run(post, move |failure| report(&diag, failure))));
        }
        Err(_) => st.diag.report("telemetry.unavailable", "", "telemetry_client_failed", "telemetria sem cliente HTTP: nada é enviado"),
    }
    let (snapshot, book) = (telemetry.clone(), st.peers.clone());
    tasks.push(tokio::spawn(async move {
        let mut done: Option<String> = None;
        loop {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            if done.as_deref() != Some(today.as_str()) {
                let (telemetry, book) = (snapshot.clone(), book.clone());
                if let Ok(true) = tokio::task::spawn_blocking(move || peers_snapshot(&telemetry, &book)).await { done = Some(today); }
            }
            tokio::time::sleep(SNAPSHOT_EVERY).await;
        }
    }));
    let peers = st.peers.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            converge(&telemetry, &peers).await;
            tokio::time::sleep(CONVERGE_EVERY).await;
        }
    }));
    tasks
}

/// Quantas máquinas do `peers.json` há de cada tipo, uma vez por dia (UTC). `true` quando o dia já está
/// contado; desligada, `false`, e a próxima hora tenta de novo. Bloqueante.
pub(crate) fn peers_snapshot(telemetry: &Telemetry, peers: &PeerClient) -> bool {
    if !telemetry.enabled() { return false; }
    let enabled: HashSet<String> = peers.enabled_ids().into_iter().collect();
    let mut kinds: BTreeMap<PeerKind, u64> = BTreeMap::new();
    for id in peers.ids() {
        // Só entrada de máquina (endereço e token), como a varredura do `PeerBook`.
        let Some(base_url) = peers.base_url(&id) else { continue };
        let kind = if enabled.contains(&id) { classify(&base_url) } else { Some(PeerKind::Disabled) };
        if let Some(kind) = kind { *kinds.entry(kind).or_default() += 1; }
    }
    if telemetry.once(&format!("peers:{}", chrono::Utc::now().format("%Y-%m-%d"))) {
        for (kind, n) in kinds { telemetry.count(Event::Peers, Dims::new().with("kind", kind), n); }
    }
    true
}

/// O tipo de ligação de um endereço; o endereço em si nunca sai daqui. `None` quando não é URL.
pub(crate) fn classify(base_url: &str) -> Option<PeerKind> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<IpAddr>() { return Some(ip_kind(ip)); }
    // ponytail: nome sem ponto (hosts, DNS da casa) conta como rede local; nome curto do MagicDNS cai aqui também.
    let local = !host.contains('.') || [".local", ".lan", ".home.arpa", ".internal"].iter().any(|suffix| host.ends_with(suffix));
    Some(if host.ends_with(".ts.net") { PeerKind::Tailscale }
        else if host.ends_with(".hangar.dev.br") { PeerKind::Connect }
        else if local { PeerKind::Lan }
        else { PeerKind::Public })
}

/// 100.64.0.0/10 e fd7a:115c:a1e0::/48 são as faixas que o Tailscale distribui.
fn ip_kind(ip: IpAddr) -> PeerKind {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            if a == 100 && (64..128).contains(&b) { PeerKind::Tailscale }
            else if v4.is_private() || v4.is_loopback() || v4.is_link_local() { PeerKind::Lan }
            else { PeerKind::Public }
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 { PeerKind::Tailscale }
            else if v6.is_loopback() || s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfe80 { PeerKind::Lan }
            else { PeerKind::Public }
        }
    }
}

/// Conta as linhas da tela que a telemetria conhece, das `accepted` primeiras que o diário gravou
/// (o resto a tela reenvia). Campo fora da lista vira `other`; texto nunca sai.
pub(crate) fn count_screen(telemetry: &Telemetry, body: &[u8], accepted: usize) {
    let Ok(batch) = serde_json::from_slice::<Value>(body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body)) else { return };
    let items = match &batch { Value::Object(fields) => fields.get("eventos"), other => Some(other) };
    let Some(Value::Array(items)) = items else { return };
    for item in items.iter().take(accepted) {
        screen_event(telemetry, item);
    }
}

fn screen_event(telemetry: &Telemetry, item: &Value) {
    let text = |key: &str| item[key].as_str().unwrap_or_default();
    let client = ClientKind::parse(text("cliente")).unwrap_or(ClientKind::Other);
    let took = item["ms"].as_u64();
    let dims = Dims::new().with("client", client);
    match text("evento") {
        "app.tempo" => {
            let minutes = took.map_or(0, |ms| (ms.min(VISIBLE_CAP_MS) + 30_000) / 60_000);
            if minutes > 0 { telemetry.count(Event::AppVisibleMinutes, dims, minutes); }
        }
        "rota.escolhida" => {
            let dims = dims.with("failed", false).with("route", Code::known(text("detalhe"), ROUTES));
            match took {
                Some(ms) => telemetry.duration(Event::ClientRoute, dims, Duration::from_millis(ms)),
                None => telemetry.count(Event::ClientRoute, dims, 1),
            }
        }
        "rota.falhou" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), ROUTE_FAILURES);
            telemetry.count(Event::ClientRoute, dims.with("failed", true).with("code", code), 1);
        }
        "chat.aberto" => if let Some(ms) = took { telemetry.duration(Event::ChatOpen, dims, Duration::from_millis(ms)); },
        "app.queda" => {
            let code = Code::known(&text("codigo").to_ascii_lowercase(), FATAL_CODES);
            telemetry.count(Event::ClientFatal, dims.with("code", code), 1);
        }
        _ => {}
    }
}

fn report(diag: &crate::diag::DiagClient, failure: Failure) {
    match failure {
        Failure::Refused(status) => diag.report("telemetry.rejected", "", &format!("http_{status}"), "pacote de telemetria recusado"),
        Failure::Disk(e) => {
            tracing::warn!(code = "telemetry_disk_failed", kind = ?e.kind(), "telemetria: fila não lida ou não gravada");
            diag.report("telemetry.disk", "", "telemetry_disk_failed", "fila da telemetria não lida ou não gravada");
        }
    }
}

/// Uma passada: cada máquina habilitada do `peers.json` diz o dono dela, juntas, e o mais antigo é
/// adotado (empate: menor id). Máquina fora do ar, sem a rota (versão antiga) ou que recusa não muda nada.
pub(crate) async fn converge(telemetry: &Arc<Telemetry>, peers: &Arc<PeerClient>) {
    let ids = { let peers = peers.clone(); tokio::task::spawn_blocking(move || peers.enabled_ids()).await.unwrap_or_default() };
    let answers = futures_util::future::join_all(ids.iter().map(|id| peers.call(id, reqwest::Method::GET, OWNER_PATH, None))).await;
    let oldest = answers.into_iter().filter_map(|answer| {
        let answer = answer.ok()??;
        Some((answer["owner_created"].as_i64()?, answer["owner_id"].as_str()?.to_owned()))
    }).min();
    let Some((created, owner_id)) = oldest else { return };
    let telemetry = telemetry.clone();
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || telemetry.adopt_owner(&owner_id, created)).await {
        tracing::warn!(code = "telemetry_owner_unsaved", kind = ?e.kind(), "telemetria: dono comum não gravado");
    }
}

/// Pedido de convidado por qualquer porta: convite, par, login ou a 8766.
pub(crate) fn guest(req: &Request) -> bool {
    crate::routes::on_guest_port(req) || req.headers().contains_key("x-hangar-guest")
        || req.extensions().get::<crate::login_guests::gate::Login>().is_some()
        || matches!(crate::share::caller::caller(req), Some(crate::share::caller::Caller::Guest(_)))
}

fn refused() -> Response { detail(StatusCode::FORBIDDEN, OWNER_ONLY.0, OWNER_ONLY.1, json!({})) }

fn unavailable() -> Response {
    detail(StatusCode::SERVICE_UNAVAILABLE, "telemetry_unavailable", "não foi possível ler ou guardar a telemetria", json!({}))
}

/// Só o dono. Convidado (convite, par, login, porta 8766) leva 403 sem contar falha de senha; token
/// errado, a recusa contada. O pedido é lido antes da espera: `&Request` atravessando um `await` tira o `Send`.
fn admit(st: &AppState, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let guest = guest(req);
    let owner = st.edge().owner_only(peer, req, OWNER_ONLY);
    async move {
        if guest { return Err(refused()); }
        owner.await
    }
}

fn view(telemetry: &Telemetry) -> Value {
    json!({"consent": telemetry.consent(), "disabled_by": telemetry.disabled_by().map(Disabled::as_str)})
}

/// `GET`/`PUT /api/telemetry`: `{"consent": null|true|false, "disabled_by": null|"env"|"ci"}`; o `PUT`
/// leva `{"consent": bool}`, e `false` apaga a fila.
pub async fn setting(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(response) = admit(&st, peer, &req).await { return response; }
    let Some(telemetry) = st.telemetry.clone() else { return unavailable() };
    let choice = if req.method() == Method::PUT {
        let parsed = to_bytes(req.into_body(), 1024).await.ok().and_then(|raw| serde_json::from_slice::<Choice>(&raw).ok());
        let Some(choice) = parsed else {
            return detail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "esperado {\"consent\": true ou false}", json!({}));
        };
        Some(choice.consent)
    } else {
        None
    };
    let done = tokio::task::spawn_blocking(move || {
        if let Some(consent) = choice { telemetry.set_consent(consent)?; }
        Ok::<Value, std::io::Error>(view(&telemetry))
    }).await;
    match done {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(e)) => {
            tracing::warn!(code = "telemetry_unavailable", kind = ?e.kind(), "telemetria: escolha não guardada");
            unavailable()
        }
        Err(_) => unavailable(),
    }
}

/// `GET /api/telemetry/owner`: o dono desta máquina, para as outras do mesmo dono convergirem.
pub async fn owner(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Err(response) = admit(&st, peer, &req).await { return response; }
    let Some(telemetry) = st.telemetry.clone() else { return unavailable() };
    match telemetry.owner() {
        Ok((owner_id, owner_created)) => json_response(StatusCode::OK, json!({"owner_id": owner_id, "owner_created": owner_created})),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::peers::PeerBook;
    use crate::telemetry_hooks::{test_state, test_telemetry};
    use axum::body::Body;

    const TOKEN: &str = "owner";

    async fn put(st: &Arc<AppState>, body: &str, guest: bool) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder().method(Method::PUT).uri("/api/telemetry")
            .header("authorization", format!("Bearer {TOKEN}")).header("content-type", "application/json");
        if guest { req = req.header("x-hangar-guest", "1"); }
        let response = setting(State(st.clone()), ConnectInfo("127.0.0.1:5000".parse().unwrap()), req.body(Body::from(body.to_owned())).unwrap()).await;
        let status = response.status();
        let raw = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn guest_put_is_refused_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        let (status, body) = put(&st, r#"{"consent": true}"#, true).await;
        assert_eq!((status, &body["detail"]["code"]), (StatusCode::FORBIDDEN, &json!("telemetry_owner_only")));
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
    }

    #[tokio::test]
    async fn only_a_boolean_consent_is_accepted() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), false);
        for body in [r#"{"consent": "sim"}"#, r#"{"consent": null}"#, r#"{"consent": true, "x": 1}"#, "{}", "nada"] {
            let (status, reply) = put(&st, body, false).await;
            assert_eq!((status, &reply["detail"]["code"]), (StatusCode::UNPROCESSABLE_ENTITY, &json!("erro_corpo_invalido")), "{body}");
        }
        assert_eq!(st.telemetry.as_ref().unwrap().consent(), None);
        assert_eq!(put(&st, r#"{"consent": true}"#, false).await, (StatusCode::OK, json!({"consent": true, "disabled_by": null})));
    }

    /// Uma máquina do dono que responde `status` com `body` em `/api/telemetry/owner`.
    async fn peer(dir: &Path, status: StatusCode, body: Value) -> (Arc<PeerClient>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answer = (status, body.to_string());
        let router = axum::Router::new().route(OWNER_PATH, axum::routing::get(move || { let answer = answer.clone(); async move { answer } }));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
        let file = dir.join("peers.json");
        std::fs::write(&file, json!({"lab": {"base_url": format!("http://{address}"), "token": TOKEN}}).to_string()).unwrap();
        (Arc::new(PeerClient::new(PeerBook::new(Some(file)))), task)
    }

    #[tokio::test]
    async fn older_owner_of_a_peer_is_adopted() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), false);
        let older = "0".repeat(32);
        let (peers, task) = peer(home.path(), StatusCode::OK, json!({"owner_id": older, "owner_created": 1})).await;
        converge(&telemetry, &peers).await;
        task.abort();
        assert_eq!(telemetry.owner().unwrap(), (older, 1));
    }

    #[tokio::test]
    async fn peer_without_the_route_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), false);
        let before = telemetry.owner().unwrap();
        let (peers, task) = peer(home.path(), StatusCode::NOT_FOUND, json!({"detail": "Not Found"})).await;
        converge(&telemetry, &peers).await;
        task.abort();
        assert_eq!(telemetry.owner().unwrap(), before);
    }

    fn has(telemetry: &Telemetry, event: Event, dims: Dims, bucket: Option<hangar_telemetry::buckets::Bucket>, count: u64) -> bool {
        telemetry.counters().contains(&hangar_telemetry::wire::Counter::new(event, dims, bucket, count))
    }

    #[test]
    fn screen_events_count_only_known_values() {
        use hangar_telemetry::buckets::Bucket;
        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let body = json!({"eventos": [
            {"evento": "app.tempo", "ms": 900_000, "cliente": "ios"},
            {"evento": "rota.escolhida", "detalhe": "tailscale", "ms": 250, "cliente": "android"},
            {"evento": "rota.falhou", "codigo": "TimeoutError", "cliente": "android"},
            {"evento": "rota.falhou", "codigo": "https://casa.exemplo", "cliente": "android"},
            {"evento": "chat.aberto", "ms": 1_200, "cliente": "native"},
            {"evento": "app.queda", "codigo": "TypeError", "cliente": "celular-do-jeff"},
            {"evento": "sse.conectou", "ms": 10, "cliente": "ios"},
            // Fora das linhas que o diário gravou: a tela reenvia, e aqui não conta.
            {"evento": "app.tempo", "ms": 900_000, "cliente": "ios"},
        ]}).to_string();
        count_screen(&telemetry, body.as_bytes(), 7);
        let android = || Dims::new().with("client", ClientKind::Android);
        assert!(has(&telemetry, Event::AppVisibleMinutes, Dims::new().with("client", ClientKind::Ios), None, 5), "teto de 5 min");
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", false).with("route", Code::known("tailscale", ROUTES)), Some(Bucket::Lt300ms), 1));
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", true).with("code", Code::known("timeouterror", ROUTE_FAILURES)), None, 1));
        assert!(has(&telemetry, Event::ClientRoute, android().with("failed", true).with("code", Code::OTHER), None, 1));
        assert!(has(&telemetry, Event::ChatOpen, Dims::new().with("client", ClientKind::Native), Some(Bucket::Lt3s), 1));
        assert!(has(&telemetry, Event::ClientFatal, Dims::new().with("client", ClientKind::Other).with("code", Code::known("typeerror", FATAL_CODES)), None, 1));
        assert_eq!(telemetry.counters().len(), 6);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("exemplo") && !wire.contains("celular"), "{wire}");
    }

    #[test]
    fn tailscale_address_is_counted_by_kind_only() {
        assert_eq!(classify("https://x.tail1.ts.net:8765"), Some(PeerKind::Tailscale));
        assert_eq!(classify("http://100.64.0.2:8765"), Some(PeerKind::Tailscale));
        assert_eq!(classify("https://notebook.jeff.hangar.dev.br"), Some(PeerKind::Connect));
        assert_eq!(classify("http://192.168.0.10:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("http://[fe80::1]:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("http://notebook-tunel:8765"), Some(PeerKind::Lan));
        assert_eq!(classify("https://exemplo.com.br"), Some(PeerKind::Public));
        assert_eq!(classify("não é url"), None);

        let home = tempfile::tempdir().unwrap();
        let telemetry = test_telemetry(home.path(), true);
        let file = home.path().join("peers.json");
        std::fs::write(&file, json!({
            "a": {"base_url": "https://x.tail1.ts.net:8765", "token": "t"},
            "b": {"base_url": "http://100.64.0.2:8765", "token": "t"},
            "c": {"base_url": "https://notebook.jeff.hangar.dev.br", "token": "t"},
            "d": {"base_url": "http://192.168.0.10:8765", "token": "t"},
            "e": {"base_url": "https://exemplo.com.br", "token": "t"},
            "f": {"base_url": "http://10.0.0.2:8765", "token": "t", "enabled": false},
            "g": {"app": {"nome": "celular"}},
            "h": {"base_url": "https://y.ts.net", "token": ""},
        }).to_string()).unwrap();
        let peers = PeerClient::new(PeerBook::new(Some(file)));
        assert!(peers_snapshot(&telemetry, &peers));
        assert!(peers_snapshot(&telemetry, &peers), "o dia já contado também é feito");
        let kind = |kind: PeerKind, n: u64| has(&telemetry, Event::Peers, Dims::new().with("kind", kind), None, n);
        assert!(kind(PeerKind::Tailscale, 2), "um retrato por dia");
        assert!(kind(PeerKind::Connect, 1) && kind(PeerKind::Lan, 1) && kind(PeerKind::Public, 1) && kind(PeerKind::Disabled, 1));
        assert_eq!(telemetry.counters().len(), 5);
        let wire = serde_json::to_string(&telemetry.counters()).unwrap();
        assert!(!wire.contains("tail1") && !wire.contains("192.168") && !wire.contains("exemplo"), "{wire}");
    }
}
