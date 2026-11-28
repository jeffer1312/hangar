//! Ganchos da telemetria sem handler próprio: fim de turno, recurso usado pela API e o motor desta vida
//! para quem não recebe o estado do servidor. Contar, guardar e enviar é do crate `hangar-telemetry`.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use hangar_telemetry::{Collection, Telemetry};
use hangar_telemetry::events::{Dims, Event, OpenedBy, Provider, Surface};
use hangar_telemetry::speed::{Measure, Session, SpeedClock};
use hangar_usage_diary::OwnerAction;

use crate::list::bridge::ListBridge;
use crate::push::watch::TurnEvent;
use crate::routes::AppState;

static INSTALLED: OnceLock<Arc<Telemetry>> = OnceLock::new();

tokio::task_local! { pub(crate) static NO_TELEMETRY: (); }

/// O bearer sintético do MCP mantém as permissões da rota, não prova a autoria da ação.
#[derive(Clone, Copy)]
pub(crate) struct NoTelemetry;

pub(crate) fn eligible_request(req: &Request) -> bool {
    req.extensions().get::<NoTelemetry>().is_none() && !crate::telemetry_routes::guest(req)
}

/// O motor desta vida, só contando, para os ganchos que não recebem o estado do servidor (lista, aviso
/// tirado da conversa, sessão caída). Vazio até a subida ligar.
pub(crate) fn installed() -> Option<&'static Telemetry> { INSTALLED.get().map(Arc::as_ref).filter(|t| t.enabled()) }

/// Liga o motor desta vida aos ganchos, conta a saída da vida anterior e assina os fins de turno.
pub fn spawn(st: &Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    LazyLock::force(&ANCHOR);
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

/// O relógio e a época de consentimento em que ele começou: aceite novo zera o que ficou pendente.
static SPEED: LazyLock<Mutex<(u64, SpeedClock)>> = LazyLock::new(Default::default);
static ANCHOR: LazyLock<Instant> = LazyLock::new(Instant::now);
/// Desde quando o `Monitor` de cada sessão lê a tela: sinal dele só vale para o que aconteceu depois.
static WATCHERS: LazyLock<Mutex<HashMap<String, (Instant, u64)>>> = LazyLock::new(Default::default);

fn epoch_now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }
fn since_anchor(at: Instant) -> Duration { at.saturating_duration_since(*ANCHOR) }
/// A hora de parede gravada na fila vira o relógio monotônico pela idade dela agora: a suspensão da máquina não
/// anda o `Instant` e deixaria a âncora fixa no futuro.
fn epoch_to_anchor(epoch: f64, now: Duration) -> Duration {
    now.saturating_sub(Duration::try_from_secs_f64(epoch_now() - epoch).unwrap_or(Duration::ZERO))
}

fn speed(change: impl FnOnce(&mut SpeedClock, Duration) -> Option<Measure>) {
    let Some(telemetry) = installed() else { return };
    let epoch = telemetry.consent_epoch();
    let measure = {
        let mut guard = SPEED.lock().unwrap_or_else(|e| e.into_inner());
        if guard.0 != epoch { guard.1.clear(); guard.0 = epoch; }
        change(&mut guard.1, since_anchor(Instant::now()))
    };
    if let Some(m) = measure { telemetry.duration_in(epoch, m.event, m.dims, m.took); }
}

/// Marca do texto, só em memória: liga a linha confirmada na fila à mensagem que o dono mandou.
fn message_mark(text: &str) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(text.trim())
}

/// Abertura do dono, antes do lançamento, do `started` da abertura.
pub(crate) fn speed_opened(name: &str, provider: Provider, surface: Surface, opened_by: OpenedBy, first_send: bool, started: Instant) {
    speed(|clock, _| { clock.opened(name, Session { provider, surface }, opened_by, first_send, since_anchor(started)); None });
}

pub(crate) fn speed_forget(name: &str) { speed(|clock, _| { clock.forget(name); None }); }

/// Mensagem do dono pela API; comando (`/cost`) não tem resposta para medir.
pub(crate) fn speed_touch(name: &str, provider: Provider, surface: Surface, text: &str) {
    if text.trim_start().starts_with('/') { return; }
    speed(|clock, now| { clock.touch(name, Session { provider, surface }, message_mark(text), now); None });
}

/// O envio da mensagem foi recusado.
pub(crate) fn speed_untouch(name: &str, text: &str) { speed(|clock, _| { clock.untouch(name, message_mark(text)); None }); }

/// Barato e sem o motor: quem paga para juntar o sinal (fila) pergunta antes.
pub(crate) fn speed_knows(name: &str) -> bool {
    installed().is_some() && SPEED.lock().unwrap_or_else(|e| e.into_inner()).1.knows(name)
}

/// Quem paga para ler a tela pergunta antes.
pub(crate) fn speed_awaits_ready(name: &str) -> bool {
    installed().is_some() && SPEED.lock().unwrap_or_else(|e| e.into_inner()).1.awaits_ready(name)
}

/// Pronta vista por quem observa a sessão desde sempre (ator do runtime, captura dos outros com terminal).
pub(crate) fn speed_ready(name: &str) { speed(|clock, now| clock.ready(name, Duration::ZERO, now)); }

/// Linha da fila confirmada no agente, despachada a ele em `sent` (hora de parede da tentativa, `row_clock`): a espera
/// na fila com o agente ocupado não entra no `first_reply`. `text` só decide se é recado
/// de outra sessão ou comando e não sai daqui.
pub(crate) fn speed_delivered(name: &str, text: &str, steered: bool, sent: Option<f64>) {
    if text.trim_start().starts_with('/') { return; }
    speed(|clock, now| {
        let sent = sent.map_or(now, |epoch| epoch_to_anchor(epoch, now));
        clock.delivered(name, message_mark(text), crate::session_write::input::is_relay(text), steered, sent, now)
    });
}

/// Primeiro texto da resposta visto pelo ator do runtime.
pub(crate) fn speed_replied(name: &str) { speed(|clock, now| clock.replied(name, Duration::ZERO, now)); }

/// O `Monitor` `owner` da sessão nasceu (só existe com a conversa aberta num app).
pub(crate) fn speed_watch(name: &str, owner: u64) {
    WATCHERS.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), (Instant::now(), owner));
}

/// O `Monitor` acabou; o que já o substituiu fica.
pub(crate) fn speed_unwatch(name: &str, owner: u64) {
    let mut watchers = WATCHERS.lock().unwrap_or_else(|e| e.into_inner());
    if watchers.get(name).is_some_and(|(_, current)| *current == owner) { watchers.remove(name); }
}

fn watched_since(name: &str) -> Option<Duration> {
    WATCHERS.lock().unwrap_or_else(|e| e.into_inner()).get(name).map(|(since, _)| since_anchor(*since))
}

/// Pronta e primeiro texto vistos pelo `Monitor`: só valem para o que aconteceu depois de ele começar a ler.
pub(crate) fn speed_ready_seen(name: &str) {
    let Some(since) = watched_since(name) else { return };
    speed(|clock, now| clock.ready(name, since, now));
}

pub(crate) fn speed_replied_seen(name: &str) {
    let Some(since) = watched_since(name) else { return };
    speed(|clock, now| clock.replied(name, since, now));
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
    let Some(telemetry) = st.counting() else { return next.run(req).await };
    let epoch = telemetry.consent_epoch();
    let feature = req.extensions().get::<MatchedPath>()
        .and_then(|path| hangar_telemetry::events::feature(req.method().as_str(), path.as_str()))
        .filter(|_| eligible_request(&req));
    let (Some(feature), Some(period)) = (feature, telemetry.collection()) else { return next.run(req).await };
    let auth = req.extensions().get::<hangar_core::edge::RequestAuth>().cloned();
    let marker = req.extensions().get::<crate::routes::NotOwner>().cloned();
    let start = std::time::Instant::now();
    let response = next.run(req).await;
    if marker.is_some_and(|marker| marker.seen()) || telemetry.consent_epoch() != epoch || !telemetry.enabled()
        || period.since > chrono::Utc::now() { return response; }
    if let Some(auth) = auth && auth.owner_verified().await {
        let status = response.status();
        if status.is_success() {
            telemetry.count_in(epoch, Event::FeatureUsed, Dims::new().with("feature", feature), 1);
            record_action(&st, period, OwnerAction::Http, feature.as_str(), "request", None, start.elapsed()).await;
        } else if status.is_client_error() || status.is_server_error() {
            let failure = response.extensions().get::<crate::diag::FailureCode>().map(|code| code.0)
                .filter(|code| hangar_telemetry::diary::known_failure(code));
            let fallback = format!("http_{}", status.as_u16());
            record_action(&st, period, OwnerAction::Http, feature.as_str(), "request", Some(failure.unwrap_or(&fallback)), start.elapsed()).await;
        }
    }
    response
}

pub(crate) async fn record_action(st: &AppState, period: Collection, origin: OwnerAction, action: &str,
                                  stage: &'static str, code: Option<&str>, took: Duration) {
    if st.counting().and_then(Telemetry::collection).is_none_or(|current| current.id != period.id) { return; }
    let Some(base) = crate::diag::diary_base(st) else { return };
    let now = chrono::Local::now().fixed_offset();
    let (action, code) = (action.to_owned(), code.map(str::to_owned));
    let ms = took.as_millis().min(3_600_000) as u64;
    match tokio::task::spawn_blocking(move || hangar_usage_diary::record_owner_action(
        &base, origin, &period.id, now, &action, stage, code.as_deref(), ms)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(code = "telemetry_diary_write_failed", ?error, "falha ao gravar ação no diário da telemetria"),
        Err(_) => crate::telemetry_routes::task_failed(&st.diag),
    }
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
            .route("/api/sessions/{name}/navegador", get(|State(st): State<Arc<AppState>>, req: Request| async move {
                let owner = crate::routes::gate(&st, "127.0.0.1:5000".parse().unwrap(), &req);
                drop(req);
                assert!(matches!(owner.await, Ok((_, true))));
                StatusCode::OK
            }))
            .route("/api/sessions/{name}/history", get(|| async { StatusCode::OK }))
            .route("/api/tags", put(|| async { StatusCode::FORBIDDEN }))
            .layer(axum::middleware::from_fn_with_state(st.clone(), count_feature))
            .with_state(st)
    }

    fn request(method: &str, uri: &str) -> Request {
        let mut req = axum::http::Request::builder().method(method).uri(uri).header("authorization", "Bearer owner").body(Body::empty()).unwrap();
        req.extensions_mut().insert(hangar_core::edge::RequestAuth::default());
        req
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
