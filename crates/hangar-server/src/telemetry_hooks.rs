//! Ganchos da telemetria sem handler próprio: fim de turno, recurso usado pela API e o motor desta vida
//! para quem não recebe o estado do servidor. Contar, guardar e enviar é do crate `hangar-telemetry`.
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use hangar_telemetry::Telemetry;
use hangar_telemetry::events::{Dims, Event, Provider};

use crate::list::bridge::ListBridge;
use crate::push::watch::TurnEvent;
use crate::routes::AppState;

static INSTALLED: OnceLock<Arc<Telemetry>> = OnceLock::new();

/// O motor desta vida, só contando, para os ganchos que não recebem o estado do servidor (lista, aviso
/// tirado da conversa, sessão caída). Vazio até a subida ligar.
pub(crate) fn installed() -> Option<&'static Telemetry> { INSTALLED.get().map(Arc::as_ref).filter(|t| t.enabled()) }

/// Liga o motor desta vida aos ganchos, conta a saída da vida anterior e assina os fins de turno.
pub fn spawn(st: &Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let telemetry = st.telemetry.clone()?;
    let _ = INSTALLED.set(telemetry.clone());
    crate::update::stop_intent::count_previous_exits(&telemetry);
    Some(tokio::spawn(turns(st.list.clone(), telemetry)))
}

async fn turns(list: Arc<ListBridge>, telemetry: Arc<Telemetry>) {
    use tokio::sync::broadcast::error::RecvError;
    let mut events = crate::push::watch::subscribe();
    let mut clock = TurnClock::default();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        let Some(took) = clock.on(&event) else { continue };
        if !telemetry.enabled() { continue; }
        telemetry.duration(Event::TurnFinished, Dims::new().with("provider", provider_of(&list, &event.sid)), took);
    }
}

/// O provedor da conversa pela última lista boa, sem pedir rodada; fora dela, `other`.
fn provider_of(list: &ListBridge, sid: &str) -> Provider {
    list.last_rows().and_then(|rows| rows.iter().find(|row| row.jsonl.as_deref().is_some_and(|jsonl| crate::locate::session_key(jsonl) == sid))
        .and_then(|row| Provider::parse(&row.provider))).unwrap_or(Provider::Other)
}

/// Início do turno de cada conversa: `working` abre, `idle` fecha e dá a duração.
#[derive(Default)]
struct TurnClock(HashMap<String, f64>);

impl TurnClock {
    fn on(&mut self, event: &TurnEvent) -> Option<Duration> {
        match event.state.as_str() {
            "working" => {
                // ponytail: conversa que morre trabalhando fica aqui; o teto limpa tudo de uma vez.
                if self.0.len() >= 1024 { self.0.clear(); }
                self.0.entry(event.sid.clone()).or_insert(event.ts);
                None
            }
            "idle" => self.0.remove(&event.sid).and_then(|started| Duration::try_from_secs_f64((event.ts - started).max(0.0)).ok()),
            _ => None,
        }
    }
}

/// `feature_used`, decidido depois do handler: modelo de rota da lista, resposta 2xx e só o dono. O MCP
/// chama o roteador interno, que não passa por esta camada.
pub(crate) async fn count_feature(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if st.counting().is_none() { return next.run(req).await; }
    let feature = req.extensions().get::<MatchedPath>()
        .and_then(|path| hangar_telemetry::events::feature(req.method().as_str(), path.as_str()))
        .filter(|_| !crate::telemetry_routes::guest(&req));
    let Some(feature) = feature else { return next.run(req).await };
    let marker = req.extensions().get::<crate::routes::NotOwner>().cloned();
    let response = next.run(req).await;
    if response.status().is_success() && !marker.is_some_and(|marker| marker.seen()) && let Some(telemetry) = st.counting() {
        telemetry.count(Event::FeatureUsed, Dims::new().with("feature", feature), 1);
    }
    response
}

/// O motor dos testes do servidor; `consent` responde ao aviso, senão fica sem resposta.
#[cfg(test)]
pub(crate) fn test_telemetry(home: &std::path::Path, consent: bool) -> Arc<Telemetry> {
    let telemetry = Telemetry::for_test(home, Arc::new(chrono::Utc::now));
    if consent { telemetry.set_consent(true).unwrap(); }
    Arc::new(telemetry)
}

/// Um servidor de teste com o motor de `test_telemetry`; o token do dono é `owner`.
#[cfg(test)]
pub(crate) fn test_state(home: &std::path::Path, consent: bool) -> Arc<AppState> { Arc::new(test_app_state(home, consent)) }

#[cfg(test)]
fn test_app_state(home: &std::path::Path, consent: bool) -> AppState {
    let mut st = AppState::new(crate::config::Config { listen: "127.0.0.1:0".parse().unwrap(), internal_secret: "s".into(),
        auth_token: "owner".into(), log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1") });
    st.telemetry = Some(test_telemetry(home, consent));
    st
}

/// O motor de `installed()` nos testes: um por processo, com consentimento; cada teste confere só os
/// códigos dele, porque os outros contam no mesmo motor.
#[cfg(test)]
pub(crate) fn installed_for_test() -> &'static Telemetry {
    INSTALLED.get_or_init(|| {
        let home = std::env::temp_dir().join(format!("hangar-telemetry-installed-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        test_telemetry(&home, true)
    }).as_ref()
}

/// Os contadores de `event` como (valor da dimensão `key`, contagem).
#[cfg(test)]
pub(crate) fn counted(telemetry: &Telemetry, event: Event, key: &str) -> Vec<(String, u64)> {
    telemetry.counters().into_iter().filter(|c| c.event == event.as_str())
        .map(|c| (c.dims.get(key).cloned().unwrap_or_default(), c.count)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::{get, put};
    use tower::ServiceExt;

    fn app(st: Arc<AppState>) -> axum::Router {
        axum::Router::new()
            .route("/api/sessions/{name}/navegador", get(|| async { StatusCode::OK }))
            .route("/api/sessions/{name}/history", get(|| async { StatusCode::OK }))
            .route("/api/tags", put(|| async { StatusCode::FORBIDDEN }))
            .layer(axum::middleware::from_fn_with_state(st.clone(), count_feature))
            .with_state(st)
    }

    fn request(method: &str, uri: &str) -> Request {
        axum::http::Request::builder().method(method).uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn telemetry_feature_counts_the_route_template_of_the_owner_only() {
        let home = tempfile::tempdir().unwrap();
        let st = test_state(home.path(), true);
        let app = app(st.clone());
        for uri in ["/api/sessions/minha-sessao/navegador", "/api/sessions/outra/navegador"] {
            assert_eq!(app.clone().oneshot(request("GET", uri)).await.unwrap().status(), StatusCode::OK);
        }
        app.clone().oneshot(request("GET", "/api/sessions/minha-sessao/history")).await.unwrap();
        assert_eq!(app.clone().oneshot(request("PUT", "/api/tags")).await.unwrap().status(), StatusCode::FORBIDDEN);
        let mut guest = request("GET", "/api/sessions/minha-sessao/navegador");
        guest.extensions_mut().insert(axum::extract::ConnectInfo(crate::share::gate::GuestConn { peer: "127.0.0.1:1".parse().unwrap(), local: None }));
        app.clone().oneshot(guest).await.unwrap();
        let mut header = request("GET", "/api/sessions/minha-sessao/navegador");
        header.headers_mut().insert("x-hangar-guest", "1".parse().unwrap());
        app.clone().oneshot(header).await.unwrap();
        let mut login = request("GET", "/api/sessions/minha-sessao/navegador");
        login.extensions_mut().insert(crate::login_guests::gate::Login { token: "t".into(), guest: crate::login_guests::store::Guest {
            id: "g".into(), name: "convidado".into(), root: "/x".into(), sees_owner: false, owner_sees: false } });
        app.clone().oneshot(login).await.unwrap();
        let mut invited = request("GET", "/api/sessions/minha-sessao/navegador");
        invited.extensions_mut().insert(crate::share::caller::Caller::Guest(Arc::new(crate::share::caller::GuestGrant {
            token: "t".into(), token_hash: "h".into(), sessions: Default::default(), pair_roots: Vec::new(), invite: true })));
        app.clone().oneshot(invited).await.unwrap();
        let mut stranger = request("GET", "/api/sessions/minha-sessao/navegador");
        stranger.extensions_mut().insert(crate::routes::NotOwner::seen_for_test());
        app.clone().oneshot(stranger).await.unwrap();
        let telemetry = st.telemetry.as_ref().unwrap();
        assert_eq!(counted(telemetry, Event::FeatureUsed, "feature"), [("session.browser".to_owned(), 2)]);
        assert!(telemetry.counters().iter().all(|c| c.dims.values().all(|v| !v.contains("minha-sessao"))), "nome de sessão nunca sai");
    }

    /// Cada (método, modelo) de `FEATURE_ROUTES` casa uma rota do roteador real com esse `MatchedPath`, e o
    /// método está na tabela das rotas do Rust. A sonda responde antes dos handlers: nada roda.
    #[tokio::test]
    async fn telemetry_every_feature_route_is_registered() {
        let home = tempfile::tempdir().unwrap();
        let probe = axum::middleware::from_fn(|req: Request, _: Next| async move {
            req.extensions().get::<MatchedPath>().map_or_else(String::new, |path| path.as_str().to_owned())
        });
        // Convite e par externo só montam com o `shares.json`; sem ele as rotas do par não existem.
        let mut st = test_app_state(home.path(), false);
        let dirs = crate::list::discover_other::Dirs { home: home.path().into(), claude: home.path().join(".claude"),
            codex_home: home.path().join(".codex"), pi_sessions: home.path().join("pi"), omp_config: home.path().join("omp"),
            omp_agent: home.path().join("omp/agent"), kimi_home: home.path().join("kimi") };
        st.share = crate::share::from_env(Some(home.path().join("shares.json")), Some(&dirs), "missing-test-mux".into(), None);
        let app = crate::routes::router(Arc::new(st)).layer(probe);
        for (method, template, _) in hangar_telemetry::events::FEATURE_ROUTES {
            let path: String = template.split('/').map(|part| if part.starts_with('{') { "x" } else { part }).collect::<Vec<_>>().join("/");
            let response = app.clone().oneshot(request(method, &path)).await.unwrap();
            let matched = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(String::from_utf8_lossy(&matched), *template, "{method} {template} fora do roteador");
            let method = axum::http::Method::from_bytes(method.as_bytes()).unwrap();
            assert!(crate::owned_routes::rust_route(&method, &path), "{method} {template} fora da tabela das rotas do Rust");
        }
    }

    #[test]
    fn telemetry_turn_lasts_from_working_to_idle() {
        let event = |sid: &str, state: &str, ts: f64| TurnEvent { sid: sid.into(), state: state.into(), ts };
        let mut clock = TurnClock::default();
        assert_eq!(clock.on(&event("a", "idle", 1.0)), None, "fim sem começo não conta");
        assert_eq!(clock.on(&event("a", "working", 10.0)), None);
        assert_eq!(clock.on(&event("a", "awaiting_input", 11.0)), None);
        assert_eq!(clock.on(&event("a", "working", 12.0)), None, "o mesmo turno segue do primeiro");
        assert_eq!(clock.on(&event("a", "idle", 12.5)), Some(Duration::from_millis(2_500)));
        assert_eq!(clock.on(&event("a", "idle", 13.0)), None, "um fim por turno");
    }
}
