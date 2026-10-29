//! Conferência do link de convite: só passa quando a página de fora é a desta máquina.
use axum::{Router, http::{StatusCode, header}, routing::get};
use hangar_server::share_reach::{Failure, InviteLink, Probe, check_once, check_until, parse_link};
use std::time::Duration;

async fn serve(router: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

fn doh_answer(ip: &'static str) -> Router {
    Router::new().route("/resolve", get(move || async move {
        ([(header::CONTENT_TYPE, "application/dns-json")],
         format!(r#"{{"Status":0,"Answer":[{{"name":"maq.tail1.ts.net.","type":1,"TTL":60,"data":"{ip}"}}]}}"#))
    }))
}

fn doh_empty() -> Router {
    Router::new().route("/resolve", get(|| async { ([(header::CONTENT_TYPE, "application/dns-json")], r#"{"Status":3}"#) }))
}

fn page(body: &'static str) -> Router {
    Router::new().route("/par/abc123", get(move || async move { body }))
}

async fn probe(doh: Vec<std::net::SocketAddr>, funnel: std::net::SocketAddr, local: std::net::SocketAddr) -> Probe {
    Probe {
        doh: doh.into_iter().map(|a| format!("http://{a}/resolve")).collect(),
        funnel_scheme: "http",
        funnel_port: funnel.port(),
        local_base: format!("http://{local}"),
    }
}

fn link() -> InviteLink { InviteLink { host: "maq.tail1.ts.net".into(), path: "/par/abc123".into() } }

#[test]
fn only_this_machine_links_are_accepted() {
    let own = "maq.tail1.ts.net";
    assert_eq!(parse_link("https://maq.tail1.ts.net:8443/par/abc123", own), Some(link()));
    assert_eq!(parse_link("https://maq.tail1.ts.net:8443/convite/Z9", own).unwrap().path, "/convite/Z9");
    for bad in [
        "https://outra.tail1.ts.net:8443/par/abc123",
        "https://maq.tail1.ts.net:443/par/abc123",
        "http://maq.tail1.ts.net:8443/par/abc123",
        "https://100.64.0.1:8443/par/abc123",
        "https://maq.tail1.ts.net:8443/api/sessions",
        "https://maq.tail1.ts.net:8443/par/abc-123",
        "https://maq.tail1.ts.net:8443/par/abc123?x=1",
        "https://user@maq.tail1.ts.net:8443/par/abc123",
    ] {
        assert_eq!(parse_link(bad, own), None, "{bad}");
    }
}

#[tokio::test]
async fn passes_when_outside_page_matches_local_page() {
    let doh = serve(doh_answer("127.0.0.1")).await;
    let funnel = serve(page("<h1>convite X</h1>")).await;
    let local = serve(page("<h1>convite X</h1>")).await;
    assert_eq!(check_once(&probe(vec![doh], funnel, local).await, &link()).await, Ok(()));
}

#[tokio::test]
async fn one_dns_provider_is_enough() {
    let dead = serve(doh_empty()).await;
    let doh = serve(doh_answer("127.0.0.1")).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    assert_eq!(check_once(&probe(vec![dead, doh], funnel, local).await, &link()).await, Ok(()));
}

#[tokio::test]
async fn no_public_dns_answer_fails_at_dns() {
    let dead = serve(doh_empty()).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    let got = check_once(&probe(vec![dead], funnel, local).await, &link()).await;
    assert!(matches!(got, Err(Failure::Dns { .. })), "{got:?}");
}

#[tokio::test]
async fn another_machine_answering_fails_at_funnel() {
    let doh = serve(doh_answer("127.0.0.1")).await;
    let funnel = serve(page("<h1>outra máquina</h1>")).await;
    let local = serve(page("<h1>convite X</h1>")).await;
    let got = check_once(&probe(vec![doh], funnel, local).await, &link()).await;
    assert!(matches!(got, Err(Failure::Funnel { .. })), "{got:?}");
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let doh = serve(doh_answer("127.0.0.1")).await;
    let funnel = serve(Router::new().route("/par/abc123", get(|| async {
        (StatusCode::FOUND, [(header::LOCATION, "/par/abc123/x")], "")
    }))).await;
    let local = serve(page("p")).await;
    let got = check_once(&probe(vec![doh], funnel, local).await, &link()).await;
    assert!(matches!(got, Err(Failure::Funnel { .. })), "{got:?}");
}

#[tokio::test]
async fn check_until_gives_up_at_the_deadline() {
    let dead = serve(doh_empty()).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    let p = probe(vec![dead], funnel, local).await;
    let got = check_until(&p, &link(), Duration::from_millis(20), Duration::from_millis(100)).await;
    assert!(matches!(got, Err(Failure::Dns { .. })), "{got:?}");
}

fn doh_answers(ips: &'static [&'static str]) -> Router {
    Router::new().route("/resolve", get(move || async move {
        let answer: Vec<String> = ips.iter()
            .map(|ip| format!(r#"{{"name":"maq.tail1.ts.net.","type":1,"TTL":60,"data":"{ip}"}}"#)).collect();
        ([(header::CONTENT_TYPE, "application/dns-json")], format!(r#"{{"Status":0,"Answer":[{}]}}"#, answer.join(",")))
    }))
}

#[tokio::test]
async fn any_public_address_that_delivers_the_page_is_enough() {
    // O primeiro endereço não tem ninguém na porta do Funnel; o segundo entrega a página.
    let doh = serve(doh_answers(&["::1", "127.0.0.1"])).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    assert_eq!(check_once(&probe(vec![doh], funnel, local).await, &link()).await, Ok(()));
}

#[tokio::test]
async fn check_until_answers_within_the_deadline_even_with_a_hung_provider() {
    let slow = serve(Router::new().route("/resolve", get(|| async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        ([(header::CONTENT_TYPE, "application/dns-json")], r#"{"Status":3}"#)
    }))).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    let p = probe(vec![slow], funnel, local).await;
    let started = std::time::Instant::now();
    let got = check_until(&p, &link(), Duration::from_millis(50), Duration::from_millis(200)).await;
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert!(matches!(got, Err(Failure::Dns { .. })), "{got:?}");
}

use hangar_server::share_reach::answer;

#[tokio::test]
async fn route_refuses_foreign_link_without_network() {
    // Probe apontando para portas fechadas: se a rota tentasse rede, o teste veria Dns/Funnel, não 400.
    let p = Probe { doh: vec!["http://127.0.0.1:9/resolve".into()], funnel_scheme: "http", funnel_port: 9, local_base: "http://127.0.0.1:9".into() };
    let (status, body) = answer(br#"{"link":"https://outra.tail1.ts.net:8443/par/abc"}"#, Some("maq.tail1.ts.net".into()), &p,
        Duration::from_millis(10), Duration::from_millis(20)).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"]["code"], "share_reach_link_refused");
}

#[tokio::test]
async fn route_without_tailscale_name_is_503() {
    let p = Probe::production();
    let (status, body) = answer(br#"{"link":"https://maq.tail1.ts.net:8443/par/abc"}"#, None, &p,
        Duration::from_millis(10), Duration::from_millis(20)).await;
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["detail"]["code"], "share_reach_no_tailscale");
}

#[tokio::test]
async fn route_reports_stage_on_failure_and_ok_on_success() {
    let dead = serve(doh_empty()).await;
    let doh = serve(doh_answer("127.0.0.1")).await;
    let funnel = serve(page("p")).await;
    let local = serve(page("p")).await;
    let body = br#"{"link":"https://maq.tail1.ts.net:8443/par/abc123"}"#;
    let (s, v) = answer(body, Some("maq.tail1.ts.net".into()), &probe(vec![dead], funnel, local).await,
        Duration::from_millis(10), Duration::from_millis(30)).await;
    assert_eq!((s, v["ok"].clone(), v["stage"].clone()), (axum::http::StatusCode::OK, false.into(), "dns".into()));
    // Cada chamada cabe no prazo restante: 30 ms cortaria a chamada numa máquina carregada.
    let (s, v) = answer(body, Some("maq.tail1.ts.net".into()), &probe(vec![doh], funnel, local).await,
        Duration::from_millis(10), Duration::from_secs(5)).await;
    assert_eq!((s, v), (axum::http::StatusCode::OK, serde_json::json!({"ok": true})));
}

#[tokio::test]
async fn route_answers_401_to_non_owner_in_rust() {
    // O Python não tem esta rota: repassar daria 404, que o cliente lê como "precisa do Rust".
    let (_fake, up) = crate::fake::spawn_fake().await;
    let srv = crate::fake::spawn_server(crate::fake::config(up, "127.0.0.1")).await;
    let r = crate::fake::client().post(format!("http://{srv}/api/share/reachability"))
        .bearer_auth("errado").body(r#"{"link":"https://maq.tail1.ts.net:8443/par/abc"}"#).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}));
}

#[tokio::test]
async fn retired_connect_bridge_is_not_available_with_any_secret() {
    use std::net::SocketAddr;
    let (_fake, up) = crate::fake::spawn_fake().await;
    let state = std::sync::Arc::new(hangar_server::routes::AppState::new(crate::fake::config(up, "")));
    let serve_app = |app: Router| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        addr
    };
    let private = serve_app(hangar_server::routes::terminal_router(state.clone())).await;
    let public = serve_app(hangar_server::routes::router(state)).await;
    let post = |addr: SocketAddr, secret: Option<&'static str>| {
        let mut req = crate::fake::client().post(format!("http://{addr}/__hangar_server/share/reachability")).body("não é json");
        if let Some(s) = secret { req = req.header("x-hangar-internal", s); }
        req.send()
    };
    assert_eq!(post(private, None).await.unwrap().status(), 404);
    assert_eq!(post(private, Some("outro")).await.unwrap().status(), 404);
    assert_eq!(post(public, Some(crate::fake::SECRET)).await.unwrap().status(), 404, "a porta pública não tem a ponte");
    let r = post(private, Some(crate::fake::SECRET)).await.unwrap();
    assert_eq!(r.status(), 404);
}
